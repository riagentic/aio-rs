//! `ax dev`'s hot-patching: a "fat" build of the app (every
//! dependency's code kept, every workspace crate's rustc call recorded),
//! then per edit a "thin" build: the edited crates' rustc calls replayed,
//! the app's crate linked into a small shared library whose missing symbols
//! jump into the running fat binary, and a jump table (old address → new)
//! for `aio_hotpatch::apply_patch` in the app.
//! Ported from dioxus-cli 0.7.10 (MIT/Apache-2.0): build/{link,patch,request}.rs, rustcwrapper.rs.
//!
//! ax takes two roles in the fat build, chosen by env var at the top of
//! `main` ([`hook`]): cargo's `RUSTC_WORKSPACE_WRAPPER` (records each
//! workspace crate's rustc args, env and folder, then runs rustc) and the
//! app crate's linker (records the link args and writes an empty object
//! where the binary goes: the real link is [`fat_build`]'s, with every
//! dependency object in it).
//!
//! Linux (ELF, x86_64 and aarch64), macOS (Mach-O, arm64 and x86_64) and
//! Windows (PE/COFF, x86_64; MSVC and GNU toolchains). On Windows the patch
//! is a DLL linked at a fixed base the app names, near the fat binary, and
//! the fat binary has a COFF symbol table (no PDB reading): see
//! [`rewrite_coff`] for how a patch reaches the running binary's data.
pub use aio_hotpatch::JumpTable;
use object::write::{MachOBuildVersion, SectionId, StandardSection, Symbol, SymbolId, SymbolSection};
use object::{Object, ObjectSection, ObjectSymbol, SymbolFlags, SymbolKind, SymbolScope};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Set by [`fat_build`] on cargo: ax was run as rustc's workspace wrapper;
/// each crate's call is recorded in this folder.
const RUSTC_ENV: &str = "AX_HOTPATCH_RUSTC";
/// Set on rustc for the app crate: ax was run as its linker; the link args
/// go to this file and no binary is linked.
const LINK_ENV: &str = "AX_HOTPATCH_LINK";
/// Not recorded: the wrapper's own, and cargo's jobserver, gone by replay time.
const UNRECORDED: [&str; 7] =
    [RUSTC_ENV, LINK_ENV, "RUSTC_WORKSPACE_WRAPPER", "RUSTC_WRAPPER", "CARGO_MAKEFLAGS", "MAKEFLAGS", "MFLAGS"];

/// ax as the fat build's rustc wrapper or linker, when one of its env vars
/// says so: the exit code to end with. `None`: ax as itself.
pub fn hook() -> Option<ExitCode> {
    if let Some(dir) = std::env::var_os(RUSTC_ENV) {
        return Some(wrap_rustc(Path::new(&dir)));
    }
    let file = std::env::var_os(LINK_ENV)?;
    Some(match record_link(Path::new(&file)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ax: hot-patch link: {e}");
            ExitCode::FAILURE
        }
    })
}

/// One crate's rustc call: args (the first is rustc), env and folder.
#[derive(Clone, Debug)]
struct Rustc {
    args: Vec<String>,
    envs: Vec<(String, String)>,
    cwd: PathBuf,
}

impl Rustc {
    fn to_json(&self) -> String {
        serde_json::json!({ "args": self.args, "envs": self.envs, "cwd": self.cwd }).to_string()
    }

    fn from_json(text: &str) -> Option<Rustc> {
        let v: serde_json::Value = serde_json::from_str(text).ok()?;
        let strings = |v: &serde_json::Value| -> Option<Vec<String>> {
            v.as_array()?.iter().map(|s| s.as_str().map(String::from)).collect()
        };
        let envs = v["envs"].as_array()?.iter().map(|p| Some((p[0].as_str()?.into(), p[1].as_str()?.into())));
        Some(Rustc { args: strings(&v["args"])?, envs: envs.collect::<Option<_>>()?, cwd: v["cwd"].as_str()?.into() })
    }

    /// The value after flag `name` (`--crate-name x`).
    fn flag(&self, name: &str) -> Option<&str> {
        self.args.iter().position(|a| a == name).and_then(|i| self.args.get(i + 1)).map(String::as_str)
    }

    /// The last `-C key=value`'s value, in either spelling (`-Ckey=v`, `-C key=v`).
    fn codegen(&self, key: &str) -> Option<&str> {
        let prefix = format!("{key}=");
        let joined = format!("-C{prefix}");
        let mut found = None;
        for (i, a) in self.args.iter().enumerate() {
            if let Some(v) = a.strip_prefix(&joined) {
                found = Some(v);
            } else if a == "-C"
                && let Some(v) = self.args.get(i + 1).and_then(|n| n.strip_prefix(&prefix))
            {
                found = Some(v);
            }
        }
        found
    }

    fn name(&self) -> &str {
        self.flag("--crate-name").unwrap_or_default()
    }

    /// `lib<name><extra>`: a library's output files, `.rlib` and `.rmeta`.
    fn lib_stem(&self) -> String {
        format!("lib{}{}", self.name(), self.codegen("extra-filename").unwrap_or_default())
    }

    fn out_dir(&self) -> Option<PathBuf> {
        self.flag("--out-dir").map(|d| self.cwd.join(d))
    }

    fn rlib(&self) -> Option<PathBuf> {
        Some(self.out_dir()?.join(format!("{}.rlib", self.lib_stem())))
    }

    /// The source files rustc read for this crate, from its dep-info file.
    fn sources(&self) -> Vec<PathBuf> {
        let Some(dir) = self.out_dir() else { return vec![] };
        let d = dir.join(format!("{}{}.d", self.name(), self.codegen("extra-filename").unwrap_or_default()));
        std::fs::read_to_string(d).map_or_else(|_| vec![], |text| dep_info(&text, &self.cwd))
    }

    /// The `lib<name><extra>` stems of the libraries it is given (`--extern`).
    fn externs(&self) -> HashSet<String> {
        let paths = self.args.windows(2).filter(|w| w[0] == "--extern").filter_map(|w| w[1].split_once('='));
        paths.filter_map(|(_, p)| Some(Path::new(p).file_stem()?.to_string_lossy().into_owned())).collect()
    }

    /// Run it again as recorded: its outputs rewritten in place. `link`:
    /// the file the app crate's link args go to.
    fn replay(&self, link: Option<&Path>) -> Result<(), String> {
        let mut cmd = Command::new(&self.args[0]);
        cmd.args(&self.args[1..]).current_dir(&self.cwd).env_clear().envs(self.envs.iter().cloned());
        if let Some(file) = link {
            cmd.env(LINK_ENV, file);
        }
        let out = cmd.output().map_err(|e| format!("hot-patch: can't run {}: {e}", self.args[0]))?;
        match out.status.success() {
            true => Ok(()),
            false => {
                Err(format!("hot-patch: {} failed:\n{}", self.name(), rendered(&String::from_utf8_lossy(&out.stderr))))
            }
        }
    }
}

/// The dependencies a dep-info file lists (each on its own `path:` line),
/// relative ones in `cwd` (pure but for canonical paths).
fn dep_info(text: &str, cwd: &Path) -> Vec<PathBuf> {
    let deps = text.lines().filter(|l| !l.starts_with('#')).filter_map(|l| l.strip_suffix(':'));
    deps.map(|p| canonical(&cwd.join(p.replace("\\ ", " ")))).collect()
}

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()))
}

/// rustc's JSON diagnostics as it renders them; other lines as they are (pure).
fn rendered(stderr: &str) -> String {
    let line = |l: &str| match serde_json::from_str::<serde_json::Value>(l) {
        Ok(v) => v["rendered"].as_str().map(String::from),
        Err(_) => Some(format!("{l}\n")),
    };
    stderr.lines().filter_map(line).collect()
}

/// As cargo's workspace wrapper: record the call (not for build scripts
/// or rustc's probes), then become rustc.
fn wrap_rustc(dir: &Path) -> ExitCode {
    let raw: Vec<OsString> = std::env::args_os().skip(1).collect();
    let Some((rustc, rest)) = raw.split_first() else {
        eprintln!("ax: hot-patch: run as a rustc wrapper without rustc");
        return ExitCode::FAILURE;
    };
    let call = Rustc {
        args: raw.iter().map(|a| a.to_string_lossy().into_owned()).collect(),
        envs: std::env::vars_os()
            .filter(|(k, _)| !UNRECORDED.iter().any(|u| k == u))
            .map(|(k, v)| (k.to_string_lossy().into_owned(), v.to_string_lossy().into_owned()))
            .collect(),
        cwd: std::env::current_dir().unwrap_or_default(),
    };
    let name = call.name();
    if !name.is_empty() && name != "___" && !name.starts_with("build_script_") {
        let types: Vec<&str> = call.args.windows(2).filter(|w| w[0] == "--crate-type").map(|w| w[1].as_str()).collect();
        let kind = if types.iter().any(|t| matches!(*t, "lib" | "rlib")) {
            "lib"
        } else {
            types.first().copied().unwrap_or("lib")
        };
        if let Err(e) = write_new(&dir.join(format!("{name}.{kind}.json")), call.to_json().as_bytes()) {
            eprintln!("ax: hot-patch: can't record {name}: {e}");
            return ExitCode::FAILURE;
        }
    }
    let mut cmd = Command::new(rustc);
    cmd.args(rest).env_remove(RUSTC_ENV);
    let e = become_(cmd);
    eprintln!("ax: hot-patch: can't run {}: {e}", rustc.to_string_lossy());
    ExitCode::FAILURE
}

/// Unix: this process becomes `cmd` (returns only if it can't).
#[cfg(unix)]
fn become_(mut cmd: Command) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    cmd.exec()
}

/// Elsewhere: `cmd` runs, and this process ends as it did.
#[cfg(not(unix))]
fn become_(mut cmd: Command) -> std::io::Error {
    match cmd.status() {
        Ok(s) => std::process::exit(s.code().unwrap_or(1)),
        Err(e) => e,
    }
}

/// Write `file` whole or not at all: a reader never sees half of it.
fn write_new(file: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = file.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, file)
}

/// As the app crate's linker: its args (response files read) and the env
/// rustc ran it with (MSVC's `LIB`, the bundled MinGW's `PATH`) to `file`,
/// and an empty object where the binary goes, for rustc to find.
fn record_link(file: &Path) -> Result<(), String> {
    let mut args = vec![];
    for a in std::env::args_os().skip(1) {
        let a = a.to_string_lossy().into_owned();
        match a.strip_prefix('@') {
            Some(rsp) => args.extend(response_file(&std::fs::read(rsp).map_err(|e| format!("{rsp}: {e}"))?)),
            None => args.push(a),
        }
    }
    let link = Link { args, envs: recorded_env() };
    write_new(file, link.to_json().as_bytes()).map_err(|e| format!("{}: {e}", file.display()))?;
    let out = output_arg(&link.args).ok_or("no output (-o, /OUT:) in the link args")?;
    let (format, arch) = host()?;
    let empty = object::write::Object::new(format, arch, object::Endianness::Little);
    write_new(Path::new(out), &empty.write().map_err(|e| e.to_string())?).map_err(|e| format!("{out}: {e}"))
}

/// This process's env, but for what is gone by replay time ([`UNRECORDED`]).
fn recorded_env() -> Vec<(String, String)> {
    std::env::vars_os()
        .filter(|(k, _)| !UNRECORDED.iter().any(|u| k == u))
        .map(|(k, v)| (k.to_string_lossy().into_owned(), v.to_string_lossy().into_owned()))
        .collect()
}

/// A recorded link: its args and env.
struct Link {
    args: Vec<String>,
    envs: Vec<(String, String)>,
}

impl Link {
    fn to_json(&self) -> String {
        serde_json::json!({ "args": self.args, "envs": self.envs }).to_string()
    }
}

/// The output a link names: `-o <file>`, or link.exe's `/OUT:<file>` (pure).
fn output_arg(args: &[String]) -> Option<&str> {
    let gnu = args.iter().position(|a| a == "-o").and_then(|i| args.get(i + 1)).map(String::as_str);
    gnu.or_else(|| args.iter().rev().find_map(|a| msvc_value(a, "/OUT:")))
}

/// The value of link.exe option `opt` (`/OUT:`) in `arg`, in any case (pure).
fn msvc_value<'a>(arg: &'a str, opt: &str) -> Option<&'a str> {
    let head = arg.get(..opt.len())?;
    head.eq_ignore_ascii_case(opt).then(|| &arg[opt.len()..])
}

/// A linker response file's args: rustc writes link.exe's as UTF-16 with a
/// byte-order mark, one quoted arg a line; a GNU linker's as GNU's (pure).
fn response_file(bytes: &[u8]) -> Vec<String> {
    match bytes.strip_prefix(&[0xFF, 0xFE]) {
        Some(wide) => {
            let units: Vec<u16> = wide.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
            msvc_response_args(&String::from_utf16_lossy(&units))
        }
        None => response_args(&String::from_utf8_lossy(bytes)),
    }
}

/// link.exe's response file as rustc writes it: an arg a line, in quotes,
/// a quote in it as `\"` (pure).
fn msvc_response_args(text: &str) -> Vec<String> {
    let lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    lines.map(|l| l.strip_prefix('"').and_then(|l| l.strip_suffix('"')).unwrap_or(l).replace("\\\"", "\"")).collect()
}

/// An arg for an lld-link response file, quoted as Windows' command line
/// reads it: backslashes before a quote (and the closing one) doubled (pure).
fn msvc_response_arg(arg: &str) -> String {
    let (mut s, mut slashes) = (String::from('"'), 0);
    for c in arg.chars() {
        match c {
            '\\' => slashes += 1,
            '"' => {
                s.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
                slashes = 0;
            }
            _ => {
                s.extend(std::iter::repeat_n('\\', slashes));
                slashes = 0;
            }
        }
        if c != '\\' {
            s.push(c);
        }
    }
    s.extend(std::iter::repeat_n('\\', slashes * 2));
    s.push('"');
    s
}

/// A GNU-style response file's args: split on whitespace, quotes group,
/// a backslash takes the next character as it is (pure).
fn response_args(text: &str) -> Vec<String> {
    let (mut args, mut cur, mut quote, mut any) = (vec![], String::new(), None, false);
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (c, quote) {
            ('\\', _) => {
                cur.extend(chars.next());
                any = true;
            }
            (q, None) if q == '"' || q == '\'' => (quote, any) = (Some(q), true),
            (q, Some(open)) if q == open => quote = None,
            (w, None) if w.is_whitespace() => {
                if any {
                    args.push(std::mem::take(&mut cur));
                }
                any = false;
            }
            (c, _) => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        args.push(cur);
    }
    args
}

/// An arg for a GNU-style response file (pure).
fn response_arg(arg: &str) -> String {
    let mut s = String::with_capacity(arg.len());
    for c in arg.chars() {
        if c == '\\' || c == '"' || c == '\'' || c.is_whitespace() {
            s.push('\\');
        }
        s.push(c);
    }
    s
}

/// This machine's object format and architecture: what patches are built for.
fn host() -> Result<(object::BinaryFormat, object::Architecture), String> {
    let format = match std::env::consts::OS {
        "linux" => object::BinaryFormat::Elf,
        "macos" => object::BinaryFormat::MachO,
        "windows" => object::BinaryFormat::Coff,
        other => return Err(format!("hot-patch: not supported on {other} yet")),
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => object::Architecture::X86_64,
        "aarch64" if !COFF => object::Architecture::Aarch64,
        other => return Err(format!("hot-patch: not supported on {} {other} yet", std::env::consts::OS)),
    };
    Ok((format, arch))
}

const MACHO: bool = cfg!(target_os = "macos");
/// Windows: PE executables, COFF objects; the patch a DLL ([`rewrite_coff`]).
const COFF: bool = cfg!(windows);
/// Windows: the fat binary's base (1 GiB): an executable is mapped first, so
/// it is free; low enough for 32-bit absolute symbols, with room above.
const FAT_BASE: u64 = 0x4000_0000;
/// Where the app may load a patch DLL (`Aio.aslr`'s `base`) must have this
/// much room free: far more than a patch takes (checked once linked).
pub const PATCH_ROOM: u64 = 256 << 20;
/// The stub's symbol for the binary's copy of a writable static (COFF):
/// `aio$fat$<name>` ([`rewrite_coff`]).
const FAT_PREFIX: &str = "aio$fat$";

/// The symbol at the same place in the fat binary and each patch: `main`
/// (`aio_hotpatch::aslr_reference`), `_main` in Mach-O's symbol table.
const MAIN: &str = if MACHO { "_main" } else { "main" };

/// What to build: the app's binary, as `ax dev` builds it.
pub struct Request {
    /// The folder cargo runs in (the app's).
    pub dir: PathBuf,
    pub package: String,
    pub bin: String,
    /// cargo's: the layout (`dev::layout()`), features, `--offline`.
    pub cargo_args: Vec<String>,
    pub envs: Vec<(String, String)>,
    /// This ax (`std::env::current_exe()`): the wrapper and linker.
    pub ax: PathBuf,
}

/// A fat build: the binary to run and what each patch is made from.
pub struct Fat {
    pub exe: PathBuf,
    /// cargo's time, then the fat link's.
    pub build: Duration,
    pub link: Duration,
    scope: PathBuf,
    tip: String,
    /// Each workspace crate the binary is built from, by `<name>.<lib|bin|proc-macro>`.
    crates: BTreeMap<String, Rustc>,
    linker: String,
    /// Windows with MSVC: `linker` is rust-lld, in link.exe's flavor.
    msvc: bool,
    symbols: Symbols,
    /// Libraries replayed since the fat build: every patch links them all
    /// (each patch replaces the fat binary's code, not the last patch's).
    modified: BTreeSet<String>,
    patches: u32,
}

/// A patch's times: rustc (the edited crates, then the app's), the stub
/// for its missing symbols, the link; the total with the jump table.
#[derive(Clone, Copy, Debug)]
pub struct Times {
    pub compile: Duration,
    pub stub: Duration,
    pub link: Duration,
    pub total: Duration,
}

pub struct Patch {
    /// For the app: `aio_hotpatch::apply_patch`.
    pub table: JumpTable,
    pub times: Times,
}

/// Build the app fat: in ax dev's layout (`req.cargo_args`), every
/// workspace crate's rustc call recorded, then linked with every object of
/// every dependency built here, `main` exported. The binary is cargo's.
pub fn fat_build(req: &Request) -> Result<Fat, String> {
    host()?;
    let started = Instant::now();
    let msvc = COFF && target_triple(req)?.ends_with("-msvc");
    let meta = metadata(req)?;
    let target = PathBuf::from(meta["target_directory"].as_str().ok_or("hot-patch: cargo metadata: no target")?);
    let tip = req.bin.replace('-', "_");
    let scope = target.join("aio").join("hotpatch").join(format!("{}-{:016x}", req.bin, scope_hash(req)));
    let rustc_dir = scope.join("rustc");
    let link_file = scope.join("fat-link.json");
    std::fs::create_dir_all(&rustc_dir).map_err(|e| format!("hot-patch: {}: {e}", rustc_dir.display()))?;
    remove_patches(&scope, &tip);
    // Libraries of the workspace whose calls aren't recorded yet (a fresh
    // build doesn't run rustc): rebuilt, and so recorded.
    let unrecorded: Vec<String> = workspace_libs(&meta, &req.package)
        .into_iter()
        .filter(|l| !rustc_dir.join(format!("{l}.lib.json")).is_file())
        .collect();
    // The app crate is always rebuilt: its link args are the fat link's.
    let previous = std::fs::read_to_string(rustc_dir.join(format!("{tip}.bin.json"))).ok();
    if let Some(dir) = previous.as_deref().and_then(Rustc::from_json).and_then(|r| r.out_dir()) {
        bust(&dir.with_file_name(".fingerprint"), &req.bin, &unrecorded);
    }
    let _ = std::fs::remove_file(&link_file);
    let mut exe = cargo(req, &rustc_dir, &link_file, msvc)?;
    if !link_file.is_file() {
        bust(&exe.with_file_name(".fingerprint"), &req.bin, &unrecorded);
        exe = cargo(req, &rustc_dir, &link_file, msvc)?;
    }
    let fat_link_rec = read_link(&link_file)?;
    let build = started.elapsed();
    let crates = recorded(&rustc_dir, &tip, &fat_link_rec.args);
    let app = crates.get(&format!("{tip}.bin")).ok_or_else(|| format!("hot-patch: no rustc call of {tip} recorded"))?;
    let linker = if msvc { rust_lld(app)? } else { linker(app, &req.ax) };
    let started = Instant::now();
    fat_link(&fat_link_rec, app, &linker, msvc, &exe, &target)?;
    let link = started.elapsed();
    let symbols = Symbols::of(&exe)?;
    Ok(Fat { exe, build, link, scope, tip, crates, linker, msvc, symbols, modified: BTreeSet::new(), patches: 0 })
}

/// The target the app is built for: `--target`'s in its cargo args, else rustc's host.
fn target_triple(req: &Request) -> Result<String, String> {
    if let Some(t) = req.cargo_args.iter().position(|a| a == "--target").and_then(|i| req.cargo_args.get(i + 1)) {
        return Ok(t.clone());
    }
    let out = Command::new("rustc").arg("-vV").envs(req.envs.iter().cloned()).output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.lines()
        .find_map(|l| l.strip_prefix("host: "))
        .map(String::from)
        .ok_or_else(|| "hot-patch: rustc -vV: no host".into())
}

/// MSVC: the toolchain's own lld (`rust-lld -flavor link`, as dx): it
/// writes the binary's COFF symbol table beside its PDB (`/DEBUG:SYMTAB`),
/// which link.exe can't.
fn rust_lld(app: &Rustc) -> Result<String, String> {
    let out = Command::new(&app.args[0])
        .args(["--print", "sysroot"])
        .env_clear()
        .envs(app.envs.iter().cloned())
        .output()
        .map_err(|e| format!("hot-patch: can't run {}: {e}", app.args[0]))?;
    let sysroot = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let target = app.flag("--target").unwrap_or("x86_64-pc-windows-msvc");
    let lld = Path::new(&sysroot).join("lib/rustlib").join(target).join("bin/rust-lld.exe");
    match lld.is_file() {
        true => Ok(lld.display().to_string()),
        false => Err(format!(
            "hot-patch: no {} (rustup component llvm-tools isn't it: rust-lld ships with rustc)",
            lld.display()
        )),
    }
}

/// The workspace's packages (`cargo metadata --no-deps`).
fn metadata(req: &Request) -> Result<serde_json::Value, String> {
    let out = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(&req.dir)
        .envs(req.envs.iter().cloned())
        .output()
        .map_err(|e| format!("hot-patch: can't run cargo: {e}"))?;
    if !out.status.success() {
        return Err(format!("hot-patch: cargo metadata: {}", String::from_utf8_lossy(&out.stderr)));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("hot-patch: cargo metadata: {e}"))
}

/// The crate names of the libraries of `package` and of the workspace
/// packages it is built on (path dependencies), from `cargo metadata` (pure).
fn workspace_libs(meta: &serde_json::Value, package: &str) -> Vec<String> {
    let pkgs: Vec<&serde_json::Value> = meta["packages"].as_array().into_iter().flatten().collect();
    let dir = |p: &serde_json::Value| {
        Path::new(p["manifest_path"].as_str().unwrap_or_default()).parent().map(Path::to_path_buf)
    };
    let mut todo: Vec<&serde_json::Value> = pkgs.iter().copied().filter(|p| p["name"] == package).collect();
    let mut seen: Vec<&serde_json::Value> = vec![];
    while let Some(p) = todo.pop() {
        if seen.iter().any(|s| std::ptr::eq(*s, p)) {
            continue;
        }
        seen.push(p);
        let deps = p["dependencies"].as_array().into_iter().flatten().filter(|d| d["kind"] != "dev");
        for path in deps.filter_map(|d| d["path"].as_str()) {
            todo.extend(pkgs.iter().copied().filter(|q| dir(q).as_deref() == Some(Path::new(path))));
        }
    }
    let targets = seen.into_iter().flat_map(|p| p["targets"].as_array().into_iter().flatten());
    let libs = targets.filter(|t| t["kind"].as_array().is_some_and(|k| k.iter().any(|k| k == "lib" || k == "rlib")));
    libs.filter_map(|t| t["name"].as_str().map(|n| n.replace('-', "_"))).collect()
}

/// Its own folder per app and build settings: a build with other features
/// has other artifacts, and other recorded calls.
fn scope_hash(req: &Request) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::hash::DefaultHasher::new();
    (&req.package, &req.bin, &req.cargo_args, &req.ax).hash(&mut h);
    h.finish()
}

/// Earlier sessions' patch libraries of `tip`.
fn remove_patches(scope: &Path, tip: &str) {
    let prefix = format!("lib{tip}-patch-");
    for e in std::fs::read_dir(scope).into_iter().flatten().flatten() {
        if e.file_name().to_string_lossy().starts_with(&prefix) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Make cargo rebuild binary `bin` and libraries `libs`: their fingerprints
/// (in `fingerprints`, a profile's `.fingerprint`) removed.
fn bust(fingerprints: &Path, bin: &str, libs: &[String]) {
    let mut marks = vec![format!("bin-{bin}")];
    marks.extend(libs.iter().map(|l| format!("lib-{l}")));
    for e in std::fs::read_dir(fingerprints).into_iter().flatten().flatten() {
        if marks.iter().any(|m| e.path().join(m).is_file()) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// Whether MinGW's gcc is on `PATH` (rustc's test for a system MinGW).
fn mingw_on_path() -> bool {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path).any(|d| d.join("x86_64-w64-mingw32-gcc.exe").is_file())
}

/// The fat build's cargo: ax as rustc's wrapper and the app crate's
/// linker, the app crate's objects kept (`-Csave-temps`) and none of its
/// code dropped (`-Clink-dead-code`). Windows GNU without a MinGW on
/// `PATH`: rustc's bundled one (`-Clink-self-contained`, as rustc itself
/// would choose; with ax as the linker it can't tell). The binary, as cargo
/// says it wrote it.
fn cargo(req: &Request, rustc_dir: &Path, link_file: &Path, msvc: bool) -> Result<PathBuf, String> {
    let mut child = Command::new("cargo")
        .args(["rustc", "-p", &req.package, "--bin", &req.bin, "--message-format=json-render-diagnostics"])
        .args(&req.cargo_args)
        .args(["--", "-Csave-temps=true", "-Clink-dead-code"])
        .args((COFF && !msvc && !mingw_on_path()).then_some("-Clink-self-contained=yes"))
        .arg(format!("-Clinker={}", req.ax.display()))
        .current_dir(&req.dir)
        .envs(req.envs.iter().cloned())
        .env("RUSTC_WORKSPACE_WRAPPER", &req.ax)
        .env(RUSTC_ENV, rustc_dir)
        .env(LINK_ENV, link_file)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("hot-patch: can't run cargo: {e}"))?;
    let stdout = child.stdout.take().map(|mut o| {
        std::thread::spawn(move || {
            let mut s = String::new();
            let _ = std::io::Read::read_to_string(&mut o, &mut s);
            s
        })
    });
    // cargo's progress and the compiler's messages, as they come.
    let mut said = String::new();
    if let Some(err) = child.stderr.take() {
        for line in std::io::BufRead::lines(std::io::BufReader::new(err)).map_while(Result::ok) {
            eprintln!("{line}");
            said.push_str(&line);
            said.push('\n');
        }
    }
    let messages = stdout.and_then(|t| t.join().ok()).unwrap_or_default();
    let status = child.wait().map_err(|e| format!("hot-patch: cargo: {e}"))?;
    if !status.success() {
        return Err(format!("hot-patch: the build failed:\n{said}"));
    }
    let artifacts = messages.lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok());
    artifacts
        .filter(|m| m["reason"] == "compiler-artifact" && m["target"]["name"] == req.bin.as_str())
        .find_map(|m| m["executable"].as_str().map(PathBuf::from))
        .ok_or_else(|| format!("hot-patch: cargo built no binary {}", req.bin))
}

fn read_link(file: &Path) -> Result<Link, String> {
    let fail = |e: &dyn std::fmt::Display| format!("hot-patch: no link args ({}): {e}", file.display());
    let text = std::fs::read_to_string(file).map_err(|e| fail(&e))?;
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| fail(&e))?;
    let strings = |v: &serde_json::Value| v.as_str().map(String::from);
    let args = v["args"].as_array().ok_or_else(|| fail(&"no args"))?.iter().filter_map(strings).collect();
    let pairs = v["envs"].as_array().into_iter().flatten();
    let envs = pairs.filter_map(|p| Some((strings(&p[0])?, strings(&p[1])?))).collect();
    Ok(Link { args, envs })
}

/// The recorded calls of the crates the binary is built from: its own,
/// the libraries its link takes, the proc macros (pure but for reading).
fn recorded(dir: &Path, tip: &str, link_args: &[String]) -> BTreeMap<String, Rustc> {
    let linked: HashSet<&str> =
        link_args.iter().filter(|a| a.ends_with(".rlib")).filter_map(|a| Path::new(a).file_stem()?.to_str()).collect();
    let mut crates = BTreeMap::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let file = e.path();
        let Some(key) = file.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".json")) else {
            continue;
        };
        let Some(call) = std::fs::read_to_string(&file).ok().as_deref().and_then(Rustc::from_json) else { continue };
        let keep = match key.rsplit_once('.') {
            Some((name, "bin")) => name == tip,
            Some((_, "lib")) => linked.contains(call.lib_stem().as_str()),
            Some((_, "proc-macro")) => true,
            _ => false,
        };
        if keep {
            crates.insert(key.to_string(), call);
        }
    }
    crates
}

/// The linker rustc would have run for the app crate: the one the build
/// settings name, else `cc` (MinGW's gcc on Windows, which rustc finds in
/// its own `self-contained` folder: [`cargo`] asks for that) (pure).
fn linker(app: &Rustc, ax: &Path) -> String {
    let mut found = None;
    for (i, a) in app.args.iter().enumerate() {
        let v = a
            .strip_prefix("-Clinker=")
            .or_else(|| (a == "-C").then(|| app.args.get(i + 1).and_then(|n| n.strip_prefix("linker="))).flatten());
        if let Some(v) = v
            && Path::new(v) != ax
        {
            found = Some(v);
        }
    }
    found.unwrap_or(if COFF { "x86_64-w64-mingw32-gcc" } else { "cc" }).to_string()
}

/// The fat link: the app crate's link, with every object of every library
/// built in `target` in place of the library (the standard library's stay
/// libraries), `main` exported, into `exe`. Windows: also loaded at a
/// fixed base below 4 GiB ([`FAT_BASE`], no ASLR: a patch's stub names the
/// binary's data by COFF's 32-bit absolute symbols; Windows places a
/// non-high-entropy binary above 4 GiB all the same) with a COFF symbol
/// table. A system that forces ASLR moves it: the patch then says so.
fn fat_link(link: &Link, app: &Rustc, linker: &str, msvc: bool, exe: &Path, target: &Path) -> Result<(), String> {
    let link_args = &link.args;
    let objects_dir = target.join("aio").join("hotpatch").join("objects");
    let mut objects = vec![];
    let mut libraries = vec![];
    for rlib in link_args.iter().filter(|a| a.ends_with(".rlib")).map(PathBuf::from) {
        if !rlib.starts_with(target) {
            libraries.push(rlib);
            continue;
        }
        let (objs, keep) = extracted(&rlib, &objects_dir)?;
        objects.extend(objs);
        // Members not of rustc's own objects (a native library's): the
        // linker takes what it needs from the library itself.
        if keep {
            libraries.push(rlib);
        }
    }
    let mut args: Vec<String> = link_args.iter().filter(|a| !a.ends_with(".rlib")).cloned().collect();
    if let Some(i) = args.iter().position(|a| a == "-o") {
        args.drain(i..(i + 2).min(args.len()));
    }
    args.retain(|a| msvc_value(a, "/OUT:").is_none());
    let at = args.iter().rposition(|a| a.ends_with(".o")).map_or(0, |i| i + 1);
    let mut inserted: Vec<String> = objects.iter().chain(&libraries).map(|p| p.display().to_string()).collect();
    if COFF {
        let export = exe.with_extension("hotpatch-export.o");
        std::fs::write(&export, exports_main()?).map_err(|e| format!("hot-patch: {}: {e}", export.display()))?;
        inserted.push(export.display().to_string());
    }
    args.splice(at..at, inserted);
    let tail: Vec<String> = match (MACHO, COFF, msvc) {
        (true, ..) => vec!["-Wl,-exported_symbol,_main".into()],
        (_, true, true) => vec![
            "/HIGHENTROPYVA:NO".into(),
            "/DYNAMICBASE:NO".into(),
            format!("/BASE:{FAT_BASE:#x}"),
            "/DEBUG:SYMTAB".into(),
        ],
        (_, true, false) => vec![
            "-Wl,--disable-high-entropy-va".into(),
            "-Wl,--disable-dynamicbase".into(),
            format!("-Wl,--image-base={FAT_BASE:#x}"),
        ],
        _ => vec!["-Wl,--export-dynamic-symbol,main".into()],
    };
    args.extend(tail);
    match msvc {
        true => args.push(format!("/OUT:{}", exe.display())),
        false => args.extend(["-o".to_string(), exe.display().to_string()]),
    }
    // cargo's copy of the empty object rustc linked: replaced, not written through.
    let _ = std::fs::remove_file(exe);
    let rsp = exe.with_extension("hotpatch-link");
    let body: Vec<String> = args.iter().map(|a| if msvc { msvc_response_arg(a) } else { response_arg(a) }).collect();
    std::fs::write(&rsp, body.join("\n")).map_err(|e| format!("hot-patch: {}: {e}", rsp.display()))?;
    let out = Command::new(linker)
        .args(if msvc { &["-flavor", "link"][..] } else { &[] })
        .arg(format!("@{}", rsp.display()))
        .current_dir(&app.cwd)
        .env_clear()
        .envs(link.envs.iter().cloned())
        .output()
        .map_err(|e| format!("hot-patch: can't run {linker}: {e}"))?;
    let _ = std::fs::remove_file(exe.with_extension("hotpatch-export.o"));
    let _ = std::fs::remove_file(&rsp);
    remove_temps(link_args, app);
    match out.status.success() {
        true => Ok(()),
        false => Err(format!(
            "hot-patch: the fat link failed:\n{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )),
    }
}

/// A COFF object whose directives export `main` (aio-rs-hotpatch finds the
/// binary and each patch by it), for GNU ld and lld-link alike.
fn exports_main() -> Result<Vec<u8>, String> {
    let mut obj = object::write::Object::new(
        object::BinaryFormat::Coff,
        object::Architecture::X86_64,
        object::Endianness::Little,
    );
    export_main(&mut obj);
    obj.write().map_err(|e| format!("hot-patch: the export object: {e}"))
}

fn export_main(obj: &mut object::write::Object) {
    let drectve = obj.add_section(vec![], b".drectve".to_vec(), object::SectionKind::Linker);
    obj.append_section_data(drectve, b" -export:main", 1);
}

/// The objects rustc wrote into `rlib`, as files in `dir` (kept while the
/// rlib is the same), and whether it holds other members too.
fn extracted(rlib: &Path, dir: &Path) -> Result<(Vec<PathBuf>, bool), String> {
    let fail = |e: &dyn std::fmt::Display| format!("hot-patch: {}: {e}", rlib.display());
    let meta = std::fs::metadata(rlib).map_err(|e| fail(&e))?;
    let modified = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).unwrap_or_default();
    let stamp = format!("{} {}", meta.len(), modified.as_nanos());
    let name = rlib.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let out = dir.join(&name);
    let index = out.join("index");
    if let Ok(text) = std::fs::read_to_string(&index) {
        let mut lines = text.lines();
        if lines.next() == Some(stamp.as_str()) {
            let keep = lines.next() == Some("keep");
            return Ok((lines.map(|l| out.join(l)).collect(), keep));
        }
    }
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).map_err(|e| fail(&e))?;
    let data = std::fs::read(rlib).map_err(|e| fail(&e))?;
    let archive = object::read::archive::ArchiveFile::parse(&*data).map_err(|e| fail(&e))?;
    let (mut names, mut keep) = (vec![], false);
    for member in archive.members() {
        let member = member.map_err(|e| fail(&e))?;
        let member_name = String::from_utf8_lossy(member.name()).replace(['/', '\\'], "_");
        if member_name.ends_with(".rmeta") || member.size() == 0 {
            continue;
        }
        if !member_name.ends_with(".rcgu.o") {
            keep = true;
            continue;
        }
        std::fs::write(out.join(&member_name), member.data(&*data).map_err(|e| fail(&e))?).map_err(|e| fail(&e))?;
        names.push(member_name);
    }
    let body = format!("{stamp}\n{}\n{}", if keep { "keep" } else { "drop" }, names.join("\n"));
    std::fs::write(&index, body).map_err(|e| fail(&e))?;
    Ok((names.iter().map(|n| out.join(n)).collect(), keep))
}

/// What rustc left for the link (`-Csave-temps`): the app crate's objects
/// (not on macOS, whose debug info stays in them, as rustc leaves every
/// crate's), its bitcode, and rustc's own temporary folders.
fn remove_temps(link_args: &[String], app: &Rustc) {
    for a in link_args.iter().filter(|a| !MACHO && a.ends_with(".rcgu.o")) {
        let _ = std::fs::remove_file(a);
    }
    let prefix = format!("{}{}.", app.name(), app.codegen("extra-filename").unwrap_or_default());
    for e in app.out_dir().and_then(|d| std::fs::read_dir(d).ok()).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with(&prefix) && (name.ends_with(".rcgu.bc") || name.ends_with(".rcgu.no-opt.bc")) {
            let _ = std::fs::remove_file(e.path());
        }
        // The binary's empty metadata, in rustc's folder for it (`rmetaXXXXXX`).
        let empty = e.path().join("full.rmeta");
        if name.len() == 11
            && name.starts_with("rmeta")
            && std::fs::metadata(&empty).is_ok_and(|m| m.len() == 0)
            && std::fs::read_dir(e.path()).map_or(0, Iterator::count) == 1
        {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
    let symbols = link_args.iter().map(Path::new).find(|p| p.file_name().is_some_and(|n| n == "symbols.o"));
    if let Some(dir) = symbols.and_then(Path::parent)
        && dir.file_name().is_some_and(|n| n.to_string_lossy().starts_with("rustc"))
    {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// A symbol of the fat binary.
struct Sym {
    address: u64,
    kind: SymbolKind,
    undefined: bool,
    weak: bool,
    size: u64,
    flags: SymbolFlags<SectionId, SymbolId>,
    /// PE: in a writable section (a static's state, [`rewrite_coff`]).
    writable: bool,
}

/// The fat binary's symbols, read once: every patch is resolved against them.
struct Symbols {
    table: HashMap<String, Sym>,
    /// Its thread-local data's initial image (`.tdata`, `__thread_data`).
    tls_data: Vec<u8>,
    /// Mach-O: each `<name>$tlv$init`'s offset and size in that image (its
    /// symbols carry no size).
    tls_sizes: HashMap<String, (u64, u64)>,
    /// ELF: how far below the thread pointer its thread-local block starts
    /// (x86_64's variant II layout): a thread-local's offset from the thread
    /// pointer is its symbol's value less this.
    #[cfg_attr(not(all(target_os = "linux", target_arch = "x86_64")), allow(dead_code))]
    tls_block: Option<u64>,
    /// PE: where its `.tls` section starts (a thread-local's offset in each
    /// thread's block is its address less this).
    tls_start: u64,
}

impl Symbols {
    fn of(exe: &Path) -> Result<Symbols, String> {
        let fail = |e: &dyn std::fmt::Display| format!("hot-patch: {}: {e}", exe.display());
        let bytes = std::fs::read(exe).map_err(|e| fail(&e))?;
        let obj = object::File::parse(&*bytes).map_err(|e| fail(&e))?;
        let pe = matches!(obj.format(), object::BinaryFormat::Pe);
        let table: HashMap<String, Sym> = if pe {
            pe_table(&obj)
        } else {
            obj.symbols()
                .filter_map(|s| {
                    let flags = match s.flags() {
                        SymbolFlags::Elf { st_info, st_other } => SymbolFlags::Elf { st_info, st_other },
                        SymbolFlags::MachO { n_desc } => SymbolFlags::MachO { n_desc },
                        _ => SymbolFlags::None,
                    };
                    let sym = Sym {
                        address: s.address(),
                        kind: s.kind(),
                        undefined: s.is_undefined(),
                        weak: s.is_weak(),
                        size: s.size(),
                        flags,
                        writable: false,
                    };
                    Some((s.name().ok()?.to_string(), sym))
                })
                .collect()
        };
        if !table.contains_key(MAIN) {
            return Err(fail(&format!("no `{MAIN}` symbol (stripped?)")));
        }
        let tls = obj.sections().find(|s| matches!(s.name(), Ok(".tdata" | "__thread_data")));
        let tls_data = tls.as_ref().and_then(|s| s.data().ok()).unwrap_or_default().to_vec();
        let (start, size) = tls.as_ref().map_or((0, 0), |s| (s.address(), s.size()));
        let mut inits: Vec<(u64, String)> = obj
            .symbols()
            .filter(|s| tls.is_some() && s.section_index() == tls.as_ref().map(|t| t.index()))
            .filter_map(|s| Some((s.address().saturating_sub(start), s.name().ok()?.to_string())))
            .collect();
        inits.sort();
        inits.dedup_by_key(|(at, _)| *at);
        let ends: Vec<u64> = inits.iter().skip(1).map(|(at, _)| *at).chain([size]).collect();
        let tls_sizes =
            inits.into_iter().zip(ends).map(|((at, name), end)| (name, (at, end.saturating_sub(at)))).collect();
        let tls_start = obj.sections().find(|s| pe && s.name() == Ok(".tls")).map_or(0, |s| s.address());
        Ok(Symbols { table, tls_data, tls_sizes, tls_block: tls_block(&bytes), tls_start })
    }

    /// PE: `name` is a writable static of the binary's, whose copy a patch
    /// must use ([`rewrite_coff`]).
    fn is_state(&self, name: &str) -> bool {
        self.table.get(name).is_some_and(|s| s.kind == SymbolKind::Data && s.writable)
    }
}

/// A PE binary's COFF symbol table (rust-lld's `/DEBUG:SYMTAB`, GNU ld's
/// own): its code, data and thread-locals by name, each kind told by its
/// section (an import thunk is code whatever its symbol says). A name
/// defined at two addresses (C statics of two files) is left out: a patch
/// can't tell which it means, so a use of it fails to link (a rebuild).
fn pe_table(obj: &object::File) -> HashMap<String, Sym> {
    use object::pe::{IMAGE_SCN_CNT_CODE, IMAGE_SCN_MEM_WRITE};
    let (mut table, mut twice) = (HashMap::new(), HashSet::new());
    for s in obj.symbols().filter(|s| matches!(s.kind(), SymbolKind::Text | SymbolKind::Data)) {
        let (Some(index), Ok(name)) = (s.section_index(), s.name()) else { continue };
        let Ok(section) = obj.section_by_index(index) else { continue };
        let ch = match section.flags() {
            object::SectionFlags::Coff { characteristics } => characteristics,
            _ => 0,
        };
        let kind = match (ch & IMAGE_SCN_CNT_CODE != 0, section.name() == Ok(".tls")) {
            (true, _) => SymbolKind::Text,
            (_, true) => SymbolKind::Tls,
            _ => SymbolKind::Data,
        };
        let sym = Sym {
            address: s.address(),
            kind,
            undefined: false,
            weak: false,
            size: 0,
            flags: SymbolFlags::None,
            writable: ch & IMAGE_SCN_MEM_WRITE != 0,
        };
        match table.get(name) {
            Some(Sym { address, .. }) if *address != sym.address => _ = twice.insert(name.to_string()),
            Some(_) => {}
            None => _ = table.insert(name.to_string(), sym),
        }
    }
    table.retain(|name, _| !twice.contains(name));
    table
}

/// An ELF executable's thread-local block below the thread pointer (its
/// `PT_TLS` segment, aligned as the linker places it), if it has one.
fn tls_block(bytes: &[u8]) -> Option<u64> {
    use object::read::elf::{FileHeader, ProgramHeader};
    let header = object::elf::FileHeader64::<object::Endianness>::parse(bytes).ok()?;
    let endian = header.endian().ok()?;
    let tls = header.program_headers(endian, bytes).ok()?.iter().find(|p| p.p_type(endian) == object::elf::PT_TLS)?;
    let (size, vaddr, align) = (tls.p_memsz(endian), tls.p_vaddr(endian), tls.p_align(endian).max(1));
    Some(size + (vaddr.wrapping_neg().wrapping_sub(size) & (align - 1)))
}

/// A patched object rewritten for the patch link, when this platform can
/// ([`rewrite_object`]); nothing to do elsewhere, the object stays as it is.
fn maybe_rewrite(path: &Path, symbols: &Symbols, dir: &Path) -> Result<Option<PathBuf>, String> {
    if COFF {
        return rewrite_coff(path, symbols, dir);
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        rewrite_object(path, symbols, dir)
    }
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    {
        let _ = (path, symbols, dir);
        Ok(None)
    }
}

/// Patched objects are rewritten here (each replayed library's members too):
/// Linux x86_64 ([`rewrite_object`]) and Windows ([`rewrite_coff`]).
const REWRITES: bool = cfg!(any(all(target_os = "linux", target_arch = "x86_64"), windows));

/// A COFF object's layout (plain or `/bigobj`), as offsets into its bytes;
/// x86_64 only.
struct Coff {
    big: bool,
    sections: usize,
    section_table: usize,
    symtab: usize,
    symbols: usize,
    strtab: usize,
}

impl Coff {
    fn parse(b: &[u8]) -> Option<Coff> {
        let u16_at = |o: usize| Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?));
        let u32_at = |o: usize| Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?) as usize);
        // `/bigobj` (version 2 on): an import object starts as one does.
        let big = u16_at(0)? == 0 && u16_at(2)? == 0xffff && u16_at(4)? >= 2;
        let (machine, sections, symtab, symbols, section_table) = match big {
            true => (u16_at(6)?, u32_at(44)?, u32_at(48)?, u32_at(52)?, 56),
            false => (u16_at(0)?, usize::from(u16_at(2)?), u32_at(8)?, u32_at(12)?, 20 + usize::from(u16_at(16)?)),
        };
        let coff = Coff { big, sections, section_table, symtab, symbols, strtab: 0 };
        let strtab = symtab + symbols * coff.symsize();
        u32_at(strtab)?;
        (machine == object::pe::IMAGE_FILE_MACHINE_AMD64).then_some(Coff { strtab, ..coff })
    }

    fn symsize(&self) -> usize {
        if self.big { 20 } else { 18 }
    }

    /// Section `n`'s (1-based) header.
    fn header(&self, n: usize) -> usize {
        self.section_table + (n - 1) * 40
    }

    fn sym(&self, k: usize) -> usize {
        self.symtab + k * self.symsize()
    }

    /// A name of 8 bytes, or a string table offset (`/123` for a section).
    fn name(&self, b: &[u8], field: &[u8]) -> String {
        let at = |off: usize| {
            let s = &b[(self.strtab + off).min(b.len())..];
            String::from_utf8_lossy(&s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())]).into_owned()
        };
        if field[..4] == [0; 4] {
            return at(u32::from_le_bytes(field[4..8].try_into().unwrap()) as usize);
        }
        let short = String::from_utf8_lossy(&field[..field.iter().position(|&c| c == 0).unwrap_or(8)]).into_owned();
        match short.strip_prefix('/').and_then(|n| n.parse().ok()) {
            Some(off) => at(off),
            None => short,
        }
    }

    /// Symbol `k`: name, value, section number, storage class, aux records.
    fn symbol(&self, b: &[u8], k: usize) -> (String, u32, i32, u8, u8) {
        let o = self.sym(k);
        let value = u32::from_le_bytes(b[o + 8..o + 12].try_into().unwrap());
        let (section, rest) = match self.big {
            true => (i32::from_le_bytes(b[o + 12..o + 16].try_into().unwrap()), o + 18),
            false => (i32::from(i16::from_le_bytes(b[o + 12..o + 14].try_into().unwrap())), o + 16),
        };
        (self.name(b, &b[o..o + 8]), value, section, b[rest], b[rest + 1])
    }
}

/// Windows x86_64: a patched object (COFF) rewritten so the patch uses the
/// running fat binary's thread-locals and writable statics, not fresh ones:
/// a copy in `dir`, or `None` when nothing needed rewriting.
///
/// The patch is a DLL linked at a fixed base (`/FIXED`, no relocations)
/// the app names, a free span near the binary, and the binary is loaded
/// below 4 GiB ([`fat_link`]), so the linker resolves a reference to the
/// binary's data (an absolute symbol of the stub) exactly, rip-relative or
/// absolute, whatever the instruction: no code is rewritten. What's left:
/// - writable statics the binary also has: each reference (but debug
///   info's) points at a new undefined `aio$fat$<name>`, which the stub
///   defines at the binary's copy, so state is kept; read-only data stays
///   the patch's own (an edited constant shows).
/// - thread-locals (MSVC: `_tls_index` and an offset in the module's block,
///   `SECREL`): the offset becomes the variable's in the binary's block, its
///   relocation dropped, and `_tls_index` resolves to the binary's; a
///   new thread-local is an error, for a rebuild. (GNU's are OS keys:
///   writable statics.)
fn rewrite_coff(path: &Path, symbols: &Symbols, dir: &Path) -> Result<Option<PathBuf>, String> {
    use object::pe::{
        IMAGE_REL_AMD64_ABSOLUTE, IMAGE_REL_AMD64_ADDR32, IMAGE_REL_AMD64_ADDR64, IMAGE_REL_AMD64_REL32,
        IMAGE_REL_AMD64_REL32_5, IMAGE_REL_AMD64_SECREL, IMAGE_SCN_CNT_CODE, IMAGE_SCN_LNK_INFO,
        IMAGE_SCN_LNK_NRELOC_OVFL, IMAGE_SCN_MEM_DISCARDABLE, IMAGE_SYM_CLASS_EXTERNAL, IMAGE_SYM_CLASS_STATIC,
    };
    let fail = |e: &dyn std::fmt::Display| format!("hot-patch: {}: {e}", path.display());
    let orig = std::fs::read(path).map_err(|e| fail(&e))?;
    let Some(c) = Coff::parse(&orig) else { return Ok(None) };
    let u16_at = |o: usize| u16::from_le_bytes(orig[o..o + 2].try_into().unwrap());
    let u32_at = |o: usize| u32::from_le_bytes(orig[o..o + 4].try_into().unwrap());
    let section_name = |n: usize| c.name(&orig, &orig[c.header(n)..c.header(n) + 8]);

    // Each symbol (aux records skipped), and the named ones each section
    // defines, by value: a section-relative reference's target.
    let mut syms = HashMap::new();
    let mut named: HashMap<i32, Vec<(u32, String)>> = HashMap::new();
    let mut k = 0;
    while k < c.symbols {
        let (name, value, section, class, aux) = c.symbol(&orig, k);
        let section_symbol = class == IMAGE_SYM_CLASS_STATIC && aux > 0;
        if section > 0 && !section_symbol && matches!(class, IMAGE_SYM_CLASS_EXTERNAL | IMAGE_SYM_CLASS_STATIC) {
            named.entry(section).or_default().push((value, name.clone()));
        }
        syms.insert(k, (name, section_symbol.then_some(section)));
        k += 1 + usize::from(aux);
    }
    named.values_mut().for_each(|v| v.sort());

    let mut d = orig.clone();
    let mut appended: Vec<String> = vec![];
    for n in 1..=c.sections {
        let h = c.header(n);
        let ch = u32_at(h + 36);
        if ch & (IMAGE_SCN_MEM_DISCARDABLE | IMAGE_SCN_LNK_INFO) != 0 || section_name(n).starts_with(".debug") {
            continue;
        }
        let (raw, relocs, mut count) = (u32_at(h + 20) as usize, u32_at(h + 24) as usize, usize::from(u16_at(h + 32)));
        let mut first = 0;
        if ch & IMAGE_SCN_LNK_NRELOC_OVFL != 0 && count == 0xffff {
            (count, first) = (u32_at(relocs) as usize, 1);
        }
        // Relocations done with here (a thread-local's offset, written in):
        // dropped from the table, lld taking no no-op relocation.
        let mut dropped = HashSet::new();
        for i in first..count {
            let r = relocs + i * 10;
            let (field, index, kind) = (raw + u32_at(r) as usize, u32_at(r + 4) as usize, u16_at(r + 8));
            let wide = kind == IMAGE_REL_AMD64_ADDR64;
            let Some(bytes) = orig.get(field..field + if wide { 8 } else { 4 }) else { continue };
            if kind == IMAGE_REL_AMD64_ABSOLUTE {
                continue;
            }
            let addend = match wide {
                true => i64::from_le_bytes(bytes.try_into().unwrap()),
                false => i64::from(i32::from_le_bytes(bytes.try_into().unwrap())),
            };
            let Some((name, of_section)) = syms.get(&index) else { continue };
            // What it reaches: a named symbol, or one in a section (at the
            // greatest value not past the addend).
            let (target, delta) = match of_section {
                None => (name.clone(), addend),
                Some(s) => {
                    let found = named.get(s).and_then(|v| v.iter().rev().find(|(v, _)| i64::from(*v) <= addend));
                    let Some((value, name)) = found else { continue };
                    (name.clone(), addend - i64::from(*value))
                }
            };
            if kind == IMAGE_REL_AMD64_SECREL && ch & IMAGE_SCN_CNT_CODE != 0 {
                let found = symbols.table.get(&target).filter(|s| s.kind == SymbolKind::Tls);
                let Some(found) = found else {
                    return Err(format!(
                        "hot-patch: thread-local {target} is new (the running binary has none): rebuild"
                    ));
                };
                let offset = u32::try_from(found.address as i64 - symbols.tls_start as i64 + delta)
                    .map_err(|_| fail(&format!("thread-local {target}: offset out of range")))?;
                d[field..field + 4].copy_from_slice(&offset.to_le_bytes());
                dropped.insert(i);
                continue;
            }
            let reaches = wide
                || kind == IMAGE_REL_AMD64_ADDR32
                || (IMAGE_REL_AMD64_REL32..=IMAGE_REL_AMD64_REL32_5).contains(&kind);
            if !reaches || !symbols.is_state(&target) {
                continue;
            }
            let fat = format!("{FAT_PREFIX}{target}");
            let at = appended.iter().position(|a| *a == fat).unwrap_or_else(|| {
                appended.push(fat);
                appended.len() - 1
            });
            d[r + 4..r + 8].copy_from_slice(&((c.symbols + at) as u32).to_le_bytes());
            if of_section.is_some() {
                match wide {
                    true => d[field..field + 8].copy_from_slice(&delta.to_le_bytes()),
                    false => d[field..field + 4].copy_from_slice(&(delta as i32).to_le_bytes()),
                }
            }
        }
        if !dropped.is_empty() {
            let kept: Vec<Vec<u8>> = (first..count)
                .filter(|i| !dropped.contains(i))
                .map(|i| d[relocs + i * 10..relocs + i * 10 + 10].to_vec())
                .collect();
            for (j, e) in kept.iter().enumerate() {
                let at = relocs + (first + j) * 10;
                d[at..at + 10].copy_from_slice(e);
            }
            let left = first + kept.len();
            match first {
                // The overflow form: the count (itself included) in the first entry.
                1 => d[relocs..relocs + 4].copy_from_slice(&(left as u32).to_le_bytes()),
                _ => d[h + 32..h + 34].copy_from_slice(&(left as u16).to_le_bytes()),
            }
        }
    }
    if d == orig {
        return Ok(None);
    }
    if !appended.is_empty() {
        // The new undefined externals after the old symbols, the string
        // table after them (it follows the symbols, by the format).
        let strsize = u32_at(c.strtab) as usize;
        let mut strtab = orig[c.strtab..c.strtab + strsize].to_vec();
        let mut table = orig[c.symtab..c.strtab].to_vec();
        for name in &appended {
            let mut e = vec![0u8; c.symsize()];
            e[4..8].copy_from_slice(&(strtab.len() as u32).to_le_bytes());
            strtab.extend_from_slice(name.as_bytes());
            strtab.push(0);
            e[c.symsize() - 2] = IMAGE_SYM_CLASS_EXTERNAL;
            table.extend_from_slice(&e);
        }
        let len = strtab.len() as u32;
        strtab[..4].copy_from_slice(&len.to_le_bytes());
        let at = if c.strtab + strsize == orig.len() { c.symtab } else { d.len() };
        d.truncate(at);
        d.extend_from_slice(&table);
        d.extend_from_slice(&strtab);
        let (symtab_field, count_field) = if c.big { (48, 52) } else { (8, 12) };
        d[symtab_field..symtab_field + 4].copy_from_slice(&(at as u32).to_le_bytes());
        d[count_field..count_field + 4].copy_from_slice(&((c.symbols + appended.len()) as u32).to_le_bytes());
    }
    std::fs::create_dir_all(dir).map_err(|e| fail(&e))?;
    let copy = dir.join(path.file_name().unwrap_or_default());
    std::fs::write(&copy, &d).map_err(|e| fail(&e))?;
    Ok(Some(copy))
}

/// The `.o` members of an archive (an rlib), by name and bytes.
fn members(rlib: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    let fail = |e: &dyn std::fmt::Display| format!("hot-patch: {}: {e}", rlib.display());
    let data = std::fs::read(rlib).map_err(|e| fail(&e))?;
    let archive = object::read::archive::ArchiveFile::parse(&*data).map_err(|e| fail(&e))?;
    let mut out = vec![];
    for member in archive.members() {
        let member = member.map_err(|e| fail(&e))?;
        let name = String::from_utf8_lossy(member.name()).replace(['/', '\\'], "_");
        if name.ends_with(".o") {
            out.push((name, member.data(&*data).map_err(|e| fail(&e))?.to_vec()));
        }
    }
    Ok(out)
}

/// Linux x86_64: a patched object (the app crate's, a replayed library's)
/// rewritten so it uses the running fat binary's copy of each thread-local
/// and static it shares, not a fresh one; a copy in `dir`, or `None` when
/// nothing needed rewriting.
///
/// rustc builds these crates as an executable, so a static or thread-local
/// is reached by a form a shared library can't hold:
/// - thread-locals: at a fixed offset from the thread pointer (`TPOFF32`),
///   or through the GOT (`GOTTPOFF`). Each is rewritten to the offset the
///   fat binary gives it, as a linker relaxes initial-exec to local-exec,
///   so the patch reads and writes the app's own thread-local. A new one
///   (not in the fat binary): an error, for a rebuild.
/// - statics: taken by address with `lea reg, [rip+static]` against the
///   static's own section. A static the fat binary also has is redirected
///   to the fat binary's copy so state is kept across a patch: the `lea`
///   becomes `mov reg, [rip+GOT]` and the relocation a `GOTPCREL` against a
///   new undefined global of the static's name, which the stub defines at
///   the fat binary's aslr-adjusted address. A static only the patch has
///   (not in the fat binary) keeps its fresh copy. Defined globals the fat
///   binary shares are made undefined so the stub's copy wins, not a
///   duplicate definition.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn rewrite_object(path: &Path, symbols: &Symbols, dir: &Path) -> Result<Option<PathBuf>, String> {
    use object::elf::{
        R_X86_64_GOTPCREL, R_X86_64_GOTPCRELX, R_X86_64_GOTTPOFF, R_X86_64_PC32, R_X86_64_REX_GOTPCRELX,
        R_X86_64_TPOFF32, SHN_UNDEF, SHT_RELA, SHT_SYMTAB, STB_GLOBAL, STT_SECTION,
    };
    let fail = |e: &dyn std::fmt::Display| format!("hot-patch: {}: {e}", path.display());
    let orig = std::fs::read(path).map_err(|e| fail(&e))?;
    if orig.len() < 64 || orig[..4] != *b"\x7fELF" {
        return Ok(None);
    }
    let u16_at = |b: &[u8], o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
    let u32_at = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let u64_at = |b: &[u8], o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
    let cstr = |b: &[u8], o: usize| {
        let end = b[o..].iter().position(|&c| c == 0).map_or(b.len(), |n| o + n);
        String::from_utf8_lossy(&b[o..end]).into_owned()
    };

    let (shoff, shentsize, shnum, shstrndx) = (
        u64_at(&orig, 40) as usize,
        u16_at(&orig, 58) as usize,
        u16_at(&orig, 60) as usize,
        u16_at(&orig, 62) as usize,
    );
    let sh = |k: usize| shoff + k * shentsize;
    let sh_name = |k: usize| u32_at(&orig, sh(k)) as usize;
    let sh_type = |k: usize| u32_at(&orig, sh(k) + 4);
    let sh_offset = |k: usize| u64_at(&orig, sh(k) + 24) as usize;
    let sh_size = |k: usize| u64_at(&orig, sh(k) + 32) as usize;
    let sh_link = |k: usize| u32_at(&orig, sh(k) + 40) as usize;
    let shstr = sh_offset(shstrndx);
    let section_name = |k: usize| cstr(&orig, shstr + sh_name(k));

    let Some(symtab) = (0..shnum).find(|&k| sh_type(k) == SHT_SYMTAB) else { return Ok(None) };
    let strtab = sh_link(symtab);
    let (symoff, symsize, stroff, strsize) = (sh_offset(symtab), sh_size(symtab), sh_offset(strtab), sh_size(strtab));
    let sym_count = symsize / 24;
    let sym = |k: usize| symoff + k * 24;
    let sym_name = |k: usize| cstr(&orig, stroff + u32_at(&orig, sym(k)) as usize);
    let sym_bind = |k: usize| orig[sym(k) + 4] >> 4;
    let sym_type = |k: usize| orig[sym(k) + 4] & 0xf;
    let sym_shndx = |k: usize| u16_at(&orig, sym(k) + 6);
    let defined = |k: usize| sym_shndx(k) != SHN_UNDEF && sym_shndx(k) < 0xff00;

    // A section symbol's static (its own `.data`/`.bss`/`.rodata` section),
    // or a symbol's own name: the name to match against the fat binary.
    let static_of = |section: &str| -> Option<String> {
        for p in [".data.rel.ro.", ".data.", ".bss.", ".rodata."] {
            if let Some(name) = section.strip_prefix(p) {
                return Some(name.to_string());
            }
        }
        None
    };
    let is_fat_data = |name: &str| {
        symbols
            .table
            .get(name)
            .is_some_and(|s| !s.undefined && matches!(s.kind, SymbolKind::Data | SymbolKind::Unknown))
    };

    let mut d = orig.clone();
    let mut new_symtab = orig[symoff..symoff + symsize].to_vec();
    let mut appended: Vec<String> = vec![];
    let mut und_index: HashMap<String, u32> = HashMap::new();
    let mut sym_changed = false;
    // Make symbol `k` undefined in the new symbol table (section UND, no
    // value or size): the stub's copy is used, not this fresh definition.
    let make_undef = |table: &mut [u8], k: usize| {
        let e = k * 24;
        table[e + 6..e + 8].copy_from_slice(&0u16.to_le_bytes());
        table[e + 8..e + 16].copy_from_slice(&0u64.to_le_bytes());
        table[e + 16..e + 24].copy_from_slice(&0u64.to_le_bytes());
    };
    // Globals by name, to reuse (made undefined) rather than append a second.
    let mut globals: HashMap<String, usize> = HashMap::new();
    for k in 1..sym_count {
        if sym_bind(k) == STB_GLOBAL {
            globals.entry(sym_name(k)).or_insert(k);
        }
    }
    // The fat static a relocation reaches: its own section's (a section
    // symbol) or the symbol's own name, when the fat binary shares it.
    let static_name = |symidx: usize| -> Option<String> {
        let name = if sym_type(symidx) == STT_SECTION {
            static_of(&section_name(sym_shndx(symidx) as usize))?
        } else {
            sym_name(symidx)
        };
        is_fat_data(&name).then_some(name)
    };
    // A `lea reg, [rip+x]` (REX.W, opcode 8d, rip-relative): a static taken
    // by address, which must use the fat binary's copy; anything else at a
    // relocation is a by-value read, fine from a fresh copy.
    let is_lea = |at: usize| orig[at - 3] & 0xf8 == 0x48 && orig[at - 2] == 0x8d && orig[at - 1] & 0xc7 == 0x05;

    // Classify every static the code reaches: taken by address (redirect to
    // the fat binary) or read by value (leave its fresh, identical copy). A
    // static reached both ways can't be done in place: a rebuild.
    let mut by_address: HashSet<String> = HashSet::new();
    let mut by_value: HashSet<String> = HashSet::new();
    for relsec in (0..shnum).filter(|&k| sh_type(k) == SHT_RELA) {
        if !section_name(sh_link_info(&orig, sh(relsec))).starts_with(".text") {
            continue;
        }
        let (code, table, count) =
            (sh_offset(sh_link_info(&orig, sh(relsec))), sh_offset(relsec), sh_size(relsec) / 24);
        for i in 0..count {
            let r = table + i * 24;
            let r_info = u64_at(&orig, r + 8);
            let (kind, symidx) = ((r_info & 0xffff_ffff) as u32, (r_info >> 32) as usize);
            let at = code + u64_at(&orig, r) as usize;
            if kind == R_X86_64_PC32 {
                if let Some(name) = static_name(symidx) {
                    if is_lea(at) { &mut by_address } else { &mut by_value }.insert(name);
                }
            } else if matches!(kind, R_X86_64_GOTPCREL | R_X86_64_GOTPCRELX | R_X86_64_REX_GOTPCRELX)
                && defined(symidx)
                && is_fat_data(&sym_name(symidx))
            {
                by_address.insert(sym_name(symidx));
            }
        }
    }
    if let Some(name) = by_address.intersection(&by_value).next() {
        return Err(fail(&format!(
            "static {name}: taken by address and read by value, which ax can't redirect: rebuild"
        )));
    }

    // The index of an undefined global of `name`: an existing global (made
    // undefined) or a freshly appended one.
    let mut undef_global = |name: &str, new_symtab: &mut Vec<u8>, sym_changed: &mut bool| -> u32 {
        if let Some(&i) = und_index.get(name) {
            return i;
        }
        let idx = if let Some(&k) = globals.get(name) {
            make_undef(new_symtab, k);
            *sym_changed = true;
            k as u32
        } else {
            let i = sym_count + appended.len();
            appended.push(name.to_string());
            i as u32
        };
        und_index.insert(name.to_string(), idx);
        idx
    };

    for relsec in (0..shnum).filter(|&k| sh_type(k) == SHT_RELA) {
        let target = sh_link_info(&orig, sh(relsec));
        let in_text = section_name(target).starts_with(".text");
        let (code, table, count) = (sh_offset(target), sh_offset(relsec), sh_size(relsec) / 24);
        for i in 0..count {
            let r = table + i * 24;
            let (r_offset, r_info, r_addend) = (
                u64_at(&orig, r) as usize,
                u64_at(&orig, r + 8),
                i64::from_le_bytes(orig[r + 16..r + 24].try_into().unwrap()),
            );
            let (kind, symidx) = ((r_info & 0xffff_ffff) as u32, (r_info >> 32) as usize);
            let at = code + r_offset;

            if kind == R_X86_64_TPOFF32 || kind == R_X86_64_GOTTPOFF {
                let name = sym_name(symidx);
                let found = symbols.table.get(&name).filter(|s| s.kind == SymbolKind::Tls && !s.undefined);
                let (Some(found), Some(block)) = (found, symbols.tls_block) else {
                    return Err(format!(
                        "hot-patch: thread-local {name} is new (the running binary has none): rebuild"
                    ));
                };
                if kind == R_X86_64_GOTTPOFF {
                    // `mov x@gottpoff(%rip), %reg` → `mov $off, %reg`;
                    // `add x@gottpoff(%rip), %reg` → `add $off, %reg`.
                    let (rex, op, modrm) = (d[at - 3], d[at - 2], d[at - 1]);
                    if rex & 0xf8 != 0x48 || modrm & 0xc7 != 0x05 || (op != 0x8b && op != 0x03) {
                        return Err(fail(&format!(
                            "thread-local {name}: an access ax can't relax ({rex:#x} {op:#x} {modrm:#x})"
                        )));
                    }
                    let reg = (modrm >> 3) & 7;
                    d[at - 3] = 0x48 | ((rex >> 2) & 1);
                    d[at - 2] = if op == 0x8b { 0xc7 } else { 0x81 };
                    d[at - 1] = 0xc0 | reg;
                }
                // The value `S + A - TP`; GOTTPOFF's addend takes back the
                // `-4` of a rip-relative operand.
                let addend = r_addend + if kind == R_X86_64_GOTTPOFF { 4 } else { 0 };
                let value = i32::try_from(found.address as i64 - block as i64 + addend)
                    .map_err(|_| fail(&format!("thread-local {name}: offset out of range")))?;
                d[at..at + 4].copy_from_slice(&value.to_le_bytes());
                // The relocation becomes R_X86_64_NONE, its symbol kept.
                d[r + 8..r + 16].copy_from_slice(&(r_info & !0xffff_ffff).to_le_bytes());
                continue;
            }

            if !in_text {
                continue;
            }

            // A static taken by address with `lea reg, [rip+static]` (against
            // its own section or its symbol): redirected to the fat binary's
            // copy through the GOT, so its state is kept across the patch.
            if kind == R_X86_64_PC32 {
                let Some(name) = static_name(symidx).filter(|n| by_address.contains(n)) else { continue };
                if r_addend != -4 {
                    return Err(fail(&format!("static {name}: taken at an offset ax can't redirect: rebuild")));
                }
                d[at - 2] = 0x8b; // lea → mov, loading the GOT entry
                let idx = undef_global(&name, &mut new_symtab, &mut sym_changed);
                d[r + 8..r + 16].copy_from_slice(&(((idx as u64) << 32) | u64::from(R_X86_64_GOTPCREL)).to_le_bytes());
                continue;
            }

            // A static reached through the GOT, defined in this object: its
            // reference is pointed at the fat binary's copy via the stub.
            let got = matches!(kind, R_X86_64_GOTPCREL | R_X86_64_GOTPCRELX | R_X86_64_REX_GOTPCRELX);
            if got && defined(symidx) && is_fat_data(&sym_name(symidx)) {
                let name = sym_name(symidx);
                // rustc may have relaxed the GOT load to a `lea`; undo that so
                // the GOT entry (the fat address) is loaded, not computed.
                if d[at - 2] == 0x8d {
                    d[at - 2] = 0x8b;
                }
                let idx = undef_global(&name, &mut new_symtab, &mut sym_changed);
                // GOTPCREL (not the relaxable forms): the linker must keep
                // the load, never fold it to a `lea` of the absolute address.
                d[r + 8..r + 16].copy_from_slice(&(((idx as u64) << 32) | u64::from(R_X86_64_GOTPCREL)).to_le_bytes());
                continue;
            }
        }
    }

    let changed = sym_changed || !appended.is_empty() || d != orig;
    if !changed {
        return Ok(None);
    }

    // Grow the symbol and string tables for the appended undefined globals,
    // then move both to the end so no other section's bytes shift.
    if sym_changed || !appended.is_empty() {
        let mut new_strtab = orig[stroff..stroff + strsize].to_vec();
        for name in &appended {
            let st_name = new_strtab.len() as u32;
            new_strtab.extend_from_slice(name.as_bytes());
            new_strtab.push(0);
            let mut entry = [0u8; 24];
            entry[0..4].copy_from_slice(&st_name.to_le_bytes());
            entry[4] = STB_GLOBAL << 4; // global, STT_NOTYPE, section UND
            new_symtab.extend_from_slice(&entry);
        }
        while d.len() % 8 != 0 {
            d.push(0);
        }
        let new_symoff = d.len();
        d.extend_from_slice(&new_symtab);
        let new_stroff = d.len();
        d.extend_from_slice(&new_strtab);
        let set = |d: &mut [u8], k: usize, off: usize, size: usize| {
            let h = shoff + k * shentsize;
            d[h + 24..h + 32].copy_from_slice(&(off as u64).to_le_bytes());
            d[h + 32..h + 40].copy_from_slice(&(size as u64).to_le_bytes());
        };
        set(&mut d, symtab, new_symoff, new_symtab.len());
        set(&mut d, strtab, new_stroff, new_strtab.len());
    }

    std::fs::create_dir_all(dir).map_err(|e| fail(&e))?;
    let copy = dir.join(path.file_name().unwrap_or_default());
    std::fs::write(&copy, &d).map_err(|e| fail(&e))?;
    Ok(Some(copy))
}

/// A relocation section's target section index (its `sh_info`).
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn sh_link_info(b: &[u8], header: usize) -> usize {
    u32::from_le_bytes(b[header + 44..header + 48].try_into().unwrap()) as usize
}

/// Patch the running fat binary with the edits to `changed` (source files
/// of the app's workspace crates): the edited crates (and the crates built
/// on them) and the app's own crate built again, linked into a new shared
/// library, the symbols it misses resolved into the running binary
/// (`aio_hotpatch::aslr_reference`, from the app; Windows: `base`, where the
/// app has [`PATCH_ROOM`] free for the DLL). Any file no crate builds from
/// (a manifest, a build script): an error, for a rebuild.
pub fn patch(fat: &mut Fat, aslr_reference: u64, base: Option<u64>, changed: &[PathBuf]) -> Result<Patch, String> {
    let base = match (COFF, base) {
        (true, None) => return Err("hot-patch: the app named no free span to load the patch at".into()),
        (_, b) => b.unwrap_or_default(),
    };
    let started = Instant::now();
    let edited = fat.edited(changed)?;
    let compiled = fat.order(&fat.dependents(&edited))?;
    for key in &compiled {
        fat.crates[key].replay(None)?;
    }
    fat.modified.extend(compiled);
    let link_file = fat.scope.join("thin-link.json");
    let _ = std::fs::remove_file(&link_file);
    let app = fat.crates[&format!("{}.bin", fat.tip)].clone();
    if let Err(e) = app.replay(Some(&link_file)) {
        remove_temps(&[], &app);
        return Err(e);
    }
    let link = read_link(&link_file)?;
    let compile = started.elapsed();
    let thin = thin_link(fat, &app, &link, aslr_reference, base);
    remove_temps(&link.args, &app);
    let _ = std::fs::remove_dir_all(fat.scope.join("rewrite"));
    let _ = std::fs::remove_file(fat.scope.join("stub.o"));
    let _ = std::fs::remove_file(fat.scope.join("thin-link.rsp"));
    // rustc's output, the empty object: a stale file of that name confuses
    // the dynamic loader on a later patch (dx's finding).
    if let Some(o) = output_arg(&link.args) {
        let _ = std::fs::remove_file(o);
    }
    let (lib, stub, link) = thin?;
    let table = jump_table(&lib, &fat.symbols)?;
    Ok(Patch { table, times: Times { compile, stub, link, total: started.elapsed() } })
}

/// The patch library from the app crate's new objects (and the libraries
/// replayed since the fat build) and a stub for what they miss: its path,
/// and the stub's and the link's times.
fn thin_link(
    fat: &mut Fat,
    app: &Rustc,
    link: &Link,
    aslr_reference: u64,
    base: u64,
) -> Result<(PathBuf, Duration, Duration), String> {
    let link_args = &link.args;
    let at = Instant::now();
    let work = fat.scope.join("rewrite");
    let _ = std::fs::remove_dir_all(&work);
    let mut objects = vec![];
    for o in link_args.iter().filter(|a| a.ends_with(".rcgu.o")).map(PathBuf::from) {
        let rewritten = maybe_rewrite(&o, &fat.symbols, &work)?;
        objects.push(rewritten.unwrap_or(o));
    }
    objects.sort();
    let wanted: HashMap<String, PathBuf> =
        fat.modified.iter().filter_map(|k| Some((fat.crates[k].lib_stem(), fat.crates[k].rlib()?))).collect();
    // The replayed libraries in the fat link's order. Their fresh objects
    // are rewritten too (statics redirected) where the platform can, else
    // the whole rlib goes to the linker as the fat build's did.
    let linked = link_args.iter().filter(|a| a.ends_with(".rlib")).filter_map(|a| Path::new(a).file_stem()?.to_str());
    for rlib in linked.filter_map(|s| wanted.get(s)) {
        if REWRITES {
            let into = work.join("libs");
            std::fs::create_dir_all(&into).map_err(|e| format!("hot-patch: {}: {e}", into.display()))?;
            for (name, bytes) in members(rlib)? {
                let o = into.join(name);
                std::fs::write(&o, bytes).map_err(|e| format!("hot-patch: {}: {e}", o.display()))?;
                objects.push(maybe_rewrite(&o, &fat.symbols, &into)?.unwrap_or(o));
            }
        } else {
            objects.push(rlib.clone());
        }
    }
    let stub = fat.scope.join("stub.o");
    let bytes = stub_object(&fat.symbols, &objects, aslr_reference)?;
    std::fs::write(&stub, bytes).map_err(|e| format!("hot-patch: {}: {e}", stub.display()))?;
    let stub_time = at.elapsed();

    let at = Instant::now();
    fat.patches += 1;
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis();
    let ext = match (MACHO, COFF) {
        (true, _) => "dylib",
        (_, true) => "dll",
        _ => "so",
    };
    let lib = fat.scope.join(format!("lib{}-patch-{ms}-{}.{ext}", fat.tip, fat.patches));
    let mut cmd = Command::new(&fat.linker);
    if COFF {
        // A response file: the objects alone outgrow Windows' command line.
        let mut args: Vec<String> = objects.iter().chain([&stub]).map(|p| p.display().to_string()).collect();
        args.extend(coff_thin_link_args(link_args, fat.msvc, base));
        args.push(if fat.msvc { format!("/OUT:{}", lib.display()) } else { format!("-o{}", lib.display()) });
        let body: Vec<String> =
            args.iter().map(|a| if fat.msvc { msvc_response_arg(a) } else { response_arg(a) }).collect();
        let rsp = fat.scope.join("thin-link.rsp");
        std::fs::write(&rsp, body.join("\n")).map_err(|e| format!("hot-patch: {}: {e}", rsp.display()))?;
        cmd.args(if fat.msvc { &["-flavor", "link"][..] } else { &[] }).arg(format!("@{}", rsp.display()));
    } else {
        let dylibs = link_args.iter().filter(|a| a.ends_with(".so") || a.ends_with(".dylib"));
        cmd.args(&objects).arg(&stub).args(dylibs).args(thin_link_args(link_args)).arg("-o").arg(&lib);
    }
    let out = cmd
        .current_dir(&app.cwd)
        .env_clear()
        .envs(link.envs.iter().cloned())
        .output()
        .map_err(|e| format!("hot-patch: can't run {}: {e}", fat.linker))?;
    if !out.status.success() {
        return Err(format!(
            "hot-patch: the patch link failed:\n{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok((lib, stub_time, at.elapsed()))
}

impl Fat {
    /// The crates each changed file is a source of (their dep-info says),
    /// by key; the app's own crate is always built, so not among them.
    fn edited(&self, changed: &[PathBuf]) -> Result<BTreeSet<String>, String> {
        let sources: Vec<(&String, Vec<PathBuf>)> = self.crates.iter().map(|(k, c)| (k, c.sources())).collect();
        let mut edited = BTreeSet::new();
        for file in changed {
            let file = canonical(file);
            let users: Vec<&String> = sources.iter().filter(|(_, s)| s.contains(&file)).map(|(k, _)| *k).collect();
            if users.is_empty() {
                return Err(format!("hot-patch: no crate of the app builds from {}: rebuild", file.display()));
            }
            if let Some(m) = users.iter().find(|k| k.ends_with(".proc-macro")) {
                return Err(format!(
                    "hot-patch: {} is a proc macro's ({m}), run at build time: rebuild",
                    file.display()
                ));
            }
            edited.extend(users.into_iter().filter(|k| k.ends_with(".lib")).cloned());
        }
        Ok(edited)
    }

    /// `libs` and every library built on them, by key.
    fn dependents(&self, libs: &BTreeSet<String>) -> BTreeSet<String> {
        let mut all = libs.clone();
        let mut todo: Vec<String> = libs.iter().cloned().collect();
        while let Some(key) = todo.pop() {
            let stem = self.crates[&key].lib_stem();
            for (k, c) in self.crates.iter().filter(|(k, _)| k.ends_with(".lib")) {
                if c.externs().contains(&stem) && all.insert(k.clone()) {
                    todo.push(k.clone());
                }
            }
        }
        all
    }

    /// `libs` in build order: each after the libraries it is built on.
    fn order(&self, libs: &BTreeSet<String>) -> Result<Vec<String>, String> {
        let stems: HashMap<String, &String> = libs.iter().map(|k| (self.crates[k].lib_stem(), k)).collect();
        let mut needs: BTreeMap<&String, BTreeSet<&String>> = libs
            .iter()
            .map(|k| {
                (k, self.crates[k].externs().iter().filter_map(|s| stems.get(s).copied()).filter(|d| *d != k).collect())
            })
            .collect();
        let mut order = vec![];
        while !needs.is_empty() {
            let ready: Vec<&String> = needs.iter().filter(|(_, n)| n.is_empty()).map(|(k, _)| *k).collect();
            if ready.is_empty() {
                return Err("hot-patch: the workspace's libraries depend on each other in a cycle".into());
            }
            for k in ready {
                needs.remove(k);
                needs.values_mut().for_each(|n| {
                    n.remove(k);
                });
                order.push(k.clone());
            }
        }
        Ok(order)
    }
}

/// Windows: the patch link's args but its inputs and output: a DLL at
/// `base` with no relocations (the loader can't move it: it fails to load
/// rather than resolve the binary's data wrongly), every section kept, its
/// COFF symbol table written, `main` exported (the stub's `.drectve`), its
/// entry the stub's (no C runtime of its own), the app link's library
/// search paths and system libraries (pure).
fn coff_thin_link_args(link_args: &[String], msvc: bool, base: u64) -> Vec<String> {
    let mut out: Vec<String> = match msvc {
        true => ["/DLL", "/NOLOGO", "/MACHINE:X64", "/FIXED", "/OPT:NOREF", "/OPT:NOICF", "/DEBUG:SYMTAB", "/NOIMPLIB"]
            .map(String::from)
            .into_iter()
            .chain([format!("/ENTRY:{PATCH_ENTRY}"), format!("/BASE:{base:#x}")])
            .collect(),
        false => {
            ["-shared", "-nostartfiles", "-nodefaultlibs", "-Wl,--disable-dynamicbase", "-Wl,--disable-reloc-section"]
                .map(String::from)
                .into_iter()
                .chain([format!("-Wl,-e,{PATCH_ENTRY}"), format!("-Wl,--image-base={base:#x}")])
                .collect()
        }
    };
    let mut i = 0;
    while i < link_args.len() {
        let a = &link_args[i];
        if !msvc && a == "-L" && i + 1 < link_args.len() {
            out.extend([a.clone(), link_args[i + 1].clone()]);
            i += 2;
            continue;
        }
        let lower = a.to_ascii_lowercase();
        let keep = match msvc {
            true => {
                (lower.ends_with(".lib") && !lower.starts_with('/'))
                    || ["/libpath:", "/defaultlib:", "/nodefaultlib"].iter().any(|o| lower.starts_with(o))
            }
            false => {
                a.starts_with("-l")
                    || a.starts_with("-L")
                    || a.starts_with("-m")
                    || a.starts_with("-B")
                    || a.contains("-fuse-ld")
            }
        };
        if keep {
            out.push(a.clone());
        }
        i += 1;
    }
    out
}

/// The patch DLL's entry (the stub's): returns TRUE, so loading runs no
/// C runtime start-up of the patch's own.
const PATCH_ENTRY: &str = "aio_patch_entry";

/// The patch link's args: a shared library, with the app link's library
/// search paths, system libraries and target (pure).
fn thin_link_args(link_args: &[String]) -> Vec<String> {
    let mut out: Vec<String> = match MACHO {
        true => vec!["-Wl,-dylib".into()],
        false => [
            "-shared",
            "-Wl,--eh-frame-hdr",
            "-Wl,-z,noexecstack",
            "-Wl,-z,relro,-z,now",
            "-nodefaultlibs",
            "-Wl,-Bdynamic",
        ]
        .map(String::from)
        .into(),
    };
    let pairs: &[&str] = if MACHO { &["-framework", "-arch", "-L", "-target"] } else { &["-L"] };
    let mut i = 0;
    while i < link_args.len() {
        let a = &link_args[i];
        if pairs.contains(&a.as_str()) && i + 1 < link_args.len() {
            out.extend([a.clone(), link_args[i + 1].clone()]);
            i += 2;
            continue;
        }
        // GNU: `-B<dir>` and `-fuse-ld`: rustc's bundled lld, as the app was linked.
        let single = a.starts_with("-l")
            || a.starts_with("-m")
            || a.starts_with("-nodefaultlibs")
            || (!MACHO
                && (a.starts_with("-Wl,--target=")
                    || a.starts_with("-Wl,-fuse-ld")
                    || a.starts_with("-fuse-ld")
                    || a.starts_with("-B")
                    || a.contains("-ld-path")));
        if single {
            out.push(a.clone());
        }
        i += 1;
    }
    out
}

/// An object defining each symbol `objects` (objects, rlibs) use but don't
/// define, at its place in the running fat binary: code as a jump there,
/// data as its absolute address, thread-locals as a new copy initialized
/// as the fat binary's. Windows: thread-locals and `aio$fat$` statics as
/// absolute addresses too ([`rewrite_coff`]), an import slot (`__imp_x`)
/// of the binary's `x` as a pointer to it (the system's imports left to its
/// libraries), and the DLL's entry and `main` export.
fn stub_object(symbols: &Symbols, objects: &[PathBuf], aslr_reference: u64) -> Result<Vec<u8>, String> {
    let (mut undefined, mut defined) = (HashSet::new(), HashSet::new());
    for path in objects {
        let fail = |e: &dyn std::fmt::Display| format!("hot-patch: {}: {e}", path.display());
        let data = std::fs::read(path).map_err(|e| fail(&e))?;
        if path.extension().is_some_and(|e| e == "rlib" || e == "a") {
            let archive = object::read::archive::ArchiveFile::parse(&*data).map_err(|e| fail(&e))?;
            for member in archive.members() {
                let member = member.map_err(|e| fail(&e))?;
                let name = String::from_utf8_lossy(member.name());
                if name.ends_with(".o") {
                    used(member.data(&*data).map_err(|e| fail(&e))?, &mut undefined, &mut defined)
                        .map_err(|e| fail(&e))?;
                }
            }
        } else {
            used(&data, &mut undefined, &mut defined).map_err(|e| fail(&e))?;
        }
    }
    let (format, arch) = host()?;
    let mut obj = object::write::Object::new(format, arch, object::Endianness::Little);
    if MACHO {
        let mut v = MachOBuildVersion::default();
        v.platform = object::macho::PLATFORM_MACOS;
        v.minos = 11 << 16;
        v.sdk = 11 << 16;
        obj.set_macho_build_version(v);
    }
    let main = symbols.table[MAIN].address;
    let slide = aslr_reference.checked_sub(main).ok_or_else(|| {
        format!("hot-patch: the app's aslr reference {aslr_reference:#x} is below the binary's main {main:#x}: another binary?")
    })?;
    let text = obj.section_id(StandardSection::Text);
    if COFF {
        // mov eax, 1; ret
        let offset = obj.append_section_data(text, &[0xB8, 1, 0, 0, 0, 0xC3], 16);
        obj.add_symbol(Symbol {
            name: PATCH_ENTRY.into(),
            value: offset,
            size: 6,
            scope: SymbolScope::Linkage,
            kind: SymbolKind::Text,
            weak: false,
            section: SymbolSection::Section(text),
            flags: SymbolFlags::None,
        });
        export_main(&mut obj);
    }
    let mut names: Vec<&String> = undefined.difference(&defined).collect();
    names.sort();
    for name in names {
        // An import slot (`__imp_x`, rustc's dllimport of an upstream static
        // or function): a pointer to the binary's `x`; the system's own
        // imports are left to its libraries.
        if let Some(x) = name.strip_prefix("__imp_").filter(|_| COFF) {
            if let Some(sym) = symbols.table.get(x).filter(|s| !s.undefined) {
                let at = sym.address.checked_add(slide).ok_or("hot-patch: an address out of range")?;
                let data = obj.section_id(StandardSection::Data);
                let offset = obj.append_section_data(data, &at.to_le_bytes(), 8);
                obj.add_symbol(Symbol {
                    name: name.as_bytes().to_vec(),
                    value: offset,
                    size: 8,
                    scope: SymbolScope::Linkage,
                    kind: SymbolKind::Data,
                    weak: false,
                    section: SymbolSection::Section(data),
                    flags: SymbolFlags::None,
                });
            }
            continue;
        }
        let fat = name.strip_prefix(FAT_PREFIX);
        let Some(sym) = symbols.table.get(fat.unwrap_or(name)) else { continue };
        // Imports of the fat binary (address 0 until loaded): the system's,
        // which the patch links itself.
        if sym.undefined {
            continue;
        }
        // Mach-O's leading underscore: the writer adds it again.
        let bare = name.as_bytes()[usize::from(MACHO)..].to_vec();
        let at = sym.address + slide;
        if COFF && (fat.is_some() || sym.kind != SymbolKind::Text) {
            // COFF's absolute symbols hold 32 bits.
            if at > u64::from(u32::MAX) {
                return Err(format!(
                    "hot-patch: the app's data is above 4 GiB ({at:#x}: high-entropy ASLR?), past a patch's reach"
                ));
            }
            obj.add_symbol(Symbol {
                name: bare,
                value: at,
                size: 0,
                scope: SymbolScope::Linkage,
                kind: SymbolKind::Data,
                weak: false,
                section: SymbolSection::Absolute,
                flags: SymbolFlags::None,
            });
            continue;
        }
        match sym.kind {
            SymbolKind::Text => {
                let jump = match arch {
                    // jmp [rip+0]; the address.
                    object::Architecture::X86_64 => [&[0xFF, 0x25, 0, 0, 0, 0][..], &at.to_le_bytes()].concat(),
                    // ldr x16, #8; br x16; the address.
                    _ => [&[0x50, 0x00, 0x00, 0x58, 0x00, 0x02, 0x1F, 0xD6][..], &at.to_le_bytes()].concat(),
                };
                let offset = obj.append_section_data(text, &jump, 8);
                obj.add_symbol(Symbol {
                    name: bare,
                    value: offset,
                    size: jump.len() as u64,
                    scope: SymbolScope::Linkage,
                    kind: SymbolKind::Text,
                    weak: false,
                    section: SymbolSection::Section(text),
                    flags: SymbolFlags::None,
                });
            }
            // The patch's own copy, initialized as the fat binary's: ELF
            // gives the offset and size in the image, Mach-O its `$tlv$init`.
            SymbolKind::Tls => {
                let (offset, size) = match symbols.tls_sizes.get(&format!("{name}$tlv$init")) {
                    Some(&found) => found,
                    None if sym.size > 0 => (sym.address, sym.size),
                    None if !symbols.tls_sizes.is_empty() => (0, symbols.tls_data.len() as u64),
                    None => (sym.address, 8),
                };
                let (start, end) = (offset as usize, (offset + size) as usize);
                let init = symbols.tls_data.get(start..end).map_or_else(|| vec![0; size as usize], <[u8]>::to_vec);
                let tls = obj.section_id(StandardSection::Tls);
                let id = obj.add_symbol(Symbol {
                    name: bare,
                    value: 0,
                    size: 0,
                    scope: SymbolScope::Linkage,
                    kind: SymbolKind::Tls,
                    weak: false,
                    section: SymbolSection::Undefined,
                    flags: SymbolFlags::None,
                });
                obj.add_symbol_data(id, tls, &init, size.clamp(1, 8).next_power_of_two());
            }
            kind => {
                // The stub defines these globally (the patch references them
                // undefined), so a binding copied from the fat binary's own
                // symbol — a local static's is local — would place a local
                // among globals; force global binding, keep the visibility.
                let flags = match sym.flags {
                    SymbolFlags::Elf { st_info, st_other } => {
                        SymbolFlags::Elf { st_info: (object::elf::STB_GLOBAL << 4) | (st_info & 0xf), st_other }
                    }
                    other => other,
                };
                obj.add_symbol(Symbol {
                    name: bare,
                    value: at,
                    size: 0,
                    scope: SymbolScope::Linkage,
                    // Mach-O's statics read as unknown.
                    kind: if kind == SymbolKind::Unknown { SymbolKind::Data } else { kind },
                    weak: sym.weak,
                    section: SymbolSection::Absolute,
                    flags,
                });
            }
        }
    }
    obj.write().map_err(|e| format!("hot-patch: the stub: {e}"))
}

/// An object file's symbols: those it uses undefined, those it defines.
fn used(bytes: &[u8], undefined: &mut HashSet<String>, defined: &mut HashSet<String>) -> Result<(), object::Error> {
    let file = object::File::parse(bytes)?;
    for s in file.symbols() {
        if s.is_undefined() {
            undefined.insert(s.name()?.to_string());
        } else if s.is_global() {
            defined.insert(s.name()?.to_string());
        }
    }
    Ok(())
}

/// Old address → new, of each symbol the patch library `lib` and the fat
/// binary share; `main` of each for the app to find both in memory.
fn jump_table(lib: &Path, symbols: &Symbols) -> Result<JumpTable, String> {
    let fail = |e: &dyn std::fmt::Display| format!("hot-patch: {}: {e}", lib.display());
    let bytes = std::fs::read(lib).map_err(|e| fail(&e))?;
    let obj = object::File::parse(&*bytes).map_err(|e| fail(&e))?;
    if let object::File::Pe64(pe) = &obj {
        let size = u64::from(pe.nt_headers().optional_header.size_of_image.get(object::LittleEndian));
        if size > PATCH_ROOM {
            return Err(fail(&format!("{size} bytes: more than the {PATCH_ROOM} the app has free to load it")));
        }
    }
    let new = obj.symbol_map();
    let mut map = aio_hotpatch::AddressMap::default();
    let mut new_base = None;
    for s in new.symbols() {
        if s.name() == MAIN {
            new_base = Some(s.address());
        }
        if let Some(old) = symbols.table.get(s.name()).filter(|o| !o.undefined && s.address() != 0) {
            map.insert(old.address, s.address());
        }
    }
    Ok(JumpTable {
        lib: lib.to_path_buf(),
        map,
        aslr_reference: symbols.table[MAIN].address,
        new_base_address: new_base.ok_or_else(|| fail(&format!("no `{MAIN}` symbol")))?,
        ifunc_count: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_response_file_reads_back_as_written() {
        let args = ["-o", "/a b/c", r"x\y", "q\"uo'te", "", "-Wl,--as-needed"].map(String::from);
        let body: Vec<String> = args.iter().map(|a| response_arg(a)).collect();
        let back = response_args(&body.join("\n"));
        assert_eq!(back, args.iter().filter(|a| !a.is_empty()).cloned().collect::<Vec<_>>());
        // rustc's own: one per line, spaces and backslashes escaped.
        assert_eq!(response_args("/t/a\\ b.o\n-lc\n"), ["/t/a b.o", "-lc"]);
    }

    #[test]
    fn dep_info_lists_each_dependency_once_in_the_crate_folder() {
        let d = "/t/deps/app-1.d: src/main.rs src/a\\ b.rs\n\nsrc/main.rs:\nsrc/a\\ b.rs:\n# env-dep:X=1\n";
        let deps = dep_info(d, Path::new("/w"));
        // Absolute as the OS makes it (a drive on Windows).
        assert_eq!(deps, ["/w/src/main.rs", "/w/src/a b.rs"].map(|p| canonical(Path::new(p))));
    }

    fn call(args: &[&str]) -> Rustc {
        Rustc {
            args: args.iter().map(|a| a.to_string()).collect(),
            envs: vec![("K".into(), "V".into())],
            cwd: "/w".into(),
        }
    }

    #[test]
    fn a_recorded_call_names_its_outputs_and_libraries() {
        let c = call(&[
            "rustc",
            "--crate-name",
            "lib_a",
            "--crate-type",
            "lib",
            "-C",
            "extra-filename=-ab12",
            "--out-dir",
            "/t/deps",
            "--extern",
            "lib_b=/t/deps/liblib_b-cd34.rmeta",
            "-Clinker=/x/ax",
            "-C",
            "linker=clang",
        ]);
        assert_eq!(c.lib_stem(), "liblib_a-ab12");
        assert_eq!(c.rlib(), Some(PathBuf::from("/t/deps/liblib_a-ab12.rlib")));
        assert_eq!(c.externs(), HashSet::from(["liblib_b-cd34".to_string()]));
        assert_eq!(linker(&c, Path::new("/x/ax")), "clang");
        let default = if COFF { "x86_64-w64-mingw32-gcc" } else { "cc" };
        assert_eq!(linker(&call(&["rustc", "-Clinker=/x/ax"]), Path::new("/x/ax")), default);
        let back = Rustc::from_json(&c.to_json()).unwrap();
        assert_eq!((back.args, back.envs, back.cwd), (c.args, c.envs, c.cwd));
    }

    #[test]
    fn link_exe_response_files_and_outputs_read_as_rustc_writes_them() {
        // rustc's: UTF-16 with a byte-order mark, an arg a line, quoted.
        let text = "\"/OUT:C:\\t\\app.exe\"\n\"/LIBPATH:C:\\a b\"\n\"q\\\"uo\"\n";
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        let args = response_file(&bytes);
        assert_eq!(args, [r"/OUT:C:\t\app.exe", r"/LIBPATH:C:\a b", "q\"uo"]);
        assert_eq!(output_arg(&args), Some(r"C:\t\app.exe"));
        assert_eq!(output_arg(&["-o".into(), "/t/app".into()]), Some("/t/app"));
        // Without a mark: GNU's.
        assert_eq!(response_file(b"/t/a\\ b.o\n-lc\n"), ["/t/a b.o", "-lc"]);
        // What lld-link reads back: backslashes doubled before a quote only.
        assert_eq!(msvc_response_arg(r"C:\a b\x.o"), r#""C:\a b\x.o""#);
        assert_eq!(msvc_response_arg(r#"a\"b"#), r#""a\\\"b""#);
        assert_eq!(msvc_response_arg(r"C:\dir\"), r#""C:\dir\\""#);
    }

    #[test]
    fn the_windows_patch_link_is_a_fixed_dll_with_the_apps_system_libraries() {
        let msvc = [
            "/NOLOGO",
            r"C:\t\a.rcgu.o",
            r"C:\t\libx.rlib",
            "kernel32.lib",
            "/LIBPATH:C:\\l",
            "/OUT:C:\\t\\app.exe",
            "/DEBUG",
        ]
        .map(String::from);
        let thin = coff_thin_link_args(&msvc, true, 0x7ff6_1000_0000);
        for kept in
            ["/DLL", "/FIXED", "/BASE:0x7ff610000000", "/ENTRY:aio_patch_entry", "/DEBUG:SYMTAB", "kernel32.lib"]
        {
            assert!(thin.contains(&kept.to_string()), "{kept}: {thin:?}");
        }
        assert!(thin.iter().any(|a| a.starts_with("/LIBPATH:")), "{thin:?}");
        assert!(!thin.iter().any(|a| a.ends_with(".o") || a.ends_with(".rlib") || a.starts_with("/OUT")), "{thin:?}");
        let gnu = ["-m64", "crt2.o", "a.rcgu.o", "-L", "C:/l", "-lkernel32", "-o", "app.exe", "-Wl,--gc-sections"]
            .map(String::from);
        let thin = coff_thin_link_args(&gnu, false, 0x1000_0000);
        for kept in [
            "-shared",
            "-Wl,--disable-reloc-section",
            "-Wl,--image-base=0x10000000",
            "-L",
            "C:/l",
            "-lkernel32",
            "-m64",
        ] {
            assert!(thin.contains(&kept.to_string()), "{kept}: {thin:?}");
        }
        assert!(!thin.iter().any(|a| a.ends_with(".o") || a == "-o" || a.contains("gc-sections")), "{thin:?}");
    }

    /// A COFF object as rustc's for Windows x86_64: code reaching a
    /// writable static by name and through its section, a thread-local
    /// (`SECREL`), read-only data and an undefined writable static.
    fn coff_object() -> Vec<u8> {
        use object::pe::{IMAGE_REL_AMD64_ADDR64, IMAGE_REL_AMD64_REL32, IMAGE_REL_AMD64_SECREL};
        use object::write::{Object, Relocation};
        let mut obj = Object::new(object::BinaryFormat::Coff, object::Architecture::X86_64, object::Endianness::Little);
        let text = obj.add_section(vec![], b".text".to_vec(), object::SectionKind::Text);
        obj.append_section_data(text, &[0; 32], 16);
        let data = obj.add_section(vec![], b".data".to_vec(), object::SectionKind::Data);
        obj.append_section_data(data, &[0; 16], 8);
        let rdata = obj.add_section(vec![], b".rdata".to_vec(), object::SectionKind::ReadOnlyData);
        obj.append_section_data(rdata, &[0; 8], 8);
        let tls = obj.add_section(vec![], b".tls$".to_vec(), object::SectionKind::Data);
        obj.append_section_data(tls, &[0; 8], 8);
        let def = |obj: &mut Object, name: &str, section, value, scope| {
            obj.add_symbol(Symbol {
                name: name.into(),
                value,
                size: 0,
                kind: SymbolKind::Data,
                scope,
                weak: false,
                section: SymbolSection::Section(section),
                flags: SymbolFlags::None,
            })
        };
        let s = def(&mut obj, "S", data, 0, SymbolScope::Compilation);
        def(&mut obj, "G", data, 8, SymbolScope::Linkage);
        let r = def(&mut obj, "R", rdata, 0, SymbolScope::Compilation);
        let t = def(&mut obj, "T", tls, 0, SymbolScope::Compilation);
        let u = obj.add_symbol(Symbol {
            name: b"U".to_vec(),
            value: 0,
            size: 0,
            kind: SymbolKind::Data,
            scope: SymbolScope::Linkage,
            weak: false,
            section: SymbolSection::Undefined,
            flags: SymbolFlags::None,
        });
        let data_section = obj.section_symbol(data);
        let reloc = |obj: &mut Object, offset, symbol, addend, typ| {
            let flags = object::RelocationFlags::Coff { typ };
            obj.add_relocation(text, Relocation { offset, symbol, addend, flags }).unwrap();
        };
        reloc(&mut obj, 0, s, 0, IMAGE_REL_AMD64_REL32);
        reloc(&mut obj, 4, t, 0, IMAGE_REL_AMD64_SECREL);
        reloc(&mut obj, 8, r, 0, IMAGE_REL_AMD64_REL32);
        reloc(&mut obj, 12, u, 0, IMAGE_REL_AMD64_REL32);
        // The writer stores a REL32 addend plus 4: the field holds G's offset, 8.
        reloc(&mut obj, 16, data_section, 4, IMAGE_REL_AMD64_REL32);
        reloc(&mut obj, 24, s, 0, IMAGE_REL_AMD64_ADDR64);
        obj.write().unwrap()
    }

    fn fat_symbols(names: &[(&str, SymbolKind, bool, u64)]) -> Symbols {
        let sym = |&(_, kind, writable, address): &(&str, SymbolKind, bool, u64)| Sym {
            address,
            kind,
            undefined: false,
            weak: false,
            size: 0,
            flags: SymbolFlags::None,
            writable,
        };
        Symbols {
            table: names.iter().map(|n| (n.0.to_string(), sym(n))).collect(),
            tls_data: vec![],
            tls_sizes: HashMap::new(),
            tls_block: None,
            tls_start: 0x1_4000_8000,
        }
    }

    #[test]
    fn a_windows_patch_object_uses_the_binarys_writable_statics_and_thread_locals() {
        use object::read::coff::CoffFile;
        use object::{ObjectSection, RelocationTarget};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.rcgu.o");
        std::fs::write(&path, coff_object()).unwrap();
        let fat = fat_symbols(&[
            ("S", SymbolKind::Data, true, 0x1_4000_1000),
            ("G", SymbolKind::Data, true, 0x1_4000_1008),
            ("R", SymbolKind::Data, false, 0x1_4000_2000),
            ("U", SymbolKind::Data, true, 0x1_4000_3000),
            ("T", SymbolKind::Tls, true, 0x1_4000_8010),
        ]);
        let copy = rewrite_coff(&path, &fat, &dir.path().join("rewrite")).unwrap().expect("rewritten");
        let bytes = std::fs::read(copy).unwrap();
        let obj = CoffFile::<&[u8]>::parse(&*bytes).unwrap();
        let text = obj.section_by_name(".text").unwrap();
        let code = text.data().unwrap();
        let named: Vec<(u64, String, u16)> = text
            .relocations()
            .map(|(at, r)| {
                let name = match r.target() {
                    RelocationTarget::Symbol(i) => obj.symbol_by_index(i).unwrap().name().unwrap().to_string(),
                    _ => String::new(),
                };
                let object::RelocationFlags::Coff { typ } = r.flags() else { panic!() };
                (at, name, typ)
            })
            .collect();
        let at = |o: u64| named.iter().find(|(a, ..)| *a == o).cloned().unwrap();
        // A writable static, by name, by its section and by address: the binary's.
        assert_eq!(at(0).1, "aio$fat$S");
        assert_eq!((at(16).1.as_str(), &code[16..20]), ("aio$fat$G", &[0u8; 4][..]));
        assert_eq!(at(24).1, "aio$fat$S");
        // Undefined, but the binary's state: the same.
        assert_eq!(at(12).1, "aio$fat$U");
        // Read-only data: the patch's own copy.
        assert_eq!(at(8).1, "R");
        // The thread-local: its offset in the binary's block, its relocation gone.
        assert!(!named.iter().any(|(a, ..)| *a == 4), "{named:?}");
        assert_eq!(u32::from_le_bytes(code[4..8].try_into().unwrap()), 0x10);
        assert_eq!(named.len(), 5, "{named:?}");

        // A thread-local the binary hasn't: a rebuild's.
        let fat = fat_symbols(&[("S", SymbolKind::Data, true, 0x1_4000_1000)]);
        let err = rewrite_coff(&path, &fat, &dir.path().join("rewrite")).unwrap_err();
        assert!(err.contains("thread-local T is new"), "{err}");
    }

    #[test]
    fn the_patch_link_keeps_search_paths_and_system_libraries_only() {
        let args = [
            "-m64",
            "/tmp/rustc1/symbols.o",
            "/t/a.rcgu.o",
            "/t/libx.rlib",
            "-L",
            "/t/deps",
            "-lc",
            "-B/r/gcc-ld",
            "-fuse-ld=lld",
            "-o",
            "/t/app",
            "-pie",
        ]
        .map(String::from);
        let thin = thin_link_args(&args);
        assert!(
            !thin.iter().any(|a| a.ends_with(".o") || a.ends_with(".rlib") || a == "-o" || a == "-pie"),
            "{thin:?}"
        );
        for kept in ["-m64", "-L", "/t/deps", "-lc"] {
            assert!(thin.contains(&kept.to_string()), "{kept}: {thin:?}");
        }
    }
}

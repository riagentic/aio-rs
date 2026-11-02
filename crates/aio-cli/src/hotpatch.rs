//! `ax dev`'s hot-patching, without dx: a "fat" build of the app (every
//! dependency's code kept, every workspace crate's rustc call recorded),
//! then per edit a "thin" build: the edited crates' rustc calls replayed,
//! the app's crate linked into a small shared library whose missing symbols
//! jump into the running fat binary, and a jump table (old address → new)
//! for subsecond's `apply_patch` in the app.
//! Ported from dioxus-cli 0.7.10 (MIT/Apache-2.0): build/{link,patch,request}.rs, rustcwrapper.rs.
//!
//! ax takes two roles in the fat build, chosen by env var at the top of
//! `main` ([`hook`]): cargo's `RUSTC_WORKSPACE_WRAPPER` (records each
//! workspace crate's rustc args, env and folder, then runs rustc) and the
//! app crate's linker (records the link args and writes an empty object
//! where the binary goes: the real link is [`fat_build`]'s, with every
//! dependency object in it).
//!
//! Linux (ELF, x86_64 and aarch64) and macOS (Mach-O, arm64 and x86_64).
//! Windows (PE, symbols in PDB files) is not ported: [`fat_build`] says so.
use object::write::{MachOBuildVersion, SectionId, StandardSection, Symbol, SymbolId, SymbolSection};
use object::{Object, ObjectSection, ObjectSymbol, SymbolFlags, SymbolKind, SymbolScope};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
pub use subsecond_types::JumpTable;

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

/// As the app crate's linker: its args (response files read) to `file`,
/// and an empty object where the binary goes, for rustc to find.
fn record_link(file: &Path) -> Result<(), String> {
    let mut args = vec![];
    for a in std::env::args_os().skip(1) {
        let a = a.to_string_lossy().into_owned();
        match a.strip_prefix('@') {
            Some(rsp) => args.extend(response_args(&std::fs::read_to_string(rsp).map_err(|e| format!("{rsp}: {e}"))?)),
            None => args.push(a),
        }
    }
    write_new(file, serde_json::Value::from(args.clone()).to_string().as_bytes())
        .map_err(|e| format!("{}: {e}", file.display()))?;
    let out = args.iter().position(|a| a == "-o").and_then(|i| args.get(i + 1)).ok_or("no -o in the link args")?;
    let (format, arch) = host()?;
    let empty = object::write::Object::new(format, arch, object::Endianness::Little);
    write_new(Path::new(out), &empty.write().map_err(|e| e.to_string())?).map_err(|e| format!("{out}: {e}"))
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
        other => return Err(format!("hot-patch: not supported on {other} yet")),
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => object::Architecture::X86_64,
        "aarch64" => object::Architecture::Aarch64,
        other => return Err(format!("hot-patch: not supported on {other} yet")),
    };
    Ok((format, arch))
}

const MACHO: bool = cfg!(target_os = "macos");

/// The symbol at the same place in the fat binary and each patch: `main`
/// (subsecond's `aslr_reference`), `_main` in Mach-O's symbol table.
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
    /// For the app: subsecond's `apply_patch`.
    pub table: JumpTable,
    pub times: Times,
}

/// Build the app fat: in ax dev's layout (`req.cargo_args`), every
/// workspace crate's rustc call recorded, then linked with every object of
/// every dependency built here, `main` exported. The binary is cargo's.
pub fn fat_build(req: &Request) -> Result<Fat, String> {
    host()?;
    let started = Instant::now();
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
    let mut exe = cargo(req, &rustc_dir, &link_file)?;
    if !link_file.is_file() {
        bust(&exe.with_file_name(".fingerprint"), &req.bin, &unrecorded);
        exe = cargo(req, &rustc_dir, &link_file)?;
    }
    let link_args = read_link_args(&link_file)?;
    let build = started.elapsed();
    let crates = recorded(&rustc_dir, &tip, &link_args);
    let app = crates.get(&format!("{tip}.bin")).ok_or_else(|| format!("hot-patch: no rustc call of {tip} recorded"))?;
    let linker = linker(app, &req.ax);
    let started = Instant::now();
    fat_link(&link_args, app, &linker, &exe, &target)?;
    let link = started.elapsed();
    let symbols = Symbols::of(&exe)?;
    Ok(Fat { exe, build, link, scope, tip, crates, linker, symbols, modified: BTreeSet::new(), patches: 0 })
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

/// The fat build's cargo: ax as rustc's wrapper and the app crate's
/// linker, the app crate's objects kept (`-Csave-temps`) and none of its
/// code dropped (`-Clink-dead-code`). The binary, as cargo says it wrote it.
fn cargo(req: &Request, rustc_dir: &Path, link_file: &Path) -> Result<PathBuf, String> {
    let mut child = Command::new("cargo")
        .args(["rustc", "-p", &req.package, "--bin", &req.bin, "--message-format=json-render-diagnostics"])
        .args(&req.cargo_args)
        .args(["--", "-Csave-temps=true", "-Clink-dead-code"])
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

fn read_link_args(file: &Path) -> Result<Vec<String>, String> {
    let text =
        std::fs::read_to_string(file).map_err(|e| format!("hot-patch: no link args ({}): {e}", file.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("hot-patch: {}: {e}", file.display()))
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
/// settings name, else `cc` (pure).
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
    found.unwrap_or("cc").to_string()
}

/// The fat link: the app crate's link, with every object of every library
/// built in `target` in place of the library (the standard library's stay
/// libraries), `main` exported, into `exe`.
fn fat_link(link_args: &[String], app: &Rustc, linker: &str, exe: &Path, target: &Path) -> Result<(), String> {
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
    let at = args.iter().rposition(|a| a.ends_with(".o")).map_or(0, |i| i + 1);
    let inserted: Vec<String> = objects.iter().chain(&libraries).map(|p| p.display().to_string()).collect();
    args.splice(at..at, inserted);
    args.push(if MACHO { "-Wl,-exported_symbol,_main" } else { "-Wl,--export-dynamic-symbol,main" }.into());
    args.extend(["-o".to_string(), exe.display().to_string()]);
    // cargo's copy of the empty object rustc linked: replaced, not written through.
    let _ = std::fs::remove_file(exe);
    let rsp = exe.with_extension("hotpatch-link");
    let body: Vec<String> = args.iter().map(|a| response_arg(a)).collect();
    std::fs::write(&rsp, body.join("\n")).map_err(|e| format!("hot-patch: {}: {e}", rsp.display()))?;
    let out = Command::new(linker)
        .arg(format!("@{}", rsp.display()))
        .current_dir(&app.cwd)
        .env_clear()
        .envs(app.envs.iter().cloned())
        .output()
        .map_err(|e| format!("hot-patch: can't run {linker}: {e}"))?;
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
    tls_block: Option<u64>,
}

impl Symbols {
    fn of(exe: &Path) -> Result<Symbols, String> {
        let fail = |e: &dyn std::fmt::Display| format!("hot-patch: {}: {e}", exe.display());
        let bytes = std::fs::read(exe).map_err(|e| fail(&e))?;
        let obj = object::File::parse(&*bytes).map_err(|e| fail(&e))?;
        let table: HashMap<String, Sym> = obj
            .symbols()
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
                };
                Some((s.name().ok()?.to_string(), sym))
            })
            .collect();
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
        Ok(Symbols { table, tls_data, tls_sizes, tls_block: tls_block(&bytes) })
    }
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

/// Linux x86_64: object `path` of the app's crate with its thread-local
/// accesses made the running binary's (a copy in `dir`, if it has any).
/// rustc builds the app's crate as an executable: its own thread-locals at
/// fixed offsets from the thread pointer (`TPOFF32`), others' through the
/// GOT (`GOTTPOFF`), neither of which a shared library may hold. Each is
/// made the offset the fat binary has, as a linker relaxes initial-exec to
/// local-exec, so the patch reads and writes the app's own thread-locals,
/// their values kept. A thread-local the fat binary lacks: an error.
fn relaxed_tls(path: &Path, symbols: &Symbols, dir: &Path) -> Result<Option<PathBuf>, String> {
    use object::elf::{R_X86_64_GOTTPOFF, R_X86_64_TPOFF32, Rela64, SHT_RELA, SHT_SYMTAB};
    use object::read::elf::{FileHeader, Rela, SectionHeader, Sym as _};
    let fail = |e: &dyn std::fmt::Display| format!("hot-patch: {}: {e}", path.display());
    let data = std::fs::read(path).map_err(|e| fail(&e))?;
    let header = object::elf::FileHeader64::<object::Endianness>::parse(&*data).map_err(|e| fail(&e))?;
    let endian = header.endian().map_err(|e| fail(&e))?;
    let sections = header.sections(endian, &*data).map_err(|e| fail(&e))?;
    let syms = sections.symbols(endian, &*data, SHT_SYMTAB).map_err(|e| fail(&e))?;
    let mut out: Option<Vec<u8>> = None;
    for rela_section in sections.iter().filter(|s| s.sh_type(endian) == SHT_RELA) {
        let relas: &[Rela64<object::Endianness>] = rela_section.data_as_array(endian, &*data).map_err(|e| fail(&e))?;
        let target =
            sections.section(object::SectionIndex(rela_section.sh_info(endian) as usize)).map_err(|e| fail(&e))?;
        let (code, table) = (target.sh_offset(endian) as usize, rela_section.sh_offset(endian) as usize);
        for (i, r) in relas.iter().enumerate() {
            let kind = r.r_type(endian, false);
            if kind != R_X86_64_TPOFF32 && kind != R_X86_64_GOTTPOFF {
                continue;
            }
            let sym = syms.symbol(object::SymbolIndex(r.r_sym(endian, false) as usize)).map_err(|e| fail(&e))?;
            let name = String::from_utf8_lossy(sym.name(endian, syms.strings()).map_err(|e| fail(&e))?).into_owned();
            let found = symbols.table.get(&name).filter(|s| s.kind == SymbolKind::Tls && !s.undefined);
            let (Some(found), Some(block)) = (found, symbols.tls_block) else {
                return Err(format!("hot-patch: thread-local {name} is new (the running binary has none): rebuild"));
            };
            let at = code + r.r_offset(endian) as usize;
            let bytes = out.get_or_insert_with(|| data.clone());
            if kind == R_X86_64_GOTTPOFF {
                // `mov x@gottpoff(%rip), %reg` → `mov $off, %reg`;
                // `add x@gottpoff(%rip), %reg` → `add $off, %reg`.
                let (rex, op, modrm) =
                    (bytes.get(at.wrapping_sub(3)), bytes.get(at.wrapping_sub(2)), bytes.get(at.wrapping_sub(1)));
                let (Some(&rex), Some(&op), Some(&modrm)) = (rex, op, modrm) else {
                    return Err(fail(&"GOTTPOFF at the start"));
                };
                if rex & 0xf8 != 0x48 || modrm & 0xc7 != 0x05 || (op != 0x8b && op != 0x03) {
                    return Err(fail(&format!(
                        "thread-local {name}: an access ax can't relax ({rex:#x} {op:#x} {modrm:#x})"
                    )));
                }
                let reg = (modrm >> 3) & 7;
                bytes[at - 3] = 0x48 | ((rex >> 2) & 1);
                bytes[at - 2] = if op == 0x8b { 0xc7 } else { 0x81 };
                bytes[at - 1] = 0xc0 | reg;
            }
            // The relocation's own value (`S + A - TP`); GOTTPOFF's addend
            // has the `-4` of a rip-relative operand, taken back.
            let addend = r.r_addend(endian) + if kind == R_X86_64_GOTTPOFF { 4 } else { 0 };
            let value = i32::try_from(found.address as i64 - block as i64 + addend)
                .map_err(|_| fail(&format!("thread-local {name}: offset out of range")))?;
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
            // The relocation itself: R_X86_64_NONE, its symbol kept.
            let info = table + i * std::mem::size_of::<Rela64<object::Endianness>>() + 8;
            let sym_bits = r.r_info(endian, false) & !0xffff_ffff;
            bytes[info..info + 8].copy_from_slice(&sym_bits.to_le_bytes());
        }
    }
    let Some(bytes) = out else { return Ok(None) };
    std::fs::create_dir_all(dir).map_err(|e| fail(&e))?;
    let copy = dir.join(path.file_name().unwrap_or_default());
    std::fs::write(&copy, bytes).map_err(|e| fail(&e))?;
    Ok(Some(copy))
}

/// Patch the running fat binary with the edits to `changed` (source files
/// of the app's workspace crates): the edited crates (and the crates built
/// on them) and the app's own crate built again, linked into a new shared
/// library, the symbols it misses resolved into the running binary
/// (`aslr_reference`: subsecond's, from the app). Any file no crate builds
/// from (a manifest, a build script): an error, for a rebuild.
pub fn patch(fat: &mut Fat, aslr_reference: u64, changed: &[PathBuf]) -> Result<Patch, String> {
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
    let link_args = read_link_args(&link_file)?;
    let compile = started.elapsed();
    let thin = thin_link(fat, &app, &link_args, aslr_reference);
    remove_temps(&link_args, &app);
    let _ = std::fs::remove_dir_all(fat.scope.join("tls"));
    let _ = std::fs::remove_file(fat.scope.join("stub.o"));
    // rustc's output, the empty object: a stale file of that name confuses
    // the dynamic loader on a later patch (dx's finding).
    if let Some(o) = link_args.iter().position(|a| a == "-o").and_then(|i| link_args.get(i + 1)) {
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
    link_args: &[String],
    aslr_reference: u64,
) -> Result<(PathBuf, Duration, Duration), String> {
    let at = Instant::now();
    let mut objects = vec![];
    for o in link_args.iter().filter(|a| a.ends_with(".rcgu.o")).map(PathBuf::from) {
        let relaxed = match (MACHO, std::env::consts::ARCH) {
            (false, "x86_64") => relaxed_tls(&o, &fat.symbols, &fat.scope.join("tls"))?,
            _ => None,
        };
        objects.push(relaxed.unwrap_or(o));
    }
    objects.sort();
    let wanted: HashMap<String, PathBuf> =
        fat.modified.iter().filter_map(|k| Some((fat.crates[k].lib_stem(), fat.crates[k].rlib()?))).collect();
    // The libraries in the fat link's order.
    let linked = link_args.iter().filter(|a| a.ends_with(".rlib")).filter_map(|a| Path::new(a).file_stem()?.to_str());
    objects.extend(linked.filter_map(|s| wanted.get(s).cloned()));
    let stub = fat.scope.join("stub.o");
    let bytes = stub_object(&fat.symbols, &objects, aslr_reference)?;
    std::fs::write(&stub, bytes).map_err(|e| format!("hot-patch: {}: {e}", stub.display()))?;
    let stub_time = at.elapsed();

    let at = Instant::now();
    fat.patches += 1;
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis();
    let lib =
        fat.scope.join(format!("lib{}-patch-{ms}-{}.{}", fat.tip, fat.patches, if MACHO { "dylib" } else { "so" }));
    let dylibs = link_args.iter().filter(|a| a.ends_with(".so") || a.ends_with(".dylib"));
    let out = Command::new(&fat.linker)
        .args(&objects)
        .arg(&stub)
        .args(dylibs)
        .args(thin_link_args(link_args))
        .arg("-o")
        .arg(&lib)
        .current_dir(&app.cwd)
        .env_clear()
        .envs(app.envs.iter().cloned())
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
/// as the fat binary's.
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
    let mut names: Vec<&String> = undefined.difference(&defined).collect();
    names.sort();
    for name in names {
        let Some(sym) = symbols.table.get(name.as_str()) else { continue };
        // Imports of the fat binary (address 0 until loaded): the system's,
        // which the patch links itself.
        if sym.undefined {
            continue;
        }
        // Mach-O's leading underscore: the writer adds it again.
        let bare = name.as_bytes()[usize::from(MACHO)..].to_vec();
        let at = sym.address + slide;
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
                obj.add_symbol(Symbol {
                    name: bare,
                    value: at,
                    size: 0,
                    scope: SymbolScope::Linkage,
                    // Mach-O's statics read as unknown.
                    kind: if kind == SymbolKind::Unknown { SymbolKind::Data } else { kind },
                    weak: sym.weak,
                    section: SymbolSection::Absolute,
                    flags: sym.flags,
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
/// binary share; `main` of each for subsecond to find both in memory.
fn jump_table(lib: &Path, symbols: &Symbols) -> Result<JumpTable, String> {
    let fail = |e: &dyn std::fmt::Display| format!("hot-patch: {}: {e}", lib.display());
    let bytes = std::fs::read(lib).map_err(|e| fail(&e))?;
    let obj = object::File::parse(&*bytes).map_err(|e| fail(&e))?;
    let new = obj.symbol_map();
    let mut map = subsecond_types::AddressMap::default();
    let mut new_base = None;
    for s in new.symbols() {
        if s.name() == MAIN {
            new_base = Some(s.address());
        }
        if let Some(old) = symbols.table.get(s.name()).filter(|o| !o.undefined) {
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
        assert_eq!(deps, [PathBuf::from("/w/src/main.rs"), PathBuf::from("/w/src/a b.rs")]);
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
        assert_eq!(linker(&call(&["rustc", "-Clinker=/x/ax"]), Path::new("/x/ax")), "cc");
        let back = Rustc::from_json(&c.to_json()).unwrap();
        assert_eq!((back.args, back.envs, back.cwd), (c.args, c.envs, c.cwd));
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

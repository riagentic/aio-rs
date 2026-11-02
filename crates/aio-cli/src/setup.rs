//! `ax setup`: what a machine needs to make apps. Checked: Rust and the C
//! toolchain it links with (each missing one said with the command that
//! gets it), and the optional tools. Fetched prebuilt and checked before
//! anything runs: the shared Electron runtime (then started once, so a
//! missing system library is named) and the wasm-bindgen of aio's lock the
//! web build binds with (its release binary, SHA-256 pinned here; minutes
//! of `cargo install` saved). aio's Rust toolchain is checked for the targets
//! Android and the web build with. docs/basics/setup.md lists it all per OS.
use std::io::Read;
use std::path::{Path, PathBuf};

/// `bytes` as the executable `to` (written aside, then moved in place).
fn write_exe(bytes: &[u8], to: &Path) -> Result<(), String> {
    let tmp = to.with_extension("partial");
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, to).map_err(|e| format!("{}: {e}", to.display()))
}

/// The file of tar archive `tar` whose path ends in `/name` (pure; a
/// plain ustar, as wasm-bindgen's release archives are).
pub fn tar_entry<'a>(tar: &'a [u8], name: &str) -> Option<&'a [u8]> {
    let field = |b: &'a [u8]| std::str::from_utf8(b).ok().map(|s| s.trim_matches(['\0', ' ']));
    let mut at = 0;
    while let Some(h) = tar.get(at..at + 512).filter(|h| h.iter().any(|&b| b != 0)) {
        let size = usize::from_str_radix(field(&h[124..136])?, 8).ok()?;
        let path = field(&h[..100])?;
        let body = at + 512;
        if h[156] != b'5' && (path == name || path.ends_with(&format!("/{name}"))) {
            return tar.get(body..body + size);
        }
        at = body + size.div_ceil(512) * 512;
    }
    None
}

/// The `wasm-bindgen` of release archive `tgz`, written to `to` (executable).
fn unpack_wasm_bindgen(tgz: &Path, to: &Path) -> Result<(), String> {
    let file = std::fs::File::open(tgz).map_err(|e| e.to_string())?;
    let mut tar = Vec::new();
    flate2::read::GzDecoder::new(file).read_to_end(&mut tar).map_err(|e| format!("{}: {e}", tgz.display()))?;
    let name = format!("wasm-bindgen{}", std::env::consts::EXE_SUFFIX);
    write_exe(tar_entry(&tar, &name).ok_or_else(|| format!("{}: no {name} in it", tgz.display()))?, to)
}

/// A tool at `to`: there already, else fetched from `url` and checked
/// against `sha` before `unpack` takes it out (a mismatch leaves nothing behind).
pub fn fetch_tool(
    url: &str,
    sha: &str,
    to: &Path,
    unpack: fn(&Path, &Path) -> Result<(), String>,
) -> Result<(), String> {
    if to.is_file() {
        return Ok(());
    }
    let dir = to.parent().ok_or("the tool has no folder")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tmp = dir.join(format!(".download.partial-{}", std::process::id()));
    let got = aio_shell::runtime::download(url, &tmp, &mut |_, _| true).map_err(|e| e.to_string());
    let r = match got {
        Ok(got) if got == sha => unpack(&tmp, to),
        Ok(got) => Err(format!("{url}: SHA-256 {got}, expected {sha}: not installed")),
        Err(e) => Err(e),
    };
    _ = std::fs::remove_file(&tmp);
    r
}

/// The wasm-bindgen aio's lock and aio-rs pin (`=` in its Cargo.toml), so
/// every app's lock has it too: the web build's CLI must be the same version.
pub const WASM_BINDGEN: &str = "0.2.129";

/// wasm-bindgen's release archive per host (OS, arch) and its SHA-256: the
/// `wasm-bindgen-<v>-<triple>.tar.gz` assets of wasm-bindgen [`WASM_BINDGEN`].
const BINDGEN: &[(&str, &str, &str, &str)] = &[
    (
        "linux",
        "x86_64",
        "x86_64-unknown-linux-musl",
        "82d12bb940e2d4e72e0d5605387fc1b8ca179044e012b620f0ce4e7440e8320e",
    ),
    (
        "linux",
        "aarch64",
        "aarch64-unknown-linux-musl",
        "2ed4351c35dd9440308bbb02767d47ea278efe851a52465300f3c94f5b6c2a87",
    ),
    ("macos", "x86_64", "x86_64-apple-darwin", "7e028879a68ec53dae14048b55cc5215f6df75b17a9c1426fac8fa086d3202fd"),
    ("macos", "aarch64", "aarch64-apple-darwin", "81d4a23d56b3c3eb8187658329116d50e0b228a93b343825fb71f70179051cd1"),
    ("windows", "x86_64", "x86_64-pc-windows-msvc", "79e348c169d0c10c0b287647f1e753d0985353ca96800bc29be8f4261baf13e6"),
];

/// The release archive for `os`/`arch` and its SHA-256 (pure).
pub fn wasm_bindgen_release(os: &str, arch: &str) -> Option<(String, &'static str)> {
    let (.., triple, sha) = BINDGEN.iter().find(|(o, a, ..)| *o == os && *a == arch)?;
    Some((
        format!(
            "https://github.com/wasm-bindgen/wasm-bindgen/releases/download/{WASM_BINDGEN}/wasm-bindgen-{WASM_BINDGEN}-{triple}.tar.gz"
        ),
        sha,
    ))
}

/// Where aio keeps wasm-bindgen `version`: its own, beside the runtime, never on PATH.
pub fn wasm_bindgen_home(tool_home: &Path, version: &str) -> PathBuf {
    tool_home
        .join("tools")
        .join(format!("wasm-bindgen-{version}"))
        .join(format!("wasm-bindgen{}", std::env::consts::EXE_SUFFIX))
}

/// The version a `wasm-bindgen --version` run printed (pure).
fn bindgen_version(said: &str) -> &str {
    said.trim().trim_start_matches("wasm-bindgen ").split(' ').next().unwrap_or("")
}

/// A wasm-bindgen of `version` here (nothing fetched): the one on PATH
/// when it is that version, else aio's own; Err: what was found.
pub fn wasm_bindgen_at(version: &str) -> Result<PathBuf, String> {
    let path = ran("wasm-bindgen", &["--version"]);
    let found = path.as_deref().map(bindgen_version).unwrap_or("none");
    let own = wasm_bindgen_home(&aio::paths::tool_home(), version);
    match () {
        _ if found == version => Ok("wasm-bindgen".into()),
        _ if own.is_file() => Ok(own),
        _ => Err(format!("found {found}")),
    }
}

/// The wasm-bindgen a web build of a lock with `version` runs: one here
/// ([`wasm_bindgen_at`]), else aio's version fetched (prebuilt, checked);
/// another version is said with the command that builds it.
pub fn wasm_bindgen(version: &str) -> Result<PathBuf, String> {
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    wasm_bindgen_at(version).or_else(|found| match wasm_bindgen_release(os, arch) {
        Some((url, sha)) if version == WASM_BINDGEN => {
            let to = wasm_bindgen_home(&aio::paths::tool_home(), version);
            eprintln!("ax: fetching wasm-bindgen {version}…");
            fetch_tool(&url, sha, &to, unpack_wasm_bindgen).map(|()| to)
        }
        _ => Err(format!(
            "the web build needs wasm-bindgen {version} ({found}): cargo install wasm-bindgen-cli --version {version} --locked"
        )),
    })
}

/// The Rust aio builds with (its `rust-toolchain.toml`), pinned in every
/// app `ax create` makes: one compiler, so the shared build folder is reused.
/// Written here, not read from the file, so the published crate builds alone
/// (a test holds the two equal).
pub fn toolchain() -> &'static str {
    "1.98.0"
}

/// The targets an app's other platforms build with: Android's and the web's.
pub const PLATFORM_TARGETS: [&str; 4] =
    ["aarch64-linux-android", "armv7-linux-androideabi", "x86_64-linux-android", "wasm32-unknown-unknown"];

/// aio's toolchain `tc` and its platform targets, from `rustup +tc target
/// list --installed` (none: no such toolchain) (pure).
pub fn targets(tc: &str, installed: Option<&str>) -> Need {
    let found = match installed {
        None => {
            Err(format!("Rust {tc} is not installed: rustup toolchain install {tc} (an app's first build does it too)"))
        }
        Some(have) => {
            let missing: Vec<&str> =
                PLATFORM_TARGETS.into_iter().filter(|t| !have.lines().any(|l| l.trim() == *t)).collect();
            match missing.is_empty() {
                true => Ok(PLATFORM_TARGETS.join(", ")),
                false => Err(format!("rustup target add --toolchain {tc} {}", missing.join(" "))),
            }
        }
    };
    need(format!("Rust {tc} targets"), "ax build --android, ax dev --web / build --web", false, found)
}

/// One thing `ax create → dev → test → build` needs, as found here:
/// what it is, what for, and what was found or how to get it.
#[derive(Debug, PartialEq)]
pub struct Need {
    pub what: String,
    pub why: &'static str,
    /// Without it nothing is made; else one verb is lesser without it.
    pub required: bool,
    /// What was found, or why it is missing and the command that gets it.
    pub found: Result<String, String>,
}

fn need(what: impl Into<String>, why: &'static str, required: bool, found: Result<String, String>) -> Need {
    Need { what: what.into(), why, required, found }
}

/// Rust, from `rustc -vV`'s text (none: no rustc) (pure).
pub fn rust(os: &str, vv: Option<&str>) -> Need {
    let line = |k: &str| vv?.lines().find_map(|l| l.strip_prefix(k)).map(str::trim);
    let fix = match os {
        "windows" => "winget install Rustlang.Rustup (or https://rustup.rs), then a new terminal",
        _ => "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh, then a new terminal",
    };
    let found = match (line("release:"), line("host:")) {
        (Some(v), Some(h)) => Ok(format!("rustc {v}, host {h}")),
        _ => Err(format!("no rustc on PATH; install: {fix}")),
    };
    need("Rust (rustc, cargo)", "builds every app", true, found)
}

/// The host triple `rustc -vV` says (pure).
pub fn host_of(vv: &str) -> Option<&str> {
    vv.lines().find_map(|l| l.strip_prefix("host:")).map(str::trim)
}

/// The C compiler and linker Rust links with and SQLite and ring build
/// with, for host `host` (none yet: rustup's default, msvc on Windows)
/// (pure): `found`, what the probe found.
pub fn c_tools(os: &str, host: &str, found: Option<String>) -> Need {
    let fix = match os {
        "macos" => "the Command Line Tools: xcode-select --install",
        "windows" if !host.ends_with("-gnu") => {
            "the Visual Studio Build Tools with C++: winget install Microsoft.VisualStudio.2022.BuildTools \
             --override \"--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended\""
        }
        "windows" => "a MinGW-w64 gcc on PATH (MSYS2: pacman -S mingw-w64-ucrt-x86_64-gcc), or the msvc toolchain",
        _ => {
            "sudo apt install build-essential (Debian, Ubuntu) · sudo dnf install gcc (Fedora) · sudo pacman -S base-devel (Arch)"
        }
    };
    let found = found.ok_or_else(|| format!("none found; install: {fix}"));
    need("C compiler and linker", "links every app; builds SQLite and ring", true, found)
}

/// The library a program could not load, from what the loader said (pure).
pub fn missing_lib(said: &str) -> Option<&str> {
    let at = said.find("error while loading shared libraries: ")?;
    said[at..].split(": ").nth(1).filter(|l| !l.is_empty())
}

/// The runtime's own check, from its `--version` run (pure): Ok its
/// version, Err what it said (or why it did not start).
pub fn runtime_runs(os: &str, ran: Result<String, String>) -> Need {
    let found = ran.map_err(|said| match (os, missing_lib(&said)) {
        ("linux", Some(lib)) => format!(
            "it can't load {lib}; install Electron's libraries: sudo apt install libgtk-3-0t64 libnss3 libgbm1 \
             libasound2t64 libxss1 libxtst6 (Debian, Ubuntu; on older ones without t64) · sudo dnf install gtk3 nss \
             mesa-libgbm alsa-lib (Fedora)"
        ),
        _ => format!("it does not start: {}", said.trim()),
    });
    need(format!("Electron {} starts", aio_shell::runtime::VERSION), "draws every window and UI test", true, found)
}

/// An optional tool, from its version run (pure): its first line, or the fix.
fn optional(what: &str, why: &'static str, ran: Option<String>, fix: &str) -> Need {
    let found = ran.map(|o| o.lines().next().unwrap_or("").trim().to_string()).ok_or_else(|| format!("install: {fix}"));
    need(what, why, false, found)
}

/// The optional tools for `os`, `ran` running each with its version flag,
/// on a machine with (or without) a desktop to nest windows in.
pub fn optionals(os: &str, desktop: bool, ran: &dyn Fn(&str, &str) -> Option<String>) -> Vec<Need> {
    let pkg = |name: &str| match os {
        "macos" => format!("brew install {name}"),
        "windows" => format!("winget install {}", if name == "git" { "Git.Git" } else { "Gyan.FFmpeg" }),
        _ => format!("sudo apt install {name} (or your distribution's {name})"),
    };
    let mut v = vec![
        optional(
            "git",
            "build numbers (M.m.<commit count>) and releases; without it builds are M.m.0-nogit",
            ran("git", "--version"),
            &pkg("git"),
        ),
        optional("ffmpeg", "videos of UI tests (ax test --video)", ran("ffmpeg", "-version"), &pkg("ffmpeg")),
    ];
    if os == "linux" {
        let (xephyr, xvfb) = (ran("Xephyr", "-version").is_some(), ran("Xvfb", "-help").is_some());
        let fix = "sudo apt install xvfb xserver-xephyr (Debian, Ubuntu) · sudo dnf install xorg-x11-server-Xvfb \
                   xorg-x11-server-Xephyr (Fedora)";
        let used = match (xephyr, xvfb) {
            (true, true) => Some("Xephyr on a desktop (a window holding the app's), else Xvfb (headless)"),
            // Xephyr nests in a desktop: with none here it holds no window.
            (true, false) if !desktop => None,
            (true, false) => Some("Xephyr, on a desktop only: with no DISPLAY nothing nests (Xvfb does)"),
            (false, true) => Some("Xvfb (headless, seen by no one)"),
            (false, false) => None,
        };
        v.push(optional(
            "Xephyr or Xvfb",
            "agents' windows off the person's screen (ax dev from no terminal, --display isolated)",
            used.map(String::from),
            fix,
        ));
    }
    v
}

/// The report (pure): one line per need, then the verdict; Err when a
/// required one is missing (each line says how to get it).
pub fn report(os: &str, arch: &str, needs: &[Need]) -> Result<String, String> {
    let mut lines = vec![format!("ax setup: making apps on {os} {arch}")];
    for n in needs {
        let kind = if n.required { "" } else { " (optional)" };
        lines.push(match &n.found {
            Ok(s) => format!("  ✓ {}: {s}", n.what),
            Err(e) => format!("  {} {}{kind}, {}: {e}", if n.required { "✗" } else { "–" }, n.what, n.why),
        });
    }
    let missing = needs.iter().filter(|n| n.required && n.found.is_err()).count();
    if missing > 0 {
        lines.push(format!("not ready: {missing} missing (✗); each says how to get it, then run ax setup again"));
        return Err(lines.join("\n"));
    }
    lines.push("ready: ax create <name> → ax dev <name> → ax test <name> → ax build <name>".into());
    Ok(lines.join("\n"))
}

/// What `program args…` printed, when it is here to run.
fn ran(program: &str, args: &[&str]) -> Option<String> {
    let o = std::process::Command::new(program).args(args).output().ok()?;
    Some(String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr))
}

/// What this machine has to compile and link C, for host `host`.
fn c_found(os: &str, host: &str) -> Option<String> {
    match os {
        "macos" => ran("xcrun", &["--find", "cc"]).map(|p| format!("Command Line Tools ({})", p.trim())),
        "windows" if !host.ends_with("-gnu") => {
            let pf = std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| r"C:\Program Files (x86)".into());
            let vswhere = format!(r"{pf}\Microsoft Visual Studio\Installer\vswhere.exe");
            let args = ["-latest", "-products", "*", "-requires", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64"];
            let at = ran(&vswhere, &[&args[..], &["-property", "installationPath"]].concat())?;
            Some(at.trim().to_string()).filter(|p| !p.is_empty()).map(|p| format!("Visual C++ in {p}"))
        }
        "windows" => ran("gcc", &["--version"]).map(|v| v.lines().next().unwrap_or("gcc").to_string()),
        _ => ran("cc", &["--version"]).map(|v| v.lines().next().unwrap_or("cc").trim().to_string()),
    }
}

/// `ax setup`: check what making apps needs and fetch what aio can (the
/// shared runtime and wasm-bindgen, prebuilt and checked); `check`: only look. Every
/// missing piece is said with the command that gets it.
pub fn run(check: bool) -> Result<String, String> {
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    let vv = ran("rustc", &["-vV"]);
    let host = vv.as_deref().and_then(host_of).unwrap_or("").to_string();
    let mut needs = vec![rust(os, vv.as_deref()), c_tools(os, &host, c_found(os, &host))];
    // Fetched with no window (a terminal shows the progress); one named by
    // AIORT_RUNTIME is only looked at.
    let rt = match check || std::env::var_os("AIORT_RUNTIME").is_some() {
        true => Some(aio_shell::runtime::dir())
            .filter(|d| match std::env::var_os("AIORT_RUNTIME") {
                Some(_) => aio_shell::runtime::exe(d).is_file(),
                None => aio_shell::runtime::usable(std::env::consts::OS, d),
            })
            .ok_or_else(|| "not here; ax setup fetches it".to_string()),
        false => aio_shell::runtime::prefetch(
            aio_shell::runtime::VERSION,
            aio_shell::runtime::pinned_sha(aio_shell::runtime::VERSION).as_deref(),
        )
        .map_err(|e| e.to_string()),
    };
    let runtime = format!("Electron {} runtime", aio_shell::runtime::VERSION);
    needs.push(match rt {
        Ok(dir) => {
            let exe = aio_shell::runtime::exe(&dir);
            // Started as an app starts it: with the sandbox flag this machine needs.
            let out = std::process::Command::new(&exe)
                .args(aio_shell::runtime::sandbox_args(&dir))
                .arg("--version")
                .env_remove("ELECTRON_RUN_AS_NODE")
                .output();
            let ran = match out {
                Ok(o) if o.status.success() => {
                    Ok(format!("{} at {}", String::from_utf8_lossy(&o.stdout).trim(), dir.display()))
                }
                Ok(o) => Err(String::from_utf8_lossy(&o.stderr).into_owned()),
                Err(e) => Err(format!("{}: {e}", exe.display())),
            };
            runtime_runs(os, ran)
        }
        Err(e) => need(runtime, "draws every window and UI test", true, Err(e)),
    });
    let tc = toolchain();
    let installed = std::process::Command::new("rustup")
        .args(["target", "list", "--installed", "--toolchain", tc])
        .env("RUSTUP_AUTO_INSTALL", "0")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
    needs.push(targets(tc, installed.as_deref()));
    let bindgen = match check {
        true => wasm_bindgen_at(WASM_BINDGEN).map_err(|f| format!("not here ({f}); ax setup fetches it")),
        false => wasm_bindgen(WASM_BINDGEN),
    };
    needs.push(need(
        format!("wasm-bindgen {WASM_BINDGEN}"),
        "ax dev --web, ax build --web",
        false,
        bindgen.map(|p| format!("at {}", p.display())),
    ));
    needs.extend(optionals(os, crate::display::desktop(), &|p, a| ran(p, &[a])));
    report(os, arch, &needs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_missing_need_says_the_command_that_gets_it_per_os() {
        let vv = "rustc 1.98.0 (abc 2026-01-01)\nbinary: rustc\nhost: x86_64-pc-windows-msvc\nrelease: 1.98.0\n";
        assert_eq!(rust("windows", Some(vv)).found, Ok("rustc 1.98.0, host x86_64-pc-windows-msvc".into()));
        assert_eq!(host_of(vv), Some("x86_64-pc-windows-msvc"));
        assert!(rust("windows", None).found.unwrap_err().contains("winget install Rustlang.Rustup"));
        assert!(rust("linux", None).found.unwrap_err().contains("sh.rustup.rs"));
        let c = |os, host| c_tools(os, host, None).found.unwrap_err();
        assert!(c("linux", "x86_64-unknown-linux-gnu").contains("build-essential"));
        assert!(c("macos", "aarch64-apple-darwin").contains("xcode-select --install"));
        assert!(c("windows", "x86_64-pc-windows-msvc").contains("Microsoft.VisualStudio.Workload.VCTools"));
        assert!(c("windows", "x86_64-pc-windows-gnu").contains("gcc"));
        assert!(c("windows", "").contains("VCTools"), "no Rust yet: rustup's default, msvc");
        assert!(c_tools("linux", "", Some("cc: gcc 14".into())).found.is_ok());
    }

    #[test]
    fn a_runtime_that_cant_load_a_library_names_it_and_its_packages() {
        let said = "electron: error while loading shared libraries: libnss3.so: cannot open shared object file";
        assert_eq!(missing_lib(said), Some("libnss3.so"));
        assert_eq!(missing_lib("Segmentation fault"), None);
        let e = runtime_runs("linux", Err(said.into())).found.unwrap_err();
        assert!(
            e.starts_with("it can't load libnss3.so") && e.contains("sudo apt install") && e.contains("libnss3"),
            "{e}"
        );
        assert!(runtime_runs("windows", Err("boom\n".into())).found.unwrap_err().ends_with("does not start: boom"));
        assert!(runtime_runs("linux", Ok("v44".into())).found.is_ok());
    }

    #[test]
    fn the_report_is_ready_only_with_every_needed_piece_and_says_each_missing_one() {
        let ok = |r| need("a", "x", r, Ok("here".into()));
        let gone = |r| need("b", "for y", r, Err("install: z".into()));
        let r = report("linux", "x86_64", &[ok(true), gone(false)]).unwrap();
        assert!(r.contains("  ✓ a: here\n  – b (optional), for y: install: z\nready: ax create"), "{r}");
        let e = report("linux", "x86_64", &[ok(true), gone(true)]).unwrap_err();
        assert!(e.contains("  ✗ b, for y: install: z") && e.ends_with("then run ax setup again"), "{e}");
        // Optional tools: found or each with its own fix; Linux adds a nested display.
        let none = |_: &str, _: &str| None;
        let mac = optionals("macos", true, &none);
        assert_eq!(mac.iter().map(|n| n.what.as_str()).collect::<Vec<_>>(), ["git", "ffmpeg"]);
        assert!(mac.iter().all(|n| !n.required && n.found.as_ref().unwrap_err().contains("brew install")));
        let linux = optionals("linux", false, &|p, _| (p == "Xvfb").then(|| "Xvfb".into()));
        assert_eq!(linux[2].found, Ok("Xvfb (headless, seen by no one)".into()), "Xvfb is enough");
        let xephyr = |desktop| optionals("linux", desktop, &|p, _| (p == "Xephyr").then(|| "Xephyr".into()));
        assert!(xephyr(true)[2].found.as_ref().unwrap().contains("with no DISPLAY nothing nests"), "which one is used");
        assert!(
            xephyr(false)[2].found.as_ref().unwrap_err().contains("apt install xvfb"),
            "no desktop: Xephyr holds nothing"
        );
        assert!(optionals("windows", false, &none)[0].found.as_ref().unwrap_err().contains("winget install Git.Git"));
    }

    #[test]
    fn the_web_builds_wasm_bindgen_is_the_one_aio_pins_and_each_host_has_its_release() {
        let lock = crate::web::locked(include_str!("../../../Cargo.lock"), "wasm-bindgen");
        assert_eq!(lock.as_deref(), Some(WASM_BINDGEN), "aio's lock");
        let pin = format!("wasm-bindgen = \"={WASM_BINDGEN}\"");
        assert!(include_str!("../../aio/Cargo.toml").contains(&pin), "aio-rs pins it, so every app's lock has it");
        for (os, arch) in [("linux", "x86_64"), ("linux", "aarch64"), ("macos", "aarch64"), ("windows", "x86_64")] {
            let (url, sha) = wasm_bindgen_release(os, arch).unwrap_or_else(|| panic!("{os} {arch}"));
            assert!(url.contains(&format!("/download/{WASM_BINDGEN}/wasm-bindgen-{WASM_BINDGEN}-")), "{url}");
            assert!(url.ends_with(".tar.gz") && sha.len() == 64 && sha.chars().all(|c| c.is_ascii_hexdigit()));
        }
        assert_eq!(wasm_bindgen_release("freebsd", "x86_64"), None);
        assert!(wasm_bindgen_home(Path::new("/h"), "0.2.1").starts_with("/h/tools/wasm-bindgen-0.2.1"));
        assert_eq!(bindgen_version("wasm-bindgen 0.2.129\n"), "0.2.129");
        assert_eq!(bindgen_version("wasm-bindgen 0.2.117 (abc 2026-01-01)"), "0.2.117");
    }

    #[test]
    fn a_tar_gives_the_named_file_and_nothing_else() {
        let entry = |path: &str, kind: u8, body: &[u8]| {
            let mut h = [0u8; 512];
            h[..path.len()].copy_from_slice(path.as_bytes());
            h[124..135].copy_from_slice(format!("{:011o}", body.len()).as_bytes());
            h[156] = kind;
            let mut v = h.to_vec();
            v.extend(body);
            v.resize(v.len().div_ceil(512) * 512, 0);
            v
        };
        let tar = [
            entry("wb-x/", b'5', b""),
            entry("wb-x/wasm-bindgen-test-runner", b'0', b"runner"),
            entry("wb-x/wasm-bindgen", b'0', b"the cli"),
            vec![0; 1024],
        ]
        .concat();
        assert_eq!(tar_entry(&tar, "wasm-bindgen"), Some(&b"the cli"[..]));
        assert_eq!(tar_entry(&tar, "wasm-bindgen-test-runner"), Some(&b"runner"[..]));
        assert_eq!(tar_entry(&tar, "wasm2es6js"), None);
        assert_eq!(tar_entry(&tar[..600], "wasm-bindgen"), None, "a cut archive");
    }

    #[test]
    fn the_platform_targets_are_named_for_aios_toolchain() {
        assert_eq!(toolchain(), include_str!("../../../rust-toolchain.toml").split('"').nth(1).unwrap());
        let e = targets("1.98.0", Some("x86_64-unknown-linux-gnu\naarch64-linux-android\n")).found.unwrap_err();
        assert_eq!(
            e,
            "rustup target add --toolchain 1.98.0 armv7-linux-androideabi x86_64-linux-android wasm32-unknown-unknown"
        );
        assert!(targets("1.98.0", Some(&PLATFORM_TARGETS.join("\n"))).found.is_ok());
        let none = targets("1.98.0", None);
        assert!(!none.required && none.found.unwrap_err().contains("rustup toolchain install 1.98.0"));
    }

    #[test]
    fn a_tool_that_fails_its_checksum_is_never_unpacked() {
        let d = tempfile::tempdir().unwrap();
        let body = b"#!/bin/sh\n".to_vec();
        fn copy(from: &Path, to: &Path) -> Result<(), String> {
            write_exe(&std::fs::read(from).map_err(|e| e.to_string())?, to)
        }
        let fetch_tool = |url: &str, sha: &str, to: &Path| fetch_tool(url, sha, to, copy);
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/tool", l.local_addr().unwrap());
        let served = body.clone();
        std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut s, _) = l.accept().unwrap();
                _ = s.read(&mut [0; 4096]);
                let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", served.len());
                _ = std::io::Write::write_all(&mut s, head.as_bytes());
                _ = std::io::Write::write_all(&mut s, &served);
            }
        });
        let to = d.path().join("tools/tool");
        let e = fetch_tool(&url, &"0".repeat(64), &to).unwrap_err();
        assert!(e.contains("expected 000") && !to.exists(), "{e}");
        assert_eq!(std::fs::read_dir(d.path().join("tools")).unwrap().count(), 0, "nothing left behind");
        let sha = aio::sha256::hex(&aio::sha256::Sha256::digest(&body));
        fetch_tool(&url, &sha, &to).unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b"#!/bin/sh\n");
        fetch_tool("http://127.0.0.1:1/never", &sha, &to).expect("there already: nothing fetched");
    }
}

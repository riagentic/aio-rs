//! `ax dev <app> [args…]`: run an app while you edit it. With `dx` (the
//! Dioxus CLI, the same version as aio's Dioxus) on the PATH, edits are
//! hot-patched into the running app (a small crate's in under a second), state kept: Rust in
//! every workspace crate (views and cell methods alike), `rsx!` markup at
//! once. Without it: build, run, and on every saved change build again; a
//! good build restarts the app (its state comes back from the log), a failed
//! one leaves the running app alone.
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::mpsc;
use std::time::Duration;

/// The Dioxus version aio is built on; `dx` must be the same to patch.
pub const DIOXUS: &str = "0.7.10";

/// The profile of every debug build aio runs (`ax dev`, `ax test`,
/// `ax lint`) and dx's for a desktop app: one copy of the app's libraries
/// in `<target>/<host>/desktop-dev`, plain cargo and dx alike.
pub const PROFILE: &str = "desktop-dev";

/// This machine's target (`rustc -vV`'s host), as dx names it.
pub fn host_triple() -> &'static str {
    static HOST: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HOST.get_or_init(|| {
        let out = Command::new("rustc").arg("-vV").output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
        let host = out.ok().and_then(|o| o.lines().find_map(|l| l.strip_prefix("host: ").map(String::from)));
        // No rustc: cargo can't build either, and says so.
        host.or_else(|| crate::platform::host().ok().map(|p| p.triple.into())).unwrap_or_default()
    })
}

/// cargo's arguments for that layout, as dx passes them: the profile
/// (defined here too, for an app whose Cargo.toml doesn't), unstripped,
/// for this machine, so the artifacts are dx's.
pub fn layout() -> Vec<String> {
    let strip = format!("profile.{PROFILE}.strip=false");
    let inherits = format!("profile.{PROFILE}.inherits=\"dev\"");
    ["--config", &strip, "--config", &inherits, "--profile", PROFILE, "--target", host_triple()]
        .map(String::from)
        .into()
}

/// The build folder every cargo and dx ax runs shares (`CARGO_TARGET_DIR`,
/// set by `ax` for its children): the person's own, else one per machine
/// in aio's home, so an app's libraries are built once for all apps and
/// nothing of this machine is written into a repo.
pub fn shared_target() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .filter(|v| !v.is_empty())
        .map_or_else(|| aio::paths::tool_home().join("target"), PathBuf::from)
}

/// Binary `name` built into `target` is the app in `root`'s: in a shared
/// build folder two apps with one binary name would overwrite each other,
/// so the first to claim it keeps it while its workspace exists. Git
/// worktrees of one app are that app: the last built takes the name.
pub fn claim(target: &Path, name: &str, root: &Path) -> Result<(), String> {
    let file = target.join("aio").join("owners").join(name);
    let root = std::path::absolute(root).map_err(|e| e.to_string())?;
    if let Ok(owner) = std::fs::read_to_string(&file).map(PathBuf::from)
        && owner != root
        && owner.join("Cargo.toml").is_file()
        && checkout(&owner).is_none_or(|o| checkout(&root) != Some(o))
    {
        return Err(format!(
            "binary `{name}` is already {}'s in the shared build folder {}: name this app apart (its binary is its id)",
            owner.display(),
            target.display()
        ));
    }
    std::fs::create_dir_all(file.parent().unwrap_or(target)).map_err(|e| format!("{}: {e}", file.display()))?;
    std::fs::write(&file, root.to_string_lossy().as_bytes()).map_err(|e| format!("{}: {e}", file.display()))
}

/// Which app of which repository `dir` is, whichever worktree of it: the
/// repository's shared git folder and `dir`'s place in its checkout (None:
/// in no git checkout).
fn checkout(dir: &Path) -> Option<(PathBuf, PathBuf)> {
    let top = dir.ancestors().find(|d| d.join(".git").exists())?;
    let git = top.join(".git");
    let common = if git.is_dir() {
        git
    } else {
        // A worktree's `.git` file names its own git folder, whose `commondir` is the repository's.
        let own = std::fs::read_to_string(&git).ok()?;
        let own = top.join(own.strip_prefix("gitdir:")?.trim());
        match std::fs::read_to_string(own.join("commondir")) {
            Ok(c) => own.join(c.trim()),
            Err(_) => own,
        }
    };
    Some((std::fs::canonicalize(common).ok()?, dir.strip_prefix(top).ok()?.to_path_buf()))
}

/// [`claim`] for binary `name` of the workspace `meta` describes.
pub fn claim_in(meta: &serde_json::Value, name: &str) -> Result<(), String> {
    let at = |k: &str| meta[k].as_str().map(PathBuf::from).ok_or(format!("cargo metadata: no {k}"));
    claim(&at("target_directory")?, name, &at("workspace_root")?)
}

/// What `cargo metadata` says about the app.
#[derive(Clone, Debug)]
pub struct Pkg {
    /// The package with the binary (an app's launcher).
    pub name: String,
    /// Its dir and the dirs of the workspace packages it uses (the app itself).
    pub dirs: Vec<PathBuf>,
    /// Where its debug build puts the binary ([`build`] says where it did).
    pub bin: PathBuf,
    pub version: String,
}

/// `cargo metadata` for the workspace here.
pub fn metadata() -> Result<serde_json::Value, String> {
    let out = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .map_err(|e| format!("cargo: {e} (Rust is not on PATH: ax setup says how to get it)"))?;
    if !out.status.success() {
        let said = String::from_utf8_lossy(&out.stderr);
        return Err(match said.contains("could not find `Cargo.toml`") {
            true => format!("{}: no app's workspace here or above: cd into the app's folder", said.trim_end()),
            false => said.into_owned(),
        });
    }
    serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())
}

/// The package with binary `name`, and what to watch for it.
/// The package of binary `name`, or of the binary a workspace app's
/// `[build]` target labelled `name` builds (aio's `am start <label>`).
pub fn locate(meta: &serde_json::Value, name: &str) -> Result<Pkg, String> {
    bin_pkg(meta, name).or_else(|e| labelled(meta, name).ok_or(e).and_then(|bin| bin_pkg(meta, &bin)))
}

/// The binary built by the `[build]` target labelled `label` of one of the
/// workspace's apps, when it names one.
fn labelled(meta: &serde_json::Value, label: &str) -> Option<String> {
    let bins = meta["packages"].as_array()?.iter().flat_map(|p| p["targets"].as_array().into_iter().flatten());
    bins.filter(|t| t["kind"].as_array().is_some_and(|k| k.iter().any(|k| k == "bin")))
        .filter_map(|t| crate::build::app_of(&bin_pkg(meta, t["name"].as_str()?).ok()?).ok())
        .find_map(|(_, cfg)| cfg.build.targets.into_iter().find(|t| t.name == label).and_then(|t| t.bin))
}

fn bin_pkg(meta: &serde_json::Value, name: &str) -> Result<Pkg, String> {
    pkg_with(meta, name, "bin").ok_or_else(|| format!("no binary `{name}` in this workspace"))
}

/// The package of the web build `name`: its binary's (an app served by
/// its own server), else the package of that name with a `cdylib` (a web
/// app alone, its cells in the page).
pub fn web_pkg(meta: &serde_json::Value, name: &str) -> Result<Pkg, String> {
    locate(meta, name).or_else(|e| pkg_with(meta, &name.replace('-', "_"), "cdylib").ok_or(e))
}

/// The package with a target `name` of `kind`.
fn pkg_with(meta: &serde_json::Value, name: &str, kind: &str) -> Option<Pkg> {
    let target = meta["target_directory"].as_str()?;
    let pkgs = meta["packages"].as_array()?;
    let is_it =
        |t: &serde_json::Value| t["name"] == name && t["kind"].as_array().is_some_and(|k| k.iter().any(|k| k == kind));
    let pkg = pkgs.iter().find(|p| p["targets"].as_array().is_some_and(|ts| ts.iter().any(is_it)))?;
    let dir_of = |p: &serde_json::Value| -> Option<PathBuf> {
        Some(Path::new(p["manifest_path"].as_str()?).parent()?.to_path_buf())
    };
    let mut dirs = vec![dir_of(pkg)?];
    // Path dependencies that are workspace members, theirs too: the app's
    // own code, however many crates it is split into.
    let mut i = 0;
    while let Some(dir) = dirs.get(i).cloned() {
        i += 1;
        let Some(p) = pkgs.iter().find(|p| dir_of(p).as_ref() == Some(&dir)) else { continue };
        for d in p["dependencies"].as_array().into_iter().flatten() {
            if let Some(dep) = d["path"].as_str().map(PathBuf::from)
                && !dirs.contains(&dep)
                && pkgs.iter().filter_map(dir_of).any(|m| m == dep)
            {
                dirs.push(dep);
            }
        }
    }
    let pkg_name = pkg["name"].as_str().unwrap_or(name).to_string();
    let version = pkg["version"].as_str().unwrap_or("0.0.0").to_string();
    Some(Pkg { name: pkg_name, dirs, bin: dev_bin(Path::new(target), name), version })
}

/// Binary `name` of a debug build into `target` ([`layout`]).
pub fn dev_bin(target: &Path, name: &str) -> PathBuf {
    target.join(host_triple()).join(PROFILE).join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

/// The app `cwd` is in: the binary whose app dirs (one with `aio.toml`)
/// hold `cwd` most closely. Two at the same depth: name one.
pub fn here(meta: &serde_json::Value, cwd: &Path) -> Result<String, String> {
    let bins =
        meta["packages"].as_array().into_iter().flatten().flat_map(|p| p["targets"].as_array().into_iter().flatten());
    let mut best: Vec<(usize, String)> = Vec::new();
    for t in bins.filter(|t| t["kind"].as_array().is_some_and(|k| k.iter().any(|k| k == "bin"))) {
        let Some(name) = t["name"].as_str() else { continue };
        let Ok(pkg) = locate(meta, name) else { continue };
        let Some(depth) = pkg.dirs.iter().filter(|d| cwd.starts_with(d)).map(|d| d.components().count()).max() else {
            continue;
        };
        if !pkg.dirs.iter().any(|d| d.join("aio.toml").is_file()) {
            continue;
        }
        best.push((depth, name.into()));
    }
    let top = best.iter().map(|(d, _)| *d).max();
    let mut names: Vec<String> = best.into_iter().filter(|(d, _)| Some(*d) == top).map(|(_, n)| n).collect();
    names.dedup();
    match names.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err("no app here: give one, or --app <name>".into()),
        many => Err(format!("{} apps here ({}): give one, or --app <name>", many.len(), many.join(", "))),
    }
}

/// Is this `dx --version` output the Dioxus aio is built on?
pub fn dx_matches(version: &str) -> bool {
    version.split_whitespace().any(|w| w == DIOXUS)
}

/// `dx serve` for package `pkg`: hot-patching, aio's renderer, its own
/// port, and `aio-shell/dev` when the package depends on the shell; no TUI
/// unless `interactive` (`--tui`): it would hide the app's own lines
/// (`[aio] hot-patch 1 applied`) and shows a failed patch as raw JSON.
pub fn dx_args(pkg: &str, port: u16, dev: bool, interactive: bool, args: &[String]) -> Vec<String> {
    let mut v: Vec<String> = ["serve", "--hot-patch", "-p", pkg, "--renderer", "native", "--open", "false", "--port"]
        .map(String::from)
        .into();
    v.push(port.to_string());
    if dev {
        v.extend(["--features".into(), "aio-shell/dev".into()]);
    }
    if !interactive {
        v.extend(["--interactive".into(), "false".into()]);
    }
    // One `--args=` word: a separate value starting with `--` (`--home <dir>`)
    // is taken by dx's parser for a flag of its own. dx splits it on spaces.
    if !args.is_empty() {
        v.push(format!("--args={}", args.join(" ")));
    }
    v
}

fn dx_version() -> Option<String> {
    let out = Command::new(crate::setup::dx()).arg("--version").output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A port nothing listens on now (dx's default, 8080, is often taken).
pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()).map_or(18731, |a| a.port())
}

/// Changes that matter: sources and assets, not build output or editor files.
pub fn relevant(p: &Path) -> bool {
    let s = p.to_string_lossy();
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    !s.contains("/target/") && !name.starts_with('.') && !name.ends_with('~') && !name.ends_with(".swp")
}

/// A real change. Not a read: a build reads every source, and reads (access
/// events) would restart the app forever. Not a `touch` either.
pub fn changes(kind: &notify::EventKind) -> bool {
    use notify::EventKind::{Create, Modify, Remove};
    use notify::event::ModifyKind;
    matches!(kind, Create(_) | Modify(_) | Remove(_)) && !matches!(kind, Modify(ModifyKind::Metadata(_)))
}

/// Build package `name` ([`layout`]): its binary, as cargo says it wrote
/// it; a failed build's errors are also kept in `logs/build.log` for `aio
/// errors` (a good build removes it).
pub fn build(name: &str, logs: Option<&Path>) -> Option<PathBuf> {
    let out = Command::new("cargo")
        .args(["build", "-q", "-p", name, "--message-format=json-render-diagnostics"])
        .args(layout())
        .stderr(std::process::Stdio::piped())
        .output();
    let (bin, text) = match out {
        Ok(o) => (
            o.status.success().then(|| executable(&String::from_utf8_lossy(&o.stdout))).flatten(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        ),
        Err(e) => (None, format!("can't run cargo: {e}")),
    };
    let text = with_hints(&text);
    eprint!("{text}");
    if let Some(dir) = logs {
        let file = dir.join(BUILD_LOG);
        let kept = match bin {
            Some(_) => std::fs::remove_file(&file)
                .or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) }),
            None => aio::paths::private_dir(dir)
                .and_then(|()| aio::paths::write_atomic(&file, build_log(now_ms(), &text).as_bytes(), false)),
        };
        if let Err(e) = kept {
            eprintln!("ax dev: {}: {e}", file.display());
        }
    }
    bin
}

/// Compiler errors whose fix is aio's to say, by what rustc says, and the fix.
const HINTS: &[(&[&str], &str)] = &[(
    &["aio::serde::Serialize", "aio::serde::Deserialize"],
    "ax: a type a cell keeps (a field's, a method argument's) is saved and sent: put #[aio::data] on it \
     (it derives serde through aio: no serde dependency to add)",
)];

/// Compiler output `text` with aio's hint after it for each error it knows (pure).
pub fn with_hints(text: &str) -> String {
    let hints: Vec<&str> =
        HINTS.iter().filter(|(said, _)| said.iter().any(|s| text.contains(s))).map(|(_, h)| *h).collect();
    match hints.is_empty() {
        true => text.to_string(),
        false => format!("{}\n  = {}\n", text.trim_end(), hints.join("\n  = ")),
    }
}

/// The binary a build wrote, from cargo's JSON messages (the last one: the
/// package's own). Pure.
pub fn executable(messages: &str) -> Option<PathBuf> {
    messages
        .lines()
        .rev()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|m| m["reason"] == "compiler-artifact")
        .find_map(|m| m["executable"].as_str().map(PathBuf::from))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// Where `ax dev` keeps the last failed build, in the app's logs dir.
pub const BUILD_LOG: &str = "build.log";
/// dx's whole output during `ax dev`, as it said it ([`relay`]).
const DEV_LOG: &str = "dev.log";

/// `build.log`: when, then cargo's errors.
pub fn build_log(at: i64, errors: &str) -> String {
    format!("build failed at {}\n{errors}", aio::log::stamp(at))
}

/// What shows a failed build's `errors` on a dev page, open (`aio::page::OVERLAY`;
/// a release page has none, so nothing) (pure).
pub fn show_failed(errors: &str) -> String {
    let e = serde_json::Value::from(errors);
    format!("window.__aioDiag && window.__aioDiag('build failed', {e}, true)")
}

/// What takes a shown build failure off a dev page: a build or patch landed.
pub const SHOWN_FIXED: &str = "window.__aioDiagDrop && window.__aioDiagDrop('build failed')";

/// Run `js` on the running app's page: `window` is its id and plan.
fn on_page(window: Option<&(String, aio::paths::Plan)>, js: &str) -> Result<String, String> {
    let (id, p) = window.ok_or("no app to show it on")?;
    let door = crate::cdp::door(id, p).ok_or("it isn't running")?;
    crate::eval::run(&crate::cdp::Reach::Door(door), 0, js)
}

/// What a line of dx's output means to `ax dev`.
#[derive(Debug, PartialEq)]
pub enum DxEvent {
    /// Nothing more than its words.
    Said,
    /// A compiler error, as rustc renders it.
    Error(String),
    /// A build or hot-patch failed.
    Failed,
    /// A build or hot-patch landed.
    Landed,
}

/// One line of dx's output (pure): what to say, if anything, and what it
/// means. A compiler message dx passes on raw (cargo's JSON: a failed
/// hot-patch of a workspace library, as the app's) is said as rustc renders
/// it, its other JSON lines not at all; any other line without dx's
/// `  23.03s ERROR ` (dx labels all the app's stderr `ERROR`), dx's banner
/// and Chromium's own log lines left out (they stay in `logs/dev.log`).
pub fn dx_line(line: &str) -> (Option<String>, DxEvent) {
    let json = line.trim_start();
    if json.starts_with("{\"$message_type\"") {
        let Ok(m) = serde_json::from_str::<serde_json::Value>(json) else { return (Some(line.into()), DxEvent::Said) };
        let Some(text) = m["rendered"].as_str().filter(|_| m["$message_type"] == "diagnostic") else {
            return (None, DxEvent::Said);
        };
        let text = with_hints(plain(text).trim_end());
        let error = m["level"] == "error" && !m["spans"].as_array().is_none_or(Vec::is_empty);
        return (Some(text.clone()), if error { DxEvent::Error(text) } else { DxEvent::Said });
    }
    let event = match line {
        l if l.contains("Build failed") => DxEvent::Failed,
        l if ["Hot-patching: ", "Hotreloading: ", "Build completed successfully"].iter().any(|w| l.contains(w)) => {
            DxEvent::Landed
        }
        _ => DxEvent::Said,
    };
    let body = unprefixed(line);
    let words = plain(body);
    let w = words.trim();
    let noise = w.starts_with("• Press")
        || w.starts_with("Serving your app")
        || (!w.is_empty() && w.chars().all(|c| c == '-'))
        || chromium_log(w);
    (if noise { None } else { Some(body.into()) }, event)
}

/// `line` without dx's `  23.03s  INFO ` before it, or the 15 spaces of its
/// next lines (pure).
fn unprefixed(line: &str) -> &str {
    let t = line.trim_start();
    let level = t
        .split_once(' ')
        .filter(|(time, _)| time.strip_suffix('s').is_some_and(|n| n.parse::<f64>().is_ok()))
        .and_then(|(_, rest)| {
            let rest = rest.trim_start();
            ["INFO ", "WARN ", "ERROR ", "DEBUG ", "TRACE "].iter().find_map(|l| rest.strip_prefix(l))
        });
    level.unwrap_or_else(|| line.strip_prefix("               ").unwrap_or(line))
}

/// Chromium's own log line, `[pid:MMDD/HHMMSS.us:ERROR:file.cc:1] …` or
/// ANGLE's `ERR: …`, short of FATAL (pure).
fn chromium_log(w: &str) -> bool {
    let tagged = w.strip_prefix('[').and_then(|r| r.split_once(']')).is_some_and(|(tag, _)| {
        let f: Vec<&str> = tag.splitn(4, ':').collect();
        f.len() == 4
            && f[0].bytes().all(|b| b.is_ascii_digit())
            && f[1].contains('/')
            && ["INFO", "WARNING", "ERROR", "VERBOSE1"].contains(&f[2])
    });
    tagged || w.starts_with("ERR: ")
}

/// `text` without terminal colour codes (pure).
fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            chars.by_ref().find(|c| c.is_ascii_alphabetic());
        } else {
            out.push(c);
        }
    }
    out
}

/// When the last saved change came (ms; 0: none waits): what a landed patch
/// or build is timed from. dx's own "took" leaves its compile out.
static SAVED: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// dx's output passes through `ax dev` ([`dx_line`]), compact and readable:
/// a failed build's or patch's errors also go on the running app's page
/// and into `logs/build.log` (`ax errors`), until one lands. dx says the
/// errors after "Build failed", so they are shown once its output pauses.
fn relay(rx: mpsc::Receiver<(bool, String)>, window: Option<(String, aio::paths::Plan)>, logs: Option<PathBuf>) {
    let (mut errors, mut failed, mut shown) = (Vec::new(), None, false);
    // All of dx's lines as they came, the ones left off the terminal too.
    let mut raw = logs.as_ref().and_then(|l| {
        aio::paths::private_dir(l).ok()?;
        std::fs::File::create(l.join(DEV_LOG)).ok()
    });
    loop {
        let got = rx.recv_timeout(Duration::from_millis(300));
        if let Ok((err, line)) = &got {
            if let Some(f) = &mut raw {
                use std::io::Write;
                _ = writeln!(f, "{line}");
            }
            let (say, event) = dx_line(line);
            match say {
                Some(s) if *err => eprintln!("{s}"),
                Some(s) => println!("{s}"),
                None => {}
            }
            match event {
                DxEvent::Said => {}
                DxEvent::Error(e) => errors.push(e),
                DxEvent::Failed => {
                    SAVED.store(0, std::sync::atomic::Ordering::Relaxed);
                    failed = Some(plain(unprefixed(line)).trim().to_string());
                }
                DxEvent::Landed => {
                    let saved = SAVED.swap(0, std::sync::atomic::Ordering::Relaxed);
                    if saved > 0 {
                        eprintln!("ax dev: in {:.1} s from the save", (now_ms() - saved) as f64 / 1000.0);
                    }
                    errors.clear();
                    failed = None;
                    if let Some(l) = &logs {
                        _ = std::fs::remove_file(l.join(BUILD_LOG));
                        // `ax doctor`: the running app has the edits up to now.
                        _ = std::fs::write(l.join(PATCHED), now_ms().to_string());
                    }
                    if std::mem::take(&mut shown) {
                        _ = on_page(window.as_ref(), SHOWN_FIXED);
                    }
                }
            }
            continue;
        }
        if let Some(line) = failed.take() {
            let text = if errors.is_empty() { line } else { std::mem::take(&mut errors).join("\n\n") };
            if let Some(l) = &logs {
                _ = aio::paths::private_dir(l).and_then(|()| {
                    aio::paths::write_atomic(&l.join(BUILD_LOG), build_log(now_ms(), &text).as_bytes(), false)
                });
            }
            match on_page(window.as_ref(), &show_failed(&text)) {
                Ok(_) => shown = true,
                Err(e) => eprintln!("ax dev: the errors aren't on its page: {e}"),
            }
        }
        if got == Err(mpsc::RecvTimeoutError::Disconnected) {
            return;
        }
    }
}

/// A signal on every saved change to the app's sources and assets.
/// What `ax dev` watches for changes (aio's `--watch=false`, `watch: [..]`).
#[derive(Debug, PartialEq)]
pub enum Watching {
    /// The app's sources: `src`, `assets` and `Cargo.toml` of each of its dirs.
    All,
    /// Nothing: build and run it once.
    Off,
    /// Only these, relative to the app's dir (its `aio.toml`).
    Only(Vec<String>),
}

/// `--watch=false` or `--watch=<dir,dir>` taken out of a launch's args,
/// before a bare `--` (pure).
pub fn take_watch(args: &[String]) -> Result<(Vec<String>, Watching), String> {
    let end = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let mut w = Watching::All;
    let mut rest = vec![];
    for (i, a) in args.iter().enumerate() {
        match a.strip_prefix("--watch=") {
            Some("false") if i < end => w = Watching::Off,
            Some(dirs) if i < end => {
                let dirs: Vec<String> = dirs.split(',').filter(|d| !d.is_empty()).map(String::from).collect();
                if dirs.is_empty()
                    || dirs.iter().any(|d| Path::new(d).is_absolute() || Path::new(d).has_root() || d.contains(".."))
                {
                    return Err(format!("{a}: false, or dirs inside the app (--watch=src/ui,assets)"));
                }
                w = Watching::Only(dirs);
            }
            _ if i < end && a == "--watch" => return Err("--watch=false, or --watch=<dir,dir> inside the app".into()),
            _ => rest.push(a.clone()),
        }
    }
    Ok((rest, w))
}

/// The paths `w` watches for `pkg` (pure; `Only`: under its app dir `app`).
pub fn watched(pkg: &Pkg, app: &Path, w: &Watching) -> Vec<PathBuf> {
    match w {
        Watching::All => pkg.dirs.iter().flat_map(|d| ["src", "assets", "Cargo.toml"].map(|sub| d.join(sub))).collect(),
        Watching::Off => vec![],
        Watching::Only(dirs) => dirs.iter().map(|d| app.join(d)).collect(),
    }
}

/// A watcher on `paths` (those there): a message on each real change,
/// true when a `Cargo.toml` changed (a dependency dx can't hot-patch in).
fn watch_these(paths: &[PathBuf]) -> Result<(notify::RecommendedWatcher, mpsc::Receiver<bool>), String> {
    let (tx, rx) = mpsc::channel();
    let mut w = notify::recommended_watcher(move |e: notify::Result<notify::Event>| {
        if let Ok(e) = e
            && changes(&e.kind)
            && e.paths.iter().any(|p| relevant(p))
        {
            _ = tx.send(e.paths.iter().any(|p| p.file_name().is_some_and(|n| n == "Cargo.toml")));
        }
    })
    .map_err(|e| e.to_string())?;
    for p in paths.iter().filter(|p| p.exists()) {
        notify::Watcher::watch(&mut w, p, notify::RecursiveMode::Recursive).map_err(|e| e.to_string())?;
    }
    Ok((w, rx))
}

/// Wait out a burst of saves: true when one changed a `Cargo.toml`.
fn settle(rx: &mpsc::Receiver<bool>) -> bool {
    let mut manifest = false;
    while let Ok(m) = rx.recv_timeout(Duration::from_millis(300)) {
        manifest |= m;
    }
    manifest
}

/// The app's `[build] css` step, if any: a failure is said, the app kept.
/// What the step wrote is not a change of its own.
fn css(pkg: &Pkg, rx: &mpsc::Receiver<bool>) {
    let Ok((_, cfg)) = crate::build::app_of(pkg) else { return };
    match crate::build::app_dir(pkg).and_then(|dir| crate::css::step(&cfg, &dir)) {
        Ok(Some(did)) => eprintln!("ax dev: {did}"),
        Ok(None) => return,
        Err(e) => eprintln!("ax dev: {e}"),
    }
    settle(rx);
}

/// The app, started so it stops when `ax dev` does, placed by `put`
/// (its display, contained or not).
fn launch(bin: &Path, args: &[String], put: &dyn Fn(&mut Command)) -> Option<(Child, aio::lifecycle::Watch)> {
    let mut cmd = Command::new(bin);
    cmd.args(args);
    put(&mut cmd);
    aio::lifecycle::scrub(&mut cmd, true);
    let watch = aio::lifecycle::watched(&mut cmd).inspect_err(|e| eprintln!("ax dev: {e}")).ok()?;
    let child = cmd.spawn().inspect_err(|e| eprintln!("ax dev: can't start {}: {e}", bin.display())).ok()?;
    Some((child, watch))
}

/// Stop the app the graceful way: its effects finish and commit.
/// The files an `ax dev` session keeps in the app's logs dir: its dx's pid,
/// the mark the app's window leaves when the person closes it, and the mark
/// that asks the session for a full rebuild and restart (`ax restart`, a
/// changed `Cargo.toml`).
const DX_PID: &str = "dev-dx.pid";
const CLOSED: &str = "dev-closed";
const RESTART: &str = "dev-restart";
/// The session's own pid, while it runs.
const SESSION: &str = "dev.pid";
/// When the session's last build or hot-patch landed (ms).
pub const PATCHED: &str = "dev-patched";

/// Whether an `ax dev` session runs the app whose logs are `logs`.
pub fn session(logs: &Path) -> bool {
    let pid = std::fs::read_to_string(logs.join(SESSION)).ok().and_then(|s| s.trim().parse::<u32>().ok());
    // A session ended by a signal leaves its file: its pid must still run.
    #[cfg(unix)]
    // SAFETY: signal 0 only asks whether the pid is there.
    return pid.is_some_and(|p| unsafe { libc::kill(p as i32, 0) } == 0);
    #[cfg(not(unix))]
    pid.is_some()
}

/// The session's pid file, removed when dropped.
struct Session(PathBuf);

impl Session {
    fn open(logs: &Path) -> Option<Self> {
        _ = std::fs::remove_file(logs.join(RESTART));
        _ = std::fs::remove_file(logs.join(PATCHED));
        let file = logs.join(SESSION);
        aio::paths::private_dir(logs).and_then(|()| std::fs::write(&file, std::process::id().to_string())).ok()?;
        Some(Session(file))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        _ = std::fs::remove_file(&self.0);
    }
}

/// Ask the `ax dev` session of `logs` to rebuild the app whole and start it again.
pub fn ask_restart(logs: &Path) -> std::io::Result<()> {
    std::fs::write(logs.join(RESTART), b"")
}

/// Run `dx` for one `ax dev` session. It never outlives the session: it
/// ends with this process (Linux: even when killed), a dx an earlier session
/// left running (its terminal closed) is stopped first, so two never rebuild
/// the same app, and closing the app's window ends the session (aio's `am dev`).
/// Asked to restart ([`RESTART`]), it runs a new dx: a full build, the app
/// started again with the same flags on the same display.
/// `relayed` (no dx TUI, all but `--tui` in a terminal): its output goes
/// through [`relay`], shown on `window`. `make` makes the dx command.
fn serve(
    make: &dyn Fn() -> Command,
    logs: Option<&Path>,
    relayed: bool,
    window: Option<(String, aio::paths::Plan)>,
) -> std::io::Result<()> {
    let files = logs.map(|l| (l.join(DX_PID), l.join(CLOSED), l.join(RESTART)));
    if let Some((pid, closed, again)) = &files {
        _ = std::fs::remove_file(closed);
        _ = std::fs::remove_file(again);
        if let Some(p) = std::fs::read_to_string(pid).ok().and_then(|s| s.trim().parse::<u32>().ok())
            && stop_stray(p)
        {
            eprintln!("ax dev: stopped the dx an earlier `ax dev` of this app left running (pid {p})");
        }
    }
    let (tx, rx) = mpsc::channel();
    if relayed {
        let logs = logs.map(Path::to_path_buf);
        std::thread::spawn(move || relay(rx, window, logs));
    }
    let spawn = || -> std::io::Result<Child> {
        let mut dx = make();
        if let Some((_, closed, _)) = &files {
            dx.env("AIORT_DEV_CLOSED", closed);
        }
        #[cfg(target_os = "linux")]
        // SAFETY: only prctl between fork and exec (async-signal-safe).
        unsafe {
            use std::os::unix::process::CommandExt;
            dx.pre_exec(|| match libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) {
                0 => Ok(()),
                _ => Err(std::io::Error::last_os_error()),
            });
        }
        if relayed {
            dx.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
        }
        let mut child = dx.spawn()?;
        if relayed {
            let pipe = |out: Option<Box<dyn std::io::Read + Send>>, err: bool, tx: mpsc::Sender<(bool, String)>| {
                std::thread::spawn(move || {
                    use std::io::BufRead;
                    for line in std::io::BufReader::new(out?).lines().map_while(Result::ok) {
                        tx.send((err, line)).ok()?;
                    }
                    Some(())
                })
            };
            pipe(child.stdout.take().map(|o| Box::new(o) as _), false, tx.clone());
            pipe(child.stderr.take().map(|o| Box::new(o) as _), true, tx.clone());
        }
        if let Some((pid, ..)) = &files {
            _ = std::fs::write(pid, child.id().to_string());
        }
        Ok(child)
    };
    let mut child = spawn()?;
    let ended = loop {
        if child.try_wait()?.is_some() {
            break Ok(());
        }
        if files.as_ref().is_some_and(|(_, closed, _)| closed.exists()) {
            eprintln!("ax dev: the window was closed; the session ends");
            aio::lifecycle::stop(&mut child, Duration::from_secs(5));
            break Ok(());
        }
        if let Some((.., again)) = &files
            && std::fs::remove_file(again).is_ok()
        {
            eprintln!("ax dev: rebuilding the app whole and starting it again");
            aio::lifecycle::stop(&mut child, Duration::from_secs(10));
            child = spawn()?;
        }
        std::thread::sleep(Duration::from_millis(250));
    };
    if let Some((pid, closed, again)) = &files {
        _ = std::fs::remove_file(pid);
        _ = std::fs::remove_file(closed);
        _ = std::fs::remove_file(again);
    }
    ended
}

/// Stop pid `p` if it is still a `dx serve` (an earlier session's, by its
/// pid file: never a process found by name). True when one was stopped.
fn stop_stray(p: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        let cmd = std::fs::read(format!("/proc/{p}/cmdline")).unwrap_or_default();
        let args: Vec<&[u8]> = cmd.split(|b| *b == 0).collect();
        let is_dx = args.first().is_some_and(|a| a.ends_with(b"dx")) && args.get(1) == Some(&&b"serve"[..]);
        if !is_dx {
            return false;
        }
        // SAFETY: a signal to one pid this session's file named.
        unsafe { libc::kill(p as i32, libc::SIGTERM) };
        for _ in 0..50 {
            if !std::path::Path::new(&format!("/proc/{p}")).exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        true
    }
    #[cfg(not(target_os = "linux"))]
    {
        _ = p;
        false
    }
}

fn stop(app: &mut Option<(Child, aio::lifecycle::Watch)>) {
    if let Some((mut c, _watch)) = app.take() {
        aio::lifecycle::stop(&mut c, Duration::from_secs(10));
    }
}

/// `--fresh`: the private instance `plan` starts on new, empty data. Only
/// an instance's home is ever thrown away, never the user's, and never
/// while it runs.
pub fn throw_away(plan: Option<&aio::paths::Plan>) -> Result<(), String> {
    let p = plan
        .filter(|p| p.from == "--instance")
        .ok_or("--fresh needs --instance <name>: only a private instance's data is thrown away")?;
    if aio::paths::running(&p.data) {
        return Err(format!("the instance on {} is running: ax stop it first", p.data.display()));
    }
    let home = p.data.parent().ok_or("an instance's data has a home")?;
    match std::fs::remove_dir_all(home) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("{}: {e}", home.display())),
        _ => Ok(()),
    }
}

/// `ax dev <app> --android [--device serial] [--native]` (aio's
/// `dev:android`): on the attached emulator (before a phone) or phone, else
/// an emulator booted (`AIO_AVD`). An app with a web build runs as its dev
/// page: a thin dev APK, installed once beside the real app, shows what
/// `ax dev --web` serves over `adb reverse`, and each edit reloads it
/// there. Else (or `--native`) the app itself, as a debug dev copy beside
/// the real app, built for that device's CPU only and installed again on
/// every saved change. The app's log, its page's console and a crash reach
/// the terminal (`[device]`).
pub fn android(name: &str, device: Option<&str>, native: bool) -> Result<String, String> {
    let meta = metadata()?;
    let pkg = locate(&meta, name)?;
    let (app, cfg) = crate::build::app_of(&pkg)?;
    let dev = crate::install::ensure_device(device)?;
    let id = crate::android::application_id_of(&app.id, cfg.app.port_of.as_deref(), &cfg.build.android) + ".dev";
    if !native && crate::web::has_web(name) {
        let target = Path::new(meta["target_directory"].as_str().ok_or("cargo metadata: no target_directory")?);
        let (apk, hash) = crate::android::dev_apk(&app, &cfg, &crate::build::app_dir(&pkg)?, target)?;
        let record = target.join("aio").join("android-dev").join(format!("{}-{id}.installed", dev.serial));
        let installed = crate::install::adb_out(&["-s", &dev.serial, "shell", "pm", "path", &id])
            .is_ok_and(|o| o.contains("package:"));
        if !installed || std::fs::read_to_string(&record).ok().as_deref() != Some(hash.as_str()) {
            eprintln!("ax dev: installing {} on {}", apk.display(), dev.label());
            crate::install::install_on(&dev, &apk)?;
            _ = std::fs::write(&record, &hash);
        }
        // A session ended by a signal leaves its reverse: drop it now. Only
        // ax's own: another tool's reverses on the device stay.
        let reversed = target.join("aio").join("android-dev").join(format!("{}-{id}.reverse", dev.serial));
        if let Ok(p) = std::fs::read_to_string(&reversed) {
            _ = crate::install::adb_out(&["-s", &dev.serial, "reverse", "--remove", p.trim()]);
            _ = std::fs::remove_file(&reversed);
        }
        let mut logs = crate::install::logcat(&dev.serial)?;
        let on = dev.clone();
        let record = reversed.clone();
        let ran = crate::web::dev_then(name, move |port| {
            let p = format!("tcp:{port}");
            crate::install::adb_out(&["-s", &on.serial, "reverse", &p, &p])
                .map_err(|e| format!("{e}: the app can't reach the dev server"))?;
            _ = std::fs::write(&record, &p);
            let url = format!("http://127.0.0.1:{port}/");
            match crate::install::start_on(&on, &format!("{id}/{}", crate::android::DEV), &["--es", "url", &url]) {
                Ok(()) => eprintln!("ax dev: on {} → {url}: edits reload it there; Ctrl-C ends it", on.label()),
                Err(why) => eprintln!("ax dev: installed, but did not start ({why}): open \"(dev)\" from the launcher"),
            }
            Ok(())
        });
        _ = logs.kill();
        _ = logs.wait();
        if let Ok(p) = std::fs::read_to_string(&reversed) {
            _ = crate::install::adb_out(&["-s", &dev.serial, "reverse", "--remove", p.trim()]);
            _ = std::fs::remove_file(&reversed);
        }
        return ran;
    }
    let (_w, rx) = watch_these(&watched(&pkg, Path::new(""), &Watching::All))?;
    let abi = crate::install::abi_of(&crate::install::adb_out(&[
        "-s",
        &dev.serial,
        "shell",
        "getprop",
        "ro.product.cpu.abilist",
    ])?)
    .and_then(|a| crate::android::ABIS.iter().find(|(b, _)| *b == a))
    .ok_or_else(|| format!("{} runs a CPU ax builds no APK for", dev.label()))?;
    let ask = crate::android::Ask { debug: true, abis: std::slice::from_ref(abi), kind: crate::android::Apk::Dev };
    let mut logs = crate::install::logcat(&dev.serial)?;
    let component = format!("{id}/aio.rt.MainActivity");
    loop {
        let did = crate::build::android(name, ask).and_then(|(_, apks)| {
            let apk = apks.first().ok_or("the build made no APK")?;
            crate::install::install_on(&dev, apk)?;
            crate::install::start_on(&dev, &component, &[])
                .map_err(|why| format!("installed, but did not start ({why})"))
        });
        match did {
            Ok(()) => eprintln!("ax dev: {} (dev) runs on {}; watching for changes", app.name, dev.label()),
            Err(e) => eprintln!("ax dev: {e}"),
        }
        // What the build wrote (a CSS step's output) is not a change.
        settle(&rx);
        if rx.recv().is_err() {
            _ = logs.kill();
            _ = logs.wait();
            return Ok(String::new());
        }
        settle(&rx);
        eprintln!("ax dev: change — building for Android…");
    }
}

/// What a launch of `ax dev` with `args` asks of the app's dirs: always a
/// dev build's (pure).
fn dev_ask(args: &[String]) -> Option<aio::paths::Ask> {
    Some(aio::paths::Ask { dev: true, ..aio::paths::Ask::parse(args).ok()? })
}

pub fn run(name: &str, args: &[String]) -> Result<String, String> {
    let meta = metadata()?;
    let pkg = locate(&meta, name)?;
    claim_in(&meta, name)?;
    // The app's dirs, as this launch's flags have them: a dev build's
    // (its `[home] dev`), whatever build ax itself is. A release ax planned
    // its own, the person's real home, and wrote its pid and port there.
    let plan = crate::build::app_of(&pkg).ok().and_then(|(_, cfg)| aio::paths::plan_app(&cfg, &dev_ask(args)?).ok());
    let fresh = args.iter().any(|a| a == "--fresh");
    let tui = args.iter().any(|a| a == "--tui");
    // `--display <d>` is aio's, like `--fresh`: neither reaches the app.
    let at = args.iter().position(|a| a == "--display");
    let asked = at.and_then(|i| args.get(i + 1)).cloned().or_else(|| std::env::var("AIORT_DISPLAY").ok());
    if at.is_some_and(|i| i + 1 >= args.len()) {
        return Err("--display needs a value: auto, isolated, current or :N".into());
    }
    // `--size WxH`: the window's size for this run (its saved one untouched).
    let sz = args.iter().position(|a| a == "--size");
    let size = match sz {
        Some(i) => Some(crate::shot::size(args.get(i + 1).map_or("", String::as_str))?),
        None => None,
    };
    let ours = |i: usize| [at, sz].iter().flatten().any(|f| i == *f || i == f + 1);
    let args: &[String] = &args
        .iter()
        .enumerate()
        .filter(|(i, a)| *a != "--fresh" && *a != "--tui" && !ours(*i))
        .map(|(_, a)| a.clone())
        .collect::<Vec<_>>();
    let (args, cdp) = crate::cdp::take_flag(args)?;
    let (args, watching) = take_watch(&args)?;
    let args: &[String] = &args;
    let paths = watched(&pkg, &crate::build::app_dir(&pkg).unwrap_or_default(), &watching);
    if let Some(gone) = paths.iter().find(|p| matches!(watching, Watching::Only(_)) && !p.exists()) {
        return Err(format!("--watch: {} isn't there", gone.display()));
    }
    if fresh {
        throw_away(plan.as_ref())?;
    }
    // One port for the whole run (restarts and dx's alike), so `ax shot` finds it.
    let cdp = match plan.as_ref() {
        Some(p) => crate::cdp::open(&p.logs, cdp)?,
        None if cdp.is_some() => return Err(format!("--cdp: {name} has no aio.toml to keep its port by")),
        None => None,
    };
    if let Some(p) = cdp {
        eprintln!("ax dev: {name}'s window opens the DevTools protocol on 127.0.0.1:{p}");
    }
    use crate::display::Choice;
    use std::io::IsTerminal;
    let choice = crate::display::choose(asked.as_deref(), std::io::stdin().is_terminal(), crate::display::desktop())?;
    // Held for the whole run: dropped, the nested display stops.
    let nested = match choice {
        Choice::Nested => crate::display::nest(asked.as_deref(), "ax dev", true)?,
        _ => None,
    };
    // Contained unless `current` (aio's `am start`): no browser tab for the person either.
    let inherited = std::env::var("AIORT_ELECTRON_ARGS").ok();
    let put = |cmd: &mut Command| {
        if let Some(p) = cdp {
            cmd.env("AIORT_CDP", p.to_string());
        }
        if let Some((w, h)) = size {
            cmd.env("AIORT_WINDOW_SIZE", format!("{w}x{h}"));
        }
        match (&choice, &nested) {
            (_, Some(n)) => n.onto(cmd, inherited.as_deref()),
            (Choice::Named(d), _) => {
                crate::display::onto(cmd, Some(d));
                crate::display::contain(cmd);
            }
            (Choice::Nested, None) => crate::display::contain(cmd),
            (Choice::Current, _) => {}
        }
    };
    // Where a failed rebuild's errors are shown: the running app's window.
    let window = plan.clone().zip(crate::build::app_of(&pkg).ok()).map(|(p, (a, _))| (a.id, p));
    let logs = plan.map(|p| p.logs);
    // `ax restart` finds the session by it, and asks it (its flags, its display).
    let _session = logs.as_deref().filter(|_| watching != Watching::Off).and_then(Session::open);
    if watching == Watching::Off {
        // Built and run once, in the foreground, no reload.
        let (_w, rx) = watch_these(&[])?;
        css(&pkg, &rx);
        let bin = build(&pkg.name, logs.as_deref()).ok_or(format!("{name}: the build failed"))?;
        let mut app = launch(&bin, args, &put).ok_or(format!("{name}: it didn't start"))?;
        let ended = app.0.wait().map_err(|e| e.to_string())?;
        return Ok(if ended.success() { String::new() } else { format!("{name} ended: {ended}") });
    }
    match dx_version() {
        Some(v) if dx_matches(&v) => {
            eprintln!("ax dev: {name} with hot-patching (dx {DIOXUS})");
            // dx rebuilds on change (all of the app); the CSS step is aio's to run again.
            let (w, rx) = watch_these(&paths)?;
            css(&pkg, &rx);
            // A changed Cargo.toml (a dependency added): dx can't patch that in, so a full rebuild.
            let (p, l) = (pkg.clone(), logs.clone());
            std::thread::spawn(move || {
                while let Ok(m) = rx.recv() {
                    SAVED.store(now_ms(), std::sync::atomic::Ordering::Relaxed);
                    let manifest = settle(&rx) | m;
                    css(&p, &rx);
                    if let Some(l) = l.as_deref().filter(|_| manifest) {
                        eprintln!("ax dev: Cargo.toml changed");
                        _ = ask_restart(l);
                    }
                }
            });
            // The shell's `dev` feature: live rsx! and hot-patching; a release has neither.
            let dev = meta["packages"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|p| p["name"] == pkg.name.as_str())
                .and_then(|p| p["dependencies"].as_array())
                .is_some_and(|d| d.iter().any(|d| d["name"] == "aio-rs-shell"));
            if !dev {
                eprintln!("ax dev: {} doesn't depend on aio-rs-shell, so no live reload or hot-patch", pkg.name);
            }
            // dx's own screen only on `--tui` (its keys; a failed patch's raw JSON).
            let tui = tui && std::io::stdin().is_terminal();
            let port = free_port();
            let make = || {
                let mut dx = Command::new(crate::setup::dx());
                put(dx.args(dx_args(&pkg.name, port, dev, tui, args)));
                dx
            };
            let served = serve(&make, logs.as_deref(), !tui, window);
            drop(w);
            return served.map_err(|e| e.to_string()).map(|_| String::new());
        }
        Some(v) => {
            eprintln!("ax dev: dx is {}, ax needs {DIOXUS} to hot-patch; rebuilding on change instead", v.trim())
        }
        None => eprintln!(
            "ax dev: no dx; rebuilding on change instead (for edits in under a second: ax setup fetches it, \
             or {})",
            crate::setup::dx_fallback()
        ),
    }
    let (_w, rx) = watch_these(&paths)?;
    let shown: Vec<String> = match &watching {
        Watching::Only(_) => paths.iter().map(|p| p.display().to_string()).collect(),
        _ => pkg.dirs.iter().map(|d| d.display().to_string()).collect(),
    };
    eprintln!("ax dev: {name} — watching {}", shown.join(", "));
    css(&pkg, &rx);
    let mut app = build(&pkg.name, logs.as_deref()).and_then(|bin| launch(&bin, args, &put));
    let asked = || logs.as_ref().is_some_and(|l| std::fs::remove_file(l.join(RESTART)).is_ok());
    loop {
        // Wait for a change, then for the burst of saves to settle; or for `ax restart`.
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(_) => {
                settle(&rx);
                eprintln!("ax dev: change — building…");
            }
            Err(mpsc::RecvTimeoutError::Timeout) if asked() => eprintln!("ax dev: restart asked — building…"),
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        css(&pkg, &rx);
        if let Some(bin) = build(&pkg.name, logs.as_deref()) {
            stop(&mut app);
            eprintln!("ax dev: restarting {name}");
            app = launch(&bin, args, &put);
        } else {
            eprintln!("ax dev: build failed; the running app is unchanged and shows why");
            if let (Some(w), Some(l)) = (&window, &logs) {
                let errors = crate::web::fatal(&std::fs::read_to_string(l.join(BUILD_LOG)).unwrap_or_default());
                if let Err(e) = on_page(Some(w), &show_failed(&errors)) {
                    eprintln!("ax dev: the errors aren't on its page: {e}");
                }
            }
        }
    }
    stop(&mut app);
    Ok(String::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ax_dev_plans_a_dev_builds_home_whatever_build_ax_is() {
        let a = dev_ask(&["--profile=qa".to_string()]).unwrap();
        assert!(a.dev && a.profile.as_deref() == Some("qa"));
    }

    /// aio's diagnostic page: a failed rebuild's errors (from its build.log)
    /// on the running app's dev page, open; quotes and lines kept.
    #[test]
    fn a_failed_rebuild_shows_its_errors_on_the_running_apps_page() {
        let log =
            build_log(0, "   Compiling notes\nerror[E0308]: mismatched types: expected `\"a\"`\n --> src/ui.rs:2:5\n");
        let errors = crate::web::fatal(&log);
        assert_eq!(errors, "error[E0308]: mismatched types: expected `\"a\"`\n --> src/ui.rs:2:5");
        let js = format!(
            "let got = null; globalThis.window = {{ __aioDiag: (...a) => got = a }};
             {}; console.log(JSON.stringify(got));
             delete window.__aioDiag; {};",
            show_failed(&errors),
            show_failed("x")
        );
        let Ok(out) = Command::new("node").args(["-e", &js]).output() else {
            return eprintln!("skipped: no node");
        };
        assert!(out.status.success(), "a release page (no overlay) too: {}", String::from_utf8_lossy(&out.stderr));
        let got: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(got, serde_json::json!(["build failed", errors, true]));
    }

    /// dx labels every line the app writes ERROR: the label goes, the app's own level stays.
    #[test]
    fn the_apps_lines_keep_their_own_level_through_dx() {
        let said = |l: &str| dx_line(l).0;
        assert_eq!(said("  9.39s ERROR [aio] window ready").as_deref(), Some("[aio] window ready"));
        assert_eq!(said("  1.38s ERROR WARN  [page] slow").as_deref(), Some("WARN  [page] slow"));
        assert_eq!(said(" 12.00s ERROR ERROR [app] broke").as_deref(), Some("ERROR [app] broke"));
        let panic = "  3.00s ERROR thread 'main' panicked at src/lib.rs:1:1";
        assert_eq!(said(panic).as_deref(), Some("thread 'main' panicked at src/lib.rs:1:1"));
        assert_eq!(said("  2.0s ERROR [1:2/3.4:ERROR:bus.cc(408)] Failed to connect to the bus"), None);
        assert_eq!(said("  1.47s ERROR [9:1031/08.5:ERROR:dbus/object_proxy.cc:572] Failed to call method"), None);
        assert_eq!(dx_line("  4.1s  INFO Hotreloading: src/lib.rs").1, DxEvent::Landed, "markup landed");
    }

    #[test]
    fn an_error_whose_fix_is_aios_says_it() {
        let e = "error[E0277]: the trait bound `Note: aio::serde::Serialize` is not satisfied\n  --> src/lib.rs:9:5\n";
        let said = with_hints(e);
        assert!(said.starts_with(e.trim_end()) && said.contains("= ax: ") && said.contains("#[aio::data]"), "{said}");
        assert_eq!(with_hints("error[E0425]: cannot find value `x`\n"), "error[E0425]: cannot find value `x`\n");
    }

    /// dx 0.7.10's failed hot-patch of a workspace library: cargo's raw JSON.
    #[test]
    fn a_failed_hot_patch_is_said_as_rustc_renders_it() {
        let error = serde_json::json!({"$message_type": "diagnostic", "level": "error",
            "message": "no field `cuont` on type `Arc<Counter>`", "spans": [{"line_start": 34}],
            "rendered": "\u{1b}[1m\u{1b}[91merror[E0609]\u{1b}[0m: no field `cuont`\n  --> src/lib.rs:34:46\n"});
        let abort = serde_json::json!({"$message_type": "diagnostic", "level": "error", "spans": [],
            "rendered": "error: aborting due to 1 previous error\n"});
        let artifact = r#"{"$message_type":"artifact","artifact":"x.d","emit":"dep-info"}"#;
        let said = "error[E0609]: no field `cuont`\n  --> src/lib.rs:34:46".to_string();
        assert_eq!(dx_line(&format!("   {error}")), (Some(said.clone()), DxEvent::Error(said)));
        assert_eq!(
            dx_line(&abort.to_string()),
            (Some("error: aborting due to 1 previous error".into()), DxEvent::Said)
        );
        assert_eq!(dx_line(artifact), (None, DxEvent::Said));
        let failed = " 52.76s ERROR Build failed: Failed to replay workspace crate 'notes'";
        let said = "Build failed: Failed to replay workspace crate 'notes'";
        assert_eq!(dx_line(failed), (Some(said.into()), DxEvent::Failed));
        assert_eq!(dx_line(" 37.88s  INFO Hot-patching: src/lib.rs took 203ms").1, DxEvent::Landed);
        assert_eq!(dx_line("[aio] window ready"), (Some("[aio] window ready".into()), DxEvent::Said));
    }

    /// dx's lines without its time and level, its banner and Chromium's own logs.
    #[test]
    fn dxs_lines_are_compact() {
        let said = |l: &str| dx_line(l).0;
        assert_eq!(said(" 23.03s ERROR [aio] window ready").as_deref(), Some("[aio] window ready"));
        assert_eq!(said("  0.13s  INFO Hot-patching: src/lib.rs").as_deref(), Some("Hot-patching: src/lib.rs"));
        assert_eq!(
            said("               \u{1b}[33m1\u{1b}[0m: Failed").as_deref(),
            Some("\u{1b}[33m1\u{1b}[0m: Failed")
        );
        assert_eq!(said("  0.13s  INFO -----------------------------------"), None);
        assert_eq!(said("                Serving your app: notes! 🚀"), None);
        assert_eq!(said("                • Press \u{1b}[33m`r`\u{1b}[0m to rebuild the app"), None);
        let gl = " 23.20s ERROR [4056790:1031/070339.046834:ERROR:ui/gl/angle_platform_impl.cc:48] Display.cpp: no X";
        assert_eq!(said(gl), None);
        assert_eq!(said(" 23.20s ERROR ERR: Display.cpp:1115 (initialize): no X"), None);
        let fatal = "[1:1031/070339.0:FATAL:main.cc:1] gone";
        assert_eq!(said(fatal).as_deref(), Some(fatal), "a crash is said");
        assert_eq!(said("error[E0609]: no field").as_deref(), Some("error[E0609]: no field"));
        assert_eq!(said("   |").as_deref(), Some("   |"), "rustc's own indent stays");
    }

    #[test]
    fn only_real_changes_restart() {
        use notify::EventKind;
        use notify::event::{AccessKind, CreateKind, DataChange, MetadataKind, ModifyKind, RemoveKind};
        assert!(changes(&EventKind::Create(CreateKind::File)));
        assert!(changes(&EventKind::Modify(ModifyKind::Data(DataChange::Content))));
        assert!(changes(&EventKind::Remove(RemoveKind::File)));
        assert!(!changes(&EventKind::Access(AccessKind::Read)));
        assert!(!changes(&EventKind::Modify(ModifyKind::Metadata(MetadataKind::WriteTime))));
    }

    #[test]
    fn finds_the_launcher_and_the_app_it_runs() {
        let meta = serde_json::json!({
            "target_directory": "/w/target",
            "packages": [{
                "name": "notes", "manifest_path": "/w/notes/Cargo.toml",
                "targets": [{ "name": "notes", "kind": ["lib"] }], "dependencies": []
            }, {
                "name": "notes-app", "manifest_path": "/w/notes/app/Cargo.toml",
                "targets": [{ "name": "notes", "kind": ["bin"] }],
                "dependencies": [{ "name": "notes", "path": "/w/notes" }, { "name": "aio-rs", "path": "/elsewhere/aio" }]
            }]
        });
        let p = locate(&meta, "notes").unwrap();
        assert_eq!(p.name, "notes-app");
        assert_eq!(p.dirs, [PathBuf::from("/w/notes/app"), PathBuf::from("/w/notes")], "not the framework");
        assert_eq!(
            p.bin,
            Path::new("/w/target")
                .join(host_triple())
                .join("desktop-dev")
                .join(format!("notes{}", std::env::consts::EXE_SUFFIX))
        );
        assert!(locate(&meta, "nope").unwrap_err().contains("no binary"));
        // An app split into crates: the ones its library uses are its code too.
        let mut meta = meta;
        meta["packages"][0]["dependencies"] = serde_json::json!([{ "name": "notes-views", "path": "/w/notes/views" }]);
        meta["packages"].as_array_mut().unwrap().push(serde_json::json!({
            "name": "notes-views", "manifest_path": "/w/notes/views/Cargo.toml",
            "targets": [{ "name": "notes_views", "kind": ["lib"] }],
            "dependencies": [{ "name": "notes", "path": "/w/notes" }]
        }));
        let dirs = ["/w/notes/app", "/w/notes", "/w/notes/views"].map(PathBuf::from);
        assert_eq!(locate(&meta, "notes").unwrap().dirs, dirs, "each once");
    }

    #[test]
    fn a_build_targets_label_names_the_binary_it_builds() {
        // aio am-components: `am start <label>`.
        let d = tempfile::tempdir().unwrap();
        let w = d.path();
        std::fs::create_dir_all(w.join("notes/server")).unwrap();
        std::fs::write(
            w.join("notes/aio.toml"),
            "[app]\nid = \"notes\"\n[build]\ntargets = [{ name = \"api\", kind = \"server\", bin = \"notes-server\" }, \"deb\"]\n",
        )
        .unwrap();
        std::fs::write(
            w.join("notes/server/aio.toml"),
            "[app]\nid = \"notes-server\"\nmode = \"server\"\n[server]\nlisten = \"127.0.0.1:0\"\n",
        )
        .unwrap();
        let pkg = |name: &str, dir: &str, bin: &str| {
            serde_json::json!({ "name": name, "manifest_path": w.join(dir).join("Cargo.toml"),
                "targets": [{ "name": bin, "kind": ["bin"] }], "dependencies": [] })
        };
        let meta = serde_json::json!({ "target_directory": w.join("target"), "packages": [
            pkg("notes-app", "notes", "notes"), pkg("notes-server", "notes/server", "notes-server"),
        ]});
        assert_eq!(locate(&meta, "api").unwrap().name, "notes-server");
        assert_eq!(locate(&meta, "api").unwrap().bin, dev_bin(&w.join("target"), "notes-server"));
        assert_eq!(locate(&meta, "notes").unwrap().name, "notes-app", "a binary's own name still");
        assert!(locate(&meta, "deb").unwrap_err().contains("no binary `deb`"), "a label without a bin is no app");
    }

    #[test]
    fn watching_is_all_the_app_none_or_dirs_inside_it() {
        // aio's `am dev --watch=false`, `watch: ["src/ui"]`.
        let s = |v: &[&str]| v.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        assert_eq!(take_watch(&s(&["--x"])), Ok((s(&["--x"]), Watching::All)));
        assert_eq!(take_watch(&s(&["--watch=false", "a.md"])), Ok((s(&["a.md"]), Watching::Off)));
        assert_eq!(take_watch(&s(&["--watch=src/ui,assets"])).unwrap().1, Watching::Only(s(&["src/ui", "assets"])));
        assert_eq!(take_watch(&s(&["--", "--watch=false"])).unwrap().1, Watching::All, "the app's own word");
        for bad in ["--watch=", "--watch=../x", "--watch=/etc", "--watch"] {
            assert!(take_watch(&s(&[bad])).is_err(), "{bad}");
        }
        let pkg =
            Pkg { name: "n".into(), dirs: vec!["/w/app".into(), "/w".into()], bin: "/b".into(), version: "1".into() };
        assert_eq!(watched(&pkg, Path::new("/w"), &Watching::Off), Vec::<PathBuf>::new());
        assert_eq!(watched(&pkg, Path::new("/w"), &Watching::Only(s(&["src/ui"]))), [PathBuf::from("/w/src/ui")]);
        let all = watched(&pkg, Path::new("/w"), &Watching::All);
        assert!(all.contains(&PathBuf::from("/w/app/src")) && all.contains(&PathBuf::from("/w/Cargo.toml")), "{all:?}");
    }

    #[test]
    fn the_app_of_a_folder_is_the_nearest_one() {
        let d = tempfile::tempdir().unwrap();
        let w = d.path();
        for dir in ["notes/app", "notes/src/ui", "mail", "tool"] {
            std::fs::create_dir_all(w.join(dir)).unwrap();
        }
        std::fs::write(w.join("notes/aio.toml"), "").unwrap();
        std::fs::write(w.join("mail/aio.toml"), "").unwrap();
        let pkg = |name: &str, dir: &str, bin: &str, deps: &[&str]| {
            let deps: Vec<_> = deps.iter().map(|p| serde_json::json!({ "path": w.join(p) })).collect();
            serde_json::json!({
                "name": name, "manifest_path": w.join(dir).join("Cargo.toml"),
                "targets": [{ "name": bin, "kind": [if bin.is_empty() { "lib" } else { "bin" }] }],
                "dependencies": deps
            })
        };
        let meta = serde_json::json!({ "target_directory": w.join("target"), "packages": [
            pkg("notes", "notes", "", &[]), pkg("notes-app", "notes/app", "notes", &["notes"]),
            pkg("mail", "mail", "mail", &[]), pkg("tool", "tool", "tool", &[]),
        ]});
        assert_eq!(here(&meta, &w.join("notes/src/ui")), Ok("notes".into()));
        assert_eq!(here(&meta, &w.join("notes/app")), Ok("notes".into()));
        assert_eq!(here(&meta, &w.join("mail")), Ok("mail".into()));
        // No aio.toml: not an app; above them all: none.
        assert!(here(&meta, &w.join("tool")).unwrap_err().contains("no app here"));
        assert!(here(&meta, w).unwrap_err().contains("no app here"));
    }

    #[test]
    fn fresh_throws_away_only_a_stopped_instance() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("instances/agent1/notes");
        std::fs::create_dir_all(home.join("data")).unwrap();
        std::fs::write(home.join("data/state.db"), "x").unwrap();
        let plan = |from: &'static str| aio::paths::Plan {
            data: home.join("data"),
            logs: home.join("logs"),
            cache: home.join("cache"),
            store: home.join("data/state.db"),
            profile: None,
            port_of: None,
            from,
            asked: false,
        };
        assert!(throw_away(None).unwrap_err().contains("needs --instance"));
        assert!(throw_away(Some(&plan("default"))).unwrap_err().contains("needs --instance"));
        assert!(home.join("data/state.db").exists(), "the user's data is never thrown away");
        {
            let _running = aio::paths::AppDirs::at(&home, "notes").unwrap();
            assert!(throw_away(Some(&plan("--instance"))).unwrap_err().contains("is running"));
        }
        // A child another test forks shares the lock's descriptor until it execs.
        let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while aio::paths::running(&home.join("data")) {
            assert!(std::time::Instant::now() < end, "the dropped lock is never released");
            std::thread::yield_now();
        }
        throw_away(Some(&plan("--instance"))).unwrap();
        assert!(!home.exists());
        throw_away(Some(&plan("--instance"))).unwrap();
    }

    #[test]
    fn hot_patches_only_with_the_same_dioxus() {
        assert!(dx_matches("dioxus 0.7.10 (abc1234)"));
        assert!(!dx_matches("dioxus 0.7.9 (abc1234)"));
        assert!(!dx_matches("dioxus 0.7.100"));
        let a = dx_args("notes-app", 4242, false, true, &["a.md".into(), "--x".into()]);
        assert_eq!(
            a.join(" "),
            "serve --hot-patch -p notes-app --renderer native --open false --port 4242 --args=a.md --x"
        );
        let a = dx_args("notes-app", 4242, true, true, &[]);
        assert_eq!(
            a.join(" "),
            "serve --hot-patch -p notes-app --renderer native --open false --port 4242 --features aio-shell/dev"
        );
        let a = dx_args("notes-app", 4242, false, false, &[]).join(" ");
        assert_eq!(a, "serve --hot-patch -p notes-app --renderer native --open false --port 4242 --interactive false");
    }

    #[test]
    fn a_debug_build_is_dxs_and_its_binary_is_the_one_cargo_wrote() {
        let l = layout().join(" ");
        assert!(l.ends_with(&format!("--profile desktop-dev --target {}", host_triple())), "{l}");
        assert!(l.contains("profile.desktop-dev.strip=false"), "{l}");
        assert!(!host_triple().is_empty());
        let msgs = [
            r#"{"reason":"compiler-artifact","target":{"name":"aio"},"executable":null}"#,
            "not json",
            r#"{"reason":"compiler-artifact","target":{"name":"notes"},"executable":"/t/x/desktop-dev/notes"}"#,
            r#"{"reason":"build-finished","success":true}"#,
        ];
        assert_eq!(executable(&msgs.join("\n")), Some(PathBuf::from("/t/x/desktop-dev/notes")));
        assert_eq!(executable(msgs[0]), None);
    }

    #[test]
    fn a_binary_name_is_one_apps_in_the_shared_build_folder() {
        let d = tempfile::tempdir().unwrap();
        let (target, a, b) = (d.path().join("target"), d.path().join("a"), d.path().join("b"));
        for w in [&a, &b] {
            std::fs::create_dir_all(w).unwrap();
            std::fs::write(w.join("Cargo.toml"), "").unwrap();
        }
        claim(&target, "notes", &a).unwrap();
        claim(&target, "notes", &a).unwrap();
        claim(&target, "other", &b).unwrap();
        let e = claim(&target, "notes", &b).unwrap_err();
        assert!(e.contains("already") && e.contains(&a.display().to_string()), "{e}");
        std::fs::remove_dir_all(&a).unwrap();
        claim(&target, "notes", &b).expect("a removed app's name is free again");
    }

    #[test]
    fn worktrees_of_one_app_share_its_binary_name() {
        let d = tempfile::tempdir().unwrap();
        let (target, repo, tree) = (d.path().join("target"), d.path().join("repo"), d.path().join("tree"));
        // A repository with two apps, and a worktree of it (git's layout).
        let own = repo.join(".git").join("worktrees").join("tree");
        std::fs::create_dir_all(&own).unwrap();
        std::fs::write(own.join("commondir"), "../..\n").unwrap();
        for app in ["notes", "chat"] {
            for w in [&repo, &tree] {
                std::fs::create_dir_all(w.join(app)).unwrap();
                std::fs::write(w.join(app).join("Cargo.toml"), "").unwrap();
            }
        }
        std::fs::write(tree.join(".git"), format!("gitdir: {}\n", own.display())).unwrap();
        claim(&target, "notes", &repo.join("notes")).unwrap();
        claim(&target, "notes", &tree.join("notes")).expect("the same app in a worktree");
        claim(&target, "notes", &repo.join("notes")).expect("and back");
        let e = claim(&target, "notes", &tree.join("chat")).unwrap_err();
        assert!(e.contains("already"), "another app of the repository: {e}");
    }

    #[test]
    fn ignores_build_output_and_editor_files() {
        assert!(relevant(Path::new("/w/notes/src/main.rs")));
        assert!(!relevant(Path::new("/w/target/debug/x")));
        assert!(!relevant(Path::new("/w/notes/src/.main.rs.swp")));
        assert!(!relevant(Path::new("/w/notes/src/main.rs~")));
    }
}

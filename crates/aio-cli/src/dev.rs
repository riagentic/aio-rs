//! `ax dev <app> [args…]`: run an app while you edit it. Edits reach the
//! running app with its state kept: a stylesheet and `rsx!` markup at once,
//! Rust hot-patched by ax itself (`crate::hotpatch`, a small crate's in
//! under a second) in every workspace crate (views and cell methods alike).
//! What can't be patched (a `Cargo.toml`, a platform ax can't patch on)
//! builds again; a good build restarts the app (its state comes back from
//! the log), a failed one leaves the running app alone.
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::mpsc;
use std::time::Duration;

use crate::hotpatch::cargo_out::Stop;

/// The profile of every debug build aio runs (`ax dev`, `ax test`,
/// `ax lint`): one copy of the app's libraries in `<target>/desktop-dev`.
pub const PROFILE: &str = "desktop-dev";

/// This machine's target (`rustc -vV`'s host).
pub fn host_triple() -> &'static str {
    static HOST: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HOST.get_or_init(|| {
        let out = Command::new("rustc").arg("-vV").output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
        let host = out.ok().and_then(|o| o.lines().find_map(|l| l.strip_prefix("host: ").map(String::from)));
        // No rustc: cargo can't build either, and says so.
        host.or_else(|| crate::platform::host().ok().map(|p| p.triple.into())).unwrap_or_default()
    })
}

/// cargo's arguments for that layout: the profile (defined here too, for
/// an app whose Cargo.toml doesn't), unstripped, for this machine. No
/// `--target`: with one, cargo builds every library a build script or
/// macro shares with the app twice (libc, serde, base64…): the
/// template's first build 68 → 62 CPU-s.
pub fn layout() -> Vec<String> {
    let strip = format!("profile.{PROFILE}.strip=false");
    let inherits = format!("profile.{PROFILE}.inherits=\"dev\"");
    // The app's Rust as a release build's: no debug assertions, no overflow
    // checks, so every `cfg!(debug_assertions)` (the app's, its libraries',
    // std's) reads as it does in the build people get. What `ax dev` adds
    // is tooling behind aio's `dev` feature, inert until it is used.
    let assertions = format!("profile.{PROFILE}.debug-assertions=false");
    let overflow = format!("profile.{PROFILE}.overflow-checks=false");
    let mut l = vec!["--config", &strip, "--config", &inherits, "--config", &assertions, "--config", &overflow];
    l.extend(["--profile", PROFILE]);
    l.into_iter().map(String::from).collect()
}

/// The build folder every cargo ax runs shares (`CARGO_TARGET_DIR`, set by
/// `ax` for its children). The person's own (`AIORT_TARGET_DIR`, else
/// `CARGO_TARGET_DIR`), else one per machine and toolchain in aio's cache
/// home (`~/.cache/aio-rt/target/<toolchain>`). Every app built with the
/// same features and profile builds its libraries once on this machine,
/// nothing of this machine is written into a repo, and two toolchains never
/// thrash one folder's fingerprints. Two builds at once share it safely:
/// cargo locks the folder, and `claim` keeps two apps' binaries apart.
pub fn shared_target() -> PathBuf {
    let asked = std::env::var("RUSTUP_TOOLCHAIN").ok();
    shared_target_with(&workspace_root(&std::env::current_dir().unwrap_or_default()), asked.as_deref())
}

/// The root of the workspace `dir` is in, as cargo finds it (pure but for
/// reading files): the nearest folder at or above `dir` whose `Cargo.toml`
/// has a `[workspace]`, else the nearest with a `Cargo.toml`, else `dir`.
/// Its toolchain names the build folder, wherever in it ax is started.
pub fn workspace_root(dir: &Path) -> PathBuf {
    let manifest = |d: &Path| std::fs::read_to_string(d.join("Cargo.toml")).ok();
    let workspace = |d: &&Path| {
        manifest(d).and_then(|t| toml::from_str::<toml::Table>(&t).ok()).is_some_and(|t| t.contains_key("workspace"))
    };
    let found = dir.ancestors().find(workspace).or_else(|| dir.ancestors().find(|d| manifest(d).is_some()));
    found.unwrap_or(dir).to_path_buf()
}

/// [`shared_target`] for the app in `dir`, built with toolchain `asked`
/// when one is (an app `ax create` is about to write: the template's pin).
pub fn shared_target_with(dir: &Path, asked: Option<&str>) -> PathBuf {
    match std::env::var_os(TARGET_DIR).filter(|v| !v.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => aio::paths::cache_home().join("target").join(toolchain_key(dir, asked)),
    }
}

/// The person's own build folder for ax. A `CARGO_TARGET_DIR` ax is
/// started with is taken as this (`keep_persons_target`): ax sets
/// `CARGO_TARGET_DIR` itself for its cargo.
pub const TARGET_DIR: &str = "AIORT_TARGET_DIR";

/// The `CARGO_TARGET_DIR` an ax set for its children (an ax among them
/// names its own, by its own folder's toolchain).
const SET_BY_AX: &str = "AIORT_TARGET_BY_AX";

/// A `CARGO_TARGET_DIR` the person set (not one an ax set) is ax's folder
/// (`AIORT_TARGET_DIR`, unless that is set too).
///
/// # Safety
/// As `std::env::set_var`: before any thread is started.
pub unsafe fn keep_persons_target() {
    let set = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
    if let Some(d) = persons_target(set(TARGET_DIR), set("CARGO_TARGET_DIR"), set(SET_BY_AX)) {
        // SAFETY: the caller's.
        unsafe { std::env::set_var(TARGET_DIR, d) };
    }
}

/// The `CARGO_TARGET_DIR` to keep as the person's (pure): theirs when
/// `AIORT_TARGET_DIR` isn't set and an ax didn't set it (`by_ax`).
fn persons_target(
    ours: Option<std::ffi::OsString>,
    cargo: Option<std::ffi::OsString>,
    by_ax: Option<std::ffi::OsString>,
) -> Option<std::ffi::OsString> {
    cargo.filter(|d| ours.is_none() && by_ax.as_ref() != Some(d))
}

/// `CARGO_TARGET_DIR` for every cargo this ax runs: [`shared_target`].
///
/// # Safety
/// As `std::env::set_var`: before any thread is started.
pub unsafe fn set_shared_target() {
    let root = workspace_root(&std::env::current_dir().unwrap_or_default());
    let asked = std::env::var("RUSTUP_TOOLCHAIN").ok().filter(|t| !t.is_empty());
    let pinned = toolchain_at(&root, asked.as_deref(), &rustup_active);
    let t = shared_target_with(&root, pinned.as_deref());
    // SAFETY: the caller's.
    unsafe {
        // Every cargo ax runs (in whatever folder: rustup picks by it) builds
        // with the toolchain the folder is named by.
        if let (None, Some(p)) = (&asked, &pinned) {
            std::env::set_var("RUSTUP_TOOLCHAIN", p);
        }
        std::env::set_var("CARGO_TARGET_DIR", &t);
        std::env::set_var(SET_BY_AX, &t);
    }
}

/// The toolchain of the workspace at `root` (pure but for reading files
/// and `active`): `asked` (`RUSTUP_TOOLCHAIN`), else the channel of the
/// nearest toolchain file at or above `root`, else what rustup picks
/// there (`active`); `None` without rustup (one toolchain, nothing to pick).
pub fn toolchain_at(root: &Path, asked: Option<&str>, active: &dyn Fn(&Path) -> Option<String>) -> Option<String> {
    asked.map(str::to_string).or_else(|| toolchain_file(root)).or_else(|| active(root))
}

/// The toolchain rustup picks in `dir`, by name (`stable-x86_64-…`).
fn rustup_active(dir: &Path) -> Option<String> {
    let out = Command::new("rustup").args(["show", "active-toolchain"]).current_dir(dir).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    out.status.success().then(|| text.split_whitespace().next().map(str::to_string)).flatten()
}

/// The channel of the nearest `rust-toolchain.toml` (or legacy
/// `rust-toolchain`) at or above `dir`.
fn toolchain_file(dir: &Path) -> Option<String> {
    let channel = |p: &Path| -> Option<String> {
        let text = std::fs::read_to_string(p).ok()?;
        let parsed = toml::from_str::<toml::Table>(&text).ok();
        match parsed {
            Some(t) => t.get("toolchain")?.get("channel")?.as_str().map(str::to_string),
            None => text.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#')).map(str::to_string),
        }
    };
    dir.ancestors().find_map(|d| channel(&d.join("rust-toolchain.toml")).or_else(|| channel(&d.join("rust-toolchain"))))
}

/// The toolchain cargo would build `dir` with, as a folder name (pure but
/// for reading files): `RUSTUP_TOOLCHAIN`, else the nearest
/// `rust-toolchain.toml` (or legacy `rust-toolchain`) channel, else
/// `default`. A changed `stable` is the same name: cargo rebuilds what the
/// new compiler needs and keeps the rest (its hashes name the compiler).
pub fn toolchain_key(dir: &Path, asked: Option<&str>) -> String {
    let found = asked.map(str::to_string).or_else(|| toolchain_file(dir));
    let name = found.unwrap_or_else(|| "default".into());
    let safe: String =
        name.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect();
    if safe.is_empty() || safe.starts_with('.') { "default".into() } else { safe }
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
    /// `<its aio-shell dependency>/dev`: the feature that takes hot-patches
    /// in (`aio-shell/dev`); `None`: it has no window shell, nothing to patch.
    pub dev_feature: Option<String>,
    /// The shell features its aio.toml turns on (`build::shell_features`:
    /// `updates` for an `[updates] url`): every dev build takes them, so the
    /// `updates` cell works under `ax dev` as in the release.
    pub features: Vec<String>,
}

impl Pkg {
    /// Its features for a dev-profile build, `dev`'s tooling too when
    /// asked, comma-separated (`--features`); None: none (pure).
    pub fn features_with(&self, dev: bool) -> Option<String> {
        let all: Vec<&str> =
            self.dev_feature.iter().filter(|_| dev).chain(&self.features).map(String::as_str).collect();
        (!all.is_empty()).then(|| all.join(","))
    }
}

/// `cargo metadata` for the workspace here.
pub fn metadata() -> Result<serde_json::Value, String> {
    let Some(dir) = IN.with_borrow(Clone::clone) else { return cargo_metadata(None) };
    let mut known = KNOWN.lock().unwrap_or_else(|e| e.into_inner());
    known.retain(|(_, at, _)| at.elapsed() < KNOWN_FOR);
    if let Some((_, _, m)) = known.iter().find(|(d, ..)| *d == dir) {
        return m.clone();
    }
    let m = cargo_metadata(Some(&dir));
    known.push((dir, std::time::Instant::now(), m.clone()));
    m
}

thread_local! {
    /// The folder [`in_dir`] runs this thread's verbs in.
    static IN: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// What `cargo metadata` said of a folder [`in_dir`] ran in, and when.
type Known = Vec<(PathBuf, std::time::Instant, Result<serde_json::Value, String>)>;
static KNOWN: std::sync::Mutex<Known> = std::sync::Mutex::new(Vec::new());
/// How long [`in_dir`]'s answers stand: a workspace's packages rarely change.
const KNOWN_FOR: Duration = Duration::from_secs(30);

/// `f`, its verbs run as if in folder `dir` (`ax ui` reads in-process, as
/// many threads at once): `cargo metadata` asked there, its answer kept
/// [`KNOWN_FOR`], not asked again on each read.
pub fn in_dir<T>(dir: PathBuf, f: impl FnOnce() -> T) -> T {
    IN.set(Some(dir));
    let r = f();
    IN.set(None);
    r
}

fn cargo_metadata(dir: Option<&Path>) -> Result<serde_json::Value, String> {
    let mut cmd = Command::new("cargo");
    if let Some(d) = dir {
        cmd.current_dir(d);
    }
    let out = cmd
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
    let dev_feature = shell_dev_feature(pkg);
    let cfg = aio_config::config::find(&dirs[0])
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|t| aio::config::Config::parse(&t).ok());
    let features = cfg.map(|c| crate::build::shell_features(meta, &pkg_name, &c, false)).unwrap_or_default();
    Some(Pkg { name: pkg_name, dirs, bin: dev_bin(Path::new(target), name), version, dev_feature, features })
}

/// The `dev` feature of package `pkg`'s aio-shell dependency, by the name
/// its manifest gives it (`aio-shell/dev`), if it has one (pure).
pub fn shell_dev_feature(pkg: &serde_json::Value) -> Option<String> {
    let deps = pkg["dependencies"].as_array()?;
    let shell = deps.iter().find(|d| d["name"] == "aio-rs-shell" && d["kind"].is_null())?;
    let named = shell["rename"].as_str().or(shell["name"].as_str())?;
    Some(format!("{named}/dev"))
}

/// Binary `name` of a debug build into `target` ([`layout`]).
pub fn dev_bin(target: &Path, name: &str) -> PathBuf {
    target.join(PROFILE).join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
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

/// A port nothing listens on now.
pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()).map_or(18731, |a| a.port())
}

/// Changes that matter: sources and assets, not build output or an editor's
/// own files (vim's `4913` probe and swap files, emacs' `#x#`, `x~`, `*.tmp`,
/// JetBrains' safe-write copies, hidden files) (pure).
pub fn relevant(p: &Path) -> bool {
    let s = format!("/{}", p.to_string_lossy().replace('\\', "/"));
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let editors = name == "4913"
        || name.starts_with('.')
        || (name.starts_with('#') && name.ends_with('#'))
        || [".swp", ".swx", ".tmp", "~", "___jb_tmp___", "___jb_old___"].iter().any(|e| name.ends_with(e));
    !s.contains("/target/") && !editors
}

/// The paths of a settled burst that are changes (pure but for `exists`):
/// each that is there, or was a file the running app was built from (a
/// deleted source). One gone that never was a source (an editor's or a
/// tool's temporary copy, written and renamed over the real file: `sed -i`'s
/// `sedXXXXXX`, an agent's `x.rs.tmp.123`) is no change: it would cost a
/// rebuild and a restart for nothing.
pub fn real_changes(changed: Vec<PathBuf>, known: &dyn Fn(&Path) -> bool) -> Vec<PathBuf> {
    changed.into_iter().filter(|p| p.exists() || known(p)).collect()
}

/// A real change. Not a read: a build reads every source, and reads (access
/// events) would restart the app forever. Not a `touch` either.
pub fn changes(kind: &notify::EventKind) -> bool {
    use notify::EventKind::{Create, Modify, Remove};
    use notify::event::ModifyKind;
    matches!(kind, Create(_) | Modify(_) | Remove(_)) && !matches!(kind, Modify(ModifyKind::Metadata(_)))
}

/// Build package `name` ([`layout`]), with ax dev's tooling when `dev`
/// ([`build_features`]): its binary, as cargo says it wrote it; a failed
/// build's errors are also kept in `logs/build.log` for `aio errors` (a good
/// build removes it).
pub fn build(pkg: &Pkg, logs: Option<&Path>, dev: bool) -> Option<PathBuf> {
    match build_or_stale(pkg, logs, dev, &Stop::default()) {
        Built::Bin(b) => Some(b),
        Built::Failed | Built::Stale => None,
    }
}

/// What a dev build came to.
pub enum Built {
    Bin(PathBuf),
    /// It doesn't compile (its errors kept in `build.log`).
    Failed,
    /// A newer save stopped it ([`Stop::stale`]).
    Stale,
}

/// [`build`], or stopped by a newer save (`stop`).
fn build_or_stale(pkg: &Pkg, logs: Option<&Path>, dev: bool, stop: &Stop) -> Built {
    let name = &pkg.name;
    let stamp = pkg.dirs.first().map_or_else(String::new, |d| crate::build::assets_stamp(d));
    let mut cmd = Command::new("cargo");
    cmd.args(["build", "-p", name, "--message-format=json-render-diagnostics"])
        .env("AIORT_ASSETS", stamp)
        .args(layout())
        .args(build_features(pkg, dev));
    // Each line as cargo says it (its progress bar on a terminal).
    let (bin, text) = match crate::hotpatch::cargo_out::run(&mut cmd, true, stop) {
        Ok(o) if o.stopped => return Built::Stale,
        Ok(o) => (o.status.success().then(|| executable(&o.stdout)).flatten(), o.said),
        Err(e) => {
            eprintln!("ax: can't run cargo: {e}");
            (None, format!("can't run cargo: {e}"))
        }
    };
    if let Some(h) = hints(&text) {
        eprintln!("{h}");
    }
    let text = with_hints(&text);
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
    bin.map_or(Built::Failed, Built::Bin)
}

/// The features a dev-profile build of `pkg` is built with (pure): its
/// aio.toml's ([`Pkg::features`]), and for an `ax dev` run (`dev`)
/// aio-shell's `dev`, as its fat build has it; for any other (`ax start`)
/// not that: it runs as the release build does, with no operator door, no
/// DevTools port, its process hardened.
pub fn build_features(pkg: &Pkg, dev: bool) -> Vec<String> {
    pkg.features_with(dev).into_iter().flat_map(|f| ["--features".to_string(), f]).collect()
}

/// Compiler errors whose fix is aio's to say, by what rustc says, and the fix.
const HINTS: &[(&[&str], &str)] = &[(
    &["aio::serde::Serialize", "aio::serde::Deserialize"],
    "ax: a type a cell keeps (a field's, a method argument's) is saved and sent: put #[aio::data] on it \
     (it derives serde through aio: no serde dependency to add)",
)];

/// Compiler output `text` with aio's hint after it for each error it knows (pure).
pub fn with_hints(text: &str) -> String {
    match hints(text) {
        None => text.to_string(),
        Some(h) => format!("{}\n{h}\n", text.trim_end()),
    }
}

/// aio's hints for the errors in `text` it knows, one `  = ` line each (pure).
pub fn hints(text: &str) -> Option<String> {
    let hints: Vec<&str> =
        HINTS.iter().filter(|(said, _)| said.iter().any(|s| text.contains(s))).map(|(_, h)| *h).collect();
    (!hints.is_empty()).then(|| format!("  = {}", hints.join("\n  = ")))
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

/// `--watch=false` (`--no-watch`) or `--watch=<dir,dir>` taken out of a launch's args,
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
            _ if i < end && a == "--no-watch" => w = Watching::Off,
            _ if i < end && a == "--watch" => return Err("--watch=false, or --watch=<dir,dir> inside the app".into()),
            _ => rest.push(a.clone()),
        }
    }
    Ok((rest, w))
}

/// `[dev] watch` (`dirs`) when no `--watch` said otherwise (pure).
pub fn or_config(w: Watching, dirs: Vec<String>) -> Watching {
    match w {
        Watching::All if !dirs.is_empty() => Watching::Only(dirs),
        w => w,
    }
}

/// The paths `w` watches for `pkg` (pure; `Only`: under its app dir `app`).
pub fn watched(pkg: &Pkg, app: &Path, w: &Watching) -> Vec<PathBuf> {
    match w {
        Watching::All => pkg.dirs.iter().flat_map(|d| ["src", "assets", "Cargo.toml"].map(|sub| d.join(sub))).collect(),
        Watching::Off => vec![],
        Watching::Only(dirs) => dirs.iter().map(|d| app.join(d)).collect(),
    }
}

/// Whether `changed` has a `Cargo.toml` (a dependency): no patch takes
/// it in, so the app is built again (pure).
fn manifest(changed: &[PathBuf]) -> bool {
    changed.iter().any(|p| p.file_name().is_some_and(|n| n == "Cargo.toml"))
}

/// The files the build of `artifact` read, from cargo's dep-info beside it
/// (`<bin>.d`), none of cargo's own or the build folder's: `aio.toml`,
/// `include_str!` files, an aio-rs checkout the app depends on by path.
pub fn deps_of(artifact: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(artifact.with_extension("d")) else { return vec![] };
    // `<target>/<triple>/<profile>/<bin>`.
    let target = artifact.ancestors().nth(3).unwrap_or(artifact);
    crate::watch::dep_info(&text, &crate::watch::not_the_apps(target))
}

/// Whether `changed` is stylesheets alone (pure): a debug build's
/// `use_css!` reads them from disk as they are saved.
fn sheets_only(changed: &[PathBuf]) -> bool {
    !changed.is_empty() && changed.iter().all(|p| p.extension().is_some_and(|x| x == "css"))
}

/// The bytes of `bin`, hashed: a build that changed nothing leaves them as they were.
fn digest(bin: &Path) -> Option<u64> {
    use std::hash::Hasher;
    use std::io::Read;
    let mut f = std::fs::File::open(bin).ok()?;
    let (mut h, mut buf) = (std::hash::DefaultHasher::new(), vec![0; 1 << 20]);
    loop {
        match f.read(&mut buf).ok()? {
            0 => return Some(h.finish()),
            n => h.write(&buf[..n]),
        }
    }
}

/// aio's restart-storm guard (`server/dev-restart.ts`): a change this soon
/// after a restart, this many restarts in a row, is a loop (a formatter, a
/// generator, another watcher), not a person saving: each next rebuild waits.
pub const STORM: Duration = Duration::from_secs(1);
pub const STORM_LIMIT: u32 = 5;
pub const STORM_COOLDOWN: Duration = Duration::from_secs(5);

/// The storm guard's count of fast restarts in a row.
#[derive(Default)]
pub struct Storm(u32);

impl Storm {
    /// A change came `since` the last restart: how long to wait before the
    /// rebuild, if this is a storm.
    pub fn wait(&mut self, since: Duration) -> Option<Duration> {
        self.0 = if since < STORM { self.0 + 1 } else { 0 };
        (self.0 >= STORM_LIMIT).then_some(STORM_COOLDOWN)
    }
}

/// The app's `[build] css` step, if any: a failure is said, the app kept.
/// What the step wrote is not a change of its own. Whether it ran.
fn css(pkg: &Pkg, rx: &mpsc::Receiver<Vec<PathBuf>>) -> bool {
    let Ok((_, cfg)) = crate::build::app_of(pkg) else { return false };
    match crate::build::app_dir(pkg).and_then(|dir| crate::css::step(&cfg, &dir)) {
        Ok(Some(did)) => eprintln!("ax dev: {did}"),
        Ok(None) => return false,
        Err(e) => eprintln!("ax dev: {e}"),
    }
    crate::watch::settle(rx, vec![]);
    true
}

/// The app, started so it stops when `ax dev` does, placed by `put`
/// (its display, contained or not).
fn launch(bin: &Path, args: &[String], put: &dyn Fn(&mut Command)) -> Option<(Child, aio::lifecycle::Watch)> {
    #[cfg(target_os = "macos")]
    let _ = read_through(bin);
    let mut cmd = Command::new(bin);
    cmd.args(args);
    put(&mut cmd);
    aio::lifecycle::scrub(&mut cmd, true);
    let watch = aio::lifecycle::watched(&mut cmd).inspect_err(|e| eprintln!("ax dev: {e}")).ok()?;
    let child = cmd.spawn().inspect_err(|e| eprintln!("ax dev: can't start {}: {e}", bin.display())).ok()?;
    Some((child, watch))
}

/// Why the app's run `pid` ended, as it said it (pure): in its log's
/// `text`, that run's last error line (`Error: …` from its `main`, an
/// `ERROR` line, a panic), its stamp cut off.
fn why_ended(text: &str, pid: u32) -> Option<String> {
    let run = text.rfind(&format!(" starting (pid {pid})")).map_or("", |at| &text[at..]);
    run.lines()
        .skip(1)
        .take_while(|l| !l.contains(" starting (pid "))
        .map(|l| if l.as_bytes().get(23) == Some(&b'Z') { l.get(25..).unwrap_or_default() } else { l })
        .filter(|l| l.starts_with("Error:") || l.starts_with("ERROR") || l.contains("panicked"))
        .last()
        .map(str::to_string)
}

/// [`why_ended`] from the app's `logs` (the end of its `app.log`).
fn said_why(logs: Option<&Path>, pid: u32) -> Option<String> {
    use std::io::{Read, Seek};
    let mut f = std::fs::File::open(logs?.join("app.log")).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(std::io::SeekFrom::Start(len.saturating_sub(64 * 1024))).ok()?;
    let mut b = vec![];
    f.read_to_end(&mut b).ok()?;
    why_ended(&String::from_utf8_lossy(&b), pid)
}

/// Read the file `bin` once, start to end (its length). macOS checks a new
/// binary whole at its first start; one the linker just wrote isn't in
/// memory yet, and that check then reads it slowly: read through first, a
/// 60 MB binary's first start takes ~0.65 s instead of ~1.15 s (the read:
/// ~40 ms; `codesign -s -` gets the same for ~170 ms, reusing the path
/// nothing). See docs/build/dev.md.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn read_through(bin: &Path) -> std::io::Result<u64> {
    let f = std::fs::File::open(bin)?;
    std::io::copy(&mut std::io::BufReader::with_capacity(1 << 20, f), &mut std::io::sink())
}

/// The files an `ax dev` session keeps in the app's logs dir: the mark the
/// app's window leaves when the person closes it (the session ends, as
/// aio's `am dev`), and the mark that asks the session for a full rebuild
/// and restart (`ax restart`, a changed `Cargo.toml`).
const CLOSED: &str = "dev-closed";
/// What the app's window leaves when its views panicked (the panic, where,
/// the app's frames): the app ends as a release build's would, and the
/// session says it.
const PANICKED: &str = "dev-panicked";
const RESTART: &str = "dev-restart";
/// The mark that asks the session to end (`ax stop`).
const STOP: &str = "dev-stop";
/// What a detached session (`ax dev --detach`) says.
pub const DETACHED: &str = "dev.out";
/// The session's own pid, while it runs.
const SESSION: &str = "dev.pid";
/// When the session's last build or hot-patch landed (ms).
pub const PATCHED: &str = "dev-patched";

/// Whether an `ax dev` session runs the app whose logs are `logs`.
pub fn session(logs: &Path) -> bool {
    running_pid(&logs.join(SESSION)).is_some()
}

/// A pid file's text: `pid start` (when the OS says when it started).
fn pid_record(pid: u32) -> String {
    aio::lifecycle::started(pid).map_or_else(|| pid.to_string(), |s| format!("{pid} {s}"))
}

/// The pid a pid file's `text` names, while that very process runs: one
/// ended by a signal leaves its file, and the OS hands its pid out again
/// (pure: `started` and `alive` ask the OS).
pub fn recorded(text: &str, started: &dyn Fn(u32) -> Option<String>, alive: &dyn Fn(u32) -> bool) -> Option<u32> {
    let mut w = text.split_whitespace();
    let pid: u32 = w.next()?.parse().ok()?;
    let same = match w.next() {
        Some(s) => started(pid).as_deref() == Some(s),
        None => alive(pid),
    };
    same.then_some(pid)
}

fn running_pid(file: &Path) -> Option<u32> {
    recorded(&std::fs::read_to_string(file).ok()?, &aio::lifecycle::started, &aio::lifecycle::alive)
}

/// How long `ax stop` gives a session to end once asked, a build included.
const SESSION_WAIT: Duration = Duration::from_secs(20);

/// End the `ax dev` session of `logs`, if one runs (aio: `am stop` ends the
/// dev session; it would start the app again on the next save): asked, so
/// its app stops the graceful way; still there after
/// [`SESSION_WAIT`], ended. True when there was one.
pub fn end_session(logs: &Path) -> bool {
    let file = logs.join(SESSION);
    let Some(pid) = running_pid(&file).filter(|p| *p != std::process::id()) else { return false };
    _ = std::fs::write(logs.join(STOP), b"");
    let end = std::time::Instant::now() + SESSION_WAIT;
    while running_pid(&file) == Some(pid) && std::time::Instant::now() < end {
        std::thread::sleep(Duration::from_millis(100));
    }
    if running_pid(&file) == Some(pid) {
        _ = aio::lifecycle::terminate(pid, true);
        aio::lifecycle::wait_gone(pid, Duration::from_secs(5));
    }
    _ = std::fs::remove_file(logs.join(STOP));
    true
}

/// How long a restart gives the app to stop once asked.
const QUIT_WAIT: Duration = Duration::from_secs(15);

/// Ask the app `window` names to stop the graceful way, over its door (any
/// OS: its calls in flight finish, its effects commit), and wait until it
/// is gone, `budget` at most: what ax does before it restarts it.
fn quit_app(window: Option<&(String, aio::paths::Plan)>, budget: Duration) {
    let Some((id, p)) = window else { return };
    if !aio::paths::running(&p.data) || aio_shell::instance::quit_at(id, &p.data).is_err() {
        return;
    }
    let end = std::time::Instant::now() + budget;
    while aio::paths::running(&p.data) && std::time::Instant::now() < end {
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The session's pid file, removed when dropped.
struct Session(PathBuf);

impl Session {
    fn open(logs: &Path) -> Option<Self> {
        _ = std::fs::remove_file(logs.join(RESTART));
        _ = std::fs::remove_file(logs.join(STOP));
        _ = std::fs::remove_file(logs.join(PATCHED));
        let file = logs.join(SESSION);
        aio::paths::private_dir(logs).and_then(|()| std::fs::write(&file, pid_record(std::process::id()))).ok()?;
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

/// Stop the app the graceful way (asked over its door, then its watch let
/// go and signalled) and wait for it.
fn stop(app: &mut Option<(Child, aio::lifecycle::Watch)>, window: Option<&(String, aio::paths::Plan)>) {
    if let Some((mut c, watch)) = app.take() {
        if c.try_wait().ok().flatten().is_none() {
            quit_app(window, QUIT_WAIT);
        }
        // A Windows child stops once its watch is let go (no SIGTERM there).
        drop(watch);
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
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(format!("{}: {e}", home.display())),
        _ => {}
    }
    // `ax stop` throws this run's data away too, once it is gone.
    aio::paths::private_dir(home)
        .and_then(|_| std::fs::write(home.join(FRESH), ""))
        .map_err(|e| format!("{}: {e}", home.display()))
}

/// In a `--fresh` instance's home: its data goes when it stops.
pub const FRESH: &str = "fresh";

/// After `ax stop`: the stopped `--fresh` instance `plan`'s data thrown
/// away (aio: `am stop --instance` discards it); `None` when it is no such run.
pub fn discard_fresh(plan: &aio::paths::Plan) -> Option<Result<(), String>> {
    let home = plan.data.parent()?;
    (plan.from == "--instance" && home.join(FRESH).exists()).then(|| {
        std::fs::remove_dir_all(home)
            .or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) })
            .map_err(|e| format!("{}: {e}", home.display()))
    })
}

/// `ax dev <app> --android [--device serial] [--native]` (aio's
/// `dev:android`): on the attached emulator (before a phone) or phone, else
/// an emulator booted (`AIO_AVD`). An app with a web build runs as its dev
/// page: a thin dev APK, installed once beside the real app, shows what
/// `ax dev --web` serves over `adb reverse`, and each edit shows there as
/// in a browser tab. Else (or `--native`) the app itself, as a debug dev copy beside
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
        // One a session left (its end killed): dropped now.
        let reversed = target.join("aio").join("android-dev").join(format!("{}-{id}.reverse", dev.serial));
        crate::install::unreverse(&dev.serial, &reversed);
        let mut logs = crate::install::logcat(&dev.serial)?;
        let on = dev.clone();
        let record = reversed.clone();
        let ran = crate::web::dev_then(name, crate::web::Serve::default(), move |port| {
            crate::install::reverse(&on.serial, &format!("tcp:{port}"), &record)
                .map_err(|e| format!("{e}: the app can't reach the dev server"))?;
            let url = format!("http://127.0.0.1:{port}/");
            match crate::install::start_on(&on, &format!("{id}/{}", crate::android::DEV), &["--es", "url", &url]) {
                Ok(()) => eprintln!("ax dev: on {} → {url}: edits show there; Ctrl-C ends it", on.label()),
                Err(why) => eprintln!("ax dev: installed, but did not start ({why}): open \"(dev)\" from the launcher"),
            }
            Ok(())
        });
        _ = logs.kill();
        _ = logs.wait();
        crate::install::unreverse(&dev.serial, &reversed);
        return ran;
    }
    let (_w, rx) = crate::watch::Watch::new(&watched(&pkg, Path::new(""), &Watching::All))?;
    let cwd = std::env::current_dir().unwrap_or_default();
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
        crate::watch::settle(&rx, vec![]);
        let Ok(first) = rx.recv() else {
            _ = logs.kill();
            _ = logs.wait();
            return Ok(String::new());
        };
        let changed = crate::watch::settle(&rx, first);
        eprintln!("ax dev: {} changed — building for Android…", crate::watch::describe(&changed, &cwd));
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
    // ax's newer flags only before a bare `--`: after it, the app's own words.
    let end = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let mine = &args[..end];
    if let Some(i) = mine.iter().position(|a| a == "--detach") {
        let rest: Vec<String> = args.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, a)| a.clone()).collect();
        return detach(name, &rest, plan.as_ref());
    }
    // `--warm`: the build `ax dev` starts from, and no more (ax create runs
    // it in the background, so a new app's first start is a launch).
    let workspace = PathBuf::from(meta["workspace_root"].as_str().unwrap_or_default());
    let target = PathBuf::from(meta["target_directory"].as_str().unwrap_or_default());
    let app_dir = pkg.dirs.first().cloned().unwrap_or_default();
    let feature = pkg.features_with(true).unwrap_or_default();
    if mine.iter().any(|a| a == "--warm") {
        // `--pack <dir>`: then the prebuilt libraries of this build into it.
        let pack = mine.iter().position(|a| a == "--pack").map(|i| mine.get(i + 1).cloned());
        let pack = match pack {
            Some(None) => return Err("--pack needs a folder: ax dev <app> --warm --pack <dir>".into()),
            Some(Some(d)) => Some(PathBuf::from(d)),
            None => None,
        };
        // Packed from a build of its own (no other app's units, nothing of this machine's).
        if let Some(out) = pack {
            return crate::prebuilt::pack(&pkg.name, &app_dir, &workspace, &feature, &out);
        }
        if let Some(said) = crate::prebuilt::fetch(&app_dir, &workspace, &target, &feature) {
            eprintln!("{said}");
        }
        return warm(&pkg, name, plan.as_ref().map(|p| p.logs.as_path()));
    }
    let fresh = args.iter().any(|a| a == "--fresh");
    // aio's `--verbose`: the app's log at debug, its boot report says so.
    let verbose = mine.iter().any(|a| a == "--verbose");
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
    // aio's `--title=X`: the window's title for this run.
    let ti = mine.iter().position(|a| a == "--title");
    let title = match ti {
        Some(i) => Some(args.get(i + 1).cloned().ok_or("--title needs a value: --title \"Notes (QA)\"")?),
        None => mine.iter().find_map(|a| a.strip_prefix("--title=").map(String::from)),
    };
    let ours = |i: usize| [at, sz, ti].iter().flatten().any(|f| i == *f || i == f + 1);
    let args: &[String] = &args
        .iter()
        .enumerate()
        .filter(|(_, a)| *a != "--fresh")
        .filter(|(i, a)| *i >= end || (*a != "--verbose" && !a.starts_with("--title=")))
        .filter(|(i, _)| !ours(*i))
        .map(|(_, a)| a.clone())
        .collect::<Vec<_>>();
    let (args, cdp) = crate::cdp::take_flag(args)?;
    let (args, watching) = take_watch(&args)?;
    let watching = or_config(watching, crate::build::app_of(&pkg).map(|(_, c)| c.dev.watch).unwrap_or_default());
    let args: &[String] = &args;
    let paths = watched(&pkg, &crate::build::app_dir(&pkg).unwrap_or_default(), &watching);
    if let Some(gone) = paths.iter().find(|p| matches!(watching, Watching::Only(_)) && !p.exists()) {
        return Err(format!("--watch: {} isn't there", gone.display()));
    }
    if fresh {
        throw_away(plan.as_ref())?;
    }
    // One port for the whole run (restarts too), so `ax shot` finds it.
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
    let closed = plan.as_ref().map(|p| p.logs.join(CLOSED));
    if let Some(c) = &closed {
        _ = std::fs::remove_file(c);
        _ = std::fs::remove_file(c.with_file_name(PANICKED));
    }
    let put = |cmd: &mut Command| {
        // A dev build's window says it was closed by the person here: the session ends.
        if let Some(c) = &closed {
            cmd.env("AIORT_DEV_CLOSED", c);
        }
        if let Some(p) = cdp {
            cmd.env("AIORT_CDP", p.to_string());
        }
        if let Some((w, h)) = size {
            cmd.env("AIORT_WINDOW_SIZE", format!("{w}x{h}"));
        }
        // The boot report in one line on this terminal (all of it in the
        // log), and Chromium's harmless chatter kept out, unless --verbose.
        match verbose {
            true => cmd.env("AIORT_LOG", "debug"),
            false => cmd.env("AIORT_BOOT", "brief"),
        };
        if let Some(t) = &title {
            cmd.env("AIORT_TITLE", t);
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
        let (_w, rx) = crate::watch::Watch::new(&[])?;
        _ = css(&pkg, &rx);
        let bin = build(&pkg, logs.as_deref(), true).ok_or(format!("{name}: the build failed"))?;
        let mut app = launch(&bin, args, &put).ok_or(format!("{name}: it didn't start"))?;
        let ended = app.0.wait().map_err(|e| e.to_string())?;
        let why = said_why(logs.as_deref(), app.0.id()).map(|w| format!(": {w}")).unwrap_or_default();
        return Ok(if ended.success() { String::new() } else { format!("{name} ended: {ended}{why}") });
    }
    let cwd = std::env::current_dir().unwrap_or_default();
    match &watching {
        Watching::Only(_) => eprintln!(
            "ax dev: {name}: a stylesheet or an rsx! markup edit in what --watch names shows at once, a Rust \
             edit is hot-patched, state kept; a Cargo.toml edit rebuilds"
        ),
        _ => eprintln!(
            "ax dev: {name}: a stylesheet or an rsx! markup edit shows at once, a Rust edit is hot-patched, \
             state kept; a Cargo.toml edit rebuilds"
        ),
    }
    let (w, rx) = crate::watch::Watch::new(&paths)?;
    let saves = Stop::default();
    let rx = stale_builds(rx, saves.clone());
    let shown: Vec<String> = match &watching {
        Watching::Only(_) => paths.iter().map(|p| p.display().to_string()).collect(),
        _ => pkg.dirs.iter().map(|d| d.display().to_string()).collect(),
    };
    eprintln!("ax dev: {name} — watching {}", shown.join(", "));
    _ = css(&pkg, &rx);
    // What the running app was built from: markup edits are told against it.
    // Rust is hot-patched by ax itself: a fat build, then patches.
    let (mut fat, mut patching) = (None, true);
    // Sources changed while ax dev was off (a pull, an edit): the last fat
    // build starts at once, nothing built or linked, and the change compiled
    // beside its start is patched in as its window comes up.
    // An app the last session stopped may still be ending: the start waits
    // for it as a build would have (else the new one finds its lock held).
    let ended = |(_, p): &(String, aio::paths::Plan)| {
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while aio::paths::running(&p.data) && std::time::Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        !aio::paths::running(&p.data)
    };
    let resumed =
        resume(&pkg, name, &target, &saves, &cwd).filter(|_| logs.is_some() && window.as_ref().is_some_and(ended));
    if resumed.is_none() {
        // A window at once while the first build runs (ax's own: the app is as without it).
        let splash_title = title.clone().unwrap_or_else(|| name.to_string());
        crate::splash::open(&splash_title, &put, &shared_target().join("aio").join("dev-splash"));
        // aio-rs's libraries prebuilt for this toolchain, when a server has them.
        if let Some(said) = crate::prebuilt::fetch(&app_dir, &workspace, &target, &feature) {
            eprintln!("{said}");
        }
    }
    let current = |fat: &mut _, patching: &mut _| {
        until_current(
            // What this build reads: markup edits are told against it.
            || Some(crate::reload::Built::snapshot(&pkg.dirs)),
            || dev_build(&pkg, name, logs.as_deref(), fat, patching, &saves),
            || {
                let more = crate::watch::settle(&rx, vec![]);
                eprintln!("ax dev: {} saved while it built: building again…", crate::watch::describe(&more, &cwd));
            },
        )
    };
    let (mut built, bin, mut resumed) = match resumed {
        Some(r) => {
            let (mut fat, files) = (r.fat, r.changed.clone());
            let compile = std::thread::spawn(move || {
                let c = crate::hotpatch::compile(&mut fat, &files);
                (fat, c)
            });
            (Some(r.built), Some(r.exe), Some((compile, r.now, r.changed)))
        }
        None => {
            let (built, bin) = current(&mut fat, &mut patching);
            (built, bin, None)
        }
    };
    // A failed build's errors are on the page (once it runs) until a good one.
    let mut red = bin.is_none();
    match (&bin, &logs) {
        (Some(_), _) => crate::splash::built(),
        (None, Some(l)) => crate::splash::failed(&std::fs::read_to_string(l.join(BUILD_LOG)).unwrap_or_default()),
        (None, None) => crate::splash::failed(""),
    }
    // The first build failed: the window opens anyway (as `am dev`'s does),
    // on the last good build if there is one, the errors on its page.
    let last_good = fat_request(&pkg, name, &saves).and_then(|r| crate::hotpatch::last_good(&target, &r));
    let (bin, stale) = match (bin, last_good) {
        (Some(b), _) => (Some(b), false),
        (None, Some(good)) => {
            eprintln!("ax dev: {name} doesn't build; its last good build runs, the errors on its page (save a fix)");
            (Some(good), true)
        }
        (None, None) => {
            eprintln!("ax dev: {name} doesn't build, and no earlier build is here to open: save a fix");
            (None, false)
        }
    };
    if let (Some(b), false) = (&bin, matches!(watching, Watching::Only(_))) {
        w.add_files(deps_of(b));
    }
    let mut running = bin.as_deref().and_then(digest);
    let launched_bin = bin.is_some();
    let mut app = bin.and_then(|bin| launch(&bin, args, &put));
    // The splash goes once the window is up; a start that failed is said on it.
    let splash_off = |app: &Option<(Child, aio::lifecycle::Watch)>| match app {
        Some(_) => {
            let w = window.clone();
            crate::splash::close_when_up(move || w.is_none() || on_page(w.as_ref(), "1").is_ok());
        }
        None => crate::splash::not_started(&format!("{name} didn't start: why is on the terminal")),
    };
    if launched_bin {
        splash_off(&app);
    }
    if let (true, Some(_), Some(w), Some(l)) = (stale, &app, window.clone(), logs.clone()) {
        let errors = crate::web::fatal(&std::fs::read_to_string(l.join(BUILD_LOG)).unwrap_or_default());
        show_once_up(w, show_failed(&errors));
    }
    let (mut launched, mut storm) = (std::time::Instant::now(), Storm::default());
    // The next start's build: behind the patches since, brought up to date
    // once the session is quiet; else `ax dev --warm` when it ends, by a
    // signal too.
    let mut refresh = Refresh::default();
    let warm_log = target.join("aio").join(format!("warm-{name}.log"));
    let on_exit = || crate::hotpatch::cargo_out::OnExit {
        program: std::env::current_exe().unwrap_or_else(|_| "ax".into()),
        args: vec!["dev".into(), name.to_string(), "--warm".into()],
        cwd: cwd.clone(),
        log: warm_log.clone(),
    };
    let asked = |mark: &str| logs.as_ref().is_some_and(|l| std::fs::remove_file(l.join(mark)).is_ok());
    // `ax doctor`: the running app has the edits up to now.
    let landed = || {
        if let Some(l) = &logs {
            _ = std::fs::write(l.join(PATCHED), now_ms().to_string());
        }
    };
    if let Some((compile, now, changed)) = resumed.take() {
        let said = crate::watch::describe(&changed, &cwd);
        let (mut f, compiled) = compile.join().map_err(|_| "ax dev: the patch's compile panicked")?;
        let linked = compiled.and_then(|c| {
            let (_, _, aslr, base) = door_when_up(window.as_ref(), &mut app)?;
            crate::hotpatch::link_patch(&mut f, c, aslr, base)
        });
        match linked.and_then(|p| hot_patch(window.as_ref(), &mut f, &changed, Some((p, "compiled as it started")))) {
            Ok(patched) => {
                eprintln!("ax dev: {said} changed while ax dev was off: its last build started, the change patched in");
                eprintln!("{patched}");
                refresh.patched(on_exit());
                built = Some(now);
                landed();
            }
            Err(crate::hotpatch::Refused::Code(errors)) => {
                eprint!("{}", with_hints(&errors));
                eprintln!("ax dev: {said}: doesn't compile; its last build runs (save a fix)");
                red = true;
                keep_failed(logs.as_deref(), &errors);
                if let Some(w) = window.clone() {
                    show_once_up(w, show_failed(&crate::web::fatal(&errors)));
                }
            }
            Err(e) => {
                eprintln!("ax dev: {said} changed while ax dev was off; not patched ({e}): building");
                if let Some(l) = &logs {
                    _ = ask_restart(l);
                }
            }
        }
        fat = Some(f);
    }
    loop {
        // Wait for a change, then for the burst of saves to settle; or for `ax restart`, `ax stop`.
        let restart = match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(first) => {
                refresh.saved();
                // A save of code stops a refresh behind the session first.
                if first.iter().any(|p| is_code(p))
                    && let Some(said) = refresh.stop(&saves)
                {
                    eprintln!("{said}");
                }
                // Markup alone goes to the window at once, before the
                // burst settles: a file still being written reads as code, and
                // waits for it like any other change.
                let up = app.as_mut().is_some_and(|(c, _)| c.try_wait().ok().flatten().is_none());
                let first = real_changes(first, &|p| built.as_ref().is_some_and(|b| b.has(p)));
                let early = built
                    .as_ref()
                    .filter(|_| up && !red && !first.is_empty())
                    .and_then(|b| b.edits(&first, &workspace))
                    .and_then(|e| reload_markup(window.as_ref(), &e).ok().map(|said| (e, said)));
                // Code: its patch compiles while the burst settles, kept if
                // the burst was just these saves (each file as the compile
                // found it); the quiet a burst needs costs no time.
                let patchable = |c: &[PathBuf]| {
                    up && early.is_none()
                        && !c.is_empty()
                        && !manifest(c)
                        && !sheets_only(c)
                        && built.as_ref().is_some_and(|b| reshaped(b, c, &cwd).is_empty())
                };
                let ahead = match fat.take() {
                    Some(f) if patchable(&first) => Some(Pending::begin(window.as_ref(), f, &first)),
                    f => {
                        fat = f;
                        None
                    }
                };
                let changed = crate::watch::settle(&rx, first);
                let mut ahead = ahead.and_then(|p| p.join(&mut fat));
                // A temporary copy saved and renamed over the real file (`sed -i`,
                // safe-write editors, agents' write tools) is gone by now: no change.
                let changed = real_changes(changed, &|p| built.as_ref().is_some_and(|b| b.has(p)));
                if changed.is_empty() {
                    continue;
                }
                let said = crate::watch::describe(&changed, &cwd);
                if let Some(wait) = storm.wait(launched.elapsed()) {
                    eprintln!(
                        "ax dev: {said} changed right after each of the last {STORM_LIMIT} restarts (a formatter, \
                         a generator, another watcher?): each rebuild waits {} s",
                        wait.as_secs()
                    );
                    std::thread::sleep(wait);
                    crate::watch::settle(&rx, vec![]);
                }
                // aio's lint is said beside the build, never before it.
                let (lint, at) = (changed.clone(), cwd.clone());
                std::thread::spawn(move || lint_said(&lint, &at));
                if css(&pkg, &rx) {
                    ahead = None;
                }
                let up = app.as_mut().is_some_and(|(c, _)| c.try_wait().ok().flatten().is_none());
                // Stylesheets alone: `use_css!` reads them live, nothing to build.
                if up && sheets_only(&changed) {
                    eprintln!("ax dev: {said} changed");
                    eprintln!("ax dev: the window reads it live: nothing to build");
                    continue;
                }
                // Markup alone: to the running window, no build.
                let edits = built.as_ref().filter(|_| up && !red).and_then(|b| b.edits(&changed, &workspace));
                if let Some(edits) = edits {
                    eprintln!("ax dev: {said} changed");
                    if let Some((_, said)) = early.as_ref().filter(|(e, _)| *e == edits) {
                        eprintln!("{said}");
                        landed();
                        continue;
                    }
                    match reload_markup(window.as_ref(), &edits) {
                        Ok(said) => {
                            eprintln!("{said}");
                            landed();
                            continue;
                        }
                        Err(e) => eprintln!("ax dev: the markup didn't reach the window ({e}): building"),
                    }
                }
                // A type, a static, a signature or a macro call changed: the running
                // process holds values of the old shape, so no patch can take it.
                let reshaped = built.as_ref().map(|b| reshaped(b, &changed, &cwd)).unwrap_or_default();
                if !reshaped.is_empty() && fat.is_some() && up {
                    eprintln!(
                        "ax dev: {} changed shape: a running app can't take that as a patch; rebuilding and \
                         restarting (its cells come back from their log)",
                        reshaped.join(", ")
                    );
                }
                // Code: patched into the running app, its state kept. A patch
                // that didn't compile left the fat build as it was: the next
                // save patches again (no rebuild, no restart).
                if let Some(f) = fat.as_mut().filter(|_| up && reshaped.is_empty() && !manifest(&changed)) {
                    eprintln!("ax dev: {said} changed");
                    let snapshot = crate::reload::Built::snapshot(&pkg.dirs);
                    let ready = ahead.take().and_then(|a| a.take(&changed)).map(|p| (p, "begun with the first save"));
                    match hot_patch(window.as_ref(), f, &changed, ready) {
                        Ok(said) => {
                            eprintln!("{said}");
                            refresh.patched(on_exit());
                            if std::mem::take(&mut red) {
                                eprintln!("ax dev: fixed: the code compiles again");
                                _ = on_page(window.as_ref(), SHOWN_FIXED);
                                if let Some(l) = &logs {
                                    _ = std::fs::remove_file(l.join(BUILD_LOG));
                                }
                            }
                            built = Some(snapshot);
                            landed();
                            continue;
                        }
                        Err(crate::hotpatch::Refused::Code(errors)) => {
                            eprint!("{}", with_hints(&errors));
                            eprintln!("ax dev: {said}: doesn't compile; the app runs on as it was (save a fix)");
                            red = true;
                            keep_failed(logs.as_deref(), &errors);
                            if let Err(e) = on_page(window.as_ref(), &show_failed(&crate::web::fatal(&errors))) {
                                eprintln!("ax dev: the errors aren't on its page: {e}");
                            }
                            continue;
                        }
                        Err(crate::hotpatch::Refused::Unchanged) => {
                            eprintln!("ax dev: nothing the build reads changed: nothing to do");
                            continue;
                        }
                        Err(e) => eprintln!("ax dev: not hot-patched ({e}): building"),
                    }
                }
                eprintln!("ax dev: {said} changed — building…");
                false
            }
            Err(mpsc::RecvTimeoutError::Timeout) if asked(CLOSED) => {
                eprintln!("ax dev: the window was closed; the session ends");
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) if asked(STOP) => {
                eprintln!("ax dev: asked to stop; the session ends");
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) if asked(RESTART) => {
                eprintln!("ax dev: restart asked — building…");
                if let Some(said) = refresh.stop(&saves) {
                    eprintln!("{said}");
                }
                true
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(said) = refresh.poll() {
                    eprintln!("{said}");
                }
                if fat.is_some()
                    && refresh.due(std::time::Instant::now())
                    && let Some(mut req) = fat_request(&pkg, name, &saves)
                {
                    req.low = true;
                    refresh.start(req);
                }
                // The app ended on its own (a panic, an exit of its own): said
                // once; the session waits for a save or `ax restart`.
                if let Some((status, pid)) =
                    app.as_mut().and_then(|(c, _)| Some((c.try_wait().ok().flatten()?, c.id())))
                {
                    app = None;
                    let panicked = closed.as_ref().map(|c| c.with_file_name(PANICKED));
                    if let Some(said) = panicked.as_ref().and_then(|p| std::fs::read_to_string(p).ok()) {
                        _ = std::fs::remove_file(panicked.as_ref().unwrap());
                        eprintln!("ax dev: {name}'s window panicked (a release build ends the same way):\n{said}");
                    }
                    // Its own reason, said again last (its stderr scrolls by).
                    let why = said_why(logs.as_deref(), pid).map(|w| format!(": {w}")).unwrap_or_default();
                    eprintln!("ax dev: {name} ended ({status}){why}; waiting: save a fix, or ax restart {name}");
                    crate::splash::not_started(&format!("{name} ended ({status}) before its window opened{why}"));
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        if restart {
            _ = css(&pkg, &rx);
        }
        let started = std::time::Instant::now();
        let (snapshot, bin) = current(&mut fat, &mut patching);
        let Some(bin) = bin else {
            red = true;
            eprintln!("ax dev: build failed; the running app is unchanged and shows why");
            if let (Some(w), Some(l)) = (&window, &logs) {
                let errors = crate::web::fatal(&std::fs::read_to_string(l.join(BUILD_LOG)).unwrap_or_default());
                if let Err(e) = on_page(Some(w), &show_failed(&errors)) {
                    eprintln!("ax dev: the errors aren't on its page: {e}");
                }
            }
            continue;
        };
        let took = started.elapsed().as_secs_f64();
        if !matches!(watching, Watching::Only(_)) {
            w.add_files(deps_of(&bin));
        }
        // Red to green: said, and off the page, whether or not it restarts.
        if std::mem::take(&mut red) {
            eprintln!("ax dev: fixed: the build is good again");
            _ = on_page(window.as_ref(), SHOWN_FIXED);
        }
        let now = digest(&bin);
        let up = app.as_mut().is_some_and(|(c, _)| c.try_wait().ok().flatten().is_none());
        if !restart && up && now.is_some() && now == running {
            eprintln!("ax dev: built in {took:.1} s, the same binary: {name} keeps running");
            continue;
        }
        stop(&mut app, window.as_ref());
        refresh.current();
        eprintln!("ax dev: built in {took:.1} s; restarting {name}");
        running = now;
        built = snapshot;
        app = launch(&bin, args, &put);
        splash_off(&app);
        launched = std::time::Instant::now();
        landed();
    }
    crate::splash::close();
    stop(&mut app, window.as_ref());
    if let Some(said) = refresh.stop(&saves) {
        eprintln!("{said}");
    }
    crate::hotpatch::cargo_out::disarm();
    if refresh.behind {
        match warm_in_background(&cwd, name, &warm_log) {
            Ok(()) => eprintln!("ax dev: {name}'s next start is built in the background ({})", warm_log.display()),
            Err(e) => eprintln!("ax dev: {e}"),
        }
    }
    Ok(String::new())
}

/// How long a session is quiet before the next start's build is brought
/// up to date behind it.
const REFRESH_QUIET: Duration = Duration::from_secs(2);

/// The next start's fat build, behind the patches of a session
/// ([`crate::hotpatch::refresh`]).
#[derive(Default)]
struct Refresh {
    /// Patched since the last fat build.
    behind: bool,
    /// The last refresh failed: none again until the next patch.
    failed: bool,
    /// The last save or patch.
    quiet_since: Option<std::time::Instant>,
    running: Option<std::thread::JoinHandle<Result<bool, crate::hotpatch::Refused>>>,
}

impl Refresh {
    /// A patch landed: behind, and a signal that ends the session starts `warm`.
    fn patched(&mut self, warm: crate::hotpatch::cargo_out::OnExit) {
        (self.behind, self.failed, self.quiet_since) = (true, false, Some(std::time::Instant::now()));
        crate::hotpatch::cargo_out::arm(warm);
    }

    /// A fat build ran: the next start is it.
    fn current(&mut self) {
        self.behind = false;
        crate::hotpatch::cargo_out::disarm();
    }

    fn saved(&mut self) {
        self.quiet_since = Some(std::time::Instant::now());
    }

    /// Whether to start one at `now` (pure): behind, quiet long enough,
    /// none running, and the last one not failed.
    fn due(&self, now: std::time::Instant) -> bool {
        self.behind
            && !self.failed
            && self.running.is_none()
            && self.quiet_since.is_some_and(|t| now.duration_since(t) >= REFRESH_QUIET)
    }

    fn start(&mut self, req: crate::hotpatch::Request) {
        self.running = Some(std::thread::spawn(move || crate::hotpatch::refresh(&req)));
    }

    /// The one running, if it ended: what to say.
    fn poll(&mut self) -> Option<String> {
        match self.running.as_ref().is_some_and(|h| h.is_finished()) {
            true => self.stop(&Stop::default()),
            false => None,
        }
    }

    /// The one running stopped (`saves` marks its cargo stale) and ended:
    /// what to say.
    fn stop(&mut self, saves: &Stop) -> Option<String> {
        let running = self.running.take()?;
        saves.stale();
        match running.join() {
            Ok(Ok(built)) => {
                self.current();
                built.then(|| "ax dev: the next start's build is up to date (built behind the session)".into())
            }
            Ok(Err(crate::hotpatch::Refused::Stale)) => None,
            Ok(Err(e)) => {
                self.failed = true;
                Some(format!("ax dev: the next start's build stays behind ({e}); it builds then"))
            }
            Err(_) => {
                self.failed = true;
                None
            }
        }
    }
}

/// `build` until a newer save no longer stops it (`stale` said each time):
/// the sources the build that ended read (`snapshot`, taken before each
/// try: a save that stopped one is in the next's) and its binary (`None`:
/// it failed).
fn until_current<S>(
    snapshot: impl Fn() -> S,
    mut build: impl FnMut() -> Built,
    mut stale: impl FnMut(),
) -> (S, Option<PathBuf>) {
    loop {
        let read = snapshot();
        match build() {
            Built::Stale => stale(),
            Built::Bin(b) => return (read, Some(b)),
            Built::Failed => return (read, None),
        }
    }
}

/// Build binary `name` of `pkg` for `ax dev`: fat (every crate's rustc
/// call recorded, all its code linked in: [`crate::hotpatch`]) while
/// `patching`, with aio-shell's `dev` (the app takes patches), else as
/// [`build`] does. A fat build that fails where a plain one doesn't (a
/// platform ax can't patch on yet) turns `patching` off for the session.
fn dev_build(
    pkg: &Pkg,
    name: &str,
    logs: Option<&Path>,
    fat: &mut Option<crate::hotpatch::Fat>,
    patching: &mut bool,
    stop: &Stop,
) -> Built {
    *fat = None;
    let Some(req) = fat_request(pkg, name, stop).filter(|_| *patching) else {
        *patching = false;
        return build_or_stale(pkg, logs, true, stop);
    };
    match crate::hotpatch::fat_build(&req) {
        Ok(f) => {
            if let Some(dir) = logs {
                _ = std::fs::remove_file(dir.join(BUILD_LOG));
            }
            let exe = f.exe.clone();
            let (b, l) = (f.build.as_secs_f64(), f.link.as_secs_f64() * 1000.0);
            match f.kept {
                true => eprintln!("ax dev: built for hot-patching in {b:.1} s (nothing changed: the last fat binary)"),
                false => eprintln!("ax dev: built for hot-patching in {b:.1} s (its link {l:.0} ms)"),
            }
            *fat = Some(f);
            Built::Bin(exe)
        }
        Err(crate::hotpatch::Refused::Stale) => Built::Stale,
        // The code doesn't compile: said once, as cargo said it. Another build
        // (without hot-patching) would only fail the same way, slower.
        Err(crate::hotpatch::Refused::Code(errors)) => {
            if let Some(h) = hints(&errors) {
                eprintln!("{h}");
            }
            keep_failed(logs, &with_hints(&errors));
            Built::Failed
        }
        Err(e) => {
            let built = build_or_stale(pkg, logs, true, stop);
            if matches!(built, Built::Bin(_)) {
                *patching = false;
                eprintln!("ax dev: no hot-patching ({e}): code edits rebuild and restart");
            }
            built
        }
    }
}

/// The fat build of binary `name` of `pkg`, as `ax dev` asks it (`None`:
/// the app has no window shell, nothing to patch).
fn fat_request(pkg: &Pkg, name: &str, stop: &Stop) -> Option<crate::hotpatch::Request> {
    let mut cargo_args = layout();
    pkg.dev_feature.as_ref()?;
    cargo_args.extend(["--features".into(), pkg.features_with(true)?]);
    let stamp = pkg.dirs.first().map_or_else(String::new, |d| crate::build::assets_stamp(d));
    Some(crate::hotpatch::Request {
        dir: pkg.dirs.first().cloned().unwrap_or_default(),
        package: pkg.name.clone(),
        bin: name.into(),
        cargo_args,
        envs: vec![("AIORT_ASSETS".into(), stamp)],
        ax: std::env::current_exe().unwrap_or_else(|_| "ax".into()),
        stop: stop.clone(),
        low: false,
        sources: pkg.dirs.clone(),
    })
}

/// `ax dev <app> --warm`: the fat build, so the next `ax dev` finds it done.
fn warm(pkg: &Pkg, name: &str, logs: Option<&Path>) -> Result<String, String> {
    let started = std::time::Instant::now();
    let (mut fat, mut patching) = (None, true);
    match dev_build(pkg, name, logs, &mut fat, &mut patching, &Stop::default()) {
        Built::Bin(_) => Ok(format!(
            "{name}: built for ax dev in {:.1} s: its next start is a launch",
            started.elapsed().as_secs_f64()
        )),
        Built::Failed => Err(format!("{name}: the build failed (ax errors {name})")),
        Built::Stale => Ok(format!("{name}: stopped (a newer save)")),
    }
}

/// Whether `ax create` warms the new app (`ax setup`, aio-rs) in the
/// background: at a terminal (a person, who runs `ax dev` next), unless
/// asked not to (pure).
pub fn warms_after_create(terminal: bool, no_warm: bool, env: Option<&str>) -> bool {
    terminal && !no_warm && !env_on(env)
}

/// An `AIORT_NO_*` switch's value is on (pure): set, and neither empty nor `0`.
pub fn env_on(v: Option<&str>) -> bool {
    v.is_some_and(|v| !v.is_empty() && v != "0")
}

/// `ax dev <name> --warm` in `dir`, in the background (its own process
/// group, on Windows with no console: a Ctrl-C at the terminal, or the
/// terminal closing, leaves it), its words in `log`.
pub fn warm_in_background(dir: &Path, name: &str, log: &Path) -> Result<(), String> {
    let args = ["dev", name, "--warm"];
    if let Some(d) = log.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let out = std::fs::File::create(log).map_err(|e| format!("{}: {e}", log.display()))?;
    let err = out.try_clone().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    cmd.args(args).current_dir(dir).stdin(std::process::Stdio::null()).stdout(out).stderr(err);
    detached(&mut cmd);
    cmd.spawn().map(|_| ()).map_err(|e| format!("can't start ax {}: {e}", args.join(" ")))
}

/// `cmd` apart from this terminal: its own process group (Unix); on
/// Windows its own group and no console (DETACHED_PROCESS), so neither a
/// Ctrl-C nor the console closing reaches it.
fn detached(cmd: &mut Command) {
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(cmd, 0);
    // CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS.
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(cmd, 0x200 | 0x8);
}

/// The watcher's saves, passed on; a save of code (a `.rs` file or a
/// manifest still there) while cargo builds makes that build stale (`stop`).
fn stale_builds(rx: mpsc::Receiver<Vec<PathBuf>>, stop: Stop) -> mpsc::Receiver<Vec<PathBuf>> {
    let (tx, out) = mpsc::channel();
    std::thread::spawn(move || {
        for saved in rx {
            if saved.iter().any(|p| is_code(p)) {
                stop.stale();
            }
            if tx.send(saved).is_err() {
                break;
            }
        }
    });
    out
}

/// A save a build reads (pure but for `exists`: a temporary copy renamed
/// away is none).
fn is_code(p: &Path) -> bool {
    let named = p.extension().is_some_and(|x| x == "rs") || p.file_name().is_some_and(|n| n == "Cargo.toml");
    named && p.exists()
}

/// Run `js` on the app's page as soon as it is up (a window just launched),
/// trying for half a minute.
fn show_once_up(window: (String, aio::paths::Plan), js: String) {
    std::thread::spawn(move || {
        let until = std::time::Instant::now() + Duration::from_secs(30);
        while std::time::Instant::now() < until {
            // Before its page has the overlay, nothing is shown: again.
            let asked = format!("window.__aioDiag ? ({js}, 'shown') : 'later'");
            if on_page(Some(&window), &asked).is_ok_and(|said| said.contains("shown")) {
                return;
            }
            std::thread::sleep(Duration::from_millis(300));
        }
    });
}

/// A failed build's or patch's `errors` kept in `logs/build.log`, for the
/// page and `ax errors`.
fn keep_failed(logs: Option<&Path>, errors: &str) {
    let Some(dir) = logs else { return };
    let file = dir.join(BUILD_LOG);
    let kept = aio::paths::private_dir(dir)
        .and_then(|()| aio::paths::write_atomic(&file, build_log(now_ms(), errors).as_bytes(), false));
    if let Err(e) = kept {
        eprintln!("ax dev: {}: {e}", file.display());
    }
}

/// The items of the changed `.rs` files whose shape differs from what the
/// running app was built from ([`crate::shape`]), each with its file.
fn reshaped(built: &crate::reload::Built, changed: &[PathBuf], cwd: &Path) -> Vec<String> {
    let mut out = vec![];
    for f in changed.iter().filter(|f| f.extension().is_some_and(|x| x == "rs")) {
        let (Some(before), Ok(now)) = (built.text(f), std::fs::read_to_string(f)) else { continue };
        let file = f.strip_prefix(cwd).unwrap_or(f).display().to_string();
        let items = crate::shape::changed(before, &now).unwrap_or_default();
        out.extend(items.into_iter().map(|i| format!("{i} ({file})")));
    }
    out
}

/// [`resume`]'s start: the last fat build, the change compiling beside it.
struct Resumed {
    exe: PathBuf,
    /// The sources the binary was built from.
    built: crate::reload::Built,
    /// The sources now, and the changed ones among them.
    now: crate::reload::Built,
    changed: Vec<PathBuf>,
    fat: crate::hotpatch::Fat,
}

/// A start whose sources changed since the last fat build of `name` only
/// in what a patch takes (`.rs` files of its crates, no type's shape):
/// that build, kept as it was, to patch the change into. `None`:
/// nothing changed (cargo finds the build current), or a build is needed
/// (a manifest, the lock, `aio.toml`, a file outside the crates, a shape).
fn resume(pkg: &Pkg, name: &str, target: &Path, stop: &Stop, cwd: &Path) -> Option<Resumed> {
    let (fat, read) = crate::hotpatch::last(target, &fat_request(pkg, name, stop)?)?;
    let at = std::fs::metadata(&fat.exe).and_then(|m| m.modified()).ok()?;
    let now = crate::hotpatch::sources_of(&pkg.dirs);
    let mut changed: Vec<PathBuf> =
        now.iter().filter(|(f, t)| read.get(*f) != Some(*t)).map(|(f, _)| f.clone()).collect();
    changed.extend(read.keys().filter(|f| !now.contains_key(*f)).cloned());
    if changed.is_empty() {
        return None;
    }
    // Anything else the build read, saved since: a build's.
    let root = workspace_root(pkg.dirs.first()?);
    let manifests =
        pkg.dirs.iter().map(|d| d.join("Cargo.toml")).chain([root.join("Cargo.lock"), root.join("Cargo.toml")]);
    let newer = |f: &PathBuf| std::fs::metadata(f).and_then(|m| m.modified()).is_ok_and(|t| t > at);
    if deps_of(&fat.exe).into_iter().chain(manifests).any(|f| !now.contains_key(&f) && newer(&f)) {
        return None;
    }
    let built = crate::reload::Built::of(read, at);
    if !reshaped(&built, &changed, cwd).is_empty() {
        return None;
    }
    let now = crate::reload::Built::of(now, std::time::SystemTime::now());
    Some(Resumed { exe: fat.exe.clone(), built, now, changed, fat })
}

/// [`patch_door`] once the app just launched has opened it (while it runs).
fn door_when_up(
    window: Option<&(String, aio::paths::Plan)>,
    app: &mut Option<(Child, aio::lifecycle::Watch)>,
) -> Result<(tokio::runtime::Runtime, crate::cdp::Session, u64, Option<u64>), crate::hotpatch::Refused> {
    let until = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        match patch_door(window) {
            Ok(d) => return Ok(d),
            Err(e) => {
                let up = app.as_mut().is_some_and(|(c, _)| c.try_wait().ok().flatten().is_none());
                if !up || std::time::Instant::now() > until {
                    return Err(e);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The running app's window: a session on its door, and where its binary
/// is loaded (`aio_hotpatch::aslr_reference`; Windows: the patch's room).
fn patch_door(
    window: Option<&(String, aio::paths::Plan)>,
) -> Result<(tokio::runtime::Runtime, crate::cdp::Session, u64, Option<u64>), crate::hotpatch::Refused> {
    let (id, p) = window.ok_or("no app to patch".to_string())?;
    let door = crate::cdp::door(id, p).ok_or("it isn't running".to_string())?;
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    let mut s = rt.block_on(crate::cdp::Session::door(&door))?;
    let at = rt.block_on(s.call(aio_shell::inspect::ASLR, serde_json::json!({})))?;
    let aslr = at["aslr"].as_u64().ok_or_else(|| format!("{}: no aslr", aio_shell::inspect::ASLR))?;
    Ok((rt, s, aslr, at["base"].as_u64()))
}

/// The patch of `changed` for the running app, not sent to it.
fn compile_patch(
    window: Option<&(String, aio::paths::Plan)>,
    fat: &mut crate::hotpatch::Fat,
    changed: &[PathBuf],
) -> Result<crate::hotpatch::Patch, crate::hotpatch::Refused> {
    let (_, _, aslr, base) = patch_door(window)?;
    crate::hotpatch::patch(fat, aslr, base, changed)
}

/// What `files` hold now (`None`: gone), each with its path.
pub(crate) fn contents(files: &[PathBuf]) -> Vec<(PathBuf, Option<Vec<u8>>)> {
    files.iter().map(|f| (f.clone(), std::fs::read(f).ok())).collect()
}

/// A patch compiled while a burst of saves settled, from its first saves:
/// the files as they were when it began. Dropped unused, its library goes.
struct Early {
    read: Vec<(PathBuf, Option<Vec<u8>>)>,
    patch: Option<crate::hotpatch::Patch>,
}

/// A patch compiling beside a settling burst: the session's fat build is
/// the compile's until [`Pending::join`].
struct Pending {
    read: Vec<(PathBuf, Option<Vec<u8>>)>,
    compile: std::thread::JoinHandle<(crate::hotpatch::Fat, Option<crate::hotpatch::Patch>)>,
}

impl Pending {
    /// The compile of `changed`, begun now.
    fn begin(window: Option<&(String, aio::paths::Plan)>, mut fat: crate::hotpatch::Fat, changed: &[PathBuf]) -> Self {
        let read = contents(changed);
        let (window, changed) = (window.cloned(), changed.to_vec());
        let compile = std::thread::spawn(move || {
            let p = compile_patch(window.as_ref(), &mut fat, &changed).ok();
            (fat, p)
        });
        Pending { read, compile }
    }

    /// Its end: the fat build back in `fat`.
    fn join(self, fat: &mut Option<crate::hotpatch::Fat>) -> Option<Early> {
        let (f, patch) = self.compile.join().ok()?;
        *fat = Some(f);
        Some(Early { read: self.read, patch })
    }
}

impl Early {
    /// The patch for the settled burst `changed`, if it is this one's: the
    /// same files, each as it was when the compile began. Else none (its
    /// library removed): `changed` is compiled afresh.
    fn take(mut self, changed: &[PathBuf]) -> Option<crate::hotpatch::Patch> {
        (contents(changed) == self.read).then(|| self.patch.take()).flatten()
    }
}

impl Drop for Early {
    fn drop(&mut self) {
        if let Some(p) = &self.patch {
            _ = std::fs::remove_file(&p.table.lib);
        }
    }
}

/// Patch the running app's code with `changed` (its window's door: where
/// it is loaded, then the patch), or send it `ready`, compiled already;
/// then what a patch of the edited libraries alone owes (the crates built
/// on them replayed), sent too when their code changed: what to say.
fn hot_patch(
    window: Option<&(String, aio::paths::Plan)>,
    fat: &mut crate::hotpatch::Fat,
    changed: &[PathBuf],
    ready: Option<(crate::hotpatch::Patch, &str)>,
) -> Result<String, crate::hotpatch::Refused> {
    let (rt, mut s, aslr, base) = patch_door(window)?;
    let (p, how) = match ready {
        Some((p, how)) => (p, format!("; {how}")),
        None => (crate::hotpatch::patch(fat, aslr, base, changed)?, String::new()),
    };
    let send = |s: &mut crate::cdp::Session, p: &crate::hotpatch::Patch| -> Result<(), crate::hotpatch::Refused> {
        let table = serde_json::to_value(&p.table).map_err(|e| e.to_string())?;
        rt.block_on(s.call(aio_shell::inspect::PATCH, serde_json::json!({ "table": table })))?;
        Ok(())
    };
    send(&mut s, &p)?;
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let t = p.times;
    let mut said = format!(
        "ax dev: hot-patched in {:.0} ms (rustc {:.0}, stub {:.0}, link {:.0}{how}), state kept",
        ms(t.total),
        ms(t.compile),
        ms(t.stub),
        ms(t.link),
    );
    // Where the next patch loads is the app's to say again (Windows).
    let at = rt.block_on(s.call(aio_shell::inspect::ASLR, serde_json::json!({})))?;
    if let Some(p) = crate::hotpatch::finish(fat, aslr, at["base"].as_u64())? {
        send(&mut s, &p)?;
        said.push_str(&format!(
            "\nax dev: the crates built on it changed with it: patched again in {:.0} ms",
            ms(p.times.total)
        ));
    }
    Ok(said)
}

/// Send markup `edits` to the running app's window (`Aio.hot`): what to say.
fn reload_markup(
    window: Option<&(String, aio::paths::Plan)>,
    edits: &[(String, Vec<crate::reload::Edited>)],
) -> Result<String, String> {
    // A file whose markup is as built again (an edit undone) is sent too:
    // the window drops what an earlier reload gave it.
    let n: usize = edits.iter().map(|(_, e)| e.len()).sum();
    let (id, p) = window.ok_or("no app to reload")?;
    let door = crate::cdp::door(id, p).ok_or("it isn't running")?;
    let t = std::time::Instant::now();
    let windows = crate::reload::send(&door, edits)?;
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    let what = match n {
        0 => "the markup as built again".to_string(),
        n => format!("reloaded {n} template{}", plural(n)),
    };
    Ok(format!("ax dev: {what} in {windows} window{} ({ms:.0} ms), state kept", plural(windows as usize)))
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// aio's lint while editing (`air/dev-air.ts`): the aio checks of the saved
/// `.rs` files (keys, accessible names, hooks…), said in the terminal.
fn lint_said(changed: &[PathBuf], cwd: &Path) {
    for l in crate::lint::changed(changed, cwd) {
        eprintln!("ax dev: lint: {l}");
    }
}

/// `ax dev <app> --detach` (aio's `am start` of a dev run): the session in
/// the background, on the display this terminal would choose, its output in
/// the app's `logs/dev.out`; back once the app is up, or its first build
/// failed (the session waits for a fix). `ax stop` ends it.
fn detach(name: &str, args: &[String], plan: Option<&aio::paths::Plan>) -> Result<String, String> {
    let plan = plan.ok_or(format!("--detach: {name} has no aio.toml to keep the session's output by"))?;
    let logs = plan.logs.as_path();
    if take_watch(args)?.1 == Watching::Off {
        return Err("--detach runs the dev loop: without watching, ax start runs the app".into());
    }
    if session(logs) {
        return Err(format!("{name} already runs under ax dev: ax stop {name} ends it"));
    }
    let out = logs.join(DETACHED);
    let file = aio::paths::private_dir(logs)
        .and_then(|()| std::fs::File::create(&out))
        .map_err(|e| format!("{}: {e}", out.display()))?;
    let err = file.try_clone().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    cmd.args(["dev", name]).args(args).stdin(std::process::Stdio::null()).stdout(file).stderr(err);
    if !args.iter().any(|a| a == "--display") {
        use std::io::IsTerminal;
        let asked = std::env::var("AIORT_DISPLAY").ok();
        let choice =
            crate::display::choose(asked.as_deref(), std::io::stdin().is_terminal(), crate::display::desktop())?;
        let word = match choice {
            crate::display::Choice::Current => "current".to_string(),
            crate::display::Choice::Nested => "isolated".to_string(),
            crate::display::Choice::Named(d) => d,
        };
        cmd.args(["--display", &word]);
    }
    // Out of this terminal's process group: its Ctrl-C is not the session's.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x8;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    // A failed build of an earlier session is not this one's.
    _ = std::fs::remove_file(logs.join(BUILD_LOG));
    let mut child = cmd.spawn().map_err(|e| format!("can't start ax dev: {e}"))?;
    // A first build may take minutes.
    let end = std::time::Instant::now() + Duration::from_secs(600);
    let up = || session(logs) && (aio::paths::running(&plan.data) || logs.join(BUILD_LOG).exists());
    while !up() {
        if let Ok(Some(st)) = child.try_wait() {
            let said = std::fs::read_to_string(&out).unwrap_or_default();
            let tail: Vec<&str> = said.lines().rev().take(20).collect();
            return Err(format!("ax dev ended ({st}):\n{}", tail.into_iter().rev().collect::<Vec<_>>().join("\n")));
        }
        if std::time::Instant::now() > end {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(format!(
        "{name} runs under ax dev in the background (pid {}): its output in {}; ax status {name} says how it is, \
         ax stop {name} ends it",
        child.id(),
        out.display()
    ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_binary_is_read_through_before_its_start() {
        let d = tempfile::tempdir().unwrap();
        let bin = d.path().join("app");
        std::fs::write(&bin, vec![7u8; 3 << 20]).unwrap();
        assert_eq!(super::read_through(&bin).unwrap(), 3 << 20);
        assert!(super::read_through(&d.path().join("gone")).is_err());
    }

    #[test]
    fn an_app_that_ended_is_said_by_its_own_reason() {
        let log = "2026-11-07T03:16:47.224Z [aio] notes 0.1.0 starting (pid 7)
2026-11-07T03:16:47.225Z Error: an older run
2026-11-07T03:16:48.000Z [aio] notes 0.1.0 starting (pid 9)
2026-11-07T03:16:48.001Z [aio] boot app        notes
2026-11-07T03:16:48.002Z Error: Custom { kind: Other, error: \"counter: the saved data has fields\" }
2026-11-07T03:16:48.003Z [aio] notes stopped
";
        assert_eq!(
            super::why_ended(log, 9).as_deref(),
            Some("Error: Custom { kind: Other, error: \"counter: the saved data has fields\" }")
        );
        assert_eq!(super::why_ended(log, 7).as_deref(), Some("Error: an older run"), "that run's own");
        assert_eq!(super::why_ended(log, 8), None, "a run the log doesn't have");
        assert_eq!(
            super::why_ended("x [aio] a starting (pid 3)\nERROR [aio] a: the window failed\n", 3).as_deref(),
            Some("ERROR [aio] a: the window failed")
        );
    }

    use super::*;

    #[test]
    fn the_dev_feature_is_named_as_the_manifest_names_aio_shell() {
        let pkg = |deps: serde_json::Value| serde_json::json!({ "dependencies": deps });
        let renamed = pkg(serde_json::json!([{ "name": "aio-rs-shell", "rename": "aio-shell", "kind": null }]));
        assert_eq!(shell_dev_feature(&renamed).as_deref(), Some("aio-shell/dev"));
        let plain = pkg(serde_json::json!([{ "name": "aio-rs-shell", "rename": null, "kind": null }]));
        assert_eq!(shell_dev_feature(&plain).as_deref(), Some("aio-rs-shell/dev"));
        // A service or a CLI app: no window shell, nothing to patch (no feature to ask cargo for).
        let none = pkg(serde_json::json!([{ "name": "aio-rs", "rename": "aio", "kind": null }]));
        assert_eq!(shell_dev_feature(&none), None);
        let dev_only = pkg(serde_json::json!([{ "name": "aio-rs-shell", "rename": null, "kind": "dev" }]));
        assert_eq!(shell_dev_feature(&dev_only), None);
    }

    #[test]
    fn a_cells_new_field_is_a_restart_a_body_edit_a_patch() {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("lib.rs");
        std::fs::write(
            &lib,
            "pub struct Counter {\n    pub count: i64,\n}\nfn add(c: &mut Counter) { c.count += 1; }\n",
        )
        .unwrap();
        let built = crate::reload::Built::snapshot(&[dir.path().to_path_buf()]);
        std::fs::write(
            &lib,
            "pub struct Counter {\n    pub count: i64,\n}\nfn add(c: &mut Counter) { c.count += 2; }\n",
        )
        .unwrap();
        assert!(reshaped(&built, std::slice::from_ref(&lib), dir.path()).is_empty());
        std::fs::write(
            &lib,
            "pub struct Counter {\n    pub count: i64,\n    pub note: String,\n}\nfn add(c: &mut Counter) {}\n",
        )
        .unwrap();
        assert_eq!(reshaped(&built, std::slice::from_ref(&lib), dir.path()), ["struct Counter (lib.rs)"]);
    }

    #[test]
    fn a_temporary_copy_renamed_away_is_no_change() {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("lib.rs");
        std::fs::write(&lib, "fn a() {}").unwrap();
        // `sed -i`: `sedAbC123` written, then renamed over lib.rs.
        let temp = dir.path().join("sedAbC123");
        let deleted = dir.path().join("old.rs");
        let known = |p: &Path| p == deleted.as_path();
        let got = real_changes(vec![lib.clone(), temp, deleted.clone()], &known);
        assert_eq!(got, vec![lib, deleted], "the source kept, a deleted source kept, the gone copy dropped");
        assert!(real_changes(vec![dir.path().join("x.rs.tmp.42")], &|_| false).is_empty());
    }

    #[test]
    fn an_unread_file_changes_nothing_only_when_gone_or_a_new_module() {
        let dir = tempfile::tempdir().unwrap();
        let new_mod = dir.path().join("panels.rs");
        let input = dir.path().join("schema.json");
        std::fs::write(&new_mod, "").unwrap();
        std::fs::write(&input, "{}").unwrap();
        let dirs = [dir.path().to_path_buf()];
        assert!(crate::hotpatch::unread_is_no_change(&dir.path().join("sedXYZ"), &dirs));
        assert!(crate::hotpatch::unread_is_no_change(&new_mod, &dirs));
        assert!(!crate::hotpatch::unread_is_no_change(&input, &dirs), "a build script's input, maybe: a rebuild's");
        let elsewhere = tempfile::tempdir().unwrap();
        let dep = elsewhere.path().join("boot.rs");
        std::fs::write(&dep, "").unwrap();
        assert!(!crate::hotpatch::unread_is_no_change(&dep, &dirs), "a path dependency's source: a rebuild's");
    }

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

    #[test]
    fn an_error_whose_fix_is_aios_says_it() {
        let e = "error[E0277]: the trait bound `Note: aio::serde::Serialize` is not satisfied\n  --> src/lib.rs:9:5\n";
        let said = with_hints(e);
        assert!(said.starts_with(e.trim_end()) && said.contains("= ax: ") && said.contains("#[aio::data]"), "{said}");
        assert_eq!(with_hints("error[E0425]: cannot find value `x`\n"), "error[E0425]: cannot find value `x`\n");
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
            Path::new("/w/target").join("desktop-dev").join(format!("notes{}", std::env::consts::EXE_SUFFIX))
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
        let pkg = Pkg {
            name: "n".into(),
            dirs: vec!["/w/app".into(), "/w".into()],
            bin: "/b".into(),
            version: "1".into(),
            dev_feature: None,
            features: vec![],
        };
        assert_eq!(watched(&pkg, Path::new("/w"), &Watching::Off), Vec::<PathBuf>::new());
        assert!(build_features(&pkg, true).is_empty());
        assert!(warms_after_create(true, false, None), "a person at a terminal: warmed");
        assert!(!warms_after_create(false, false, None), "an agent, a script, a test: never");
        assert!(!warms_after_create(true, true, None), "--no-warm");
        assert!(!warms_after_create(true, false, Some("1")), "AIORT_NO_WARM=1");
        assert!(warms_after_create(true, false, Some("0")));
        let shell = Pkg { dev_feature: Some("aio-shell/dev".into()), ..pkg.clone() };
        assert_eq!(build_features(&shell, true), ["--features", "aio-shell/dev"], "ax dev's tooling");
        assert!(build_features(&shell, false).is_empty(), "ax start: no door, no DevTools port, hardened");
        let fed = Pkg { features: vec!["aio-shell/updates".into()], ..shell.clone() };
        assert_eq!(build_features(&fed, true), ["--features", "aio-shell/dev,aio-shell/updates"], "an [updates] url");
        assert_eq!(build_features(&fed, false), ["--features", "aio-shell/updates"], "ax start updates as the release");
        assert_eq!(watched(&pkg, Path::new("/w"), &Watching::Only(s(&["src/ui"]))), [PathBuf::from("/w/src/ui")]);
        let all = watched(&pkg, Path::new("/w"), &Watching::All);
        assert!(all.contains(&PathBuf::from("/w/app/src")) && all.contains(&PathBuf::from("/w/Cargo.toml")), "{all:?}");
    }

    /// The dev build of an app with an `[updates] url` has the updater (its
    /// `updates` cell works under `ax dev`); one without, none.
    #[test]
    fn an_apps_feed_turns_on_the_updater_in_its_dev_builds() {
        let d = tempfile::tempdir().unwrap();
        let w = d.path();
        let (_, key) = aio::update::keygen().unwrap();
        let feed = format!(
            "[app]\nid = \"fed\"\n[capabilities]\nnetwork = [\"x.example\"]\n[updates]\nurl = \"https://x.example/fed\"\nkeys = [\"{key}\"]"
        );
        for (dir, toml) in [("fed", feed.as_str()), ("plain", "[app]\nid = \"plain\"")] {
            std::fs::create_dir_all(w.join(dir)).unwrap();
            std::fs::write(w.join(dir).join("aio.toml"), toml).unwrap();
        }
        let pkg = |name: &str| {
            serde_json::json!({
                "name": name, "manifest_path": w.join(name).join("Cargo.toml"),
                "targets": [{ "name": name, "kind": ["bin"] }],
                "dependencies": [{ "name": "aio-rs-shell", "rename": "aio-shell", "kind": null }]
            })
        };
        let meta = serde_json::json!({ "target_directory": w.join("target"), "packages": [pkg("fed"), pkg("plain")] });
        let fed = locate(&meta, "fed").unwrap();
        assert_eq!(build_features(&fed, true), ["--features", "aio-shell/dev,aio-shell/updates"]);
        let plain = locate(&meta, "plain").unwrap();
        assert_eq!(build_features(&plain, true), ["--features", "aio-shell/dev"], "no feed: no updater");
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
        assert!(!home.join("data").exists() && home.join(FRESH).exists(), "gone, marked for ax stop");
        throw_away(Some(&plan("--instance"))).unwrap();
        // aio's `am stop --instance`: the stopped --fresh run's data goes too.
        assert_eq!(discard_fresh(&plan("--instance")), Some(Ok(())));
        assert!(!home.exists());
        assert_eq!(discard_fresh(&plan("--instance")), None, "no such run: nothing");
        assert_eq!(discard_fresh(&plan("default")), None);
    }

    #[test]
    fn a_cargo_target_dir_the_person_set_is_kept_one_an_ax_set_is_not() {
        let o = |s: &str| Some(std::ffi::OsString::from(s));
        assert_eq!(persons_target(None, o("/t"), None), o("/t"), "the person's");
        assert_eq!(persons_target(None, o("/t"), o("/t")), None, "an ax's, for its children");
        assert_eq!(persons_target(None, o("/t"), o("/other")), o("/t"), "set again by the person");
        assert_eq!(persons_target(o("/mine"), o("/t"), None), None, "AIORT_TARGET_DIR wins");
        assert_eq!(persons_target(None, None, None), None);
    }

    /// Run again as a child by [`the_warm_up_runs_with_no_console_of_its_own`]:
    /// its console window (0: none) to the file `AX_CONSOLE_PROBE` names.
    #[cfg(windows)]
    #[test]
    fn console_probe() {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetConsoleWindow() -> *mut std::ffi::c_void;
        }
        if let Some(to) = std::env::var_os("AX_CONSOLE_PROBE") {
            // SAFETY: no arguments; it only reads this process's console.
            let window = unsafe { GetConsoleWindow() } as usize;
            std::fs::write(to, window.to_string()).unwrap();
        }
    }

    #[cfg(windows)]
    #[test]
    fn the_warm_up_runs_with_no_console_of_its_own() {
        let d = tempfile::tempdir().unwrap();
        let said = d.path().join("console");
        let probe = |detach: bool| {
            let mut cmd = Command::new(std::env::current_exe().unwrap());
            cmd.args(["--exact", "dev::tests::console_probe"]).env("AX_CONSOLE_PROBE", &said);
            match detach {
                true => detached(&mut cmd),
                // A console of its own: this test's may have none (a CI runner).
                false => _ = std::os::windows::process::CommandExt::creation_flags(&mut cmd, 0x10),
            }
            assert!(cmd.output().unwrap().status.success());
            std::fs::read_to_string(&said).unwrap()
        };
        assert_eq!(probe(true), "0", "detached: no console");
        assert_ne!(probe(false), "0", "the probe sees a console when there is one");
    }

    #[cfg(unix)]
    #[test]
    fn a_warm_up_is_its_own_process_group() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "ps -o pgid= -p $$"]).stdout(std::process::Stdio::piped());
        detached(&mut cmd);
        let child = cmd.spawn().unwrap();
        let pid = child.id().to_string();
        let out = child.wait_with_output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), pid, "the leader of its own group");
    }

    #[test]
    fn a_patch_compiled_while_saves_settle_is_kept_only_for_those_saves_as_it_read_them() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = (d.path().join("a.rs"), d.path().join("b.rs"));
        std::fs::write(&a, "fn a() {}").unwrap();
        std::fs::write(&b, "fn b() {}").unwrap();
        let early = |files: &[PathBuf]| {
            let lib = d.path().join(format!("patch-{}.so", files.len()));
            std::fs::write(&lib, "").unwrap();
            let table = aio_hotpatch::JumpTable {
                lib: lib.clone(),
                map: Default::default(),
                aslr_reference: 0,
                new_base_address: 0,
                ifunc_count: 0,
            };
            let times = crate::hotpatch::Times {
                compile: Duration::ZERO,
                stub: Duration::ZERO,
                link: Duration::ZERO,
                total: Duration::ZERO,
            };
            (Early { read: contents(files), patch: Some(crate::hotpatch::Patch { table, times }) }, lib)
        };
        // The burst was just its saves, as the compile read them: its patch.
        let (e, lib) = early(std::slice::from_ref(&a));
        assert_eq!(e.take(std::slice::from_ref(&a)).map(|p| p.table.lib), Some(lib.clone()));
        assert!(lib.is_file(), "a patch taken keeps its library");
        // Another file saved in the burst: compiled afresh, the early one gone.
        let (e, lib) = early(std::slice::from_ref(&a));
        assert!(e.take(&[a.clone(), b.clone()]).is_none());
        assert!(!lib.exists(), "an unused patch leaves no library");
        // The file saved again after the compile read it (an editor's
        // truncate, then its write): compiled afresh.
        let (e, lib) = early(std::slice::from_ref(&a));
        std::fs::write(&a, "fn a() { 1; }").unwrap();
        assert!(e.take(std::slice::from_ref(&a)).is_none());
        assert!(!lib.exists());
        // Never taken (the burst was a stylesheet's after all): its library goes.
        let (e, lib) = early(std::slice::from_ref(&b));
        drop(e);
        assert!(!lib.exists());
    }

    #[test]
    fn the_next_starts_build_is_refreshed_once_a_patched_session_is_quiet() {
        let t0 = std::time::Instant::now();
        let r = |behind, failed, quiet: Option<u64>| Refresh {
            behind,
            failed,
            quiet_since: quiet.map(|ms| t0 - Duration::from_millis(ms)),
            running: None,
        };
        assert!(r(true, false, Some(2000)).due(t0), "patched, 2 s quiet");
        assert!(!r(true, false, Some(1500)).due(t0), "a save 1.5 s ago");
        assert!(!r(false, false, Some(5000)).due(t0), "nothing patched since the last build");
        assert!(!r(true, true, Some(5000)).due(t0), "the last one failed: not again until a patch");
        let mut running = r(true, false, Some(5000));
        running.running = Some(std::thread::spawn(|| Ok(true)));
        assert!(!running.due(t0), "one at a time");
        assert!(running.stop(&Stop::default()).is_some(), "its end said");
        assert!(!running.behind, "up to date after it");
    }

    #[test]
    fn markup_is_told_against_what_the_build_that_ended_read() {
        // The first build stopped by a save, the second built: the sources
        // are the second's, the save in them (a markup edit after is told
        // against them, or it reaches the window as the wrong text).
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("lib.rs");
        std::fs::write(&src, "rsx! { h1 { \"one\" } }").unwrap();
        let mut tries = 0;
        let (read, bin) = until_current(
            || std::fs::read_to_string(&src).unwrap(),
            || {
                tries += 1;
                match tries {
                    1 => {
                        std::fs::write(&src, "// saved while it built\nrsx! { h1 { \"one\" } }").unwrap();
                        Built::Stale
                    }
                    _ => Built::Bin("app".into()),
                }
            },
            || {},
        );
        assert_eq!(bin, Some(PathBuf::from("app")));
        assert!(read.starts_with("// saved while it built"), "{read}");
        let (read, bin) = until_current(|| 7, || Built::Failed, || panic!("no save"));
        assert_eq!((read, bin), (7, None), "a failed build: what it read too");
    }

    #[cfg(unix)]
    #[test]
    fn a_save_of_code_stops_the_build_a_stylesheet_does_not() {
        let d = tempfile::tempdir().unwrap();
        let (rs, css, gone) = (d.path().join("a.rs"), d.path().join("a.css"), d.path().join("tmp.rs"));
        std::fs::write(&rs, "").unwrap();
        std::fs::write(&css, "").unwrap();
        let (tx, rx) = mpsc::channel();
        let stop = Stop::default();
        let out = stale_builds(rx, stop.clone());
        // A save while a build runs: passed on, and the build stopped or not.
        let stops = |saved: PathBuf| {
            let tx = tx.clone();
            let sent = vec![saved];
            let again = sent.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(300));
                tx.send(again).unwrap();
            });
            let ran = crate::hotpatch::cargo_out::run(Command::new("sleep").arg("2"), true, &stop).unwrap();
            assert_eq!(out.recv_timeout(Duration::from_secs(5)).unwrap(), sent, "passed on");
            ran.stopped
        };
        assert!(!stops(css), "a stylesheet: read live, nothing to build");
        assert!(!stops(gone), "a temporary copy renamed away");
        assert!(stops(rs), "code: the build is stale");
    }

    #[test]
    fn the_shared_target_is_one_per_toolchain() {
        let d = tempfile::tempdir().unwrap();
        let app = d.path().join("app").join("src");
        std::fs::create_dir_all(&app).unwrap();
        assert_eq!(toolchain_key(&app, None), "default", "no toolchain file");
        std::fs::write(d.path().join("app/rust-toolchain.toml"), "[toolchain]\nchannel = \"1.98.0\"\n").unwrap();
        assert_eq!(toolchain_key(&app, None), "1.98.0", "the app's pin, from a folder in it");
        assert_eq!(toolchain_key(&app, Some("nightly-2026-10-01")), "nightly-2026-10-01", "RUSTUP_TOOLCHAIN first");
        assert_eq!(toolchain_key(&app, Some("../x")), "default", "never a path");
        assert_eq!(toolchain_key(&app, Some("1.98.0/x")), "1.98.0_x");
        std::fs::remove_file(d.path().join("app/rust-toolchain.toml")).unwrap();
        std::fs::write(d.path().join("rust-toolchain"), "# legacy\nstable\n").unwrap();
        assert_eq!(toolchain_key(&app, None), "stable", "the legacy file, above");
        // A member's own toolchain file is not the workspace's: the root names the folder.
        let ws = d.path().join("ws");
        std::fs::create_dir_all(ws.join("app/src")).unwrap();
        std::fs::write(ws.join("Cargo.toml"), "[workspace]\nmembers = [\"app\"]\n").unwrap();
        std::fs::write(ws.join("app/Cargo.toml"), "[package]\nname = \"app\"\n").unwrap();
        std::fs::write(ws.join("rust-toolchain.toml"), "[toolchain]\nchannel = \"1.98.0\"\n").unwrap();
        std::fs::write(ws.join("app/rust-toolchain.toml"), "[toolchain]\nchannel = \"nightly\"\n").unwrap();
        assert_eq!(workspace_root(&ws.join("app/src")), ws);
        assert_eq!(toolchain_key(&workspace_root(&ws.join("app/src")), None), "1.98.0");
        // The toolchain every cargo of ax runs on is the one the folder is named by,
        // wherever that cargo runs (a member's own file is not it).
        let none = |_: &Path| None;
        let pinned = toolchain_at(&ws, None, &none);
        assert_eq!(pinned.as_deref(), Some("1.98.0"));
        assert_eq!(toolchain_key(&ws.join("app"), pinned.as_deref()), "1.98.0", "a child ax in the member: the same");
        assert_eq!(toolchain_at(&ws, Some("beta"), &none).as_deref(), Some("beta"), "RUSTUP_TOOLCHAIN first");
        let other = tempfile::tempdir().unwrap();
        let bare = other.path().join("bare");
        std::fs::create_dir_all(bare.join("m")).unwrap();
        std::fs::write(bare.join("m/rust-toolchain"), "nightly\n").unwrap();
        let active = |_: &Path| Some("stable-x86_64-unknown-linux-gnu".to_string());
        assert_eq!(
            toolchain_at(&bare, None, &active).as_deref(),
            Some("stable-x86_64-unknown-linux-gnu"),
            "no file at the root: what rustup picks there, never the member's"
        );
        let lone = d.path().join("lone");
        std::fs::create_dir_all(lone.join("src")).unwrap();
        std::fs::write(lone.join("Cargo.toml"), "[package]\nname = \"lone\"\n").unwrap();
        assert_eq!(workspace_root(&lone.join("src")), lone, "a package alone is its own root");
    }

    #[test]
    fn a_debug_build_is_dxs_and_its_binary_is_the_one_cargo_wrote() {
        let l = layout().join(" ");
        assert!(l.ends_with("--profile desktop-dev") && !l.contains("--target"), "host and app share libraries: {l}");
        assert!(l.contains("profile.desktop-dev.strip=false"), "{l}");
        // Release semantics: what runs under ax dev is what people get.
        assert!(l.contains("profile.desktop-dev.debug-assertions=false"), "{l}");
        assert!(l.contains("profile.desktop-dev.overflow-checks=false"), "{l}");
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
        assert!(!relevant(Path::new("target/debug/x")), "relative to its root");
        assert!(!relevant(Path::new("/w/notes/src/.main.rs.swp")));
        assert!(!relevant(Path::new("/w/notes/src/main.rs~")));
        // vim's write probe, emacs' autosave, temp and safe-write copies.
        for editor in ["4913", "#lib.rs#", "lib.rs.tmp", "lib.rs.swx", "lib.rs___jb_tmp___", ".#lib.rs"] {
            assert!(!relevant(&Path::new("/w/notes/src").join(editor)), "{editor}");
        }
        assert!(relevant(Path::new("/w/notes/src/4913.rs")) && relevant(Path::new("/w/notes/assets/a#b.css")));
        assert!(!relevant(Path::new("C:\\w\\target\\debug\\x")), "Windows' separators");
    }

    #[test]
    fn no_watch_is_watch_false() {
        let s = |v: &[&str]| v.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        assert_eq!(take_watch(&s(&["--no-watch", "a.md"])), Ok((s(&["a.md"]), Watching::Off)));
        assert_eq!(take_watch(&s(&["--", "--no-watch"])).unwrap().1, Watching::All, "the app's own word");
        // `[dev] watch`: when no flag says otherwise.
        let dirs = s(&["src/ui"]);
        assert_eq!(or_config(Watching::All, dirs.clone()), Watching::Only(dirs.clone()));
        assert_eq!(or_config(Watching::Off, dirs.clone()), Watching::Off, "--watch=false wins");
        assert_eq!(or_config(Watching::Only(s(&["assets"])), dirs), Watching::Only(s(&["assets"])));
        assert_eq!(or_config(Watching::All, vec![]), Watching::All);
    }

    #[test]
    fn a_pid_file_names_its_process_only_while_that_one_runs() {
        let started = |p: u32| (p == 7).then(|| "100".to_string());
        let alive = |p: u32| p == 7 || p == 8;
        assert_eq!(recorded("7 100", &started, &alive), Some(7));
        assert_eq!(recorded("7 99", &started, &alive), None, "the pid handed out again");
        assert_eq!(recorded("9 100", &started, &alive), None, "gone");
        assert_eq!(recorded("8\n", &started, &alive), Some(8), "an older file: the pid alone");
        assert_eq!(recorded("x", &started, &alive), None);
        assert_eq!(
            recorded(&pid_record(std::process::id()), &aio::lifecycle::started, &aio::lifecycle::alive),
            Some(std::process::id())
        );
    }

    #[test]
    fn a_restart_storm_cools_down_and_a_pause_ends_it() {
        // aio's RESTART_STORM_MS, _LIMIT, _COOLDOWN_MS.
        let mut s = Storm::default();
        let fast = Duration::from_millis(300);
        for _ in 1..STORM_LIMIT {
            assert_eq!(s.wait(fast), None);
        }
        assert_eq!(s.wait(fast), Some(Duration::from_secs(5)));
        assert_eq!(s.wait(fast), Some(Duration::from_secs(5)), "still a storm");
        assert_eq!(s.wait(Duration::from_secs(3)), None, "a person's pace");
        assert_eq!(s.wait(fast), None);
    }

    #[test]
    fn a_manifest_change_and_an_unchanged_binary_are_told() {
        assert!(manifest(&[PathBuf::from("/w/src/lib.rs"), PathBuf::from("/w/Cargo.toml")]));
        assert!(!manifest(&[PathBuf::from("/w/src/lib.rs")]));
        assert!(sheets_only(&[PathBuf::from("/w/assets/style.css"), PathBuf::from("/w/assets/kit.css")]));
        assert!(!sheets_only(&[PathBuf::from("/w/assets/style.css"), PathBuf::from("/w/src/lib.rs")]));
        assert!(!sheets_only(&[]));
        let d = tempfile::tempdir().unwrap();
        let (a, b) = (d.path().join("a"), d.path().join("b"));
        std::fs::write(&a, vec![7u8; 3 << 20]).unwrap();
        std::fs::write(&b, vec![7u8; 3 << 20]).unwrap();
        assert_eq!(digest(&a), digest(&b));
        std::fs::write(&b, [vec![7u8; 3 << 20], vec![1]].concat()).unwrap();
        assert_ne!(digest(&a), digest(&b));
        assert_eq!(digest(&d.path().join("none")), None);
    }

    #[test]
    fn the_files_a_build_read_come_from_its_dep_info() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("target");
        let bin = target.join("x86").join("desktop-dev").join("notes");
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        let gen_rs = target.join("x86/desktop-dev/build/out/gen.rs");
        std::fs::write(bin.with_extension("d"), format!("{}: /w/aio.toml {}\n", bin.display(), gen_rs.display()))
            .unwrap();
        assert_eq!(deps_of(&bin), [PathBuf::from("/w/aio.toml")], "not the build folder's own");
        assert_eq!(deps_of(&d.path().join("none")), Vec::<PathBuf>::new());
    }
}

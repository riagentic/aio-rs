//! `ax build <app>`: an installable app for this OS (or Windows, cross-built
//! with `--windows`; other platforms with `[build] platforms`, `platform`). On Linux a release build is packed as
//! `dist/<id>-<version>-linux-<arch>.tar.gz` with the binary, its icon and
//! two scripts: `install.sh` puts it in `~/.local/bin` and the app menu (no
//! root), `uninstall.sh` takes that away again and keeps the user's data;
//! macOS and Windows bundles are in `bundle`. The shared runtime is not
//! inside: the app fetches it once per machine. What to make is the app's
//! `[build] targets` (`fleet`). With `--smoke`, a Linux bundle is then
//! started and stopped as a user would (`smoke`).
use crate::dev::Pkg;
use crate::fleet::{Kind, Planned};
use crate::platform::Platform;
use std::path::{Path, PathBuf};
use std::process::Command;

/// What the package says about the app it builds.
#[derive(Debug, PartialEq)]
pub struct App {
    pub id: String,
    pub name: String,
    pub version: String,
    /// `assets/icon.png` beside its `aio.toml`, if there is one.
    pub icon: Option<PathBuf>,
}

/// The app's `aio.toml`: the one `aio::config!()` builds in, the nearest
/// from its launcher's folder up (`aio_config::config::find`).
pub fn toml_of(pkg: &Pkg) -> Result<PathBuf, String> {
    let bin = pkg.dirs.first().ok_or("no app folder")?;
    aio_config::config::find(bin).ok_or_else(|| format!("no aio.toml in {} or above it", bin.display()))
}

/// The app's dir: the folder of its `aio.toml`.
pub fn app_dir(pkg: &Pkg) -> Result<PathBuf, String> {
    Ok(toml_of(pkg)?.parent().ok_or("aio.toml without a folder")?.to_path_buf())
}

/// The app behind `pkg` and its `aio.toml`.
pub fn app_of(pkg: &Pkg) -> Result<(App, aio::config::Config), String> {
    let file = toml_of(pkg)?;
    let dir = app_dir(pkg)?;
    let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    let cfg = aio::config::Config::parse(&text).map_err(|e| format!("{}: {e}", file.display()))?;
    let icon = Some(dir.join("assets/icon.png")).filter(|p| p.exists());
    let version = crate::version::of(cfg.app.version.as_deref(), &pkg.version, &pkg.dirs)?;
    Ok((App { id: cfg.app.id.clone(), name: cfg.app.name.clone(), version, icon }, cfg))
}

/// The menu entry, written by `install.sh` with the installed binary's path.
/// `StartupWMClass` is the app id: the window class aio gives it. `%U`:
/// the files and links "Open with" hands it, as its arguments.
pub fn desktop_entry(app: &App, exec: &str) -> String {
    let icon = if app.icon.is_some() { format!("Icon={}\n", app.id) } else { String::new() };
    format!(
        "[Desktop Entry]\nType=Application\nName={}\nExec=\"{exec}\" %U\n{icon}StartupWMClass={}\nTerminal=false\nCategories=Utility;\n",
        app.name.replace('\n', " "),
        app.id
    )
}

/// `install.sh`: for this user only, into the XDG dirs. The binary goes in
/// `aio-rt/install/<id>/versions/<version>/` with `current` pointing at it
/// (how updates swap versions), `~/.local/bin/<id>` through `current`,
/// and `installed.json` saying what and from where (`bundle`: its name).
/// `menu`: and in the app menu, with its icon (a CLI is not). POSIX `sh`:
/// Linux and macOS.
pub fn install_sh(app: &App, bundle: &str, menu: bool) -> String {
    let (id, v) = (&app.id, &app.version);
    let icon = if app.icon.is_some() {
        format!(
            "mkdir -p \"$data/icons/hicolor/256x256/apps\"\ninstall -m 0644 \"$here/{id}.png\" \"$data/icons/hicolor/256x256/apps/{id}.png\"\n"
        )
    } else {
        String::new()
    };
    let (menu, said) = match menu {
        true => (
            format!(
                "mkdir -p \"$data/applications\"\n{icon}cat > \"$data/applications/{id}.desktop\" <<EOF\n{}EOF\n\
                 if command -v update-desktop-database >/dev/null 2>&1; then update-desktop-database \"$data/applications\" || true; fi\n",
                desktop_entry(app, &format!("$bin/{id}"))
            ),
            ", and in the app menu",
        ),
        false => (String::new(), ""),
    };
    format!(
        "#!/bin/sh\n# Install {name} for this user (no root needed).\nset -eu\nhere=$(cd \"$(dirname \"$0\")\" && pwd)\n\
         bin=\"${{XDG_BIN_HOME:-$HOME/.local/bin}}\"\ndata=\"${{XDG_DATA_HOME:-$HOME/.local/share}}\"\n\
         home=\"$data/aio-rt/install/{id}\"\n\
         mkdir -p \"$bin\" \"$home/versions/{v}\"\n\
         install -m 0755 \"$here/{id}\" \"$home/versions/{v}/{id}\"\n\
         if [ -d \"$here/electron\" ]; then rm -rf \"$home/versions/{v}/electron\"; cp -R \"$here/electron\" \"$home/versions/{v}/electron\"; fi\n\
         rm -f \"$home/pending.json\" \"$home/first-boot\" \"$home/update-failed\"\n\
         ln -sfn \"versions/{v}\" \"$home/current\"\n\
         ln -sfn \"$home/current/{id}\" \"$bin/{id}\"\n\
         printf '{{\"id\":\"{id}\",\"version\":\"{v}\",\"source\":\"{bundle}.tar.gz\",\"installedAt\":\"%s\"}}\\n' \"$(date -u +%Y-%m-%dT%H:%M:%SZ)\" > \"$home/installed.json\"\n\
         {menu}\
         echo \"{name} {v} is installed: $bin/{id}{said}.\"\n",
        name = app.name,
    )
}

/// `uninstall.sh`: the app, its versions, menu entry and icon go; its data
/// stays unless `--data` is given.
pub fn uninstall_sh(app: &App) -> String {
    let id = &app.id;
    format!(
        "#!/bin/sh\n# Remove {name} for this user. Its data stays, unless --data (ax backup can save it first).\nset -eu\n\
         bin=\"${{XDG_BIN_HOME:-$HOME/.local/bin}}\"\ndata=\"${{XDG_DATA_HOME:-$HOME/.local/share}}\"\n\
         cache=\"${{XDG_CACHE_HOME:-$HOME/.cache}}\"\n\
         rm -f \"$bin/{id}\" \"$data/applications/{id}.desktop\" \"$data/icons/hicolor/256x256/apps/{id}.png\"\n\
         rm -rf \"$data/aio-rt/install/{id}\"\n\
         if [ \"${{1:-}}\" = \"--data\" ]; then\n\
         \x20 rm -rf \"$data/aio-rt/apps/{id}\" \"$cache/aio-rt/{id}\"\n\
         \x20 echo \"{name} and its data are removed.\"\n\
         else\n\
         \x20 echo \"{name} is removed. Its data is still in $data/aio-rt/apps/{id} (uninstall.sh --data removes it).\"\n\
         fi\n",
        name = app.name,
    )
}

/// A desktop app's budget in MB: core + `kit`, the shared runtime not
/// counted (an app with heavy batteries declares its own: `[build] budget_mb`).
pub const BUDGET_MB: u64 = 10;
/// A server's (no window, no `kit`).
pub const SERVER_BUDGET_MB: u64 = 6;

/// The budget `cfg` builds within: its own, else its mode's.
pub fn budget_of(cfg: &aio::config::Config) -> u64 {
    let default = if cfg.app.mode == aio::config::Mode::Server { SERVER_BUDGET_MB } else { BUDGET_MB };
    cfg.build.budget_mb.unwrap_or(default)
}

/// Why a binary of `bytes` is refused, if it is over `budget_mb`.
pub fn over_budget(bytes: u64, budget_mb: u64) -> Option<String> {
    (bytes > budget_mb * 1_000_000).then(|| format!("{:.1} MB is over the {budget_mb} MB budget", bytes as f64 / 1e6))
}

/// `<id>-<version>-<os>-<arch>` for platform `p`.
pub fn bundle_name(app: &App, p: &Platform) -> String {
    format!("{}-{}-{}-{}", app.id, app.version, p.os, crate::platform::arch_word(p))
}

/// The desktop bundle's file for platform `p`: a `.zip` on macOS and
/// Windows, else a `.tar.gz`.
pub fn bundle_file(app: &App, p: &Platform) -> String {
    format!("{}.{}", bundle_name(app, p), if matches!(p.os, "macos" | "windows") { "zip" } else { "tar.gz" })
}

/// A CLI's: `<id>-<version>-<os>-<arch>-cli`.
pub fn cli_bundle_name(app: &App, p: &Platform) -> String {
    format!("{}-cli", bundle_name(app, p))
}

/// The Windows kind's platform: x64.
fn windows() -> &'static Platform {
    crate::platform::of("windows", "x86_64").expect("windows x64 is a platform")
}

/// Lay out the bundle `bundle` (its name) in `stage/<bundle>/`; `menu`: an
/// app menu entry (a desktop app's; not a CLI's).
pub fn stage(app: &App, bundle: &str, binary: &Path, stage: &Path, menu: bool) -> std::io::Result<PathBuf> {
    let dir = stage.join(bundle);
    std::fs::create_dir_all(&dir)?;
    std::fs::copy(binary, dir.join(&app.id))?;
    if let Some(icon) = app.icon.as_ref().filter(|_| menu) {
        std::fs::copy(icon, dir.join(format!("{}.png", app.id)))?;
    }
    for (file, text) in [("install.sh", install_sh(app, bundle, menu)), ("uninstall.sh", uninstall_sh(app))] {
        std::fs::write(dir.join(file), text)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.join(file), std::fs::Permissions::from_mode(0o755))?;
        }
    }
    Ok(dir)
}

/// A build: the app, its config, the release binary and where its
/// bundles go.
pub struct Built {
    pub app: App,
    pub cfg: aio::config::Config,
    pub binary: PathBuf,
    pub dist: PathBuf,
    /// The OS and CPU it is for.
    pub platform: &'static Platform,
    /// Its own Electron, to go beside the binary in every package
    /// (`[build] runtime = "bundled"`).
    pub runtime: Option<PathBuf>,
}

/// What builds keep between runs: `~/.cache/aio-rt`.
pub fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache"))
        .join("aio-rt")
}

/// `[build] runtime = "bundled"`: Electron for `p`, trimmed and fused in
/// the target dir, ready to go in the app's packages (a macOS one is made
/// on a Mac: it is signed again).
fn bundled_runtime(app: &App, cfg: &aio::config::Config, p: &Platform, target: &Path) -> Result<PathBuf, String> {
    use aio::config::{Extras, Mode};
    if matches!(cfg.app.mode, Mode::Server | Mode::Cli) {
        return Err("[build] runtime = \"bundled\": this app has no window, so no Electron to bundle".into());
    }
    let slug = electron_slug(p);
    let into = runtime_dir(target, &app.id, p);
    let cache = cache_dir().join("electron");
    let gone = aio_shell::runtime::bundle(&into, &slug, &cache, cfg.build.chromium_extras == Extras::Strip)
        .map_err(|e| format!("the bundled runtime: {e}"))?;
    eprintln!("ax build: Electron {} for {slug} bundled; trimmed: {}", aio_shell::runtime::VERSION, trimmed(&gone));
    Ok(into)
}

/// What a runtime's trim took, said short: the locales by count (a macOS
/// runtime has hundreds), the rest by name. Pure.
fn trimmed(gone: &[String]) -> String {
    let (locales, extras): (Vec<&String>, Vec<&String>) =
        gone.iter().partition(|g| g.ends_with(".pak") || g.ends_with(".lproj"));
    let mut said: Vec<String> = extras.iter().map(|e| e.to_string()).collect();
    if !locales.is_empty() {
        said.insert(0, format!("{} locales", locales.len()));
    }
    if said.is_empty() { "nothing".into() } else { said.join(", ") }
}

/// Where app `id`'s bundled Electron for `p` is made, in the target dir.
fn runtime_dir(target: &Path, id: &str, p: &Platform) -> PathBuf {
    target.join("aio").join(format!("{id}-electron-{}", electron_slug(p)))
}

/// Electron's name for `p` (`darwin-arm64`, `win32-x64`). Pure.
fn electron_slug(p: &Platform) -> String {
    let os = match p.os {
        "macos" => "darwin",
        "windows" => "win32",
        o => o,
    };
    format!("{os}-{}", crate::platform::arch_word(p))
}

/// The runtime an earlier build bundled for app `id` (`cfg`'s) on `p`,
/// so a package made again from it still holds it (pure).
fn kept_runtime(cfg: &aio::config::Config, target: &Path, p: &Platform) -> Option<PathBuf> {
    (cfg.build.runtime == aio::config::RuntimeFrom::Bundled).then(|| runtime_dir(target, &cfg.app.id, p))
}

/// Put build `b`'s own runtime, if it has one, in `dir` beside its binary.
pub fn put_runtime(b: &Built, dir: &Path) -> Result<(), String> {
    let Some(rt) = &b.runtime else { return Ok(()) };
    if !rt.is_dir() {
        return Err(format!("{}: the bundled runtime is gone (ax build {} makes it again)", rt.display(), b.app.id));
    }
    let to = dir.join(aio_shell::runtime::BUNDLED);
    _ = std::fs::remove_dir_all(&to);
    let st = Command::new("cp").arg("-R").arg("-P").arg(rt).arg(&to).status().map_err(|e| e.to_string())?;
    if !st.success() {
        return Err(format!("could not copy the runtime into {}", dir.display()));
    }
    Ok(())
}

/// Is `tool` a program on `PATH`?
fn on_path(tool: &str) -> bool {
    let exe = if cfg!(windows) { format!("{tool}.exe") } else { tool.to_string() };
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(&exe).is_file()))
}

/// Which of Rust's `targets` rustup lacks for the toolchain here (none
/// when there is no rustup: cargo then says it).
pub fn missing_targets<'a>(targets: &[&'a str]) -> Vec<&'a str> {
    let Ok(o) = Command::new("rustup").args(["target", "list", "--installed"]).output() else { return vec![] };
    if !o.status.success() {
        return vec![];
    }
    let have = String::from_utf8_lossy(&o.stdout);
    targets.iter().copied().filter(|t| !have.lines().any(|l| l.trim() == *t)).collect()
}

/// The cargo for a build for `cross` (None: this machine): refused before it
/// runs when the platform can't be built here (`platform::blocker`), else
/// told its target and linker.
fn cargo_for(what: &str, args: &[&str], cross: Option<&'static Platform>) -> Result<Command, String> {
    let mut cargo = crate::freeze::cargo(what, args)?;
    let Some(p) = cross else { return Ok(cargo) };
    let missing = missing_targets(&[p.triple]);
    let has_target = |t: &str| !missing.contains(&t);
    if let Some(why) = crate::platform::blocker(p, crate::platform::host()?, &has_target, &on_path) {
        return Err(why);
    }
    cargo.args(["--target", p.triple]);
    let var = crate::platform::linker_var(p.triple);
    if let Some(l) = p.linker
        && std::env::var_os(&var).is_none()
    {
        cargo.env(var, l);
    }
    Ok(cargo)
}

/// The release link's own arguments for `triple`: identical functions folded
/// and string tails merged where the linker is LLD (Rust's own on x86_64
/// Linux; other targets link with linkers that may not), 0.8% off a
/// desktop app. `relr`: the build machine's glibc loads packed relative
/// relocations (DT_RELR), so they are packed too, 3% off a server (the
/// binary then needs glibc 2.36, which one built there needs anyway).
pub fn link_args(triple: &str, relr: bool) -> &'static [&'static str] {
    const LLD: [&str; 2] = ["-Clink-arg=-Wl,--icf=all", "-Clink-arg=-Wl,-O2"];
    match (triple == "x86_64-unknown-linux-gnu", relr) {
        (false, _) => &[],
        (true, false) => &LLD,
        (true, true) => &[LLD[0], LLD[1], "-Clink-arg=-Wl,-z,pack-relative-relocs"],
    }
}

/// Does `getconf GNU_LIBC_VERSION`'s answer (`glibc 2.39`) name a glibc
/// that loads DT_RELR (2.36 on)?
pub fn relr_loads(getconf: &str) -> bool {
    let v = getconf.trim().strip_prefix("glibc ").unwrap_or("");
    let mut n = v.split('.').map(|p| p.parse::<u32>().ok());
    matches!((n.next(), n.next()), (Some(Some(major)), Some(Some(minor))) if (major, minor) >= (2, 36))
}

/// The release link's arguments on this machine for `triple`.
fn link_args_here(triple: &str) -> &'static [&'static str] {
    let getconf = Command::new("getconf").arg("GNU_LIBC_VERSION").output();
    link_args(triple, getconf.is_ok_and(|o| o.status.success() && relr_loads(&String::from_utf8_lossy(&o.stdout))))
}

/// `--remap-path-prefix` flags that keep the build machine's paths (its
/// user's name, its folders) out of a release binary's panic locations:
/// the home as `~`, cargo's home as `/cargo`, the workspace as `/build`
/// (rustc takes the last that matches, so the narrower come last) (pure).
pub fn remaps(home: Option<&Path>, cargo_home: Option<&Path>, root: &Path) -> Vec<String> {
    let wide = |p: &&Path| p.components().count() > 1;
    [(home, "~"), (cargo_home, "/cargo"), (Some(root), "/build")]
        .into_iter()
        .filter_map(|(p, to)| Some(format!("--remap-path-prefix={}={to}", p.filter(wide)?.display())))
        .collect()
}

/// `cargo` told [`remaps`] for the workspace at `root`, after the flags
/// it already has: `CARGO_ENCODED_RUSTFLAGS` or `RUSTFLAGS` when one is
/// set (each wins over the config's), else the config's `build.rustflags`.
pub fn remap(cargo: &mut Command, root: &Path) {
    let home = std::env::home_dir();
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".cargo")));
    let ours = remaps(home.as_deref(), cargo_home.as_deref(), root);
    let set = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let had: Option<Vec<String>> = match (set("CARGO_ENCODED_RUSTFLAGS"), set("RUSTFLAGS")) {
        (Some(e), _) => Some(e.split('\x1f').map(String::from).collect()),
        (None, Some(r)) => Some(r.split_whitespace().map(String::from).collect()),
        (None, None) => None,
    };
    match had {
        Some(mut flags) => {
            flags.extend(ours);
            cargo.env("CARGO_ENCODED_RUSTFLAGS", flags.join("\x1f")).env_remove("RUSTFLAGS");
        }
        None => {
            let list: Vec<String> = ours.iter().map(|f| aio::serde_json::Value::from(f.as_str()).to_string()).collect();
            cargo.arg("--config").arg(format!("build.rustflags=[{}]", list.join(", ")));
        }
    }
}

/// Refused when `bin` (the binary at `at`) still holds the build machine's
/// home: its user's name would ship with it.
pub fn no_home_paths(bin: &[u8], at: &Path) -> Result<(), String> {
    let Some(home) = std::env::home_dir().filter(|h| h.components().count() > 1) else { return Ok(()) };
    let home = home.display().to_string();
    let n = bin.windows(home.len()).filter(|w| *w == home.as_bytes()).count();
    match n {
        0 => Ok(()),
        n => Err(format!(
            "{} holds this machine's home path {n}× ({home}…): it would ship the user's name and folders",
            at.display()
        )),
    }
}

/// The release binary `name` of package `pkg`, built for `cross` (None:
/// this machine); `link`: an object its link takes too (the exe's icon).
pub fn release_binary(
    meta: &serde_json::Value,
    pkg: &str,
    name: &str,
    cross: Option<&'static Platform>,
    link: Option<&Path>,
    channel: Option<&str>,
    version: &str,
) -> Result<PathBuf, String> {
    let root = Path::new(meta["workspace_root"].as_str().ok_or("cargo metadata: no workspace_root")?);
    let mut cargo = cargo_for("the release build", &["rustc", "--release", "-p", pkg, "--bin", name], cross)?;
    remap(&mut cargo, root);
    cargo.arg("--").args(link_args_here(cross.unwrap_or(crate::platform::host()?).triple));
    if let Some(obj) = link {
        cargo.arg(format!("-Clink-arg={}", obj.display()));
    }
    // The channel the build follows (`aio::update::STAMPED_CHANNEL`), or none.
    match channel {
        Some(c) => cargo.env("AIORT_CHANNEL", c),
        None => cargo.env_remove("AIORT_CHANNEL"),
    };
    // The app's version (`aio::app_version!()`, `crate::version`).
    cargo.env("AIORT_APP_VERSION", version);
    let ok = cargo.status().map_err(|e| e.to_string())?;
    if !ok.success() {
        return Err("the release build failed".into());
    }
    let target = meta["target_directory"].as_str().ok_or("cargo metadata: no target_directory")?;
    let os = cross.map_or(std::env::consts::OS, |p| p.os);
    let bin = crate::bundle::binary(Path::new(target), cross.map(|p| p.triple), name, os);
    no_home_paths(&std::fs::read(&bin).map_err(|e| format!("{}: {e}", bin.display()))?, &bin)?;
    Ok(bin)
}

/// The name the monogram draws: the app's, else its id.
pub fn label(app: &App) -> &str {
    if app.name.is_empty() { &app.id } else { &app.name }
}

/// The icon of an app that has none, in the target dir: its monogram
/// (`aio::icon`) in the app's hue as a 512 px PNG and an SVG beside it (aio's
/// `writeDefaultIcon`), and for a Mac an `.icns` drawn at each slot's size.
fn default_icon(target: &Path, app: &App, hue: u16, mac: bool) -> Result<PathBuf, String> {
    let p = target.join("aio").join(format!("{}-icon.png", app.id));
    let e = |e: std::io::Error| e.to_string();
    std::fs::create_dir_all(p.parent().ok_or("bad path")?).map_err(e)?;
    std::fs::write(&p, aio_config::icon::icon_png(label(app), hue, 512)).map_err(e)?;
    std::fs::write(p.with_extension("svg"), aio_config::icon::svg(label(app), hue, 512)).map_err(e)?;
    match mac {
        true => std::fs::write(p.with_extension("icns"), crate::icon::icon_icns(label(app), hue)).map_err(e)?,
        false => _ = std::fs::remove_file(p.with_extension("icns")),
    }
    Ok(p)
}

/// Which crates take the space in binary `name` of `pkg`: a release build
/// that keeps its symbols (the next build strips them again).
pub fn sizes(
    meta: &serde_json::Value,
    pkg: &str,
    name: &str,
    cross: Option<&'static Platform>,
) -> Result<String, String> {
    eprintln!("ax build: {name} with its symbols, to see which crates take the space…");
    let root = Path::new(meta["workspace_root"].as_str().ok_or("cargo metadata: no workspace_root")?);
    let mut cargo = cargo_for("the size build", &["rustc", "--release", "-p", pkg, "--bin", name], cross)?;
    remap(&mut cargo, root);
    let ok = cargo
        .arg("--")
        .args(link_args_here(cross.unwrap_or(crate::platform::host()?).triple))
        .env("CARGO_PROFILE_RELEASE_STRIP", "none")
        .status()
        .map_err(|e| e.to_string())?;
    if !ok.success() {
        return Err("the size build failed".into());
    }
    let target = meta["target_directory"].as_str().ok_or("cargo metadata: no target_directory")?;
    let os = cross.map_or(std::env::consts::OS, |p| p.os);
    let bin = std::fs::read(crate::bundle::binary(Path::new(target), cross.map(|p| p.triple), name, os))
        .map_err(|e| e.to_string())?;
    Ok(crate::sizes::report(&crate::sizes::analyze(&crate::sizes::symbols(&bin)?, 20), crate::sizes::shipped(&bin)?))
}

/// `ax build <app> --sizes`: its release binary's size against its budget,
/// built as `ax build` builds it (a plain `cargo build --release` lacks
/// aio's link flags: some % bigger), and which crates take the space.
pub fn sizes_of(name: &str) -> Result<String, String> {
    // Over its budget, that fails saying so, with the crates.
    let b = compile(name, None, None)?;
    let bytes = std::fs::metadata(&b.binary).map_err(|e| format!("{}: {e}", b.binary.display()))?.len();
    let meta = crate::dev::metadata()?;
    let pkg = crate::dev::locate(&meta, name)?;
    let budget = budget_of(&b.cfg);
    Ok(format!(
        "{name}: {:.2} MB of its {budget} MB budget\n{}",
        bytes as f64 / 1e6,
        sizes(&meta, &pkg.name, name, None)?
    ))
}

/// Build `name` (release, within budget) and pack its bundle in `dist/`
/// (`pack`): for platform `p` (None: this machine's); stamped with
/// `channel`.
pub fn make(name: &str, p: Option<&'static Platform>, channel: Option<&str>) -> Result<(Built, PathBuf), String> {
    let b = compile(name, p, channel)?;
    let bundle = pack(&b)?;
    Ok((b, bundle))
}

/// What [`make`] made earlier for platform `p` (None: this machine's), as
/// the app is now: refused when its bundle is not in `dist/`.
pub fn built(name: &str, p: Option<&'static Platform>) -> Result<(Built, PathBuf), String> {
    let meta = crate::dev::metadata()?;
    let (app, cfg) = app_of(&crate::dev::locate(&meta, name)?)?;
    let host = crate::platform::host()?;
    let p = p.unwrap_or(host);
    let target = Path::new(meta["target_directory"].as_str().ok_or("cargo metadata: no target_directory")?);
    let binary = crate::bundle::binary(target, (p != host).then_some(p.triple), name, p.os);
    let root = meta["workspace_root"].as_str().ok_or("cargo metadata: no workspace_root")?;
    let dist = Path::new(root).join("dist");
    let bundle = dist.join(bundle_file(&app, p));
    if !bundle.is_file() {
        return Err(format!(
            "{}: not built (ax build {name} --platforms {}), or leave out --no-build",
            bundle.display(),
            p.name
        ));
    }
    // Its bundle holds a bundled runtime already; whatever packs it again takes it too.
    let runtime = kept_runtime(&cfg, target, p);
    Ok((Built { app, cfg, binary, dist, platform: p, runtime }, bundle))
}

/// Build `name` (release, within budget) for platform `p` (cross-built
/// when it isn't this machine; None: this machine's), stamped with
/// `channel`.
fn compile(name: &str, p: Option<&'static Platform>, channel: Option<&str>) -> Result<Built, String> {
    let meta = crate::dev::metadata()?;
    let pkg = crate::dev::locate(&meta, name)?;
    crate::dev::claim_in(&meta, name)?;
    let (mut app, cfg) = app_of(&pkg)?;
    let host = crate::platform::host()?;
    let p = p.unwrap_or(host);
    let (os, cross) = (p.os, (p != host).then_some(p));
    let windows = os == "windows";
    let target = Path::new(meta["target_directory"].as_str().ok_or("cargo metadata: no target_directory")?);
    let dir = app_dir(&pkg)?;
    for hint in crate::icon::hints(&dir) {
        eprintln!("ax build: {hint}");
    }
    // The exe wears the app's icon: its own `assets/icon.ico` as it is, else its
    // `icon.png` scaled, else its monogram drawn per size.
    let read = |p: &Path| std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()));
    let exe_icon = match (windows, &app.icon) {
        (false, _) => None,
        (true, _) if dir.join("assets/icon.ico").is_file() => Some(read(&dir.join("assets/icon.ico"))?),
        (true, Some(p)) => Some(crate::icon::ico_from_png(&read(p)?)?),
        (true, None) => Some(crate::icon::icon_ico(label(&app), cfg.hue())),
    };
    let exe_icon = exe_icon
        .map(|ico| {
            let dir = target.join("aio").join(format!("{}-exe-icon-{}", app.id, p.arch));
            crate::icon::exe_icon_object(&ico, &dir, p.arch)
        })
        .transpose()?;
    if app.icon.is_none() {
        app.icon = Some(default_icon(target, &app, cfg.hue(), os == "macos")?);
    }
    eprintln!("ax build: {} {} for {} (release)…", app.name, app.version, p.name);
    if let Some(did) = crate::css::step(&cfg, &app_dir(&pkg)?)? {
        eprintln!("ax build: {did}");
    }
    let binary = release_binary(&meta, &pkg.name, name, cross, exe_icon.as_deref(), channel, &app.version)?;
    let bytes = std::fs::metadata(&binary).map_err(|e| format!("{}: {e}", binary.display()))?.len();
    if cfg.app.version.is_some()
        && !crate::version::carried(&std::fs::read(&binary).map_err(|e| e.to_string())?, &app.version)
    {
        return Err(format!(
            "{}: the binary does not say its version {}: give aio_shell::start aio::app_version!(), not the \
             package's (env!(\"CARGO_PKG_VERSION\"))",
            app.name, app.version
        ));
    }
    if let Some(why) = over_budget(bytes, budget_of(&cfg)) {
        let sizes = sizes(&meta, &pkg.name, name, cross).unwrap_or_else(|e| format!("(no size report: {e})"));
        return Err(format!("{}: {why}\n{sizes}", app.name));
    }
    let runtime = match cfg.build.runtime {
        aio::config::RuntimeFrom::Shared => None,
        aio::config::RuntimeFrom::Bundled => Some(bundled_runtime(&app, &cfg, p, target)?),
    };
    let root = meta["workspace_root"].as_str().ok_or("cargo metadata: no workspace_root")?;
    Ok(Built { app, cfg, binary, dist: Path::new(root).join("dist"), platform: p, runtime })
}

/// The desktop bundle of build `b` in its `dist`: Linux `.tar.gz`, macOS
/// zipped `.app`, Windows `.zip` beside the one-click `.exe`.
fn pack(b: &Built) -> Result<PathBuf, String> {
    let (app, cfg, binary, dist, p) = (&b.app, &b.cfg, &b.binary, &b.dist, b.platform);
    let staging = dist.join(format!(".{}-staging", app.id));
    _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let out = dist.join(bundle_file(app, p));
    let made = (|| {
        match p.os {
            "macos" => crate::bundle::mac_zip(
                &crate::bundle::mac_app(app, &cfg.mac_bundle_id(), binary, b.runtime.as_deref(), &staging)?,
                &out,
            )?,
            "windows" => {
                crate::bundle::windows_zip(app, binary, b.runtime.as_deref(), &out)?;
                // The one-click download beside it (`aio_shell::oneclick`),
                // carrying the runtime of an offline build.
                let exe = out.with_extension("exe");
                match &b.runtime {
                    None => std::fs::copy(binary, &exe).map(drop),
                    Some(rt) => std::fs::write(
                        &exe,
                        aio_shell::oneclick::with_runtime(
                            &std::fs::read(binary).map_err(|e| e.to_string())?,
                            &crate::bundle::runtime_zip(rt)?,
                        ),
                    ),
                }
                .map_err(|e| e.to_string())?;
            }
            _ => tar_bundle(b, &bundle_name(app, p), &staging, &out, true)?,
        };
        Ok(out)
    })();
    _ = std::fs::remove_dir_all(&staging);
    made
}

/// Build `b` as the bundle `bundle` (its name) in `out` (a `.tar.gz`),
/// staged in `staging`; `menu`: as `stage`.
fn tar_bundle(b: &Built, bundle: &str, staging: &Path, out: &Path, menu: bool) -> Result<(), String> {
    let dir = stage(&b.app, bundle, &b.binary, staging, menu).map_err(|e| e.to_string())?;
    put_runtime(b, &dir)?;
    let tar = Command::new("tar").arg("-czf").arg(out).arg("-C").arg(staging).arg(bundle).status();
    if !tar.map_err(|e| e.to_string())?.success() {
        return Err("tar failed".into());
    }
    Ok(())
}

/// The CLI bundle of build `b`: on Linux and macOS a `.tar.gz` whose
/// `install.sh` puts it on `PATH` (no menu entry), on Windows a `.zip`
/// holding `<id>.exe`; with how to install it.
fn cli(b: &Built) -> Result<(String, Vec<PathBuf>), String> {
    let dist = &b.dist;
    std::fs::create_dir_all(dist).map_err(|e| e.to_string())?;
    let name = cli_bundle_name(&b.app, b.platform);
    let windows = b.platform.os == "windows";
    let out = dist.join(format!("{name}.{}", if windows { "zip" } else { "tar.gz" }));
    if windows {
        crate::bundle::windows_zip(&b.app, &b.binary, None, &out)?;
    } else {
        let staging = dist.join(format!(".{}-cli-staging", b.app.id));
        _ = std::fs::remove_dir_all(&staging);
        let made = tar_bundle(b, &name, &staging, &out, false);
        _ = std::fs::remove_dir_all(&staging);
        made?;
    }
    let file = out.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let mut how = match windows {
        true => format!("install: unzip {file}; {}.exe is the program", b.app.id),
        false => format!("install: tar -xzf {file}, then run its install.sh (puts {} on PATH)", b.app.id),
    };
    let mut files = vec![out.clone()];
    // A server: its supervisor's file beside it (`service`).
    if b.cfg.app.mode == aio::config::Mode::Server
        && let Some((file, text, install)) = crate::service::file(&b.app, b.platform.os, &name)
    {
        let p = dist.join(file);
        std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))?;
        how = format!("{how}\n{install}");
        files.push(p);
    }
    Ok((format!("{} ({:.1} MB)\n{how}", out.display(), size(&out)), files))
}

/// `ax build <app> --list`: the kinds and the app's targets.
pub fn list(name: &str) -> Result<String, String> {
    let (_, cfg) = app_of(&crate::dev::locate(&crate::dev::metadata()?, name)?)?;
    Ok(crate::fleet::list(name, &cfg.build.targets))
}

/// `ax build <app>`: each of its targets (`[build] targets`, or `select`:
/// `a,b`) on each of its platforms (`[build] platforms`, or `platforms`),
/// a failed one not stopping the rest. `smoke`: then every Linux artifact
/// (a Mac: server or CLI bundle) made for this machine, and each built on a
/// Mac there, is started and stopped as a user would (`smoke`).
pub fn run(name: &str, select: Option<&str>, platforms: Option<&str>, smoke: bool) -> Result<String, String> {
    let meta = crate::dev::metadata()?;
    let (_, cfg) = app_of(&crate::dev::locate(&meta, name)?)?;
    let host = crate::platform::host()?;
    let names: Vec<&str> = match platforms {
        Some(p) => p.split(',').collect(),
        None => cfg.build.platforms.iter().map(String::as_str).collect(),
    };
    let fallback = match cfg.app.mode {
        aio::config::Mode::Server | aio::config::Mode::Cli => crate::fleet::Kind::Cli,
        _ => crate::fleet::Kind::Desktop,
    };
    let plan = crate::fleet::across(
        crate::fleet::plan(name, &cfg.build.targets, select, fallback)?,
        &crate::platform::resolve(&names, host)?,
    );
    let (apps, waits): (Vec<App>, Vec<crate::smoke::Wait>) = plan
        .iter()
        .map(|t| app_of(&crate::dev::locate(&meta, &t.bin)?).map(|(a, c)| (a, crate::smoke::Wait::of(&c.app.mode))))
        .collect::<Result<Vec<_>, String>>()?
        .into_iter()
        .unzip();
    let pairs: Vec<_> = plan.iter().zip(&apps).map(|(t, a)| (t, (a.id.as_str(), a.name.as_str()))).collect();
    for w in crate::fleet::clashes(&pairs)? {
        eprintln!("ax build: {w}");
    }
    let (mut made, mut out, mut failed, mut records) = (vec![], vec![], vec![], vec![]);
    for t in &plan {
        let r = target(name, &meta, t, &mut made, smoke);
        let application_id = match t.kind {
            #[cfg(unix)]
            Kind::Android => {
                let (a, c) = app_of(&crate::dev::locate(&meta, &t.bin)?)?;
                Some(crate::android::application_id_of(&a.id, &c.build.android))
            }
            _ => None,
        };
        records.push(crate::manifest::Target {
            label: t.shown(),
            kind: t.kind.name().into(),
            ok: r.is_ok(),
            application_id,
            files: r.as_ref().map(|(_, f)| f.clone()).unwrap_or_default(),
        });
        match r {
            Ok((lines, _)) => out.push(lines),
            Err(e) if plan.len() == 1 => {
                record(&meta, name, &records);
                return Err(e);
            }
            Err(e) => {
                let on = t.platform.map_or(String::new(), |p| format!(" --platforms {}", p.name));
                failed.push(format!("{}: {e}\n  alone: ax build {name} --targets {}{on}", t.shown(), t.label));
            }
        }
    }
    record(&meta, name, &records);
    if !failed.is_empty() {
        out.push(format!("{} of {} targets failed:\n{}", failed.len(), plan.len(), failed.join("\n")));
        return Err(out.join("\n"));
    }
    if smoke || cfg.build.smoke {
        // Every Linux artifact made for this machine: the bundle (a CLI's
        // too), the .deb, the AppImage; a Windows bundle on the Windows
        // machine AIO_WINDOWS_SSH names.
        let ids: Vec<(&str, crate::smoke::Wait)> = apps.iter().map(|a| a.id.as_str()).zip(waits).collect();
        let windows = std::env::var_os(crate::smoke::WINDOWS_ENV).is_some_and(|v| !v.is_empty());
        let here: Vec<(&PathBuf, (&str, crate::smoke::Wait))> = records
            .iter()
            .zip(&ids)
            .zip(&plan)
            .filter(|(_, t)| t.platform.is_none_or(|p| p == host || (windows && p.os == "windows")))
            .flat_map(|((r, id), _)| r.files.iter().map(move |f| (f, *id)))
            .filter(|(f, (id, wait))| {
                crate::smoke::startable(host.os, f, id, *wait) || (windows && crate::smoke::is_windows_zip(f))
            })
            .collect();
        // A macOS target built on a Mac was smoke-tested there.
        let on_mac = plan.iter().any(|t| remote_mac(name, &meta, t).is_ok_and(|m| m.is_some()));
        match (here.is_empty(), on_mac) {
            (true, true) => return Ok(out.join("\n")),
            (true, false) => {
                return Err("--smoke starts a Linux bundle, .deb or AppImage, or a macOS server's or CLI's \
                            bundle, of this machine: build one there; a Windows bundle on Windows \
                            (AIO_WINDOWS_SSH names one)"
                    .into());
            }
            _ => {}
        }
        eprintln!("{}", out.join("\n"));
        // One report per artifact, each named by its file.
        let named = |f: &Path, id: &str, r: String| {
            let file = f.file_name().map_or_else(|| f.display().to_string(), |n| n.to_string_lossy().into_owned());
            r.replacen(&format!("smoke {id}:"), &format!("smoke {file}:"), 1)
        };
        return here
            .iter()
            .map(|(f, (id, wait))| {
                match crate::smoke::is_windows_zip(f) {
                    true => crate::smoke::windows(f, id, *wait),
                    false => crate::smoke::run(f, id, *wait),
                }
                .map(|r| named(f, id, r))
                .map_err(|e| named(f, id, e))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|r| r.join("\n"));
    }
    Ok(out.join("\n"))
}

/// Write `dist/<id>.manifest.json` for this build of `name`, saying what
/// the last one tells about it.
fn record(meta: &serde_json::Value, name: &str, targets: &[crate::manifest::Target]) {
    let wrote = (|| -> Result<(), String> {
        let pkg = crate::dev::locate(meta, name)?;
        let app = app_of(&pkg)?.0;
        let dist = Path::new(meta["workspace_root"].as_str().ok_or("cargo metadata: no workspace_root")?).join("dist");
        let tree = crate::manifest::tree(&pkg.dirs);
        if let Some((commit, Some(hash))) = &tree {
            eprintln!("ax build: built from a dirty tree ({commit}, edits {hash}): not a release");
        }
        let now =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
        let next = crate::manifest::manifest(&app.id, &app.name, &app.version, &tree, &aio::log::stamp(now), targets);
        let file = crate::manifest::path(&dist, &app.id);
        let prev = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        for w in crate::manifest::warnings(&prev, &next) {
            eprintln!("ax build: {w}");
        }
        std::fs::create_dir_all(&dist).map_err(|e| e.to_string())?;
        std::fs::write(&file, serde_json::to_string_pretty(&next).map_err(|e| e.to_string())? + "\n")
            .map_err(|e| e.to_string())
    })();
    if let Err(e) = wrote {
        eprintln!("ax build: no manifest: {e}");
    }
}

/// The Mac target `t` of app `name` is built on over SSH, if it is a macOS
/// one and this machine is no Mac: `[build.macos] host`, else `AIO_MACOS_SSH`.
fn remote_mac(name: &str, meta: &serde_json::Value, t: &Planned) -> Result<Option<crate::remote::Host>, String> {
    let for_mac = t.kind == Kind::Dmg || t.platform.is_some_and(|p| p.os == "macos");
    if !for_mac || crate::platform::host()?.os == "macos" {
        return Ok(None);
    }
    Ok(crate::remote::configured(app_of(&crate::dev::locate(meta, name)?)?.1.build.macos.host.as_deref()))
}

/// Build `t` of app `name` (its bundle once: `made` keeps what is built;
/// a macOS one away from a Mac on its Mac, smoke-tested there with
/// `smoke`: [`remote_mac`]); how to install what it made, and the files.
fn target(
    name: &str,
    meta: &serde_json::Value,
    t: &Planned,
    made: &mut Vec<Built>,
    smoke: bool,
) -> Result<(String, Vec<PathBuf>), String> {
    let host = crate::platform::host()?;
    if let Some(mac) = remote_mac(name, meta, t)? {
        let root = meta["workspace_root"].as_str().ok_or("cargo metadata: no workspace_root")?;
        let dist = Path::new(root).join("dist");
        let (how, files) = crate::remote::build(&mac, meta, name, &t.label, t.platform.map(|p| p.name), &dist, smoke)?;
        let mut lines: Vec<String> = files.iter().map(|f| format!("{} ({:.1} MB)", f.display(), size(f))).collect();
        lines.extend(how);
        return Ok((lines.join("\n"), files));
    }
    let p = match t.kind {
        Kind::Android => {
            #[cfg(unix)]
            return android(&t.bin);
            #[cfg(not(unix))]
            return Err("android is built on Linux (the NDK's Linux toolchain)".into());
        }
        Kind::Windows => t.platform.unwrap_or(windows()),
        Kind::Deb | Kind::AppImage if host.os != "linux" => {
            return Err(format!("{} is a Linux package: build it on Linux", t.kind.name()));
        }
        Kind::Dmg if host.os != "macos" => {
            return Err(format!(
                "a .dmg is made by hdiutil, which runs on a Mac: build it there, or name one ([build.macos] host, or {}=[user@]mac)",
                crate::remote::HOST_ENV
            ));
        }
        _ => t.platform.unwrap_or(host),
    };
    let of = |b: &Built| b.platform == p && b.binary.file_stem().is_some_and(|s| *s == *t.bin);
    let b = match made.iter().position(of) {
        Some(i) => &made[i],
        None => {
            made.push(compile(&t.bin, Some(p), None)?);
            &made[made.len() - 1]
        }
    };
    match t.kind {
        Kind::Deb | Kind::AppImage => package(t.kind, b),
        Kind::Dmg => dmg(b),
        Kind::Cli => cli(b),
        _ => {
            let bundle = pack(b)?;
            let mut files = vec![bundle.clone()];
            if b.platform.os == "windows" {
                files.insert(0, bundle.with_extension("exe"));
            }
            let sizes: Vec<String> = files.iter().map(|f| format!("{} ({:.1} MB)", f.display(), size(f))).collect();
            Ok((
                format!("{}\n{}", sizes.join("\n"), crate::bundle::how_to_install(&b.app, b.platform.os, &bundle)),
                files,
            ))
        }
    }
}

fn size(p: &Path) -> f64 {
    std::fs::metadata(p).map(|m| m.len() as f64 / 1e6).unwrap_or(0.0)
}

/// `--android`: the APKs, one per CPU type.
#[cfg(unix)]
pub(crate) fn android(name: &str) -> Result<(String, Vec<PathBuf>), String> {
    let meta = crate::dev::metadata()?;
    let pkg = crate::dev::locate(&meta, name)?;
    let (app, cfg) = app_of(&pkg)?;
    let is_lib = |t: &&serde_json::Value| t["kind"].as_array().is_some_and(|k| k.iter().any(|k| k == "lib"));
    let lib = meta["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["name"] == pkg.name.as_str())
        .flat_map(|p| p["targets"].as_array().into_iter().flatten())
        .find(is_lib)
        .and_then(|t| t["name"].as_str())
        .map(|n| n.replace('-', "_"))
        .ok_or_else(|| format!("{}: no lib target (src/lib.rs calling aio_shell::android_app!)", pkg.name))?;
    let target = meta["target_directory"].as_str().ok_or("cargo metadata: no target_directory")?;
    let root = meta["workspace_root"].as_str().ok_or("cargo metadata: no workspace_root")?;
    eprintln!("ax build: {} {} for Android (release)…", app.name, app.version);
    if let Some(did) = crate::css::step(&cfg, &app_dir(&pkg)?)? {
        eprintln!("ax build: {did}");
    }
    let dist = Path::new(root).join("dist");
    let apks = crate::android::build(&app, &cfg, &app_dir(&pkg)?, &pkg.name, &lib, Path::new(target), &dist)?;
    let how = |p: &Path| match p.extension().and_then(|e| e.to_str()) {
        Some("aab") => "upload it to Play".to_string(),
        _ => format!("ax install {name} --android"),
    };
    let lines: Vec<String> = apks.iter().map(|p| format!("{} ({:.1} MB): {}", p.display(), size(p), how(p))).collect();
    Ok((lines.join("\n"), apks))
}

/// The disk image of macOS build `b`, with how to install it.
fn dmg(b: &Built) -> Result<(String, Vec<PathBuf>), String> {
    let dist = &b.dist;
    let staging = dist.join(format!(".{}-dmg", b.app.id));
    _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let out = dist.join(format!("{}.dmg", bundle_name(&b.app, b.platform)));
    let made = crate::bundle::mac_app(&b.app, &b.cfg.mac_bundle_id(), &b.binary, b.runtime.as_deref(), &staging)
        .and_then(|app| crate::bundle::mac_dmg(&b.app, &app, &out));
    _ = std::fs::remove_dir_all(&staging);
    made?;
    let how = crate::bundle::how_to_install(&b.app, "macos", &out);
    Ok((format!("{} ({:.1} MB)\n{how}", out.display(), size(&out)), vec![out]))
}

/// A system package of Linux bundle `b`, with how to install it.
#[cfg(unix)]
fn package(kind: Kind, b: &Built) -> Result<(String, Vec<PathBuf>), String> {
    if kind == Kind::Deb {
        let p = crate::package::deb(b)?;
        let file = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
        return Ok((format!("{} ({:.1} MB): sudo apt install ./{file}", p.display(), size(&p)), vec![p]));
    }
    let p = crate::package::appimage(b)?;
    Ok((format!("{} ({:.1} MB): run it as is", p.display(), size(&p)), vec![p]))
}

#[cfg(not(unix))]
fn package(kind: Kind, _: &Built) -> Result<(String, Vec<PathBuf>), String> {
    Err(format!("{} is built on Linux", kind.name()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_platform_bundles_electrons_build_for_it_and_says_what_went_short() {
        let slug = |n: &str| electron_slug(crate::platform::ALL.iter().find(|p| p.name == n).unwrap());
        assert_eq!(
            ["linux", "linux-arm64", "windows", "windows-arm64", "macos", "macos-arm64"].map(slug),
            ["linux-x64", "linux-arm64", "win32-x64", "win32-arm64", "darwin-x64", "darwin-arm64"]
        );
        let gone = ["locales/de.pak", "a/fr.lproj", "a/b/ja.lproj", "dxil.dll"].map(String::from);
        assert_eq!(trimmed(&gone), "3 locales, dxil.dll");
        assert_eq!(trimmed(&[]), "nothing");
    }

    #[cfg(unix)]
    #[test]
    fn what_was_built_keeps_its_bundled_runtime_when_packed_again() {
        let d = tempfile::tempdir().unwrap();
        let p = crate::platform::host().unwrap();
        let cfg = |t: &str| aio::config::Config::parse(&format!("[app]\nid = \"notes\"\n{t}")).unwrap();
        assert_eq!(kept_runtime(&cfg(""), d.path(), p), None, "a shared runtime is fetched at the first start");
        let bundled = cfg("[build]\nruntime = \"bundled\"");
        let rt = kept_runtime(&bundled, d.path(), p).unwrap();
        assert_eq!(rt, d.path().join(format!("aio/notes-electron-{}", aio::paths::electron_slug())));
        let app = App { id: "notes".into(), name: "Notes".into(), version: "1.0.0".into(), icon: None };
        let b = Built {
            app,
            cfg: bundled,
            binary: d.path().join("notes"),
            dist: d.path().into(),
            platform: p,
            runtime: Some(rt.clone()),
        };
        let pkg = d.path().join("pkg");
        assert!(put_runtime(&b, &pkg).unwrap_err().contains("is gone"), "never packed without it");
        std::fs::create_dir_all(rt.join("locales")).unwrap();
        std::fs::write(rt.join("locales/en-US.pak"), "pak").unwrap();
        std::fs::create_dir_all(&pkg).unwrap();
        put_runtime(&b, &pkg).unwrap();
        assert!(pkg.join(aio_shell::runtime::BUNDLED).join("locales/en-US.pak").is_file());
    }

    fn notes(icon: bool) -> App {
        App {
            id: "notes".into(),
            name: "Notes".into(),
            version: "1.2.0".into(),
            icon: icon.then(|| PathBuf::from("icon.png")),
        }
    }

    #[test]
    fn the_menu_entry_names_the_window_class() {
        let e = desktop_entry(&notes(true), "/h/.local/bin/notes");
        assert!(e.contains("Exec=\"/h/.local/bin/notes\" %U\n") && e.contains("StartupWMClass=notes\n"), "{e}");
        assert!(e.contains("Icon=notes\n"));
        assert!(!desktop_entry(&notes(false), "x").contains("Icon="));
        let arm = crate::platform::of("linux", "aarch64").unwrap();
        assert_eq!(bundle_name(&notes(false), arm), "notes-1.2.0-linux-arm64");
        assert_eq!(bundle_name(&notes(false), windows()), "notes-1.2.0-windows-x64", "the one-click download's name");
        assert_eq!(bundle_file(&notes(false), windows()), "notes-1.2.0-windows-x64.zip");
        assert_eq!(bundle_file(&notes(false), arm), "notes-1.2.0-linux-arm64.tar.gz");
        let mac = crate::platform::of("macos", "aarch64").unwrap();
        assert_eq!(bundle_file(&notes(false), mac), "notes-1.2.0-macos-arm64.zip");
    }

    #[test]
    fn identical_functions_are_folded_only_where_the_linker_is_lld() {
        let lld = ["-Clink-arg=-Wl,--icf=all", "-Clink-arg=-Wl,-O2"];
        assert_eq!(link_args("x86_64-unknown-linux-gnu", false), lld);
        assert_eq!(link_args("x86_64-unknown-linux-gnu", true)[..2], lld);
        assert_eq!(link_args("x86_64-unknown-linux-gnu", true)[2], "-Clink-arg=-Wl,-z,pack-relative-relocs");
        for t in ["aarch64-unknown-linux-gnu", "x86_64-pc-windows-gnu", "aarch64-apple-darwin"] {
            assert!(link_args(t, true).is_empty(), "{t}");
        }
    }

    #[test]
    fn a_release_build_ships_no_build_machine_path() {
        let (home, cargo, root) = (Path::new("/u/ann"), Path::new("/u/ann/.cargo"), Path::new("/u/ann/src/app"));
        assert_eq!(
            remaps(Some(home), Some(cargo), root),
            [
                "--remap-path-prefix=/u/ann=~",
                "--remap-path-prefix=/u/ann/.cargo=/cargo",
                "--remap-path-prefix=/u/ann/src/app=/build"
            ],
            "the narrower last: rustc takes the last that matches"
        );
        assert_eq!(
            remaps(Some(Path::new("/")), None, root),
            ["--remap-path-prefix=/u/ann/src/app=/build"],
            "never all of /"
        );
        let mut c = Command::new("cargo");
        remap(&mut c, root);
        let said = format!("{:?} {:?}", c.get_args().collect::<Vec<_>>(), c.get_envs().collect::<Vec<_>>());
        assert!(said.contains("=/build"), "{said}");
        // The build's own check: a binary that still holds the home is refused.
        let home = std::env::home_dir().unwrap().display().to_string();
        let bin = [b"\x00panicked at ".as_slice(), home.as_bytes(), b"/.cargo/registry/src/x.rs\x00"].concat();
        assert!(no_home_paths(&bin, Path::new("app")).unwrap_err().contains("1×"));
        assert_eq!(no_home_paths(b"\x00/cargo/registry/src/x.rs ~/src/a.rs\x00", Path::new("app")), Ok(()));
    }

    #[test]
    fn relocations_are_packed_only_for_a_glibc_that_loads_them() {
        assert!(relr_loads("glibc 2.39\n") && relr_loads("glibc 2.36") && relr_loads("glibc 3.0"));
        for old in ["glibc 2.35", "glibc 2.17", "", "musl", "glibc x.y"] {
            assert!(!relr_loads(old), "{old}");
        }
    }

    #[test]
    fn an_app_over_its_size_budget_is_not_packed() {
        assert_eq!(over_budget(10_000_000, BUDGET_MB), None);
        assert_eq!(over_budget(12_340_000, BUDGET_MB).as_deref(), Some("12.3 MB is over the 10 MB budget"));
        assert_eq!(over_budget(12_340_000, 25), None, "a declared budget");
        assert_eq!(over_budget(25_000_001, 25).as_deref(), Some("25.0 MB is over the 25 MB budget"));
        let toml =
            |t: &str| aio::config::Config::parse(&format!("[app]\nid = \"relay\"\nname = \"Relay\"\n{t}")).unwrap();
        assert_eq!(budget_of(&toml("")), BUDGET_MB);
        let server = "mode = \"server\"\n[server]\nlisten = \"127.0.0.1:8080\"\n";
        assert_eq!(budget_of(&toml(server)), SERVER_BUDGET_MB, "a server: 6 MB");
        assert_eq!(budget_of(&toml(&format!("{server}[build]\nbudget_mb = 8\n"))), 8, "unless it says");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn install_then_uninstall_for_this_user_only() {
        let d = tempfile::tempdir().unwrap();
        let (home, src) = (d.path().join("home dir"), d.path().join("src"));
        std::fs::create_dir_all(&src).unwrap();
        let fake_bin = src.join("notes-built");
        std::fs::write(&fake_bin, "#!/bin/sh\necho hi\n").unwrap();
        std::fs::write(src.join("icon.png"), b"png").unwrap();
        let app = App { icon: Some(src.join("icon.png")), ..notes(true) };
        let bundle = bundle_name(&app, crate::platform::host().unwrap());
        let dir = stage(&app, &bundle, &fake_bin, &d.path().join("stage"), true).unwrap();
        // An offline build's runtime beside the binary goes along.
        std::fs::create_dir_all(dir.join("electron/locales")).unwrap();
        std::fs::write(dir.join("electron/locales/en-US.pak"), "us").unwrap();
        let run = |dir: &Path, script: &str, args: &[&str]| {
            let out = Command::new("sh")
                .arg(dir.join(script))
                .args(args)
                .env("HOME", &home)
                .env_remove("XDG_BIN_HOME")
                .env_remove("XDG_DATA_HOME")
                .env_remove("XDG_CACHE_HOME")
                .output()
                .unwrap();
            assert!(out.status.success(), "{script}: {}", String::from_utf8_lossy(&out.stderr));
        };
        let sh = |script: &str, args: &[&str]| run(&dir, script, args);
        sh("install.sh", &[]);
        let (bin, data) = (home.join(".local/bin/notes"), home.join(".local/share"));
        let install = data.join("aio-rt/install/notes");
        // a versioned dir, `current` → it, the command through `current`.
        assert_eq!(std::fs::read_link(install.join("current")).unwrap(), Path::new("versions/1.2.0"));
        assert_eq!(std::fs::read_link(&bin).unwrap(), install.join("current/notes"));
        assert_eq!(Command::new(&bin).output().unwrap().stdout, b"hi\n");
        assert!(install.join("versions/1.2.0/electron/locales/en-US.pak").exists());
        let rec: aio::serde_json::Value =
            aio::serde_json::from_slice(&std::fs::read(install.join("installed.json")).unwrap()).unwrap();
        assert_eq!(
            (rec["version"].as_str(), rec["source"].as_str()),
            (Some("1.2.0"), Some(format!("{bundle}.tar.gz")).as_deref())
        );
        let entry = std::fs::read_to_string(data.join("applications/notes.desktop")).unwrap();
        assert!(entry.contains(&format!("Exec=\"{}\"", bin.display())), "{entry}");
        assert!(data.join("icons/hicolor/256x256/apps/notes.png").exists());
        // The user's data is not the installer's to remove…
        let kept = data.join("aio-rt/apps/notes/data");
        std::fs::create_dir_all(&kept).unwrap();
        sh("uninstall.sh", &[]);
        assert!(!bin.exists() && !install.exists() && !data.join("applications/notes.desktop").exists());
        assert!(!data.join("icons/hicolor/256x256/apps/notes.png").exists());
        assert!(kept.exists());
        // …unless they ask: `--data`.
        sh("install.sh", &[]);
        sh("uninstall.sh", &["--data"]);
        assert!(!kept.exists() && !install.exists());
        // A CLI: on PATH through `current` as well, and nowhere in the menu.
        let cli = stage(&app, &format!("{bundle}-cli"), &fake_bin, &d.path().join("cli"), false).unwrap();
        assert!(!cli.join("notes.png").exists());
        run(&cli, "install.sh", &[]);
        assert_eq!(Command::new(&bin).output().unwrap().stdout, b"hi\n");
        assert!(
            !data.join("applications/notes.desktop").exists()
                && !data.join("icons/hicolor/256x256/apps/notes.png").exists()
        );
        run(&cli, "uninstall.sh", &[]);
        assert!(!bin.exists() && !install.exists());
    }

    #[test]
    fn a_cli_is_installed_on_path_and_named_apart_from_the_desktop_bundle() {
        let s = install_sh(&notes(true), "notes-1.2.0-linux-x64-cli", false);
        assert!(!s.contains(".desktop") && !s.contains("icons") && s.contains("is installed: $bin/notes.\""), "{s}");
        assert!(s.contains("\"source\":\"notes-1.2.0-linux-x64-cli.tar.gz\""), "{s}");
        assert!(install_sh(&notes(true), "b", true).contains("and in the app menu"));
        assert_eq!(cli_bundle_name(&notes(false), windows()), "notes-1.2.0-windows-x64-cli");
        assert!(!s.contains("mv -T"), "POSIX sh: macOS installs it too");
    }
}

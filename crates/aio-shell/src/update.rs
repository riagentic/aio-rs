//! Updates. New versions are staged in `aio-rt/install/<id>/
//! versions/<v>/` (3 kept). On Linux an app installed from its bundle runs
//! from there: `current` → the one that runs, `installed.json` (version,
//! where it came from); `~/.local/bin/<id>` points through `current`. On
//! macOS the app is its `.app` (wherever it was put) and on Windows its
//! `.exe` in `%LOCALAPPDATA%\Programs\<id>`, and an app run from an
//! AppImage is that file ([`appimage`]): an update is copied into that
//! place by [`crate::swap`].
//!
//! A running app looks for a release every `[updates] every`; one that is
//! signed by a trusted key (`aio::update`), newer, not refused before and able
//! to read the data here is downloaded, checked and staged. It installs the
//! next time the app starts: the data is backed up, then an outside helper
//! switches `current` and starts the new version, which must open its window
//! within [`FIRST_BOOT`] and claim the first-boot token (one that exits first
//! is started once more). If it doesn't, the helper stops it and starts the
//! old version again, which puts the backup back (the data the new one wrote
//! is kept aside) and never offers that release again. A swap that could not
//! be made is tried again at the next start, three times at most. Some
//! version always ends up running.
//!
//! With `[updates] auto = false` a release is only offered. The app sees it
//! in its `updates` cell ([`cell`], `aio::update::Updates`), and its `apply`
//! installs now: the release is staged, this app stops and starts again with
//! the same arguments ([`RELAUNCH`]), and that start installs it as above
//! (the data backed up while nothing writes it).
use aio::sha256::Sha256;
use aio::update::{self as up, Data, Found, Manifest, Offer, Ready, Step, Updates, UpdatesCalls};
use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

/// How long a new version has to open its window.
pub const FIRST_BOOT: Duration = Duration::from_secs(120);
/// Installed versions kept by default (`[updates] keep`; the current one
/// and the one before it always).
pub const KEEP: usize = 3;
/// In a staged version: how its install treats the data (`accept`: over
/// the gate's blockers; `retire`: on fresh data). Absent: gated.
const DATA_MARK: &str = "data-override";
/// Set on a start that must wait for the process it names to be gone (the
/// app restarting to install an update).
pub const RELAUNCH: &str = "AIORT_RELAUNCH_AFTER";
/// `1`: a supervisor (the systemd unit, launchd agent or Windows task that
/// `ax build` writes for a server) starts this app again whenever it exits.
/// An update then starts no process of its own: one the supervisor doesn't
/// know is killed with the service, or fights its restart for the lock.
pub const SUPERVISED: &str = "AIO_SUPERVISED";
/// Beside `first-boot`: the supervised new version started once already.
const TRIED: &str = "first-boot.tried";

/// A server or CLI that a supervisor runs ([`SUPERVISED`]).
fn supervised(cfg: &aio::config::Config) -> bool {
    matches!(cfg.app.mode, aio::config::Mode::Server | aio::config::Mode::Cli)
        && std::env::var_os(SUPERVISED).is_some_and(|v| v == "1")
}

/// What the update before this start did, for the cell: the release that
/// was rolled back, the backup the one now running took.
static ROLLED_BACK: OnceLock<String> = OnceLock::new();
static BACKUP: OnceLock<String> = OnceLock::new();

/// `aio-rt/install/<id>`; an app run from an AppImage keeps its updates
/// apart, in `<id>-appimage` (the same app installed from its bundle may be
/// there too).
pub fn dir(id: &str) -> PathBuf {
    let name = match appimage() {
        Some(_) => format!("{id}-appimage"),
        None => id.to_string(),
    };
    aio::paths::tool_home().join("install").join(name)
}

/// The AppImage that `exe` runs from: `appimage` (`$APPIMAGE`) when `exe`
/// is inside the mount `appdir` (`$APPDIR`) names. A program started from
/// an AppImage inherits both and is none. Pure.
pub fn appimage_of(appimage: Option<&str>, appdir: Option<&str>, exe: &Path) -> Option<PathBuf> {
    let (file, mount) = (appimage.filter(|a| !a.is_empty())?, appdir.filter(|a| !a.is_empty())?);
    exe.starts_with(mount).then(|| PathBuf::from(file))
}

/// Linux: the AppImage this app runs from, if it does. An update replaces
/// that file ([`crate::swap`], as on macOS and Windows), and a release for
/// it is `<os>-<arch>.appimage.json` (`ax release --appimage`).
pub fn appimage() -> Option<PathBuf> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let (file, mount) = (std::env::var("APPIMAGE").ok(), std::env::var("APPDIR").ok());
    appimage_of(file.as_deref(), mount.as_deref(), &std::env::current_exe().ok()?)
}

/// What this app's releases are named: `<os>-<arch>`, and `.appimage`
/// after it for an AppImage.
fn manifest_name() -> String {
    match appimage() {
        Some(_) => format!("{}.appimage", platform()),
        None => platform(),
    }
}

/// This process runs from an installed version (not a dev build)?
#[cfg(target_os = "linux")]
pub fn installed(id: &str) -> bool {
    if appimage().is_some() {
        return true;
    }
    let (Ok(exe), Ok(versions)) = (std::env::current_exe(), dir(id).join("versions").canonicalize()) else {
        return false;
    };
    exe.canonicalize().is_ok_and(|e| e.starts_with(versions))
}

/// This process runs from an installed version (not a dev build)? An
/// Android app always runs from its installed APK.
#[cfg(not(target_os = "linux"))]
pub fn installed(id: &str) -> bool {
    cfg!(target_os = "android") || place(id).is_some()
}

/// Where the running app is installed: (its place, the binary to start in
/// it). macOS: its `.app`; Windows: its `.exe` in `Programs\<id>`.
#[cfg(not(target_os = "linux"))]
fn place(id: &str) -> Option<(PathBuf, PathBuf)> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    if cfg!(target_os = "macos") {
        let bundle = exe.ancestors().find(|a| a.extension().is_some_and(|e| e == "app"))?;
        return Some((bundle.to_path_buf(), exe.clone()));
    }
    (exe.parent()? == crate::instance::programs(id).canonicalize().ok()?).then(|| (exe.clone(), exe))
}

/// The staged version `v`: (what goes in place, its binary).
#[cfg(not(target_os = "linux"))]
fn staged_version(d: &Path, v: &str, bin: &str) -> Option<(PathBuf, PathBuf)> {
    let vd = d.join("versions").join(v);
    if cfg!(target_os = "macos") {
        let app = std::fs::read_dir(&vd)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e == "app"))?;
        let exe = app.join("Contents/MacOS").join(bin);
        return Some((app, exe));
    }
    let exe = vd.join(format!("{bin}.exe"));
    Some((exe.clone(), exe))
}

/// `<os>-<arch>`, as manifests are named.
pub fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

fn read_json<T: serde::de::DeserializeOwned>(p: &Path) -> Option<T> {
    aio::serde_json::from_slice(&std::fs::read(p).ok()?).ok()
}

fn write_json(p: &Path, v: &aio::serde_json::Value) -> io::Result<()> {
    aio::paths::write_atomic(p, format!("{v}\n").as_bytes(), false)
}

/// Versions the user's machine refused (a failed first boot): never again.
fn dismissed(dir: &Path) -> Vec<String> {
    read_json(&dir.join("dismissed.json")).unwrap_or_default()
}

fn dismiss(dir: &Path, version: &str) -> io::Result<()> {
    let mut all = dismissed(dir);
    if !all.iter().any(|v| v == version) {
        all.push(version.into());
    }
    write_json(&dir.join("dismissed.json"), &aio::serde_json::json!(all))
}

/// Swaps of one release that could not be made before it is dismissed.
const MAX_FAILED_SWAPS: u64 = 3;

/// The staged release newest of those newer than `current`, if any.
fn staged(dir: &Path, current: &str) -> Option<Manifest> {
    let gone = dismissed(dir);
    std::fs::read_dir(dir.join("versions"))
        .ok()?
        .flatten()
        .filter_map(|e| read_json::<Manifest>(&e.path().join("release.json")))
        .filter(|m| up::compare(&m.release.version, current) == Some(std::cmp::Ordering::Greater))
        .filter(|m| !gone.contains(&m.release.version))
        .max_by(|a, b| up::compare(&a.release.version, &b.release.version).unwrap_or(std::cmp::Ordering::Equal))
}

/// The swap was never made: the new version never ran, and what held the
/// place (a file in use) is usually gone soon. The data set aside for a
/// fresh start comes back, and the release stays staged for the next start
/// unless that was its last try (never this start: that would restart the
/// app in a loop). What happened, said.
fn swap_unmade(d: &Path, pending: Option<&aio::serde_json::Value>, data: &Path, version: &str) -> io::Result<String> {
    let to = pending.and_then(|p| p["to"].as_str()).unwrap_or_default().to_string();
    let mut why = String::new();
    if let Some(a) = pending.and_then(|p| p["retired"].as_str()).map(PathBuf::from)
        && let Err(e) = retire(&a, data, &[]).and_then(|()| std::fs::remove_dir(&a))
    {
        why = format!("; the data set aside for it could not be put back: {e} (it is in {})", a.display());
    }
    // Counted per release: another release starts again at one.
    let n = read_json::<aio::serde_json::Value>(&d.join("swaps.json"))
        .filter(|s| s["to"] == to.as_str())
        .map_or(0, |s| s["count"].as_u64().unwrap_or(0))
        + 1;
    let head = if n < MAX_FAILED_SWAPS {
        write_json(&d.join("swaps.json"), &aio::serde_json::json!({ "to": to, "count": n }))?;
        format!(
            "{to} could not be put in place, so {version} runs; it is tried again at the next start ({n} of {MAX_FAILED_SWAPS})"
        )
    } else {
        _ = std::fs::remove_file(d.join("swaps.json"));
        if up::parse_version(&to).is_some() && to != version {
            dismiss(d, &to)?;
            _ = std::fs::remove_dir_all(d.join("versions").join(&to));
        }
        format!(
            "{to} could not be put in place {n} times, so it is given up; close what holds the app's files, then install it by hand"
        )
    };
    _ = std::fs::remove_file(d.join(crate::swap::SWAP_FAILED));
    _ = std::fs::remove_file(d.join("pending.json"));
    Ok(head + &why)
}

/// What `start` does about updates before the app boots.
pub enum Next {
    /// Boot as usual.
    Boot,
    /// The helper is installing a release: exit now.
    Exit,
}

/// At start (the lock held, the log running): a failed update is undone, a
/// staged one installed.
pub fn on_start(cfg: &aio::config::Config, version: &str, dirs: &aio::paths::AppDirs) -> io::Result<Next> {
    let id = &cfg.app.id;
    let d = dir(id);
    let pending = read_json(&d.join("pending.json"));
    if d.join(crate::swap::SWAP_FAILED).exists() {
        let why = swap_unmade(&d, pending.as_ref(), &dirs.data, version)?;
        eprintln!("[aio] update failed: {why}");
        aio::report::problem("update:failed", &why);
        return Ok(Next::Boot);
    }
    // The helper gave up on the new version: back to our data, never again.
    if d.join("update-failed").exists() {
        let to = pending.as_ref().and_then(|p| p["to"].as_str().map(String::from)).unwrap_or_default();
        let backup = pending.as_ref().and_then(|p| p["backup"].as_str().map(PathBuf::from));
        let retired = pending.as_ref().and_then(|p| p["retired"].as_str().map(PathBuf::from));
        let restored = match (retired, backup) {
            // Started fresh: its data goes aside, the data from before comes back whole.
            (Some(a), _) => {
                let aside = a.with_file_name(format!("{}-{to}-failed", a.file_name().unwrap_or_default().display()));
                retire(&dirs.data, &aside, &kept(dirs))
                    .and_then(|()| retire(&a, &dirs.data, &[]))
                    .and_then(|()| std::fs::remove_dir(&a))
                    .map(|()| format!("; the data from before it is back, its own kept at {}", aside.display()))
                    .unwrap_or_else(|e| {
                        format!("; the data from before it could not be put back: {e} (it is in {})", a.display())
                    })
            }
            (None, Some(b)) => aio::backup::restore(id, &b, &dirs.store, dirs)
                .map(|aside| match aside {
                    Some(a) => format!("; the data from before it is back, its own kept at {}", a.display()),
                    None => "; the data from before it is back".into(),
                })
                .unwrap_or_else(|e| format!("; the backup could not be restored: {e}")),
            (None, None) => String::new(),
        };
        let why = format!("{to} did not start, so {version} runs again{restored}");
        eprintln!("[aio] update failed: {why}");
        aio::report::problem("update:failed", &why);
        if up::parse_version(&to).is_some() && to != version {
            _ = ROLLED_BACK.set(to.clone());
            dismiss(&d, &to)?;
            _ = std::fs::remove_dir_all(d.join("versions").join(&to));
        }
        _ = std::fs::remove_file(d.join("update-failed"));
        _ = std::fs::remove_file(d.join(crate::swap::BACK));
        _ = std::fs::remove_file(d.join(TRIED));
        _ = std::fs::remove_file(d.join("pending.json"));
        return Ok(Next::Boot);
    }
    // The new version's first boot: it confirms once its window is up.
    if let Some(p) = pending.as_ref().filter(|p| p["to"] == version && d.join("first-boot").exists()) {
        if let Some(b) = p["backup"].as_str() {
            _ = BACKUP.set(b.into());
        }
        eprintln!("[aio] first start of {version} after an update: it is kept once the window opens");
        return Ok(Next::Boot);
    }
    if !installed(id) {
        return Ok(Next::Boot);
    }
    let Some(m) = staged(&d, version) else { return Ok(Next::Boot) };
    let to = m.release.version.clone();
    // Checked again now: the data may have moved on since the download.
    let store = dirs.store.clone();
    let local = up::local_data(&store).map_err(io::Error::other)?;
    let blocked = local.map(|l| up::blockers(&m.release.data, &l)).unwrap_or_default();
    let how = std::fs::read_to_string(d.join("versions").join(&to).join(DATA_MARK)).unwrap_or_default();
    match (blocked.is_empty(), how.as_str()) {
        (false, "accept") => eprintln!(
            "[aio] update {to} is installed OVER the data gate, as asked: {}; the data is backed up first",
            blocked.join("; ")
        ),
        (false, "retire") => {
            eprintln!("[aio] update {to} can't read the data here ({}): it starts fresh, as asked", blocked.join("; "))
        }
        (false, _) => {
            eprintln!("[aio] update {to} can't read the data here: {}; not installed", blocked.join("; "));
            dismiss(&d, &to)?;
            _ = std::fs::remove_dir_all(d.join("versions").join(&to));
            return Ok(Next::Boot);
        }
        _ => {}
    }
    // Start fresh: the data moves aside whole (`archive/` beside it), never deleted.
    let retired = if how == "retire" {
        let stamp = aio::log::stamp(now_ms()).replace([':', '.'], "-");
        let a = dirs.data.parent().unwrap_or(&dirs.data).join("archive").join(format!("{id}-{version}-{stamp}"));
        retire(&dirs.data, &a, &kept(dirs))?;
        eprintln!("[aio] the data is set aside for {to}'s fresh start: {}", a.display());
        Some(a)
    } else {
        None
    };
    let backup = if store.exists() {
        let b = dirs
            .data
            .join("backups")
            .join(format!("pre-{to}-{}.db", aio::log::stamp(now_ms()).replace([':', '.'], "-")));
        aio::paths::private_dir(&dirs.data.join("backups"))?;
        aio::backup::backup(id, &store, &b)?;
        prune_backups(&dirs.data.join("backups"));
        eprintln!("[aio] data backed up before the update: {}", b.display());
        Some(b)
    } else {
        None
    };
    let keep = cfg.updates.keep;
    write_json(
        &d.join("pending.json"),
        &aio::serde_json::json!({ "from": version, "to": to, "backup": backup, "retired": retired, "keep": keep }),
    )?;
    aio::paths::write_atomic(&d.join("first-boot"), to.as_bytes(), false)?;
    eprintln!("[aio] installing {to} (from {version})");
    // The new version starts as this one was: same arguments.
    let args: Vec<String> = std::env::args().skip(1).collect();
    spawn_helper(&d, id, version, &to, &args, &dirs.logs.join("update.log"), supervised(cfg))?;
    Ok(Next::Exit)
}

/// This start (a window's: the helper waits on it) is the first of
/// `version` of app `id` after an update, and the version it replaced is
/// still there: the runtime's failure question may offer that one
/// ([`go_back`]).
pub fn can_go_back(id: &str, version: &str) -> bool {
    let d = dir(id);
    d.join("first-boot").exists()
        && read_json::<aio::serde_json::Value>(&d.join("pending.json"))
            .is_some_and(|p| p["to"] == version && p["from"].as_str().is_some_and(|f| !f.is_empty()))
}

/// Give this first boot up: the helper waiting on it puts the version
/// before back and starts it, as for a version that never booted.
pub fn go_back(id: &str) -> io::Result<()> {
    std::fs::write(dir(id).join(crate::swap::BACK), b"")
}

/// What stays in the data dir when it starts fresh: the lock, the release
/// keys and the `updates` cell (the app's updates, not the user's data), and
/// the logs and cache dirs where they live inside it.
fn kept(dirs: &aio::paths::AppDirs) -> Vec<String> {
    let inside =
        |p: &Path| (p.parent() == Some(&dirs.data)).then(|| p.file_name()?.to_str().map(String::from)).flatten();
    ["instance.lock", "update-keys.json", "updates.db", "updates.db-wal", "updates.db-shm"]
        .map(String::from)
        .into_iter()
        .chain(inside(&dirs.logs))
        .chain(inside(&dirs.cache))
        .collect()
}

/// Move everything in `from` but `keep` into `to` (made), one rename each;
/// a failure moves back what had moved, so `from` is as it was.
fn retire(from: &Path, to: &Path, keep: &[String]) -> io::Result<()> {
    std::fs::create_dir_all(to)?;
    let mut moved = vec![];
    let r = (|| {
        for e in std::fs::read_dir(from)? {
            let name = e?.file_name();
            if keep.iter().any(|k| name == k.as_str()) {
                continue;
            }
            std::fs::rename(from.join(&name), to.join(&name))?;
            moved.push(name);
        }
        Ok(())
    })();
    if r.is_err() {
        for n in moved.iter().rev() {
            _ = std::fs::rename(to.join(n), from.join(n));
        }
    }
    r
}

/// The newest 3 pre-update backups stay.
fn prune_backups(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut pre: Vec<_> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("pre-")))
        .collect();
    pre.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
    for p in &pre[..pre.len().saturating_sub(3)] {
        _ = std::fs::remove_file(p);
    }
}

/// The swap helper: plain `sh`, every value an argument. It waits for
/// the old app to exit, switches `current`, starts the new version (with the
/// app's arguments, the ones after the sixth) and waits for it to claim
/// `first-boot`, starting it once more if it exits first ([`crate::swap::BOOTS`]);
/// else it claims it (`update-failed`), stops the new version, switches back
/// and starts the old one. A switch that fails leaves `swap-failed`.
#[cfg(target_os = "linux")]
const HELPER: &str = r#"
parent=$1; dir=$2; new=$3; old=$4; bin=$5; wait=$6
shift 6
while kill -0 "$parent" 2>/dev/null; do sleep 0.2; done
flip() { ln -sfn "versions/$1" "$dir/current.next" && mv -T "$dir/current.next" "$dir/current"; }
if ! flip "$new"; then
  echo "ax update: could not switch to $new" >&2
  mv "$dir/first-boot" "$dir/swap-failed" 2>/dev/null
  exec "$dir/current/$bin" "$@" >/dev/null 2>&1
fi
i=0
boot=0
while [ "$boot" -lt 2 ]; do
  boot=$((boot+1))
  "$dir/current/$bin" "$@" >/dev/null 2>&1 &
  pid=$!
  while [ -e "$dir/first-boot" ] && [ "$i" -lt "$wait" ] && kill -0 "$pid" 2>/dev/null; do sleep 0.2; i=$((i+1)); done
  [ -e "$dir/first-boot.back" ] && echo "ax update: $new asked for $old" >&2 && break
  { [ -e "$dir/first-boot" ] && [ "$i" -lt "$wait" ] && ! kill -0 "$pid" 2>/dev/null; } || break
  [ "$boot" -lt 2 ] && echo "ax update: $new exited before its first boot; starting it once more" >&2
done
if mv "$dir/first-boot" "$dir/update-failed" 2>/dev/null; then
  echo "ax update: $new did not start; back to $old" >&2
  kill -TERM "$pid" 2>/dev/null
  j=0
  while kill -0 "$pid" 2>/dev/null && [ "$j" -lt 50 ]; do sleep 0.2; j=$((j+1)); done
  kill -KILL "$pid" 2>/dev/null
  flip "$old" || echo "ax update: could not switch back to $old" >&2
  exec "$dir/current/$bin" "$@" >/dev/null 2>&1
fi
wait "$pid"
"#;

/// macOS and Windows: the swap helper, run from the staged new version.
/// `by_supervisor`: it swaps and watches the first boot, but the supervisor
/// starts each version.
#[cfg(not(target_os = "linux"))]
fn spawn_helper(
    dir: &Path,
    bin: &str,
    _from: &str,
    to: &str,
    args: &[String],
    log: &Path,
    by_supervisor: bool,
) -> io::Result<()> {
    let place = place(bin).ok_or_else(|| io::Error::other("not installed"))?;
    let staged = staged_version(dir, to, bin).ok_or_else(|| io::Error::other(format!("{to} is not staged whole")))?;
    let (place, staged) = with_runtime(place, staged);
    swap_helper(dir, bin, args, log, by_supervisor, place, staged)
}

/// Windows: an offline build's exe goes in place with the runtime beside
/// it, so the place is the exe's whole folder (`Programs\<id>`); else
/// as they are.
#[cfg_attr(target_os = "linux", allow(dead_code))]
fn with_runtime(
    (live, launch): (PathBuf, PathBuf),
    (new, helper): (PathBuf, PathBuf),
) -> ((PathBuf, PathBuf), (PathBuf, PathBuf)) {
    let carries = |exe: &Path| {
        exe.extension().is_some_and(|e| e == "exe")
            && exe
                .parent()
                .is_some_and(|d| crate::runtime::exe_for("windows", &d.join(crate::runtime::BUNDLED)).exists())
    };
    match (live.parent(), new.parent()) {
        (Some(l), Some(n)) if carries(&new) => ((l.to_path_buf(), launch), (n.to_path_buf(), helper)),
        _ => ((live, launch), (new, helper)),
    }
}

/// Start [`crate::swap`]'s helper from `helper` (the staged new version):
/// it puts `new` in the app's place `live` and starts `launch` there.
fn swap_helper(
    dir: &Path,
    bin: &str,
    args: &[String],
    log: &Path,
    by_supervisor: bool,
    (live, launch): (PathBuf, PathBuf),
    (new, helper): (PathBuf, PathBuf),
) -> io::Result<()> {
    let plan = crate::swap::Plan {
        id: bin.into(),
        parent: std::process::id(),
        live,
        new,
        launch,
        first_boot: dir.join("first-boot"),
        failed: dir.join("update-failed"),
        wait: FIRST_BOOT,
        args: args.to_vec(),
        by_supervisor,
    };
    let err = std::fs::OpenOptions::new().create(true).append(true).open(log)?;
    let mut cmd = crate::swap::command(&helper, &plan)?;
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(err);
    detach(&mut cmd);
    cmd.spawn().map(drop)
}

/// Linux: the swap helper ([`HELPER`]). `by_supervisor`: no helper; `current`
/// is switched now, the supervisor starts the new version once this one
/// exits, and [`supervised_start`] takes it back if that never boots. An
/// AppImage is one file, replaced as on Windows ([`crate::swap`]).
#[cfg(target_os = "linux")]
fn spawn_helper(
    dir: &Path,
    bin: &str,
    from: &str,
    to: &str,
    args: &[String],
    log: &Path,
    by_supervisor: bool,
) -> io::Result<()> {
    if let Some(live) = appimage() {
        let new = staged_binary(&dir.join("versions").join(to), bin)
            .filter(|f| f.is_file())
            .ok_or_else(|| io::Error::other(format!("{to} is not staged whole")))?;
        return swap_helper(dir, bin, args, log, by_supervisor, (live.clone(), live), (new.clone(), new));
    }
    if by_supervisor {
        return flip(dir, to);
    }
    let err = std::fs::OpenOptions::new().create(true).append(true).open(log)?;
    let ticks = (FIRST_BOOT.as_millis() / 200).to_string();
    let parent = std::process::id().to_string();
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", HELPER, "aio-update"])
        .arg(&parent)
        .arg(dir)
        .args([to, from, bin, &ticks])
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(err);
    detach(&mut cmd);
    cmd.spawn().map(drop)
}

/// `current` → `versions/<version>`, in one rename.
#[cfg(target_os = "linux")]
fn flip(dir: &Path, version: &str) -> io::Result<()> {
    let next = dir.join("current.next");
    _ = std::fs::remove_file(&next);
    std::os::unix::fs::symlink(Path::new("versions").join(version), &next)?;
    std::fs::rename(&next, dir.join("current"))
}

/// Before the lock, under a supervisor ([`SUPERVISED`]): a new version
/// started again with its first boot still unclaimed died before it booted,
/// so `current` goes back to the version before it, which then undoes the
/// update as after the helper's rollback. True: exit now, the supervisor
/// starts that one.
#[cfg(target_os = "linux")]
pub fn supervised_start(cfg: &aio::config::Config, version: &str) -> io::Result<bool> {
    if !supervised(cfg) {
        return Ok(false);
    }
    let back = second_start(&dir(&cfg.app.id), version)?;
    if let Some(from) = &back {
        eprintln!("[aio] update: {version} stopped before it booted; back to {from}");
    }
    Ok(back.is_some())
}

/// [`supervised_start`] in `dir`: the version gone back to, if any.
#[cfg(target_os = "linux")]
fn second_start(dir: &Path, version: &str) -> io::Result<Option<String>> {
    let Some(p) = read_json::<aio::serde_json::Value>(&dir.join("pending.json")).filter(|p| p["to"] == version) else {
        return Ok(None);
    };
    let (tried, from) = (dir.join(TRIED), p["from"].as_str().unwrap_or_default().to_string());
    if !dir.join("first-boot").exists() || up::parse_version(&from).is_none() {
        return Ok(None);
    }
    if !tried.exists() {
        std::fs::write(&tried, version)?;
        return Ok(None);
    }
    // Claimed here, as the helper claims it: the new version can't keep it now.
    std::fs::rename(dir.join("first-boot"), dir.join("update-failed"))?;
    _ = std::fs::remove_file(&tried);
    flip(dir, &from)?;
    Ok(Some(from))
}

/// A child that outlives this app: its own session (Unix), no console
/// (Windows), none of its descriptors (`swap::inherit_none`), and no wait
/// for this process ([`RELAUNCH`]).
fn detach(cmd: &mut Command) {
    cmd.env_remove(RELAUNCH);
    crate::swap::inherit_none(cmd);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid in the child before exec.
        unsafe {
            cmd.pre_exec(|| if libc::setsid() < 0 { Err(io::Error::last_os_error()) } else { Ok(()) });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x8;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
}

/// Start this app again once this process is gone: same binary, arguments
/// and folder. That start installs what is staged.
fn relaunch() -> io::Result<()> {
    let mut cmd = Command::new(appimage().map_or_else(std::env::current_exe, Ok)?);
    cmd.args(std::env::args_os().skip(1))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    detach(&mut cmd);
    cmd.env(RELAUNCH, std::process::id().to_string()).spawn().map(drop)
}

/// The window opened: a new version claims its first boot, then the
/// install is final: `installed.json` says so, old versions go.
pub fn confirm(id: &str, version: &str) {
    let d = dir(id);
    let Some(p) = read_json::<aio::serde_json::Value>(&d.join("pending.json")) else { return };
    if p["to"] != version {
        return;
    }
    match std::fs::remove_file(d.join("first-boot")) {
        Ok(()) => {
            _ = std::fs::remove_file(d.join(TRIED));
            _ = std::fs::remove_file(d.join("pending.json"));
            _ = std::fs::remove_file(d.join("swaps.json"));
            let mut rec = read_json(&d.join("installed.json")).unwrap_or_else(|| aio::serde_json::json!({ "id": id }));
            rec["version"] = version.into();
            rec["updatedAt"] = aio::log::stamp(now_ms()).into();
            rec["updatedFrom"] = p["from"].clone();
            _ = write_json(&d.join("installed.json"), &rec);
            let keep = p["keep"].as_u64().map_or(KEEP, |k| k as usize);
            prune(&d, version, p["from"].as_str().unwrap_or(""), keep);
            eprintln!("[aio] update to {version} is in place");
        }
        // The helper gave up first: it stops this version now.
        Err(_) => eprintln!("[aio] update to {version} came too late: the helper is going back"),
    }
}

/// Keep the newest `keep` versions, and always `current` and `previous`.
fn prune(d: &Path, current: &str, previous: &str, keep: usize) {
    let Ok(rd) = std::fs::read_dir(d.join("versions")) else { return };
    let mut all: Vec<String> = rd.flatten().filter_map(|e| e.file_name().to_str().map(String::from)).collect();
    all.retain(|v| up::parse_version(v).is_some());
    all.sort_by(|a, b| up::compare(b, a).unwrap_or(std::cmp::Ordering::Equal));
    for v in all.iter().skip(keep).filter(|v| *v != current && *v != previous) {
        _ = std::fs::remove_dir_all(d.join("versions").join(v));
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// How long a host may send nothing before a download is given up (a
/// slow link is fine; a host that stopped is not).
const STALL: Duration = Duration::from_secs(60);

/// GET `url` (at most `cap` bytes): https, or loopback http for tests.
/// `step` hears the percent of `cap` read, as it grows.
fn fetch(url: &str, cap: u64, step: &dyn Fn(u8)) -> io::Result<Vec<u8>> {
    Ok(get(url, cap, None, STALL, step)?.map(|(b, _)| b).unwrap_or_default())
}

/// [`fetch`], asking only for a body other than `etag`'s: `None` when the
/// host says it is the same (304); else the body and its ETag. Given up
/// once the host sends nothing for `stall`.
fn get(
    url: &str,
    cap: u64,
    etag: Option<&str>,
    stall: Duration,
    step: &dyn Fn(u8),
) -> io::Result<Option<(Vec<u8>, Option<String>)>> {
    let mut req = aio::capabilities::agent("updates", crate::runtime::config()).get(url);
    if let Some(e) = etag {
        req = req.header("if-none-match", e);
    }
    let res = req.call().map_err(|e| {
        let kind =
            if matches!(e, ureq::Error::StatusCode(404)) { io::ErrorKind::NotFound } else { io::ErrorKind::Other };
        io::Error::new(kind, format!("{url}: {e}"))
    })?;
    if res.status() == 304 {
        return Ok(None);
    }
    let tag = res.headers().get("etag").and_then(|v| v.to_str().ok()).map(String::from);
    // Read on a thread, each chunk awaited for `stall` at most.
    let mut body = res.into_body().into_reader().take(cap + 1);
    let (tx, rx) = std::sync::mpsc::sync_channel::<io::Result<Vec<u8>>>(4);
    std::thread::spawn(move || {
        loop {
            let mut buf = vec![0; 1 << 16];
            let r = body.read(&mut buf).map(|n| {
                buf.truncate(n);
                buf
            });
            let end = !matches!(&r, Ok(b) if !b.is_empty());
            if tx.send(r).is_err() || end {
                break;
            }
        }
    });
    let (mut out, mut said) = (vec![], 0);
    loop {
        let chunk = rx.recv_timeout(stall).map_err(|_| {
            io::Error::new(io::ErrorKind::TimedOut, format!("{url}: the host sent nothing for {} s", stall.as_secs()))
        })??;
        if chunk.is_empty() {
            break;
        }
        out.extend_from_slice(&chunk);
        let pct = (out.len() as u64 * 100 / cap.max(1)).min(100) as u8;
        if pct > said {
            said = pct;
            step(pct);
        }
    }
    if out.len() as u64 > cap {
        return Err(io::Error::other(format!("{url}: larger than {cap} bytes")));
    }
    Ok(Some((out, tag)))
}

/// The manifest at `url`, asked for only if it changed since the copy kept
/// in `d` (its ETag): a check that finds nothing new costs the host no body.
/// The answer is judged from the body each time, kept or new.
fn manifest(url: &str, d: &Path) -> io::Result<Vec<u8>> {
    let file = d.join("manifest.json");
    let kept = read_json::<aio::serde_json::Value>(&file).filter(|c| c["url"] == url);
    let etag = kept.as_ref().and_then(|c| c["etag"].as_str());
    match get(url, 1 << 20, etag, STALL, &|_| {})? {
        Some((body, etag)) => {
            if let (Some(etag), Ok(text)) = (etag, std::str::from_utf8(&body)) {
                _ = write_json(&file, &aio::serde_json::json!({ "url": url, "etag": etag, "body": text }));
            }
            Ok(body)
        }
        None => kept
            .and_then(|c| c["body"].as_str().map(|b| b.as_bytes().to_vec()))
            .ok_or_else(|| io::Error::other(format!("{url}: unchanged (304), but no copy is kept here"))),
    }
}

/// Bytes free on the disk holding `dir`, if the OS says.
fn free_bytes(dir: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
        // SAFETY: a valid C string and a zeroed out-struct.
        let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
        (unsafe { libc::statvfs(c.as_ptr(), &mut s) } == 0).then(|| s.f_bavail as u64 * s.f_frsize as u64)
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let w: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
        let mut free = 0u64;
        // SAFETY: a NUL-terminated path; the other outputs may be null.
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
                w.as_ptr(),
                &mut free,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        (ok != 0).then_some(free)
    }
    #[cfg(not(any(unix, windows)))]
    None
}

/// Can `size` bytes of bundle be staged in `dir`, before it is offered? It
/// must be writable, and have room for it unpacked (about thrice).
fn room(dir: &Path, size: u64, free: Option<u64>) -> io::Result<()> {
    let probe = dir.join(format!(".probe-{}", std::process::id()));
    std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(&probe, b""))
        .and_then(|()| std::fs::remove_file(&probe))
        .map_err(|e| io::Error::new(e.kind(), format!("updates can't be written to {}: {e}", dir.display())))?;
    let need = size.saturating_mul(3);
    match free {
        Some(f) if f < need => Err(io::Error::new(
            io::ErrorKind::StorageFull,
            format!(
                "the update needs about {} MB free in {}, and {} MB are: free some space",
                need.div_ceil(1 << 20),
                dir.display(),
                f >> 20
            ),
        )),
        _ => Ok(()),
    }
}

/// How long after `failures` failed checks in a row the next one comes:
/// `every`, doubled each time (at most 64 times), up to 4 × `every` but at
/// least an hour, and never past a day (aio updateBackoffMs).
fn backoff(every: Duration, failures: u32) -> Duration {
    const HOUR: Duration = Duration::from_secs(3600);
    if failures == 0 {
        return every;
    }
    let cap = (every * 4).max(HOUR).min(HOUR * 24);
    every.max((every * 2u32.pow(failures.min(6))).min(cap))
}

/// The channel this app follows now: the `updates` cell's once it is open
/// (where `set_channel` changes it), else as at start.
fn channel(cfg: &aio::config::Config) -> String {
    cell().map(|c| c.state().channel.clone()).filter(|c| !c.is_empty()).unwrap_or_else(|| resolved(cfg, None))
}

/// The channel at start (`aio::update::channel`), the user's `pinned` one
/// among the choices.
fn resolved(cfg: &aio::config::Config, pinned: Option<&str>) -> String {
    let env = std::env::var("AIORT_UPDATE_CHANNEL").ok();
    up::channel(env.as_deref(), pinned, up::STAMPED_CHANNEL, &cfg.updates.channel).to_string()
}

fn current(reason: String) -> Found {
    Found::Current { reason }
}

/// Look for a release once (`d`: the app's install dir): what to offer, and
/// its manifest when there is one to download.
fn look(cfg: &aio::config::Config, version: &str, data: &Path, d: &Path) -> io::Result<(Found, Option<Manifest>)> {
    let u = &cfg.updates;
    let Some(src) = u.url.as_deref() else { return Ok((current("updates are off".into()), None)) };
    if let Some(remote) = up::git_remote(src) {
        return look_git(cfg, version, data, d, remote, &|src| aio_build(&cfg.app.id, src));
    }
    let ch = channel(cfg);
    let url = format!("{}/{}.json", up::release_dir(src, &ch), manifest_name());
    let m: Manifest =
        aio::serde_json::from_slice(&manifest(&url, d)?).map_err(|e| io::Error::other(format!("{url}: {e}")))?;
    let keys_file = data.join("update-keys.json");
    let trusted: Vec<String> = read_json(&keys_file).unwrap_or_else(|| u.keys.clone());
    let expect = up::Expect { id: &cfg.app.id, channel: &ch, os: std::env::consts::OS, arch: std::env::consts::ARCH };
    let next_keys = up::check(&m, &trusted, &expect).map_err(|e| io::Error::other(format!("release refused: {e}")))?;
    let r = &m.release;
    if next_keys != trusted {
        aio::paths::write_atomic(&keys_file, aio::serde_json::to_string(&next_keys)?.as_bytes(), true)?;
        eprintln!("[aio] release keys updated by {}'s signed roster", r.version);
    }
    let pre = up::follows_prereleases(u.prerelease, version, &ch);
    if !up::newer(&r.version, version, pre) {
        let why = up::skipped_prerelease(&r.version, version, pre).unwrap_or_else(|| format!("{version} is current"));
        return Ok((current(why), None));
    }
    if dismissed(d).contains(&r.version) {
        return Ok((current(format!("{} failed here before: not offered", r.version)), None));
    }
    if std::env::consts::OS == "android" {
        return Ok((Found::Blocked(apk(r, &up::release_dir(src, &ch))), Some(m)));
    }
    let staged = d.join("versions").join(&r.version).join("release.json").exists();
    if !staged {
        let v = d.join("versions");
        room(&v, r.size, std::fs::create_dir_all(&v).ok().and_then(|()| free_bytes(&v)))?;
    }
    if !staged && let Some(local) = up::local_data(&data.join(&cfg.data.store)).map_err(io::Error::other)? {
        let blockers = up::blockers(&r.data, &local);
        if !blockers.is_empty() {
            let b = up::Blocked { version: r.version.clone(), blockers };
            return Ok((Found::Blocked(b), Some(m)));
        }
    }
    let offer = Offer {
        version: r.version.clone(),
        notes: r.notes.clone(),
        size: if staged { 0 } else { r.size },
        released_at: r.released_at.clone(),
        key: m.key.clone(),
        staged,
    };
    Ok((Found::Offer(offer), Some(m)))
}

/// A git source (`git+…`): its ref's commit, looked up. A new one is cloned
/// into a throwaway dir, built only if it or the tag followed is signed by a
/// key built into this app ([`signed`]: nothing else runs), asked
/// what data it reads, and staged like a download: a check builds, so its
/// offer says what it does to the data here. What each commit came to is
/// kept (`git.json`), so it is built or refused once. `build`: the cloned
/// source → the bundle it builds ([`aio_build`]).
fn look_git(
    cfg: &aio::config::Config,
    version: &str,
    data: &Path,
    d: &Path,
    remote: &str,
    build: &dyn Fn(&Path) -> io::Result<PathBuf>,
) -> io::Result<(Found, Option<Manifest>)> {
    let ch = channel(cfg);
    let listed = git(&["ls-remote", "--", remote, &ch, &format!("{ch}^{{}}")], None)?;
    let sha = up::git_ref_commit(&listed, &ch).map_err(|e| io::Error::other(format!("{remote}: {e}")))?;
    let pre = up::follows_prereleases(cfg.updates.prerelease, version, &ch);
    let staged = |v: &str| read_json::<Manifest>(&d.join("versions").join(v).join("release.json"));
    let seen = read_json::<aio::serde_json::Value>(&d.join("git.json")).filter(|s| s["commit"] == sha.as_str());
    if let Some(why) = seen.as_ref().and_then(|s| s["refused"].as_str()) {
        return Err(io::Error::other(format!("release refused: {why}")));
    }
    let known = seen.and_then(|s| s["version"].as_str().map(String::from));
    let v = match known.filter(|v| !up::newer(v, version, pre) || staged(v).is_some()) {
        Some(v) => v,
        None => {
            let trusted: Vec<String> =
                read_json(&data.join("update-keys.json")).unwrap_or_else(|| cfg.updates.keys.clone());
            let built = build_git(cfg, (remote, &ch, &sha), d, version, pre, Some(&trusted), build);
            let seen = match &built {
                Ok(v) => aio::serde_json::json!({ "commit": sha, "version": v }),
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                    aio::serde_json::json!({ "commit": sha, "refused": e.to_string() })
                }
                // A failed clone or build is tried again at the next check.
                Err(_) => aio::serde_json::Value::Null,
            };
            if !seen.is_null() {
                write_json(&d.join("git.json"), &seen)?;
            }
            built.map_err(|e| match e.kind() {
                io::ErrorKind::PermissionDenied => io::Error::other(format!("release refused: {e}")),
                _ => e,
            })?
        }
    };
    let short = &sha[..12];
    if !up::newer(&v, version, pre) {
        return Ok((current(format!("{ch} is at {short}, which builds {v}: {version} is current")), None));
    }
    if dismissed(d).contains(&v) {
        return Ok((current(format!("{v} failed here before: not offered")), None));
    }
    let m = staged(&v).ok_or_else(|| io::Error::other(format!("{v} is not staged")))?;
    if let Some(local) = up::local_data(&data.join(&cfg.data.store)).map_err(io::Error::other)? {
        let blockers = up::blockers(&m.release.data, &local);
        if !blockers.is_empty() {
            return Ok((Found::Blocked(up::Blocked { version: v, blockers }), Some(m)));
        }
    }
    let r = &m.release;
    let offer = Offer {
        version: v,
        notes: r.notes.clone(),
        size: 0,
        released_at: r.released_at.clone(),
        key: m.key.clone(),
        staged: true,
    };
    Ok((Found::Offer(offer), Some(m)))
}

/// Clone `at` (remote, ref, its commit) into `d/git-build` (removed after),
/// check its signature against `trusted` (refused: `PermissionDenied`;
/// `None`: unchecked, only for [`from_git`]), build it and, when it is
/// newer than `version`, stage it. Its version.
fn build_git(
    cfg: &aio::config::Config,
    (remote, r, sha): (&str, &str, &str),
    d: &Path,
    version: &str,
    pre: bool,
    trusted: Option<&[String]>,
    build: &dyn Fn(&Path) -> io::Result<PathBuf>,
) -> io::Result<String> {
    let id = &cfg.app.id;
    let work = d.join("git-build");
    _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work)?;
    let made = (|| {
        let src = work.join("src");
        eprintln!("[aio] updates: cloning {remote} @ {r} ({})", &sha[..12]);
        git(&["clone", "--quiet", "--single-branch", "--branch", r, "--", remote, &src.to_string_lossy()], None)?;
        let head = git(&["rev-parse", "HEAD"], Some(&src))?;
        let key = match trusted {
            Some(trusted) => {
                let signers = work.join("allowed_signers");
                std::fs::write(&signers, up::allowed_signers(trusted).map_err(io::Error::other)?)?;
                let by = signed(&src, r, &signers).map_err(|why| {
                    io::Error::new(io::ErrorKind::PermissionDenied, format!("{remote} @ {r} ({}): {why}", &head[..12]))
                })?;
                let key = trusted.iter().find(|k| up::ssh_fingerprint(k).is_ok_and(|f| f == by)).cloned().unwrap_or(by);
                eprintln!("[aio] updates: {} is signed by {key}: building it", &head[..12]);
                key
            }
            None => UNSIGNED.to_string(),
        };
        let bundle = std::fs::read(build(&src)?)?;
        // What it is, asked of the build itself, before anything is staged.
        let probe = work.join("probe");
        std::fs::create_dir_all(&probe)?;
        match appimage() {
            Some(_) => put_appimage(&bundle, &probe.join(format!("{id}.AppImage")))?,
            None => unpack_for(std::env::consts::OS, &bundle, &probe)?,
        }
        let exe = staged_binary(&probe, id)
            .filter(|e| e.is_file())
            .ok_or_else(|| io::Error::other("the build has no app binary"))?;
        let said = Command::new(&exe).arg("--version").stdin(std::process::Stdio::null()).output()?;
        let said = String::from_utf8_lossy(&said.stdout);
        let v = said
            .trim()
            .strip_prefix(&format!("{id} "))
            .ok_or_else(|| io::Error::other(format!("the build is not {id}: {said:?}")))?
            .to_string();
        if !up::newer(&v, version, pre) {
            return Ok(v);
        }
        let release = up::Release {
            format: up::FORMAT,
            id: id.clone(),
            version: v.clone(),
            channel: r.into(),
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            file: format!("git:{head}"),
            sha256: aio::sha256::hex(&Sha256::digest(&bundle)),
            size: bundle.len() as u64,
            released_at: aio::log::stamp(now_ms()),
            notes: match trusted {
                Some(_) => format!("built from {remote} @ {r}, commit {head}"),
                None => format!("UNSIGNED: built from {remote} @ {r}, commit {head} by `ax update --from-git`"),
            },
            data: contract_of(&exe).map_err(io::Error::other)?,
            roster: None,
        };
        let m = Manifest { release, key, signature: format!("git:{head}") };
        // The probe is the staged version: moved, not unpacked again.
        stage(d, &m, id, |into| std::fs::remove_dir(into).and_then(|()| std::fs::rename(&probe, into)))?;
        Ok(v)
    })();
    _ = std::fs::remove_dir_all(&work);
    made
}

/// The signer of a release [`from_git`] built: nobody.
pub const UNSIGNED: &str = "unsigned";

/// `ax update <app> --from-git <ref>`: the developer's own way around the signature check.
/// Ref `r` (a branch or tag) of `remote` is cloned, built and staged like a
/// git release, but its signature is never checked, so it runs whatever
/// the ref holds: said loudly first, and never part of the update checks
/// (those build only what a built-in key signed, `look_git`). The data
/// gate still holds (`d`: its install dir, [`dir`]; `data`: its data dir; a build that
/// can't read it is not staged). The app's next start installs it as any
/// update (backup, first boot, rollback). `version`: the installed one;
/// `build`: the clone → its bundle ([`aio_build`]). What was staged, said.
pub fn from_git(
    cfg: &aio::config::Config,
    (remote, r): (&str, &str),
    d: &Path,
    data: &Path,
    version: &str,
    build: &dyn Fn(&Path) -> io::Result<PathBuf>,
) -> io::Result<String> {
    let id = &cfg.app.id;
    eprintln!(
        "[aio] WARNING: UNSIGNED CODE. {remote} @ {r} is built and installed into {id} with no signature check: \
         it runs as you, with {id}'s data. Use it only on a source you control; releases go through `ax release`."
    );
    let listed = git(&["ls-remote", "--", remote, r, &format!("{r}^{{}}")], None)?;
    let sha = up::git_ref_commit(&listed, r).map_err(|e| io::Error::other(format!("{remote}: {e}")))?;
    let v = build_git(cfg, (remote, r, &sha), d, version, true, None, build)?;
    if !up::newer(&v, version, true) {
        return Err(io::Error::other(format!(
            "{remote} @ {r} builds {v}, not newer than the installed {version}: give it a higher version"
        )));
    }
    if dismissed(d).contains(&v) {
        return Err(io::Error::other(format!("{v} failed to start here before, so it is never installed again")));
    }
    let staged = d.join("versions").join(&v);
    let m = read_json::<Manifest>(&staged.join("release.json"))
        .ok_or_else(|| io::Error::other(format!("{v} is not staged")))?;
    if let Some(local) = up::local_data(&data.join(&cfg.data.store)).map_err(io::Error::other)? {
        let blockers = up::blockers(&m.release.data, &local);
        if !blockers.is_empty() {
            _ = std::fs::remove_dir_all(&staged);
            return Err(io::Error::other(format!("{v} can't read {id}'s data: {}; not staged", blockers.join("; "))));
        }
    }
    Ok(format!("{v} (UNSIGNED: {remote} @ {r}, commit {}) is staged: {id} installs it at its next start", &sha[..12]))
}

/// Who signed what `src` (a clone of ref `r`) builds, by fingerprint: its
/// commit, else the tag `r` naming that very commit, signed by a key of
/// the allowed-signers file `signers`. Unsigned, or signed by any other
/// key: why not.
fn signed(src: &Path, r: &str, signers: &Path) -> Result<String, String> {
    let file = format!("gpg.ssh.allowedSignersFile={}", signers.display());
    let ssh = ["-c", "gpg.format=ssh", "-c", "gpg.ssh.program=ssh-keygen", "-c", &file];
    let at = |args: &[&str]| git(&[&ssh[..], args].concat(), Some(src));
    // %G?: G is a good signature by an allowed signer (U: a key not in it).
    let commit = at(&["log", "-1", "--format=%G?%n%GK", "HEAD"]).map_err(|e| e.to_string())?;
    if let Some(fp) = commit.strip_prefix("G\n") {
        return Ok(fp.trim().to_string());
    }
    let tag = format!("refs/tags/{r}");
    let same = at(&["rev-parse", &format!("{tag}^{{commit}}")]).ok() == at(&["rev-parse", "HEAD"]).ok();
    if same
        && let Ok(said) = git_with_stderr(&[&ssh[..], &["verify-tag", "--", &tag]].concat(), src)
        && let Some(fp) = said.split_whitespace().find(|w| w.starts_with("SHA256:"))
    {
        return Ok(fp.to_string());
    }
    Err(format!("neither its commit nor a tag {r} on it is signed by a key this app trusts ([updates] keys)"))
}

/// What a git run says on stderr (`verify-tag`'s verdict), if it succeeds.
fn git_with_stderr(args: &[&str], cwd: &Path) -> io::Result<String> {
    let out = git_cmd(args, Some(cwd)).output()?;
    match out.status.success() {
        true => Ok(String::from_utf8_lossy(&out.stderr).into_owned()),
        false => Err(io::Error::other(String::from_utf8_lossy(&out.stderr).trim().to_string())),
    }
}

/// A git that never asks: no terminal prompt or askpass window, ssh in
/// batch mode unless the user chose their own, stalled HTTP given up after
/// a minute, the repository its arguments name (never one an inherited
/// `GIT_DIR` points at), stdin closed, no `AIORT_*` settings.
fn git_cmd(args: &[&str], cwd: Option<&Path>) -> Command {
    let mut c = Command::new("git");
    aio::lifecycle::scrub(&mut c, false);
    c.args(args).stdin(std::process::Stdio::null());
    if let Some(d) = cwd {
        c.current_dir(d);
    }
    for v in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_OBJECT_DIRECTORY", "GIT_ALTERNATE_OBJECT_DIRECTORIES"]
    {
        c.env_remove(v);
    }
    c.env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .env("GIT_HTTP_LOW_SPEED_LIMIT", "1000")
        .env("GIT_HTTP_LOW_SPEED_TIME", "60");
    let own_ssh = std::env::var_os("GIT_SSH_COMMAND").is_some()
        || Command::new("git").args(["config", "--get", "core.sshCommand"]).output().is_ok_and(|o| o.status.success());
    if !own_ssh {
        c.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes -o ServerAliveInterval=15 -o ServerAliveCountMax=4");
    }
    c
}

/// Run git ([`git_cmd`]): its output, or what it said went wrong.
fn git(args: &[&str], cwd: Option<&Path>) -> io::Result<String> {
    let out = git_cmd(args, cwd).output().map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => {
            io::Error::other("git is not installed or not on PATH: a git update source needs it")
        }
        _ => e,
    })?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let last: Vec<&str> = err.lines().filter(|l| !l.trim().is_empty()).rev().take(3).collect();
        let said = last.into_iter().rev().collect::<Vec<_>>().join(" ");
        return Err(io::Error::other(format!("git: {}", if said.is_empty() { out.status.to_string() } else { said })));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Build the cloned `src` as `ax build` does (`ax` and cargo on PATH):
/// the bundle this machine installs.
pub fn aio_build(id: &str, src: &Path) -> io::Result<PathBuf> {
    let mut cmd = Command::new("ax");
    aio::lifecycle::scrub(&mut cmd, false);
    cmd.args(["build", id]).current_dir(src).stdin(std::process::Stdio::null());
    if appimage().is_some() {
        cmd.arg("--appimage");
    }
    let out = cmd.output().map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => {
            io::Error::other("ax is not on PATH: a git update source is built with ax and cargo")
        }
        _ => e,
    })?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = err.lines().rev().take(3).collect();
        return Err(io::Error::other(format!(
            "ax build failed: {}",
            tail.into_iter().rev().collect::<Vec<_>>().join(" ")
        )));
    }
    let names: Vec<String> =
        std::fs::read_dir(src.join("dist"))?.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    let pick = pick_bundle(&names, std::env::consts::OS, appimage().is_some())
        .ok_or_else(|| io::Error::other(format!("ax build made no bundle for this machine ({})", names.join(", "))))?;
    Ok(src.join("dist").join(pick))
}

/// The bundle among `names` (a fresh build's `dist/`) that an install on
/// `os` takes: its AppImage, else its `.tar.gz` (Linux) or `.zip`.
fn pick_bundle<'a>(names: &'a [String], os: &str, appimage: bool) -> Option<&'a String> {
    let ext = match (appimage, os) {
        (true, _) => ".AppImage",
        (_, "linux") => ".tar.gz",
        _ => ".zip",
    };
    names.iter().find(|n| n.ends_with(ext))
}

/// What a build reads: it runs once in a throwaway data dir and says
/// (`AIORT_DATA_CONTRACT`), touching no data of this machine.
pub fn contract_of(binary: &Path) -> Result<up::Contract, String> {
    let tmp = std::env::temp_dir().join(format!("aio-contract-{}-{}", std::process::id(), now_ms()));
    _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let out = tmp.join("contract.json");
    let st = Command::new(binary)
        .env("AIORT_DATA", tmp.join("root"))
        .env("AIORT_DATA_CONTRACT", &out)
        .env_remove("AIORT_CONFIG")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    let read = std::fs::read(&out);
    _ = std::fs::remove_dir_all(&tmp);
    if !st.map_err(|e| e.to_string())?.success() {
        return Err("the build did not say what data it reads".into());
    }
    aio::serde_json::from_slice(&read.map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

/// An Android release (`ax release --android`), from `at`: Android puts an
/// APK in place through its system installer, never the app itself, so it
/// is offered as the link to open on the device.
fn apk(r: &up::Release, at: &str) -> up::Blocked {
    let why = format!(
        "{} is available as an Android package, and an app can't replace its own: open {at}/{} on the device and tap it to update",
        r.version, r.file
    );
    up::Blocked { version: r.version.clone(), blockers: vec![why] }
}

/// Download a release, check it against its signed size and SHA-256, and
/// stage it: made from `version` (the one running) and a patch when the
/// release has one from it ([`crate::delta`]), else whole.
fn download(cfg: &aio::config::Config, m: &Manifest, d: &Path, version: &str, step: &dyn Fn(u8)) -> io::Result<()> {
    let r = &m.release;
    let src = cfg.updates.url.as_deref().ok_or_else(|| io::Error::other("updates are off"))?;
    // macOS: a bundle that can't replace itself where it runs is said before a byte comes.
    #[cfg(target_os = "macos")]
    if let Some(why) = place(&cfg.app.id)
        .and_then(|(app, _)| crate::macmove::blocker(&app.to_string_lossy(), &crate::macmove::can_write))
    {
        return Err(io::Error::other(why));
    }
    // The release's own channel: the one it was found on, signed.
    let at = up::release_dir(src, &r.channel);
    match by_patch(&at, m, d, version, &cfg.app.id, step) {
        Some(Ok(())) => eprintln!("[aio] update {}: made from {version} and a patch", r.version),
        found => {
            if let Some(Err(why)) = found {
                eprintln!("[aio] update {}: no patch used ({why}): downloading the whole release", r.version);
            }
            let bytes = fetch(&format!("{at}/{}", r.file), r.size, step)?;
            if bytes.len() as u64 != r.size || aio::sha256::hex(&Sha256::digest(&bytes)) != r.sha256 {
                return Err(io::Error::other(format!("{}: not the signed bundle (size or SHA-256 differ)", r.file)));
            }
            stage(d, m, &cfg.app.id, |into| match appimage() {
                Some(_) => put_appimage(&bytes, &into.join(format!("{}.AppImage", cfg.app.id))),
                None => unpack_for(std::env::consts::OS, &bytes, into),
            })?;
        }
    }
    // A window's new version starts on the runtime it pins: got now, so its
    // first start doesn't wait on a download inside FIRST_BOOT. One that
    // carries its own (an offline build: beside its binary) needs none.
    let staged = d.join("versions").join(&r.version);
    if !matches!(cfg.app.mode, aio::config::Mode::Server | aio::config::Mode::Cli)
        && let Some(exe) = staged_binary(&staged, &cfg.app.id)
        && !exe.parent().is_some_and(crate::runtime::carries_runtime)
        && let Some(rt) = std::fs::read(&exe).ok().as_deref().and_then(crate::runtime::pinned_in)
    {
        let bytes = std::fs::read(&exe).unwrap_or_default();
        let got = crate::runtime::prefetch(&rt, crate::runtime::sha_in(&bytes, &rt).as_deref()).and_then(|_| {
            // Named in the store by the version that needs it, so no prune
            // takes it: an AppImage is replaced in place, so its record is
            // the installed file's, pending its first start (aio's update).
            let (exe, pending) = match appimage() {
                Some(file) => (file, true),
                None => (exe.clone(), false),
            };
            let used = crate::prune::Use {
                exe: exe.to_string_lossy().into(),
                version: rt.clone(),
                slug: crate::runtime::slug(),
                window: None,
                pending,
            };
            crate::prune::record(&crate::runtime::root(), &used)
        });
        if let Err(e) = got {
            _ = std::fs::remove_dir_all(d.join("versions").join(&r.version));
            return Err(io::Error::other(format!("{} needs Electron {rt}, which could not be got: {e}", r.version)));
        }
    }
    Ok(())
}

/// Stage `m` from `version`, kept in `d`, and the release's patch from it
/// (fetched from `at`). `None`: the release has no patches (a channel
/// published without them installs as it always did, not a word said).
fn by_patch(at: &str, m: &Manifest, d: &Path, version: &str, bin: &str, step: &dyn Fn(u8)) -> Option<io::Result<()>> {
    let list = match get(&format!("{at}/{}.patches.json", manifest_name()), 1 << 20, None, STALL, &|_| {}) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
        r => r,
    };
    Some((|| {
        let list: up::Patches = aio::serde_json::from_slice(&list?.map(|(b, _)| b).unwrap_or_default())
            .map_err(|e| io::Error::other(format!("its patch list: {e}")))?;
        let p = up::patch_for(&list, m, version).map_err(io::Error::other)?;
        let base = d.join("versions").join(version);
        if !base.is_dir() {
            return Err(io::Error::other(format!("{version} is not kept here to patch from")));
        }
        let bytes = fetch(&format!("{at}/{}", p.file), p.size, step)?;
        if bytes.len() as u64 != p.size || aio::sha256::hex(&Sha256::digest(&bytes)) != p.sha256 {
            return Err(io::Error::other(format!("{}: not the signed patch (size or SHA-256 differ)", p.file)));
        }
        stage(d, m, bin, |into| crate::delta::apply(&bytes, &base, into))
    })())
}

/// What a look found, for the log.
pub fn said(f: &Found) -> String {
    match f {
        Found::Current { reason } => reason.clone(),
        Found::Offer(o) if o.staged => format!("{} is ready: it installs the next time the app starts", o.version),
        Found::Offer(o) => format!("{} is available", o.version),
        // Android: every release is offered as its APK to open ([`apk`]).
        Found::Blocked(b) if cfg!(target_os = "android") => b.blockers.join("; "),
        Found::Blocked(b) => format!("{} can't read the data here ({}): not offered", b.version, b.blockers.join("; ")),
    }
}

/// Look for a release once. With `[updates] auto` an offered one is
/// downloaded, checked and staged too.
pub fn check(cfg: &aio::config::Config, version: &str, data: &Path) -> io::Result<Found> {
    let d = dir(&cfg.app.id);
    match look(cfg, version, data, &d)? {
        (Found::Offer(o), Some(m)) if cfg.updates.auto && !o.staged => {
            download(cfg, &m, &d, version, &|_| {})?;
            Ok(Found::Offer(Offer { staged: true, size: 0, ..o }))
        }
        (found, _) => Ok(found),
    }
}

/// Install `want` now (the cell's `apply`): staged if it isn't yet, then this
/// app stops and starts again with the same arguments, and installs it.
pub fn apply_now(
    cfg: &aio::config::Config,
    version: &str,
    data: &Path,
    want: &str,
    how: Data,
    step: &dyn Fn(Step),
) -> io::Result<()> {
    if !installed(&cfg.app.id) {
        return Err(io::Error::other(
            "this is not an installed copy of the app (a dev build?): updates install into one",
        ));
    }
    let d = dir(&cfg.app.id);
    // A git source's look stages what it builds.
    let ready = || d.join("versions").join(want).join("release.json").exists();
    if !ready() {
        match look(cfg, version, data, &d)? {
            (Found::Offer(o), Some(m)) if o.version == want => {
                if !ready() {
                    download(cfg, &m, &d, version, &|p| step(Step::Downloaded(p)))?
                }
            }
            (Found::Blocked(b), Some(m)) if b.version == want && how != Data::Gated => {
                if !ready() {
                    download(cfg, &m, &d, version, &|p| step(Step::Downloaded(p)))?
                }
            }
            (found, _) => return Err(io::Error::other(format!("{want} is no longer offered: {}", said(&found)))),
        }
    }
    // How the start that installs it treats the data.
    let mark = d.join("versions").join(want).join(DATA_MARK);
    match how {
        Data::Gated => _ = std::fs::remove_file(&mark),
        Data::AcceptLoss => std::fs::write(&mark, "accept")?,
        Data::Retire => std::fs::write(&mark, "retire")?,
    }
    step(Step::Installing);
    // Supervised: the supervisor's restart is the relaunch.
    if !supervised(cfg) {
        relaunch()?;
    }
    eprintln!("[aio] updates: installing {want} now: the app restarts");
    aio::lifecycle::ask_restart();
    Ok(())
}

/// The built-in runtime behind the `updates` cell.
struct Shell {
    cfg: aio::config::Config,
    version: String,
    data: PathBuf,
}

impl up::Runtime for Shell {
    fn check(&self) -> Result<Found, String> {
        check(&self.cfg, &self.version, &self.data).map_err(|e| e.to_string())
    }
    fn apply(&self, version: &str, how: Data, step: &dyn Fn(Step)) -> Result<(), String> {
        apply_now(&self.cfg, &self.version, &self.data, version, how, step).map_err(|e| e.to_string())
    }
}

static CELL: RwLock<Option<aio::Source<Updates>>> = RwLock::new(None);

/// The app's `updates` cell, once `start` ran with `[updates] url` set:
/// what a UI shows of its updates (available, blocked, progress), and the
/// verbs it calls (`check`, `apply`, `dismiss`).
pub fn cell() -> Option<aio::Source<Updates>> {
    CELL.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// A test's `updates` (aio's stub runtime): from now on [`cell`] is a fresh
/// cell, ready as `current`, whose checks find `found` and whose install
/// steps through to `Restarting` touching nothing ([`up::Stub`]): the app's
/// own update UI is driven on it as on the real one. Its handle, to wait on
/// its effects (`done("check")`).
pub fn stub(current: &str, found: Found) -> aio::Handle<Updates> {
    up::install(Arc::new(up::Stub(found)));
    let h = aio::Handle::<Updates>::memory();
    let ready = Ready { channel: "stable".into(), current: current.into(), ..Ready::default() };
    h.ready(ready).expect("a fresh updates cell takes ready");
    *CELL.write().unwrap_or_else(|e| e.into_inner()) = Some(aio::Source::Local(h.clone()));
    h
}

/// Open the `updates` cell (`updates.db`, beside the app's data) on this
/// runtime.
pub(crate) fn open(cfg: &aio::config::Config, version: &str, dirs: &aio::paths::AppDirs) -> io::Result<()> {
    if cfg.updates.url.is_none() || cell().is_some() {
        return Ok(());
    }
    let s = aio::Source::<Updates>::open(&dirs.data.join("updates.db")).map_err(|e| io::Error::other(e.0))?;
    up::install(Arc::new(Shell { cfg: cfg.clone(), version: version.into(), data: dirs.data.clone() }));
    let pinned = s.state().pinned.clone();
    s.ready(Ready {
        channel: resolved(cfg, pinned.as_deref()),
        current: version.into(),
        rolled_back: ROLLED_BACK.get().cloned(),
        backup: BACKUP.get().cloned(),
    })
    .map_err(|e| io::Error::other(e.why))?;
    *CELL.write().unwrap_or_else(|e| e.into_inner()) = Some(s);
    Ok(())
}

/// Put a checked release into `versions/<v>` (whole, or not at all):
/// `fill` writes its files into the folder it is given.
fn stage(d: &Path, m: &Manifest, bin: &str, fill: impl FnOnce(&Path) -> io::Result<()>) -> io::Result<()> {
    let v = &m.release.version;
    let partial = d.join("versions").join(format!(".{v}.partial"));
    _ = std::fs::remove_dir_all(&partial);
    std::fs::create_dir_all(&partial)?;
    let unpacked = fill(&partial);
    let exe = staged_binary(&partial, bin);
    let made = (|| {
        unpacked?;
        if !exe.as_ref().is_some_and(|e| e.is_file()) {
            return Err(io::Error::other("the bundle has no app binary"));
        }
        // The binary must run and be the version the manifest says.
        let said = Command::new(exe.as_ref().ok_or_else(|| io::Error::other("no app binary"))?)
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .output()?;
        let want = format!("{bin} {v}");
        if String::from_utf8_lossy(&said.stdout).trim() != want {
            return Err(io::Error::other(format!("the bundle's binary is not {want}")));
        }
        write_json(&partial.join("release.json"), &aio::serde_json::to_value(m)?)?;
        let target = d.join("versions").join(v);
        _ = std::fs::remove_dir_all(&target);
        std::fs::rename(&partial, &target)
    })();
    if made.is_err() {
        _ = std::fs::remove_dir_all(&partial);
    }
    made
}

/// An AppImage release staged as `to`, ready to run.
fn put_appimage(bytes: &[u8], to: &Path) -> io::Result<()> {
    std::fs::write(to, bytes)?;
    #[cfg(unix)]
    std::fs::set_permissions(to, std::os::unix::fs::PermissionsExt::from_mode(0o755))?;
    Ok(())
}

/// Unpack a bundle into `into` as it is installed: Linux's `.tar.gz` (its
/// one top dir stripped), macOS's zipped `.app` (`ditto`, which keeps its
/// links and signature), Windows's zipped `.exe`. `ax release` unpacks
/// bundles for `os` so to make patches.
pub fn unpack_for(os: &str, bundle: &[u8], into: &Path) -> io::Result<()> {
    let ok = |st: std::process::ExitStatus| {
        if st.success() { Ok(()) } else { Err(io::Error::other(format!("unpack: {st}"))) }
    };
    if os == "linux" {
        let tgz = into.join(".bundle.tar.gz");
        std::fs::write(&tgz, bundle)?;
        let st = Command::new("tar")
            .arg("-xzf")
            .arg(&tgz)
            .arg("-C")
            .arg(into)
            .args(["--strip-components=1", "--no-same-owner", "--no-same-permissions"])
            .status();
        std::fs::remove_file(&tgz)?;
        return ok(st?);
    }
    if os == "macos" {
        let zip = into.join(".bundle.zip");
        std::fs::write(&zip, bundle)?;
        let st = Command::new("/usr/bin/ditto").args(["-x", "-k"]).arg(&zip).arg(into).status();
        std::fs::remove_file(&zip)?;
        return ok(st?);
    }
    zip::ZipArchive::new(std::io::Cursor::new(bundle)).and_then(|mut z| z.extract(into)).map_err(io::Error::other)
}

/// The app binary in an unpacked bundle.
fn staged_binary(dir: &Path, bin: &str) -> Option<PathBuf> {
    match std::env::consts::OS {
        "macos" => std::fs::read_dir(dir)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e == "app"))
            .map(|app| app.join("Contents/MacOS").join(bin)),
        "windows" => Some(dir.join(format!("{bin}.exe"))),
        _ if appimage().is_some() => Some(dir.join(format!("{bin}.AppImage"))),
        _ => Some(dir.join(bin)),
    }
}

/// What an app with no window does about a release a check found.
#[derive(Debug, PartialEq, Eq)]
enum Act {
    None,
    /// Install it now: the app restarts with the same arguments.
    Apply(String),
    /// Ask on the terminal (yes: install now; else: not this one).
    Ask(String),
}

/// No window to show the `updates` cell in (a server, a CLI): with `auto`
/// a staged release installs now, unattended (a server may never start
/// again on its own); without, a person on the terminal is asked. With a
/// window, the app's UI decides, and `auto` installs at the next start.
fn headless(cfg: &aio::config::Config, f: &Found, tty: bool) -> Act {
    use aio::config::Mode;
    let Found::Offer(o) = f else { return Act::None };
    match (matches!(cfg.app.mode, Mode::Server | Mode::Cli), cfg.updates.auto) {
        (false, _) => Act::None,
        (true, true) if o.staged => Act::Apply(o.version.clone()),
        (true, false) if tty => Act::Ask(o.version.clone()),
        _ => Act::None,
    }
}

/// `question` on the terminal: true for y or yes.
fn ask(question: &str) -> bool {
    use std::io::Write;
    print!("\n{question}");
    _ = std::io::stdout().flush();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).is_ok() && yes(&line)
}

fn yes(answer: &str) -> bool {
    matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
}

/// Look for releases in the background, while the app runs from an
/// installed version: soon after start, then every `[updates] every`.
pub fn watch(cfg: &aio::config::Config, version: &str, data: &Path) {
    if cfg.updates.url.is_none() || !installed(&cfg.app.id) || cfg.updates.every_on(&channel(cfg)).is_none() {
        return;
    }
    let (cfg, version, data) = (cfg.clone(), version.to_string(), data.to_path_buf());
    let spawned = std::thread::Builder::new().name("aio-updates".into()).spawn(move || {
        // The cell's `apply` and `dismiss` are effects: they run on aio's runtime.
        let _rt = aio::rt().enter();
        // Some jitter, so many machines don't ask at the same moment.
        let jitter = u64::from(std::process::id() % 60);
        std::thread::sleep(Duration::from_secs(30 + jitter));
        let mut failures = 0;
        loop {
            let found = check(&cfg, &version, &data);
            failures = if found.is_ok() { 0 } else { failures + 1 };
            match &found {
                Ok(f) => {
                    aio::health::ok("updates");
                    eprintln!("[aio] updates: {}", said(f));
                }
                Err(e) => {
                    aio::health::fail("updates", &e.to_string());
                    eprintln!("[aio] updates: {e}");
                }
            }
            let act = found.as_ref().map_or(Act::None, |f| headless(&cfg, f, std::io::stdin().is_terminal()));
            if let Some(c) = cell() {
                _ = c.checked(found.map_err(|e| e.to_string()));
                match act {
                    Act::None => {}
                    Act::Apply(v) => {
                        eprintln!("[aio] updates: installing {v} now (auto, no window)");
                        _ = c.apply();
                    }
                    Act::Ask(v) => match ask(&format!("Update to {v}? The app will restart. [y/N] ")) {
                        true => _ = c.apply(),
                        false => _ = c.dismiss(),
                    },
                }
            }
            // The channel may have changed meanwhile, and its interval with it.
            match cfg.updates.every_on(&channel(&cfg)) {
                Some(every) => std::thread::sleep(backoff(every, failures)),
                None => return eprintln!("[aio] updates: no more checks on channel {}", channel(&cfg)),
            }
        }
    });
    if let Err(e) = spawned {
        eprintln!("[aio] no update checks: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_windows_offline_build_swaps_its_whole_folder_with_its_runtime() {
        let d = tempfile::tempdir().unwrap();
        let (live, new) = (d.path().join("Programs/notes/notes.exe"), d.path().join("versions/1.1.0/notes.exe"));
        let pair = |p: &Path| (p.to_path_buf(), p.to_path_buf());
        // A shared-runtime build: the exe alone.
        assert_eq!(with_runtime(pair(&live), pair(&new)), (pair(&live), pair(&new)));
        let rt = d.path().join("versions/1.1.0/electron");
        std::fs::create_dir_all(&rt).unwrap();
        std::fs::write(rt.join("electron.exe"), b"MZ").unwrap();
        let (place, staged) = with_runtime(pair(&live), pair(&new));
        assert_eq!(place, (d.path().join("Programs/notes"), live.clone()), "the exe still starts it");
        assert_eq!(staged, (d.path().join("versions/1.1.0"), new.clone()), "the helper is still the exe");
        // A macOS bundle is a folder already.
        let app = d.path().join("versions/1.1.0/Notes.app");
        assert_eq!(with_runtime(pair(&live), pair(&app)), (pair(&live), pair(&app)));
    }

    #[test]
    fn an_app_runs_from_an_appimage_only_from_inside_its_mount() {
        let (file, mount) = (Some("/home/u/Notes.AppImage"), Some("/tmp/.mount_NotesX"));
        let inside = Path::new("/tmp/.mount_NotesX/usr/bin/notes");
        assert_eq!(appimage_of(file, mount, inside), Some(PathBuf::from("/home/u/Notes.AppImage")));
        // Started from an AppImage's terminal: its variables, another binary.
        assert_eq!(appimage_of(file, mount, Path::new("/home/u/.local/bin/other")), None);
        assert_eq!(appimage_of(None, mount, inside), None);
        assert_eq!(appimage_of(file, None, inside), None);
        assert_eq!(appimage_of(Some(""), mount, inside), None);
        // A test is no AppImage: releases are named as for its bundle.
        assert_eq!(appimage(), None);
        assert_eq!(manifest_name(), platform());
    }

    #[test]
    fn an_app_with_no_window_installs_unattended_or_asks_on_the_terminal() {
        use aio::config::Mode;
        let cfg = |mode, auto| {
            let mut c = aio::config::Config::parse("[app]\nid = \"t\"").unwrap();
            (c.app.mode, c.updates.auto) = (mode, auto);
            c
        };
        let offer = |staged| Found::Offer(Offer { version: "2.0.0".into(), staged, ..Offer::default() });
        let apply = Act::Apply("2.0.0".into());
        assert_eq!(headless(&cfg(Mode::Server, true), &offer(true), false), apply);
        assert_eq!(headless(&cfg(Mode::Cli, true), &offer(true), true), apply);
        // A download that failed to stage is not installed.
        assert_eq!(headless(&cfg(Mode::Server, true), &offer(false), false), Act::None);
        assert_eq!(headless(&cfg(Mode::Cli, false), &offer(false), true), Act::Ask("2.0.0".into()));
        // No one to ask (a service): it waits for `ax call … updates apply`.
        assert_eq!(headless(&cfg(Mode::Server, false), &offer(false), false), Act::None);
        // A window: the app's UI decides, auto installs at the next start.
        for m in [Mode::Standalone, Mode::ClientServer, Mode::Client] {
            assert_eq!(headless(&cfg(m, true), &offer(true), true), Act::None);
            assert_eq!(headless(&cfg(m, false), &offer(false), true), Act::None);
        }
        let current = Found::Current { reason: "x".into() };
        assert_eq!(headless(&cfg(Mode::Server, true), &current, true), Act::None);
        assert!(yes("y\n") && yes(" YES ") && !yes("") && !yes("n") && !yes("yep"));
    }

    #[cfg(target_os = "linux")]
    /// A fake installed app in `d`: versions/<v>/app scripts, `current` → old.
    fn fake(d: &Path, versions: &[(&str, &str)]) {
        use std::os::unix::fs::PermissionsExt;
        for (v, body) in versions {
            let vd = d.join("versions").join(v);
            std::fs::create_dir_all(&vd).unwrap();
            std::fs::write(vd.join("app"), format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(vd.join("app"), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::os::unix::fs::symlink("versions/1.0.0", d.join("current")).unwrap();
    }

    #[cfg(target_os = "linux")]
    fn helper(d: &Path, ticks: &str) -> std::process::ExitStatus {
        // The "old app" is a process that has already exited.
        let gone = Command::new("true").spawn().unwrap();
        let pid = gone.id().to_string();
        let mut gone = gone;
        gone.wait().unwrap();
        Command::new("/bin/sh")
            .args(["-c", HELPER, "aio-update", &pid])
            .arg(d)
            .args(["2.0.0", "1.0.0", "app", ticks, "a file.md", "--x"])
            .status()
            .unwrap()
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_new_version_that_claims_its_first_boot_stays() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        let mark = d.join("ran");
        fake(
            d,
            &[
                ("1.0.0", &format!("echo old >> '{}'", mark.display())),
                ("2.0.0", &format!("rm '{}/first-boot'; echo new \"$@\" >> '{}'", d.display(), mark.display())),
            ],
        );
        std::fs::write(d.join("first-boot"), "2.0.0").unwrap();
        assert!(helper(d, "50").success());
        assert_eq!(std::fs::read_link(d.join("current")).unwrap(), Path::new("versions/2.0.0"));
        assert_eq!(std::fs::read_to_string(&mark).unwrap(), "new a file.md --x\n", "with the app's arguments");
        assert!(!d.join("update-failed").exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn supervised_current_switches_at_once_and_a_second_start_unbooted_goes_back() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        let ran = d.join("ran");
        fake(d, &[("1.0.0", ""), ("2.0.0", &format!("echo new >> '{}'", ran.display()))]);
        spawn_helper(d, "app", "1.0.0", "2.0.0", &[], &d.join("update.log"), true).unwrap();
        assert_eq!(std::fs::read_link(d.join("current")).unwrap(), Path::new("versions/2.0.0"));
        std::thread::sleep(Duration::from_millis(200));
        assert!(!ran.exists() && !d.join("update.log").exists(), "no helper, nothing started");
        write_json(&d.join("pending.json"), &aio::serde_json::json!({ "from": "1.0.0", "to": "2.0.0" })).unwrap();
        std::fs::write(d.join("first-boot"), "2.0.0").unwrap();
        assert_eq!(second_start(d, "2.0.0").unwrap(), None, "its first start boots");
        assert_eq!(second_start(d, "2.0.0").unwrap().as_deref(), Some("1.0.0"), "it died before it booted");
        assert_eq!(std::fs::read_link(d.join("current")).unwrap(), Path::new("versions/1.0.0"));
        assert!(d.join("update-failed").exists() && !d.join("first-boot").exists() && !d.join(TRIED).exists());
        assert_eq!(second_start(d, "1.0.0").unwrap(), None, "the old one undoes the rest as it starts");
        // A first boot claimed is kept, however often it starts after.
        std::fs::write(d.join("first-boot"), "2.0.0").unwrap();
        assert_eq!(second_start(d, "2.0.0").unwrap(), None);
        std::fs::remove_file(d.join("first-boot")).unwrap();
        assert_eq!(second_start(d, "2.0.0").unwrap(), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_new_version_that_asks_for_the_one_before_it_is_rolled_back_at_once() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        let mark = d.join("ran");
        fake(
            d,
            &[
                ("1.0.0", &format!("echo old >> '{}'", mark.display())),
                (
                    "2.0.0",
                    &format!("echo new >> '{}'; touch '{}/first-boot.back'; exit 1", mark.display(), d.display()),
                ),
            ],
        );
        std::fs::write(d.join("first-boot"), "2.0.0").unwrap();
        assert!(helper(d, "50").success());
        assert_eq!(std::fs::read_to_string(&mark).unwrap(), "new\nold\n", "never started a second time");
        assert_eq!(std::fs::read_link(d.join("current")).unwrap(), Path::new("versions/1.0.0"));
        assert!(d.join("update-failed").exists() && !d.join("first-boot").exists());
        assert!(!can_go_back("aio-test-none", "2.0.0"), "not a first boot of an installed app");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_new_version_that_never_opens_is_rolled_back() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        let mark = d.join("ran");
        // 2.0.0 crashes at once, twice (aio's two boot attempts); one that hangs is the same to the helper.
        fake(
            d,
            &[
                ("1.0.0", &format!("echo old >> '{}'", mark.display())),
                ("2.0.0", &format!("echo new >> '{}'; exit 3", mark.display())),
            ],
        );
        std::fs::write(d.join("first-boot"), "2.0.0").unwrap();
        assert!(helper(d, "50").success());
        assert_eq!(std::fs::read_link(d.join("current")).unwrap(), Path::new("versions/1.0.0"));
        assert_eq!(std::fs::read_to_string(&mark).unwrap(), "new\nnew\nold\n");
        assert!(d.join("update-failed").exists() && !d.join("first-boot").exists());
        // A hang: stopped after the wait, then the old one runs.
        std::fs::remove_file(d.join("update-failed")).unwrap();
        std::fs::remove_file(&mark).unwrap();
        std::fs::write(
            d.join("versions/2.0.0/app"),
            format!("#!/bin/sh\necho new >> '{}'\nexec sleep 30\n", mark.display()),
        )
        .unwrap();
        std::fs::write(d.join("first-boot"), "2.0.0").unwrap();
        let t0 = std::time::Instant::now();
        assert!(helper(d, "5").success());
        assert!(t0.elapsed() < Duration::from_secs(15), "the hung version was stopped");
        assert_eq!(std::fs::read_link(d.join("current")).unwrap(), Path::new("versions/1.0.0"));
        assert_eq!(std::fs::read_to_string(&mark).unwrap(), "new\nold\n");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_new_version_that_fails_its_first_start_gets_a_second() {
        // aio tests/updates-rollback.test.ts "retry, retry, then a REAL restore".
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        let (mark, tried) = (d.join("ran"), d.join("tried"));
        let second = format!(
            "if [ -e '{t}' ]; then rm '{d}/first-boot'; echo new >> '{m}'; else touch '{t}'; echo crash >> '{m}'; exit 1; fi",
            t = tried.display(),
            d = d.display(),
            m = mark.display()
        );
        fake(d, &[("1.0.0", &format!("echo old >> '{}'", mark.display())), ("2.0.0", &second)]);
        std::fs::write(d.join("first-boot"), "2.0.0").unwrap();
        assert!(helper(d, "50").success());
        assert_eq!(std::fs::read_link(d.join("current")).unwrap(), Path::new("versions/2.0.0"));
        assert_eq!(std::fs::read_to_string(&mark).unwrap(), "crash\nnew\n");
        assert!(!d.join("update-failed").exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_switch_that_cannot_be_made_runs_the_old_version_and_says_so() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        let mark = d.join("ran");
        fake(d, &[("2.0.0", "echo new")]);
        // `current` held as a real folder: the link can't replace it.
        std::fs::remove_file(d.join("current")).unwrap();
        std::fs::create_dir(d.join("current")).unwrap();
        std::fs::write(d.join("current/app"), format!("#!/bin/sh\necho old >> '{}'\n", mark.display())).unwrap();
        std::fs::set_permissions(d.join("current/app"), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        std::fs::write(d.join("first-boot"), "2.0.0").unwrap();
        assert!(helper(d, "50").success());
        assert_eq!(std::fs::read_to_string(&mark).unwrap(), "old\n");
        assert!(d.join(crate::swap::SWAP_FAILED).exists() && !d.join("update-failed").exists());
    }

    #[test]
    fn a_swap_never_made_is_tried_again_until_the_third_time() {
        // aio tests/updates-failed-swap-retry.test.ts: on offer, counted per release, the bound dismisses it.
        let t = tempfile::tempdir().unwrap();
        let (d, data) = (t.path().join("install"), t.path().join("data"));
        std::fs::create_dir_all(d.join("versions/2.0.0")).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        let fail = |to: &str, retired: Option<&Path>| {
            std::fs::write(d.join(crate::swap::SWAP_FAILED), to).unwrap();
            let p = aio::serde_json::json!({ "from": "1.0.0", "to": to, "retired": retired });
            swap_unmade(&d, Some(&p), &data, "1.0.0").unwrap()
        };
        // A fresh start's data comes back: the new version never ran.
        let aside = t.path().join("archive/a");
        std::fs::create_dir_all(&aside).unwrap();
        std::fs::write(aside.join("state.db"), "mine").unwrap();
        assert!(fail("2.0.0", Some(&aside)).ends_with("tried again at the next start (1 of 3)"));
        assert_eq!(std::fs::read_to_string(data.join("state.db")).unwrap(), "mine");
        assert!(!aside.exists() && !d.join(crate::swap::SWAP_FAILED).exists());
        // Another release counts on its own.
        assert!(fail("1.5.0", None).ends_with("(1 of 3)"));
        assert!(fail("2.0.0", None).ends_with("(1 of 3)"));
        assert!(fail("2.0.0", None).ends_with("(2 of 3)"));
        assert!(dismissed(&d).is_empty() && d.join("versions/2.0.0").exists());
        let last = fail("2.0.0", None);
        assert!(last.contains("3 times, so it is given up"), "{last}");
        assert_eq!(dismissed(&d), ["2.0.0"]);
        assert!(!d.join("versions/2.0.0").exists() && !d.join("swaps.json").exists());
    }

    #[test]
    fn a_fresh_start_moves_the_data_aside_whole_and_a_failed_one_brings_it_back() {
        // aio tests/updates-retire.test.ts: archived, never deleted; a failure leaves it in place.
        let t = tempfile::tempdir().unwrap();
        let (data, archive) = (t.path().join("data"), t.path().join("archive/notes-1.0.0-x"));
        std::fs::create_dir_all(data.join("backups")).unwrap();
        for f in ["state.db", "instance.lock", "update-keys.json", "backups/pre.db"] {
            std::fs::write(data.join(f), f).unwrap();
        }
        let keep = ["instance.lock", "update-keys.json"].map(String::from);
        retire(&data, &archive, &keep).unwrap();
        let names = |d: &Path| {
            let mut n: Vec<String> =
                std::fs::read_dir(d).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
            n.sort();
            n
        };
        assert_eq!(names(&data), ["instance.lock", "update-keys.json"]);
        assert_eq!(names(&archive), ["backups", "state.db"]);
        assert_eq!(std::fs::read_to_string(archive.join("backups/pre.db")).unwrap(), "backups/pre.db");
        // The new version wrote its own; going back keeps that aside.
        std::fs::write(data.join("state.db"), "new").unwrap();
        retire(&data, &t.path().join("archive/failed"), &keep).unwrap();
        retire(&archive, &data, &[]).unwrap();
        assert_eq!(std::fs::read_to_string(data.join("state.db")).unwrap(), "state.db");
        assert_eq!(std::fs::read_to_string(t.path().join("archive/failed/state.db")).unwrap(), "new");
        // A move that fails puts back what had moved.
        let blocked = t.path().join("blocked");
        std::fs::create_dir_all(blocked.join("state.db")).unwrap();
        std::fs::write(blocked.join("state.db/x"), "").unwrap();
        assert!(retire(&data, &blocked, &keep).is_err());
        assert_eq!(names(&data), ["backups", "instance.lock", "state.db", "update-keys.json"]);
    }

    #[test]
    fn three_versions_stay_and_never_the_running_or_previous_one() {
        let t = tempfile::tempdir().unwrap();
        for v in ["1.0.0", "1.1.0", "1.2.0", "1.3.0", "0.9.0"] {
            std::fs::create_dir_all(t.path().join("versions").join(v)).unwrap();
        }
        let left = || {
            let mut l: Vec<_> = std::fs::read_dir(t.path().join("versions"))
                .unwrap()
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .collect();
            l.sort();
            l
        };
        prune(t.path(), "1.3.0", "0.9.0", KEEP);
        assert_eq!(left(), ["0.9.0", "1.1.0", "1.2.0", "1.3.0"]);
        // `[updates] keep = 2`: the newest two, and the one before always.
        prune(t.path(), "1.3.0", "0.9.0", 2);
        assert_eq!(left(), ["0.9.0", "1.2.0", "1.3.0"]);
    }

    #[aio::cell(persist, version = 3)]
    pub struct Notes {
        n: u32,
    }

    #[aio::methods]
    impl Notes {
        pub fn add(&mut self) {
            self.n += 1;
        }
    }

    /// A loopback release host serving `files` (path → body), until dropped.
    fn host(files: Vec<(String, Vec<u8>)>) -> String {
        use std::io::{BufRead, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                let mut line = String::new();
                _ = io::BufReader::new(&s).read_line(&mut line);
                let path = line.split(' ').nth(1).unwrap_or("").to_string();
                let mut w = &s;
                match files.iter().find(|(p, _)| *p == path) {
                    Some((_, b)) => {
                        _ = write!(w, "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", b.len());
                        _ = w.write_all(b);
                    }
                    None => _ = write!(w, "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"),
                }
            }
        });
        url
    }

    /// A signed release of `version` for this machine, its bundle `bytes`.
    fn release(pkcs8: &[u8], version: &str, bytes: &[u8], data: up::Contract) -> Manifest {
        let r = up::Release {
            format: up::FORMAT,
            id: "notes".into(),
            version: version.into(),
            channel: "stable".into(),
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            file: format!("notes-{version}.tar.gz"),
            sha256: aio::sha256::hex(&Sha256::digest(bytes)),
            size: bytes.len() as u64,
            released_at: "2026-10-28T00:00:00.000Z".into(),
            notes: "faster".into(),
            data,
            roster: None,
        };
        up::sign(r, pkcs8).unwrap()
    }

    fn config(url: &str, key: &str, more: &str) -> aio::config::Config {
        aio::config::Config::parse(&format!(
            "[app]\nid = \"notes\"\n[updates]\nurl = \"{url}\"\nkeys = [\"{key}\"]\n{more}"
        ))
        .unwrap()
    }

    fn serve(m: &Manifest, bundle: &[u8]) -> String {
        host(vec![
            (format!("/stable/{}.json", platform()), aio::serde_json::to_vec(m).unwrap()),
            (format!("/stable/{}", m.release.file), bundle.to_vec()),
        ])
    }

    #[test]
    fn a_signed_newer_release_is_offered_with_what_the_user_sees() {
        let (k, public) = up::keygen().unwrap();
        let t = tempfile::tempdir().unwrap();
        let (data, d) = (t.path().join("data"), t.path().join("install"));
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(&d).unwrap();
        let m = release(&k, "1.1.0", b"bundle", up::Contract::default());
        let cfg = config(&serve(&m, b"bundle"), &public, "");
        let (found, got) = look(&cfg, "1.0.0", &data, &d).unwrap();
        let Found::Offer(o) = found else { panic!("{found:?}") };
        assert_eq!((o.version.as_str(), o.notes.as_str(), o.size, o.staged), ("1.1.0", "faster", 6, false));
        assert_eq!((o.key, got), (public.clone(), Some(m.clone())));
        assert_eq!(look(&cfg, "1.1.0", &data, &d).unwrap().0, current("1.1.0 is current".into()));
        // A release that failed here is never offered again.
        dismiss(&d, "1.1.0").unwrap();
        assert!(said(&look(&cfg, "1.0.0", &data, &d).unwrap().0).contains("failed here before"));
    }

    #[test]
    fn a_prerelease_is_offered_only_when_asked_for() {
        let (k, public) = up::keygen().unwrap();
        let t = tempfile::tempdir().unwrap();
        let m = release(&k, "1.1.0-rc.1", b"b", up::Contract::default());
        let url = serve(&m, b"b");
        let (found, _) = look(&config(&url, &public, ""), "1.0.0", t.path(), t.path()).unwrap();
        assert!(said(&found).contains("prerelease = true"), "{found:?}");
        let (found, _) = look(&config(&url, &public, "prerelease = true"), "1.0.0", t.path(), t.path()).unwrap();
        assert!(matches!(found, Found::Offer(o) if o.version == "1.1.0-rc.1"));
    }

    #[test]
    fn a_release_that_cant_read_the_data_here_is_offered_as_blocked() {
        let (k, public) = up::keygen().unwrap();
        let t = tempfile::tempdir().unwrap();
        // The data here is notes v3.
        let h = aio::Handle::<Notes>::open(t.path().join("state.db")).unwrap();
        h.add().unwrap();
        drop(h);
        let old = up::Contract { schema: up::contract().schema, cells: [("notes".to_string(), 2)].into() };
        let m = release(&k, "1.1.0", b"b", old);
        let (found, m) = look(&config(&serve(&m, b"b"), &public, ""), "1.0.0", t.path(), t.path()).unwrap();
        // Its manifest is kept for apply_anyway / start_fresh; `check` downloads only offers.
        assert_eq!(m.map(|m| m.release.version).as_deref(), Some("1.1.0"));
        let Found::Blocked(b) = found else { panic!("{found:?}") };
        assert_eq!(b.blockers, ["notes: the data is v3, the release reads up to v2"]);
    }

    #[test]
    fn an_android_release_is_the_link_to_its_apk() {
        let (k, _) = up::keygen().unwrap();
        let mut r = release(&k, "1.1.0", b"b", up::Contract::default()).release;
        r.file = "notes-1.1.0-android-arm64-v8a.apk".into();
        let b = apk(&r, &up::release_dir("github:me/notes", "stable"));
        assert_eq!(b.version, "1.1.0");
        assert_eq!(
            b.blockers,
            ["1.1.0 is available as an Android package, and an app can't replace its own: open \
                 https://github.com/me/notes/releases/latest/download/notes-1.1.0-android-arm64-v8a.apk on the device \
                 and tap it to update"]
        );
    }

    #[test]
    fn an_unchanged_manifest_is_not_sent_again_and_is_judged_anew() {
        use std::io::{BufRead, Write};
        let (k, public) = up::keygen().unwrap();
        let t = tempfile::tempdir().unwrap();
        let body = aio::serde_json::to_vec(&release(&k, "1.1.0", b"b", up::Contract::default())).unwrap();
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let asked = Arc::new(std::sync::Mutex::new(vec![]));
        let seen = asked.clone();
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                let mut lines = io::BufReader::new(&s).lines().map_while(Result::ok);
                let tag = lines
                    .by_ref()
                    .take_while(|l| !l.is_empty())
                    .find_map(|l| l.to_lowercase().strip_prefix("if-none-match: ").map(String::from));
                seen.lock().unwrap().push(tag.clone());
                let mut w = &s;
                if tag.as_deref() == Some("\"r1\"") {
                    _ = write!(w, "HTTP/1.1 304 Not Modified\r\netag: \"r1\"\r\nconnection: close\r\n\r\n");
                } else {
                    _ = write!(
                        w,
                        "HTTP/1.1 200 OK\r\netag: \"r1\"\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    );
                    _ = w.write_all(&body);
                }
            }
        });
        let cfg = config(&url, &public, "");
        for current in ["1.0.0", "1.0.0", "1.1.0"] {
            let (found, _) = look(&cfg, current, t.path(), t.path()).unwrap();
            assert_eq!(matches!(found, Found::Offer(_)), current == "1.0.0", "{found:?}");
        }
        assert_eq!(*asked.lock().unwrap(), [None, Some("\"r1\"".into()), Some("\"r1\"".into())]);
        // Another channel's (another URL's) copy is never used.
        assert_eq!(
            read_json::<aio::serde_json::Value>(&t.path().join("manifest.json")).unwrap()["url"],
            format!("{url}/stable/{}.json", platform())
        );
    }

    #[test]
    fn a_host_that_stops_sending_is_given_up_and_a_full_disk_is_said_before_the_offer() {
        use std::io::Write;
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/x", l.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            _ = s.read(&mut [0; 4096]);
            _ = s.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 100\r\n\r\nhalf");
            std::thread::sleep(Duration::from_secs(60));
        });
        let t0 = std::time::Instant::now();
        let e = get(&url, 100, None, Duration::from_millis(300), &|_| {}).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::TimedOut, "{e}");
        // Well short of the stall: given up at its timeout, not when the server let go.
        assert!(t0.elapsed() < Duration::from_secs(20) && e.to_string().contains("sent nothing"));
        let t = tempfile::tempdir().unwrap();
        let e = room(&t.path().join("versions"), 100 << 20, Some(10 << 20)).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::StorageFull);
        assert!(e.to_string().contains("needs about 300 MB free") && e.to_string().contains("10 MB are"), "{e}");
        room(&t.path().join("versions"), 100 << 20, Some(1 << 40)).unwrap();
        assert!(room(&t.path().join("versions"), 1, None).is_ok(), "an unknown is no refusal");
        assert!(free_bytes(t.path()).is_some_and(|f| f > 0));
        assert_eq!(std::fs::read_dir(t.path().join("versions")).unwrap().count(), 0, "the probe is gone");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let ro = t.path().join("ro");
            std::fs::create_dir(&ro).unwrap();
            std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o500)).unwrap();
            // Root writes anyway: only a refusal is checked.
            if let Err(e) = room(&ro, 1, None) {
                assert!(e.to_string().contains("can't be written"), "{e}");
            }
            std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
    }

    #[test]
    fn failed_checks_back_off_to_a_day_at_most() {
        // aio tests: updateBackoffMs.
        let (m, h) = (Duration::from_secs(60), Duration::from_secs(3600));
        assert_eq!(backoff(h * 6, 0), h * 6);
        assert_eq!(backoff(m, 1), m * 2);
        assert_eq!(backoff(m, 6), m * 60, "a minute's poll backs off to the hour");
        assert_eq!(backoff(m, 30), m * 60);
        assert_eq!(backoff(h * 6, 1), h * 12);
        assert_eq!(backoff(h * 6, 9), h * 24, "never past a day");
        assert_eq!(backoff(h * 48, 3), h * 48, "never sooner than every");
    }

    #[test]
    fn a_download_reports_progress_and_must_be_the_signed_bundle() {
        let (k, public) = up::keygen().unwrap();
        let t = tempfile::tempdir().unwrap();
        let bundle = vec![7u8; 300_000];
        let m = release(&k, "1.1.0", &bundle, up::Contract::default());
        let cfg = config(&serve(&m, &bundle), &public, "");
        let seen = std::sync::Mutex::new(vec![]);
        let got = fetch(
            &format!("{}/stable/{}", cfg.updates.url.as_deref().unwrap(), m.release.file),
            m.release.size,
            &|p| seen.lock().unwrap().push(p),
        )
        .unwrap();
        assert_eq!(got, bundle);
        let seen = seen.into_inner().unwrap();
        assert!(seen.windows(2).all(|w| w[0] < w[1]) && seen.last() == Some(&100), "{seen:?}");
        // Other bytes than the signed ones are refused, and nothing is staged.
        let cfg = config(&serve(&m, &vec![8u8; 300_000]), &public, "");
        let e = download(&cfg, &m, t.path(), "1.0.0", &|_| {}).unwrap_err();
        assert!(e.to_string().contains("not the signed bundle"), "{e}");
        assert!(!t.path().join("versions/1.1.0").exists());
        // A bigger body than signed stops at the cap.
        assert!(
            fetch(&format!("{}/stable/{}", cfg.updates.url.as_deref().unwrap(), m.release.file), 10, &|_| {}).is_err()
        );
    }

    /// An unpacked release of notes `version`: its binary says its version.
    #[cfg(target_os = "linux")]
    fn unpacked(dir: &Path, version: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(dir).unwrap();
        // Padding a binary would have: the patch is far smaller than the release.
        let pad: String =
            (0..20_000u32).map(|i| char::from(b'a' + (i.wrapping_mul(2_654_435_761) >> 27) as u8 % 26)).collect();
        std::fs::write(dir.join("notes"), format!("#!/bin/sh\necho notes {version}\n# {pad}\n")).unwrap();
        std::fs::set_permissions(dir.join("notes"), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(dir.join("notes.png"), b"icon").unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_release_is_made_from_the_version_here_and_a_patch_else_downloaded_whole() {
        let (k, public) = up::keygen().unwrap();
        let t = tempfile::tempdir().unwrap();
        let (d, new) = (t.path().join("install"), t.path().join("new"));
        unpacked(&d.join("versions/1.0.0"), "1.0.0");
        unpacked(&new, "1.1.0");
        let patch = crate::delta::make(&d.join("versions/1.0.0"), &new).unwrap();
        // The bundle served is not the release: a whole download fails, so only a patch can stage it.
        let bundle = vec![0u8; 100_000];
        let m = release(&k, "1.1.0", &bundle, up::Contract::default());
        let entry = up::Patch {
            from: "1.0.0".into(),
            file: "notes-1.1.0-from-1.0.0.patch".into(),
            size: patch.len() as u64,
            sha256: aio::sha256::hex(&Sha256::digest(&patch)),
        };
        let list = up::sign_patches(&m, vec![entry.clone()], &k).unwrap();
        let host_with = |list: Option<&up::Patches>, patch: &[u8]| {
            let mut files = vec![
                (format!("/stable/{}.json", platform()), aio::serde_json::to_vec(&m).unwrap()),
                (format!("/stable/{}", m.release.file), vec![1u8; 100_000]),
                (format!("/stable/{}", entry.file), patch.to_vec()),
            ];
            if let Some(l) = list {
                files.push((format!("/stable/{}.patches.json", platform()), aio::serde_json::to_vec(l).unwrap()));
            }
            config(&host(files), &public, "")
        };
        let cfg = host_with(Some(&list), &patch);
        download(&cfg, &m, &d, "1.0.0", &|_| {}).unwrap();
        let staged = d.join("versions/1.1.0");
        assert_eq!(std::fs::read(staged.join("notes")).unwrap(), std::fs::read(new.join("notes")).unwrap());
        assert_eq!(
            read_json::<aio::serde_json::Value>(&staged.join("release.json")).unwrap()["release"]["version"],
            "1.1.0"
        );
        std::fs::remove_dir_all(&staged).unwrap();
        // No patch from the version here, a list without one, one that isn't the signed one,
        // or an installed file changed since: the whole release, which (here) is refused.
        let whole = |cfg: &aio::config::Config, from: &str| {
            let e = download(cfg, &m, &d, from, &|_| {}).unwrap_err().to_string();
            assert!(e.contains("not the signed bundle"), "{e}");
            assert!(!staged.exists());
        };
        whole(&host_with(None, &patch), "1.0.0");
        whole(&cfg, "0.9.0");
        whole(&host_with(Some(&list), b"other bytes"), "1.0.0");
        std::fs::write(d.join("versions/1.0.0/notes.png"), b"edited").unwrap();
        whole(&cfg, "1.0.0");
    }

    /// The cell on the built-in runtime: it offers what the host has, and a
    /// dev build says why it can't install.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_cell_hears_what_the_built_in_runtime_finds() {
        // Its look stages into `install/notes` of the tool home: the test world's, never the person's.
        let _world = aio::paths::sandbox().unwrap();
        let (k, public) = up::keygen().unwrap();
        let t = tempfile::tempdir().unwrap();
        let m = release(&k, "1.1.0", b"b", up::Contract::default());
        let cfg = config(&serve(&m, b"b"), &public, "auto = false");
        up::install(Arc::new(Shell { cfg, version: "1.0.0".into(), data: t.path().into() }));
        let h = aio::Handle::<Updates>::memory();
        h.ready(Ready { channel: "stable".into(), current: "1.0.0".into(), ..Ready::default() }).unwrap();
        h.check().unwrap();
        h.done("check").await;
        let s = h.state();
        assert!(matches!(&s.available, Some(o) if o.version == "1.1.0" && !o.staged), "{s:?}");
        h.apply().unwrap();
        h.done("apply").await;
        let s = h.state();
        assert_eq!(s.phase, up::Phase::Error);
        assert!(s.error.as_deref().is_some_and(|e| e.contains("not an installed copy")), "{s:?}");
    }

    /// A shell in `dir` that must succeed: the test's own git runs.
    #[cfg(target_os = "linux")]
    fn sh(dir: &Path, script: &str) -> String {
        let out = Command::new("sh").arg("-c").arg(script).current_dir(dir).output().unwrap();
        assert!(out.status.success(), "{script}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// What `ax build` makes of a notes clone: a bundle whose binary is
    /// notes 1.0.<its commit count> and says what data it reads.
    #[cfg(target_os = "linux")]
    fn fake_build(src: &Path) -> io::Result<PathBuf> {
        use std::os::unix::fs::PermissionsExt;
        let v = format!("1.0.{}", sh(src, "git rev-list --count HEAD"));
        let top = src.join("dist/notes-x");
        std::fs::create_dir_all(&top)?;
        let script = format!(
            "#!/bin/sh\n[ \"$1\" = --version ] && echo notes {v} && exit 0\nprintf '{{\"schema\":1,\"cells\":{{}}}}' > \"$AIORT_DATA_CONTRACT\"\n"
        );
        std::fs::write(top.join("notes"), script)?;
        std::fs::set_permissions(top.join("notes"), std::fs::Permissions::from_mode(0o755))?;
        let bundle = src.join(format!("dist/notes-{v}-linux-x64.tar.gz"));
        sh(&src.join("dist"), &format!("tar -czf {} notes-x", bundle.display()));
        Ok(bundle)
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_git_source_builds_only_what_a_built_in_key_signed() {
        let t = tempfile::tempdir().unwrap();
        let (origin, data, d) = (t.path().join("origin"), t.path().join("data"), t.path().join("install"));
        for p in [&origin, &data, &d] {
            std::fs::create_dir_all(p).unwrap();
        }
        let key = |name: &str| {
            sh(t.path(), &format!("ssh-keygen -q -t ed25519 -N '' -C '' -f {name}"));
            t.path().join(name)
        };
        let (ours, theirs) = (key("ours"), key("theirs"));
        let trusted = std::fs::read_to_string(ours.with_extension("pub")).unwrap().trim().to_string();
        let git = "git -c user.name=t -c user.email=t@example.org -c gpg.format=ssh -c commit.gpgsign=false \
                   -c tag.gpgsign=false";
        let commit = |key: Option<&Path>| {
            let sign = key.map_or(String::new(), |k| format!("-c user.signingkey={} ", k.display()));
            sh(
                &origin,
                &format!(
                    "echo x >> f && git add f && {git} {sign}commit -q {} -m c",
                    ["", "-S"][key.is_some() as usize]
                ),
            );
        };
        sh(&origin, "git init -q -b main");
        let cfg = |ch: &str| {
            aio::config::Config::parse(&format!(
                "[app]\nid = \"notes\"\n[updates]\nurl = \"git+file://{}\"\nkeys = [\"{trusted}\"]\nchannel = \"{ch}\"",
                origin.display()
            ))
            .unwrap()
        };
        let remote = format!("file://{}", origin.display());
        let builds = std::sync::atomic::AtomicU32::new(0);
        let build = |src: &Path| {
            builds.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            fake_build(src)
        };
        let look = |ch: &str, current: &str| look_git(&cfg(ch), current, &data, &d, &remote, &build);
        // Signed by the key built in: built once, staged, offered with its signer.
        commit(Some(&ours));
        let (found, m) = look("main", "1.0.0").unwrap();
        let Found::Offer(o) = found else { panic!("{found:?}") };
        assert_eq!((o.version.as_str(), o.staged, o.size, o.key.as_str()), ("1.0.1", true, 0, trusted.as_str()));
        assert!(m.unwrap().release.file.starts_with("git:"));
        assert!(d.join("versions/1.0.1/notes").is_file() && !d.join("git-build").exists());
        assert!(matches!(look("main", "1.0.0").unwrap().0, Found::Offer(o) if o.version == "1.0.1"));
        assert_eq!(builds.load(std::sync::atomic::Ordering::SeqCst), 1, "a commit is built once");
        // Unsigned, or signed by any other key: refused, never built, and remembered.
        commit(None);
        let e = look("main", "1.0.0").unwrap_err().to_string();
        assert!(e.starts_with("release refused:") && e.contains("signed by a key this app trusts"), "{e}");
        assert!(look("main", "1.0.0").unwrap_err().to_string().starts_with("release refused:"));
        commit(Some(&theirs));
        assert!(look("main", "1.0.0").unwrap_err().to_string().starts_with("release refused:"));
        assert_eq!(builds.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(!d.join("versions/1.0.2").exists() && !d.join("versions/1.0.3").exists());
        // An unsigned commit under a tag the built-in key signed: the tag vouches for it.
        commit(None);
        sh(&origin, &format!("{git} -c user.signingkey={} tag -s v2 -m v2", ours.display()));
        let (found, _) = look("v2", "1.0.0").unwrap();
        assert!(matches!(&found, Found::Offer(o) if o.version == "1.0.4" && o.key == trusted), "{found:?}");
        // A tag signed by another key vouches for nothing.
        commit(None);
        sh(&origin, &format!("{git} -c user.signingkey={} tag -s v3 -m v3", theirs.display()));
        assert!(look("v3", "1.0.0").unwrap_err().to_string().starts_with("release refused:"));
        // Not newer than what runs: said, and nothing is staged over it.
        commit(Some(&ours));
        let (found, _) = look("main", "9.0.0").unwrap();
        assert!(said(&found).contains("9.0.0 is current"), "{found:?}");
        assert!(!d.join("versions/1.0.6").exists());
        // A ref that isn't there, named exactly, is an error before any clone.
        assert!(look("mai", "1.0.0").unwrap_err().to_string().contains("exactly \"mai\""));
    }

    /// `ax update --from-git`: what the update checks refuse (an unsigned
    /// commit) is staged when asked for by hand, marked unsigned, and only
    /// when it is newer and can read the data.
    #[cfg(target_os = "linux")]
    #[test]
    fn from_git_stages_an_unsigned_ref_only_when_asked_by_hand() {
        let t = tempfile::tempdir().unwrap();
        let (origin, data, d) = (t.path().join("origin"), t.path().join("data"), t.path().join("install"));
        for p in [&origin, &data, &d] {
            std::fs::create_dir_all(p).unwrap();
        }
        sh(&origin, "git init -q -b main && echo x > f && git add f");
        sh(&origin, "git -c user.name=t -c user.email=t@example.org -c commit.gpgsign=false commit -q -m c");
        let remote = format!("file://{}", origin.display());
        sh(t.path(), "ssh-keygen -q -t ed25519 -N '' -C '' -f ours");
        let key = std::fs::read_to_string(t.path().join("ours.pub")).unwrap();
        let cfg = aio::config::Config::parse(&format!(
            "[app]\nid = \"notes\"\n[updates]\nurl = \"git+{remote}\"\nchannel = \"main\"\nkeys = [\"{}\"]",
            key.trim()
        ))
        .unwrap();
        let e = look_git(&cfg, "1.0.0", &data, &d, &remote, &fake_build).unwrap_err().to_string();
        assert!(e.starts_with("release refused:"), "the update checks never take it: {e}");
        _ = std::fs::remove_file(d.join("git.json"));
        let said = from_git(&cfg, (&remote, "main"), &d, &data, "1.0.0", &fake_build).unwrap();
        assert!(said.starts_with("1.0.1 (UNSIGNED:") && said.contains("installs it at its next start"), "{said}");
        let m: Manifest = read_json(&d.join("versions/1.0.1/release.json")).unwrap();
        assert_eq!(m.key, UNSIGNED);
        assert!(m.release.notes.starts_with("UNSIGNED:"), "{}", m.release.notes);
        assert_eq!(staged(&d, "1.0.0").unwrap().release.version, "1.0.1", "the next start installs it");
        assert!(!d.join("git-build").exists() && !d.join("git.json").exists());
        let e = from_git(&cfg, (&remote, "main"), &d, &data, "1.0.1", &fake_build).unwrap_err().to_string();
        assert!(e.contains("not newer than the installed 1.0.1"), "{e}");
        let e = from_git(&cfg, (&remote, "nope"), &d, &data, "1.0.0", &fake_build).unwrap_err().to_string();
        assert!(e.contains("\"nope\""), "{e}");
    }

    #[test]
    fn a_fresh_build_s_bundle_is_the_one_this_machine_installs() {
        let names: Vec<String> =
            ["n-1-windows-x64.exe", "n-1-windows-x64.zip", "n-1-linux-x64.tar.gz", "n-1-linux-x64.AppImage"]
                .map(String::from)
                .into();
        assert_eq!(pick_bundle(&names, "windows", false).unwrap(), "n-1-windows-x64.zip");
        assert_eq!(pick_bundle(&names, "linux", false).unwrap(), "n-1-linux-x64.tar.gz");
        assert_eq!(pick_bundle(&names, "linux", true).unwrap(), "n-1-linux-x64.AppImage");
        assert_eq!(pick_bundle(&names[..1], "windows", false), None);
    }
}

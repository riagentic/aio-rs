//! `ax build <app> --smoke`: the bundle just built, unpacked into a
//! throwaway sandbox and started as a user would, on a display no one sees
//! (Xvfb; without it the runtime's own headless display, never Xephyr on
//! the person's screen; or `AIORT_SMOKE_DISPLAY`) with a private D-Bus
//! (`display::bus`), so no dialog reaches the user's screen. It must open its window
//! within [`BOOT`], stop on SIGTERM within [`STOP`], leave no process behind
//! and log no panic, problem report or degraded part. A server ([`Wait::Boot`])
//! starts with no display on a free port and must boot instead; a CLI
//! ([`Wait::Answer`]) must answer `--version` with its id and exit 0. The
//! sandbox, display and everything started go away again, pass or fail.
//!
//! A macOS app's `.zip` is smoke-tested on the Mac that built it, its
//! `.app` started from the sandbox as from Applications: there is no hidden
//! display on a Mac, so its window shows on the Mac's screen until it is
//! asked to stop (aio's `desktop` mode).
//!
//! A Windows bundle (`.zip`) is smoke-tested on Windows: this one, or the
//! machine `AIO_WINDOWS_SSH` names (`[ssh options] [user@]host`, its
//! PowerShell; nothing to install there). It stops as a user's would, asked
//! through its door (`<app> --quit`: Windows has no SIGTERM).
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub const BOOT: Duration = Duration::from_secs(90);
pub const STOP: Duration = Duration::from_secs(20);
pub const READY: &str = "[aio] window ready";

/// How long after its window is ready an app's memory is read: its first
/// frames drawn, its start-up work done.
const SETTLED: Duration = Duration::from_secs(3);

/// What a smoke test waits for, by the app's mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wait {
    /// Its window's page loaded.
    Window,
    /// A server: booted (its health file names it).
    Boot,
    /// A CLI: `--version` answered.
    Answer,
}

impl Wait {
    pub fn of(mode: &aio::config::Mode) -> Wait {
        match mode {
            aio::config::Mode::Server => Wait::Boot,
            aio::config::Mode::Cli => Wait::Answer,
            _ => Wait::Window,
        }
    }

    /// The arguments it starts with: a server on a free port, so a running
    /// one is never in the way.
    fn args(self) -> &'static [&'static str] {
        match self {
            Wait::Window => &[],
            Wait::Boot => &["--port=0"],
            Wait::Answer => &["--version"],
        }
    }
}

/// Log lines that fail a smoke test.
pub fn bad_lines(log: &str) -> Vec<&str> {
    // A page script that throws (the shell logs it `ERROR [page] Uncaught …`).
    const BAD: [&str; 5] = ["panicked at", "problem report written", " is degraded", "load failed", "[page] Uncaught"];
    log.lines().filter(|l| BAD.iter().any(|b| l.contains(b))).collect()
}

/// The first display number from 90 that `taken` says is free.
pub fn free_display(taken: impl Fn(u32) -> bool) -> Option<u32> {
    (90..200).find(|n| !taken(*n))
}

/// Everything the smoke test made: removed and stopped when dropped.
struct Sandbox {
    dir: PathBuf,
    /// The display server started here.
    display: Option<crate::display::Nested>,
    /// The private D-Bus, with `AIORT_SMOKE_DISPLAY`.
    bus: Option<crate::display::Bus>,
    app: Option<Child>,
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        if let Some(c) = self.app.as_mut() {
            _ = c.kill();
            _ = c.wait();
        }
        self.display = None;
        self.bus = None;
        _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn on_path(exe: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(exe).is_file()))
}

/// The runtime's own display, none at all (Chromium's headless ozone):
/// for a machine with no Xvfb.
const HEADLESS: &str = "--ozone-platform=headless";

/// Put `cmd` on a display for the app: `AIORT_SMOKE_DISPLAY`, one started
/// here (Xvfb), else the runtime's headless one; on a private D-Bus, or none.
fn display(sb: &mut Sandbox, cmd: &mut Command) -> Result<(), String> {
    if let Ok(d) = std::env::var("AIORT_SMOKE_DISPLAY")
        && !d.is_empty()
    {
        sb.bus = crate::display::bus(None);
        crate::display::onto(cmd, Some(&d));
        crate::display::contain(cmd);
        crate::display::on_bus(cmd, sb.bus.as_ref());
        return Ok(());
    }
    // A Mac has one screen: the window shows there (the module's doc).
    if cfg!(target_os = "macos") {
        return Ok(());
    }
    if !on_path("Xvfb") {
        let inherited = std::env::var("AIORT_ELECTRON_ARGS").unwrap_or_default();
        cmd.env_remove("DISPLAY").env_remove("WAYLAND_DISPLAY");
        cmd.env("AIORT_ELECTRON_ARGS", format!("{HEADLESS} {inherited}").trim_end());
        crate::display::contain(cmd);
        crate::display::on_bus(cmd, None);
        return Ok(());
    }
    let nested = crate::display::start(false, false)?;
    nested.onto(cmd, std::env::var("AIORT_ELECTRON_ARGS").ok().as_deref());
    sb.display = Some(nested);
    Ok(())
}

/// The pids in `ps -axww -o pid=,command=` output whose command line names
/// `needle` (pure).
pub fn pids_naming(listing: &str, needle: &str) -> Vec<u32> {
    listing
        .lines()
        .filter_map(|l| l.trim_start().split_once(' '))
        .filter(|(_, cmd)| cmd.contains(needle))
        .filter_map(|(pid, _)| pid.parse().ok())
        .collect()
}

/// Processes whose command line names `dir` (the app's, the runtime's):
/// through `ps`, as Linux and macOS both have it (macOS has no `/proc`).
fn left_in(dir: &Path) -> Vec<u32> {
    let out = Command::new("ps").args(["-axww", "-o", "pid=,command="]).output();
    out.map(|o| pids_naming(&String::from_utf8_lossy(&o.stdout), &dir.to_string_lossy())).unwrap_or_default()
}

/// How a Linux artifact of app `id` is unpacked (the command, run in the
/// unpack dir) and where its program then is, relative to that dir: a
/// bundle (`.tar.gz`), a Debian package, an AppImage (extracted, so nothing
/// is mounted or left in /tmp). None: not one smoke can start.
pub fn unpack_plan(artifact: &Path, id: &str) -> Option<(Vec<String>, PathBuf)> {
    let file = artifact.to_string_lossy().into_owned();
    let name = artifact.file_name()?.to_str()?;
    if let Some(dir) = name.strip_suffix(".tar.gz") {
        return Some((vec!["tar".into(), "-xzf".into(), file, "-C".into(), ".".into()], Path::new(dir).join(id)));
    }
    if name.ends_with(".deb") {
        return Some((vec!["dpkg-deb".into(), "-x".into(), file, ".".into()], Path::new("usr/lib").join(id).join(id)));
    }
    // A macOS app's zip (`ditto`): its one `.app`, whatever it is named ([`APP`]).
    if name.ends_with(".zip") && name.contains("-macos-") {
        let ditto = ["/usr/bin/ditto", "-x", "-k"].map(String::from);
        return Some((
            ditto.into_iter().chain([file, ".".into()]).collect(),
            Path::new(APP).join("Contents/MacOS").join(id),
        ));
    }
    name.ends_with(".AppImage")
        .then(|| (vec![file, "--appimage-extract".into()], PathBuf::from("squashfs-root/AppRun")))
}

/// In an unpack plan: the one `.app` unpacked, by any name.
const APP: &str = "*.app";

/// `rel` (from [`unpack_plan`]) in the folder `unpacked`: [`APP`] is the
/// `.app` there.
fn unpacked(unpacked: &Path, rel: &Path) -> PathBuf {
    let Ok(rest) = rel.strip_prefix(APP) else { return unpacked.join(rel) };
    let app = std::fs::read_dir(unpacked)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "app"));
    app.unwrap_or_else(|| unpacked.join(APP)).join(rest)
}

/// How an app was asked to stop, and how that went.
#[derive(Clone, Debug, PartialEq)]
pub enum Stop {
    /// It never ran (no boot), or it was a CLI (it exited by itself).
    NotTried,
    /// No pid in its `data/health.json`: nothing to ask.
    NoPid,
    /// It exited this long after it was asked, with this status (`Err`:
    /// not 0, as said), its health file `left` behind or not.
    Exited { after: Duration, status: Result<(), String>, left: bool },
    /// Still running [`STOP`] after it was asked.
    Hung { left: bool },
}

/// What one smoke run saw, here or on another machine.
#[derive(Clone, Debug, PartialEq)]
pub struct Seen {
    /// How long it took to boot: its window, its health file, its exit.
    pub booted: Option<Duration>,
    /// A CLI's exit (`Ok`: 0; `Err`: the status, as said) and what it printed.
    pub answer: Option<(Result<(), String>, String)>,
    pub stop: Stop,
    /// Pids still running from its sandbox, a few seconds after.
    pub left: Vec<u32>,
    /// Its log.
    pub log: String,
    /// What its process tree held once its window was up (Linux).
    pub memory: Option<Memory>,
}

/// A process tree's memory (the app and every process under it: the
/// runtime's browser, GPU, network, renderer…), in bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct Memory {
    /// The app's own process (its resident memory: an aio-rs app's smaps is closed).
    pub app: u64,
    /// PSS: each shared page split among the processes that map it, so it
    /// shrinks while other apps run the same runtime.
    pub pss: u64,
    /// Pages no other process maps: what this app adds while nothing else
    /// runs on its runtime (another app sharing it makes the runtime's file
    /// pages shared: some 40 MB less).
    pub private: u64,
    /// The private pages it wrote: what it adds whatever else runs.
    pub dirty: u64,
    pub processes: usize,
}

/// The app id in an artifact's name as `ax build` names it
/// (`<id>-<version>-…`), pure.
pub fn id_of(artifact: &Path) -> Option<&str> {
    let name = artifact.file_name()?.to_str()?;
    // Debian's own naming: `<id>_<version>_<arch>.deb`.
    if let Some(stem) = name.strip_suffix(".deb") {
        return stem.split_once('_').map(|(id, _)| id).filter(|id| !id.is_empty());
    }
    let at = name.match_indices('-').find(|(i, _)| name[i + 1..].starts_with(|c: char| c.is_ascii_digit()))?.0;
    Some(&name[..at]).filter(|id| !id.is_empty())
}

/// PSS, private and private dirty bytes of a `/proc/<pid>/smaps_rollup` (pure).
pub fn pss(rollup: &str) -> Option<(u64, u64, u64)> {
    let kb = |k: &str| -> Option<u64> {
        rollup.lines().find_map(|l| l.strip_prefix(k))?.split_whitespace().next()?.parse::<u64>().ok().map(|v| v * 1024)
    };
    let dirty = kb("Private_Dirty:")?;
    Some((kb("Pss:")?, kb("Private_Clean:")? + dirty, dirty))
}

/// `root` and every pid under it, given each pid's parent (pure).
pub fn tree(root: u32, parents: &[(u32, u32)]) -> Vec<u32> {
    let mut all = vec![root];
    let mut i = 0;
    while i < all.len() {
        let p = all[i];
        all.extend(parents.iter().filter(|(_, pp)| *pp == p).map(|(c, _)| *c));
        i += 1;
    }
    all
}

/// The parent of the process a `/proc/<pid>/stat` line is about (pure).
fn parent(stat: &str) -> Option<u32> {
    stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()
}

/// What `root`'s tree holds now (Linux; `None` elsewhere or when gone).
fn memory(root: u32) -> Option<Memory> {
    let parents: Vec<(u32, u32)> = std::fs::read_dir("/proc")
        .ok()?
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .filter_map(|pid| Some((pid, parent(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)?)))
        .collect();
    // An aio-rs app is not dumpable (`lifecycle`): its own smaps is closed to
    // its user, so its resident pages stand in (its statm is open).
    let rss = |pid: u32| {
        let pages: u64 = std::fs::read_to_string(format!("/proc/{pid}/statm")).ok()?.split(' ').nth(1)?.parse().ok()?;
        #[cfg(unix)]
        // SAFETY: sysconf only reads a constant.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as u64;
        #[cfg(not(unix))]
        let page = 4096;
        Some((pages * page, pages * page, pages * page))
    };
    let of = |pid: u32| {
        std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).ok().as_deref().and_then(pss).or_else(|| rss(pid))
    };
    let pids = tree(root, &parents);
    let all: Vec<(u64, u64, u64)> = pids.iter().filter_map(|p| of(*p)).collect();
    Some(Memory {
        app: of(root)?.0,
        pss: all.iter().map(|m| m.0).sum(),
        private: all.iter().map(|m| m.1).sum(),
        dirty: all.iter().map(|m| m.2).sum(),
        processes: pids.len(),
    })
}

/// The smoke report for app `id` from what was `seen`; `how` it was asked
/// to stop ("SIGTERM", "--quit"): `Err` when a row failed, with its log's
/// tail. Pure.
pub fn report(id: &str, wait: Wait, seen: &Seen, how: &str) -> Result<String, String> {
    let mut rows = vec![];
    let mut failed = false;
    let mut row = |ok: bool, text: String| {
        failed |= !ok;
        rows.push(format!("  {} {text}", if ok { "✓" } else { "✗" }));
    };
    let secs = |t: Duration| t.as_secs_f64();
    match (wait, seen.booted) {
        (Wait::Window, Some(t)) => row(true, format!("boots: window ready in {:.1} s", secs(t))),
        (Wait::Window, None) => row(false, format!("boots: no window within {} s", BOOT.as_secs())),
        (Wait::Boot, Some(t)) => row(true, format!("boots: serving in {:.1} s", secs(t))),
        (Wait::Boot, None) => row(false, format!("boots: not booted within {} s", BOOT.as_secs())),
        (Wait::Answer, _) => {
            let (ok, how, said) = match &seen.answer {
                Some((st, said)) => {
                    let shown = st.as_ref().map_or_else(Clone::clone, |()| "exit status: 0".into());
                    (st.is_ok() && said.split_whitespace().next() == Some(id), shown, said.trim())
                }
                None => (false, format!("no exit within {} s", BOOT.as_secs()), ""),
            };
            row(ok, format!("answers --version with its id ({how}: {said:?})"));
        }
    }
    match &seen.stop {
        Stop::NotTried => {}
        Stop::NoPid => row(false, "stops: no pid in data/health.json".into()),
        Stop::Exited { after, status, left } => {
            match status {
                Ok(()) => row(true, format!("stops on {how} in {:.1} s", secs(*after))),
                Err(st) => row(false, format!("stops on {how}, but {st}")),
            }
            row(!left, "stops cleanly (no health file left)".into());
        }
        Stop::Hung { left } => {
            row(false, format!("does not stop within {} s of {how}", STOP.as_secs()));
            row(!left, "stops cleanly (no health file left)".into());
        }
    }
    let left = &seen.left;
    row(
        left.is_empty(),
        format!(
            "leaves no process behind{}",
            if left.is_empty() { String::new() } else { format!(" ({left:?} left)") }
        ),
    );
    let bad = bad_lines(&seen.log);
    row(bad.is_empty(), "logs no panic, problem report or degraded part".into());
    if let Some(m) = &seen.memory {
        let mb = |b: u64| b as f64 / 1e6;
        row(
            true,
            format!(
                "memory: {:.0} MB private, {:.0} MB PSS in {} processes (the app {:.1} MB), {:.0} MB written",
                mb(m.private),
                mb(m.pss),
                m.processes,
                mb(m.app),
                mb(m.dirty)
            ),
        );
    }
    let mut out = format!("smoke {id}:\n{}", rows.join("\n"));
    for l in bad.iter().take(10) {
        out.push_str(&format!("\n    {l}"));
    }
    if failed {
        let tail: Vec<&str> = seen.log.lines().rev().take(30).collect();
        out.push_str(&format!(
            "\n  log (newest last):\n    {}",
            tail.into_iter().rev().collect::<Vec<_>>().join("\n    ")
        ));
        return Err(out);
    }
    Ok(out)
}

/// Can smoke start `artifact` of app `id` on an `os` machine? Any Linux
/// artifact on Linux; on macOS a server's or CLI's bundle, or an app's zip
/// (its `.app`). Pure.
pub fn startable(os: &str, artifact: &Path, id: &str, wait: Wait) -> bool {
    let mac_zip = |name: &str| name.ends_with(".zip") && name.contains("-macos-");
    match os {
        "linux" => !mac_zip(&artifact.to_string_lossy()) && unpack_plan(artifact, id).is_some(),
        "macos" => {
            let name = artifact.to_string_lossy();
            match wait {
                Wait::Window => mac_zip(&name),
                _ => name.ends_with(".tar.gz"),
            }
        }
        "windows" => is_windows_zip(artifact),
        _ => false,
    }
}

/// A Windows bundle, as `build` packs it (`<id>-<version>-windows-<arch>.zip`).
pub fn is_windows_zip(artifact: &Path) -> bool {
    artifact.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(".zip") && n.contains("-windows-"))
}

/// What is wrong with the desktop entry of a Debian package of app `id`
/// unpacked at `root`: it is missing, its `Exec` program is not in the
/// package, or no icon of its `Icon` name is. Nothing: it would start
/// from the menu, with its icon.
pub fn desktop_problems(root: &Path, id: &str) -> Vec<String> {
    let rel = format!("usr/share/applications/{id}.desktop");
    let Ok(entry) = std::fs::read_to_string(root.join(&rel)) else { return vec![format!("no {rel}")] };
    let key = |k: &str| entry.lines().find_map(|l| l.strip_prefix(k)).map(str::trim);
    let inside = |p: &str| root.join(p.trim_start_matches('/'));
    let mut problems = vec![];
    let exec = key("Exec=").and_then(|e| e.split(' ').next()).map(|e| e.trim_matches('"'));
    // Its link resolved in the package, not on this machine.
    let resolves = |p: &str| {
        let at = inside(p);
        match std::fs::read_link(&at) {
            Ok(to) if to.is_absolute() => inside(&to.to_string_lossy()).is_file(),
            Ok(to) => at.parent().is_some_and(|d| d.join(to).is_file()),
            Err(_) => at.is_file(),
        }
    };
    match exec {
        Some(e) if resolves(e) => {}
        e => problems.push(format!("{rel}: its Exec program {:?} is not in the package", e.unwrap_or_default())),
    }
    let icon = key("Icon=").unwrap_or_default();
    let themed =
        |size: std::fs::DirEntry| ["png", "svg"].iter().any(|x| size.path().join(format!("apps/{icon}.{x}")).is_file());
    let found = match icon {
        "" => false,
        i if i.starts_with('/') => inside(i).is_file(),
        _ => std::fs::read_dir(root.join("usr/share/icons/hicolor")).into_iter().flatten().flatten().any(themed),
    };
    if !found {
        problems.push(format!("{rel}: no icon {icon:?} in the package"));
    }
    problems
}

/// `report` with the package's own rows: each of `problems` a failure,
/// none a pass (pure).
pub fn with_package(report: Result<String, String>, problems: &[String]) -> Result<String, String> {
    let ok = problems.is_empty();
    let rows = match ok {
        true => "  ✓ its desktop entry starts its program, with its icon".to_string(),
        false => problems.iter().map(|p| format!("  ✗ {p}")).collect::<Vec<_>>().join("\n"),
    };
    let add = |r: String| match r.split_once('\n') {
        Some((head, rest)) => format!("{head}\n{rows}\n{rest}"),
        None => format!("{r}\n{rows}"),
    };
    match report {
        Ok(r) if ok => Ok(add(r)),
        Ok(r) | Err(r) => Err(add(r)),
    }
}

/// Smoke-test `artifact` (a Linux bundle, `.deb` or AppImage of app `id`;
/// a macOS server's or CLI's bundle).
pub fn run(artifact: &Path, id: &str, wait: Wait) -> Result<String, String> {
    // From here: it is unpacked in a folder of its own.
    let artifact = &std::path::absolute(artifact).map_err(|e| format!("{}: {e}", artifact.display()))?;
    let (unpack, rel) = unpack_plan(artifact, id)
        .ok_or_else(|| format!("{}: no bundle this machine smoke-tests", artifact.display()))?;
    let dir = std::env::temp_dir().join(format!("aio-smoke-{id}-{}", std::process::id()));
    let mut sb = Sandbox { dir: dir.clone(), display: None, bus: None, app: None };
    for d in ["unpack", "root", "cwd"] {
        std::fs::create_dir_all(dir.join(d)).map_err(|e| e.to_string())?;
    }
    let ok = Command::new(&unpack[0])
        .args(&unpack[1..])
        .current_dir(dir.join("unpack"))
        .stdout(Stdio::null())
        .status()
        .map_err(|e| format!("{}: {e}", unpack[0]))?;
    if !ok.success() {
        return Err(format!("could not unpack {}", artifact.display()));
    }
    let exe = unpacked(&dir.join("unpack"), &rel);
    let root = dir.join("root");
    let mut cmd = Command::new(&exe);
    if wait == Wait::Window {
        // The shared runtime first, so boot time is the app's (once per machine).
        aio_shell::runtime::ensure("aio-rs").map_err(|e| format!("runtime: {e}"))?;
        display(&mut sb, &mut cmd)?;
    }
    let answer = dir.join("answer");
    cmd.args(wait.args())
        .current_dir(dir.join("cwd"))
        .env("AIORT_DATA", &root)
        // A throwaway copy: never offered a move to Applications (macOS).
        .env(aio_shell::macmove::ENV, "never")
        .env_remove("AIORT_CONFIG")
        .env_remove("AIORT_TOKEN")
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(&answer).map_err(|e| e.to_string())?)
        .stderr(Stdio::null());
    let started = Instant::now();
    sb.app = Some(cmd.spawn().map_err(|e| format!("{}: {e}", exe.display()))?);
    let log = root.join("logs/app.log");
    let read_log = || std::fs::read_to_string(&log).unwrap_or_default();
    let health = || {
        std::fs::read(root.join("data/health.json"))
            .ok()
            .and_then(|b| aio::serde_json::from_slice::<aio::serde_json::Value>(&b).ok())
            .and_then(|v| v["pid"].as_u64())
    };
    // Boots: the window's page loaded, the server's health file is there,
    // the CLI exited.
    let mut exit = None;
    let booted = loop {
        exit = exit.or_else(|| sb.app.as_mut().and_then(|c| c.try_wait().ok().flatten()));
        let up = match wait {
            Wait::Window => read_log().contains(READY),
            Wait::Boot => health().is_some(),
            Wait::Answer => exit.is_some(),
        };
        if up {
            break Some(started.elapsed());
        }
        if started.elapsed() > BOOT || exit.is_some() {
            break None;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    // Its weight once the window is up and has drawn (the app, the runtime).
    let memory = match (wait, booted) {
        (Wait::Window, Some(_)) => {
            std::thread::sleep(SETTLED);
            sb.app.as_ref().and_then(|c| memory(c.id()))
        }
        _ => None,
    };
    let status = |st: std::process::ExitStatus| if st.success() { Ok(()) } else { Err(st.to_string()) };
    let answer = (wait == Wait::Answer)
        .then(|| exit.map(|st| (status(st), std::fs::read_to_string(&answer).unwrap_or_default())))
        .flatten();
    // Stops (a CLI did): SIGTERM to the app (its pid from its health file).
    let stop = match (booted.is_some() && wait != Wait::Answer, health()) {
        (false, _) => Stop::NotTried,
        (true, None) => Stop::NoPid,
        (true, Some(pid)) => {
            let t = Instant::now();
            _ = Command::new("kill").arg("-TERM").arg(pid.to_string()).status();
            let child = sb.app.as_mut().ok_or("no app")?;
            let exited = loop {
                match child.try_wait() {
                    Ok(Some(st)) => break Some(st),
                    _ if t.elapsed() > STOP => break None,
                    _ => std::thread::sleep(Duration::from_millis(100)),
                }
            };
            let left = root.join("data/health.json").exists();
            match exited {
                Some(st) => Stop::Exited { after: t.elapsed(), status: status(st), left },
                None => Stop::Hung { left },
            }
        }
    };
    // Leaves nothing running (whatever is left is the test's own: stopped).
    std::thread::sleep(Duration::from_secs(3));
    let left = left_in(&dir);
    for pid in &left {
        _ = Command::new("kill").arg("-KILL").arg(pid.to_string()).status();
    }
    forget_starts(&aio_shell::runtime::root(), &dir);
    let r = report(id, wait, &Seen { booted, answer, stop, left, log: read_log(), memory }, "SIGTERM");
    match artifact.extension().is_some_and(|x| x == "deb") {
        true => with_package(r, &desktop_problems(&dir.join("unpack"), id)),
        false => r,
    }
}

/// The shared runtime store's records of the programs started from
/// `sandbox` (the smoke's own starts: `apps/<key>-<version>.json`) go, and
/// a macOS window bundle they name that no other record names.
pub fn forget_starts(store: &Path, sandbox: &Path) {
    let Ok(all) = aio_shell::prune::uses(store) else { return };
    let real = sandbox.canonicalize().ok();
    let ours = |u: &aio_shell::prune::Use| {
        let exe = Path::new(&u.exe);
        exe.starts_with(sandbox) || real.as_ref().is_some_and(|r| exe.starts_with(r))
    };
    for (file, u, _) in &all {
        let Some(u) = u.as_ref().filter(|u| ours(u)) else { continue };
        _ = std::fs::remove_file(file);
        let named_else = |w: &String| {
            all.iter().any(|(_, o, _)| o.as_ref().is_some_and(|o| !ours(o) && o.window.as_ref() == Some(w)))
        };
        if let Some(w) = u.window.as_ref().filter(|w| !named_else(w)) {
            _ = std::fs::remove_dir_all(w);
        }
    }
}

/// The PowerShell that smoke-tests `zip` (already in `sandbox`, a fresh
/// folder) of app `id` on Windows, and says what it saw ([`seen_of`]): the
/// bundle unpacked there, started with its data there, asked to stop with
/// `--quit`; its door's folder in `%TEMP%\aio-rt`, its record in the
/// runtime store's app list ([`forget_starts`]) and the sandbox go again,
/// pass or fail. Pure.
pub fn windows_script(sandbox: &str, zip: &str, id: &str, wait: Wait) -> String {
    let q = |s: &str| format!("'{}'", s.replace('\'', "''"));
    let args = wait.args().iter().map(|a| q(a)).collect::<Vec<_>>().join(",");
    let up = match wait {
        Wait::Window => {
            format!("(Test-Path $log) -and (Select-String -LiteralPath $log -SimpleMatch {} -Quiet)", q(READY))
        }
        Wait::Boot => "$null -ne (Pid-Of)".into(),
        Wait::Answer => "$p.HasExited".into(),
    };
    format!(
        r#"$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$sb = {sb}; $id = {id}
$log = "$sb\root\logs\app.log"; $health = "$sb\root\data\health.json"
$doors = Join-Path $env:TEMP 'aio-rt'
$had = @(Get-ChildItem -LiteralPath $doors -Directory -Filter "$id-*" -ErrorAction SilentlyContinue | ForEach-Object Name)
function Pid-Of {{ try {{ (Get-Content -Raw -LiteralPath $health | ConvertFrom-Json).pid }} catch {{ $null }} }}
$p = $null
try {{
  Expand-Archive -LiteralPath (Join-Path $sb {zip}) -DestinationPath "$sb\unpack"
  $exe = Get-ChildItem -LiteralPath "$sb\unpack" -Recurse -Filter "$id.exe" | Select-Object -First 1 -ExpandProperty FullName
  if (-not $exe) {{ throw "no $id.exe in the bundle" }}
  New-Item -ItemType Directory -Force "$sb\root", "$sb\cwd" | Out-Null
  $env:AIORT_DATA = "$sb\root"
  Remove-Item Env:AIORT_CONFIG, Env:AIORT_TOKEN -ErrorAction SilentlyContinue
  $start = @{{ FilePath = $exe; WorkingDirectory = "$sb\cwd"; PassThru = $true; RedirectStandardOutput = "$sb\answer"; RedirectStandardError = "$sb\stderr" }}
  $args = @({args})
  if ($args.Count) {{ $start.ArgumentList = $args }}
  $t = [Diagnostics.Stopwatch]::StartNew()
  $p = Start-Process @start
  $null = $p.Handle
  while ($t.Elapsed.TotalSeconds -lt {boot}) {{
    if ({up}) {{ "boot=$($t.ElapsedMilliseconds)"; break }}
    if ($p.HasExited) {{ break }}
    Start-Sleep -Milliseconds 200
  }}
  if ($p.HasExited) {{ "exit=$($p.ExitCode)"; "said=" + ((Get-Content -Raw -LiteralPath "$sb\answer") -replace '\s+', ' ') }}
  elseif ($t.Elapsed.TotalSeconds -lt {boot}) {{
    if ($null -eq (Pid-Of)) {{ 'stop=nopid' }} else {{
      $s = [Diagnostics.Stopwatch]::StartNew()
      $null = Start-Process -FilePath $exe -ArgumentList '--quit' -Wait -WorkingDirectory "$sb\cwd" -RedirectStandardError "$sb\quit"
      if ($p.WaitForExit({stop_ms})) {{ "stop=$($s.ElapsedMilliseconds):$($p.ExitCode)" }} else {{ 'stop=hung' }}
      "health=$(Test-Path -LiteralPath $health)"
    }}
  }}
  Start-Sleep -Seconds 3
  $left = @(Get-CimInstance Win32_Process | Where-Object {{ $_.CommandLine -and $_.CommandLine.Contains($sb) }} | ForEach-Object ProcessId)
  "left=$($left -join ' ')"
  $left | ForEach-Object {{ Stop-Process -Id $_ -Force -ErrorAction SilentlyContinue }}
  '--log--'
  if (Test-Path -LiteralPath $log) {{ Get-Content -Raw -LiteralPath $log }}
}} finally {{
  if ($p -and -not $p.HasExited) {{ Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue }}
  Get-ChildItem -LiteralPath $doors -Directory -Filter "$id-*" -ErrorAction SilentlyContinue | Where-Object {{ $had -notcontains $_.Name }} | Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
  $store = if ($env:AIO_SHARED_ELECTRON_DIR) {{ $env:AIO_SHARED_ELECTRON_DIR }} else {{ Join-Path $env:LOCALAPPDATA 'aio\electron' }}
  Get-ChildItem -LiteralPath (Join-Path $store 'apps') -File -Filter '*.json' -ErrorAction SilentlyContinue | Where-Object {{
    try {{ "$((Get-Content -Raw -LiteralPath $_.FullName | ConvertFrom-Json).exe)".StartsWith("$sb\", [StringComparison]::OrdinalIgnoreCase) }} catch {{ $false }}
  }} | Remove-Item -Force -ErrorAction SilentlyContinue
  Start-Sleep -Milliseconds 500
  Remove-Item -LiteralPath $sb -Recurse -Force -ErrorAction SilentlyContinue
}}
"#,
        sb = q(sandbox),
        id = q(id),
        zip = q(zip),
        boot = BOOT.as_secs(),
        stop_ms = STOP.as_millis(),
    )
}

/// What [`windows_script`] said, as [`Seen`]. Pure.
pub fn seen_of(out: &str, wait: Wait) -> Seen {
    let (said, log) = out.split_once("--log--").unwrap_or((out, ""));
    let get = |k: &str| said.lines().find_map(|l| l.trim_end().strip_prefix(k)).map(str::to_string);
    let ms = |v: &str| Duration::from_millis(v.trim().parse().unwrap_or(0));
    let code = |c: &str| if c.trim() == "0" { Ok(()) } else { Err(format!("exit code: {}", c.trim())) };
    let booted = get("boot=").map(|v| ms(&v));
    let answer = get("exit=").filter(|_| wait == Wait::Answer).map(|c| (code(&c), get("said=").unwrap_or_default()));
    let left_file = get("health=").is_some_and(|v| v == "True");
    let stop = match get("stop=").as_deref() {
        None => Stop::NotTried,
        Some("nopid") => Stop::NoPid,
        Some("hung") => Stop::Hung { left: left_file },
        Some(v) => {
            let (t, c) = v.split_once(':').unwrap_or((v, "0"));
            Stop::Exited { after: ms(t), status: code(c), left: left_file }
        }
    };
    let left = get("left=").unwrap_or_default().split_whitespace().filter_map(|p| p.parse().ok()).collect();
    Seen { booted, answer, stop, left, log: log.trim_start_matches(['\r', '\n']).to_string(), memory: None }
}

/// Smoke-test Windows bundle `zip` of app `id` on Windows: this machine,
/// or the one `AIO_WINDOWS_SSH` names (over its PowerShell).
pub fn windows(zip: &Path, id: &str, wait: Wait) -> Result<String, String> {
    let name = zip.file_name().and_then(|n| n.to_str()).ok_or("bad artifact name")?.to_string();
    let host = std::env::var(WINDOWS_ENV).ok().as_deref().and_then(crate::remote::host_of);
    if host.is_none() && !cfg!(windows) {
        return Err(format!("a Windows bundle is smoke-tested on Windows: set {WINDOWS_ENV} to reach one"));
    }
    let leaf = format!("aio-smoke-{id}-{}", std::process::id());
    // The sandbox: in the Windows user's %TEMP%.
    let made = powershell(
        host.as_ref(),
        &format!("(New-Item -ItemType Directory -Force (Join-Path $env:TEMP '{leaf}')).FullName"),
    )?;
    let sandbox = made.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or("").to_string();
    if sandbox.is_empty() {
        return Err("no sandbox made on Windows".into());
    }
    let copied = match &host {
        Some(h) => {
            let mut scp = Command::new("scp");
            scp.args(["-q", "-o", "BatchMode=yes"]).args(&h.opts).arg(zip);
            let ok = scp.arg(format!("{}:{}/{name}", h.target, sandbox.replace('\\', "/"))).status();
            ok.map_err(|e| format!("scp: {e}"))?
                .success()
                .then_some(())
                .ok_or(format!("could not copy it to {}", h.target))
        }
        None => std::fs::copy(zip, Path::new(&sandbox).join(&name)).map(drop).map_err(|e| e.to_string()),
    };
    if let Err(e) = copied {
        _ = powershell(host.as_ref(), &format!("Remove-Item -LiteralPath '{sandbox}' -Recurse -Force"));
        return Err(e);
    }
    let out = powershell(host.as_ref(), &windows_script(&sandbox, &name, id, wait))?;
    report(id, wait, &seen_of(&out, wait), "--quit")
}

/// Where a Windows smoke test runs, away from Windows.
pub const WINDOWS_ENV: &str = "AIO_WINDOWS_SSH";

/// PowerShell's `-EncodedCommand` form of `script`: UTF-16LE, base64 (one
/// argument whatever the script holds, through ssh's shell too). Pure.
pub fn encoded(script: &str) -> String {
    use base64::Engine;
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64::engine::general_purpose::STANDARD.encode(utf16)
}

/// What `script` prints, run by PowerShell here or on `host`.
fn powershell(host: Option<&crate::remote::Host>, script: &str) -> Result<String, String> {
    let mut c = match host {
        Some(h) => {
            let mut c = Command::new("ssh");
            c.args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"]).args(&h.opts).arg(&h.target).arg("powershell");
            c
        }
        None => Command::new("powershell"),
    };
    // No progress records: over ssh they are XML on stderr.
    let script = format!("$ProgressPreference = 'SilentlyContinue'\n{script}");
    c.args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &encoded(&script)]);
    let out = c.stdin(Stdio::null()).stderr(Stdio::inherit()).output().map_err(|e| format!("powershell: {e}"))?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Debian package's tree as `ax build --deb` lays it out.
    #[cfg(unix)]
    fn deb_tree(entry: &str, icon: Option<&str>) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let p = |r: &str| d.path().join(r);
        for dir in ["usr/lib/notes", "usr/bin", "usr/share/applications", "usr/share/icons/hicolor/256x256/apps"] {
            std::fs::create_dir_all(p(dir)).unwrap();
        }
        std::fs::write(p("usr/lib/notes/notes"), "").unwrap();
        std::os::unix::fs::symlink("../lib/notes/notes", p("usr/bin/notes")).unwrap();
        std::fs::write(p("usr/share/applications/notes.desktop"), entry).unwrap();
        if let Some(i) = icon {
            std::fs::write(p(&format!("usr/share/icons/hicolor/256x256/apps/{i}")), "").unwrap();
        }
        d
    }

    #[cfg(unix)]
    #[test]
    fn a_package_s_desktop_entry_must_start_its_program_with_its_icon() {
        let good = "[Desktop Entry]\nExec=\"/usr/bin/notes\" %U\nIcon=notes\n";
        assert!(desktop_problems(deb_tree(good, Some("notes.png")).path(), "notes").is_empty());
        let wrong_exec = good.replace("/usr/bin/notes", "/usr/bin/note");
        let p = desktop_problems(deb_tree(&wrong_exec, Some("notes.png")).path(), "notes");
        assert_eq!(
            p,
            ["usr/share/applications/notes.desktop: its Exec program \"/usr/bin/note\" is not in the package"]
        );
        let p = desktop_problems(deb_tree(good, None).path(), "notes");
        assert_eq!(p, ["usr/share/applications/notes.desktop: no icon \"notes\" in the package"]);
        assert_eq!(
            desktop_problems(deb_tree(good, Some("notes.svg")).path(), "other"),
            ["no usr/share/applications/other.desktop"]
        );
        let shown = with_package(Ok("smoke notes:\n  ✓ boots".into()), &[]).unwrap();
        assert_eq!(shown, "smoke notes:\n  ✓ its desktop entry starts its program, with its icon\n  ✓ boots");
        assert!(
            with_package(Ok("smoke notes:\n  ✓ boots".into()), &p)
                .unwrap_err()
                .contains("  ✗ usr/share/applications/notes.desktop: no icon")
        );
    }

    #[test]
    fn panics_reports_and_degraded_parts_fail_a_smoke_test() {
        let log = "2026 [aio] window ready\n2026 thread 'main' panicked at src/x.rs:1:1\n2026 [aio] sync is degraded: refused\nok";
        assert_eq!(bad_lines(log).len(), 2);
        let page =
            "2026 ERROR [page] Uncaught SyntaxError: Unexpected token ')' src=aio://app/:1\n2026 WARN  [page] slow";
        assert_eq!(bad_lines(page), [page.lines().next().unwrap()], "a page script that throws");
        assert!(bad_lines("2026 [aio] window ready\n2026 [aio] todo stopped").is_empty());
    }

    #[test]
    fn a_mac_smokes_its_app_from_the_zip_and_servers_and_clis_from_the_bundle() {
        let (bundle, deb) = (Path::new("/d/board-1.0.0-macos-arm64-cli.tar.gz"), Path::new("/d/board.deb"));
        assert!(startable("macos", bundle, "board", Wait::Boot) && startable("macos", bundle, "board", Wait::Answer));
        assert!(!startable("macos", bundle, "board", Wait::Window));
        let zip = Path::new("/d/board-1.0.0-macos-arm64.zip");
        assert!(startable("macos", zip, "board", Wait::Window) && !startable("linux", zip, "board", Wait::Window));
        let (cmd, rel) = unpack_plan(zip, "board").unwrap();
        assert_eq!(cmd, ["/usr/bin/ditto", "-x", "-k", "/d/board-1.0.0-macos-arm64.zip", "."]);
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("Board & Co.app/Contents/MacOS")).unwrap();
        assert_eq!(unpacked(d.path(), &rel), d.path().join("Board & Co.app/Contents/MacOS/board"), "any name");
        assert_eq!(unpacked(d.path(), Path::new("x/board")), d.path().join("x/board"));
        assert!(!startable("macos", deb, "board", Wait::Boot) && startable("linux", deb, "board", Wait::Window));
        assert!(!startable("windows", bundle, "board", Wait::Answer));
        let ps =
            "    1 /sbin/launchd\n  812 /tmp/aio-smoke-board-9/unpack/board --port=0\n 99 x /tmp/aio-smoke-board-9\n";
        assert_eq!(pids_naming(ps, "/tmp/aio-smoke-board-9"), [812, 99]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn each_linux_artifact_unpacks_to_the_program_it_starts() {
        assert_eq!(unpack_plan(Path::new("/d/notes-1.0.0-windows-x64.zip"), "notes"), None);
        let (cmd, rel) = unpack_plan(Path::new("/d/notes-x86_64.AppImage"), "notes").unwrap();
        assert_eq!(
            (cmd, rel),
            (vec!["/d/notes-x86_64.AppImage".into(), "--appimage-extract".into()], "squashfs-root/AppRun".into())
        );
        let d = tempfile::tempdir().unwrap();
        let run_plan = |artifact: &Path| {
            let (cmd, rel) = unpack_plan(artifact, "notes").unwrap();
            let out = d.path().join(format!("out-{}", artifact.extension().unwrap().to_string_lossy()));
            std::fs::create_dir_all(&out).unwrap();
            assert!(Command::new(&cmd[0]).args(&cmd[1..]).current_dir(&out).status().unwrap().success());
            std::fs::read_to_string(out.join(rel)).unwrap()
        };
        // A bundle, as `build` packs it.
        let stage = d.path().join("stage/notes-1.0.0-linux-x64");
        std::fs::create_dir_all(&stage).unwrap();
        std::fs::write(stage.join("notes"), "bundle").unwrap();
        let tgz = d.path().join("notes-1.0.0-linux-x64.tar.gz");
        let st = Command::new("tar")
            .arg("-czf")
            .arg(&tgz)
            .arg("-C")
            .arg(d.path().join("stage"))
            .arg("notes-1.0.0-linux-x64")
            .status();
        assert!(st.unwrap().success());
        assert_eq!(run_plan(&tgz), "bundle");
        // A Debian package, as `package::deb` lays it out.
        let root = d.path().join("deb");
        std::fs::create_dir_all(root.join("usr/lib/notes")).unwrap();
        std::fs::create_dir_all(root.join("DEBIAN")).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root.join("DEBIAN"), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(root.join("usr/lib/notes/notes"), "deb").unwrap();
        std::fs::write(
            root.join("DEBIAN/control"),
            "Package: notes\nVersion: 1.0.0\nArchitecture: amd64\nMaintainer: t <t@example.invalid>\nDescription: t\n",
        )
        .unwrap();
        let deb = d.path().join("notes_1.0.0_amd64.deb");
        let st = Command::new("dpkg-deb")
            .args(["-b", "--root-owner-group"])
            .arg(&root)
            .arg(&deb)
            .stdout(Stdio::null())
            .status();
        assert!(st.unwrap().success());
        assert_eq!(run_plan(&deb), "deb");
    }

    /// A CLI bundle of `relay` whose program is `script`.
    #[cfg(target_os = "linux")]
    fn cli_bundle(d: &Path, script: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let stage = d.join("stage/relay-1.0.0-linux-x64-cli");
        std::fs::create_dir_all(&stage).unwrap();
        std::fs::write(stage.join("relay"), format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(stage.join("relay"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let tgz = d.join("relay-1.0.0-linux-x64-cli.tar.gz");
        let st = Command::new("tar")
            .arg("-czf")
            .arg(&tgz)
            .arg("-C")
            .arg(d.join("stage"))
            .arg("relay-1.0.0-linux-x64-cli")
            .status();
        assert!(st.unwrap().success());
        tgz
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_cli_answers_its_version_and_a_server_boots_and_stops_with_no_display() {
        let d = tempfile::tempdir().unwrap();
        let cli = cli_bundle(d.path(), r#"[ "$1" = --version ] && echo "relay 1.0.0""#);
        let out = run(&cli, "relay", Wait::Answer).unwrap();
        assert!(out.contains("✓ answers --version with its id") && !out.contains('✗'), "{out}");
        // Named from here, as a person types it.
        let up = "../".repeat(std::env::current_dir().unwrap().components().count() - 1);
        let rel = PathBuf::from(up).join(cli.strip_prefix("/").unwrap());
        assert!(rel.is_relative() && run(&rel, "relay", Wait::Answer).is_ok(), "{}", rel.display());
        let wrong = cli_bundle(d.path(), "echo other 1.0.0");
        assert!(run(&wrong, "relay", Wait::Answer).unwrap_err().contains("✗ answers --version"));
        // A server on a free port: its health file names it until SIGTERM.
        let server = cli_bundle(
            d.path(),
            r#"[ "$1" = --port=0 ] || exit 2
h="$AIORT_DATA/data/health.json"; mkdir -p "$AIORT_DATA/data"
trap 'rm -f "$h"; exit 0' TERM
echo "{\"pid\": $$}" > "$h"
while :; do sleep 0.1; done"#,
        );
        let out = run(&server, "relay", Wait::Boot).unwrap();
        assert!(out.contains("✓ boots: serving") && out.contains("✓ stops on SIGTERM"), "{out}");
        let dies = cli_bundle(d.path(), "exit 1");
        assert!(run(&dies, "relay", Wait::Boot).unwrap_err().contains("✗ boots: not booted"));
    }

    #[test]
    fn a_windows_bundle_is_smoked_on_windows_and_asked_to_stop_with_quit() {
        let zip = Path::new("/d/notes-1.0.0-windows-x64.zip");
        assert!(is_windows_zip(zip) && startable("windows", zip, "notes", Wait::Window));
        assert!(!startable("linux", zip, "notes", Wait::Window), "never on Linux itself");
        assert!(!is_windows_zip(Path::new("/d/notes-1.0.0-windows-x64.exe")), "the one-click download installs");
        let ps = windows_script(
            r"C:\Users\me\AppData\Local\Temp\aio-smoke-notes-7",
            "notes-1.0.0-windows-x64.zip",
            "notes",
            Wait::Window,
        );
        for part in [
            r"$sb = 'C:\Users\me\AppData\Local\Temp\aio-smoke-notes-7'; $id = 'notes'",
            "Expand-Archive -LiteralPath (Join-Path $sb 'notes-1.0.0-windows-x64.zip')",
            r#"$env:AIORT_DATA = "$sb\root""#,
            "Select-String -LiteralPath $log -SimpleMatch '[aio] window ready' -Quiet",
            "Start-Process -FilePath $exe -ArgumentList '--quit' -Wait",
            "$p.WaitForExit(20000)",
            "$_.CommandLine.Contains($sb)",
            "Remove-Item -LiteralPath $sb -Recurse -Force",
            "Where-Object { $had -notcontains $_.Name } | Remove-Item",
            r#"Get-ChildItem -LiteralPath (Join-Path $store 'apps') -File -Filter '*.json'"#,
            r#".StartsWith("$sb\", [StringComparison]::OrdinalIgnoreCase)"#,
        ] {
            assert!(ps.contains(part), "{part}\n{ps}");
        }
        assert!(ps.find("} finally {").unwrap() < ps.find("Remove-Item -LiteralPath $sb").unwrap(), "pass or fail");
        assert!(ps.find("} finally {").unwrap() < ps.find("(Join-Path $store 'apps')").unwrap(), "pass or fail");
        let server = windows_script("C:\\t\\s", "a.zip", "it's", Wait::Boot);
        assert!(server.contains("$id = 'it''s'") && server.contains("$args = @('--port=0')"));
        assert!(server.contains("if ($null -ne (Pid-Of))"));
        assert!(windows_script("C:\\t", "a.zip", "relay", Wait::Answer).contains("$args = @('--version')"));
    }

    #[test]
    fn a_smoke_forgets_its_own_starts_in_the_runtime_store_and_nothing_else() {
        use aio_shell::prune::{Use, record, uses};
        let t = tempfile::tempdir().unwrap();
        let (store, sb) = (t.path().join("store"), t.path().join("aio-smoke-notes-7"));
        let win = |n: &str| store.join("windows").join(n).to_string_lossy().into_owned();
        let at = |exe: &Path, window: Option<String>| Use {
            exe: exe.to_string_lossy().into(),
            version: "44.9.3".into(),
            slug: "linux-x64".into(),
            window,
            pending: false,
        };
        for w in ["notes", "mine"] {
            std::fs::create_dir_all(win(w)).unwrap();
        }
        record(&store, &at(&sb.join("unpack/notes/notes"), Some(win("notes")))).unwrap();
        record(&store, &at(&sb.join("unpack/Mine.app/x"), Some(win("mine")))).unwrap();
        record(&store, &at(Path::new("/opt/notes/notes"), Some(win("notes")))).unwrap();
        record(&store, &at(&t.path().join("aio-smoke-notes-70/x"), None)).unwrap();
        forget_starts(&store, &sb);
        let mut left: Vec<PathBuf> =
            uses(&store).unwrap().into_iter().map(|(_, u, _)| PathBuf::from(u.unwrap().exe)).collect();
        left.sort();
        let mut want = vec![t.path().join("aio-smoke-notes-70/x"), PathBuf::from("/opt/notes/notes")];
        want.sort();
        assert_eq!(left, want, "the installed app's and another sandbox's stay");
        assert!(Path::new(&win("notes")).is_dir(), "a window another record names stays");
        assert!(!Path::new(&win("mine")).exists(), "the smoke's own window goes");
        forget_starts(&t.path().join("none"), &sb);
    }

    #[test]
    fn a_script_reaches_powershell_whole_as_one_argument() {
        assert_eq!(encoded("ab"), "YQBiAA==");
        assert!(encoded("try {\n  'x'\n} finally { }").chars().all(|c| c.is_ascii_alphanumeric() || "+/=".contains(c)));
    }

    #[test]
    fn what_windows_said_is_the_same_report() {
        let out = "boot=2400\r\nstop=900:0\r\nhealth=False\r\nleft=\r\n--log--\r\n2026 [aio] window ready\r\n";
        let seen = seen_of(out, Wait::Window);
        assert_eq!(
            seen,
            Seen {
                booted: Some(Duration::from_millis(2400)),
                answer: None,
                stop: Stop::Exited { after: Duration::from_millis(900), status: Ok(()), left: false },
                left: vec![],
                log: "2026 [aio] window ready\r\n".into(),
                memory: None,
            }
        );
        let ok = report("notes", Wait::Window, &seen, "--quit").unwrap();
        assert!(ok.contains("✓ boots: window ready in 2.4 s") && ok.contains("✓ stops on --quit in 0.9 s"), "{ok}");
        let bad = seen_of(
            "boot=100\nstop=hung\nhealth=True\nleft=812 99\n--log--\nthread 'main' panicked at x\n",
            Wait::Window,
        );
        assert_eq!((bad.stop.clone(), bad.left.clone()), (Stop::Hung { left: true }, vec![812, 99]));
        let e = report("notes", Wait::Window, &bad, "--quit").unwrap_err();
        for row in [
            "✗ does not stop within 20 s of --quit",
            "✗ stops cleanly",
            "✗ leaves no process behind ([812, 99] left)",
            "✗ logs no panic",
            "log (newest last)",
        ] {
            assert!(e.contains(row), "{row}: {e}");
        }
        let never = report("notes", Wait::Window, &seen_of("left=\n--log--\n", Wait::Window), "--quit").unwrap_err();
        assert!(never.contains("✗ boots: no window within 90 s") && !never.contains("stops"), "{never}");
        let cli = seen_of("exit=0\nsaid=relay 1.0.0 \nleft=\n--log--\n", Wait::Answer);
        assert!(report("relay", Wait::Answer, &cli, "--quit").unwrap().contains("✓ answers --version with its id"));
        let wrong = seen_of("exit=3\nsaid=relay 1.0.0\nleft=\n--log--\n", Wait::Answer);
        assert!(report("relay", Wait::Answer, &wrong, "--quit").unwrap_err().contains("exit code: 3"));
        assert_eq!(seen_of("boot=5\nstop=nopid\n", Wait::Boot).stop, Stop::NoPid);
    }

    #[test]
    fn a_display_number_no_one_uses() {
        assert_eq!(free_display(|n| n < 93), Some(93));
        assert_eq!(free_display(|_| true), None);
    }

    #[test]
    fn a_windows_memory_is_its_whole_trees_pss() {
        let rollup = "Rss:  900 kB\nPss:  120 kB\nPss_Anon: 80 kB\nPrivate_Clean: 10 kB\nPrivate_Dirty: 70 kB\n";
        assert_eq!(pss(rollup), Some((120 * 1024, 80 * 1024, 70 * 1024)));
        assert_eq!(pss("Rss: 1 kB\n"), None);
        assert_eq!(parent("42 (aio shell) S 7 42 42 0"), Some(7), "a name with spaces and ')'");
        let parents = [(2, 1), (3, 2), (4, 2), (5, 9), (6, 3)];
        assert_eq!(tree(1, &parents), [1, 2, 3, 4, 6]);
        assert_eq!(id_of(Path::new("/d/my-app-0.1.3-linux-x64.tar.gz")), Some("my-app"));
        assert_eq!(id_of(Path::new("purse-0.1.0-x86_64.AppImage")), Some("purse"));
        assert_eq!(id_of(Path::new("notes.tar.gz")), None);
        assert_eq!(id_of(Path::new("dist/my-app_0.1.3~rc.1_amd64.deb")), Some("my-app"), "Debian's naming");
        assert_eq!(id_of(Path::new("notes.deb")), None);
        if cfg!(target_os = "linux") {
            let here = memory(std::process::id()).unwrap();
            assert!(
                here.app > 0 && here.pss >= here.private && here.private >= here.dirty && here.processes >= 1,
                "{here:?}"
            );
        }
        let m = Memory { app: 9_000_000, pss: 180_000_000, private: 120_000_000, dirty: 70_000_000, processes: 5 };
        let seen = Seen {
            booted: Some(Duration::from_secs(1)),
            answer: None,
            stop: Stop::NotTried,
            left: vec![],
            log: String::new(),
            memory: Some(m),
        };
        let out = report("notes", Wait::Window, &seen, "SIGTERM").unwrap();
        assert!(
            out.contains("✓ memory: 120 MB private, 180 MB PSS in 5 processes (the app 9.0 MB), 70 MB written"),
            "{out}"
        );
    }
}

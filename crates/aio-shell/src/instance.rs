//! One instance per app: a second launch hands its arguments to
//! the running one, which comes to the front, and then exits.
use aio::serde_json::{self, json};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tokio::sync::broadcast;

/// A later launch of the app: its arguments, relative to its own dir.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub cwd: PathBuf,
    pub args: Vec<String>,
}

impl Launch {
    /// `arg` as an absolute path (URLs and absolute paths stay as they are).
    pub fn path(&self, arg: &str) -> String {
        if arg.contains("://") || Path::new(arg).is_absolute() {
            return arg.into();
        }
        self.cwd.join(arg).to_string_lossy().into_owned()
    }
}

fn channel() -> &'static broadcast::Sender<Launch> {
    static TX: OnceLock<broadcast::Sender<Launch>> = OnceLock::new();
    TX.get_or_init(|| broadcast::channel(16).0)
}

/// Launches handed over while nothing listened (the app still starting,
/// its first start downloading Electron): the last [`HELD`], for the first
/// listener.
static HELD_LAUNCHES: std::sync::Mutex<Vec<Launch>> = std::sync::Mutex::new(Vec::new());
const HELD: usize = 16;

/// Later launches, for the app to act on (open the file it was given);
/// first those handed over before anything listened (the last 16).
pub fn launches() -> broadcast::Receiver<Launch> {
    let mut held = HELD_LAUNCHES.lock().unwrap_or_else(|e| e.into_inner());
    let rx = channel().subscribe();
    for l in held.drain(..) {
        _ = channel().send(l);
    }
    rx
}

pub(crate) fn deliver(l: Launch) {
    let mut held = HELD_LAUNCHES.lock().unwrap_or_else(|e| e.into_inner());
    if channel().receiver_count() > 0 {
        _ = channel().send(l);
        return;
    }
    if held.len() == HELD {
        held.remove(0);
    }
    held.push(l);
}

/// How `start` went.
// Made once per process: a box would only cost the apps' `let Start::Run(dirs)`.
#[allow(clippy::large_enum_variant)]
pub enum Start {
    /// This process runs the app.
    Run(aio::paths::AppDirs),
    /// The app was already running and has the arguments now: exit.
    HandedOver,
    /// Nothing to run: `--version` answered, or an update is installing.
    Done,
}

/// Where the app is installed on Windows: `%LOCALAPPDATA%\Programs\<id>`.
pub fn programs(id: &str) -> PathBuf {
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_default();
    local.join("Programs").join(id)
}

/// This app and its version, once `start` ran (for `update::confirm`).
pub(crate) static APP: std::sync::OnceLock<(String, String)> = std::sync::OnceLock::new();
/// Its macOS bundle id (`Config::mac_bundle_id`): its window's must match
/// the `.app`'s, or the Dock shows two apps.
pub(crate) static BUNDLE_ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
/// How the app that started shows (`start`): `[app] ui`, or the launch's
/// `--client`; `None`: no window at all (`--client=server-only`).
pub(crate) static SHOWN: std::sync::OnceLock<Option<aio::config::Shown>> = std::sync::OnceLock::new();

/// How a launch's `--client=<word>` (aio's) shows the app, `[app] ui` its
/// default (pure): `None` is no window, served until asked to stop.
pub(crate) fn shown(client: Option<&str>, ui: aio::config::Shown) -> io::Result<Option<aio::config::Shown>> {
    use aio::config::Shown;
    match client {
        None => Ok(Some(ui)),
        Some("window" | "electron") => Ok(Some(Shown::Window)),
        Some("browser") => Ok(Some(Shown::Browser)),
        Some("server-only") => Ok(None),
        Some(c) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("--client={c}: one of window (electron), browser, server-only"),
        )),
    }
}
/// Its hue (`[ui] hue`, else its id's) and its splash: what is drawn
/// outside its page (`splash`, `firstrun`).
pub(crate) static LOOK: std::sync::OnceLock<(u16, Option<aio::config::Splash>)> = std::sync::OnceLock::new();
/// The app's built-in files (`[build] assets`): its page reads them at their URL paths.
pub(crate) static ASSETS: std::sync::OnceLock<&'static [(&'static str, &'static [u8])]> = std::sync::OnceLock::new();
/// `--keep-server`, as this launch asked it.
pub(crate) static KEEP_SERVER: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

/// The running instance's door for later launches.
pub(crate) fn hand_addr(id: &str, data: &Path) -> io::Result<String> {
    Ok(aio::ipc::addr(&aio::paths::sockets(id, data)?, "hand"))
}

/// The line that asks the running instance to stop (Windows has no SIGTERM).
pub(crate) const QUIT: &str = "quit";

/// Ask app `id`'s running instance to stop, the graceful way.
pub fn quit(id: &str) -> io::Result<()> {
    let (data, _, _) = aio::paths::locate(id)?;
    quit_at(id, &data)
}

/// Ask app `id`'s instance running on data dir `data` (a profile, an
/// instance, an aio home) to stop, the graceful way.
pub fn quit_at(id: &str, data: &Path) -> io::Result<()> {
    exchange(&hand_addr(id, data)?, QUIT.into())
}

/// The app's door, open from [`start`] on (not once its window is up: a
/// first start downloading Electron outlasts a launch's 30 s wait; `run`
/// without `start` opens it itself), once per address. Later launches hand
/// their arguments here, held until the app listens ([`launches`]), and
/// bring its window forward once there is one; `ax stop` asks here, the
/// graceful way on every OS (Windows has no SIGTERM, and a kill would
/// skip the last save and restart under the supervisor). No door is not
/// fatal: SIGTERM still stops it where there is one. Once a stop is asked
/// or the window ends ([`crate::ending`]; a server kept after its window
/// serves on), a launch is told it is ending and waits for the lock.
pub(crate) fn open_door(id: &str, data: &Path) {
    static OPEN: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let _rt = aio::rt().enter();
    let mut open = OPEN.lock().unwrap_or_else(|e| e.into_inner());
    let door = hand_addr(id, data).and_then(|a| match open.contains(&a) {
        true => Ok(None),
        false => aio::ipc::Listener::bind(&a).map(|l| Some((a, l))),
    });
    match door {
        Ok(Some((a, l))) => {
            open.push(a);
            let ending = || aio::lifecycle::stopping() || crate::ending();
            _ = aio::rt().spawn(crate::take_launches(l, crate::ctl(), aio::lifecycle::ask_stop, ending));
        }
        Ok(None) => {}
        Err(e) => eprintln!("[aio] {id}: no door for later launches or `ax stop` ({e})"),
    }
}

/// The app's args `ax dev` gives a debug build as JSON, when dx's own
/// `--args` can't carry them (it splits them on spaces).
pub const DEV_ARGS: &str = "AIORT_DEV_ARGS";

/// Run this binary again with `args` (Unix: in its place, the same process).
#[cfg(feature = "dev")]
fn run_with(args: Vec<String>) -> io::Result<Start> {
    let mut cmd = std::process::Command::new(std::env::current_exe()?);
    cmd.args(args).env_remove(DEV_ARGS);
    #[cfg(unix)]
    return Err(std::os::unix::process::CommandExt::exec(&mut cmd));
    #[cfg(not(unix))]
    {
        let _watch = aio::lifecycle::watched(&mut cmd)?;
        std::process::exit(cmd.status()?.code().unwrap_or(1))
    }
}

/// Take the app's lock and boot it (`aio::boot`: log, reports, health, the
/// boot report), or hand `args` to the instance holding it. `version`: the
/// app's, `env!("CARGO_PKG_VERSION")`.
pub fn start(cfg: &aio::config::Config, version: &str) -> io::Result<Start> {
    // Windows: before anything starts a console program (the installer's
    // `reg`, a dialog's PowerShell, the app's own), so none opens a window.
    aio::process::hide_console();
    #[cfg(feature = "dev")]
    if let Some(args) = std::env::var(DEV_ARGS).ok().and_then(|a| serde_json::from_str::<Vec<String>>(&a).ok()) {
        return run_with(args);
    }
    let id = cfg.app.id.as_str();
    let args: Vec<String> = std::env::args().skip(1).collect();
    // A staged new version, run as the update swap helper (macOS, Windows).
    if let [flag, plan] = &args[..]
        && flag == crate::swap::FLAG
    {
        let done = aio::serde_json::from_str(plan).map_err(io::Error::other).and_then(|p| crate::swap::run(&p));
        if let Err(e) = done {
            eprintln!("ax update: {e}");
        }
        return Ok(Start::Done);
    }
    // The update check asks a bundle's binary who it is.
    if args == ["--version"] {
        println!("{id} {version}");
        return Ok(Start::Done);
    }
    #[cfg(windows)]
    if let [a] = &args[..]
        && (a == "--install" || a == "--uninstall")
    {
        match a.as_str() {
            "--install" => crate::winstall::install(cfg, version)?,
            _ => crate::winstall::uninstall(cfg)?,
        }
        return Ok(Start::Done);
    }
    // The one-click download, double-clicked: it installs and hands over.
    #[cfg(windows)]
    if let Some(file) = std::env::current_exe()?.file_name()
        && crate::oneclick::is_download(&file.to_string_lossy(), id)
    {
        crate::winstall::one_click(cfg, version, &args)?;
        return Ok(Start::Done);
    }
    // Restarting to install an update: the old process holds the lock until it is gone.
    if let Some(pid) = std::env::var(crate::update::RELAUNCH).ok().and_then(|p| p.parse().ok()) {
        aio::lifecycle::wait_gone(pid, std::time::Duration::from_secs(60));
    }
    // A supervised new version that died before it booted: the old one runs.
    #[cfg(target_os = "linux")]
    if crate::update::supervised_start(cfg, version)? {
        return Ok(Start::Done);
    }
    let ask = aio::paths::Ask::parse(&args).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    _ = KEEP_SERVER.set(ask.keep_server);
    let shows = shown(ask.client.as_deref(), built(option_env!("AIORT_BUILT_UI"), cfg.app.ui))?;
    let plan = aio::paths::plan_app(cfg, &ask)?;
    // `<app> --quit`: the instance running on these dirs is asked to stop,
    // the graceful way (Windows has no SIGTERM: scripts, the smoke test).
    if args.iter().any(|a| a == "--quit") {
        return quit_at(id, &plan.data).map(|()| Start::Done);
    }
    let Some(d) = claim_or_hand(id, &plan, ask.takeover, &args)? else { return Ok(Start::HandedOver) };
    aio::boot::begin(cfg, version, &d)?;
    crate::feedback::wire(cfg);
    _ = APP.set((id.into(), version.into()));
    _ = BUNDLE_ID.set(cfg.mac_bundle_id());
    _ = SHOWN.set(shows);
    _ = LOOK.set((cfg.hue(), cfg.build.splash.clone()));
    _ = ASSETS.set(cfg.build.asset_files);
    _ = crate::firstrun::WORDS.set(crate::firstrun::words_of(cfg, version));
    #[cfg(feature = "updates")]
    if let crate::update::Next::Exit = crate::update::on_start(cfg, version, &d)? {
        aio::health::stop();
        aio::log::finish();
        return Ok(Start::Done);
    }
    // Updates are never a reason not to start.
    #[cfg(feature = "updates")]
    if let Err(e) = crate::update::open(cfg, version, &d) {
        eprintln!("[aio] no updates cell: {e}");
    }
    #[cfg(not(feature = "updates"))]
    if let Some(w) = crate::update::left_out(cfg.updates.url.is_some(), aio::update::installed()) {
        eprintln!("{w}");
    }
    // No window to wait for (a server, a CLI): booted is started.
    if matches!(cfg.app.mode, aio::config::Mode::Server | aio::config::Mode::Cli) {
        crate::update::confirm(id, version);
    }
    if cfg.app.mode != aio::config::Mode::Cli {
        open_door(id, &d.data);
    }
    #[cfg(feature = "updates")]
    crate::update::watch(cfg, version, &d.data);
    Ok(Start::Run(d))
}

/// The dirs `plan` names, locked; `None`: `args` handed to the instance
/// holding them. One that has no door yet (starting) or none any more
/// (ending) is asked again until it takes them or lets go of the lock (30 s
/// at most); held by aio (or an aio-rs not answering), it says who holds it.
fn claim_or_hand(
    id: &str,
    plan: &aio::paths::Plan,
    takeover: bool,
    args: &[String],
) -> io::Result<Option<aio::paths::AppDirs>> {
    let e = match claim(id, plan, takeover) {
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => e,
        r => return r.map(Some),
    };
    let (addr, cwd) = (hand_addr(id, &plan.data)?, std::env::current_dir()?);
    let until = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let h = match hand_over(&addr, &cwd, args) {
            Ok(()) => return Ok(None),
            Err(h) if ending(&h) => h,
            Err(h) => return Err(io::Error::new(h.kind(), format!("{e}; {h}"))),
        };
        match aio::paths::AppDirs::planned(id, plan.clone()) {
            Err(w) if w.kind() == io::ErrorKind::WouldBlock && std::time::Instant::now() < until => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(w) if w.kind() == io::ErrorKind::WouldBlock => {
                return Err(io::Error::new(h.kind(), format!("{e}; {h}")));
            }
            r => return r.map(Some),
        }
    }
}

/// The dirs `plan` names, locked. `takeover` (aio's `--takeover`): the
/// instance holding them is asked to stop, the graceful way, and this one
/// waits for its lock (30 s at most, so a slow final save is not cut short).
fn claim(id: &str, plan: &aio::paths::Plan, takeover: bool) -> io::Result<aio::paths::AppDirs> {
    let first = aio::paths::AppDirs::planned(id, plan.clone());
    match &first {
        Err(e) if takeover && e.kind() == io::ErrorKind::WouldBlock => {
            eprintln!("[aio] {id}: {e}; asking it to stop (--takeover)");
            exchange(&hand_addr(id, &plan.data)?, QUIT.into())
                .map_err(|q| io::Error::new(q.kind(), format!("{e}; --takeover: {q}")))?;
        }
        _ => return first,
    }
    wait_lock(id, plan)
}

/// Whether a hand-over failed because the instance holding the lock is
/// ending or starting (pure): it said so, or its door is gone (removed as
/// it stops, the lock not yet let go) or not open yet.
fn ending(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::ResourceBusy | io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused)
}

/// The dirs `plan` names, locked once their holder lets go (30 s at most,
/// so a slow final save is not cut short).
fn wait_lock(id: &str, plan: &aio::paths::Plan) -> io::Result<aio::paths::AppDirs> {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        match aio::paths::AppDirs::planned(id, plan.clone()) {
            Err(e) if e.kind() == io::ErrorKind::WouldBlock && std::time::Instant::now() < until => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            r => return r,
        }
    }
}

/// What an ending instance answers a launch: it isn't taken, and the
/// launch waits for the lock ([`hand_over`]'s `ResourceBusy`).
pub(crate) const STOPPING: &str = "stopping";

/// Send one launch, wait for the running instance's "ok" (`ResourceBusy`:
/// it is ending, and took nothing).
pub fn hand_over(addr: &str, cwd: &Path, args: &[String]) -> io::Result<()> {
    exchange(addr, json!({ "cwd": cwd, "args": args }).to_string())
}

/// One line to the running instance and its "ok", within 10 s.
fn exchange(addr: &str, line: String) -> io::Result<()> {
    let s = aio::ipc::connect_blocking(addr)
        .map_err(|e| io::Error::new(e.kind(), format!("the app is running but not answering ({addr}): {e}")))?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let talk = || -> io::Result<String> {
            let mut w = s.try_clone()?;
            writeln!(w, "{line}")?;
            let mut reply = String::new();
            BufReader::new(s).read_line(&mut reply)?;
            Ok(reply)
        };
        _ = tx.send(talk());
    });
    let reply = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, format!("the running app did not answer ({addr})")))??;
    match reply.trim() {
        "ok" => {}
        STOPPING => return Err(io::Error::new(io::ErrorKind::ResourceBusy, "the running app is ending")),
        _ => return Err(io::Error::other(format!("the running app said {reply:?}"))),
    }
    Ok(())
}

/// Read one launch from a connection (the running side).
pub(crate) fn read_launch(line: &str) -> Option<Launch> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let args = v["args"].as_array()?.iter().map(|a| a.as_str().map(String::from)).collect::<Option<Vec<_>>>()?;
    Some(Launch { cwd: PathBuf::from(v["cwd"].as_str()?), args })
}

/// Where the UI shows: a `[build] targets` ui this build was stamped
/// with (`stamp`), else `[app] ui` (pure); a launch's `--client` outranks it.
fn built(stamp: Option<&str>, app: aio::config::Shown) -> aio::config::Shown {
    match stamp {
        Some("browser") => aio::config::Shown::Browser,
        Some("window") => aio::config::Shown::Window,
        _ => app,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_launchs_client_picks_how_the_app_shows() {
        // aio's --client=X, per run over `[app] ui`.
        use aio::config::Shown;
        assert_eq!(shown(None, Shown::Browser).unwrap(), Some(Shown::Browser));
        assert_eq!(shown(Some("electron"), Shown::Browser).unwrap(), Some(Shown::Window));
        assert_eq!(shown(Some("window"), Shown::Browser).unwrap(), Some(Shown::Window));
        assert_eq!(shown(Some("browser"), Shown::Window).unwrap(), Some(Shown::Browser));
        assert_eq!(shown(Some("server-only"), Shown::Window).unwrap(), None);
        assert!(shown(Some("cli"), Shown::Window).unwrap_err().to_string().contains("one of window"));
    }

    #[test]
    fn a_targets_stamped_ui_wins_over_the_apps() {
        use aio::config::Shown::{Browser, Window};
        assert_eq!(built(Some("browser"), Window), Browser);
        assert_eq!(built(Some("window"), Browser), Window);
        assert_eq!(built(None, Browser), Browser);
    }

    #[test]
    fn paths_resolve_against_the_launchs_dir() {
        let l = Launch { cwd: "/home/u/docs".into(), args: vec![] };
        assert_eq!(Path::new(&l.path("a.md")), Path::new("/home/u/docs").join("a.md"));
        #[cfg(unix)]
        assert_eq!(l.path("/x/b.md"), "/x/b.md");
        #[cfg(windows)]
        assert_eq!(l.path(r"C:\x\b.md"), r"C:\x\b.md");
        assert_eq!(l.path("https://e.com/c.md"), "https://e.com/c.md");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_launch_round_trips() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let d = tempfile::tempdir().unwrap();
        let addr = aio::ipc::addr(d.path(), &format!("hand{}", std::process::id()));
        let mut l = aio::ipc::Listener::bind(&addr).unwrap();
        let server = tokio::spawn(async move {
            let (r, mut w) = tokio::io::split(l.accept().await.unwrap());
            let mut line = String::new();
            tokio::io::BufReader::new(r).read_line(&mut line).await.unwrap();
            w.write_all(b"ok\n").await.unwrap();
            read_launch(&line).unwrap()
        });
        let a = addr.clone();
        tokio::task::spawn_blocking(move || hand_over(&a, Path::new("/w"), &["a b.md".into()])).await.unwrap().unwrap();
        assert_eq!(server.await.unwrap(), Launch { cwd: "/w".into(), args: vec!["a b.md".into()] });
        assert!(read_launch("{\"cwd\":1}").is_none());
    }

    #[test]
    fn a_takeover_stops_the_running_one_and_takes_its_place() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let d = tempfile::tempdir().unwrap();
        let id = format!("takeover-{}", std::process::id());
        let env = |k: &str| (k == "AIORT_DATA").then(|| d.path().to_path_buf());
        let plan = aio::paths::plan(&id, None, &aio::paths::Ask::default(), &env).unwrap();
        let running = aio::paths::AppDirs::planned(&id, plan.clone()).unwrap();
        assert_eq!(claim(&id, &plan, false).unwrap_err().kind(), io::ErrorKind::WouldBlock);
        // No one answers: the takeover says so, and takes nothing.
        assert!(claim(&id, &plan, true).unwrap_err().to_string().contains("--takeover"));
        // The running one stops when asked, as the shell's does.
        let _in = aio::rt().enter();
        let mut l = aio::ipc::Listener::bind(&hand_addr(&id, &plan.data).unwrap()).unwrap();
        aio::rt().spawn(async move {
            let (r, mut w) = tokio::io::split(l.accept().await.unwrap());
            let mut line = String::new();
            tokio::io::BufReader::new(r).read_line(&mut line).await.unwrap();
            assert_eq!(line.trim(), QUIT);
            w.write_all(b"ok\n").await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            drop(running);
        });
        let dirs = claim(&id, &plan, true).unwrap();
        assert!(aio::paths::running(&dirs.data));
        _ = std::fs::remove_dir_all(aio::paths::sockets(&id, &dirs.data).unwrap());
    }

    #[test]
    fn a_servers_door_takes_a_stop() {
        static STOPPED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let d = tempfile::tempdir().unwrap();
        let addr = aio::ipc::addr(d.path(), &format!("stops{}", std::process::id()));
        let _in = aio::rt().enter();
        let l = aio::ipc::Listener::bind(&addr).unwrap();
        let stop = || _ = STOPPED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        aio::rt().spawn(crate::take_launches(l, Default::default(), stop, || false));
        exchange(&addr, QUIT.into()).unwrap();
        // The door answers, then stops: the stop lands just after the reply.
        let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while STOPPED.load(std::sync::atomic::Ordering::SeqCst) == 0 && std::time::Instant::now() < end {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(STOPPED.load(std::sync::atomic::Ordering::SeqCst), 1, "asked once, stopped once");
    }

    #[test]
    fn an_ending_instance_takes_no_launch_and_the_launch_starts_after_it() {
        let d = tempfile::tempdir().unwrap();
        let id = format!("ending-{}", std::process::id());
        let env = |k: &str| (k == "AIORT_DATA").then(|| d.path().to_path_buf());
        let plan = aio::paths::plan(&id, None, &aio::paths::Ask::default(), &env).unwrap();
        let running = aio::paths::AppDirs::planned(&id, plan.clone()).unwrap();
        let _in = aio::rt().enter();
        let l = aio::ipc::Listener::bind(&hand_addr(&id, &plan.data).unwrap()).unwrap();
        aio::rt().spawn(crate::take_launches(l, Default::default(), || {}, || true));
        let addr = hand_addr(&id, &plan.data).unwrap();
        let busy = hand_over(&addr, Path::new("/"), &[]).unwrap_err();
        assert_eq!(busy.kind(), io::ErrorKind::ResourceBusy, "{busy}");
        assert!(ending(&busy));
        // Its door already gone as it stops: ending too; a refusal of another kind isn't.
        let gone = hand_over(&aio::ipc::addr(d.path(), "no-door"), Path::new("/"), &[]).unwrap_err();
        assert!(ending(&gone), "{gone:?}");
        assert!(!ending(&io::Error::other("the running app said \"bad launch\"")));
        // It lets go of the lock a moment later: the launch then has it.
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            drop(running);
        });
        let dirs = wait_lock(&id, &plan).unwrap();
        assert!(aio::paths::running(&dirs.data));
        _ = std::fs::remove_dir_all(aio::paths::sockets(&id, &dirs.data).unwrap());
    }

    /// A launch while the running instance starts (its lock taken, its door
    /// not yet open: a double click, two files opened at once) is handed
    /// over once the door opens, never lost after a 30 s wait.
    #[test]
    fn a_launch_while_the_app_starts_is_handed_over_once_its_door_opens() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let d = tempfile::tempdir().unwrap();
        let id = format!("starting-{}", std::process::id());
        let env = |k: &str| (k == "AIORT_DATA").then(|| d.path().to_path_buf());
        let plan = aio::paths::plan(&id, None, &aio::paths::Ask::default(), &env).unwrap();
        let _running = aio::paths::AppDirs::planned(&id, plan.clone()).unwrap();
        let addr = hand_addr(&id, &plan.data).unwrap();
        let _in = aio::rt().enter();
        let took = aio::rt().spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let mut l = aio::ipc::Listener::bind(&addr).unwrap();
            let (r, mut w) = tokio::io::split(l.accept().await.unwrap());
            let mut line = String::new();
            tokio::io::BufReader::new(r).read_line(&mut line).await.unwrap();
            w.write_all(b"ok\n").await.unwrap();
            read_launch(&line).unwrap().args
        });
        let at = std::time::Instant::now();
        assert!(claim_or_hand(&id, &plan, false, &["a.md".into()]).unwrap().is_none(), "handed over");
        assert!(at.elapsed() < std::time::Duration::from_secs(10), "{:?}", at.elapsed());
        assert_eq!(aio::rt().block_on(took).unwrap(), ["a.md"]);
        _ = std::fs::remove_dir_all(aio::paths::sockets(&id, &plan.data).unwrap());
    }

    /// A launch that connects and never says its line (a frozen process)
    /// holds up no other: the next is taken at once, not after it.
    #[test]
    fn a_silent_launch_holds_up_no_other() {
        let d = tempfile::tempdir().unwrap();
        let addr = aio::ipc::addr(d.path(), &format!("silent{}", std::process::id()));
        let _in = aio::rt().enter();
        let l = aio::ipc::Listener::bind(&addr).unwrap();
        aio::rt().spawn(crate::take_launches(l, Default::default(), || {}, || false));
        let _silent = aio::ipc::connect_blocking(&addr).unwrap();
        let at = std::time::Instant::now();
        hand_over(&addr, Path::new("/"), &[]).unwrap();
        assert!(at.elapsed() < std::time::Duration::from_secs(5), "{:?}", at.elapsed());
    }

    #[test]
    fn a_servers_door_takes_no_launch_once_stopping() {
        // Its launch brings the process's window forward: no other test's.
        let _turn = crate::WINDOW_TURN.blocking_lock();
        let d = tempfile::tempdir().unwrap();
        let id = format!("stopping-{}", std::process::id());
        let _in = aio::rt().enter();
        // Listening, as a server's main is: a stop asked is then no signal.
        let wait = aio::lifecycle::stop_asked().unwrap();
        open_door(&id, d.path());
        let addr = hand_addr(&id, d.path()).unwrap();
        hand_over(&addr, Path::new("/"), &[]).unwrap();
        aio::lifecycle::ask_stop();
        let busy = hand_over(&addr, Path::new("/"), &[]).unwrap_err();
        assert_eq!(busy.kind(), io::ErrorKind::ResourceBusy, "{busy}");
        // The stop taken here, so no other test's window hears it.
        aio::rt().block_on(wait);
        _ = std::fs::remove_dir_all(aio::paths::sockets(&id, d.path()).unwrap());
    }

    /// A launch handed over while the app starts (its door open, nothing
    /// listening yet: Electron downloading) waits for the app's first
    /// listener, never lost.
    #[test]
    fn a_launch_before_the_app_listens_waits_for_it() {
        let _turn = crate::WINDOW_TURN.blocking_lock();
        let d = tempfile::tempdir().unwrap();
        let id = format!("early-{}", std::process::id());
        let _in = aio::rt().enter();
        open_door(&id, d.path());
        let addr = hand_addr(&id, d.path()).unwrap();
        hand_over(&addr, Path::new("/w"), &["early.md".into()]).unwrap();
        let mut got = launches();
        let early = async {
            loop {
                let l = got.recv().await.unwrap();
                if l.args == ["early.md"] {
                    return l;
                }
            }
        };
        let l = aio::rt().block_on(async { tokio::time::timeout(std::time::Duration::from_secs(5), early).await });
        assert_eq!(l.expect("never heard").path("early.md"), Path::new("/w/early.md").to_string_lossy());
        // `start` opens it for every app but a CLI, before its window.
        assert!(
            include_str!("instance.rs")
                .contains("if cfg.app.mode != aio::config::Mode::Cli {\n        open_door(id, &d.data);")
        );
        _ = std::fs::remove_dir_all(aio::paths::sockets(&id, d.path()).unwrap());
    }

    #[test]
    fn a_silent_instance_is_an_error_not_a_hang() {
        let nowhere = aio::ipc::addr(Path::new("/nonexistent/aio-rt/none-00000000"), "hand");
        assert!(hand_over(&nowhere, Path::new("/"), &[]).is_err());
    }
}

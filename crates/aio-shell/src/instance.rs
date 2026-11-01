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

/// Later launches, for the app to act on (open the file it was given).
pub fn launches() -> broadcast::Receiver<Launch> {
    channel().subscribe()
}

pub(crate) fn deliver(l: Launch) {
    _ = channel().send(l);
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
/// `[app] ui` of the app that started (`start`).
pub(crate) static SHOWN: std::sync::OnceLock<aio::config::Shown> = std::sync::OnceLock::new();
/// Its hue (`[ui] hue`, else its id's) and its splash: what is drawn
/// outside its page (`splash`, `firstrun`).
pub(crate) static LOOK: std::sync::OnceLock<(u16, Option<aio::config::Splash>)> = std::sync::OnceLock::new();
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

/// A server's door (as a window's: it takes later launches, and `aio
/// stop` asks here, the graceful way on every OS: Windows has no SIGTERM,
/// and a kill would skip the last save and restart under the supervisor).
/// No door is not fatal: SIGTERM still stops it where there is one.
pub(crate) fn take_stops(id: &str, data: &Path) {
    let _rt = aio::rt().enter();
    match hand_addr(id, data).and_then(|a| aio::ipc::Listener::bind(&a)) {
        Ok(l) => _ = aio::rt().spawn(crate::take_launches(l, Default::default(), aio::lifecycle::ask_stop)),
        Err(e) => eprintln!("[aio] {id}: no door for `ax stop` ({e})"),
    }
}

/// Take the app's lock and boot it (`aio::boot`: log, reports, health, the
/// boot report), or hand `args` to the instance holding it. `version`: the
/// app's, `env!("CARGO_PKG_VERSION")`.
pub fn start(cfg: &aio::config::Config, version: &str) -> io::Result<Start> {
    // Windows: before anything starts a console program (the installer's
    // `reg`, a dialog's PowerShell, the app's own), so none opens a window.
    aio::process::hide_console();
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
    let plan = aio::paths::plan_app(cfg, &ask)?;
    // `<app> --quit`: the instance running on these dirs is asked to stop,
    // the graceful way (Windows has no SIGTERM: scripts, the smoke test).
    if args.iter().any(|a| a == "--quit") {
        return quit_at(id, &plan.data).map(|()| Start::Done);
    }
    match claim(id, &plan, ask.takeover) {
        Ok(d) => {
            aio::boot::begin(cfg, version, &d)?;
            crate::feedback::wire(cfg);
            _ = APP.set((id.into(), version.into()));
            _ = BUNDLE_ID.set(cfg.mac_bundle_id());
            _ = SHOWN.set(cfg.app.ui);
            _ = LOOK.set((cfg.hue(), cfg.build.splash.clone()));
            _ = crate::firstrun::WORDS.set(crate::firstrun::words_of(cfg));
            if let crate::update::Next::Exit = crate::update::on_start(cfg, version, &d)? {
                aio::health::stop();
                aio::log::finish();
                return Ok(Start::Done);
            }
            // Updates are never a reason not to start.
            if let Err(e) = crate::update::open(cfg, version, &d) {
                eprintln!("[aio] no updates cell: {e}");
            }
            // No window to wait for (a server, a CLI): booted is started.
            if matches!(cfg.app.mode, aio::config::Mode::Server | aio::config::Mode::Cli) {
                crate::update::confirm(id, version);
            }
            if cfg.app.mode == aio::config::Mode::Server {
                take_stops(id, &d.data);
            }
            crate::update::watch(cfg, version, &d.data);
            Ok(Start::Run(d))
        }
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
            // Held by aio (or an aio-rs not answering): say who holds it.
            hand_over(&hand_addr(id, &plan.data)?, &std::env::current_dir()?, &args)
                .map_err(|h| io::Error::new(h.kind(), format!("{e}; {h}")))?;
            Ok(Start::HandedOver)
        }
        Err(e) => Err(e),
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

/// Send one launch, wait for the running instance's "ok".
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
    if reply.trim() != "ok" {
        return Err(io::Error::other(format!("the running app said {reply:?}")));
    }
    Ok(())
}

/// Read one launch from a connection (the running side).
pub(crate) fn read_launch(line: &str) -> Option<Launch> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let args = v["args"].as_array()?.iter().map(|a| a.as_str().map(String::from)).collect::<Option<Vec<_>>>()?;
    Some(Launch { cwd: PathBuf::from(v["cwd"].as_str()?), args })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        aio::rt().spawn(crate::take_launches(l, Default::default(), stop));
        exchange(&addr, QUIT.into()).unwrap();
        // The door answers, then stops: the stop lands just after the reply.
        let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while STOPPED.load(std::sync::atomic::Ordering::SeqCst) == 0 && std::time::Instant::now() < end {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(STOPPED.load(std::sync::atomic::Ordering::SeqCst), 1, "asked once, stopped once");
    }

    #[test]
    fn a_silent_instance_is_an_error_not_a_hang() {
        let nowhere = aio::ipc::addr(Path::new("/nonexistent/aio-rt/none-00000000"), "hand");
        assert!(hand_over(&nowhere, Path::new("/"), &[]).is_err());
    }
}

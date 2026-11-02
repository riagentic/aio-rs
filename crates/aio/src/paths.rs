//! Where things live: OS standard dirs under `aio-rt`, never aio's `aio` or
//! `~/.<appId>`, so aio and aio-rs builds of an app run side by side. A ported
//! app (`[app] port_of`) is the exception: it runs on its aio home and data;
//! so is the Electron store, aio's and aio-rs's in one ([`electron_store`]).
use std::io;
use std::path::PathBuf;

/// A variable, set and not empty (tests pass their own).
pub type Env<'a> = &'a dyn Fn(&str) -> Option<PathBuf>;

fn env(k: &str) -> Option<PathBuf> {
    if let Some(root) = sandboxed() {
        match k {
            "HOME" | "USERPROFILE" => return Some(root.join("home")),
            "LOCALAPPDATA" => return Some(root.join("home/AppData/Local")),
            k if k.starts_with("XDG_") => return None,
            // The caller's own sandbox (a wrapper's) is not the test's
            // world; a test's own, in a temp dir, is.
            "AIORT_DATA" | "AIO_SHARED_ELECTRON_DIR" => {
                return real_env(k).filter(|p| p.starts_with(std::env::temp_dir()));
            }
            _ => {}
        }
    }
    real_env(k)
}

fn real_env(k: &str) -> Option<PathBuf> {
    std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// A test's world of its own: (its root, the real home, harnesses using it).
static SANDBOX: std::sync::Mutex<Option<(PathBuf, PathBuf, usize)>> = std::sync::Mutex::new(None);

fn sandbox_lock() -> std::sync::MutexGuard<'static, Option<(PathBuf, PathBuf, usize)>> {
    SANDBOX.lock().unwrap_or_else(|e| e.into_inner())
}

fn sandboxed() -> Option<PathBuf> {
    sandbox_lock().as_ref().map(|(root, _, _)| root.clone())
}

/// While one lives, the process runs in a fresh temp world: the home and
/// every OS data dir aio would use are under [`Sandbox::root`], and opening
/// app data under the real home is refused (a temp dir is fine). The test
/// harnesses (`aio::testing`) hold one, so a test never reaches the user's
/// data; the last one dropped removes the world.
pub struct Sandbox(PathBuf);

impl Sandbox {
    pub fn root(&self) -> &std::path::Path {
        &self.0
    }
}

/// Does the process run in the test world now (a [`Sandbox`] lives)? Then
/// nothing reaches the person's browser or apps (`AIORT_NO_OPEN`).
pub fn in_sandbox() -> bool {
    sandboxed().is_some()
}

/// Enter (or share) the test world.
pub fn sandbox() -> io::Result<Sandbox> {
    let real = home_in(&real_env);
    let mut s = sandbox_lock();
    match s.as_mut() {
        Some((root, _, n)) => {
            *n += 1;
            Ok(Sandbox(root.clone()))
        }
        None => {
            let root =
                std::env::temp_dir().join(format!("aio-test-{}-{}", std::process::id(), crate::uuid_v4().simple()));
            std::fs::create_dir_all(root.join("home"))?;
            *s = Some((root.clone(), real, 1));
            Ok(Sandbox(root))
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let mut s = sandbox_lock();
        if let Some((root, _, n)) = s.as_mut() {
            *n -= 1;
            if *n == 0 {
                // Its app's lock is let go first (Windows removes no open file).
                *lock(&TEST_APP) = None;
                _ = std::fs::remove_dir_all(&*root);
                *s = None;
            }
        }
    }
}

/// In the test world, data under the real home (outside the temp dir) is
/// not this test's: refused, naming it.
pub(crate) fn guard(p: &std::path::Path) -> io::Result<()> {
    let Some((_, real, _)) = sandbox_lock().clone() else { return Ok(()) };
    let p = std::path::absolute(p)?;
    let tmp = std::path::absolute(std::env::temp_dir())?;
    if !real.as_os_str().is_empty() && p.starts_with(&real) && !p.starts_with(&tmp) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("a test may not touch real app data: {} (use a temp dir, or the harness's sandbox)", p.display()),
        ));
    }
    Ok(())
}

fn home_in(env: Env) -> PathBuf {
    env("HOME").or_else(|| env("USERPROFILE")).unwrap_or_default()
}

pub(crate) fn home() -> PathBuf {
    home_in(&env)
}

fn tool_home_in(env: Env) -> PathBuf {
    match std::env::consts::OS {
        "macos" => home_in(env).join("Library/Application Support/aio-rt"),
        "windows" => env("LOCALAPPDATA").unwrap_or_else(|| home_in(env).join("AppData/Local")).join("aio-rt"),
        _ => env("XDG_DATA_HOME").unwrap_or_else(|| home_in(env).join(".local/share")).join("aio-rt"),
    }
}

/// aio-rt's tool home: the shared runtime, app data.
pub fn tool_home() -> PathBuf {
    tool_home_in(&env)
}

/// The Electron version aio pins: the shared runtime windows open in
/// (aio-shell fetches it) and the UI tests drive (`aio::testing`).
#[doc(hidden)]
#[macro_export]
macro_rules! electron_version {
    () => {
        "44.9.3"
    };
}

/// [`electron_version!`] as a value.
pub const ELECTRON: &str = electron_version!();

/// Electron's name for this OS and CPU (`linux-x64`, `darwin-arm64`).
pub fn electron_slug() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        o => o,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        a => a,
    };
    format!("{os}-{arch}")
}

/// The executable inside the Electron runtime in `dir`, for `os`
/// (`std::env::consts::OS`'s word).
pub fn electron_exe_for(os: &str, dir: &std::path::Path) -> PathBuf {
    match os {
        "macos" => dir.join("Electron.app/Contents/MacOS/Electron"),
        "windows" => dir.join("electron.exe"),
        _ => dir.join("electron"),
    }
}

/// The shared Electron store, aio's own (`shared-runtime.ts`
/// `sharedElectronRoot`): aio and aio-rs apps start from one copy per
/// version. `$AIO_SHARED_ELECTRON_DIR` moves it, as it moves aio's.
pub fn electron_store() -> PathBuf {
    electron_store_in(std::env::consts::OS, &env)
}

/// [`electron_store`] on `os` (`std::env::consts::OS`'s word) with `env`
/// (pure): spelled as aio's `join` spells it, so a path in a record matches.
pub fn electron_store_in(os: &str, env: Env) -> PathBuf {
    let root = match env("AIO_SHARED_ELECTRON_DIR") {
        Some(p) => return std::path::absolute(&p).unwrap_or(p).components().collect(),
        None if os == "windows" => env("LOCALAPPDATA").unwrap_or_else(|| home_in(env).join("AppData").join("Local")),
        None if os == "macos" => home_in(env).join("Library").join("Application Support"),
        None => env("XDG_DATA_HOME").unwrap_or_else(|| home_in(env).join(".local").join("share")),
    };
    root.join("aio").join("electron").components().collect()
}

/// Where development builds before 0.1.0 kept their own store (`aio-rt/electron`): what is
/// there moves into [`electron_store`] on first use.
pub fn old_electron_store() -> PathBuf {
    tool_home().join("electron")
}

/// The pinned runtime in the person's own store, a test sandbox or not:
/// what the UI tests draw in (they only run it).
#[cfg(all(feature = "kit", feature = "testing"))]
pub(crate) fn electron_pinned() -> PathBuf {
    electron_store_in(std::env::consts::OS, &real_env).join(format!("{ELECTRON}-{}", electron_slug()))
}

/// aio-rt's cache home: what apps can make again.
pub fn cache_home() -> PathBuf {
    cache_home_in(&env)
}

fn cache_home_in(env: Env) -> PathBuf {
    match std::env::consts::OS {
        "macos" => home_in(env).join("Library/Caches/aio-rt"),
        "windows" => env("LOCALAPPDATA").unwrap_or_else(|| home_in(env).join("AppData/Local")).join("aio-rt/cache"),
        _ => env("XDG_CACHE_HOME").unwrap_or_else(|| home_in(env).join(".cache")).join("aio-rt"),
    }
}

/// Make `d` (and its parents) and keep it private: mode 0700.
pub fn private_dir(d: &std::path::Path) -> io::Result<()> {
    std::fs::create_dir_all(d)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        // A shared temp dir: another user may have made it first, or put a
        // symlink there. Only a real dir of our own is used.
        let m = std::fs::symlink_metadata(d)?;
        // SAFETY: getuid has no preconditions.
        if !m.is_dir() || m.uid() != unsafe { libc::getuid() } {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{} is not a directory of this user's; refusing to use it", d.display()),
            ));
        }
        std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn create(p: &std::path::Path, private: bool) -> io::Result<std::fs::File> {
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    if private {
        std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    }
    // Windows: a file under the user's profile is already this user's alone.
    #[cfg(not(unix))]
    let _ = private;
    o.open(p)
}

/// Flush a file's bytes to disk (opened for writing: Windows flushes no
/// read-only handle).
pub(crate) fn sync_file(p: &std::path::Path) -> io::Result<()> {
    std::fs::OpenOptions::new().write(true).open(p)?.sync_all()
}

/// Flush a directory's entries (a new or renamed file in it) to disk.
pub(crate) fn sync_dir(d: &std::path::Path) -> io::Result<()> {
    #[cfg(unix)]
    std::fs::File::open(d)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = d;
    Ok(())
}

pub(crate) fn parent(p: &std::path::Path) -> &std::path::Path {
    p.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."))
}

/// Write a new private file (mode 0600), on disk when this returns; an
/// existing one is an error.
pub fn write_private(p: &std::path::Path, bytes: &[u8]) -> io::Result<()> {
    let mut f = create(p, true)?;
    std::io::Write::write_all(&mut f, bytes)?;
    f.sync_all()?;
    sync_dir(parent(p))
}

/// Replace `p` with `bytes` so that a crash leaves the old file or the new
/// one, never half of one (D2): a temp file, flushed, renamed over, then the
/// directory flushed. `private`: mode 0600.
pub fn write_atomic(p: &std::path::Path, bytes: &[u8], private: bool) -> io::Result<()> {
    let name = p.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    let tmp = p.with_file_name(format!(".{}.{}.tmp", name.to_string_lossy(), crate::uuid_v4().simple()));
    let written = (|| {
        let mut f = create(&tmp, private)?;
        std::io::Write::write_all(&mut f, bytes)?;
        f.sync_all()?;
        rename_over(&tmp, p)
    })();
    if written.is_err() {
        _ = std::fs::remove_file(&tmp);
    }
    written?;
    sync_dir(parent(p))
}

/// Pauses between tries of a rename that Windows refuses while another
/// program (a second launch, a backup or sync tool, a scanner) has the
/// target open: 10 more tries, 1.3 s in all. A reader holds a small file
/// for milliseconds.
pub const BACKOFF_MS: [u64; 10] = [5, 10, 20, 40, 80, 160, 250, 250, 250, 250];
/// A target that stayed held through the whole wait is tried once, with no
/// wait, for this long: a file held for minutes must not cost every write
/// another 1.3 s. The first write after it is let go still works.
pub const HELD_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(30);

/// Is `e` Windows saying another process has the file open (pure)? Access
/// denied, busy, or a sharing or lock violation (os error 32, 33).
pub fn held_open(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::PermissionDenied | io::ErrorKind::ResourceBusy)
        || matches!(e.raw_os_error(), Some(32 | 33))
}

/// Rename `from` over `to`. On Unix that either works or fails for a reason
/// waiting cannot cure; on Windows a held target is tried again ([`BACKOFF_MS`]).
pub fn rename_over(from: &std::path::Path, to: &std::path::Path) -> io::Result<()> {
    retried(|| std::fs::rename(from, to), cfg!(windows), to, &|ms| {
        std::thread::sleep(std::time::Duration::from_millis(ms))
    })
}

/// `op` until it works, fails for another reason, or the wait is spent (no
/// wait for a target in its cool-down, or off Windows). The last error
/// says what it can mean.
fn retried(
    mut op: impl FnMut() -> io::Result<()>,
    windows: bool,
    target: &std::path::Path,
    pause: &dyn Fn(u64),
) -> io::Result<()> {
    static HELD: std::sync::Mutex<Vec<(PathBuf, std::time::Instant)>> = std::sync::Mutex::new(Vec::new());
    let held = || HELD.lock().unwrap_or_else(|e| e.into_inner());
    let cooling = held().iter().any(|(p, until)| p == target && std::time::Instant::now() < *until);
    let (mut tries, mut waited) = (0, 0);
    loop {
        tries += 1;
        let e = match op() {
            Ok(()) => {
                held().retain(|(p, _)| p != target);
                return Ok(());
            }
            Err(e) if !(windows && held_open(&e)) => return Err(e),
            Err(e) => e,
        };
        match BACKOFF_MS.get(tries - 1).filter(|_| !cooling) {
            Some(&ms) => {
                pause(ms);
                waited += ms;
            }
            None => {
                let mut h = held();
                h.retain(|(p, _)| p != target);
                if h.len() >= 64 {
                    h.remove(0);
                }
                h.push((target.into(), std::time::Instant::now() + HELD_COOLDOWN));
                let what = format!(
                    "could not replace {} after {} over {waited} ms: {e}: it is open in another program, or this user may not write it",
                    target.display(),
                    crate::fmt::count(tries as u64, "try", Some("tries"))
                );
                return Err(io::Error::new(e.kind(), what));
            }
        }
    }
}

/// A temp dir short enough for Unix sockets made under it (108 bytes of
/// path): the temp dir, or `/tmp` when it is long (an agent's scratch
/// folder), so what binds there passes whoever runs it.
pub fn short_temp() -> PathBuf {
    let t = std::env::temp_dir();
    if cfg!(unix) && t.as_os_str().len() > 48 { "/tmp".into() } else { t }
}

/// A short, private dir for the app's sockets (Unix socket paths must stay
/// under ~100 bytes, so never inside a deep data dir): `$XDG_RUNTIME_DIR/
/// aio-rt/<id>-<hash of the data dir>`, else the temp dir; when that is too
/// long for a socket in it, `/tmp/aio-rt-<uid>/<id, 40 bytes at most>-<hash>`
/// (aio's fallback, `resolveSocketPath`). Mode 0700.
pub fn sockets(id: &str, data: &std::path::Path) -> io::Result<PathBuf> {
    let dir = sockets_at(id, data);
    private_dir(parent(&dir))?;
    private_dir(&dir)?;
    Ok(dir)
}

/// Where [`sockets`] puts them, without making anything.
pub fn sockets_at(id: &str, data: &std::path::Path) -> PathBuf {
    // FNV-1a: stable across runs and builds (std's hasher isn't). Over the
    // path rebuilt from its parts: `a/b` and `a\b` are one folder on Windows.
    let data: PathBuf = data.components().collect();
    let h = data.to_string_lossy().bytes().fold(0x811c_9dc5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193));
    let base = env("XDG_RUNTIME_DIR").unwrap_or_else(std::env::temp_dir);
    #[cfg(unix)]
    // SAFETY: getuid has no preconditions.
    let uid = Some(unsafe { libc::getuid() });
    #[cfg(not(unix))]
    let uid = None; // a pipe's name has no such limit
    socket_dir(&base, id, h, uid)
}

/// `<base>/aio-rt/<id>-<h>`, or for a Unix user (`uid`) whose base leaves no
/// room for a 24-byte socket name (`cell-<name>.sock`) in aio's 100 bytes of
/// path (`ipc::SUN_PATH` is 108 on Linux, 104 on macOS),
/// `/tmp/aio-rt-<uid>/<id, 40 bytes at most>-<h>`. Pure.
fn socket_dir(base: &std::path::Path, id: &str, h: u32, uid: Option<u32>) -> PathBuf {
    let dir = base.join("aio-rt").join(format!("{id}-{h:08x}"));
    match uid {
        Some(uid) if dir.as_os_str().len() + "/".len() + 24 > 100 => {
            let id: String = id.char_indices().take_while(|(i, c)| i + c.len_utf8() <= 40).map(|(_, c)| c).collect();
            PathBuf::from(format!("/tmp/aio-rt-{uid}")).join(format!("{id}-{h:08x}"))
        }
        _ => dir,
    }
}

fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The dirs the app booted on ([`booted`]).
static BOOTED: std::sync::Mutex<Option<AppDirs>> = std::sync::Mutex::new(None);

/// The test world's app dirs, with the world's root they are under.
static TEST_APP: std::sync::Mutex<Option<(PathBuf, AppDirs)>> = std::sync::Mutex::new(None);

/// The running app's dirs, asked from anywhere in it: the ones its `main`
/// booted on (`aio_shell::start`, `aio::boot::begin`); in the test world
/// (a `TestUi`, `TestCells`), the world's own, `<sandbox>/app` (`data`,
/// `logs`, `cache`; made on the first ask, gone with the world), so app
/// code finds its home the same way in the window and in a test.
pub fn app_dirs() -> io::Result<AppDirs> {
    if let Some(root) = sandboxed() {
        let mut t = lock(&TEST_APP);
        if let Some((at, d)) = t.as_ref()
            && *at == root
        {
            return Ok(d.clone());
        }
        let d = AppDirs::at(&root.join("app"), "app")?;
        *t = Some((root, d.clone()));
        return Ok(d);
    }
    lock(&BOOTED).clone().ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "no app dirs yet: the app's main boots them (aio_shell::start)")
    })
}

/// The app booted on `dirs`: [`app_dirs`] answers them from now on.
pub(crate) fn booted(dirs: &AppDirs) {
    *lock(&BOOTED) = Some(dirs.clone());
}

/// One app's dirs: `data` (backed up), `logs` (rotated), `cache` (rebuildable,
/// never backed up), and [`AppDirs::files`] in `data`. `data` and `logs` are private (0700). Holding it holds
/// the app's single-instance lock: one process writes its log.
/// A ported app's lock is aio's home claim (`compat::data::Home`) instead.
#[derive(Clone, Debug)]
pub struct AppDirs {
    pub id: String,
    pub data: PathBuf,
    pub logs: PathBuf,
    pub cache: PathBuf,
    /// The store's file: `<data>/state.db`, or the app's `[data] store`.
    pub store: PathBuf,
    /// The profile it runs as (`--profile`), if any.
    pub profile: Option<String>,
    /// A ported app's aio app id: its store is aio's `state.db` (`open_aio`).
    pub port_of: Option<String>,
    /// Who chose the dirs, for the boot report.
    pub from: &'static str,
    _lock: std::sync::Arc<Held>,
}

/// A file whose OS lock is let go when the last holder drops it, not when
/// the last copy of its descriptor closes: a child forked meanwhile (any
/// spawn, until its exec) shares that lock and would keep it held.
#[derive(Debug)]
pub(crate) struct Held(pub(crate) std::fs::File);

impl Drop for Held {
    fn drop(&mut self) {
        _ = self.0.unlock();
    }
}

pub(crate) use aio_config::paths::id_ok;
/// Where app `id`'s (data, logs, cache) dirs are, without creating or locking.
pub fn locate(id: &str) -> io::Result<(PathBuf, PathBuf, PathBuf)> {
    id_ok(id)?;
    Ok(match env("AIORT_DATA") {
        Some(root) => (root.join("data"), root.join("logs"), root.join("cache")),
        None => {
            let base = tool_home().join("apps").join(id);
            (base.join("data"), base.join("logs"), cache_home_in(&env).join(id))
        }
    })
}

/// What a launch asks of where the app runs and serves, in aio's flags.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ask {
    /// `--home=<dir>`: that folder.
    pub home: Option<String>,
    /// `--profile=<name|path>`: a second home, `<home>-<name>` (else
    /// `AIORT_PROFILE`, and aio's `AIO_PROFILE` for a ported app).
    pub profile: Option<String>,
    /// `--instance=<name>`: a private world of its own, apart from the
    /// user's (agents, tests).
    pub instance: Option<String>,
    /// `--takeover`: the running instance is asked to stop, and this one
    /// takes its place.
    pub takeover: bool,
    /// `--port=<n>` and `--host=<addr>`: where a serving app listens, over
    /// `[server] listen` ([`crate::config::listen`]).
    pub port: Option<String>,
    pub host: Option<String>,
    /// `--expose`: served to the network, over TLS.
    pub expose: bool,
    /// `--no-tls` (aio's): no TLS even where the launch would turn it on
    /// (`--expose`) or `[server] tls` asks; said loudly when exposed.
    pub no_tls: bool,
    /// `--tls-cert=<pem>` and `--tls-key=<pem>` (aio's): the certificate
    /// chain and key to serve TLS with, over `[server] cert` and `key`.
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
    /// `--keep-server` (aio's): a desktop app goes on serving once its
    /// window closes, until asked to stop (`Window { keep_server }`).
    pub keep_server: bool,
    /// A debug build: a ported app's `[home] dev` applies ([`plan_app`]
    /// asks it in a debug build of the app).
    pub dev: bool,
    /// `--no-persist` (aio's): this run keeps its state in memory only.
    pub no_persist: bool,
}

/// The flags aio takes from a launch for itself, spelled once: a flag
/// with `=` takes a value (`--home=<dir>` or `--home <dir>`).
pub const FLAGS: [&str; 12] = [
    "--home=",
    "--profile=",
    "--instance=",
    "--takeover",
    "--port=",
    "--host=",
    "--expose",
    "--keep-server",
    "--no-persist",
    "--no-tls",
    "--tls-cert=",
    "--tls-key=",
];

impl Ask {
    /// A launch's `args` without aio's own [`FLAGS`] (and their values), in
    /// order: what the app's own argument parser reads, so the same binary
    /// answers both and neither refuses the other's flags.
    pub fn app_args(args: &[String]) -> Vec<String> {
        let mut out = vec![];
        let mut it = args.iter();
        while let Some(a) = it.next() {
            let flag = a.split_once('=').map_or(a.as_str(), |(f, _)| f);
            match FLAGS.iter().find(|f| f.trim_end_matches('=') == flag) {
                // A value flag without `=`: its value is the next arg.
                Some(f) if f.ends_with('=') && !a.contains('=') => _ = it.next(),
                Some(_) => {}
                None => out.push(a.clone()),
            }
        }
        out
    }

    /// The asks among a launch's `args` (`--home=<dir>` or `--home <dir>`);
    /// the others are the app's ([`Self::app_args`]).
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut a = Ask::default();
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--takeover" => a.takeover = true,
                "--expose" => a.expose = true,
                "--keep-server" => a.keep_server = true,
                "--no-persist" => a.no_persist = true,
                "--no-tls" => a.no_tls = true,
                _ => {}
            }
            let (flag, inline) = arg.split_once('=').map_or((arg.as_str(), None), |(f, v)| (f, Some(v)));
            let slot = match flag {
                "--home" => &mut a.home,
                "--profile" => &mut a.profile,
                "--instance" => &mut a.instance,
                "--port" => &mut a.port,
                "--host" => &mut a.host,
                "--tls-cert" => &mut a.tls_cert,
                "--tls-key" => &mut a.tls_key,
                _ => continue,
            };
            let v = match inline {
                Some(v) => v.to_string(),
                None => it.next().cloned().ok_or_else(|| format!("{flag} needs a value: {flag}=<…>"))?,
            };
            if slot.replace(v).is_some() {
                return Err(format!("{flag} is given twice"));
            }
        }
        Ok(a)
    }
}

/// Where an app's dirs are, decided before anything is made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub data: PathBuf,
    pub logs: PathBuf,
    pub cache: PathBuf,
    /// The store's file: `<data>/state.db`, or `[data] store` ([`plan_app`]).
    pub store: PathBuf,
    pub profile: Option<String>,
    /// A ported app: its aio home and aio app id.
    pub port_of: Option<(PathBuf, String)>,
    /// Who chose: `--home`, `--profile`, `AIORT_PROFILE`, `AIO_PROFILE`,
    /// `--instance`, `AIORT_DATA`, `AIO_APPS_DIR`, a ported app's `[home]
    /// env`, `[home] portable` or `[home] dev`, or `default`.
    pub from: &'static str,
    /// Its home was asked for: checked for another program's files first.
    pub asked: bool,
}

/// Where app `id` (ported from aio app `aio`, if so) runs for launch `ask`
/// (pure: `env` reads a variable). A ported app's home is aio's, exactly:
/// `--home` > `--profile` > its `[home]` rules (`env`, then `portable`
/// outside a debug build, then `dev` in one without a profile) >
/// `AIO_APPS_DIR/<app>` > `~/.<app>`, and `--instance=<n>` is
/// `~/.aio-instances/<n>/<app>`; `data`, `logs` and `cache` are in it. Any other app's is aio-rt's: `apps/<id>` (cache apart)
/// or `AIORT_DATA`, a profile beside it (`<home>-<name>`), and
/// `--instance=<n>` is `instances/<n>/<id>`.
pub fn plan(id: &str, port_of: Option<(&str, &crate::config::Home)>, ask: &Ask, env: Env) -> Result<Plan, String> {
    id_ok(id).map_err(|e| e.to_string())?;
    if let Some(n) = &ask.instance
        && (n.is_empty() || n.contains(['/', '\\']) || n == "." || n == "..")
    {
        return Err(format!("--instance needs a simple name: --instance=agent1 (got {n:?})"));
    }
    let text = |k: &str| env(k).and_then(|v| v.into_os_string().into_string().ok());
    let (profile, by) = match (&ask.profile, text("AIORT_PROFILE"), port_of.and_then(|_| text("AIO_PROFILE"))) {
        (Some(p), _, _) => (Some(p.clone()), "--profile"),
        (None, Some(p), _) => (Some(p), "AIORT_PROFILE"),
        (None, None, Some(p)) => (Some(p), "AIO_PROFILE"),
        _ => (None, ""),
    };
    let user = home_in(env);
    let (base, from) = match (port_of, &ask.instance) {
        // A sandbox (tests, `ax build --smoke`) wins over a ported app's own
        // rules too: it must never open a person's real aio data.
        (Some(_), None) if env("AIORT_DATA").is_some() => (env("AIORT_DATA").unwrap_or_default(), "AIORT_DATA"),
        (Some((app, _)), Some(n)) => (user.join(".aio-instances").join(n).join(app), "--instance"),
        (Some((app, rules)), None) => {
            let named = rules.env.as_deref().and_then(text).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
            // A debug build's binary is in the build folder: never a home.
            let portable = rules
                .portable
                .as_deref()
                .filter(|_| !ask.dev)
                .and_then(|p| Some(std::env::current_exe().ok()?.parent()?.join(p)))
                .filter(|p| p.is_dir());
            match (named, portable, rules.dev.as_deref().filter(|_| ask.dev && profile.is_none())) {
                (Some(v), _, _) => (crate::compat::data::expand(&v, &user), "[home] env"),
                (None, Some(p), _) => (p, "[home] portable"),
                (None, None, Some(d)) => (user.join(d), "[home] dev"),
                (None, None, None) => match text("AIO_APPS_DIR") {
                    Some(r) => (crate::compat::data::expand(&r, &user).join(app), "AIO_APPS_DIR"),
                    None => (user.join(format!(".{app}")), "default"),
                },
            }
        }
        (None, Some(n)) => (tool_home_in(env).join("instances").join(n).join(id), "--instance"),
        (None, None) => match env("AIORT_DATA") {
            Some(root) => (root, "AIORT_DATA"),
            None => (tool_home_in(env).join("apps").join(id), "default"),
        },
    };
    let (home, name) = crate::compat::data::pick(&base, ask.home.as_deref(), profile.as_deref(), &user)?;
    let asked = ask.home.is_some() || profile.is_some();
    let from = match () {
        _ if ask.home.is_some() => "--home",
        _ if profile.is_some() => by,
        _ => from,
    };
    let cache = match (port_of, from) {
        (None, "default") => cache_home_in(env).join(id),
        _ => home.join("cache"),
    };
    let port_of = port_of.map(|(app, _)| (home.clone(), app.to_string()));
    let data = home.join("data");
    Ok(Plan { store: data.join("state.db"), data, logs: home.join("logs"), cache, profile: name, port_of, from, asked })
}

/// Is an instance running on the app data dir `data`? It holds the
/// instance lock, or aio's home claim beside it (on every OS, and no reused
/// pid can fool it).
pub fn running(data: &std::path::Path) -> bool {
    let held = |p: PathBuf| {
        std::fs::OpenOptions::new()
            .write(true)
            .open(p)
            .is_ok_and(|f| matches!(f.try_lock(), Err(std::fs::TryLockError::WouldBlock)))
    };
    held(data.join("instance.lock")) || held(parent(data).join(".aio-instance.lock"))
}

/// The dirs of app `id`, created and locked. `AIORT_DATA` puts all three under
/// one dir (tests, portable installs). A second instance gets `WouldBlock`.
/// The OS drops the lock when the process ends, so a crash leaves none stale.
pub fn app(id: &str) -> io::Result<AppDirs> {
    let (data, logs, cache) = locate(id)?;
    let from = if env("AIORT_DATA").is_some() { "AIORT_DATA" } else { "default" };
    AppDirs::open(id, data, logs, cache, from)
}

/// Where app `cfg` runs for launch `ask` ([`plan`] in this environment; a
/// debug build asks `dev`), its store `[data] store`; [`AppDirs::planned`]
/// makes and locks them.
pub fn plan_app(cfg: &crate::config::Config, ask: &Ask) -> io::Result<Plan> {
    let ask = Ask { dev: ask.dev || cfg!(debug_assertions), ..ask.clone() };
    let p = plan(&cfg.app.id, cfg.app.port_of.as_deref().map(|a| (a, &cfg.home)), &ask, &env)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    Ok(Plan { store: p.data.join(&cfg.data.store), ..p })
}

impl AppDirs {
    /// `<data>/files`: what the app keeps for its users (uploads, blobs),
    /// backed up with its data; made when used.
    pub fn files(&self) -> PathBuf {
        self.data.join("files")
    }

    /// App `id`'s dirs under `root` (`data`, `logs`, `cache`), locked, as
    /// `AIORT_DATA=<root>` gives.
    pub fn at(root: &std::path::Path, id: &str) -> io::Result<Self> {
        id_ok(id)?;
        Self::open(id, root.join("data"), root.join("logs"), root.join("cache"), "AppDirs::at")
    }

    /// App `id`'s dirs as `p` has them, created and locked. A home that is
    /// a file, another program's folder or a reserved one is refused first,
    /// with nothing made (`compat::data::refusal`).
    pub fn planned(id: &str, p: Plan) -> io::Result<Self> {
        let user = home();
        let refused = |why: String| io::Error::new(io::ErrorKind::InvalidInput, why);
        match p.port_of {
            Some((home, app)) => {
                guard(&home)?;
                if let Some(why) = crate::compat::data::refusal(&app, &home, &user) {
                    return Err(refused(why));
                }
                let claim = crate::compat::data::Home::open(&home, &app, p.profile.as_deref())?.claim;
                private_dir(&p.logs)?;
                std::fs::create_dir_all(&p.cache)?;
                let (data, logs, cache, store, profile, from) = (p.data, p.logs, p.cache, p.store, p.profile, p.from);
                Ok(AppDirs { id: id.into(), data, logs, cache, store, profile, port_of: Some(app), from, _lock: claim })
            }
            None => {
                if p.asked
                    && let Some(why) = crate::compat::data::refusal(id, parent(&p.data), &user)
                {
                    return Err(refused(why));
                }
                Ok(AppDirs { profile: p.profile, store: p.store, ..Self::open(id, p.data, p.logs, p.cache, p.from)? })
            }
        }
    }

    fn open(id: &str, data: PathBuf, logs: PathBuf, cache: PathBuf, from: &'static str) -> io::Result<Self> {
        guard(&data)?;
        private_dir(&data)?;
        private_dir(&logs)?;
        std::fs::create_dir_all(&cache)?;
        let lock =
            std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(data.join("instance.lock"))?;
        lock.try_lock().map_err(|e| match e {
            std::fs::TryLockError::WouldBlock => io::Error::new(
                io::ErrorKind::WouldBlock,
                format!("{id} is already running (lock in {})", data.display()),
            ),
            std::fs::TryLockError::Error(e) => e,
        })?;
        let _lock = std::sync::Arc::new(Held(lock));
        Ok(AppDirs {
            id: id.into(),
            store: data.join("state.db"),
            data,
            logs,
            cache,
            profile: None,
            port_of: None,
            from,
            _lock,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_electron_store_is_aios_on_each_os() {
        // aio shared-runtime.ts `sharedElectronRoot`.
        let p = PathBuf::from;
        let with = |kv: &'static [(&str, &str)]| {
            move |k: &str| kv.iter().find(|(n, _)| *n == k).map(|(_, v)| PathBuf::from(v))
        };
        let home = with(&[("HOME", "/u")]);
        assert_eq!(electron_store_in("linux", &home), p("/u/.local/share/aio/electron"));
        assert_eq!(electron_store_in("linux", &with(&[("HOME", "/u"), ("XDG_DATA_HOME", "/x/")])), p("/x/aio/electron"));
        assert_eq!(electron_store_in("macos", &home), p("/u/Library/Application Support/aio/electron"));
        assert_eq!(electron_store_in("windows", &with(&[("USERPROFILE", "/w")])), p("/w/AppData/Local/aio/electron"));
        assert_eq!(electron_store_in("windows", &with(&[("LOCALAPPDATA", "/l"), ("HOME", "/u")])), p("/l/aio/electron"));
        // The override wins on every OS, made absolute.
        for os in ["linux", "macos", "windows"] {
            let moved = electron_store_in(os, &with(&[("HOME", "/u"), ("AIO_SHARED_ELECTRON_DIR", "rel/store")]));
            assert!(moved.is_absolute() && moved.ends_with("rel/store"), "{}", moved.display());
        }
    }

    #[test]
    fn a_base_too_long_for_sockets_falls_back_to_a_short_private_dir() {
        let p = std::path::Path::new;
        let short = p("/run/user/1000");
        assert_eq!(socket_dir(short, "notes", 0xab, Some(1000)), p("/run/user/1000/aio-rt/notes-000000ab"));
        // An agent's scratch folder as XDG_RUNTIME_DIR or TMPDIR (aio's
        // `resolveSocketPath` fallback): every name the app binds fits.
        let long = p("/tmp/claude-1000/a-project-folder-of-some-length/0123456789abcdef/scratchpad");
        let dir = socket_dir(long, "notes", 0xab, Some(1000));
        assert_eq!(dir, p("/tmp/aio-rt-1000/notes-000000ab"));
        // A long id is cut to 40 bytes there, on a char boundary.
        let id = format!("{}é{}", "a".repeat(39), "b".repeat(30));
        let cut = socket_dir(long, &id, 0xab, Some(1000));
        assert_eq!(cut, PathBuf::from(format!("/tmp/aio-rt-1000/{}-000000ab", "a".repeat(39))));
        for d in [&dir, &cut] {
            let longest = d.join("cell-0123456789abcd.sock");
            assert!(longest.as_os_str().len() <= 100, "{}", longest.display());
            for limit in [108, 104] {
                assert_eq!(crate::ipc::too_long(&longest.to_string_lossy(), limit), None);
            }
        }
        // The edge: room for exactly a 24-byte name stays.
        let edge = p("/x").join("y".repeat(50)); // + "/aio-rt/notes-000000ab": 75 bytes
        assert_eq!(socket_dir(&edge, "notes", 0xab, Some(1)).as_os_str().len(), 75);
        assert!(socket_dir(&edge.join("z"), "notes", 0xab, Some(1)).starts_with("/tmp/aio-rt-1"));
        // Windows pipes take any length.
        assert!(socket_dir(long, "notes", 0xab, None).starts_with(long));
    }

    #[test]
    #[cfg(windows)]
    fn one_folder_spelled_with_either_separator_has_one_socket_dir() {
        let a = sockets_at("mail", std::path::Path::new(r"C:\Users\x\apps/mail/data"));
        assert_eq!(a, sockets_at("mail", std::path::Path::new(r"C:\Users\x\apps\mail\data")));
    }

    #[test]
    fn a_held_target_is_retried_on_windows_then_fails_fast_for_a_while() {
        use std::cell::{Cell, RefCell};
        let busy = || io::Error::from_raw_os_error(32);
        let paused = RefCell::new(vec![]);
        let pause = |ms| paused.borrow_mut().push(ms);
        // Let go after three tries: replaced, having waited 5 + 10 + 20 ms.
        let n = Cell::new(0);
        let target = std::path::Path::new("held/a.json");
        let r = retried(|| if n.replace(n.get() + 1) < 3 { Err(busy()) } else { Ok(()) }, true, target, &pause);
        assert!(r.is_ok() && *paused.borrow() == [5, 10, 20]);
        // Held for good: every pause spent, then an error that says so.
        paused.borrow_mut().clear();
        let target = std::path::Path::new("held/b.json");
        let e = retried(|| Err(busy()), true, target, &pause).unwrap_err();
        assert_eq!(*paused.borrow(), BACKOFF_MS);
        assert!(e.to_string().starts_with("could not replace held/b.json after 11 tries over 1315 ms:"), "{e}");
        assert!(e.to_string().ends_with("it is open in another program, or this user may not write it"));
        // In its cool-down: one try, no wait; once it works it is forgotten.
        paused.borrow_mut().clear();
        let e = retried(|| Err(busy()), true, target, &pause).unwrap_err();
        assert!(paused.borrow().is_empty() && e.to_string().contains("after 1 try over 0 ms"), "{e}");
        assert!(retried(|| Ok(()), true, target, &pause).is_ok());
        n.set(0);
        assert!(
            retried(|| if n.replace(n.get() + 1) < 1 { Err(busy()) } else { Ok(()) }, true, target, &pause).is_ok()
        );
        assert_eq!(*paused.borrow(), [5], "waits again");
        // Another error, or off Windows: no retry at all.
        paused.borrow_mut().clear();
        let gone = || Err(io::Error::from(io::ErrorKind::NotFound));
        assert_eq!(
            retried(gone, true, std::path::Path::new("held/c"), &pause).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert!(
            retried(|| Err(busy()), false, std::path::Path::new("held/d"), &pause).is_err()
                && paused.borrow().is_empty()
        );
        assert!(
            held_open(&io::Error::from(io::ErrorKind::PermissionDenied))
                && held_open(&io::Error::from_raw_os_error(33))
        );
    }

    #[test]
    fn a_launch_asks_in_aios_flags() {
        let a = Ask::parse(&args(&[
            "a.md",
            "--profile=dev",
            "--home",
            "/h",
            "--takeover",
            "--instance=agent1",
            "--port",
            "9000",
            "--host=0.0.0.0",
            "--expose",
            "--keep-server",
            "--no-persist",
            "--tls-cert=c.pem",
            "--tls-key",
            "k.pem",
        ]))
        .unwrap();
        let want = Ask {
            home: Some("/h".into()),
            profile: Some("dev".into()),
            instance: Some("agent1".into()),
            takeover: true,
            port: Some("9000".into()),
            host: Some("0.0.0.0".into()),
            expose: true,
            keep_server: true,
            dev: false,
            no_persist: true,
            no_tls: false,
            tls_cert: Some("c.pem".into()),
            tls_key: Some("k.pem".into()),
        };
        assert_eq!(a, want);
        assert!(Ask::parse(&args(&["--no-tls"])).unwrap().no_tls);
        assert_eq!(Ask::parse(&args(&["--verbose", "x"])).unwrap(), Ask::default());
        assert!(Ask::parse(&args(&["--home"])).unwrap_err().contains("needs a value"));
        assert!(Ask::parse(&args(&["--profile=a", "--profile=b"])).unwrap_err().contains("twice"));
        // The app's parser gets the rest, in order, values and all.
        let launch = args(&[
            "a.md",
            "--profile=dev",
            "--home",
            "/h",
            "--takeover",
            "-v",
            "--instance=x",
            "--homes=1",
            "--port",
            "1",
            "--expose",
            "--keep-server",
        ]);
        assert_eq!(Ask::app_args(&launch), ["a.md", "-v", "--homes=1"]);
        assert_eq!(Ask::app_args(&args(&["--home"])), Vec::<String>::new(), "a missing value takes nothing else");
    }

    #[cfg(unix)]
    type Got = (String, String, Option<String>, &'static str);

    /// `plan` as `(data, cache, profile, from)`, with only `vars` set.
    #[cfg(unix)]
    fn at(port_of: Option<&str>, a: &[&str], vars: &[(&str, &str)]) -> Result<Got, String> {
        ruled(port_of, &crate::config::Home::default(), false, a, vars)
    }

    /// [`at`] with a ported app's `[home]` rules, in a debug build or not.
    #[cfg(unix)]
    fn ruled(
        port_of: Option<&str>,
        rules: &crate::config::Home,
        dev: bool,
        a: &[&str],
        vars: &[(&str, &str)],
    ) -> Result<Got, String> {
        let mut vars = vars.to_vec();
        vars.extend([("HOME", "/u"), ("XDG_DATA_HOME", "/u/share"), ("XDG_CACHE_HOME", "/u/cache")]);
        let env = |k: &str| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| PathBuf::from(v));
        let p = plan("notes", port_of.map(|a| (a, rules)), &Ask { dev, ..Ask::parse(&args(a))? }, &env)?;
        assert_eq!(p.port_of.as_ref().map(|(h, _)| h.join("data")), port_of.map(|_| p.data.clone()));
        assert_eq!(p.store, p.data.join("state.db"));
        Ok((p.data.display().to_string(), p.cache.display().to_string(), p.profile, p.from))
    }

    #[test]
    #[cfg(unix)]
    fn a_ported_apps_own_home_rules_resolve_as_its_aio_code_did() {
        let rules = crate::config::Home {
            env: Some("PURSE_HOME".into()),
            portable: Some(format!("portable-{}", std::process::id())),
            dev: Some(".purse-dev".into()),
        };
        let at = |dev, a: &[&str], vars: &[(&str, &str)]| ruled(Some("purse"), &rules, dev, a, vars).unwrap();
        let home = |g: Got| (g.0, g.2, g.3);
        let h = |d: &str, p: Option<&str>, f| (d.to_string(), p.map(String::from), f);
        assert_eq!(home(at(false, &[], &[])), h("/u/.purse/data", None, "default"));
        assert_eq!(home(at(true, &[], &[])), h("/u/.purse-dev/data", None, "[home] dev"));
        // A profile counts from the plain home, in a debug build too.
        assert_eq!(home(at(true, &["--profile=qa"], &[])), h("/u/.purse-qa/data", Some("qa"), "--profile"));
        // The variable names the home (`~` expanded, blank ignored), over
        // AIO_APPS_DIR and the dev home; a profile counts from it.
        let named = [("PURSE_HOME", " ~/w "), ("AIO_APPS_DIR", "/srv")];
        assert_eq!(home(at(true, &[], &named)), h("/u/w/data", None, "[home] env"));
        assert_eq!(home(at(false, &["--profile=qa"], &named)), h("/u/w-qa/data", Some("qa"), "--profile"));
        assert_eq!(home(at(false, &[], &[("PURSE_HOME", "  ")])), h("/u/.purse/data", None, "default"));
        assert_eq!(home(at(false, &["--home=/h"], &named)).0, "/h/data", "--home wins");
        assert_eq!(home(at(true, &["--instance=a"], &named)).0, "/u/.aio-instances/a/purse/data");
        // A sandbox wins over every rule of the app's own.
        let boxed = [("AIORT_DATA", "/sandbox"), ("PURSE_HOME", "/u/w")];
        assert_eq!(home(at(false, &[], &boxed)), h("/sandbox/data", None, "AIORT_DATA"));
        // A portable folder beside the binary is the home, unless in a
        // debug build or named by the variable.
        let beside = std::env::current_exe().unwrap().parent().unwrap().join(rules.portable.as_ref().unwrap());
        std::fs::create_dir(&beside).unwrap();
        let got = (home(at(false, &[], &[])), home(at(true, &[], &[])), home(at(false, &[], &named)).2);
        std::fs::remove_dir(&beside).unwrap();
        assert_eq!(got.0, (beside.join("data").display().to_string(), None, "[home] portable"));
        assert_eq!((got.1.2, got.2), ("[home] dev", "[home] env"));
    }

    #[test]
    #[cfg(unix)]
    fn homes_and_profiles_resolve() {
        let s = |d: &str, c: &str, p: Option<&str>, f| Ok((d.to_string(), c.to_string(), p.map(String::from), f));
        // aio-rt's own (macOS: in Library): the cache apart only in the default layout.
        let (share, cache) = match cfg!(target_os = "macos") {
            true => ("/u/Library/Application Support/aio-rt", "/u/Library/Caches/aio-rt"),
            false => ("/u/share/aio-rt", "/u/cache/aio-rt"),
        };
        let data = format!("{share}/apps/notes/data");
        assert_eq!(at(None, &[], &[]), s(&data, &format!("{cache}/notes"), None, "default"));
        assert_eq!(at(None, &[], &[("AIORT_DATA", "/r")]), s("/r/data", "/r/cache", None, "AIORT_DATA"));
        let dev = format!("{share}/apps/notes-dev");
        let dev = s(&format!("{dev}/data"), &format!("{dev}/cache"), Some("dev"), "--profile");
        assert_eq!(at(None, &["--profile=dev"], &[]), dev);
        assert_eq!(at(None, &[], &[("AIORT_PROFILE", "qa")]).unwrap().3, "AIORT_PROFILE");
        assert_eq!(at(None, &["--profile", "dev"], &[("AIORT_PROFILE", "qa")]), dev, "the flag wins");
        assert_eq!(at(None, &[], &[("AIO_PROFILE", "qa")]).unwrap().2, None, "AIO_PROFILE is aio's");
        assert_eq!(at(None, &["--home=/h"], &[]), s("/h/data", "/h/cache", None, "--home"));
        let inst = at(None, &["--instance=a1", "--profile=dev"], &[]).unwrap().0;
        assert_eq!(inst, format!("{share}/instances/a1/notes-dev/data"));
        // A ported app: aio's homes, exactly.
        let port_of = Some("notes-v1");
        assert_eq!(at(port_of, &[], &[]), s("/u/.notes-v1/data", "/u/.notes-v1/cache", None, "default"));
        assert_eq!(at(port_of, &[], &[("AIO_APPS_DIR", "/srv")]).unwrap().0, "/srv/notes-v1/data");
        let dev = s("/u/.notes-v1-dev/data", "/u/.notes-v1-dev/cache", Some("dev"), "AIO_PROFILE");
        assert_eq!(at(port_of, &[], &[("AIO_PROFILE", "dev")]), dev);
        assert_eq!(at(port_of, &["--home=~/.notes-v1-qa"], &[]).unwrap().2, Some("qa".into()), "a profile's folder is it");
        let inst = at(port_of, &["--instance=a1"], &[("AIO_APPS_DIR", "/srv")]).unwrap();
        assert_eq!((inst.0.as_str(), inst.3), ("/u/.aio-instances/a1/notes-v1/data", "--instance"));
        for bad in ["--instance=..", "--instance=a/b", "--instance="] {
            assert!(at(port_of, &[bad], &[]).unwrap_err().contains("simple name"), "{bad}");
        }
        assert!(at(port_of, &["--home=/x", "--profile=dev"], &[]).unwrap_err().contains("two different folders"));
        assert!(at(None, &["--profile=Dev"], &[]).is_err());
    }

    #[test]
    fn profiles_run_beside_each_other_and_a_home_once() {
        let d = tempfile::tempdir().unwrap();
        let env = |k: &str| (k == "AIORT_DATA").then(|| d.path().join("notes"));
        let open =
            |a: &[&str]| AppDirs::planned("notes", plan("notes", None, &Ask::parse(&args(a)).unwrap(), &env).unwrap());
        let main = open(&[]).unwrap();
        let dev = open(&["--profile=dev"]).unwrap();
        assert_eq!((dev.data.clone(), dev.profile.as_deref()), (d.path().join("notes-dev").join("data"), Some("dev")));
        assert_eq!(open(&["--profile=dev"]).unwrap_err().kind(), io::ErrorKind::WouldBlock);
        assert_eq!(open(&[]).unwrap_err().kind(), io::ErrorKind::WouldBlock);
        assert!(running(&main.data) && running(&dev.data));
        assert_eq!(main.files(), main.data.join("files"));
        // A named instance is a world of its own: beside the user's copy, once.
        let user = |k: &str| matches!(k, "HOME" | "XDG_DATA_HOME" | "LOCALAPPDATA").then(|| d.path().join(k));
        let inst =
            |n: &str| AppDirs::planned("notes", plan("notes", None, &Ask::parse(&args(&[n])).unwrap(), &user).unwrap());
        let a1 = inst("--instance=a1").unwrap();
        assert_eq!(a1.data, tool_home_in(&user).join("instances/a1/notes/data"));
        assert_eq!(inst("--instance=a1").unwrap_err().kind(), io::ErrorKind::WouldBlock);
        let a2 = inst("--instance=a2").unwrap();
        assert!(running(&a1.data) && running(&a2.data) && running(&main.data));
        drop((a1, a2));
        let data = main.data.clone();
        drop((main, dev));
        assert!(!running(&data));
        // An asked home among another program's files is refused, untouched.
        let theirs = d.path().join("theirs");
        std::fs::create_dir(&theirs).unwrap();
        std::fs::write(theirs.join("id_rsa"), b"").unwrap();
        let e = open(&[&format!("--home={}", theirs.display())]).unwrap_err();
        assert!(e.to_string().contains("another program's files (id_rsa)"), "{e}");
        assert!(!theirs.join("data").exists());
    }
}

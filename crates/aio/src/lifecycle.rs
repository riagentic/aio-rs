//! A child that must not outlive its parent: `ax dev`'s app, a test's
//! app. The parent keeps one end of a pipe ([`Watch`]); the child gets the
//! other in `AIORT_WATCH_FD` (Windows: a private named pipe's name). When
//! the parent goes, however it goes (even `kill -9`), the pipe closes and the
//! child stops itself the graceful way: as if asked to ([`stop_asked`]).
use std::io;
use std::process::{Child, Command};
use std::time::Duration;

const ENV: &str = "AIORT_WATCH_FD";

static STOP: tokio::sync::Notify = tokio::sync::Notify::const_new();
static LISTENING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static RESTART: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The exit of a stop that stays down under the supervisor files `aio
/// build` writes (aio's, SIGTERM's): their restart loop ends on it.
pub const STAYS_DOWN: u8 = 143;

/// Ask this process to stop so it is started again (an update installed
/// for the supervisor to start): its exit is a clean 0 whoever supervises.
pub fn ask_restart() {
    RESTART.store(true, std::sync::atomic::Ordering::SeqCst);
    ask_stop();
}

/// The exit a stop of this process ends with (a server's `main` returns
/// it): [`STAYS_DOWN`] where its supervisor asked a stop (`ax stop`,
/// SIGTERM) to stay down, `AIO_SUPERVISED=1` with `AIO_STOP_STAYS_DOWN=1`,
/// unless it was for a restart ([`ask_restart`]); else 0.
pub fn exit_code() -> std::process::ExitCode {
    let on = |k: &str| std::env::var_os(k).is_some_and(|v| v == "1");
    let restart = RESTART.load(std::sync::atomic::Ordering::SeqCst);
    std::process::ExitCode::from(stop_code(on("AIO_SUPERVISED") && on("AIO_STOP_STAYS_DOWN"), restart))
}

/// [`exit_code`]'s number. Pure.
pub fn stop_code(stays_down: bool, restart: bool) -> u8 {
    if stays_down && !restart { STAYS_DOWN } else { 0 }
}

/// Ask this process to stop the graceful way, as SIGTERM does: a process
/// waiting in [`stop_asked`] shuts down; one that isn't ends now.
pub fn ask_stop() {
    if LISTENING.load(std::sync::atomic::Ordering::SeqCst) {
        STOP.notify_one();
    } else {
        #[cfg(unix)]
        // SAFETY: signalling this process.
        unsafe {
            libc::kill(libc::getpid(), libc::SIGTERM);
        }
        #[cfg(not(unix))]
        std::process::exit(1);
    }
}

/// Resolves when this process is asked to stop: SIGTERM or SIGINT (Unix),
/// Ctrl+C, a closed console, logoff or shutdown (Windows), or [`ask_stop`].
/// From this call on (inside a tokio runtime), those no longer end the
/// process at once.
pub fn stop_asked() -> io::Result<impl Future<Output = ()>> {
    #[cfg(unix)]
    let wait = {
        use tokio::signal::unix::{SignalKind, signal};
        let (mut term, mut int) = (signal(SignalKind::terminate())?, signal(SignalKind::interrupt())?);
        async move {
            tokio::select! { _ = term.recv() => {}, _ = int.recv() => {}, _ = STOP.notified() => {} }
        }
    };
    #[cfg(windows)]
    let wait = {
        use tokio::signal::windows::{ctrl_break, ctrl_c, ctrl_close, ctrl_logoff, ctrl_shutdown};
        let (mut c, mut b, mut cl) = (ctrl_c()?, ctrl_break()?, ctrl_close()?);
        let (mut lo, mut sh) = (ctrl_logoff()?, ctrl_shutdown()?);
        async move {
            tokio::select! {
                _ = c.recv() => {}, _ = b.recv() => {}, _ = cl.recv() => {},
                _ = lo.recv() => {}, _ = sh.recv() => {}, _ = STOP.notified() => {}
            }
        }
    };
    LISTENING.store(true, std::sync::atomic::Ordering::SeqCst);
    Ok(wait)
}

/// The parent's end of the watch: while it lives, the child runs. (It also
/// keeps this process's copy of the child's end, closed with it.)
#[cfg(unix)]
pub struct Watch {
    _write: std::os::fd::OwnedFd,
    _read: std::os::fd::OwnedFd,
}

/// Make `cmd` start a child that stops when this process (or the returned
/// [`Watch`]) goes away.
#[cfg(unix)]
pub fn watched(cmd: &mut Command) -> io::Result<Watch> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::process::CommandExt;
    let mut fds = [0; 2];
    // SAFETY: a plain pipe; both ends are closed on exec unless cleared below.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let made = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } == 0;
    // No pipe2 (macOS): the flag right after (a fork in between may inherit it).
    // SAFETY: as above, on the fds just made.
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let made = unsafe {
        libc::pipe(fds.as_mut_ptr()) == 0 && fds.iter().all(|&fd| libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) == 0)
    };
    if !made {
        return Err(io::Error::last_os_error());
    }
    let (read, write) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    let fd = read.as_raw_fd();
    cmd.env(ENV, fd.to_string());
    // SAFETY: only fcntl between fork and exec (async-signal-safe): the child
    // keeps the read end; the write end stays close-on-exec, so only this
    // process holds it.
    unsafe {
        cmd.pre_exec(move || {
            if libc::fcntl(fd, libc::F_SETFD, 0) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    // `read` must stay open until the child is spawned: keep it with the watch.
    Ok(Watch { _write: write, _read: read })
}

/// The parent's end of the watch (Windows): a one-instance named pipe only
/// this user can open; closing it breaks the child's end.
#[cfg(windows)]
pub struct Watch {
    _pipe: std::os::windows::io::OwnedHandle,
}

#[cfg(windows)]
pub fn watched(cmd: &mut Command) -> io::Result<Watch> {
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_OUTBOUND};
    use windows_sys::Win32::System::Pipes::{CreateNamedPipeW, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE};
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let name = format!(r"\\.\pipe\aio-rt-watch-{}-{n}", std::process::id());
    let sd = crate::ipc::win::OnlyMe::new()?;
    let sa = sd.attributes();
    let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
    // SAFETY: a NUL-terminated name; `sa` outlives the call.
    let h = unsafe {
        CreateNamedPipeW(
            wide.as_ptr(),
            PIPE_ACCESS_OUTBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            0,
            0,
            0,
            &sa,
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    cmd.env(ENV, name);
    // SAFETY: a fresh handle, owned from here on.
    Ok(Watch { _pipe: unsafe { OwnedHandle::from_raw_handle(h) } })
}

/// In the child: stop gracefully when the parent goes (no-op without one).
pub fn watch_parent() {
    #[cfg(windows)]
    if let Some(name) = std::env::var(ENV).ok().filter(|v| v.starts_with(r"\\.\pipe\")) {
        // Remove it, so this process's own children don't watch our parent.
        // SAFETY: before any other thread of this process reads the env.
        unsafe { std::env::remove_var(ENV) };
        _ = std::thread::Builder::new().name("aio-parent".into()).spawn(move || {
            use std::io::Read;
            if let Ok(mut f) = std::fs::File::open(&name) {
                let mut b = [0u8; 1];
                // Blocks until the parent's end closes (broken pipe) or fails.
                while f.read(&mut b).is_ok_and(|n| n > 0) {}
            }
            eprintln!("[aio] the parent process is gone; stopping");
            ask_stop();
        });
    }
    #[cfg(unix)]
    if let Some(fd) = std::env::var(ENV).ok().and_then(|v| v.parse::<i32>().ok()) {
        // SAFETY: the fd was handed to this process for this purpose only;
        // close-on-exec again, so this process's own children don't get it.
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
        _ = std::thread::Builder::new().name("aio-parent".into()).spawn(move || {
            let mut b = [0u8; 1];
            // Blocks until the parent's end closes (EOF) or fails.
            while unsafe { libc::read(fd, b.as_mut_ptr().cast(), 1) } > 0 {}
            eprintln!("[aio] the parent process is gone; stopping");
            ask_stop();
        });
    }
}

/// Ask `child` to stop (SIGTERM), give it `budget`, then kill it. Windows
/// has no SIGTERM: a watched child stops itself once its [`Watch`] is
/// dropped (drop it first); `budget` is then its time to do so.
pub fn stop(child: &mut Child, budget: Duration) {
    #[cfg(unix)]
    if let Ok(pid) = i32::try_from(child.id()) {
        // SAFETY: our own child, by pid, while not yet reaped.
        unsafe { libc::kill(pid, libc::SIGTERM) };
    }
    let end = std::time::Instant::now() + budget;
    while std::time::Instant::now() < end {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    _ = child.kill();
    _ = child.wait();
}

/// End process `pid`: `hard`, at once (SIGKILL, Windows `TerminateProcess`);
/// else asked (SIGTERM; Windows has none: `Unsupported`).
pub fn terminate(pid: u32, hard: bool) -> io::Result<()> {
    #[cfg(unix)]
    {
        let pid = i32::try_from(pid).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        // SAFETY: a plain signal to a pid.
        if unsafe { libc::kill(pid, if hard { libc::SIGKILL } else { libc::SIGTERM }) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
        if !hard {
            return Err(io::Error::new(io::ErrorKind::Unsupported, "Windows has no SIGTERM"));
        }
        // SAFETY: a handle opened here, used once, closed.
        unsafe {
            let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if h.is_null() {
                return Err(io::Error::last_os_error());
            }
            let ok = TerminateProcess(h, 1) != 0;
            let e = io::Error::last_os_error();
            CloseHandle(h);
            if ok { Ok(()) } else { Err(e) }
        }
    }
}

/// Wait until process `pid` is gone, at most `budget` (an updater waiting
/// for the app it replaces). A pid that can't be a process is gone.
pub fn wait_gone(pid: u32, budget: Duration) {
    let end = std::time::Instant::now() + budget;
    #[cfg(unix)]
    {
        let Ok(pid) = i32::try_from(pid) else { return };
        // SAFETY: signal 0 only asks whether the process exists.
        while unsafe { libc::kill(pid, 0) } == 0 && std::time::Instant::now() < end {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
        // SAFETY: a handle opened here, waited on, closed.
        unsafe {
            let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
            if h.is_null() {
                return;
            }
            let ms = end.saturating_duration_since(std::time::Instant::now()).as_millis();
            WaitForSingleObject(h, u32::try_from(ms).unwrap_or(u32::MAX));
            CloseHandle(h);
        }
    }
}

/// Is there a process `pid` (one of another user's counts: it exists)?
pub fn alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let Ok(pid) = i32::try_from(pid) else { return false };
        // SAFETY: signal 0 only asks whether the process exists.
        let sent = unsafe { libc::kill(pid, 0) == 0 };
        sent || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, GetLastError, STILL_ACTIVE};
        use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        // SAFETY: a handle opened here, read, closed.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return GetLastError() == ERROR_ACCESS_DENIED;
            }
            let mut code = 0u32;
            let ok = GetExitCodeProcess(h, &mut code) != 0;
            CloseHandle(h);
            !ok || code == STILL_ACTIVE as u32
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        _ = pid;
        true
    }
}

/// When process `pid` started, as the OS says it (a stamp only compared
/// with itself), or `None` when it is gone or can't be read. A pid is a
/// number the OS hands out again; pid + start is one process.
pub fn started(pid: u32) -> Option<String> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        // Field 22 of /proc/<pid>/stat; `comm` (field 2) may hold spaces: cut at its last `)`.
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        stat.get(stat.rfind(')')? + 2..)?.split(' ').nth(19).map(String::from)
    }
    #[cfg(target_os = "macos")]
    {
        // SAFETY: a plain C struct of integers and byte arrays: all zeroes is valid.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = i32::try_from(size_of::<libc::proc_bsdinfo>()).ok()?;
        // SAFETY: `info` is a buffer of `size` bytes the call fills.
        let n = unsafe {
            libc::proc_pidinfo(i32::try_from(pid).ok()?, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size)
        };
        (n == size).then(|| format!("{}.{:06}", info.pbi_start_tvsec, info.pbi_start_tvusec))
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
        use windows_sys::Win32::System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        let z = || FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut made, mut a, mut b, mut c) = (z(), z(), z(), z());
        // SAFETY: a handle opened here, read, closed.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return None;
            }
            let ok = GetProcessTimes(h, &mut made, &mut a, &mut b, &mut c);
            CloseHandle(h);
            (ok != 0).then(|| format!("{}", (u64::from(made.dwHighDateTime) << 32) | u64::from(made.dwLowDateTime)))
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos", windows)))]
    {
        _ = pid;
        None
    }
}

/// Make this process harder to read from outside: in release builds it
/// is not dumpable, so other processes of the same user (without
/// `CAP_SYS_PTRACE`) can't attach to it or read its memory, and no core
/// file holds its keys. Debug builds stay debuggable.
pub fn harden() {
    #[cfg(all(target_os = "linux", not(debug_assertions)))]
    // SAFETY: a plain prctl on this process.
    unsafe {
        libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0);
    }
}

/// Is the peer of a local socket this same user? A socket dir is
/// private already; this also holds if its mode is ever loosened.
#[cfg(unix)]
pub fn same_user(peer: std::io::Result<tokio::net::unix::UCred>) -> bool {
    // SAFETY: getuid has no preconditions.
    peer.is_ok_and(|c| c.uid() == unsafe { libc::getuid() })
}

/// Variables that turn a child into something else (aio's list): Electron as
/// plain Node, flags injected before the app's code. And an AppImage's own
/// variables: a child of an AppImage-run terminal inherits them, pointing into
/// a mount that is not ours and vanishes with its host.
const REFUSED: [&str; 7] = ["ELECTRON_RUN_AS_NODE", "NODE_OPTIONS", "NODE_PATH", "APPIMAGE", "APPDIR", "ARGV0", "OWD"];

/// Search paths whose entries under an inherited AppImage mount are dropped.
const PATHS: [&str; 3] = ["PATH", "LD_LIBRARY_PATH", "LD_PRELOAD"];

/// The changes that scrub `env` for a child: `None` removes a key,
/// `Some` sets it. `aio`: the child is an aio-rs app and keeps the `AIORT_*`
/// settings; any other child (Electron) gets none, the sync key included.
/// Partial by nature: whoever writes the environment can also set `PATH` or
/// `LD_PRELOAD` to their own files, which nothing can take away.
pub fn scrubbed(
    env: impl IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
    aio: bool,
) -> Vec<(String, Option<String>)> {
    let env: Vec<(String, String)> =
        env.into_iter().filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?))).collect();
    let mount = env.iter().find(|(k, _)| k == "APPDIR").map(|(_, v)| format!("{}/", v.trim_end_matches('/')));
    env.iter()
        .filter_map(|(k, v)| {
            if REFUSED.contains(&k.as_str()) || (!aio && k.starts_with("AIORT_")) {
                return Some((k.clone(), None));
            }
            let mount = mount.as_deref().filter(|_| PATHS.contains(&k.as_str()))?;
            let sep = if k == "LD_PRELOAD" { [':', ' '].as_slice() } else { [':'].as_slice() };
            let kept: Vec<&str> = v.split(sep).filter(|p| !p.is_empty() && !p.starts_with(mount)).collect();
            let kept = kept.join(":");
            (kept != *v).then(|| (k.clone(), (!kept.is_empty()).then_some(kept)))
        })
        .collect()
}

/// Scrub `cmd`'s inherited environment (see [`scrubbed`]).
pub fn scrub(cmd: &mut Command, aio: bool) {
    for (k, v) in scrubbed(std::env::vars_os(), aio) {
        match v {
            Some(v) => cmd.env(k, v),
            None => cmd.env_remove(k),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::{STAYS_DOWN, scrubbed, stop_code};

    #[test]
    fn a_stop_stays_down_where_the_supervisor_asks_but_a_restart_never_does() {
        assert_eq!(stop_code(true, false), STAYS_DOWN);
        assert_eq!((STAYS_DOWN, stop_code(true, true)), (143, 0), "an update exits for the new version");
        assert_eq!((stop_code(false, false), stop_code(false, true)), (0, 0), "unsupervised: a clean 0");
    }

    fn env(kv: &[(&str, &str)]) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
        kv.iter().map(|(k, v)| (k.into(), v.into())).collect()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_local_peer_must_be_this_user() {
        let (a, _b) = tokio::net::UnixStream::pair().unwrap();
        assert!(super::same_user(a.peer_cred()));
        assert!(!super::same_user(Err(std::io::Error::other("no peer"))));
    }

    #[test]
    fn hijackers_and_an_inherited_appimage_are_scrubbed() {
        let e = env(&[
            ("ELECTRON_RUN_AS_NODE", "1"),
            ("NODE_OPTIONS", "--require=/x.js"),
            ("APPDIR", "/tmp/.mount_host/"),
            ("PATH", "/tmp/.mount_host/usr/bin:/usr/bin"),
            ("LD_LIBRARY_PATH", "/tmp/.mount_host/usr/lib"),
            ("LD_PRELOAD", "/tmp/.mount_host/a.so /usr/lib/b.so"),
            ("AIORT_TOKEN", "secret"),
            ("HOME", "/home/u"),
        ]);
        let mut got = scrubbed(e.clone(), false);
        got.sort();
        let none = |k: &str| (k.to_string(), None);
        assert_eq!(
            got,
            [
                none("AIORT_TOKEN"),
                none("APPDIR"),
                none("ELECTRON_RUN_AS_NODE"),
                none("LD_LIBRARY_PATH"),
                ("LD_PRELOAD".into(), Some("/usr/lib/b.so".into())),
                none("NODE_OPTIONS"),
                ("PATH".into(), Some("/usr/bin".into())),
            ]
        );
        // An aio-rs app keeps its settings; without an AppImage, paths are left alone.
        assert!(!scrubbed(e, true).iter().any(|(k, _)| k == "AIORT_TOKEN"));
        assert!(scrubbed(env(&[("PATH", "/a:/b"), ("HOME", "/h")]), false).is_empty());
    }
}

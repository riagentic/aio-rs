//! A local door between this user's processes (the window runtime, a later
//! launch): a Unix socket in a private dir, or on Windows a named pipe only
//! this user can open (and no remote machine). Never a network port.
use std::io;
use std::path::Path;
use tokio::io::{AsyncRead, AsyncWrite};

/// `addr` as a Unix socket can take it, with what keeps that spelling
/// valid while it is used: the kernel holds 108 bytes of path, so a longer
/// one (a door in a long `TMPDIR`, an agent's scratch folder) is reached on
/// Linux through its folder's descriptor (`/proc/self/fd/<n>/<name>`).
#[cfg(unix)]
fn reach(addr: &str) -> io::Result<(String, Option<std::fs::File>)> {
    let p = Path::new(addr);
    match (p.parent(), p.file_name()) {
        (Some(dir), Some(name)) if cfg!(target_os = "linux") && addr.len() >= 100 => {
            use std::os::fd::AsRawFd;
            let dir = std::fs::File::open(dir)?;
            Ok((format!("/proc/self/fd/{}/{}", dir.as_raw_fd(), name.to_string_lossy()), Some(dir)))
        }
        _ => fits(addr).map(|()| (addr.into(), None)),
    }
}

/// The bytes of a Unix socket's path, its NUL included: 108 on Linux, 104 on
/// macOS and the BSDs (`sockaddr_un.sun_path`).
pub const SUN_PATH: usize = if cfg!(target_os = "linux") { 108 } else { 104 };

/// Can `addr` be bound or connected as it is spelled (Electron's Node takes
/// it so)? A Windows pipe always can; a socket path past [`SUN_PATH`] is
/// refused at once, in words, not as the kernel's EINVAL.
pub fn fits(addr: &str) -> io::Result<()> {
    match too_long(addr, if cfg!(windows) { usize::MAX } else { SUN_PATH }) {
        Some(m) => Err(io::Error::new(io::ErrorKind::InvalidInput, m)),
        None => Ok(()),
    }
}

/// What is wrong with a socket path `addr` where paths take `limit` bytes with
/// their NUL: nothing, or what to do. Pure.
pub fn too_long(addr: &str, limit: usize) -> Option<String> {
    (addr.len() >= limit).then(|| {
        format!(
            "the socket path {addr} is {} bytes; a Unix socket path here takes at most {} ({limit} with its NUL). \
             Fix: a shorter XDG_RUNTIME_DIR or TMPDIR, or a shorter app id",
            addr.len(),
            limit - 1
        )
    })
}

/// A connection, either end.
pub trait Io: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> Io for T {}
pub type Stream = Box<dyn Io>;

/// The door `name` of the app whose private socket dir is `dir`
/// (`paths::sockets`): `<dir>/<name>.sock`, or on Windows
/// `\\.\pipe\aio-rt-<dir's name>-<name>` (the dir name is `<id>-<hash>`).
pub fn addr(dir: &Path, name: &str) -> String {
    if cfg!(windows) {
        let d = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        format!(r"\\.\pipe\aio-rt-{d}-{name}")
    } else {
        dir.join(format!("{name}.sock")).to_string_lossy().into_owned()
    }
}

/// A listening door; dropping it removes it.
pub struct Listener {
    addr: String,
    #[cfg(unix)]
    inner: tokio::net::UnixListener,
    #[cfg(windows)]
    next: tokio::net::windows::named_pipe::NamedPipeServer,
}

impl Listener {
    /// Listen at `addr` (inside a tokio runtime). A stale socket file is
    /// replaced.
    #[cfg(unix)]
    pub fn bind(addr: &str) -> io::Result<Self> {
        _ = std::fs::remove_file(addr);
        let (at, _dir) = reach(addr)?;
        let l = Self { addr: addr.into(), inner: tokio::net::UnixListener::bind(at)? };
        bound().push(addr.into());
        Ok(l)
    }

    /// Listen at `addr` (inside a tokio runtime). A pipe someone else
    /// already holds is an error.
    #[cfg(windows)]
    pub fn bind(addr: &str) -> io::Result<Self> {
        Ok(Self { addr: addr.into(), next: win::server(addr, true)? })
    }

    /// The next connection of this same user; others are dropped.
    #[cfg(unix)]
    pub async fn accept(&mut self) -> io::Result<Stream> {
        loop {
            let (s, _) = self.inner.accept().await?;
            if crate::lifecycle::same_user(s.peer_cred()) {
                return Ok(Box::new(s));
            }
            eprintln!("[aio] {}: a connection from another user was refused", self.addr);
        }
    }

    /// The next connection: the pipe's DACL admits only this user.
    #[cfg(windows)]
    pub async fn accept(&mut self) -> io::Result<Stream> {
        Ok(Box::new(self.next_pipe().await?.0))
    }

    #[cfg(windows)]
    async fn next_pipe(&mut self) -> io::Result<(tokio::net::windows::named_pipe::NamedPipeServer, Option<u32>)> {
        use std::os::windows::io::AsRawHandle;
        self.next.connect().await?;
        let mut pid = 0u32;
        // SAFETY: the connected pipe's own handle, read once.
        let known = unsafe {
            windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId(self.next.as_raw_handle(), &mut pid)
        } != 0;
        let next = win::server(&self.addr, false)?;
        Ok((std::mem::replace(&mut self.next, next), known.then_some(pid)))
    }

    /// The next connection of the one process `only` names (and this user);
    /// any other is refused, said, and dropped. Unarmed, it waits a little
    /// for the process it is about to name (a window connects right after
    /// it is spawned).
    pub async fn accept_only(&mut self, only: &Only) -> io::Result<Stream> {
        loop {
            #[cfg(unix)]
            let (s, pid) = {
                let (s, _) = self.inner.accept().await?;
                let cred = s.peer_cred();
                let pid = cred.as_ref().ok().and_then(|c| c.pid()).and_then(|p| u32::try_from(p).ok());
                if !crate::lifecycle::same_user(cred) {
                    eprintln!("[aio] {}: a connection from another user was refused", self.addr);
                    continue;
                }
                (Box::new(s) as Stream, pid)
            };
            #[cfg(windows)]
            let (s, pid) = {
                let (s, pid) = self.next_pipe().await?;
                (Box::new(s) as Stream, pid)
            };
            let mut armed = only.0.subscribe();
            _ = tokio::time::timeout(ARMING, armed.wait_for(Option::is_some)).await;
            match refusal(pid, armed.borrow().as_ref(), crate::lifecycle::started) {
                None => return Ok(s),
                Some(why) => eprintln!("[aio] {}: refused a process: {why}", self.addr),
            }
        }
    }
}

/// How long a door waits for the process it will serve to be named.
const ARMING: std::time::Duration = std::time::Duration::from_secs(5);

/// The one process a door serves beyond being this user's (aio's local-peer
/// lockdown): the window this app spawned, told by pid and start time, so
/// another program of this same user, or a later process handed its pid, is
/// refused. Unarmed, it serves no one.
#[derive(Clone)]
pub struct Only(std::sync::Arc<tokio::sync::watch::Sender<Option<Process>>>);

/// A process: its pid and, where the OS says, when it started.
pub type Process = (u32, Option<String>);

impl Default for Only {
    fn default() -> Self {
        Self(std::sync::Arc::new(tokio::sync::watch::channel(None).0))
    }
}

impl Only {
    /// Serve process `pid` (just spawned), and no other.
    pub fn arm(&self, pid: u32) {
        self.0.send_replace(Some((pid, crate::lifecycle::started(pid))));
    }

    /// Process `pid` ended: serve no one (an older one ending changes nothing).
    pub fn disarm(&self, pid: u32) {
        self.0.send_if_modified(|a| a.take_if(|(p, _)| *p == pid).is_some());
    }

    /// The window's gate, one per process: every door that serves the
    /// window (aio-shell's wire and control lines, the routes' `http` door)
    /// asks this one, which aio-shell arms, so no two doors can disagree
    /// about who the window is (aio's one `localPeerGate`).
    pub fn window() -> Self {
        static W: std::sync::OnceLock<Only> = std::sync::OnceLock::new();
        W.get_or_init(Self::default).clone()
    }
}

/// Where this app serves its routes to its window (`aio://app/…`), once that
/// door is open: aio-shell hands it to the window (`--aio-http`).
static WINDOW_HTTP: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// The window's HTTP door ([`WINDOW_HTTP`]), if this app opened one.
pub fn window_http() -> Option<&'static str> {
    WINDOW_HTTP.get().map(String::as_str)
}

#[cfg_attr(not(feature = "server"), allow(dead_code))]
pub(crate) fn set_window_http(addr: String) {
    _ = WINDOW_HTTP.set(addr);
}

/// Why a door armed with `armed` (pid, start) refuses the peer `pid`, or
/// `None` (pure; `started` reads a live pid's start). An identity that can't
/// be read is a refusal, never a pass.
pub fn refusal(pid: Option<u32>, armed: Option<&Process>, started: impl Fn(u32) -> Option<String>) -> Option<String> {
    let Some(pid) = pid else { return Some("no pid could be read for it".into()) };
    let Some((want, start)) = armed else { return Some("the app's window has not been started".into()) };
    if pid != *want {
        return Some(format!("it is not this app's window (it is pid {pid}; the window is {want})"));
    }
    if start.is_some() && started(pid) != *start {
        return Some(format!("it has the window's pid ({pid}) but is not the process that was started"));
    }
    None
}

impl Drop for Listener {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            bound().retain(|a| *a != self.addr);
            _ = std::fs::remove_file(&self.addr);
        }
    }
}

/// The socket files of this process's listeners still open.
#[cfg(unix)]
fn bound() -> std::sync::MutexGuard<'static, Vec<String>> {
    static B: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    B.lock().unwrap_or_else(|e| e.into_inner())
}

/// Remove the socket file of every listener still open: a door served on
/// the runtime is never dropped (the process ends with it running), so a
/// clean stop ([`crate::shutdown`]) unlinks them. A Windows pipe goes with
/// its process.
pub fn unlink_all() {
    #[cfg(unix)]
    for a in std::mem::take(&mut *bound()) {
        _ = std::fs::remove_file(a);
    }
}

/// Connect to `addr` (inside a tokio runtime).
#[cfg(unix)]
pub async fn connect(addr: &str) -> io::Result<Stream> {
    let (at, _dir) = reach(addr)?;
    Ok(Box::new(tokio::net::UnixStream::connect(at).await?))
}

/// Connect to `addr` (inside a tokio runtime).
#[cfg(windows)]
pub async fn connect(addr: &str) -> io::Result<Stream> {
    use tokio::net::windows::named_pipe::ClientOptions;
    const ERROR_PIPE_BUSY: i32 = 231;
    // All instances busy: the server makes the next one at once.
    let mut tries = 0;
    loop {
        match ClientOptions::new().open(addr) {
            Ok(c) => return Ok(Box::new(c)),
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && tries < 50 => {}
            Err(e) => return Err(e),
        }
        tries += 1;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// Connect from blocking code: a file-like stream.
#[cfg(unix)]
pub fn connect_blocking(addr: &str) -> io::Result<Blocking> {
    let (at, _dir) = reach(addr)?;
    std::os::unix::net::UnixStream::connect(at).map(Blocking)
}

/// Connect from blocking code: a file-like stream.
#[cfg(windows)]
pub fn connect_blocking(addr: &str) -> io::Result<Blocking> {
    std::fs::OpenOptions::new().read(true).write(true).open(addr).map(Blocking)
}

/// A blocking connection.
pub struct Blocking(#[cfg(unix)] std::os::unix::net::UnixStream, #[cfg(windows)] std::fs::File);

impl Blocking {
    pub fn try_clone(&self) -> io::Result<Self> {
        self.0.try_clone().map(Blocking)
    }
}

impl io::Read for Blocking {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        self.0.read(b)
    }
}

impl io::Write for Blocking {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.0.write(b)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

#[cfg(windows)]
pub(crate) mod win {
    use std::io;
    use tokio::net::windows::named_pipe::{NamedPipeServer, PipeMode, ServerOptions};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{GetTokenInformation, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// This user's SID, as text (`S-1-5-21-…`).
    pub(crate) fn user_sid() -> io::Result<String> {
        // SAFETY: Win32 calls on this process's own token; every buffer is
        // sized by the call before it, and every handle freed.
        unsafe {
            let mut token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut len = 0u32;
            GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len);
            let mut buf = vec![0u8; len as usize];
            let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len);
            CloseHandle(token);
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            let user = &*(buf.as_ptr() as *const TOKEN_USER);
            let mut s: *mut u16 = std::ptr::null_mut();
            if ConvertSidToStringSidW(user.User.Sid, &mut s) == 0 {
                return Err(io::Error::last_os_error());
            }
            let n = (0..).take_while(|&i| *s.add(i) != 0).count();
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(s, n));
            LocalFree(s.cast());
            Ok(sid)
        }
    }

    /// A security descriptor that admits only this user (full access) and
    /// no one else, the protected way (no inherited entries).
    pub(crate) struct OnlyMe(*mut core::ffi::c_void);

    impl OnlyMe {
        pub(crate) fn new() -> io::Result<Self> {
            let sddl: Vec<u16> = format!("D:P(A;;GA;;;{})", user_sid()?).encode_utf16().chain([0]).collect();
            let mut sd = std::ptr::null_mut();
            // SAFETY: a NUL-terminated SDDL string; the descriptor is freed on drop.
            if unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    SDDL_REVISION_1,
                    &mut sd,
                    std::ptr::null_mut(),
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(Self(sd))
        }

        pub(crate) fn attributes(&self) -> SECURITY_ATTRIBUTES {
            SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: self.0,
                bInheritHandle: 0,
            }
        }
    }

    impl Drop for OnlyMe {
        fn drop(&mut self) {
            // SAFETY: allocated by ConvertStringSecurityDescriptor…, once.
            unsafe { LocalFree(self.0) };
        }
    }

    /// One pipe instance; `first`: refuse if anyone holds the name already
    /// (no squatting on the app's door).
    pub(crate) fn server(addr: &str, first: bool) -> io::Result<NamedPipeServer> {
        let sd = OnlyMe::new()?;
        let mut sa = sd.attributes();
        // SAFETY: `sa` and its descriptor outlive the call.
        unsafe {
            ServerOptions::new()
                .first_pipe_instance(first)
                .reject_remote_clients(true)
                .pipe_mode(PipeMode::Byte)
                .create_with_security_attributes_raw(addr, (&mut sa as *mut SECURITY_ATTRIBUTES).cast())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn a_door_takes_several_connections_and_goes_with_its_listener() {
        let d = tempfile::tempdir().unwrap();
        let a = addr(d.path(), &format!("t{}", std::process::id()));
        let mut l = Listener::bind(&a).unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let mut s = l.accept().await.unwrap();
                let mut b = [0u8; 2];
                s.read_exact(&mut b).await.unwrap();
                s.write_all(&b).await.unwrap();
            }
        });
        for m in [b"hi", b"yo"] {
            let mut c = connect(&a).await.unwrap();
            c.write_all(m).await.unwrap();
            let mut b = [0u8; 2];
            c.read_exact(&mut b).await.unwrap();
            assert_eq!(&b, m);
        }
        server.await.unwrap();
        assert!(connect(&a).await.is_err(), "the door went with its listener");
    }

    #[test]
    fn a_socket_path_past_the_limit_is_told_in_words() {
        let at = |n: usize| format!("/{}", "x".repeat(n - 1));
        for limit in [108, 104] {
            assert_eq!(too_long(&at(limit - 1), limit), None, "{limit}: the longest that fits");
            let m = too_long(&at(limit), limit).unwrap();
            assert!(m.contains(&at(limit)) && m.contains(&format!("is {limit} bytes")), "{m}");
            assert!(m.contains(&format!("at most {} ({limit} with its NUL)", limit - 1)), "{m}");
            assert!(m.contains("XDG_RUNTIME_DIR or TMPDIR") && m.contains("app id"), "{m}");
        }
        assert_eq!(SUN_PATH, if cfg!(target_os = "linux") { 108 } else { 104 });
        #[cfg(unix)]
        {
            assert!(fits(&at(SUN_PATH - 1)).is_ok());
            let e = fits(&at(SUN_PATH)).unwrap_err();
            assert!(e.kind() == io::ErrorKind::InvalidInput && e.to_string().contains("Fix:"), "{e}");
        }
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_door_in_a_folder_past_the_socket_path_limit_works() {
        let d = tempfile::tempdir().unwrap();
        let deep = d.path().join("a-folder-name-of-forty-characters-long-x".repeat(3));
        std::fs::create_dir_all(&deep).unwrap();
        let a = addr(&deep, "door");
        assert!(a.len() > 108, "{a}");
        let mut l = Listener::bind(&a).unwrap();
        let server = tokio::spawn(async move { l.accept().await.unwrap().write_all(b"ok").await.unwrap() });
        let mut b = [0u8; 2];
        connect(&a).await.unwrap().read_exact(&mut b).await.unwrap();
        assert_eq!(&b, b"ok");
        server.await.unwrap();
        assert!(connect_blocking(&a).is_err(), "gone with its listener");
    }

    #[test]
    fn only_the_started_process_passes() {
        let start = |s: &'static str| move |_| Some(s.to_string());
        let armed = (42, Some("t0".to_string()));
        assert_eq!(refusal(Some(42), Some(&armed), start("t0")), None);
        let no = |pid, armed, s| refusal(pid, armed, start(s)).unwrap();
        assert!(no(None, Some(&armed), "t0").contains("no pid"), "unknown is refused, not passed");
        assert!(no(Some(42), None, "t0").contains("not been started"));
        assert!(no(Some(7), Some(&armed), "t0").contains("is pid 7; the window is 42"));
        assert!(no(Some(42), Some(&armed), "t1").contains("not the process that was started"), "a reused pid");
        // Where no start can be read, the pid alone decides.
        assert_eq!(refusal(Some(42), Some(&(42, None)), |_| None), None);
        // A start is read for a live process, the same each time.
        let me = std::process::id();
        #[cfg(any(target_os = "linux", windows, target_os = "macos"))]
        assert!(crate::lifecycle::started(me).is_some());
        assert_eq!(crate::lifecycle::started(me), crate::lifecycle::started(me));
        assert_eq!(crate::lifecycle::started(u32::MAX - 1), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_door_for_one_process_serves_it_and_refuses_the_rest() {
        let d = tempfile::tempdir().unwrap();
        let a = addr(d.path(), &format!("o{}", std::process::id()));
        let mut l = Listener::bind(&a).unwrap();
        let only = Only::default();
        let other = std::process::Command::new("sleep").arg("5").spawn().unwrap();
        only.arm(other.id());
        let served = tokio::spawn({
            let only = only.clone();
            async move {
                let mut s = l.accept_only(&only).await.unwrap();
                s.write_all(b"ok").await.unwrap();
                l
            }
        });
        // This process is not the one armed: dropped, never answered.
        let mut c = connect(&a).await.unwrap();
        assert_eq!(c.read(&mut [0u8; 2]).await.unwrap(), 0);
        // Armed with this process (still the same user): served.
        only.arm(std::process::id());
        let mut c = connect(&a).await.unwrap();
        let mut b = [0u8; 2];
        c.read_exact(&mut b).await.unwrap();
        assert_eq!(&b, b"ok");
        let mut l = served.await.unwrap();
        // Its end disarms it; an older pid ending changes nothing.
        only.disarm(other.id());
        assert!(only.0.borrow().is_some());
        only.disarm(std::process::id());
        let refused = tokio::spawn(async move { tokio::time::timeout(ARMING / 10, l.accept_only(&only)).await });
        let mut c = connect(&a).await.unwrap();
        assert_eq!(c.read(&mut [0u8; 2]).await.unwrap(), 0);
        assert!(refused.await.unwrap().is_err(), "nobody served");
        let mut other = other;
        _ = other.kill();
        _ = other.wait();
    }

    /// On Windows: the pipe's DACL names only this user, protected; a
    /// second holder of the name and any other identity are refused.
    #[cfg(windows)]
    #[tokio::test]
    async fn a_pipe_admits_only_this_user() {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, LocalFree};
        use windows_sys::Win32::Security::Authorization::{
            ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertStringSidToSidW, GetSecurityInfo,
            SDDL_REVISION_1, SE_KERNEL_OBJECT,
        };
        use windows_sys::Win32::Security::{DACL_SECURITY_INFORMATION, EqualSid, ImpersonateAnonymousToken, RevertToSelf};
        use windows_sys::Win32::System::Threading::GetCurrentThread;
        let a = addr(Path::new("x"), &format!("acl{}", std::process::id()));
        let l = Listener::bind(&a).unwrap();
        // SAFETY: the listener's own pipe handle; the descriptor and its
        // text are freed once.
        let sddl = unsafe {
            let mut sd = std::ptr::null_mut();
            let e = GetSecurityInfo(
                l.next.as_raw_handle(),
                SE_KERNEL_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut sd,
            );
            assert_eq!(e, 0);
            let mut s: *mut u16 = std::ptr::null_mut();
            assert_ne!(
                ConvertSecurityDescriptorToStringSecurityDescriptorW(
                    sd,
                    SDDL_REVISION_1,
                    DACL_SECURITY_INFORMATION,
                    &mut s,
                    std::ptr::null_mut()
                ),
                0
            );
            let n = (0..).take_while(|&i| *s.add(i) != 0).count();
            let t = String::from_utf16_lossy(std::slice::from_raw_parts(s, n));
            LocalFree(s.cast());
            LocalFree(sd);
            t
        };
        // One allow entry, for this user, nothing inherited.
        assert!(sddl.starts_with("D:P(A;;") && sddl.ends_with(')'), "{sddl}");
        assert_eq!(sddl.matches('(').count(), 1, "{sddl}");
        // Its trustee is this user. The text may name a well-known account by
        // its alias (`LA` for the built-in Administrator), so the SIDs are compared.
        let sid = |s: &str| {
            let w: Vec<u16> = s.encode_utf16().chain([0]).collect();
            let mut p = std::ptr::null_mut();
            // SAFETY: a NUL-terminated SID string; the SID is freed by the caller.
            assert_ne!(unsafe { ConvertStringSidToSidW(w.as_ptr(), &mut p) }, 0, "{s}");
            p
        };
        let trustee = sid(sddl[..sddl.len() - 1].rsplit(';').next().unwrap());
        let me = sid(&win::user_sid().unwrap());
        // SAFETY: two valid SIDs from ConvertStringSidToSidW, each freed once.
        let same = unsafe {
            let same = EqualSid(trustee, me) != 0;
            LocalFree(trustee);
            LocalFree(me);
            same
        };
        assert!(same, "{sddl}");
        assert!(Listener::bind(&a).is_err(), "nobody else may hold the app's door");
        // Another identity (anonymous, on its own thread) is denied.
        let other = std::thread::spawn({
            let a = a.clone();
            move || {
                // SAFETY: this thread's own token; reverted before it ends.
                assert_ne!(unsafe { ImpersonateAnonymousToken(GetCurrentThread()) }, 0);
                let r = std::fs::OpenOptions::new().read(true).write(true).open(&a);
                // SAFETY: as above.
                unsafe { RevertToSelf() };
                r.map(drop)
            }
        });
        let e = other.join().unwrap().unwrap_err();
        assert_eq!(e.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32), "{e}");
        // This user opens it.
        let mine = std::fs::OpenOptions::new().read(true).write(true).open(&a);
        assert!(mine.is_ok(), "{mine:?}");
    }

    #[test]
    fn a_door_is_named_after_its_app() {
        let a = addr(Path::new("/run/u/aio-rt/notes-0a1b2c3d"), "hand");
        if cfg!(windows) {
            assert_eq!(a, r"\\.\pipe\aio-rt-notes-0a1b2c3d-hand");
        } else {
            assert_eq!(a, "/run/u/aio-rt/notes-0a1b2c3d/hand.sock");
        }
    }
}

//! Programs the app runs (aio's `spawn` and `runCommand`): each in a
//! process group of its own (a job object on Windows), so a kill reaches
//! the whole tree, never only the process you can see; its output as
//! lines while it runs (`\r` ends one too: a progress bar that rewrites
//! its line); its stdin when asked for; paused and resumed as a group;
//! killed when its [`Child`] is dropped (an aborted effect), and at the
//! app's stop, named, if it still runs then. A program runs on the wall
//! clock: its timeout too, also under `TestCells`' virtual one.
//!
//! On Windows no child opens a console window: each starts with
//! `CREATE_NO_WINDOW`, and a release app (the `windows_subsystem` exe,
//! which has no console) adopts one hidden console at start
//! ([`hide_console`]), so the programs anything else starts open none either.
//!
//! ```no_run
//! # async fn f() -> std::io::Result<()> {
//! use aio::process::{Command, Stream};
//! let mut job = Command::new("ffmpeg")
//!     .args(["-i", "in.mov", "out.mp4"])
//!     .on_line(|line, from| if from == Stream::Stderr { eprintln!("{line}") })
//!     .spawn()?;
//! job.pause()?;
//! job.resume()?;
//! let status = job.wait().await?;
//! # Ok(()) }
//! ```
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

/// Which of the child's outputs a line came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

/// What hears each line.
type OnLine = Box<dyn FnMut(&str, Stream) + Send>;

/// A program to run: its arguments, folder, environment and stdin.
pub struct Command {
    program: OsString,
    args: Vec<OsString>,
    cwd: Option<PathBuf>,
    env: Vec<(OsString, OsString)>,
    stdin: bool,
    grace: Duration,
    on_line: Option<OnLine>,
}

/// What a program run to its end ([`Command::run`]) wrote, and how it ended.
#[derive(Debug)]
pub struct Output {
    pub status: ExitStatus,
    /// Its stdout lines, joined by `\n`: all it wrote, also when cut short.
    pub stdout: String,
    pub stderr: String,
    /// The timeout ran out and the tree was killed.
    pub timed_out: bool,
}

impl Command {
    /// `program`, found on `PATH` as the OS finds it. The child gets the
    /// app's environment without aio-rs's own settings (`AIORT_*`, see
    /// [`crate::lifecycle::scrub`]), and its stdin is empty unless asked.
    pub fn new(program: impl AsRef<OsStr>) -> Self {
        Self {
            program: program.as_ref().into(),
            args: vec![],
            cwd: None,
            env: vec![],
            stdin: false,
            grace: Duration::from_secs(2),
            on_line: None,
        }
    }

    pub fn arg(mut self, a: impl AsRef<OsStr>) -> Self {
        self.args.push(a.as_ref().into());
        self
    }

    pub fn args<I: IntoIterator<Item = S>, S: AsRef<OsStr>>(mut self, args: I) -> Self {
        self.args.extend(args.into_iter().map(|a| a.as_ref().into()));
        self
    }

    /// Where it runs; unset: the app's folder (Windows: its own system
    /// folder, so an update can move the app's while it runs).
    pub fn cwd(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    pub fn env(mut self, k: impl AsRef<OsStr>, v: impl AsRef<OsStr>) -> Self {
        self.env.push((k.as_ref().into(), v.as_ref().into()));
        self
    }

    /// A pipe to its stdin ([`Child::write`], [`Child::close_stdin`]). Off
    /// by default: a program that reads a pipe nobody writes waits forever.
    pub fn stdin(mut self) -> Self {
        self.stdin = true;
        self
    }

    /// How long a killed tree has to end before it is killed hard; 2 s
    /// unset (Unix: Windows has no polite end for a console program).
    pub fn grace(mut self, d: Duration) -> Self {
        self.grace = d;
        self
    }

    /// Each complete line of its output, as it comes (also with [`run`](Self::run)).
    pub fn on_line(mut self, f: impl FnMut(&str, Stream) + Send + 'static) -> Self {
        self.on_line = Some(Box::new(f));
        self
    }

    /// The program's name, as said in the log.
    fn name(&self) -> String {
        std::path::Path::new(&self.program).file_name().unwrap_or(&self.program).to_string_lossy().into_owned()
    }

    /// Start it. Inside the app's runtime (an effect, a method's async part).
    pub fn spawn(self) -> io::Result<Child> {
        let mut cmd = tokio::process::Command::new(&self.program);
        crate::lifecycle::scrub(cmd.as_std_mut(), false);
        cmd.args(&self.args)
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .stdin(if self.stdin { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        cmd.process_group(0);
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);
        match &self.cwd {
            Some(d) => _ = cmd.current_dir(d),
            #[cfg(windows)]
            None => _ = cmd.current_dir(std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into())),
            #[cfg(not(windows))]
            None => {}
        }
        let name = self.name();
        let mut child = cmd.spawn().map_err(|e| io::Error::new(e.kind(), format!("{name}: {e}")))?;
        let pid = child.id().ok_or_else(|| io::Error::other(format!("{name}: exited at once")))?;
        #[cfg(windows)]
        let job = Job::of(&child).map_err(|e| {
            _ = child.start_kill();
            io::Error::new(e.kind(), format!("{name}: no job object to hold its tree: {e}"))
        })?;
        let on_line = Arc::new(Mutex::new(self.on_line.unwrap_or_else(|| Box::new(|_, _| {}))));
        let read = |r: Option<Box<dyn AsyncRead + Unpin + Send>>, from| {
            let f = on_line.clone();
            r.map(|r| tokio::spawn(lines(r, move |l| (f.lock().unwrap_or_else(|e| e.into_inner()))(l, from))))
        };
        let out = read(child.stdout.take().map(|s| Box::new(s) as _), Stream::Stdout);
        let err = read(child.stderr.take().map(|s| Box::new(s) as _), Stream::Stderr);
        let group = Arc::new(Group {
            pid,
            name,
            grace: self.grace,
            paused: Default::default(),
            #[cfg(windows)]
            job,
        });
        lock().push(group.clone());
        Ok(Child { stdin: child.stdin.take(), child, readers: [out, err], group, status: None })
    }

    /// Run it to its end and keep what it wrote (its lines also go to
    /// [`on_line`](Self::on_line) as they come); `timeout`: then the tree
    /// is killed and what it wrote so far kept. A program that fails is an
    /// [`Output`] (`status`), one that can't start an error.
    pub async fn run(mut self, timeout: Option<Duration>) -> io::Result<Output> {
        let got = Arc::new(Mutex::new([Vec::new(), Vec::new()]));
        let (keep, mut theirs) = (got.clone(), self.on_line.take());
        let mut child = self
            .on_line(move |l, from| {
                keep.lock().unwrap_or_else(|e| e.into_inner())[from as usize].push(l.to_string());
                if let Some(f) = theirs.as_mut() {
                    f(l, from);
                }
            })
            .spawn()?;
        let (status, timed_out) = match timeout {
            Some(t) => {
                // The wall clock, off the runtime's (a test's runs virtual).
                let (_alive, ended) = std::sync::mpsc::channel::<()>();
                let mut clock = tokio::task::spawn_blocking(move || {
                    ended.recv_timeout(t) == Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                });
                tokio::select! {
                    s = child.wait() => (s?, false),
                    Ok(true) = &mut clock => (child.kill().await?, true),
                }
            }
            None => (child.wait().await?, false),
        };
        let [out, err] = std::mem::take(&mut *got.lock().unwrap_or_else(|e| e.into_inner()));
        Ok(Output { status, stdout: out.join("\n"), stderr: err.join("\n"), timed_out })
    }
}

/// A running program. Dropped, its whole tree is killed.
pub struct Child {
    child: tokio::process::Child,
    stdin: Option<tokio::process::ChildStdin>,
    readers: [Option<tokio::task::JoinHandle<()>>; 2],
    group: Arc<Group>,
    status: Option<ExitStatus>,
}

/// What a kill needs, also from the registry at the app's stop.
struct Group {
    pid: u32,
    /// Unix: TERM, then KILL after this; Windows ends a job at once.
    #[cfg_attr(windows, allow(dead_code))]
    grace: Duration,
    name: String,
    paused: std::sync::atomic::AtomicBool,
    #[cfg(windows)]
    job: Job,
}

impl Child {
    /// Its process id; on Unix also its process group's.
    pub fn id(&self) -> u32 {
        self.group.pid
    }

    pub fn paused(&self) -> bool {
        self.group.paused.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Stop the whole tree where it is (SIGSTOP to its group). Windows has
    /// no such thing: an error there.
    pub fn pause(&self) -> io::Result<()> {
        self.group.signal_stop(true)
    }

    pub fn resume(&self) -> io::Result<()> {
        self.group.signal_stop(false)
    }

    /// Write to its stdin ([`Command::stdin`]); an error once it has exited
    /// or [`close_stdin`](Self::close_stdin) ran, never a lost write.
    pub async fn write(&mut self, data: impl AsRef<[u8]>) -> io::Result<()> {
        let name = &self.group.name;
        let w = self.stdin.as_mut().ok_or_else(|| {
            io::Error::new(io::ErrorKind::BrokenPipe, format!("{name}: no stdin (Command::stdin) or it was closed"))
        })?;
        w.write_all(data.as_ref()).await?;
        w.flush().await
    }

    /// End its stdin: a program that reads to the end (`sort`, a filter)
    /// goes on from here.
    pub fn close_stdin(&mut self) {
        self.stdin = None;
    }

    /// Its exit, once its output has been read to the end.
    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        if let Some(s) = self.status {
            return Ok(s);
        }
        // Held while it runs: a paused (test) clock never jumps past a
        // program, which runs on the wall clock.
        let (_running, ended) = std::sync::mpsc::channel::<()>();
        let _held = tokio::task::spawn_blocking(move || _ = ended.recv());
        let s = self.child.wait().await?;
        self.stdin = None;
        for r in self.readers.iter_mut().filter_map(Option::take) {
            _ = r.await;
        }
        self.status = Some(s);
        Ok(s)
    }

    /// Kill the whole tree: resumed if paused, asked to end (SIGTERM), then
    /// killed hard once its grace is up; its exit.
    pub async fn kill(&mut self) -> io::Result<ExitStatus> {
        if let Some(s) = self.status {
            return Ok(s);
        }
        self.group.end();
        self.wait().await
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        let g = &self.group;
        lock().retain(|x| !Arc::ptr_eq(x, g));
        // A child that exited may leave a tree: the group gets it all the same.
        g.end();
    }
}

impl Group {
    #[cfg(unix)]
    fn signal_stop(&self, stop: bool) -> io::Result<()> {
        self.group_signal(if stop { libc::SIGSTOP } else { libc::SIGCONT })?;
        self.paused.store(stop, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    #[cfg(windows)]
    fn signal_stop(&self, _: bool) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Unsupported, format!("{}: Windows can't pause a program", self.name)))
    }

    #[cfg(unix)]
    fn group_signal(&self, sig: i32) -> io::Result<()> {
        // SAFETY: a signal to this child's own group (`process_group(0)`),
        // never the app's: the negative pid is the group.
        match unsafe { libc::kill(-(self.pid as i32), sig) } {
            0 => Ok(()),
            _ => Err(io::Error::last_os_error()),
        }
    }

    /// The tree ended: on Unix SIGCONT (a stopped process can't take a
    /// TERM), SIGTERM, then SIGKILL after the grace to what is left of the
    /// group; on Windows its job ended.
    fn end(&self) {
        #[cfg(unix)]
        {
            _ = self.group_signal(libc::SIGCONT);
            if self.group_signal(libc::SIGTERM).is_err() {
                return;
            }
            let (pid, grace) = (self.pid as i32, self.grace);
            _ = std::thread::Builder::new().name("aio-kill".into()).spawn(move || {
                let until = std::time::Instant::now() + grace;
                // SAFETY: signal 0 only asks whether the group has a member.
                while std::time::Instant::now() < until && unsafe { libc::kill(-pid, 0) } == 0 {
                    std::thread::sleep(Duration::from_millis(20));
                }
                // SAFETY: as above, the child's own group.
                unsafe { libc::kill(-pid, libc::SIGKILL) };
            });
        }
        #[cfg(windows)]
        self.job.end();
    }
}

/// The programs started and not dropped, for the app's stop.
static LIVE: Mutex<Vec<Arc<Group>>> = Mutex::new(Vec::new());

fn lock() -> std::sync::MutexGuard<'static, Vec<Arc<Group>>> {
    LIVE.lock().unwrap_or_else(|e| e.into_inner())
}

/// At the app's stop ([`crate::shutdown`]): kill every program still
/// running, and say which; nobody else would ever end them.
pub(crate) fn reap() {
    if let Some(said) = kill_all(&std::mem::take(&mut *lock())) {
        eprintln!("{said}");
    }
}

/// End each of `left`; what is said, if any was.
fn kill_all(left: &[Arc<Group>]) -> Option<String> {
    left.iter().for_each(|g| g.end());
    let names: Vec<String> = left.iter().map(|g| format!("{} (pid {})", g.name, g.pid)).collect();
    (!left.is_empty())
        .then(|| format!("[aio] stopping: killed {} program(s) still running: {}", left.len(), names.join(", ")))
}

/// Read `r` to its end, each line (`\n`, `\r\n` or `\r` ended; the last
/// one unended too) to `f`.
async fn lines(mut r: impl AsyncRead + Unpin, mut f: impl FnMut(&str)) {
    let mut s = Splitter::default();
    let mut buf = [0u8; 8192];
    while let Ok(n @ 1..) = r.read(&mut buf).await {
        s.push(&buf[..n], &mut f);
    }
    s.finish(&mut f);
}

/// Bytes into lines (pure); a `\r` then `\n` is one end, even split
/// across reads.
#[derive(Default)]
struct Splitter {
    line: Vec<u8>,
    cr: bool,
}

impl Splitter {
    fn push(&mut self, b: &[u8], f: &mut impl FnMut(&str)) {
        for &c in b {
            if std::mem::take(&mut self.cr) && c == b'\n' {
                continue;
            }
            match c {
                b'\n' | b'\r' => {
                    self.cr = c == b'\r';
                    f(&String::from_utf8_lossy(&self.line));
                    self.line.clear();
                }
                _ => self.line.push(c),
            }
        }
    }

    fn finish(self, f: &mut impl FnMut(&str)) {
        if !self.line.is_empty() {
            f(&String::from_utf8_lossy(&self.line));
        }
    }
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A Windows job object holding a child's tree: ended as one, and with the
/// app (its last handle closed).
#[cfg(windows)]
struct Job(windows_sys::Win32::Foundation::HANDLE);

// SAFETY: a kernel handle, usable from any thread.
#[cfg(windows)]
unsafe impl Send for Job {}
// SAFETY: as above; its calls take no Rust-side state.
#[cfg(windows)]
unsafe impl Sync for Job {}

#[cfg(windows)]
impl Job {
    fn of(child: &tokio::process::Child) -> io::Result<Self> {
        use windows_sys::Win32::System::JobObjects::*;
        let raw = child.raw_handle().ok_or_else(|| io::Error::other("exited at once"))?;
        // SAFETY: plain Win32 calls on a new job and the child's live handle.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(io::Error::last_os_error());
            }
            let me = Self(job);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let size = std::mem::size_of_val(&info) as u32;
            if SetInformationJobObject(job, JobObjectExtendedLimitInformation, (&raw const info).cast(), size) == 0
                || AssignProcessToJobObject(job, raw as _) == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(me)
        }
    }

    fn end(&self) {
        // SAFETY: this job's live handle.
        unsafe { windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0, 1) };
    }
}

#[cfg(windows)]
impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: closed once, here.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

/// Windows: a process with no console (a release app, double-clicked)
/// adopts one hidden console, so no console program it starts afterwards,
/// by whatever means (`std::process`, a library), opens a console window.
/// Windows would give each one a new, visible console (aio's
/// `adoptHiddenConsole`). It attaches to the console of a `PING.EXE` started
/// with `CREATE_NO_WINDOW`, ends it, and keeps the console. A process with
/// a console (a terminal, `ax dev`) is left as it is; elsewhere nothing.
/// Never fatal: on a failure the app runs as before, said in the log.
pub fn hide_console() {
    #[cfg(windows)]
    if let Err(e) = adopt_console() {
        eprintln!("[aio] no hidden console ({e}): a console program this app starts may open a window");
    }
}

#[cfg(windows)]
fn adopt_console() -> io::Result<()> {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Console::{AttachConsole, GetConsoleCP};
    // SAFETY: no preconditions; 0 is "no console".
    if unsafe { GetConsoleCP() } != 0 {
        return Ok(());
    }
    let root = PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into()));
    let mut ping = std::process::Command::new(root.join(r"System32\PING.EXE"))
        .args(["-n", "30", "127.0.0.1"])
        .current_dir(&root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()?;
    let until = std::time::Instant::now() + Duration::from_secs(2);
    let attached = loop {
        // SAFETY: the helper's pid, alive until killed below.
        if unsafe { AttachConsole(ping.id()) } != 0 {
            break Ok(());
        }
        let e = io::Error::last_os_error();
        if std::time::Instant::now() > until {
            break Err(e);
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    _ = ping.kill();
    _ = ping.wait();
    attached
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(chunks: &[&[u8]]) -> Vec<String> {
        let (mut s, mut got) = (Splitter::default(), vec![]);
        for c in chunks {
            s.push(c, &mut |l| got.push(l.to_string()));
        }
        s.finish(&mut |l| got.push(l.to_string()));
        got
    }

    #[test]
    fn a_line_ends_at_a_newline_a_return_or_both() {
        // aio tests/spawn: a progress bar's `\r` lines come as they are written.
        assert_eq!(split(&[b"a\nb\r\nc\r10%\r20%", b"\rlast"]), ["a", "b", "c", "10%", "20%", "last"]);
        assert_eq!(split(&[b"x\r", b"\ny\n"]), ["x", "y"], "a \\r\\n split across reads is one end");
        assert_eq!(split(&[b"\n\n"]), ["", ""], "empty lines stay");
        assert_eq!(split(&[b""]), Vec::<String>::new());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn its_lines_stream_its_stdin_is_written_and_it_ends_as_it_exits() {
        let got = Arc::new(Mutex::new(vec![]));
        let keep = got.clone();
        let mut c = Command::new("sh")
            .args(["-c", "read x; echo \"got $x\"; echo oops >&2; exit 3"])
            .stdin()
            .on_line(move |l, from| keep.lock().unwrap().push((l.to_string(), from)))
            .spawn()
            .unwrap();
        c.write("milk\n").await.unwrap();
        let st = c.wait().await.unwrap();
        assert_eq!(st.code(), Some(3));
        let mut got = got.lock().unwrap().clone();
        got.sort_by_key(|(_, f)| *f as u8);
        assert_eq!(got, [("got milk".to_string(), Stream::Stdout), ("oops".to_string(), Stream::Stderr)]);
        assert!(c.write("late").await.is_err(), "a write after the exit is an error, never lost silently");
        assert!(lock().iter().any(|g| g.pid == c.id()), "tracked until dropped");
        let pid = c.id();
        drop(c);
        assert!(!lock().iter().any(|g| g.pid == pid));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_timeout_kills_the_whole_tree_on_time_and_keeps_what_it_wrote() {
        // aio runCommand: `sh` and its `sleep` child both go, at the deadline.
        let d = tempfile::tempdir().unwrap();
        let pidfile = d.path().join("pid");
        let script = format!("echo first; sleep 30 & echo $! > {}; wait", pidfile.display());
        let t0 = std::time::Instant::now();
        let out = Command::new("sh").args(["-c", &script]).run(Some(Duration::from_millis(300))).await.unwrap();
        assert!(out.timed_out && !out.status.success());
        assert!(t0.elapsed() < Duration::from_secs(3), "{:?}", t0.elapsed());
        assert_eq!(out.stdout, "first");
        let grandchild: i32 = std::fs::read_to_string(&pidfile).unwrap().trim().parse().unwrap();
        let until = std::time::Instant::now() + Duration::from_secs(3);
        // SAFETY: signal 0 asks only whether it lives.
        while unsafe { libc::kill(grandchild, 0) } == 0 && std::time::Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        // SAFETY: as above.
        assert_ne!(unsafe { libc::kill(grandchild, 0) }, 0, "the grandchild went with its group");
        let ok = Command::new("sh").args(["-c", "echo done"]).run(None).await.unwrap();
        assert!(ok.status.success() && !ok.timed_out && ok.stdout == "done");
        let missing = Command::new("aio-no-such-program").run(None).await.unwrap_err();
        assert!(missing.to_string().starts_with("aio-no-such-program: "), "{missing}");
    }

    /// Windows: the job ends the tree. `ping` holds the pipe `cmd` gave it,
    /// so the output ends (and `run` returns) only once the grandchild is gone.
    #[cfg(windows)]
    #[tokio::test]
    async fn a_timeout_ends_the_whole_job_on_time() {
        let t0 = std::time::Instant::now();
        let out = Command::new("cmd")
            .args(["/C", "echo first & ping -n 30 127.0.0.1 >NUL & echo never"])
            .run(Some(Duration::from_millis(500)))
            .await
            .unwrap();
        assert!(out.timed_out && !out.status.success());
        assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", t0.elapsed());
        assert_eq!(out.stdout.trim(), "first");
        let ok = Command::new("cmd").args(["/C", "echo done"]).run(None).await.unwrap();
        assert_eq!((ok.status.success(), ok.stdout.trim()), (true, "done"));
        let c = Command::new("cmd").args(["/C", "ping -n 30 127.0.0.1"]).spawn().unwrap();
        assert_eq!(c.pause().unwrap_err().kind(), io::ErrorKind::Unsupported);
        // Already in a console (the test's): nothing to adopt.
        hide_console();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_paused_tree_stops_and_is_still_killed() {
        let mut c = Command::new("sh").args(["-c", "while :; do echo t; sleep 0.05; done"]).spawn().unwrap();
        c.pause().unwrap();
        assert!(c.paused());
        // A stopped process can't take a TERM: the kill resumes it first.
        let st = tokio::time::timeout(Duration::from_secs(5), c.kill()).await.unwrap().unwrap();
        assert!(!st.success());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_apps_stop_kills_what_still_runs() {
        let mut c = Command::new("sleep").arg("30").spawn().unwrap();
        let pid = c.id();
        // Other tests' programs share the registry: this one's alone.
        let mine: Vec<_> = lock().iter().filter(|g| g.pid == pid).cloned().collect();
        let said = kill_all(&mine).unwrap();
        assert_eq!(said, format!("[aio] stopping: killed 1 program(s) still running: sleep (pid {pid})"));
        assert_eq!(kill_all(&[]), None);
        let st = tokio::time::timeout(Duration::from_secs(5), c.wait()).await.unwrap().unwrap();
        assert!(!st.success());
    }
}

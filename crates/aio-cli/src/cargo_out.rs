//! cargo's own words while a build runs, as `am dev` shows its: each line as
//! it comes, cargo's progress bar on a terminal, and a warning this session
//! already showed in full said again in one line. What is kept (the
//! `build.log`, the page) is the plain text: no colour, no progress.
use std::collections::HashSet;
use std::io::{IsTerminal, Read, Write};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::Mutex;
#[cfg(unix)]
use std::sync::atomic::AtomicI32;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

/// A finished cargo: how it ended, its stdout (when taken) and what it said
/// on stderr, plain.
pub struct Ran {
    pub status: ExitStatus,
    pub stdout: String,
    pub said: String,
    /// Stopped: a newer save made it stale ([`stale`]).
    pub stopped: bool,
}

/// A build a newer save may stop: one per session, shared by who sees the
/// saves ([`Stop::stale`]) and who runs cargo ([`run`]).
#[derive(Clone, Default)]
pub struct Stop(Arc<AtomicBool>);

impl Stop {
    /// A save landed: the build running now is stale, stopped (its finished
    /// crates kept by cargo), and the next one starts at once. A save while
    /// none runs stops nothing: [`run`] starts each build unstopped.
    pub fn stale(&self) {
        self.0.store(true, Relaxed);
    }
}

/// The process groups of the cargos running now (0: a free slot): a Ctrl-C
/// (or the terminal closing) reaches each of them, never orphaning its rustc
/// calls.
#[cfg(unix)]
static GROUPS: [AtomicI32; 8] = [const { AtomicI32::new(0) }; 8];

/// SIGINT, SIGTERM, SIGHUP to ax: passed on to every cargo's group, what
/// is [`arm`]ed started, then ax ends as it would have (only
/// async-signal-safe calls).
#[cfg(unix)]
extern "C" fn forward(sig: libc::c_int) {
    start_on_exit();
    for g in &GROUPS {
        let pgid = g.load(Relaxed);
        if pgid > 0 {
            // SAFETY: a signal to a group ax started.
            unsafe { libc::killpg(pgid, sig) };
        }
    }
    // SAFETY: the default action back, and the signal again: ax ends as without the handler.
    unsafe {
        libc::signal(sig, libc::SIG_DFL);
        libc::raise(sig);
    }
}

/// cargo `child`, the leader of its own group, in [`GROUPS`] while it runs.
#[cfg(unix)]
struct Group(usize);

#[cfg(unix)]
impl Group {
    fn of(child: &std::process::Child) -> Option<Self> {
        install();
        let pid = child.id() as i32;
        GROUPS.iter().position(|g| g.compare_exchange(0, pid, Relaxed, Relaxed).is_ok()).map(Group)
    }
}

#[cfg(unix)]
impl Drop for Group {
    fn drop(&mut self) {
        GROUPS[self.0].store(0, Relaxed);
    }
}

/// What starts when a signal ends ax while [`arm`]ed: `program args` in
/// `cwd`, apart from the terminal (Unix: its own session; Windows: no
/// console), its words in `log`.
pub struct OnExit {
    pub program: std::path::PathBuf,
    pub args: Vec<String>,
    pub cwd: std::path::PathBuf,
    pub log: std::path::PathBuf,
}

/// [`OnExit`] as the signal handler starts it: C strings made beforehand
/// (between fork and exec only async-signal-safe calls).
struct Prepared {
    #[cfg(unix)]
    argv: Vec<std::ffi::CString>,
    #[cfg(unix)]
    ptrs: Vec<*const libc::c_char>,
    #[cfg(unix)]
    cwd: std::ffi::CString,
    #[cfg(unix)]
    log: std::ffi::CString,
    #[cfg(windows)]
    run: OnExit,
}

// SAFETY: `ptrs` point into `argv`, owned alongside and never changed.
unsafe impl Sync for Prepared {}
unsafe impl Send for Prepared {}

static ON_EXIT: std::sync::OnceLock<Prepared> = std::sync::OnceLock::new();
static ARMED: AtomicBool = AtomicBool::new(false);

/// From now, a signal that ends ax (Ctrl-C, SIGTERM, the terminal or
/// console closing) first starts `run` (the first one armed this process;
/// a later `arm` re-arms it). [`disarm`]: no more.
pub fn arm(run: OnExit) {
    if let Some(d) = run.log.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    _ = ON_EXIT.get_or_init(|| prepare(run));
    install();
    ARMED.store(true, Relaxed);
}

pub fn disarm() {
    ARMED.store(false, Relaxed);
}

#[cfg(unix)]
fn prepare(run: OnExit) -> Prepared {
    use std::os::unix::ffi::OsStrExt;
    let c = |b: &[u8]| std::ffi::CString::new(b.to_vec()).unwrap_or_default();
    let mut argv = vec![c(run.program.as_os_str().as_bytes())];
    argv.extend(run.args.iter().map(|a| c(a.as_bytes())));
    let mut ptrs: Vec<*const libc::c_char> = argv.iter().map(|a| a.as_ptr()).collect();
    ptrs.push(std::ptr::null());
    Prepared { argv, ptrs, cwd: c(run.cwd.as_os_str().as_bytes()), log: c(run.log.as_os_str().as_bytes()) }
}

#[cfg(windows)]
fn prepare(run: OnExit) -> Prepared {
    Prepared { run }
}

/// The armed [`OnExit`] started, in a signal handler: fork, then only
/// async-signal-safe calls until exec.
#[cfg(unix)]
fn start_on_exit() {
    let Some(p) = ON_EXIT.get().filter(|_| ARMED.load(Relaxed)) else { return };
    // SAFETY: fork, then in the child setsid, chdir, open, dup2, execv,
    // _exit: all async-signal-safe; the strings were made before.
    unsafe {
        if libc::fork() == 0 {
            libc::setsid();
            libc::chdir(p.cwd.as_ptr());
            let null = libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY);
            if null >= 0 {
                libc::dup2(null, 0);
            }
            let log = libc::open(p.log.as_ptr(), libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC, 0o644);
            if log >= 0 {
                libc::dup2(log, 1);
                libc::dup2(log, 2);
            }
            libc::execv(p.argv[0].as_ptr(), p.ptrs.as_ptr());
            libc::_exit(127);
        }
    }
}

#[cfg(windows)]
fn start_on_exit() {
    let Some(p) = ON_EXIT.get().filter(|_| ARMED.load(Relaxed)) else { return };
    let Ok(out) = std::fs::File::create(&p.run.log) else { return };
    let Ok(err) = out.try_clone() else { return };
    let mut cmd = Command::new(&p.run.program);
    cmd.args(&p.run.args).current_dir(&p.run.cwd).stdin(Stdio::null()).stdout(out).stderr(err);
    // CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS: the console closing leaves it.
    std::os::windows::process::CommandExt::creation_flags(&mut cmd, 0x200 | 0x8);
    _ = cmd.spawn();
}

/// Windows: a console's Ctrl-C, Ctrl-Break or closing: the armed
/// [`OnExit`] started, then the default (ax ends).
#[cfg(windows)]
extern "system" fn console_ctrl(_kind: u32) -> i32 {
    start_on_exit();
    0
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetConsoleCtrlHandler(handler: extern "system" fn(u32) -> i32, add: i32) -> i32;
}

/// The handlers, once: Unix, SIGINT, SIGTERM, SIGHUP to [`forward`];
/// Windows, the console's events to [`console_ctrl`].
fn install() {
    static HANDLER: std::sync::Once = std::sync::Once::new();
    HANDLER.call_once(|| {
        #[cfg(unix)]
        for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            // SAFETY: `forward` makes only async-signal-safe calls; a
            // signal ax was started ignoring (`nohup`) stays ignored.
            unsafe {
                if libc::signal(sig, forward as extern "C" fn(libc::c_int) as libc::sighandler_t) == libc::SIG_IGN {
                    libc::signal(sig, libc::SIG_IGN);
                }
            }
        }
        // SAFETY: a handler of the documented signature, for the process' life.
        #[cfg(windows)]
        unsafe {
            SetConsoleCtrlHandler(console_ctrl, 1);
        }
    });
}

/// Windows: cargo and every process it starts, in a job object of their
/// own: ended together by [`stop`], and when ax ends (the job closes with
/// its last handle, ax's). A Ctrl-C at the console reaches them as it does
/// ax: they share it.
#[cfg(windows)]
struct Group(*mut std::ffi::c_void);

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateJobObjectW(attributes: *mut std::ffi::c_void, name: *const u16) -> *mut std::ffi::c_void;
    fn SetInformationJobObject(job: *mut std::ffi::c_void, class: i32, info: *const std::ffi::c_void, len: u32) -> i32;
    fn AssignProcessToJobObject(job: *mut std::ffi::c_void, process: *mut std::ffi::c_void) -> i32;
    fn TerminateJobObject(job: *mut std::ffi::c_void, code: u32) -> i32;
    fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
}

/// JOBOBJECT_EXTENDED_LIMIT_INFORMATION (its basic limits, IO counters, memory).
#[cfg(windows)]
#[repr(C)]
#[derive(Default)]
struct JobLimits {
    per_process_time: i64,
    per_job_time: i64,
    flags: u32,
    min_working_set: usize,
    max_working_set: usize,
    active_processes: u32,
    affinity: usize,
    priority_class: u32,
    scheduling_class: u32,
    io: [u64; 6],
    process_memory: usize,
    job_memory: usize,
    peak_process_memory: usize,
    peak_job_memory: usize,
}

#[cfg(windows)]
impl Group {
    fn of(child: &std::process::Child) -> Option<Self> {
        use std::os::windows::io::AsRawHandle;
        // SAFETY: a new unnamed job, its limits set from a struct of the
        // documented layout, our own child put in it; closed on drop.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
            if job.is_null() {
                return None;
            }
            let group = Group(job);
            // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JobObjectExtendedLimitInformation.
            let limits = JobLimits { flags: 0x2000, ..Default::default() };
            let size = std::mem::size_of::<JobLimits>() as u32;
            let set = SetInformationJobObject(job, 9, (&raw const limits).cast(), size) != 0;
            (set && AssignProcessToJobObject(job, child.as_raw_handle().cast()) != 0).then_some(group)
        }
    }

    fn end(&self) {
        // SAFETY: our job's handle, open until drop.
        unsafe { TerminateJobObject(self.0, 1) };
    }
}

#[cfg(windows)]
impl Drop for Group {
    fn drop(&mut self) {
        // SAFETY: our job's handle, closed once.
        unsafe { CloseHandle(self.0) };
    }
}

/// Ask cargo to stop, its rustc calls too: as Ctrl-C does, to its whole
/// process group (Unix); its job ended (Windows).
fn stop(child: &mut std::process::Child, _group: Option<&Group>) {
    #[cfg(unix)]
    // SAFETY: a signal to the group of our own child (its leader).
    unsafe {
        libc::killpg(child.id() as i32, libc::SIGINT);
    }
    #[cfg(windows)]
    match _group {
        Some(g) => g.end(),
        None => _ = child.kill(),
    }
}

/// Who is told cargo's count as a build runs (ax dev's splash): done, of,
/// on what.
pub static WATCHED: Mutex<Option<Watcher>> = Mutex::new(None);

/// Told cargo's count: done, of, on what.
pub type Watcher = fn(u32, u32, &str);

/// cargo's progress bar text `bar` as (done, of, on what) (pure): `Building
/// [====>   ] 12/93: syn, tokio`.
pub fn parse_progress(bar: &str) -> Option<(u32, u32, String)> {
    let at = bar.rfind("Building [")?;
    let rest = &bar[at..];
    let rest = &rest[rest.find("] ")? + 2..];
    let (count, names) = rest.split_once(':').unwrap_or((rest, ""));
    let (n, m) = count.trim().split_once('/')?;
    Some((n.trim().parse().ok()?, m.trim().parse().ok()?, names.trim().to_string()))
}

/// The warnings shown in full this session, by their text.
static SEEN: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// cargo's progress bar and colours when stderr is a terminal, by
/// environment (no flag: a fingerprint never changes for it).
pub fn on_terminal(cmd: &mut Command) {
    if !std::io::stderr().is_terminal() {
        return;
    }
    let width = std::env::var("COLUMNS").ok().and_then(|c| c.parse::<u16>().ok()).unwrap_or(100).max(40);
    cmd.env("CARGO_TERM_PROGRESS_WHEN", "always").env("CARGO_TERM_PROGRESS_WIDTH", width.to_string());
    if std::env::var_os("CARGO_TERM_COLOR").is_none() && std::env::var_os("NO_COLOR").is_none() {
        cmd.env("CARGO_TERM_COLOR", "always");
    }
}

/// Run cargo `cmd`: its stderr to ours as it comes (warnings collapsed),
/// its stdout taken when `take_stdout` (else ours); stopped once `stop`
/// says the build is stale. cargo runs in its own process group, so a stop
/// reaches its rustc calls too.
pub fn run(cmd: &mut Command, take_stdout: bool, stop_on: &Stop) -> std::io::Result<Ran> {
    on_terminal(cmd);
    // ax dev's splash shows cargo's count: its bar asked for, kept off a
    // stderr that isn't a terminal.
    let splash = WATCHED.lock().unwrap_or_else(|e| e.into_inner()).is_some();
    let hide_bars = splash && !std::io::stderr().is_terminal();
    if hide_bars {
        cmd.env("CARGO_TERM_PROGRESS_WHEN", "always").env("CARGO_TERM_PROGRESS_WIDTH", "160");
    }
    cmd.stderr(Stdio::piped());
    cmd.stdout(if take_stdout { Stdio::piped() } else { Stdio::inherit() });
    cmd.stdin(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
        // SAFETY: only signal(), async-signal-safe, between fork and exec:
        // an ax started with them ignored (nohup, `ax dev &`) passes that
        // on, and its stop would never reach cargo.
        unsafe {
            cmd.pre_exec(|| {
                libc::signal(libc::SIGINT, libc::SIG_DFL);
                libc::signal(libc::SIGTERM, libc::SIG_DFL);
                Ok(())
            });
        }
    }
    stop_on.0.store(false, Relaxed);
    let mut child = cmd.spawn()?;
    let group = Group::of(&child);
    let stdout = child.stdout.take().map(|mut o| {
        std::thread::spawn(move || {
            let mut s = String::new();
            let _ = o.read_to_string(&mut s);
            s
        })
    });
    let mut feed = Feed { hide_bars, ..Feed::default() };
    // eprint! (not stderr's writer): a test's output stays the test's.
    let say = |b: &[u8]| {
        if !b.is_empty() {
            eprint!("{}", String::from_utf8_lossy(b));
            let _ = std::io::stderr().flush();
        }
    };
    // Read on its own thread: this one can stop a stale build between reads.
    let (tx, chunks) = std::sync::mpsc::channel::<Vec<u8>>();
    if let Some(mut pipe) = child.stderr.take() {
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = pipe.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
    }
    let mut stopped = false;
    loop {
        match chunks.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(chunk) => {
                say(&feed.bytes(&chunk, &mut seen));
                let watcher = *WATCHED.lock().unwrap_or_else(|e| e.into_inner());
                if let (Some((n, m, names)), Some(f)) = (feed.progress.take(), watcher) {
                    f(n, m, &names);
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if !stopped && stop_on.0.swap(false, Relaxed) {
            stopped = true;
            stop(&mut child, group.as_ref());
        }
    }
    say(&feed.end());
    let status = child.wait()?;
    let stdout = stdout.and_then(|t| t.join().ok()).unwrap_or_default();
    Ok(Ran { status, stdout, said: feed.said, stopped })
}

fn seen(key: &str) -> bool {
    let mut s = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    !s.get_or_insert_with(HashSet::new).insert(key.to_string())
}

/// The stream as it comes (pure but for `seen`): what to write now, and the
/// plain text kept.
#[derive(Default)]
pub struct Feed {
    pending: Vec<u8>,
    /// A warning being read: its lines as they came, its plain lines.
    block: Option<(Vec<u8>, Vec<String>)>,
    pub said: String,
    /// Progress bars are read ([`Self::progress`]) but not passed on (a
    /// stderr that isn't a terminal, the bar asked for ax dev's splash).
    pub hide_bars: bool,
    /// The last progress bar read: done, of, on what.
    pub progress: Option<(u32, u32, String)>,
}

impl Feed {
    /// The next `chunk`; `seen(text)`: was this warning shown in full before
    /// (and remember it).
    pub fn bytes(&mut self, chunk: &[u8], seen: &mut dyn FnMut(&str) -> bool) -> Vec<u8> {
        self.pending.extend_from_slice(chunk);
        let mut out = vec![];
        while let Some(i) = self.pending.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=i).collect();
            self.line(line, seen, &mut out);
        }
        // A progress bar is drawn over itself (`\r`), never ended: to the
        // terminal now, never kept.
        if let Some(r) = self.pending.iter().rposition(|&b| b == b'\r')
            && self.block.is_none()
        {
            let bar: Vec<u8> = self.pending.drain(..=r).collect();
            self.read_bar(&bar);
            if !self.hide_bars {
                out.extend(bar);
            }
        }
        out
    }

    /// The last progress bar in `bytes`, kept in [`Self::progress`].
    fn read_bar(&mut self, bytes: &[u8]) {
        let text = String::from_utf8_lossy(bytes);
        if let Some(p) = text.split('\r').rev().find_map(|s| parse_progress(&plain(s.as_bytes()))) {
            self.progress = Some(p);
        }
    }

    /// The stream ended: whatever is left, as it was.
    pub fn end(&mut self) -> Vec<u8> {
        let mut out = vec![];
        if let Some((raw, plain)) = self.block.take() {
            out.extend(raw);
            self.keep(&plain);
        }
        if !self.pending.is_empty() {
            let rest = std::mem::take(&mut self.pending);
            self.keep(&[plain(&rest)]);
            out.extend(rest);
        }
        out
    }

    fn keep(&mut self, lines: &[String]) {
        for l in lines {
            self.said.push_str(l);
            self.said.push('\n');
        }
    }

    fn line(&mut self, raw: Vec<u8>, seen: &mut dyn FnMut(&str) -> bool, out: &mut Vec<u8>) {
        // A bar drawn before the line: read, and left out where bars are.
        let raw = match raw.iter().rposition(|&b| b == b'\r').filter(|&r| r + 2 < raw.len()) {
            Some(r) => {
                self.read_bar(&raw[..=r]);
                if self.hide_bars { raw[r + 1..].to_vec() } else { raw }
            }
            None => raw,
        };
        let text = plain(&raw);
        if let Some((mut braw, mut bplain)) = self.block.take() {
            // A rustc warning is its title, ` --> ` where, and ends at a blank line.
            if bplain.len() == 1 && !text.trim_start().starts_with("-->") {
                out.extend(braw);
                self.keep(&bplain);
                return self.line(raw, seen, out);
            }
            braw.extend(&raw);
            bplain.push(text.clone());
            if !text.trim().is_empty() {
                self.block = Some((braw, bplain));
                return;
            }
            self.keep(&bplain);
            let key = bplain.join("\n");
            if seen(&key) {
                let at = bplain.iter().find_map(|l| l.trim_start().strip_prefix("--> ")).unwrap_or("").trim();
                out.extend(format!("{} ({at}; as before)\n", bplain[0].trim_end()).as_bytes());
            } else {
                out.extend(braw);
            }
            return;
        }
        if is_warning(&text) {
            self.block = Some((raw, vec![text]));
            return;
        }
        self.keep(&[text]);
        out.extend(raw);
    }
}

/// A rustc warning's first line (not cargo's count of them) (pure).
pub fn is_warning(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("warning: ") else { return false };
    !(rest.starts_with('`') && (rest.contains(" generated ") || rest.contains(" emitted")))
}

/// A line as plain text: no colour, only what is left after a `\r` (pure).
pub fn plain(raw: &[u8]) -> String {
    let s = String::from_utf8_lossy(raw);
    let s = s.trim_end_matches(['\n', '\r']);
    let s = s.rsplit('\r').next().unwrap_or("");
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for d in chars.by_ref() {
                    if d.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Not a test alone: run by the next ones in a process of its own, to
    /// arm (or arm then disarm, `AX_EXIT_PROBE_DISARM`) a program writing
    /// the file `AX_EXIT_PROBE` names, then end by SIGTERM.
    #[cfg(unix)]
    #[test]
    fn exit_probe() {
        let Some(to) = std::env::var_os("AX_EXIT_PROBE") else { return };
        let to = std::path::PathBuf::from(to);
        let dir = to.parent().unwrap().to_path_buf();
        let args = vec!["-c".to_string(), format!("echo warm > '{}'", to.display())];
        arm(OnExit { program: "/bin/sh".into(), args, cwd: dir.clone(), log: dir.join("exit.log") });
        if std::env::var_os("AX_EXIT_PROBE_DISARM").is_some() {
            disarm();
        }
        // SAFETY: a signal to this process, to end it as a terminal would.
        unsafe { libc::raise(libc::SIGTERM) };
    }

    #[cfg(unix)]
    #[test]
    fn a_signal_that_ends_ax_starts_what_is_armed_first() {
        use std::os::unix::process::ExitStatusExt;
        let d = tempfile::tempdir().unwrap();
        let run = |disarm: bool| {
            let to = d.path().join(if disarm { "disarmed" } else { "armed" });
            let mut cmd = Command::new(std::env::current_exe().unwrap());
            cmd.args([
                "--exact",
                "--nocapture",
                &format!("{}::exit_probe", module_path!().split_once("::").unwrap().1),
            ]);
            cmd.env("AX_EXIT_PROBE", &to).stdout(Stdio::null()).stderr(Stdio::null());
            if disarm {
                cmd.env("AX_EXIT_PROBE_DISARM", "1");
            }
            let status = cmd.status().unwrap();
            assert_eq!(status.signal(), Some(libc::SIGTERM), "ended by the signal");
            let t = std::time::Instant::now();
            let wait = std::time::Duration::from_secs(if disarm { 1 } else { 5 });
            while !to.is_file() && t.elapsed() < wait {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            to.is_file()
        };
        assert!(run(false), "armed: started as ax ended");
        assert!(!run(true), "disarmed: nothing started");
    }

    #[cfg(windows)]
    #[test]
    fn a_console_event_that_ends_ax_starts_what_is_armed_first() {
        let d = tempfile::tempdir().unwrap();
        let to = d.path().join("armed");
        let args = vec!["/c".to_string(), format!("echo warm> {}", to.display())];
        arm(OnExit { program: "cmd".into(), args, cwd: d.path().into(), log: d.path().join("exit.log") });
        // The handler as Windows calls it on Ctrl-C (CTRL_C_EVENT): it
        // starts what is armed and leaves the ending to the default.
        assert_eq!(console_ctrl(0), 0);
        let t = std::time::Instant::now();
        while !to.is_file() && t.elapsed() < std::time::Duration::from_secs(10) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        disarm();
        assert!(to.is_file(), "started");
    }

    const WARN: &str = "warning: unused import: `x`\n --> src/a.rs:1:5\n  |\n1 | use x;\n  |     ^\n\n";

    fn fed(feed: &mut Feed, text: &str, seen: &mut HashSet<String>) -> String {
        let out = feed.bytes(text.as_bytes(), &mut |k| !seen.insert(k.to_string()));
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn a_warning_said_before_is_one_line_the_first_time_whole() {
        let mut seen = HashSet::new();
        let first = fed(&mut Feed::default(), &format!("   Compiling a\n{WARN}"), &mut seen);
        assert_eq!(first, format!("   Compiling a\n{WARN}"));
        let mut again = Feed::default();
        let out =
            fed(&mut again, &format!("   Compiling a\n{WARN}warning: `a` (lib) generated 1 warning\n"), &mut seen);
        assert_eq!(
            out,
            "   Compiling a\nwarning: unused import: `x` (src/a.rs:1:5; as before)\nwarning: `a` (lib) generated 1 warning\n"
        );
        // Kept whole, for the log and the page.
        assert!(again.said.contains("1 | use x;"), "{}", again.said);
    }

    #[test]
    fn an_error_and_a_one_line_warning_pass_as_they_are() {
        let mut seen = HashSet::new();
        let text = "warning: build failed, waiting for other jobs to finish...\nerror[E0425]: x\n --> src/a.rs:1:1\n\n";
        let mut f = Feed::default();
        let mut out = fed(&mut f, text, &mut seen);
        out.push_str(&String::from_utf8(f.end()).unwrap());
        assert_eq!(out, text);
        let mut f = Feed::default();
        assert_eq!(fed(&mut f, text, &mut seen), text, "errors are never collapsed");
    }

    #[test]
    fn progress_goes_to_the_terminal_at_once_and_is_never_kept() {
        let mut seen = HashSet::new();
        let mut f = Feed::default();
        let bar = "\r    Building [==>   ] 3/10: a\r";
        assert_eq!(fed(&mut f, bar, &mut seen), bar);
        let out = fed(&mut f, "\x1b[K\x1b[1m\x1b[32m   Compiling\x1b[0m b\n", &mut seen);
        assert_eq!(out, "\x1b[K\x1b[1m\x1b[32m   Compiling\x1b[0m b\n");
        assert_eq!(f.said, "   Compiling b\n");
    }

    #[test]
    fn a_bar_for_the_splash_is_read_and_kept_off_a_plain_stderr() {
        let mut seen = HashSet::new();
        let mut f = Feed { hide_bars: true, ..Feed::default() };
        assert_eq!(fed(&mut f, "\r    Building [==>   ] 3/10: a, b\r", &mut seen), "");
        assert_eq!(f.progress.take(), Some((3, 10, "a, b".into())));
        let out = fed(&mut f, "\r    Building [===>  ] 4/10: b\r\x1b[K   Compiling c\n", &mut seen);
        assert_eq!(out, "\x1b[K   Compiling c\n");
        assert_eq!(f.progress, Some((4, 10, "b".into())));
        assert_eq!(f.said, "   Compiling c\n");
    }

    #[cfg(unix)]
    #[test]
    fn a_save_while_it_builds_stops_the_build_and_everything_it_started() {
        let d = tempfile::tempdir().unwrap();
        let pid_file = d.path().join("pid");
        let stop = Stop::default();
        let s = stop.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(500));
            s.stale();
        });
        let started = std::time::Instant::now();
        // cargo's rustc calls stand in: a child of the child.
        let script = format!("sh -c 'echo $$ > {}; exec sleep 30'; true", pid_file.display());
        let ran = run(Command::new("sh").args(["-c", &script]), true, &stop).unwrap();
        assert!(ran.stopped && !ran.status.success());
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
        let pid: i32 = std::fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();
        let gone = (0..40).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(50));
            // SAFETY: signal 0 only asks whether the pid is there.
            unsafe { libc::kill(pid, 0) != 0 }
        });
        assert!(gone, "the grandchild {pid} outlived the stop");
        // With nothing running, a save stops nothing later; another session's never.
        stop.stale();
        let ran = run(Command::new("true").arg(""), true, &stop).unwrap();
        assert!(!ran.stopped && ran.status.success());
        Stop::default().stale();
        assert!(!run(Command::new("true").arg(""), true, &stop).unwrap().stopped);
    }

    /// The test above in a process of its own that ignores SIGINT, as an ax
    /// started by nohup or as a background job does: the stop still lands.
    #[cfg(unix)]
    #[test]
    fn a_save_stops_the_build_when_ax_ignores_ctrl_c() {
        use std::os::unix::process::CommandExt;
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args([
            "--exact",
            &format!(
                "{}::a_save_while_it_builds_stops_the_build_and_everything_it_started",
                module_path!().split_once("::").unwrap().1
            ),
        ]);
        // SAFETY: only signal(), async-signal-safe, between fork and exec.
        unsafe {
            cmd.pre_exec(|| {
                libc::signal(libc::SIGINT, libc::SIG_IGN);
                Ok(())
            });
        }
        let out = cmd.output().unwrap();
        let said = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success() && said.contains("1 passed"), "{said}{}", String::from_utf8_lossy(&out.stderr));
    }

    #[cfg(windows)]
    #[test]
    fn a_save_while_it_builds_ends_the_build_and_everything_it_started() {
        let d = tempfile::tempdir().unwrap();
        let pid_file = d.path().join("pid");
        let stop = Stop::default();
        let s = stop.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            s.stale();
        });
        let started = std::time::Instant::now();
        // cargo's rustc calls stand in: a child of the child, its pid written down.
        let script = format!(
            "$p = Start-Process -PassThru -NoNewWindow ping -ArgumentList '-n','60','127.0.0.1'; \
             $p.Id | Out-File -Encoding ascii '{}'; Wait-Process -Id $p.Id",
            pid_file.display()
        );
        let mut cmd = Command::new("powershell");
        cmd.args(["-NoProfile", "-Command", &script]);
        let ran = run(&mut cmd, true, &stop).unwrap();
        assert!(ran.stopped && !ran.status.success());
        assert!(started.elapsed() < std::time::Duration::from_secs(20), "{:?}", started.elapsed());
        let pid = std::fs::read_to_string(&pid_file).unwrap().trim().to_string();
        let alive = || {
            let out = Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/NH"]).output().unwrap();
            String::from_utf8_lossy(&out.stdout).contains(&pid)
        };
        let gone = (0..40).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(100));
            !alive()
        });
        assert!(gone, "the grandchild {pid} outlived the stop");
    }

    #[test]
    fn a_warning_split_across_reads_is_read_whole() {
        let mut seen = HashSet::new();
        let mut f = Feed::default();
        let (a, b) = WARN.split_at(20);
        let mut out = fed(&mut f, a, &mut seen);
        out += &fed(&mut f, b, &mut seen);
        assert_eq!(out, WARN, "a new one in full");
        assert!(!is_warning("warning: `a` (lib) generated 2 warnings"));
        assert!(!is_warning("warning: `a` (lib) generated 1 warning (run `cargo fix`)"));
        assert!(is_warning("warning: unused variable: `x`"));
    }
}

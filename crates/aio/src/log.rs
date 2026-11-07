//! The app's log: everything written to stderr (the app's own lines, panics,
//! the runtime child's output) also lands in `logs/app.log`, each line
//! stamped with UTC time; errors also in `error.log`, warnings in
//! `warning.log`. Files rotate at `[log] file_mb` and `keep` are kept, so
//! logs stay within their budget. Credentials are masked (`report::redact`).
//! Every `[log] heartbeat` seconds a line says uptime, calls and errors; at
//! `debug`, each call is a line (never its arguments).
//!
//! Structured lines: `aio::info!("saved")`, `aio::warn!(cat: "net", peer = id,
//! "lost {n} frames")` write `WARN  [net] lost 3 frames peer=a1` to stderr,
//! so into the same log. Without `cat:` the category is the caller's tag:
//! `aio` for the framework, `app` for everything else. A line below the
//! level (`[log] level` in `aio.toml`, [`set_level`]; `info` by default)
//! costs one load; nothing is formatted.
//!
//! The same line many times in a row is written once, then `… last line
//! repeated N more times`. On a terminal (and without `NO_COLOR`) the level
//! word is coloured; the file stays plain. When the log closes, its last
//! line counts the run's errors and warnings.
use std::fmt::{Display, Write as _};
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const KEEP: usize = 5;

/// `2026-10-27T23:01:02.123Z` for ms since the epoch.
pub fn stamp(ms: i64) -> String {
    let (secs, milli) = (ms.div_euclid(1000), ms.rem_euclid(1000));
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{milli:03}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

pub use aio_config::log::Level;
static GATE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(Level::Info as u8);

/// Lines below `level` are dropped, wherever they would go.
pub fn set_level(level: Level) {
    GATE.store(level as u8, std::sync::atomic::Ordering::Relaxed);
}

/// Would a line at `level` be written?
pub fn enabled(level: Level) -> bool {
    level as u8 >= GATE.load(std::sync::atomic::Ordering::Relaxed)
}

/// An untagged line from the framework says `aio`, any other `app`.
pub fn tag(module: &str) -> &'static str {
    if module == "aio" || module.starts_with("aio::") { "aio" } else { "app" }
}

/// One entry as exactly one line (pure): `INFO  [cat] msg k=v k=v`. Every
/// part is [`safe`]: no caller's text can start a second, forged entry.
pub fn render(level: Level, cat: &str, msg: &str, data: &[(&str, &dyn Display)]) -> String {
    let word = ["TRACE", "DEBUG", "INFO", "WARN", "ERROR"][level as usize];
    let mut l = format!("{word:<5} [{}] {}", safe(cat), safe(msg));
    for (k, v) in data {
        _ = write!(l, " {}={}", safe(k), safe(&text(v)));
    }
    l
}

/// What the macros call: one rendered line to the sink (a capture on this
/// thread, else the one [`set_sink`] set), else stderr. Unlike `eprintln!`,
/// a failed write never panics the caller, and neither does a sink.
#[doc(hidden)]
pub fn emit(level: Level, module: &str, cat: Option<&str>, msg: std::fmt::Arguments, data: &[(&str, &dyn Display)]) {
    // A line said during a call carries its id ([`call_id`]).
    let id = call_id();
    let with;
    let data = match id {
        0 => data,
        _ => {
            with = [data, &[("call", &id as &dyn Display)]].concat();
            &with[..]
        }
    };
    let line = render(level, cat.unwrap_or(tag(module)), &text(&msg), data);
    if CAPTURE.with_borrow_mut(|c| c.as_mut().map(|c| c.push(line.clone()))).is_some() {
        return;
    }
    let sink = SINK.read().unwrap_or_else(|e| e.into_inner()).clone();
    match sink {
        Some(s) => {
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s(level, &line))).is_err() {
                _ = writeln!(io::stderr(), "{line}");
            }
        }
        None => _ = writeln!(io::stderr(), "{line}"),
    }
}

/// Where structured lines go instead of stderr (and so the log files): the
/// level and the rendered line.
pub type Sink = std::sync::Arc<dyn Fn(Level, &str) + Send + Sync>;

static SINK: std::sync::RwLock<Option<Sink>> = std::sync::RwLock::new(None);

/// Send the macros' lines to `sink` (`None`: stderr again); the one it
/// replaces. A sink that panics loses no line: it goes to stderr.
pub fn set_sink(sink: Option<Sink>) -> Option<Sink> {
    std::mem::replace(&mut *SINK.write().unwrap_or_else(|e| e.into_inner()), sink)
}

/// The sink lines go to now, if any.
pub fn sink() -> Option<Sink> {
    SINK.read().unwrap_or_else(|e| e.into_inner()).clone()
}

thread_local!(static CAPTURE: std::cell::RefCell<Option<Vec<String>>> = const { std::cell::RefCell::new(None) });

/// Run `f` with this thread's lines kept instead of written: what it
/// logged, for a test to look at (other threads, other tests: untouched).
pub fn capture<R>(f: impl FnOnce() -> R) -> (R, Vec<String>) {
    struct Back(Option<Vec<String>>);
    impl Drop for Back {
        fn drop(&mut self) {
            CAPTURE.set(self.0.take());
        }
    }
    let back = Back(CAPTURE.replace(Some(vec![])));
    let r = f();
    let lines = CAPTURE.replace(None).unwrap_or_default();
    drop(back);
    (r, lines)
}

/// A value as text: a `Display` that fails keeps what it wrote, never panics.
fn text(v: &dyn Display) -> String {
    let mut s = String::new();
    _ = write!(s, "{v}");
    s
}

/// The most of one text a line holds.
const MAX_TEXT: usize = 8192;

/// Untrusted text made safe for a terminal and a log (pure): capped, line
/// breaks as `\n`, every other control character (C0 but tab, DEL, C1) as
/// `\xNN`. Colour (SGR, `ESC[…m`) stays: it cannot move the cursor. Always
/// one line: the log stamps each line, so a continuation would read as an
/// entry of its own.
pub fn safe(s: &str) -> String {
    let over = s.chars().count().saturating_sub(MAX_TEXT);
    let s = s.char_indices().nth(MAX_TEXT).map_or(s, |(i, _)| &s[..i]);
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        let n = sgr_len(rest);
        if n > 0 {
            out.push_str(&rest[..n]);
            rest = &rest[n..];
            continue;
        }
        match c {
            '\r' if rest[1..].starts_with('\n') => {}
            '\n' | '\r' => out.push_str("\\n"),
            '\t' => out.push(c),
            c if c.is_control() => _ = write!(out, "\\x{:02x}", c as u32),
            c => out.push(c),
        }
        rest = &rest[c.len_utf8()..];
    }
    if over > 0 {
        _ = write!(out, " … [truncated {over} chars]");
    }
    out
}

/// The length of the SGR sequence `s` starts with, or 0.
fn sgr_len(s: &str) -> usize {
    let Some(p) = s.strip_prefix("\x1b[") else { return 0 };
    let n = p.bytes().take_while(|b| b.is_ascii_digit() || *b == b';').count();
    if n <= 32 && p.as_bytes().get(n) == Some(&b'm') { n + 3 } else { 0 }
}

/// A size-capped, rotating log file: `app.log`, then `app.1.log` … `app.{KEEP-1}.log`.
pub struct Rotating {
    dir: PathBuf,
    stem: &'static str,
    file: File,
    size: u64,
    max: u64,
    keep: usize,
}

impl Rotating {
    pub fn open(dir: &Path, max: u64) -> io::Result<Self> {
        Self::named(dir, "app", max)
    }

    /// `<stem>.log`, rotated the same way (`perf.log`).
    pub fn named(dir: &Path, stem: &'static str, max: u64) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(dir.join(format!("{stem}.log")))?;
        let size = file.metadata()?.len();
        Ok(Self { dir: dir.into(), stem, file, size, max, keep: KEEP })
    }

    /// A run of its own: what an earlier run wrote moves to `<stem>.1.log`.
    pub fn fresh(mut self) -> io::Result<Self> {
        if self.size > 0 {
            self.rotate()?;
        }
        Ok(self)
    }

    /// Keep `keep` files (at least the live one) instead of `KEEP`.
    pub fn keeping(self, keep: usize) -> Self {
        Self { keep: keep.max(1), ..self }
    }

    fn name(&self, i: usize) -> PathBuf {
        let stem = self.stem;
        self.dir.join(if i == 0 { format!("{stem}.log") } else { format!("{stem}.{i}.log") })
    }

    fn rotate(&mut self) -> io::Result<()> {
        _ = std::fs::remove_file(self.name(self.keep - 1));
        for i in (0..self.keep - 1).rev() {
            _ = std::fs::rename(self.name(i), self.name(i + 1));
        }
        self.file = OpenOptions::new().create(true).append(true).open(self.name(0))?;
        self.size = 0;
        Ok(())
    }

    /// Write one line, stamped.
    pub fn line(&mut self, at: i64, text: &str) -> io::Result<()> {
        let l = format!("{} {}\n", stamp(at), text.trim_end_matches(['\n', '\r']));
        if self.size > 0 && self.size + l.len() as u64 > self.max {
            self.rotate()?;
        }
        self.file.write_all(l.as_bytes())?;
        self.size += l.len() as u64;
        Ok(())
    }
}

/// Copy everything written to stderr (fd 2, so child processes too) into
/// `logs/app.log`, and still to the terminal. Call once, early in `main`.
#[cfg(unix)]
pub fn start(logs: &Path) -> io::Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let log = Sinks::open(logs)?;
    let mut fds = [0; 2];
    // SAFETY: plain POSIX calls on fds this function owns; fd 2 is replaced
    // by the pipe only after a copy of the old one is taken.
    let term = unsafe {
        if libc::pipe(fds.as_mut_ptr()) != 0 {
            return Err(io::Error::last_os_error());
        }
        let term = libc::dup(2);
        if term < 0 || libc::dup2(fds[1], 2) < 0 {
            return Err(io::Error::last_os_error());
        }
        libc::close(fds[1]);
        File::from_raw_fd(term)
    };
    let pipe = unsafe { File::from_raw_fd(fds[0]) };
    // A second copy of the terminal, to give fd 2 back at `finish`.
    let back = unsafe { libc::dup(term.as_raw_fd()) };
    let paint = colour_for(&term);
    let copier =
        std::thread::Builder::new().name("aio-log".into()).spawn(move || copy(pipe, Some(term), paint, log))?;
    *tee() = Some((copier, back));
    finish_at_exit();
    heartbeat();
    Ok(())
}

/// What `finish` needs to hand stderr back: a copy of the terminal (Unix:
/// its fd; Windows: its handle and the pipe's write end, closed with it).
#[cfg(unix)]
type Back = i32;
#[cfg(windows)]
type Back = (usize, std::os::windows::io::OwnedHandle);

fn tee() -> std::sync::MutexGuard<'static, Option<(std::thread::JoinHandle<()>, Back)>> {
    static T: std::sync::Mutex<Option<(std::thread::JoinHandle<()>, Back)>> = std::sync::Mutex::new(None);
    T.lock().unwrap_or_else(|e| e.into_inner())
}

/// The copier: each line to the terminal (when there is one, its level
/// word coloured when `paint`) and the log; repeats collapsed, and the run's
/// errors and warnings counted on the last line.
fn copy(pipe: File, mut term: Option<impl Write>, paint: bool, mut log: Sinks) {
    let (mut repeats, mut tally) = (Repeats::default(), Tally::default());
    ERRORS.store(0, Relaxed);
    let mut brief = BriefBoot { on: std::env::var("AIORT_BOOT").is_ok_and(|v| v == "brief"), rows: vec![] };
    let mut out = |line: &str| {
        if let Some(t) = term.as_mut() {
            for l in brief.next(line) {
                _ = writeln!(t, "{}", if paint { painted(&l) } else { l.as_str().into() });
            }
        }
        #[cfg(target_os = "android")]
        logcat(line);
        _ = log.line(now_ms(), line);
    };
    for line in io::BufReader::new(pipe).lines() {
        let Ok(line) = line else { break };
        // No credential reaches a log: a token in a URL, a pairing code.
        let line = crate::report::redact(&line);
        let line = runtime_noise(&line).unwrap_or(line);
        // The window's render probe, into vitals (off this thread: what it
        // says goes to stderr, which this thread drains); ten a second at most.
        static HEARD: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
        if let Some((status, ms)) = crate::vitals::render_step(&line)
            && now_ms() - HEARD.load(Relaxed) >= 100
        {
            HEARD.store(now_ms(), Relaxed);
            _ = std::thread::Builder::new().name("aio-vitals".into()).spawn(move || crate::vitals::render_heard(status, ms));
        }
        tally.count(&line);
        ERRORS.store(tally.errors, Relaxed);
        repeats.next(line).iter().for_each(|l| out(l));
    }
    repeats.end().iter().chain(&tally.said()).for_each(|l| out(l));
    if let Some(t) = term.as_mut() {
        for l in brief.flush() {
            _ = writeln!(t, "{l}");
        }
    }
}

/// `ax dev`'s terminal (`AIORT_BOOT=brief`): the boot report, a dozen rows
/// at each start, said in one line there (the log keeps every row).
#[derive(Default)]
struct BriefBoot {
    on: bool,
    rows: Vec<String>,
}

impl BriefBoot {
    /// What the terminal shows for `line` (pure).
    fn next(&mut self, line: &str) -> Vec<String> {
        if !self.on {
            return vec![line.into()];
        }
        if let Some(row) = line.strip_prefix("[aio] boot ") {
            self.rows.push(row.into());
            return vec![];
        }
        let mut out = self.flush();
        out.push(line.into());
        out
    }

    /// The rows so far in one line: the mode, and each one set for this run
    /// (by the environment or a flag, not aio.toml or a default) (pure).
    fn flush(&mut self) -> Vec<String> {
        if self.rows.is_empty() {
            return vec![];
        }
        let rows = std::mem::take(&mut self.rows);
        let split = |r: &String| {
            let (key, rest) = r.split_once(' ').unwrap_or((r, ""));
            let rest = rest.trim();
            let (value, from) = rest.rsplit_once(" (").map_or((rest, ""), |(v, f)| (v, f.strip_suffix(')').unwrap_or(f)));
            (key.to_string(), value.to_string(), from.to_string())
        };
        let mut said: Vec<String> = vec![];
        for (key, value, from) in rows.iter().map(split) {
            // Set for this run (an environment variable, a flag), or open to others: said.
            let set = from.starts_with("AIORT_") || from.starts_with("--");
            let open = value.contains("OPEN") || value.contains("open to the network");
            if key == "mode" || set || open {
                said.push(if key == "mode" { value } else { format!("{key} {value} ({from})") });
            }
        }
        let n = rows.len();
        said.push(format!("{n} settings in the log (ax dev --verbose shows each)"));
        vec![format!("[aio] boot: {}", said.join(", "))]
    }
}

/// A page error the runtime (Electron) throws itself, not the app, told by
/// its message AND its source file (an app's own error that only reads
/// alike stays an error): the line as `INFO`, labelled with the upstream
/// issue, so it is kept but never counted as the app's (pure).
pub fn runtime_noise(line: &str) -> Option<String> {
    let rest = line.strip_prefix("ERROR [page] ").or_else(|| line.strip_prefix("WARN  [page] "))?;
    let (msg, src) = rest.rsplit_once(" src=")?;
    let file = src.split(':').take(2).collect::<Vec<_>>().join(":");
    let m = msg.trim();
    let m = m.strip_prefix("Uncaught ").unwrap_or(m).trim_start();
    let m = m.strip_prefix("Error:").unwrap_or(m).trim_start();
    // electron#53989: thrown in Electron's own isolated-world bundle on every
    // <webview> detach; the guest is already gone and nothing failed.
    let id = m.strip_prefix("Invalid guestInstanceId: ")?;
    let ours = file == "node:electron/js2c/isolated_bundle" && !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit());
    ours.then(|| format!("INFO  [page] {rest} (the runtime's, not the app's: electron#53989, harmless)"))
}

/// The level-split log a line also goes to (pure): `error` or `warning`.
fn split(line: &str) -> Option<&'static str> {
    if line.starts_with("ERROR ") {
        Some("error")
    } else if line.starts_with("WARN ") {
        Some("warning")
    } else {
        None
    }
}

/// Where the copier writes: every line to `app.log`; an error also to
/// `error.log`, a warning to `warning.log` (each opened at its first line,
/// so a clean run has neither), a repeat count to where its line went.
struct Sinks {
    dir: PathBuf,
    cfg: crate::config::Log,
    app: Rotating,
    split: Vec<(&'static str, Rotating)>,
    last: Option<&'static str>,
}

impl Sinks {
    fn open(dir: &Path) -> io::Result<Self> {
        let cfg = settings();
        let app = Rotating::named(dir, "app", cfg.file_mb << 20)?.keeping(cfg.keep);
        let app = if cfg.per_run { app.fresh()? } else { app };
        Ok(Self { dir: dir.into(), cfg, app, split: vec![], last: None })
    }

    fn line(&mut self, at: i64, line: &str) -> io::Result<()> {
        self.app.line(at, line)?;
        let to = if line.starts_with("… last line repeated") { self.last } else { split(line) };
        self.last = to;
        let Some(stem) = to else { return Ok(()) };
        if !self.split.iter().any(|(s, _)| *s == stem) {
            let r = Rotating::named(&self.dir, stem, self.cfg.file_mb << 20)?.keeping(self.cfg.keep);
            let r = if self.cfg.per_run { r.fresh()? } else { r };
            self.split.push((stem, r));
        }
        self.split.iter_mut().find(|(s, _)| *s == stem).map_or(Ok(()), |(_, r)| r.line(at, line))
    }
}

/// `[log]` as `configure` set it.
static SETTINGS: std::sync::Mutex<Option<crate::config::Log>> = std::sync::Mutex::new(None);

fn settings() -> crate::config::Log {
    SETTINGS.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default()
}

/// Use `[log]` from `aio.toml`: its level now, its files and heartbeat from
/// the next [`start`] (`aio::boot` calls it first).
pub fn configure(cfg: &crate::config::Log) {
    set_level(cfg.level);
    *SETTINGS.lock().unwrap_or_else(|e| e.into_inner()) = Some(cfg.clone());
}

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
/// This run's errors (the copier's count) and calls (every method call).
static ERRORS: AtomicU64 = AtomicU64::new(0);
static CALLS: AtomicU64 = AtomicU64::new(0);
/// Which `start` a heartbeat belongs to: `finish` ends it.
static RUN: AtomicU64 = AtomicU64::new(0);

/// Method calls this run.
pub fn calls() -> u64 {
    CALLS.load(Relaxed)
}

thread_local!(static CALL: std::cell::Cell<(u64, &'static str)> = const { std::cell::Cell::new((0, "")) });
static NEXT_CALL: AtomicU64 = AtomicU64::new(1);

/// The id of the call running on this thread, 0 outside one (aio's
/// correlation id): a number per call this run, as `call=<id>` on every
/// line the call logs, in the timeline's entry and in a report it causes.
pub fn call_id() -> u64 {
    CALL.get().0
}

/// The cell whose call runs on this thread, if one does.
pub(crate) fn call_cell() -> Option<&'static str> {
    Some(CALL.get().1).filter(|c| !c.is_empty())
}

/// Run `f` as a new call of `cell`: its id is [`call_id`] until it returns.
pub(crate) fn in_call<R>(cell: &'static str, f: impl FnOnce() -> R) -> R {
    // Restored on the way out, a panic's too.
    struct Was((u64, &'static str));
    impl Drop for Was {
        fn drop(&mut self) {
            CALL.set(self.0);
        }
    }
    let _was = Was(CALL.replace((NEXT_CALL.fetch_add(1, Relaxed), cell)));
    f()
}

/// A call ran: counted, and at `debug` one line, unless `[log] suppress`
/// names it. Never its arguments: they may be anything the user typed.
pub(crate) fn called(cell: &str, method: &str, took: std::time::Duration, refused: Option<&str>) {
    CALLS.fetch_add(1, Relaxed);
    if !enabled(Level::Debug) || suppressed(cell, method) {
        return;
    }
    let label = format!("{cell}.{method}");
    let took = crate::fmt::dur(took);
    match refused {
        None => emit(Level::Debug, "aio", None, format_args!("call {label}"), &[("took", &took)]),
        Some(e) => emit(Level::Debug, "aio", None, format_args!("call {label}"), &[("took", &took), ("refused", &e)]),
    }
}

/// Does `[log] suppress` name `cell.method`: none of its lines, even at debug?
pub(crate) fn suppressed(cell: &str, method: &str) -> bool {
    settings().suppress.iter().any(|s| s.split_once('.') == Some((cell, method)))
}

/// The heartbeat line (pure): `INFO  [aio] heartbeat uptime=1h 00m calls=3 errors=0`.
pub fn beat(up: std::time::Duration, calls: u64, errors: u64) -> String {
    let up = crate::fmt::dur(up);
    render(Level::Info, "aio", "heartbeat", &[("uptime", &up), ("calls", &calls), ("errors", &errors)])
}

/// Every `[log] heartbeat` seconds, one line that says the app is alive and
/// how it is doing, until [`finish`].
fn heartbeat() {
    let every = std::time::Duration::from_secs(settings().heartbeat);
    let run = RUN.fetch_add(1, Relaxed) + 1;
    if every.is_zero() {
        return;
    }
    let t0 = std::time::Instant::now();
    _ = std::thread::Builder::new().name("aio-heartbeat".into()).spawn(move || {
        loop {
            std::thread::sleep(every);
            if RUN.load(Relaxed) != run {
                return;
            }
            if enabled(Level::Info) {
                _ = writeln!(io::stderr(), "{}", beat(t0.elapsed(), CALLS.load(Relaxed), ERRORS.load(Relaxed)));
            }
        }
    });
}

/// Should this output carry colour (pure)? The one decider for every line
/// aio prints: `FORCE_COLOR` wins (`0`, `false` and empty mean off), then
/// `NO_COLOR` set to anything turns it off (no-color.org), else only on a
/// terminal. Colour is decoration: off changes no character of a line.
pub fn colour(tty: bool, no_color: Option<std::ffi::OsString>, force: Option<std::ffi::OsString>) -> bool {
    if let Some(f) = force {
        let f = f.to_string_lossy().trim().to_ascii_lowercase();
        return !matches!(f.as_str(), "" | "0" | "false");
    }
    tty && no_color.is_none_or(|v| v.is_empty())
}

/// [`colour`] for `out` under this process's environment.
pub fn colour_for(out: &impl io::IsTerminal) -> bool {
    colour(out.is_terminal(), std::env::var_os("NO_COLOR"), std::env::var_os("FORCE_COLOR"))
}

/// A line with its level word coloured: errors red, warnings yellow, debug
/// and trace dim (pure).
pub fn painted(line: &str) -> std::borrow::Cow<'_, str> {
    let sgr = match line.get(..5) {
        Some("ERROR") => "31",
        Some("WARN ") => "33",
        Some("DEBUG" | "TRACE") => "2",
        _ => return line.into(),
    };
    format!("\x1b[{sgr}m{}\x1b[0m{}", &line[..5], &line[5..]).into()
}

/// What an `ax dev` app says when the `n`th hot-patch of its run lands:
/// numbered, so patches in a row are never collapsed into one line (pure).
#[doc(hidden)]
pub fn hot_patched(n: u64, functions: usize) -> String {
    format!("[aio] hot-patch {n} applied ({functions} functions)")
}

/// The same line many times in a row: written once, then how often again.
#[derive(Default)]
struct Repeats {
    last: Option<String>,
    again: u64,
}

impl Repeats {
    /// What to write for `line`: nothing for a repeat, else the summary of
    /// the run it ends (if any) and the line.
    fn next(&mut self, line: String) -> Vec<String> {
        if self.last.as_deref() == Some(line.as_str()) {
            self.again += 1;
            return vec![];
        }
        let mut out: Vec<String> = self.end().into_iter().collect();
        out.push(line.clone());
        self.last = Some(line);
        out
    }

    /// The pending run's summary, if lines were held back.
    fn end(&mut self) -> Option<String> {
        let n = std::mem::take(&mut self.again);
        (n > 0).then(|| format!("… last line repeated {n} more time{}", if n == 1 { "" } else { "s" }))
    }
}

/// A run's errors and warnings, by the level word their lines start with.
#[derive(Default)]
struct Tally {
    errors: u64,
    warnings: u64,
}

impl Tally {
    fn count(&mut self, line: &str) {
        if line.starts_with("ERROR ") {
            self.errors += 1;
        } else if line.starts_with("WARN ") {
            self.warnings += 1;
        }
    }

    /// The log's last line, when there is anything to count.
    fn said(&self) -> Option<String> {
        let n = |k: u64, w: &str| format!("{k} {w}{}", if k == 1 { "" } else { "s" });
        (self.errors + self.warnings > 0)
            .then(|| format!("[aio] log closed: {}, {} this run", n(self.errors, "error"), n(self.warnings, "warning")))
    }
}

/// Android has no terminal: each line goes to logcat too (tag `aio`).
#[cfg(target_os = "android")]
fn logcat(line: &str) {
    #[link(name = "log")]
    unsafe extern "C" {
        fn __android_log_write(prio: i32, tag: *const std::ffi::c_char, text: *const std::ffi::c_char) -> i32;
    }
    const INFO: i32 = 4;
    if let Ok(text) = std::ffi::CString::new(line) {
        // SAFETY: NUL-terminated strings that outlive the call.
        unsafe { __android_log_write(INFO, c"aio".as_ptr(), text.as_ptr()) };
    }
}

/// Windows: the std error handle becomes a pipe (Rust's stderr and every
/// child spawned with inherited stdio write there).
#[cfg(windows)]
pub fn start(logs: &Path) -> io::Result<()> {
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, SetStdHandle};
    use windows_sys::Win32::System::Pipes::CreatePipe;
    let log = Sinks::open(logs)?;
    let (mut r, mut w) = (std::ptr::null_mut(), std::ptr::null_mut());
    // SAFETY: plain Win32 calls on handles this function owns. Neither end
    // is inheritable: Rust duplicates the std handles into each child itself.
    let term = unsafe {
        if CreatePipe(&mut r, &mut w, std::ptr::null(), 0) == 0 {
            return Err(io::Error::last_os_error());
        }
        let term = GetStdHandle(STD_ERROR_HANDLE);
        if SetStdHandle(STD_ERROR_HANDLE, w) == 0 {
            return Err(io::Error::last_os_error());
        }
        term
    };
    let (pipe, w) = unsafe { (File::from_raw_handle(r), OwnedHandle::from_raw_handle(w)) };
    // A GUI app has no terminal: then the log alone.
    let has_term = !term.is_null() && term != windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    let term_at = term as usize;
    let env = (std::env::var_os("NO_COLOR"), std::env::var_os("FORCE_COLOR"));
    let copier = std::thread::Builder::new().name("aio-log".into()).spawn(move || {
        // Not ours to close: the process's original stderr.
        // SAFETY: a live handle; `Term` never closes it.
        let t = has_term.then(|| Term(std::mem::ManuallyDrop::new(unsafe { File::from_raw_handle(term_at as _) })));
        let paint = colour(t.as_ref().is_some_and(|t| io::IsTerminal::is_terminal(&*t.0)), env.0, env.1);
        copy(pipe, t, paint, log)
    })?;
    *tee() = Some((copier, (term_at, w)));
    finish_at_exit();
    heartbeat();
    Ok(())
}

/// Writes through a borrowed (never closed) terminal file.
#[cfg(windows)]
struct Term(std::mem::ManuallyDrop<File>);
#[cfg(windows)]
impl Write for Term {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.0.write(b)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

/// [`finish`] at the process's exit too: an app whose `main` returns an
/// error (Rust prints `Error: …` after `main`) or calls `exit` never
/// reaches it, and its last line (why it ended) would be lost. Waits for
/// the copier a second at most: a child still holding the pipe never
/// keeps the exit waiting. (Windows: `std::process::exit` ends the process
/// at once, its threads first; only a return from `main` gets here.)
#[cfg(any(unix, windows))]
fn finish_at_exit() {
    #[cfg(windows)]
    unsafe extern "C" {
        fn atexit(f: extern "C" fn()) -> std::ffi::c_int;
    }
    #[cfg(windows)]
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn RtlDllShutdownInProgress() -> u8;
    }
    #[cfg(unix)]
    use libc::atexit;
    extern "C" fn at_exit() {
        // SAFETY: a plain query. True once the process is ending its threads
        // (`ExitProcess`): the copier is gone, nothing to wait for.
        #[cfg(windows)]
        if unsafe { RtlDllShutdownInProgress() } != 0 {
            return;
        }
        finish_by(Some(std::time::Duration::from_secs(1)));
    }
    static ONCE: std::sync::Once = std::sync::Once::new();
    // SAFETY: registers a plain function; `finish` is safe at any time.
    ONCE.call_once(|| _ = unsafe { atexit(at_exit) });
}

/// Write out every line still on its way to the log, then send stderr
/// straight to the terminal again. Call it last before exiting: a stop or
/// crash reason is the last line, and must not be lost.
pub fn finish() {
    finish_by(None);
}

/// [`finish`], waiting for the copier `wait` at most (else until it ends).
fn finish_by(wait: Option<std::time::Duration>) {
    let Some((copier, back)) = tee().take() else { return };
    RUN.fetch_add(1, Relaxed);
    // SAFETY: fd 2 becomes the terminal again, which closes this process's
    // end of the pipe; the copier then reads to the end and stops.
    #[cfg(unix)]
    unsafe {
        libc::dup2(back, 2);
        libc::close(back);
    }
    // SAFETY: the std error handle becomes the terminal again; dropping the
    // write end then ends the pipe the same way.
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::System::Console::SetStdHandle(
            windows_sys::Win32::System::Console::STD_ERROR_HANDLE,
            back.0 as _,
        );
        drop(back.1);
    }
    let Some(wait) = wait else {
        _ = copier.join();
        return;
    };
    let end = std::time::Instant::now() + wait;
    while !copier.is_finished() && std::time::Instant::now() < end {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    if copier.is_finished() {
        _ = copier.join();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn ax_devs_terminal_has_the_boot_report_in_one_line() {
        let mut b = super::BriefBoot { on: true, rows: vec![] };
        for l in [
            "[aio] boot app        notes (aio.toml)",
            "[aio] boot mode       standalone (aio.toml)",
            "[aio] boot config     aio.toml built into the app (aio::config!())",
            "[aio] boot log        debug (AIORT_LOG)",
            "[aio] boot gpu        hardware (default)",
        ] {
            assert!(b.next(l).is_empty(), "{l}");
        }
        assert_eq!(
            b.next("[aio] window ready"),
            [
                "[aio] boot: standalone, log debug (AIORT_LOG), 5 settings in the log (ax dev --verbose shows each)",
                "[aio] window ready"
            ]
        );
        assert!(b.flush().is_empty());
        let mut off = super::BriefBoot::default();
        assert_eq!(off.next("[aio] boot app notes (aio.toml)"), ["[aio] boot app notes (aio.toml)"]);
    }

    use super::*;

    #[test]
    fn stamps_utc() {
        assert_eq!(stamp(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(stamp(1_793_142_062_123), "2026-10-27T23:01:02.123Z");
        assert_eq!(stamp(951_782_400_000), "2000-02-29T00:00:00.000Z");
    }

    #[test]
    fn rotates_within_budget() {
        let d = tempfile_dir();
        let mut r = Rotating::open(&d, 100).unwrap();
        for i in 0..50 {
            r.line(0, &format!("line {i}")).unwrap();
        }
        let mut names: Vec<_> =
            std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        names.sort();
        assert_eq!(names, ["app.1.log", "app.2.log", "app.3.log", "app.4.log", "app.log"]);
        let last = std::fs::read_to_string(d.join("app.log")).unwrap();
        assert!(last.ends_with("line 49\n") && last.starts_with("1970-01-01T00:00:00.000Z "));
        assert!(std::fs::read_dir(&d).unwrap().all(|e| e.unwrap().metadata().unwrap().len() <= 100));
        // Windows deletes no dir with a file open in it.
        drop(r);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn per_run_each_run_starts_a_fresh_file_and_keeps_the_last_ones() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        for run in 0..4 {
            let mut r = Rotating::open(d, 1 << 20).unwrap().keeping(3).fresh().unwrap();
            r.line(0, &format!("run {run}")).unwrap();
        }
        let read = |f: &str| std::fs::read_to_string(d.join(f)).unwrap_or_default();
        assert!(read("app.log").ends_with("run 3\n") && read("app.1.log").ends_with("run 2\n"));
        assert!(read("app.2.log").ends_with("run 1\n") && !d.join("app.3.log").exists(), "keep 3");
        // An empty file is not a run: nothing moves.
        drop(Rotating::named(d, "warning", 1 << 20).unwrap().fresh().unwrap());
        drop(Rotating::named(d, "warning", 1 << 20).unwrap().fresh().unwrap());
        assert!(!d.join("warning.1.log").exists());
    }

    #[test]
    fn a_line_has_level_category_message_and_fields() {
        let (peer, n) = ("a1", 3);
        assert_eq!(render(Level::Warn, "net", "lost", &[("peer", &peer), ("n", &n)]), "WARN  [net] lost peer=a1 n=3");
        assert_eq!(render(Level::Error, "app", "boom", &[]), "ERROR [app] boom");
        assert_eq!((tag("aio"), tag("aio::store"), tag("notes::ui"), tag("aio_x")), ("aio", "aio", "app", "app"));
        assert!(Level::Trace < Level::Debug && Level::Warn < Level::Error);
    }

    #[test]
    fn no_caller_text_forges_an_entry() {
        let forged = "2026-09-02T00:00:00.000Z ERROR [auth] admin login ok";
        for p in [format!("x\n{forged}"), format!("x\r{forged}"), format!("x\r\n{forged}")] {
            let l = render(Level::Info, &p, &p, &[(p.as_str(), &p)]);
            assert!(!l.contains(['\n', '\r']), "{l:?}");
            assert_eq!(l.matches("x\\n2026").count(), 4, "kept on record: {l}");
        }
        // Colour stays; what moves, erases or retitles is escaped.
        for sgr in ["\x1b[31m", "\x1b[0m", "\x1b[1;32m", "\x1b[38;5;244m"] {
            assert_eq!(safe(&format!("a{sgr}b")), format!("a{sgr}b"));
        }
        for seq in ["\x1b[2A", "\x1b[H", "\x1b[2J", "\x1b]0;pwned\x07", "\x1bZ", "\u{9b}2J"] {
            assert!(!safe(&format!("x{seq}y")).chars().any(char::is_control), "{seq:?}");
        }
        assert_eq!(safe("a\tb\x00\x7f"), "a\tb\\x00\\x7f");
        // A literal backslash-x stays literal: nothing unescapes it.
        assert_eq!(safe("a\\x0ab"), "a\\x0ab");
    }

    #[test]
    fn a_huge_text_is_capped_loudly() {
        let out = safe(&"é".repeat(20_000));
        assert!(out.starts_with(&"é".repeat(MAX_TEXT)) && out.ends_with(" … [truncated 11808 chars]"));
        assert_eq!(safe("short"), "short");
    }

    #[test]
    fn a_failing_display_never_panics_the_call() {
        struct Bad;
        impl Display for Bad {
            fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("part")?;
                Err(std::fmt::Error)
            }
        }
        assert_eq!(render(Level::Info, "app", "m", &[("v", &Bad)]), "INFO  [app] m v=part");
    }

    #[test]
    fn repeats_collapse_into_a_count() {
        let mut r = Repeats::default();
        let mut out = vec![];
        for l in ["a", "b", "b", "b", "c", "c", "a"] {
            out.extend(r.next(l.into()));
        }
        out.extend(r.end());
        assert_eq!(out, ["a", "b", "… last line repeated 2 more times", "c", "… last line repeated 1 more time", "a"]);
        assert_eq!(r.end(), None, "nothing pending");
    }

    /// Every hot-patch of a run is said the moment it lands: an unnumbered
    /// line was the same each time (the function count rarely changes), so
    /// all patches after the first were held back as repeats and looked lost.
    #[test]
    fn each_hot_patch_in_a_row_is_its_own_line() {
        let mut r = Repeats::default();
        let out: Vec<String> = (1..=5).flat_map(|n| r.next(hot_patched(n, 7179))).collect();
        assert_eq!(out.len(), 5, "{out:?}");
        assert_eq!(out[4], "[aio] hot-patch 5 applied (7179 functions)");
        assert_eq!(r.end(), None, "nothing held back");
    }

    #[test]
    fn the_last_line_counts_errors_and_warnings() {
        let mut t = Tally::default();
        assert_eq!(t.said(), None, "a clean run says nothing");
        for l in ["ERROR [app] x", "WARN  [net] y", "WARN  [net] z", "INFO  [app] ERROR in text", "[aio] ok"] {
            t.count(l);
        }
        assert_eq!(t.said().unwrap(), "[aio] log closed: 1 error, 2 warnings this run");
    }

    #[test]
    fn colour_only_on_a_terminal_without_no_color() {
        assert!(colour(true, None, None));
        assert!(colour(true, Some("".into()), None), "an empty NO_COLOR is unset");
        assert!(!colour(true, Some("1".into()), None));
        assert!(!colour(false, None, None), "a pipe is not a terminal");
        // FORCE_COLOR wins both ways (a pipe that renders ANSI: less -R, CI).
        for on in ["1", "true", "3"] {
            assert!(colour(false, Some("1".into()), Some(on.into())), "{on}");
        }
        for off in ["0", "false", "FALSE", "", " 0 "] {
            assert!(!colour(true, None, Some(off.into())), "{off:?}");
        }
        // Off changes no character: the painted line minus its SGR is the line.
        for l in ["ERROR [db] a subscriber threw", "WARN  [vitals] slow", "DEBUG [x] y", "INFO  [aio] up"] {
            let p = painted(l);
            assert_eq!(
                ["\x1b[31m", "\x1b[33m", "\x1b[2m", "\x1b[0m"].iter().fold(p.into_owned(), |s, c| s.replace(c, "")),
                l
            );
        }
        assert_eq!(painted("ERROR [app] x"), "\x1b[31mERROR\x1b[0m [app] x");
        assert_eq!(painted("WARN  [app] x"), "\x1b[33mWARN \x1b[0m [app] x");
        assert_eq!(painted("[aio] plain"), "[aio] plain");
        assert_eq!(painted("é"), "é", "short or multibyte: as is");
    }

    #[test]
    fn errors_and_warnings_also_get_their_own_files() {
        let d = tempfile::tempdir().unwrap();
        let mut s = Sinks::open(d.path()).unwrap();
        for l in ["INFO  [app] a", "WARN  [net] b", "ERROR [db] c", "… last line repeated 2 more times", "[aio] x"] {
            s.line(0, l).unwrap();
        }
        let read = |f: &str| {
            std::fs::read_to_string(d.path().join(f)).unwrap().lines().map(|l| l[25..].to_string()).collect::<Vec<_>>()
        };
        assert_eq!(read("app.log").len(), 5);
        assert_eq!(
            read("error.log"),
            ["ERROR [db] c", "… last line repeated 2 more times"],
            "the count goes with its line"
        );
        assert_eq!(read("warning.log"), ["WARN  [net] b"]);
        drop(s);
        let clean = tempfile::tempdir().unwrap();
        Sinks::open(clean.path()).unwrap().line(0, "INFO  [app] fine").unwrap();
        assert!(!clean.path().join("error.log").exists(), "a clean run has no error.log");
        assert_eq!((split("WARNING x"), split("ERRORS")), (None, None));
    }

    #[test]
    fn keep_bounds_the_files_and_the_heartbeat_says_how_it_goes() {
        let d = tempfile::tempdir().unwrap();
        let mut r = Rotating::named(d.path(), "app", 30).unwrap().keeping(2);
        for i in 0..20 {
            r.line(0, &format!("line {i}")).unwrap();
        }
        drop(r);
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 2);
        let up = std::time::Duration::from_secs(3600);
        assert_eq!(beat(up, 312, 0), "INFO  [aio] heartbeat uptime=1h 00m calls=312 errors=0");
    }

    #[test]
    fn a_test_captures_its_own_lines_and_a_sink_replaces_stderr() {
        let ((), lines) = capture(|| {
            crate::warn!(cat: "net", peer = "a1", "lost");
            let ((), inner) = capture(|| crate::info!("nested"));
            assert_eq!(inner, ["INFO  [aio] nested"]);
            crate::debug!("below the level");
        });
        assert_eq!(lines, ["WARN  [net] lost peer=a1"], "the outer capture keeps its own");
        // A sink: replaced and read back; one that panics loses nothing.
        let got = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
        let g = got.clone();
        let before = set_sink(Some(std::sync::Arc::new(move |l: Level, line: &str| {
            g.lock().unwrap().push((l, line.to_string()))
        })));
        assert!(before.is_none() && sink().is_some());
        std::thread::spawn(|| crate::error!("from a thread")).join().unwrap();
        set_sink(Some(std::sync::Arc::new(|_: Level, _: &str| panic!("sink bug"))));
        crate::error!("still written");
        set_sink(before);
        assert!(sink().is_none());
        assert!(got.lock().unwrap().contains(&(Level::Error, "ERROR [aio] from a thread".to_string())));
    }

    #[test]
    fn the_runtimes_own_page_errors_are_labelled_and_not_counted() {
        let src = " src=node:electron/js2c/isolated_bundle:1";
        for msg in [
            "Uncaught Error: Invalid guestInstanceId: 2",
            "Error: Invalid guestInstanceId: 4",
            "Invalid guestInstanceId: 9",
        ] {
            let l = runtime_noise(&format!("ERROR [page] {msg}{src}")).unwrap();
            assert!(l.starts_with("INFO  [page] ") && l.ends_with("electron#53989, harmless)"), "{l}");
            let mut t = Tally::default();
            t.count(&l);
            assert_eq!(t.said(), None, "not an error of the app's");
        }
        // Both facts, or it is the app's own error and stays one.
        assert_eq!(
            runtime_noise("ERROR [page] Uncaught Error: Invalid guestInstanceId: 2 src=aio://app/main.js:3"),
            None
        );
        assert_eq!(runtime_noise(&format!("ERROR [page] Invalid guestInstanceId: x{src}")), None);
        assert_eq!(runtime_noise("ERROR [page] Invalid guestInstanceId: 2"), None, "no source, no rule");
        assert_eq!(runtime_noise(&format!("ERROR [app] Invalid guestInstanceId: 2{src}")), None);
        assert_eq!(runtime_noise(&format!("ERROR [page] TypeError: x is undefined{src}")), None);
    }

    #[test]
    fn levels_read_from_config_words() {
        #[derive(serde::Deserialize)]
        struct L {
            level: Level,
        }
        let l: L = toml::from_str("level = \"debug\"").unwrap();
        assert_eq!((l.level, Level::default()), (Level::Debug, Level::Info));
        assert!(toml::from_str::<L>("level = \"loud\"").is_err());
    }

    fn tempfile_dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("aio-log-test-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }
}

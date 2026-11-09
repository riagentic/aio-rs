//! What the manager reads off the machine (amui's `server/proc.server.ts`):
//! a process's CPU and memory, what a running app runs from, its project's
//! files (read-only, guarded), where a cell is defined, its logs' tails,
//! and its tasks, run cancellable and capped. Nothing here writes a file.
use std::io::{Read, Seek};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

// ── process ──────────────────────────────────────────────────────────────────

/// One reading of a process: CPU in % of one core, resident memory in MB.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stats {
    pub cpu_pct: f64,
    pub mem_mb: f64,
}

/// The last cumulative CPU reading per pid: what makes the next reading of
/// that pid a rate over the time between them (amui's `lastCpu`).
static LAST_CPU: std::sync::Mutex<Vec<(u32, f64, Instant)>> = std::sync::Mutex::new(Vec::new());

/// `pid`'s CPU seconds so far, its start (seconds after boot) and resident
/// pages, from a `/proc/<pid>/stat` and `statm` text (pure). The fields
/// after the parenthesised name (which may hold spaces): utime and stime
/// (clock ticks, 100 a second in Linux's ABI), starttime.
pub fn proc_stat(stat: &str, statm: &str) -> Option<(f64, f64, u64)> {
    let f: Vec<&str> = stat.get(stat.rfind(')')? + 2..)?.split(' ').collect();
    let tick = |i: usize| f.get(i)?.parse::<f64>().ok().map(|t| t / 100.0);
    let pages = statm.split_whitespace().nth(1)?.parse().ok()?;
    Some((tick(11)? + tick(12)?, tick(19)?, pages))
}

/// CPU % and RSS of `pid` now (amui's `psStats`): the CPU over the time
/// since the last reading of the same pid (the first: its lifetime's
/// average). `/proc` on Linux; elsewhere `None`, and the app's own
/// `health.json` answers. A dead pid is `None`, never a NaN reading.
pub fn ps_stats(pid: u32) -> Option<Stats> {
    let read = |f: &str| std::fs::read_to_string(format!("/proc/{pid}/{f}")).ok();
    let (cpu, start, pages) = proc_stat(&read("stat")?, &read("statm")?)?;
    let now = Instant::now();
    let up: f64 = std::fs::read_to_string("/proc/uptime").ok()?.split_whitespace().next()?.parse().ok()?;
    let mut last = LAST_CPU.lock().unwrap_or_else(|e| e.into_inner());
    let prev = last.iter().position(|(p, ..)| *p == pid).map(|i| last.remove(i));
    let cpu_pct = match prev {
        Some((_, was, at)) if cpu >= was && now > at => (cpu - was) / (now - at).as_secs_f64() * 100.0,
        _ => cpu / (up - start).max(0.01) * 100.0,
    };
    last.push((pid, cpu, now));
    if last.len() > 64 {
        last.remove(0);
    }
    let mem_mb = (pages * page_size()) as f64 / 1_048_576.0;
    (cpu_pct.is_finite() && mem_mb.is_finite())
        .then_some(Stats { cpu_pct: (cpu_pct * 10.0).round() / 10.0, mem_mb: mem_mb.round() })
}

fn page_size() -> u64 {
    #[cfg(unix)]
    // SAFETY: sysconf only reads a constant.
    let n = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    #[cfg(not(unix))]
    let n = 4096;
    if n > 0 { n as u64 } else { 4096 }
}

/// What a running app runs from (amui's `RuntimeInfo`): a debug build of
/// its workspace (`dev`), a release build, an installed copy, an AppImage
/// (its unpacked mount), or not known.
#[derive(Clone, Debug, PartialEq)]
pub struct Runtime {
    pub kind: &'static str,
    /// The folder whose files are what runs (the Codebase tab's).
    pub root: PathBuf,
    pub exe: Option<PathBuf>,
    pub label: String,
}

/// The runtime of an app from its executable (amui's `runtimeOf`, pure):
/// `project`, its project folder when known, `installed`, where `ax
/// install` puts apps.
pub fn runtime_of(exe: Option<&Path>, project: Option<&Path>, installed: &Path) -> Runtime {
    let fallback = |exe: Option<&Path>| project.map(Path::to_path_buf).or(exe.and_then(|e| e.parent().map(Into::into)));
    let Some(exe) = exe else {
        let root = fallback(None).unwrap_or_default();
        return Runtime { kind: "unknown", root, exe: None, label: "runtime = project folder".into() };
    };
    let text = exe.to_string_lossy();
    if let Some(at) = text.find("/.mount_") {
        let end = text[at + 1..].find('/').map_or(text.len(), |e| at + 1 + e);
        let mount = PathBuf::from(&text[..end]);
        let label = format!("AppImage — unpacked at {}", mount.display());
        return Runtime { kind: "appimage", root: mount, exe: Some(exe.into()), label };
    }
    if exe.starts_with(installed) {
        let root = exe.parent().map(Into::into).unwrap_or_default();
        let label = format!("installed — {}", exe.display());
        return Runtime { kind: "installed", root, exe: Some(exe.into()), label };
    }
    let debug = exe.components().any(|c| matches!(c.as_os_str().to_str(), Some("debug" | "desktop-dev")));
    let (kind, label) = if debug {
        ("dev", "dev — a debug build of its workspace".to_string())
    } else {
        ("release", format!("release build — {}", exe.display()))
    };
    Runtime { kind, root: fallback(Some(exe)).unwrap_or_default(), exe: Some(exe.into()), label }
}

/// The executable and working folder of `pid`: `/proc` on Linux; elsewhere
/// none ([`health_paths`] answers).
pub fn process_paths(pid: u32) -> (Option<PathBuf>, Option<PathBuf>) {
    let link = |f: &str| std::fs::read_link(format!("/proc/{pid}/{f}")).ok();
    (link("exe"), link("cwd"))
}

/// The executable and working folder an app's `health.json` says it has,
/// when it is `pid`'s (pure): on every OS, as the app knows them itself.
pub fn health_paths(pid: u32, health: &aio::serde_json::Value) -> (Option<PathBuf>, Option<PathBuf>) {
    if health["pid"].as_u64() != Some(pid.into()) {
        return (None, None);
    }
    let at = |k: &str| health[k].as_str().filter(|p| !p.is_empty()).map(PathBuf::from);
    (at("exe"), at("cwd"))
}

// ── files ────────────────────────────────────────────────────────────────────

/// One entry of a project's tree, its path relative to the root.
#[derive(Clone, Debug, PartialEq)]
pub struct FileNode {
    pub path: String,
    pub name: String,
    pub dir: bool,
}

/// At most this many nodes listed (keeps the tree responsive), this deep.
pub const FILE_CAP: usize = 1200;
const FILE_DEPTH: usize = 6;
/// Larger files show a notice instead.
pub const VIEW_MAX: u64 = 2_000_000;

/// Folders never listed nor read, whatever root the viewer is pointed at:
/// they hold private keys and cloud credentials.
const SECRET_DIRS: [&str; 3] = [".ssh", ".aws", ".gnupg"];

/// Folders never worth showing: VCS, deps, build output, caches.
const IGNORE_DIRS: [&str; 17] = [
    "node_modules",
    "dist",
    "build",
    "out",
    ".git",
    ".aio",
    ".cache",
    ".venv",
    "venv",
    "vendor",
    "target",
    "coverage",
    "pkg-web",
    ".next",
    ".turbo",
    ".data",
    ".dx",
];

fn junk(name: &str) -> bool {
    name.ends_with(".db")
        || name.ends_with(".db-wal")
        || name.ends_with(".db-shm")
        || name.ends_with(".lock")
        || name.ends_with(".log")
}

/// The project's tree under `dir` (amui's `listFiles`): folders first, each
/// by name, no deps, build output or secrets; capped (`true`: it was).
pub fn list_files(dir: &Path) -> (Vec<FileNode>, bool) {
    fn walk(root: &Path, rel: &str, depth: usize, out: &mut Vec<FileNode>) -> bool {
        if depth > FILE_DEPTH {
            return false;
        }
        let Ok(rd) = std::fs::read_dir(root.join(rel)) else { return false };
        let mut kids: Vec<(String, bool)> = rd
            .flatten()
            .map(|e| (e.file_name().to_string_lossy().into_owned(), e.file_type().is_ok_and(|t| t.is_dir())))
            .collect();
        kids.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        for (name, dir) in kids {
            if (dir && (IGNORE_DIRS.contains(&name.as_str()) || SECRET_DIRS.contains(&name.as_str())))
                || (!dir && junk(&name))
            {
                continue;
            }
            if out.len() >= FILE_CAP {
                return true;
            }
            let path = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            out.push(FileNode { path: path.clone(), name, dir });
            if dir && walk(root, &path, depth + 1, out) {
                return true;
            }
        }
        false
    }
    let mut out = Vec::new();
    let truncated = walk(dir, "", 0, &mut out);
    (out, truncated)
}

/// Does `rel` (or `dir`) pass through a secret folder?
pub fn touches_secret(dir: &Path, rel: &str) -> bool {
    dir.join(rel).components().any(|c| SECRET_DIRS.iter().any(|s| c.as_os_str() == *s))
}

/// A file's text in the viewer (amui's `readFile`): only under `dir` (no
/// `..`, no symlink out), never in a secret folder; a large or binary file
/// is refused with why.
pub fn read_file(dir: &Path, rel: &str) -> Result<String, String> {
    let p = Path::new(rel);
    if p.is_absolute() || p.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err("path outside project".into());
    }
    if touches_secret(dir, rel) {
        return Err(format!(
            "refusing to read {rel}: ax ui never opens {} — those hold private keys and cloud credentials. \
             Open it in your editor if you need it.",
            SECRET_DIRS.join("/")
        ));
    }
    let real = std::fs::canonicalize(dir.join(rel)).map_err(|e| e.to_string())?;
    let root = std::fs::canonicalize(dir).map_err(|e| e.to_string())?;
    if !real.starts_with(&root) {
        return Err("path outside project".into());
    }
    let meta = std::fs::metadata(&real).map_err(|e| e.to_string())?;
    if meta.is_dir() {
        return Err("is a directory".into());
    }
    if meta.len() > VIEW_MAX {
        let mb = |n: u64| n as f64 / 1_048_576.0;
        return Err(format!(
            "too large to view — {:.1} MB (limit {:.0} MB). Open it in your editor.",
            mb(meta.len()),
            mb(VIEW_MAX)
        ));
    }
    let bytes = std::fs::read(&real).map_err(|e| e.to_string())?;
    if bytes[..bytes.len().min(8192)].contains(&0) {
        return Err(format!("binary file ({:.1} KB) — not shown", bytes.len() as f64 / 1024.0));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// The folders [`find_repo_root`] may accept, nearest first: `dir` and its
/// parents, at most `max`, never `home` or above it, never the root (pure).
pub fn repo_candidates(dir: &Path, home: &Path, max: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for cur in dir.ancestors().take(max) {
        if cur == home || cur.parent().is_none() {
            break;
        }
        out.push(cur.to_path_buf());
    }
    out
}

/// The git repository `dir` is in (its first parent with a `.git`), within
/// six folders and never the home folder: a home kept in git is not "the
/// repo of the app", and showing it would list its secrets.
pub fn find_repo_root(dir: &Path, home: &Path) -> Option<PathBuf> {
    repo_candidates(dir, home, 6).into_iter().find(|d| d.join(".git").exists())
}

/// Where cell `cell` is defined under `root` (amui's `findCellSource`): the
/// `struct` under a `#[cell(…)]` whose id is `cell` (its `id = "…"`, else
/// its name in snake case), as its path and 1-based line.
pub fn find_cell_source(root: &Path, cell: &str) -> Option<(String, usize)> {
    let (nodes, _) = list_files(root);
    nodes.iter().filter(|n| !n.dir && n.name.ends_with(".rs")).find_map(|n| {
        let p = root.join(&n.path);
        let meta = std::fs::metadata(&p).ok()?;
        if !meta.is_file() || meta.len() > 1_000_000 {
            return None;
        }
        cell_line(&std::fs::read_to_string(&p).ok()?, cell).map(|l| (n.path.clone(), l))
    })
}

/// The line of cell `cell`'s struct in Rust `text` (pure).
pub fn cell_line(text: &str, cell: &str) -> Option<usize> {
    let mut id: Option<Option<String>> = None;
    for (i, line) in text.lines().enumerate() {
        let t = line.trim();
        if t.starts_with("#[") && (t.contains("::cell(") || t.starts_with("#[cell(") || t == "#[cell]") {
            id = Some(t.split_once("id = \"").and_then(|(_, r)| r.split_once('"')).map(|(v, _)| v.to_string()));
            continue;
        }
        let Some(declared) = &id else { continue };
        if let Some(rest) = t.strip_prefix("pub struct ").or_else(|| t.strip_prefix("struct ")) {
            let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            if declared.as_deref().unwrap_or(&snake(&name)) == cell {
                return Some(i + 1);
            }
            id = None;
        } else if !t.starts_with("#[") && !t.starts_with("//") && !t.is_empty() {
            id = None;
        }
    }
    None
}

/// `NotesBoard` → `notes_board`, as a cell's default id.
fn snake(s: &str) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.extend(c.to_lowercase());
    }
    out
}

// ── logs ─────────────────────────────────────────────────────────────────────

/// Which log the Logs tab shows (amui's sources): the launch's captured
/// output beside the log (`combined`), the app's log (`app`), its warnings
/// and errors (`error`), its window's lines (`client`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LogSource {
    #[default]
    Combined,
    App,
    Error,
    Client,
}

/// A log's tail: its last lines, the file read, whether older lines were
/// left out, and whether there was none.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawLog {
    pub lines: Vec<String>,
    pub path: Option<PathBuf>,
    pub truncated: bool,
    pub missing: bool,
}

/// At most this much of a log is read: its end.
const LOG_TAIL_MAX: u64 = 512 * 1024;
/// How far a launch's capture may trail the log and still be this run's.
const CAPTURE_SLACK: Duration = Duration::from_secs(5);

/// The lines of a log text: colour codes out, blank lines out (pure).
pub fn log_lines(text: &str) -> Vec<String> {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            for d in chars.by_ref() {
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out.lines().filter(|l| !l.is_empty()).map(String::from).collect()
}

/// The end of the file at `p` (at most [`LOG_TAIL_MAX`]), from its first
/// whole line; `true`: the start was left out.
fn tail_text(p: &Path) -> std::io::Result<(String, bool)> {
    let mut f = std::fs::File::open(p)?;
    let size = f.metadata()?.len();
    if size <= LOG_TAIL_MAX {
        let mut s = Vec::new();
        f.read_to_end(&mut s)?;
        return Ok((String::from_utf8_lossy(&s).into_owned(), false));
    }
    // One byte early: a window starting on a whole line keeps it.
    f.seek(std::io::SeekFrom::Start(size - LOG_TAIL_MAX - 1))?;
    let mut buf = Vec::new();
    f.take(LOG_TAIL_MAX + 1).read_to_end(&mut buf)?;
    let from = buf.iter().position(|b| *b == b'\n').map_or(0, |i| i + 1);
    Ok((String::from_utf8_lossy(&buf[from..]).into_owned(), true))
}

/// The files a source reads, best first (amui's `logCandidates` and its
/// freshness order): for `combined`, the launch's capture (`start.log`)
/// before the app's log unless it is an older run's.
pub fn log_candidates(logs: &Path, source: LogSource) -> Vec<PathBuf> {
    let (log, capture) = (logs.join("app.log"), logs.join("start.log"));
    match source {
        LogSource::App | LogSource::Client => vec![log],
        LogSource::Error => vec![logs.join("warning.log")],
        LogSource::Combined => {
            let at = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
            match (at(&capture), at(&log)) {
                (Some(c), Some(l)) if c + CAPTURE_SLACK < l => vec![log, capture],
                _ => vec![capture, log],
            }
        }
    }
}

/// Is `line` (or the event it continues) the window's (`ax logs --client`)?
fn client_line(line: &str, last: &mut bool) -> bool {
    if let Some(h) = crate::logs::head(line) {
        *last = matches!(h.tag.as_deref(), Some("page" | "aio-shell"));
    }
    *last
}

/// The last `n` lines of app logs folder `logs`'s `source` (amui's `readLogs`).
pub fn read_logs(logs: &Path, source: LogSource, n: usize) -> RawLog {
    for p in log_candidates(logs, source) {
        if !p.is_file() {
            continue;
        }
        let Ok((text, dropped)) = tail_text(&p) else { continue };
        let mut all = log_lines(&text);
        if source == LogSource::Client {
            let mut last = false;
            all.retain(|l| client_line(l, &mut last));
        }
        let cut = all.len().saturating_sub(n);
        return RawLog { truncated: dropped || cut > 0, lines: all.split_off(cut), path: Some(p), missing: false };
    }
    RawLog { missing: true, ..RawLog::default() }
}

// ── tasks ────────────────────────────────────────────────────────────────────

/// Every task ends within this (amui's 5 min cap): a long-running one
/// (`ax dev`) never wedges the runner.
pub const TASK_TIMEOUT: Duration = Duration::from_secs(300);
/// At most this much of a task's output is kept: its end.
const LOG_MAX: usize = 200_000;

/// How a task ended: its exit code (`None`: stopped), its output, and why it
/// stopped (`""` ran to its end, `cancelled`, `timeout`).
#[derive(Clone, Debug, PartialEq)]
pub struct Ran {
    pub code: Option<i32>,
    pub output: String,
    pub ended: &'static str,
}

/// Run `program args` in `dir` (amui's `runTask`), its output and errors
/// captured (capped) into `out` as they come, until it ends, `cancel` is
/// set, or `timeout`: then its whole process group is asked to stop, and
/// killed 2 s later. The manager's own environment (its runtime's
/// variables) never reaches it.
pub fn run_task(
    dir: &Path,
    program: &Path,
    args: &[String],
    cancel: &Arc<AtomicBool>,
    timeout: Duration,
    out: &Arc<std::sync::Mutex<Vec<u8>>>,
) -> Ran {
    let mut cmd = std::process::Command::new(program);
    cmd.args(args)
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    aio::lifecycle::scrub(&mut cmd, true);
    // Shown to a person as it comes: an ax verb's text, though its stdout is a pipe.
    cmd.env("AIORT_OUTPUT", "text");
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Ran { code: None, output: format!("failed to spawn: {e}"), ended: "" },
    };
    let pipe = |mut r: Box<dyn Read + Send>| {
        let out = out.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n @ 1..) = r.read(&mut buf) {
                let mut o = out.lock().unwrap_or_else(|e| e.into_inner());
                o.extend_from_slice(&buf[..n]);
                let over = o.len().saturating_sub(LOG_MAX);
                o.drain(..over);
            }
        })
    };
    let readers = [child.stdout.take().map(|s| pipe(Box::new(s))), child.stderr.take().map(|s| pipe(Box::new(s)))];
    let (start, mut ended, mut killed_at) = (Instant::now(), "", None::<Instant>);
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Err(_) => break None,
            Ok(None) => {}
        }
        if ended.is_empty() && cancel.load(Ordering::Relaxed) {
            ended = "cancelled";
        } else if ended.is_empty() && start.elapsed() > timeout {
            ended = "timeout";
        }
        match (ended, killed_at) {
            ("", _) => {}
            (_, None) => {
                stop_group(&mut child, false);
                killed_at = Some(Instant::now());
            }
            (_, Some(at)) if at.elapsed() > Duration::from_secs(2) => stop_group(&mut child, true),
            _ => {}
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    for r in readers.into_iter().flatten() {
        _ = r.join();
    }
    let output = String::from_utf8_lossy(&out.lock().unwrap_or_else(|e| e.into_inner())).into_owned();
    let code = if ended.is_empty() { status.and_then(|s| s.code()) } else { None };
    Ran { code, output, ended }
}

/// Ask a task's process group to end (`hard`: kill it).
fn stop_group(child: &mut std::process::Child, hard: bool) {
    #[cfg(unix)]
    {
        let sig = if hard { libc::SIGKILL } else { libc::SIGTERM };
        if let Ok(pid) = i32::try_from(child.id()) {
            // SAFETY: a signal to the group this task leads (process_group(0)).
            unsafe { libc::kill(-pid, sig) };
        }
    }
    #[cfg(not(unix))]
    {
        _ = hard;
        _ = child.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_processes_cpu_and_memory_are_read_from_proc() {
        // `cat` with a space and parentheses in its name.
        let stat = "42 (my (app) x) S 1 42 42 0 -1 4194560 100 0 0 0 250 50 0 0 20 0 1 0 1000 1000 200 ...";
        assert_eq!(proc_stat(stat, "5000 300 100 1 0 200 0"), Some((3.0, 10.0, 300)));
        assert_eq!(proc_stat("garbage", "1 2"), None);
    }

    /// amui's `psStats: a dead pid yields null (never a NaN sample)`.
    #[test]
    fn a_dead_pid_reads_as_nothing_never_a_nan() {
        assert_eq!(ps_stats(u32::MAX - 7), None);
        if cfg!(target_os = "linux") {
            let me = ps_stats(std::process::id()).expect("this process reads");
            assert!(me.cpu_pct.is_finite() && me.mem_mb > 0.0, "{me:?}");
        }
    }

    /// amui's `runtimeOf: the runtime is read off the executable`.
    #[test]
    fn the_runtime_is_read_off_the_executable() {
        let (proj, inst) = (Path::new("/w/notes"), Path::new("/h/aio-rt/install"));
        let dev = runtime_of(Some(Path::new("/t/x86_64-unknown-linux-gnu/desktop-dev/notes")), Some(proj), inst);
        assert_eq!((dev.kind, dev.root.as_path()), ("dev", proj));
        let rel = runtime_of(Some(Path::new("/w/notes/target/release/notes")), None, inst);
        assert_eq!((rel.kind, rel.root.as_path()), ("release", Path::new("/w/notes/target/release")));
        let i = runtime_of(Some(Path::new("/h/aio-rt/install/notes/notes")), Some(proj), inst);
        assert_eq!((i.kind, i.root.as_path()), ("installed", Path::new("/h/aio-rt/install/notes")));
        let img = runtime_of(Some(Path::new("/tmp/.mount_notesAb1/usr/bin/notes")), Some(proj), inst);
        assert_eq!((img.kind, img.root.as_path()), ("appimage", Path::new("/tmp/.mount_notesAb1")));
        let none = runtime_of(None, Some(proj), inst);
        assert_eq!((none.kind, none.root.as_path()), ("unknown", proj));
    }

    /// amui's `listFiles: walks the tree, ignores deps/junk, reports truncation`.
    #[test]
    fn the_tree_lists_folders_first_and_leaves_out_deps_junk_and_secrets() {
        let d = tempfile::tempdir().unwrap();
        for f in ["src/lib.rs", "aio.toml", "target/debug/x", ".git/HEAD", "data.db", "Cargo.lock", ".ssh/id", "a/b.rs"]
        {
            std::fs::create_dir_all(d.path().join(f).parent().unwrap()).unwrap();
            std::fs::write(d.path().join(f), "x").unwrap();
        }
        let (nodes, cut) = list_files(d.path());
        let paths: Vec<&str> = nodes.iter().map(|n| n.path.as_str()).collect();
        assert_eq!(paths, ["a", "a/b.rs", "src", "src/lib.rs", "aio.toml"]);
        assert!(!cut && nodes[0].dir && !nodes[4].dir);
        for i in 0..FILE_CAP + 5 {
            std::fs::write(d.path().join(format!("f{i:05}.rs")), "").unwrap();
        }
        let (nodes, cut) = list_files(d.path());
        assert!(cut && nodes.len() == FILE_CAP, "capped: {}", nodes.len());
    }

    /// amui's `readFile: reads within the project, blocks path traversal` and
    /// `secret dirs are never read`.
    #[test]
    fn a_file_is_read_only_inside_its_project_and_never_a_secret_or_a_blob() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("app");
        std::fs::create_dir_all(p.join("src")).unwrap();
        std::fs::create_dir_all(p.join(".ssh")).unwrap();
        std::fs::write(p.join("src/lib.rs"), "pub fn x() {}").unwrap();
        std::fs::write(p.join(".ssh/id_rsa"), "KEY").unwrap();
        std::fs::write(p.join("blob.bin"), [1u8, 0, 2]).unwrap();
        std::fs::write(d.path().join("outside.txt"), "no").unwrap();
        assert_eq!(read_file(&p, "src/lib.rs").unwrap(), "pub fn x() {}");
        assert_eq!(read_file(&p, "../outside.txt").unwrap_err(), "path outside project");
        assert_eq!(read_file(&p, "/etc/hostname").unwrap_err(), "path outside project");
        assert!(read_file(&p, ".ssh/id_rsa").unwrap_err().starts_with("refusing to read"));
        assert!(read_file(&p, "blob.bin").unwrap_err().starts_with("binary file"));
        assert_eq!(read_file(&p, "src").unwrap_err(), "is a directory");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(d.path().join("outside.txt"), p.join("link.txt")).unwrap();
            assert_eq!(read_file(&p, "link.txt").unwrap_err(), "path outside project", "no symlink out");
        }
        let big = std::fs::File::create(p.join("big.txt")).unwrap();
        big.set_len(VIEW_MAX + 1).unwrap();
        assert!(read_file(&p, "big.txt").unwrap_err().starts_with("too large to view"));
    }

    /// amui's `$HOME is never a repo root`.
    #[test]
    fn the_home_folder_is_never_a_repo_root() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("home");
        let app = home.join("code/mono/apps/notes");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::create_dir_all(home.join(".git")).unwrap();
        assert_eq!(find_repo_root(&app, &home), None, "a home in git is not the app's repo");
        std::fs::create_dir_all(home.join("code/mono/.git")).unwrap();
        assert_eq!(find_repo_root(&app, &home), Some(home.join("code/mono")));
        assert_eq!(repo_candidates(Path::new("/a/b/c/d/e/f/g/h"), Path::new("/x"), 6).len(), 6);
    }

    /// amui's `findCellSource: locates the cell definition, not references`.
    #[test]
    fn a_cells_source_is_its_struct_not_a_mention() {
        let text = "use x::Counter; // Counter is used here\n\n#[aio::cell(persist, id = \"board\")]\n/// Doc.\npub struct Notes {\n}\n\n#[cell(persist)]\npub struct TodoList {}\n";
        assert_eq!(cell_line(text, "board"), Some(5));
        assert_eq!(cell_line(text, "todo_list"), Some(9));
        assert_eq!(cell_line(text, "notes"), None, "its id is board");
        assert_eq!(cell_line(text, "counter"), None, "a mention is no definition");
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("src")).unwrap();
        std::fs::write(d.path().join("src/lib.rs"), text).unwrap();
        assert_eq!(find_cell_source(d.path(), "board"), Some(("src/lib.rs".into(), 5)));
    }

    /// amui's `readLogs: prefers the app's own log dir` and its freshness order.
    #[test]
    fn logs_are_tailed_by_source_the_freshest_capture_first() {
        let d = tempfile::tempdir().unwrap();
        let logs = d.path();
        assert!(read_logs(logs, LogSource::Combined, 10).missing);
        std::fs::write(
            logs.join("app.log"),
            "2026-11-01T10:00:00.000Z [aio] up\n2026-11-01T10:00:01.000Z WARN  [page] slow\n  at frame\n\x1b[31m2026-11-01T10:00:02.000Z ERROR [aio] boom\x1b[0m\n",
        )
        .unwrap();
        std::fs::write(logs.join("start.log"), "[aio] notes starting\n").unwrap();
        let r = read_logs(logs, LogSource::Combined, 10);
        assert_eq!(
            (r.path.as_deref(), r.lines.len()),
            (Some(logs.join("start.log").as_path()), 1),
            "this run's capture"
        );
        let app = read_logs(logs, LogSource::App, 2);
        assert_eq!(app.lines, ["  at frame", "2026-11-01T10:00:02.000Z ERROR [aio] boom"], "colour codes out");
        assert!(app.truncated);
        let client = read_logs(logs, LogSource::Client, 10);
        assert_eq!(client.lines, ["2026-11-01T10:00:01.000Z WARN  [page] slow", "  at frame"]);
        assert!(read_logs(logs, LogSource::Error, 10).missing);
    }

    /// amui's `runTask: cancel via signal terminates promptly (never hangs)`
    /// and its timeout.
    #[cfg(unix)]
    #[test]
    fn a_task_is_captured_and_cancel_or_its_cap_ends_it() {
        let d = tempfile::tempdir().unwrap();
        let sh = Path::new("/bin/sh");
        let no = Arc::new(AtomicBool::new(false));
        let w = |s: &str| vec!["-c".to_string(), s.to_string()];
        let buf = Arc::new(std::sync::Mutex::new(Vec::new()));
        let ok = run_task(d.path(), sh, &w("echo out; echo err >&2; exit 3"), &no, TASK_TIMEOUT, &buf);
        assert_eq!(String::from_utf8_lossy(&buf.lock().unwrap()), ok.output, "as it came");
        assert_eq!((ok.code, ok.ended), (Some(3), ""));
        assert!(ok.output.contains("out") && ok.output.contains("err"), "{}", ok.output);
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            c.store(true, Ordering::Relaxed);
        });
        let t = Instant::now();
        let r = run_task(d.path(), sh, &w("sleep 30 & sleep 30"), &cancel, TASK_TIMEOUT, &Arc::default());
        assert_eq!((r.code, r.ended), (None, "cancelled"));
        assert!(t.elapsed() < Duration::from_secs(5), "promptly: {:?}", t.elapsed());
        let r = run_task(d.path(), sh, &w("sleep 30"), &no, Duration::from_millis(100), &Arc::default());
        assert_eq!(r.ended, "timeout");
        let r = run_task(d.path(), Path::new("/no/such/program"), &[], &no, TASK_TIMEOUT, &Arc::default());
        assert!(r.output.starts_with("failed to spawn"));
    }
}

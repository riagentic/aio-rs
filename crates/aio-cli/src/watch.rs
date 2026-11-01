//! What `ax dev` watches (aio's `server/server-watcher.ts`): the app's
//! folders, and the files its last build read that lie outside them (cargo's
//! dep-info: `include_str!` files, a path dependency such as an aio-rs
//! checkout). A burst of saves is one change: 100 ms of quiet, 500 ms at
//! most. A sentinel file is touched every 30 s; a watcher that heard nothing
//! for 60 s went deaf and is made again (three times at most).
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

/// A burst of saves ends after this much quiet…
pub const DEBOUNCE: Duration = Duration::from_millis(100);
/// …or this long after its first save: a writer that never pauses (a
/// formatter, a generator, a `git checkout`) must not hold a rebuild off.
pub const DEBOUNCE_MAX: Duration = Duration::from_millis(500);
/// How often the sentinel is touched, and how long a watcher may hear nothing.
const TOUCH: Duration = Duration::from_secs(30);
const DEAF: Duration = Duration::from_secs(60);
const REVIVALS: u32 = 3;

/// The wait for the next save of a burst begun `waited` ago (pure).
pub fn debounce(waited: Duration) -> Duration {
    DEBOUNCE.min(DEBOUNCE_MAX.saturating_sub(waited))
}

/// Wait out the burst `first` began: every path it saved, each once, sorted.
pub fn settle(rx: &mpsc::Receiver<Vec<PathBuf>>, first: Vec<PathBuf>) -> Vec<PathBuf> {
    let start = Instant::now();
    let mut all: BTreeSet<PathBuf> = first.into_iter().collect();
    loop {
        let got = match debounce(start.elapsed()) {
            d if d.is_zero() => rx.try_recv().ok(),
            d => rx.recv_timeout(d).ok(),
        };
        match got {
            Some(more) => all.extend(more),
            None => return all.into_iter().collect(),
        }
    }
}

/// The changed files as a person names them: relative to `cwd`, and three
/// at most, then how many more (pure; aio's `describeChanged`).
pub fn describe(paths: &[PathBuf], cwd: &Path) -> String {
    let rel: Vec<String> =
        paths.iter().map(|p| p.strip_prefix(cwd).unwrap_or(p).display().to_string().replace('\\', "/")).collect();
    match rel.len() {
        0 => "the app".into(),
        1..=3 => rel.join(", "),
        n => format!("{} +{} more", rel[..3].join(", "), n - 3),
    }
}

/// The files a cargo dep-info file names (`<bin>.d`: `out: dep dep…`, a
/// space in a path escaped `\ `), none under `skip` (pure).
pub fn dep_info(text: &str, skip: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = BTreeSet::new();
    for line in text.lines() {
        let Some((_, deps)) = line.split_once(": ") else { continue };
        let mut word = String::new();
        let mut chars = deps.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\\' if chars.peek() == Some(&' ') => word.extend(chars.next()),
                ' ' => _ = out.insert(std::mem::take(&mut word)),
                c => word.push(c),
            }
        }
        out.insert(word);
    }
    out.into_iter()
        .filter(|w| !w.is_empty())
        .map(PathBuf::from)
        .filter(|p| !skip.iter().any(|s| p.starts_with(s)))
        .collect()
}

/// What no app's edit is: cargo's and rustup's own sources, and `target`.
pub fn not_the_apps(target: &Path) -> Vec<PathBuf> {
    let home = || std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from);
    let var = |k: &str, dflt: &str| std::env::var_os(k).map(PathBuf::from).or_else(|| home().map(|h| h.join(dflt)));
    [var("CARGO_HOME", ".cargo"), var("RUSTUP_HOME", ".rustup"), Some(target.to_path_buf())]
        .into_iter()
        .flatten()
        .collect()
}

struct Shared {
    tx: mpsc::Sender<Vec<PathBuf>>,
    /// Watched whole.
    roots: Vec<PathBuf>,
    /// Single files, each by its folder (an editor's save replaces the file).
    files: Mutex<BTreeSet<PathBuf>>,
    sentinel: Option<PathBuf>,
    /// When the watcher last said anything (ms).
    heard: AtomicI64,
}

/// A watcher on the app: a message on each real change, the paths changed.
pub struct Watch {
    shared: Arc<Shared>,
    w: Arc<Mutex<Option<notify::RecommendedWatcher>>>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

impl Watch {
    /// A watcher on `roots` (those there), with its health sentinel.
    pub fn new(roots: &[PathBuf]) -> Result<(Self, mpsc::Receiver<Vec<PathBuf>>), String> {
        static N: AtomicU64 = AtomicU64::new(0);
        let (tx, rx) = mpsc::channel();
        let sentinel = (!roots.is_empty())
            .then(|| {
                let n = N.fetch_add(1, Relaxed);
                let dir = std::env::temp_dir().join(format!("ax-watch-{}-{n}", std::process::id()));
                std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(dir.join("sentinel"), "")).ok()?;
                Some(dir.join("sentinel"))
            })
            .flatten();
        let shared = Arc::new(Shared {
            tx,
            roots: roots.to_vec(),
            files: Mutex::default(),
            sentinel,
            heard: AtomicI64::new(now_ms()),
        });
        let w = Arc::new(Mutex::new(Some(make(&shared)?)));
        if shared.sentinel.is_some() {
            let (s, w) = (Arc::downgrade(&shared), Arc::downgrade(&w));
            std::thread::spawn(move || health(s, w));
        }
        Ok((Watch { shared, w }, rx))
    }

    /// Watch `files` too (those outside the roots and there).
    pub fn add_files(&self, files: impl IntoIterator<Item = PathBuf>) {
        let mut known = self.shared.files.lock().unwrap_or_else(|e| e.into_inner());
        let dirs: BTreeSet<PathBuf> = known.iter().filter_map(|f| f.parent().map(Path::to_path_buf)).collect();
        let mut w = self.w.lock().unwrap_or_else(|e| e.into_inner());
        for f in files {
            if self.shared.roots.iter().any(|r| f.starts_with(r)) || !f.is_file() || !known.insert(f.clone()) {
                continue;
            }
            if let (Some(dir), Some(w)) = (f.parent(), w.as_mut())
                && !dirs.contains(dir)
            {
                _ = notify::Watcher::watch(w, dir, notify::RecursiveMode::NonRecursive);
            }
        }
    }

    /// Make the watcher again, on all it watched.
    #[cfg(test)]
    pub fn revive(&self) -> Result<(), String> {
        revive(&self.shared, &self.w)
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        if let Some(dir) = self.shared.sentinel.as_ref().and_then(|s| s.parent()) {
            _ = std::fs::remove_dir_all(dir);
        }
    }
}

fn revive(s: &Arc<Shared>, w: &Mutex<Option<notify::RecommendedWatcher>>) -> Result<(), String> {
    let mut w = w.lock().unwrap_or_else(|e| e.into_inner());
    *w = None;
    *w = Some(make(s)?);
    s.heard.store(now_ms(), Relaxed);
    Ok(())
}

/// Touch the sentinel; make a deaf watcher again. Ends with its [`Watch`].
fn health(s: std::sync::Weak<Shared>, w: std::sync::Weak<Mutex<Option<notify::RecommendedWatcher>>>) {
    let mut revived = 0;
    loop {
        std::thread::sleep(TOUCH);
        let (Some(s), Some(w)) = (s.upgrade(), w.upgrade()) else { return };
        if let Some(f) = &s.sentinel {
            _ = std::fs::write(f, now_ms().to_string());
        }
        if now_ms() - s.heard.load(Relaxed) <= DEAF.as_millis() as i64 {
            continue;
        }
        if revived == REVIVALS {
            eprintln!("ax dev: the file watcher still hears nothing after {REVIVALS} restarts: restart ax dev");
            return;
        }
        revived += 1;
        eprintln!("ax dev: the file watcher went deaf: starting it again ({revived}/{REVIVALS})");
        if let Err(e) = revive(&s, &w) {
            eprintln!("ax dev: the file watcher: {e}");
        }
    }
}

fn make(s: &Arc<Shared>) -> Result<notify::RecommendedWatcher, String> {
    let on = s.clone();
    let mut w = notify::recommended_watcher(move |e: notify::Result<notify::Event>| {
        let Ok(e) = e else { return };
        on.heard.store(now_ms(), Relaxed);
        if !crate::dev::changes(&e.kind) {
            return;
        }
        let files = on.files.lock().unwrap_or_else(|e| e.into_inner());
        let ps: Vec<PathBuf> = e
            .paths
            .into_iter()
            .filter(|p| Some(p) != on.sentinel.as_ref() && crate::dev::relevant(p))
            .filter(|p| on.roots.iter().any(|r| p.starts_with(r)) || files.contains(p))
            .collect();
        if !ps.is_empty() {
            _ = on.tx.send(ps);
        }
    })
    .map_err(|e| e.to_string())?;
    use notify::{RecursiveMode::*, Watcher};
    for root in s.roots.iter().filter(|p| p.exists()) {
        w.watch(root, Recursive).map_err(|e| format!("{}: {e}", root.display()))?;
    }
    let files = s.files.lock().unwrap_or_else(|e| e.into_inner());
    let dirs: BTreeSet<&Path> =
        files.iter().filter_map(|f| f.parent()).chain(s.sentinel.iter().filter_map(|f| f.parent())).collect();
    for dir in dirs {
        _ = w.watch(dir, NonRecursive);
    }
    Ok(w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_waits_100_ms_of_quiet_and_500_ms_at_most() {
        // aio's DEBOUNCE_MS, DEBOUNCE_MAX_MS.
        assert_eq!(debounce(Duration::ZERO), Duration::from_millis(100));
        assert_eq!(debounce(Duration::from_millis(450)), Duration::from_millis(50));
        assert_eq!(debounce(Duration::from_millis(900)), Duration::ZERO);
        // A writer that never pauses: the burst still ends.
        let (tx, rx) = mpsc::channel();
        let writer = std::thread::spawn(move || {
            for i in 0..200 {
                if tx.send(vec![PathBuf::from(format!("/w/f{}", i % 3))]).is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        let t = Instant::now();
        let got = settle(&rx, vec![PathBuf::from("/w/f0")]);
        assert!(t.elapsed() < Duration::from_millis(900), "{:?}", t.elapsed());
        assert_eq!(got, ["/w/f0", "/w/f1", "/w/f2"].map(PathBuf::from), "each once");
        drop(rx);
        writer.join().unwrap();
    }

    #[test]
    fn changed_files_are_named_short() {
        let p = |s: &str| PathBuf::from(s);
        assert_eq!(describe(&[p("/w/src/lib.rs")], Path::new("/w")), "src/lib.rs");
        assert_eq!(describe(&[p("/x/a.rs")], Path::new("/w")), "/x/a.rs");
        let five: Vec<PathBuf> = (0..5).map(|i| p(&format!("/w/{i}.rs"))).collect();
        assert_eq!(describe(&five, Path::new("/w")), "0.rs, 1.rs, 2.rs +2 more");
        assert_eq!(describe(&[], Path::new("/w")), "the app");
    }

    #[test]
    fn dep_info_names_the_files_a_build_read_but_not_cargos_own() {
        let d = "/t/x/desktop-dev/notes: /w/src/main.rs /w/aio.toml /w/my\\ notes/a.txt /h/.cargo/registry/s/lib.rs /t/out/gen.rs\n\n/w/src/main.rs:\n";
        let skip = [PathBuf::from("/h/.cargo"), PathBuf::from("/t")];
        assert_eq!(dep_info(d, &skip), ["/w/aio.toml", "/w/my notes/a.txt", "/w/src/main.rs"].map(PathBuf::from));
        // Windows: the drive's colon is no separator.
        let w = "C:\\t\\notes.exe: C:\\w\\src\\main.rs";
        assert_eq!(dep_info(w, &[]), [PathBuf::from("C:\\w\\src\\main.rs")]);
    }

    /// The paths heard until one is `p` (10 s at most).
    fn until(rx: &mpsc::Receiver<Vec<PathBuf>>, p: &Path) -> Vec<PathBuf> {
        let mut heard = vec![];
        while !heard.iter().any(|h: &PathBuf| h == p) {
            heard.extend(rx.recv_timeout(Duration::from_secs(10)).unwrap_or_else(|_| panic!("{p:?}: {heard:?}")));
        }
        heard
    }

    #[test]
    fn it_hears_its_folders_and_the_files_it_was_given_and_again_once_revived() {
        let d = tempfile::tempdir().unwrap();
        let (src, other) = (d.path().join("src"), d.path().join("other"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("notes.txt"), "a").unwrap();
        let (w, rx) = Watch::new(std::slice::from_ref(&src)).unwrap();
        w.add_files([other.join("notes.txt"), other.join("gone.txt")]);
        std::fs::write(other.join("unwatched.txt"), "a").unwrap();
        std::fs::write(src.join("4913"), "vim's probe").unwrap();
        std::fs::write(src.join("lib.rs"), "x").unwrap();
        let heard = until(&rx, &src.join("lib.rs"));
        assert!(heard.iter().all(|p| p == &src.join("lib.rs")), "not vim's probe, not another file: {heard:?}");
        std::fs::write(other.join("notes.txt"), "b").unwrap();
        until(&rx, &other.join("notes.txt"));
        w.revive().unwrap();
        std::fs::write(src.join("lib.rs"), "y").unwrap();
        until(&rx, &src.join("lib.rs"));
        let sentinel = w.shared.sentinel.clone().unwrap();
        std::fs::write(&sentinel, "1").unwrap();
        std::fs::write(src.join("lib.rs"), "z").unwrap();
        assert!(!until(&rx, &src.join("lib.rs")).contains(&sentinel), "the sentinel is no change");
        drop(w);
        assert!(!sentinel.exists(), "removed with its watcher");
    }
}

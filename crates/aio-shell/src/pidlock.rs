//! aio's pid lock (`server/pid-lock.ts`), the one aio and aio-rs both take
//! to install a runtime into the shared store: `<lock>` holds the bare pid,
//! `<lock>.id` `<pid>-<tail>` (pid namespace and start stamp, [`own_tail`]),
//! and both change only under the mutex `<lock>.mx`, an exclusive OS lock
//! (flock, LockFileEx: what Deno's `FsFile.lockSync` takes). A holder that
//! died is taken over, only while the lock still names it; a release
//! removes only a lock that still names us.
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};
use std::sync::{LazyLock, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime};

/// How often a holder refreshes its lock's (and stage's) mtime.
pub const HEARTBEAT: Duration = Duration::from_secs(20);
/// How long a lock from another pid namespace may sit unchanged, watched by
/// this process, before its holder counts as dead.
pub const STALE: Duration = Duration::from_secs(120);
/// How long a waiter tries the mutex before it blocks on it.
const MUTEX_WAIT: Duration = Duration::from_secs(2);
/// How long a stage whose pid proves nothing may sit untouched.
pub const STAGE_STALE: Duration = Duration::from_secs(3600);

/// A lock's owner: pid, and what its `.id` recorded.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcId {
    pub pid: u32,
    /// Linux ticks or a Windows creation time, decimal.
    pub token: Option<String>,
    /// macOS: the start, in seconds since the epoch.
    pub epoch: Option<u64>,
    /// The writer's pid namespace (Linux).
    pub ns: Option<u32>,
}

fn with(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    s.into()
}

fn read(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok()
}

/// This process's pid namespace (Linux), like aio's `ownPidNs`.
pub fn own_ns() -> Option<u32> {
    static NS: OnceLock<Option<u32>> = OnceLock::new();
    *NS.get_or_init(|| {
        let link = std::fs::read_link("/proc/self/ns/pid").ok()?;
        let s = link.to_string_lossy();
        let n: u64 = s.split_once('[')?.1.strip_suffix(']')?.parse().ok()?;
        (n > 0 && n <= 0xffff_ffff).then_some(n as u32)
    })
}

/// What the OS says of when `pid` started: Linux ticks or a Windows
/// creation time (a token), macOS seconds (an epoch).
fn identity(pid: u32) -> (Option<String>, Option<u64>) {
    let Some(s) = aio::lifecycle::started(pid) else { return (None, None) };
    match std::env::consts::OS {
        "macos" => (None, s.split('.').next().and_then(|e| e.parse().ok())),
        _ => (Some(s), None),
    }
}

/// A start token as its tag (pure): `a<hex>`, or `c<hex>` past a double's
/// exact range (a Windows creation time), as aio's `tokenTag`.
pub fn token_tag(token: &str) -> String {
    let n: u128 = token.parse().unwrap_or(0);
    if n > (1u128 << 53) - 1 { format!("c{n:x}") } else { format!("a{n:x}") }
}

/// `d<ns, 8 hex>` then the start stamp, as aio's `ownIdTail` (pure).
pub fn tail_of(ns: Option<u32>, token: Option<&str>, epoch: Option<u64>) -> String {
    let ns = ns.map_or(String::new(), |n| format!("d{n:08x}"));
    let at = match (token, epoch) {
        (Some(t), _) => token_tag(t),
        (None, Some(e)) => format!("e{e:x}"),
        _ => String::new(),
    };
    ns + &at
}

/// This process's tail.
pub fn own_tail() -> String {
    let (token, epoch) = identity(std::process::id());
    tail_of(own_ns(), token.as_deref(), epoch)
}

/// A tail read back for `pid` (pure), as aio's `withTag`.
pub fn with_tag(pid: u32, tail: &str) -> ProcId {
    let hex = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    let ns = tail.get(1..9).filter(|h| tail.starts_with('d') && hex(h)).and_then(|h| u32::from_str_radix(h, 16).ok());
    let rest = if ns.is_some() { &tail[9..] } else { tail };
    let mut id = ProcId { pid, ns, ..ProcId::default() };
    let (kind, digits) = (rest.chars().next(), rest.get(1..).unwrap_or(""));
    if !hex(digits) {
        return id;
    }
    match (kind, u128::from_str_radix(digits, 16)) {
        (Some('c'), Ok(n)) => id.token = Some(n.to_string()),
        (Some('e'), Ok(n)) => id.epoch = u64::try_from(n).ok(),
        (Some('a'), Ok(n)) if n < 1 << 53 => id.token = Some(n.to_string()),
        _ => {}
    }
    id
}

/// The owner a lock's text (and its `.id`, used only when it names the same
/// pid) records; `None`: not a pid (pure), as aio's `lockOwner`.
pub fn lock_owner(raw: &str, id: Option<&str>) -> Option<ProcId> {
    let pid: u32 = raw.trim().parse().ok().filter(|p| *p > 0)?;
    let id = id.unwrap_or("").trim();
    let mut parts = id.split('-');
    Some(match (parts.next().and_then(|p| p.parse::<u32>().ok()), parts.next()) {
        (Some(p), tail) if p == pid => with_tag(pid, tail.unwrap_or("")),
        _ => ProcId { pid, ..ProcId::default() },
    })
}

fn foreign(id: &ProcId) -> bool {
    id.ns.is_some() && id.ns != own_ns()
}

/// Is the process `id` names still the one it recorded? A stamp known on
/// both sides and different says no; anything unknown, the pid decides.
pub fn owner_alive(id: &ProcId) -> bool {
    if !aio::lifecycle::alive(id.pid) {
        return false;
    }
    let (token, epoch) = identity(id.pid);
    let differs = |a: &Option<String>, b: Option<String>| a.as_ref().is_some_and(|a| b.is_some_and(|b| *a != b));
    !(differs(&id.token, token) || id.epoch.is_some_and(|e| epoch.is_some_and(|n| n != e)))
}

fn seen() -> MutexGuard<'static, HashMap<PathBuf, (String, Instant)>> {
    static SEEN: LazyLock<Mutex<HashMap<PathBuf, (String, Instant)>>> = LazyLock::new(Mutex::default);
    SEEN.lock().unwrap_or_else(|e| e.into_inner())
}

/// Has the lock at `p` sat unchanged for [`STALE`] while this process
/// watched it? A first sight or a change starts over; gone: stale.
fn observed_stale(p: &Path) -> bool {
    let Ok(m) = std::fs::metadata(p).and_then(|m| m.modified()) else { return true };
    let sig = format!("{m:?}|{:?}|{:?}", read(p), read(&with(p, ".id")));
    let key = std::path::absolute(p).unwrap_or_else(|_| p.into());
    let mut seen = seen();
    match seen.get(&key) {
        Some((s, at)) if *s == sig => at.elapsed() >= STALE,
        _ => {
            seen.insert(key, (sig, Instant::now()));
            false
        }
    }
}

/// aio's `holderAlive`: another namespace's holder lives until watched
/// stale; our own pid only while this process holds it (`ours`).
fn holder_alive(id: &ProcId, ours: bool, lock: &Path) -> bool {
    if foreign(id) {
        return !observed_stale(lock);
    }
    if id.pid == std::process::id() { ours } else { owner_alive(id) }
}

/// Windows says another handle has it, or macOS that its folder went
/// while the open was in it: a holder leaving, tried again a moment later.
fn leaving(e: &io::Error) -> bool {
    (cfg!(windows) && aio::paths::held_open(e)) || (cfg!(target_os = "macos") && e.raw_os_error() == Some(22))
}

fn pause() {
    std::thread::sleep(Duration::from_millis(1 + u64::from(std::process::id() % 3)));
}

/// Is `f` (open) the file at `p` right now?
fn same_file(f: &File, p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let (Ok(mine), Ok(at)) = (f.metadata(), std::fs::metadata(p)) else { return false };
        !(mine.nlink() == 0 && at.nlink() != 0) && mine.ino() == at.ino() && mine.dev() == at.dev()
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle};
        let info = |f: &File| {
            // SAFETY: a plain C struct of integers: all zeroes is valid.
            let mut i: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
            // SAFETY: `f` is an open handle; `i` the buffer it fills.
            let ok = unsafe { GetFileInformationByHandle(f.as_raw_handle(), &mut i) } != 0;
            ok.then_some((i.nFileIndexHigh, i.nFileIndexLow, i.dwVolumeSerialNumber, i.nNumberOfLinks))
        };
        let (Some(mine), Some(at)) = (info(f), File::open(p).ok().as_ref().and_then(info)) else { return false };
        !(mine.3 == 0 && at.3 != 0) && mine.0 == at.0 && mine.1 == at.1 && mine.2 == at.2
    }
    #[cfg(not(any(unix, windows)))]
    {
        _ = (f, p);
        true
    }
}

/// Run `f` holding `<lock>.mx`, as aio's `withLockMutexAt` (`recreate`
/// false): `None` when the lock's folder is gone. The file is unlinked
/// before it is let go, so a waiter that locked an unlinked one starts over.
pub fn with_mutex<T>(lock: &Path, f: impl FnOnce() -> T) -> io::Result<Option<T>> {
    let mx = with(lock, ".mx");
    let deadline = Instant::now() + MUTEX_WAIT;
    let (mut denied, mut lost) = (None, 0);
    let file = loop {
        let file = match OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&mx) {
            Ok(file) => file,
            Err(e) if leaving(&e) && Instant::now() < *denied.get_or_insert_with(|| Instant::now() + MUTEX_WAIT) => {
                pause();
                continue;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        denied = None;
        loop {
            match file.try_lock() {
                Ok(()) => break,
                Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => pause(),
                Err(std::fs::TryLockError::WouldBlock) => break file.lock()?,
                Err(std::fs::TryLockError::Error(e)) => return Err(e),
            }
        }
        if same_file(&file, &mx) {
            break file;
        }
        lost += 1;
        if lost >= 1000 {
            return Err(io::Error::other(format!(
                "cannot take the lock mutex {}: after {lost} tries the file locked was never the one at that path",
                mx.display()
            )));
        }
    };
    let out = f();
    _ = std::fs::remove_file(&mx);
    drop(file);
    Ok(Some(out))
}

/// Refresh `p`'s mtime now (both times: aio's `utimeSync(p, now, now)`).
pub fn touch(p: &Path) -> io::Result<()> {
    let mut o = OpenOptions::new();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_WRITE_ATTRIBUTES, and FILE_FLAG_BACKUP_SEMANTICS for a folder.
        o.access_mode(0x100).custom_flags(0x0200_0000);
    }
    #[cfg(not(windows))]
    o.read(true);
    let now = SystemTime::now();
    o.open(p)?.set_times(std::fs::FileTimes::new().set_accessed(now).set_modified(now))
}

/// Refresh `p` every [`HEARTBEAT`] until the returned sender is dropped, or
/// `ours` says it no longer is.
pub fn keep_fresh(p: PathBuf, ours: impl Fn() -> bool + Send + 'static) -> Sender<()> {
    let (stop, beat) = channel::<()>();
    std::thread::spawn(move || {
        while let Err(std::sync::mpsc::RecvTimeoutError::Timeout) = beat.recv_timeout(HEARTBEAT) {
            if !ours() {
                return;
            }
            _ = touch(&p);
        }
    });
    stop
}

/// A lock this process holds: its heartbeat, and the `.id` it wrote.
type Held = HashMap<PathBuf, (Sender<()>, Option<String>)>;

/// The locks this process holds.
fn held() -> MutexGuard<'static, Held> {
    static HELD: LazyLock<Mutex<Held>> = LazyLock::new(Mutex::default);
    HELD.lock().unwrap_or_else(|e| e.into_inner())
}

fn key(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.into())
}

fn still_mine(lock: &Path, id: &Option<String>) -> bool {
    read(lock) == Some(std::process::id().to_string()) && read(&with(lock, ".id")) == *id
}

/// One go at `lock`, as aio's `tryPidLock`: (taken, try again at once: a
/// dead holder's lock was just removed). Never an error: a folder that
/// can't be written is a lock that can't be taken.
pub fn try_lock(lock: &Path) -> (bool, bool) {
    let mut id = None;
    let took = with_mutex(lock, || {
        let Ok(mut f) = OpenOptions::new().write(true).create_new(true).open(lock) else { return false };
        if io::Write::write_all(&mut f, std::process::id().to_string().as_bytes()).is_err() {
            return false;
        }
        let me = format!("{}-{}", std::process::id(), own_tail());
        id = std::fs::write(with(lock, ".id"), &me).ok().map(|()| me);
        true
    });
    if let Ok(Some(true)) = took {
        let (l, i) = (lock.to_path_buf(), id.clone());
        let beat = keep_fresh(lock.into(), move || still_mine(&l, &i));
        held().insert(key(lock), (beat, id));
        return (true, false);
    }
    let (raw, id_raw) = (read(lock), read(&with(lock, ".id")));
    let ours = held().contains_key(&key(lock));
    let owner = match raw.as_deref().and_then(|r| lock_owner(r, id_raw.as_deref())) {
        Some(o) if !holder_alive(&o, ours, lock) => o,
        _ => return (false, false),
    };
    let gone = with_mutex(lock, || {
        if read(lock) != raw || read(&with(lock, ".id")) != id_raw || (foreign(&owner) && !observed_stale(lock)) {
            return;
        }
        _ = std::fs::remove_file(with(lock, ".id"));
        _ = std::fs::remove_file(lock);
    });
    (false, matches!(gone, Ok(Some(()))))
}

/// Let go of a lock [`try_lock`] took, only while it still names us.
pub fn release(lock: &Path) -> io::Result<()> {
    let mine = held().remove(&key(lock));
    with_mutex(lock, || {
        if let Some((_, id)) = &mine
            && still_mine(lock, id)
        {
            _ = std::fs::remove_file(with(lock, ".id"));
            _ = std::fs::remove_file(lock);
        }
    })
    .map(|_| ())
}

/// Hold `lock` for `f`, waiting up to `wait` for its holder (`on_wait`
/// each round; false gives up), as aio's `withRuntimeLock`: `f` is told
/// whether it was had.
pub fn with_lock<T>(lock: &Path, wait: Duration, on_wait: &mut dyn FnMut() -> bool, f: impl FnOnce(bool) -> T) -> T {
    if let Some(d) = lock.parent() {
        _ = std::fs::create_dir_all(d);
    }
    let end = Instant::now() + wait;
    let mut had = false;
    while !had && Instant::now() < end {
        let (took, retry) = try_lock(lock);
        had = took;
        if !had && !retry {
            if !on_wait() {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    let out = f(had);
    if had && let Err(e) = release(lock) {
        eprintln!(
            "[aio] could not release {} ({e}): the next start takes it over once this process exits",
            lock.display()
        );
    }
    out
}

/// A stage's name after `<dir>.incoming.`: `<pid>-<nonce><tail>`, as aio's
/// `stageTag` (all hex after the `-`).
pub fn stage_tag() -> String {
    format!("{}-{}{}", std::process::id(), &aio::uuid_v4().simple().to_string()[..8], own_tail())
}

/// Remove the `<dir>.incoming.<tag>` stages whose unpack is gone, as aio's
/// `removeDeadStages`: its pid is dead here, or, when the pid proves
/// nothing, it sat untouched for [`STAGE_STALE`]. Called holding the lock.
pub fn remove_dead_stages(dir: &Path) {
    let (Some(parent), Some(name)) = (dir.parent(), dir.file_name()) else { return };
    let prefix = format!("{}.incoming.", name.to_string_lossy());
    for e in std::fs::read_dir(parent).into_iter().flatten().flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        let Some(rest) = n.strip_prefix(&prefix) else { continue };
        if !e.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        // `<pid>` or `<pid>-<hex>`, nothing else.
        let hex = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        let (pid, more) = match rest.split_once('-') {
            Some((p, m)) if hex(m) => (p, m),
            Some(_) => continue,
            None => (rest, ""),
        };
        let Some(pid) = pid.bytes().all(|b| b.is_ascii_digit()).then(|| pid.parse::<u32>().ok()).flatten() else {
            continue;
        };
        let id = with_tag(pid, more.get(8..).unwrap_or(""));
        if id.pid == std::process::id() || id.ns != own_ns() || owner_alive(&id) {
            let quiet = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
            if quiet.is_none_or(|q| q < STAGE_STALE) {
                continue;
            }
        }
        _ = std::fs::remove_dir_all(e.path());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // aio tests/pid-lock-start-tag.test.ts, value for value.
    const FILETIME: &str = "134360879030934133";

    #[test]
    fn tails_are_aios() {
        assert_eq!(token_tag("1234567"), "a12d687");
        let c = token_tag(FILETIME);
        assert!(c.starts_with('c'));
        assert_eq!(with_tag(7, &c), ProcId { pid: 7, token: Some(FILETIME.into()), ..ProcId::default() });
        assert_eq!(with_tag(7, "a12d687").token.as_deref(), Some("1234567"));
        assert_eq!(
            with_tag(7, "d0000002aa12d687"),
            ProcId { pid: 7, token: Some("1234567".into()), ns: Some(42), ..ProcId::default() }
        );
        assert_eq!(with_tag(7, "e6ab3c1f9").epoch, Some(0x6ab3c1f9));
        // An older aio's rounded Windows stamp is no stamp: the pid decides.
        let rounded = format!("a{:x}", FILETIME.parse::<f64>().unwrap() as u128);
        assert_eq!(with_tag(7, &rounded), ProcId { pid: 7, ..ProcId::default() });
        assert_eq!(tail_of(Some(42), Some("1234567"), None), "d0000002aa12d687");
        assert_eq!(tail_of(None, None, Some(0x6ab3c1f9)), "e6ab3c1f9");
        assert_eq!(tail_of(None, None, None), "");
        // What aio's lockOwner reads.
        assert_eq!(lock_owner("12", Some("12-a12d687")).unwrap().token.as_deref(), Some("1234567"));
        assert_eq!(lock_owner("12", Some("13-a12d687")), Some(ProcId { pid: 12, ..ProcId::default() }));
        assert_eq!(lock_owner("x", None), None);
        assert_eq!(lock_owner("0", None), None);
    }

    #[test]
    fn this_process_names_itself() {
        let id = with_tag(std::process::id(), &own_tail());
        assert!(owner_alive(&id));
        assert_eq!(id.ns, own_ns());
        if cfg!(target_os = "linux") {
            assert!(id.token.is_some() && id.ns.is_some());
        }
        // Another start under the same pid is not the owner.
        let other = ProcId { token: Some("1".into()), epoch: id.epoch.map(|e| e + 1), ..id.clone() };
        if id.token.is_some() || id.epoch.is_some() {
            assert!(!owner_alive(&ProcId { token: id.token.as_ref().map(|_| "1".into()), ..other }));
        }
    }

    #[test]
    fn the_lock_is_aios_shape_held_once_and_released_only_by_its_holder() {
        let t = tempfile::tempdir().unwrap();
        let lock = t.path().join("44.9.3-linux-x64.lock");
        assert_eq!(try_lock(&lock), (true, false));
        assert_eq!(std::fs::read_to_string(&lock).unwrap(), std::process::id().to_string());
        assert_eq!(
            std::fs::read_to_string(with(&lock, ".id")).unwrap(),
            format!("{}-{}", std::process::id(), own_tail())
        );
        // Held here: a second try waits.
        assert_eq!(try_lock(&lock), (false, false));
        release(&lock).unwrap();
        let left: Vec<_> = std::fs::read_dir(t.path()).unwrap().flatten().map(|e| e.file_name()).collect();
        assert!(left.is_empty(), "{left:?}");
        // A dead holder's lock is taken over; a live one's is not.
        std::fs::write(&lock, "999999999").unwrap();
        assert_eq!(try_lock(&lock), (false, true));
        assert_eq!(try_lock(&lock), (true, false));
        // Taken over meanwhile: our release leaves the new holder's alone.
        std::fs::write(&lock, "1").unwrap();
        release(&lock).unwrap();
        assert_eq!(std::fs::read_to_string(&lock).unwrap(), "1");
        // Our own pid, not held by this process: a previous run's, taken over.
        std::fs::write(&lock, std::process::id().to_string()).unwrap();
        assert!(with_lock(&lock, Duration::from_secs(5), &mut || true, |had| had && lock.exists()));
        assert!(!lock.exists() && !with(&lock, ".id").exists());
        // A live holder (this test's parent) is waited for, never taken over.
        #[cfg(unix)]
        {
            std::fs::write(&lock, std::os::unix::process::parent_id().to_string()).unwrap();
            let mut asked = 0;
            let mut on_wait = || {
                asked += 1;
                asked < 2
            };
            assert!(!with_lock(&lock, Duration::from_secs(5), &mut on_wait, |had| had));
            assert_eq!(asked, 2);
            assert_eq!(std::fs::read_to_string(&lock).unwrap(), std::os::unix::process::parent_id().to_string());
        }
    }

    #[test]
    fn a_dead_unpacks_stage_goes_a_live_ones_stays() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("44.9.3-linux-x64");
        // A dead pid, in our namespace (one naming none where one exists: its age decides).
        let ns = tail_of(own_ns(), None, None);
        let dead = t.path().join(format!("44.9.3-linux-x64.incoming.999999999-0badcafe{ns}"));
        let unknown = t.path().join("44.9.3-linux-x64.incoming.999999998-0badcafe");
        let mine = t.path().join(format!("44.9.3-linux-x64.incoming.{}", stage_tag()));
        let other = t.path().join("45.0.0-linux-x64.incoming.999999999-0badcafe");
        for d in [&dead, &mine, &other, &unknown] {
            std::fs::create_dir_all(d).unwrap();
        }
        remove_dead_stages(&dir);
        assert!(!dead.exists() && mine.exists() && other.exists());
        assert_eq!(unknown.exists(), own_ns().is_some());
    }

    #[test]
    fn a_heartbeat_touches_and_the_mutex_runs_its_work_once() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("x");
        std::fs::write(&f, "").unwrap();
        let old = SystemTime::now() - Duration::from_secs(3600);
        File::options().write(true).open(&f).unwrap().set_modified(old).unwrap();
        touch(&f).unwrap();
        assert!(std::fs::metadata(&f).unwrap().modified().unwrap() > old);
        touch(t.path()).unwrap();
        assert_eq!(with_mutex(&f, || 7).unwrap(), Some(7));
        assert!(!with(&f, ".mx").exists());
        assert_eq!(with_mutex(&t.path().join("gone/x"), || 7).unwrap(), None);
    }
}

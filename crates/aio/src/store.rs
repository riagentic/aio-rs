//! One cell's state over SQLite: the event log is the truth, snapshots make
//! boot and time travel fast. Boot = the newest snapshot + the events after it.
use crate::cx::{Asks, Plan};
use crate::{By, Cell, Cx, Inputs, Rejected, Result};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fmt, path::Path, sync::Arc, time::SystemTime};

/// A snapshot every this many events.
const SNAP_EVERY: u32 = 256;
const REWIND: &str = "$rewind";
/// A boot's repair of the saved state (`restored`), logged so replay and
/// synced copies follow it.
const RESTORE: &str = "$restore";
/// A `#[redact]` call's logged arguments: `{REDACTED: <the state it made>}`
/// (without its `#[transient]` fields), in place of its own
/// (`Cell::REDACTED`). No method's own arguments are an object (always a
/// tuple or null), so none can pose as one.
pub const REDACTED: &str = "$redacted";
/// A transaction's logged arguments (`#[cell(transaction)]`): `{TX: <the
/// state it left>, "args": <its call's arguments>}`, replayed as that state
/// (without its `#[transient]` fields), as a `#[redact]` call's is.
pub const TX: &str = "$tx";

mod shared;
mod tx;
use shared::Db;
pub(crate) use shared::{Hold, Member, Shared, Then, hold, release};
pub(crate) use tx::On as TxOn;

/// One committed call.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    pub method: String,
    pub args: Value,
    pub inputs: Inputs,
    pub by: By,
    /// ms since the Unix epoch.
    pub at: i64,
    /// The cell version that wrote it.
    pub v: u32,
}

/// A step through the history (aio time travel): each is a rewind, a new
/// event, so nothing is lost and every step can be taken back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Travel {
    /// To the state the current one was made on.
    Undo,
    /// The next step toward where the trailing undos began.
    Redo,
    /// To the state right after event `seq`.
    To(u64),
}

/// Where the cell is in its history: the event its state is right after
/// (a rewind's target, followed), the newest call, and the oldest state
/// kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    pub at: u64,
    pub head: u64,
    pub floor: u64,
}

/// A cell's log as time travel reads it: each of its events' seq, and for
/// a rewind the event it went back to. One sequence numbers every cell's
/// events, so the cell's state after any number is the one after its own
/// newest event at or before it.
pub type Steps = [(u64, Option<u64>)];

/// The cell's newest event at or before `seq` (0: none).
fn at_or_before(log: &Steps, seq: u64) -> u64 {
    log.iter().rev().map(|(s, _)| *s).find(|s| *s <= seq).unwrap_or(0)
}

/// The cell's event before `seq` (0: none).
fn before(log: &Steps, seq: u64) -> u64 {
    at_or_before(log, seq.saturating_sub(1))
}

/// The event whose state `seq`'s is: a rewind's is the one it went back
/// to, followed.
fn real(log: &Steps, seq: u64) -> u64 {
    let mut seq = at_or_before(log, seq);
    while let Some(to) = log.iter().find(|(s, _)| *s == seq).and_then(|(_, to)| *to) {
        seq = at_or_before(log, to);
    }
    seq
}

impl Position {
    /// Where the cell with this log stands, its history kept from `floor` (pure).
    pub fn of(log: &Steps, floor: u64) -> Self {
        let last = log.last().map_or(0, |(s, _)| *s);
        let head = log.iter().rev().find(|(_, to)| to.is_none()).map_or(0, |(s, _)| *s);
        Position { at: real(log, last), head, floor }
    }

    /// The event `t` goes to on the cell's `log` (pure); `Err` says why not.
    /// Undo: the state the current one was made on. Redo: the next step
    /// toward where the trailing undos began; a new event after them leaves
    /// nothing to redo.
    pub fn target(log: &Steps, floor: u64, t: Travel) -> std::result::Result<u64, String> {
        let last = log.last().map_or(0, |(s, _)| *s);
        let now = real(log, last);
        let kept = |to: u64| match to < floor {
            true => Err(format!("nothing earlier is kept (the history starts at event {floor})")),
            false => Ok(to),
        };
        match t {
            Travel::Undo if now == 0 => Err("nothing to undo: the state is the cell's start".into()),
            Travel::Undo => kept(real(log, before(log, now))),
            Travel::Redo => {
                let first = log.iter().rev().take_while(|(_, to)| to.is_some()).last().map(|(s, _)| *s);
                let Some(first) = first else {
                    return Err("nothing to redo: no undo is the last thing that happened".into());
                };
                // From the head back: each state, then the one it was made on.
                let mut path = vec![real(log, before(log, first))];
                while let Some(&x) = path.last().filter(|x| **x > 0) {
                    path.push(real(log, before(log, x)));
                }
                match path.iter().position(|x| *x == now) {
                    Some(i) if i > 0 => Ok(path[i - 1]),
                    _ => Err("nothing to redo: this is where the undos began".into()),
                }
            }
            Travel::To(n) if (floor..=last).contains(&n) => Ok(n),
            Travel::To(n) => Err(format!("no event {n} to go to (kept: {floor} to {last})")),
        }
    }
}

/// A store that can't open, boot or answer about the past.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Self(format!("storage: {e}"))
    }
}

pub struct Store<T: Cell> {
    state: Arc<T>,
    seq: u64,
    /// Its connection, and its file's shared one.
    db: Db,
    unsnapped: u32,
    effects: Vec<Box<dyn std::any::Any + Send>>,
    /// Schedules due on another cell (`cx.on::<O>()`), fired by the handle.
    foreign: Vec<Due>,
    /// Events committed since the runtime last took them, each with the
    /// cell's event before it (for synced copies to spot a gap).
    committed: Vec<(u64, Event)>,
    /// Events committed that the cells listening to it have not reacted
    /// to yet; `None`: nobody listens.
    to_react: Option<Vec<Event>>,
    /// The joint commit it last wrote in, and the store as it was before.
    mark: Option<(u64, Mark<T>)>,
    /// The file and its identity at open: a commit checks it is still there
    /// (D5: on Unix a deleted file takes writes silently, then they're lost).
    file: Option<(std::path::PathBuf, FileId)>,
    /// Set when this open had to recover from a damaged file.
    recovered: Option<Recovered>,
    /// When methods panicked lately (the third within a minute stops the cell).
    panics: Vec<std::time::Instant>,
    /// Failures since the runtime last took them: a method that panicked, or
    /// a refused call of the cell's own schedules and effects: who called,
    /// and `method: why`.
    failures: Vec<(By, String)>,
    /// A test's clock (`TestCells`); none: the wall clock.
    clock: Option<std::sync::Arc<std::sync::atomic::AtomicI64>>,
    /// An aio file (`open_aio`): every commit also writes the cell into `aio_kv`.
    aio_kv: Option<crate::compat::data::Kv>,
    /// This run's newest calls: their event and the method's own time
    /// (the state's copy, the method, its invariants), for time travel.
    took: std::collections::VecDeque<(u64, std::time::Duration)>,
    /// A run of `#[coalesce]` calls in the state but not yet in the log.
    held: Option<Held<T>>,
    /// What is saved of the committed state, kept from its commit so the
    /// next one needn't serialize it again; `None`: not known.
    saved: Option<Value>,
    /// `[history] keep` in ms: events older are folded (`keep_history`).
    keep: Option<i64>,
    /// How long a run of `#[coalesce]` calls is held (`[data] coalesce_ms`).
    coalesce: std::time::Duration,
    /// aio's journal watermark the next commit writes (an aio journal's
    /// entry replayed at boot), in the transaction with what it did.
    journal_wm: Option<i64>,
    /// Booted on a newer build's data (a rollback): its version and its saved
    /// state, whose fields this build doesn't declare every snapshot keeps,
    /// stamped with that version (never lowered: rolling forward again
    /// migrates nothing twice).
    ahead: Option<(u32, Value)>,
    /// `#[cell(transaction)]`: its async calls in flight.
    txs: tx::Txs<T>,
    /// Synced clients' calls their method or the invariants refused, by
    /// `op`, newest last ([`REFUSED_OPS`]): a re-send is refused the same,
    /// never applied on a later state.
    refused: (std::collections::HashMap<String, Rejected>, std::collections::VecDeque<String>),
}

/// The refused calls a store remembers (`Store::refused`).
const REFUSED_OPS: usize = 4_096;

/// A run of coalesced calls by one caller: each call (the last one is the
/// run's one event when it alone ends where the run did), the state before
/// the run, and when the run began.
#[derive(Clone)]
struct Held<T> {
    by: By,
    /// Each call's method, arguments and time, in order.
    calls: Vec<(String, Value, i64)>,
    base: Arc<T>,
    since: std::time::Instant,
}

/// A store as a joint commit found it ([`Store::mark`]).
pub(crate) struct Mark<T> {
    state: Arc<T>,
    seq: u64,
    unsnapped: u32,
    effects: usize,
    foreign: usize,
    committed: usize,
    to_react: usize,
    held: Option<Held<T>>,
    journal_wm: Option<i64>,
}

/// The most calls a run holds: a longer one is flushed and starts again.
const HELD_MAX: usize = 1_000;

/// How long after its first call a run of coalesced calls goes into the
/// log (aio's `persistDebounceMs`: its state is shown at once, the disk is
/// written once per window; a crash loses at most this much, as in aio). A
/// call that can't wait, a time-travel step or the close flushes it at once.
pub const PERSIST_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(100);

/// Calls [`Store::took`] remembers.
pub const TOOK: usize = 2_000;

const PANICS_MAX: usize = 3;
const PANIC_WINDOW: std::time::Duration = std::time::Duration::from_secs(60);

/// How a store came back from a damaged file (D4), for the app to tell its
/// user: what was set aside, and from when the data is.
#[derive(Clone, Debug, PartialEq)]
pub struct Recovered {
    /// The damaged file, kept beside the store (never deleted).
    pub quarantined: std::path::PathBuf,
    /// What the check found.
    pub why: String,
    /// The good copy's time (ms since the epoch): changes after it are lost.
    pub from: i64,
}

type OnRecovered = std::sync::Arc<dyn Fn(&str, &Recovered) + Send + Sync>;

static ON_RECOVERED: std::sync::RwLock<Vec<OnRecovered>> = std::sync::RwLock::new(Vec::new());

/// Hear each store that came back from a damaged file as it opens (aio's
/// `onCheckpointRestore`), as its cell and what was set aside: tell the
/// user from when their data is. Add it before the stores open; hooks
/// compose, and one that panics is logged, never stops the boot.
pub fn on_recovered(f: impl Fn(&str, &Recovered) + Send + Sync + 'static) {
    ON_RECOVERED.write().unwrap_or_else(|e| e.into_inner()).push(std::sync::Arc::new(f));
}

fn told_recovered(cell: &str, r: &Recovered) {
    let hooks = ON_RECOVERED.read().unwrap_or_else(|e| e.into_inner()).clone();
    for f in hooks {
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(cell, r))).is_err() {
            eprintln!("[aio] an on_recovered hook panicked on {cell}; the boot goes on");
        }
    }
}

/// A good copy is made at boot when the last one is older than this.
const GOOD_EVERY: std::time::Duration = std::time::Duration::from_secs(86_400);

fn good_copy(path: &Path) -> std::path::PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(".good");
    p.into()
}

/// D6: temp files a crash left beside the store (a half-made good copy, a
/// half-restored store, a half-written `write_atomic` file) are removed. A
/// `write_atomic` file under a minute old may be another thread's, mid-write
/// (the health file): it stays, for the next start.
pub(crate) fn sweep(path: &Path) {
    let (dir, name) =
        (crate::paths::parent(path), path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let f = e.file_name().to_string_lossy().into_owned();
        let old = || {
            e.metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > std::time::Duration::from_secs(60))
        };
        let stray = f == format!("{name}.good.tmp")
            || f == format!("{name}.restoring")
            || (f.starts_with('.') && f.ends_with(".tmp") && old());
        if stray && std::fs::remove_file(e.path()).is_ok() {
            eprintln!("[aio] storage: removed {f}, left by a crash");
        }
    }
}

/// `PRAGMA quick_check` on the file at `path` (fine if it doesn't exist yet).
pub(crate) fn healthy(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let db =
        Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| e.to_string())?;
    let rows: Vec<String> = db
        .prepare("PRAGMA quick_check")
        .and_then(|mut q| q.query_map([], |r| r.get(0))?.collect())
        .map_err(|e| e.to_string())?;
    if rows == ["ok"] { Ok(()) } else { Err(rows.join("; ")) }
}

/// Set the damaged store aside and put the good copy in its place.
pub(crate) fn recover(path: &Path, why: String) -> Result<Recovered, Error> {
    let good = good_copy(path);
    let Ok(meta) = std::fs::metadata(&good) else {
        return Err(Error(format!(
            "{} is damaged ({why}) and has no good copy to boot from; it is kept as is",
            path.display()
        )));
    };
    let from = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64);
    let mut quarantined = path.as_os_str().to_owned();
    quarantined.push(format!(".damaged-{}", now_ms()));
    let quarantined: std::path::PathBuf = quarantined.into();
    for ext in ["", "-wal", "-shm"] {
        let (mut a, mut b) = (path.as_os_str().to_owned(), quarantined.as_os_str().to_owned());
        a.push(ext);
        b.push(ext);
        if Path::new(&a).exists() {
            std::fs::rename(&a, &b).map_err(|e| Error(format!("can't set {} aside: {e}", path.display())))?;
        }
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".restoring");
    std::fs::copy(&good, &tmp)
        .and_then(|_| crate::paths::sync_file(Path::new(&tmp)))
        .and_then(|()| crate::paths::rename_over(Path::new(&tmp), path))
        .map_err(|e| Error(format!("can't restore {}: {e}", path.display())))?;
    crate::paths::sync_dir(crate::paths::parent(path)).map_err(|e| Error(e.to_string()))?;
    eprintln!(
        "[aio] storage: {} was damaged ({why}); it is kept as {} and the app runs on its good copy: changes after it are lost",
        path.display(),
        quarantined.display()
    );
    Ok(Recovered { quarantined, why, from })
}

/// Refresh the good copy when it's missing or a day old: `VACUUM INTO` a
/// temp file, flushed, renamed over.
pub(crate) fn refresh_good(db: &Connection, path: &Path) -> Result<(), Error> {
    let good = good_copy(path);
    let fresh = std::fs::metadata(&good)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < GOOD_EVERY);
    if fresh {
        return Ok(());
    }
    let mut tmp = good.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp: std::path::PathBuf = tmp.into();
    _ = std::fs::remove_file(&tmp);
    db.execute("VACUUM INTO ?1", [tmp.to_string_lossy()])?;
    crate::paths::sync_file(&tmp)
        .and_then(|()| crate::paths::rename_over(&tmp, &good))
        .map_err(|e| Error(format!("good copy: {e}")))?;
    crate::paths::sync_dir(crate::paths::parent(path)).map_err(|e| Error(e.to_string()))
}

/// Which file a path names now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileId(u64, u64);

fn file_id(path: &Path) -> Option<FileId> {
    let m = std::fs::metadata(path).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(FileId(m.dev(), m.ino()))
    }
    #[cfg(not(unix))]
    {
        Some(FileId(0, u64::from(m.is_file())))
    }
}

/// A savepoint: a transaction of its own, or, inside a joint commit
/// ([`open_joint`]), a part of it. Dropped before `commit`, it is undone.
struct Atomic<'a>(&'a Connection, bool);

impl<'a> Atomic<'a> {
    fn new(db: &'a Connection) -> rusqlite::Result<Self> {
        db.execute_batch("SAVEPOINT aio")?;
        Ok(Self(db, false))
    }

    fn commit(mut self) -> rusqlite::Result<()> {
        self.0.execute_batch("RELEASE aio")?;
        self.1 = true;
        Ok(())
    }
}

impl std::ops::Deref for Atomic<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.0
    }
}

impl Drop for Atomic<'_> {
    fn drop(&mut self) {
        if !self.1 {
            _ = self.0.execute_batch("ROLLBACK TO aio; RELEASE aio");
        }
    }
}

/// A saved schedule.
#[derive(Clone, Debug, PartialEq)]
pub struct Due {
    pub key: String,
    pub due: i64,
    pub every: Option<i64>,
    /// A cron expression: the next slot is found from it.
    pub cron: Option<String>,
    pub method: String,
    pub args: Value,
}

pub(crate) fn now_ms() -> i64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// The layout of the tables below; stamped as SQLite's `user_version`.
/// 2: schedules may follow a cron expression (`sched.cron`).
pub(crate) const SCHEMA: i64 = 2;

/// The tables below, which an aio file must not already have.
const TABLES: [&str; 5] = ["log", "snap", "floor", "sched", "ops"];

/// Set up a store file, or refuse one that isn't ours: a file with other
/// tables (a prototype store) is never written to or wiped. `aio`: an aio
/// `state.db` (it has `aio_kv`) is adopted, its tables left as they are.
fn init(db: &Connection, aio_file: bool) -> Result<(), Error> {
    // Read-only checks first: a refused file must stay byte for byte.
    let v = ours(db, aio_file)?;
    db.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;
    let tx = Transaction::new_unchecked(db, rusqlite::TransactionBehavior::Immediate)?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS log (seq INTEGER PRIMARY KEY AUTOINCREMENT, cell TEXT NOT NULL, method TEXT NOT NULL,
           args TEXT NOT NULL, inputs TEXT NOT NULL, by TEXT NOT NULL, at INTEGER NOT NULL, v INTEGER NOT NULL);
         CREATE INDEX IF NOT EXISTS log_cell ON log (cell, seq);
         CREATE TABLE IF NOT EXISTS snap (cell TEXT NOT NULL, seq INTEGER NOT NULL, v INTEGER NOT NULL,
           state TEXT NOT NULL, PRIMARY KEY (cell, seq));
         CREATE TABLE IF NOT EXISTS floor (cell TEXT PRIMARY KEY, seq INTEGER NOT NULL);
         CREATE TABLE IF NOT EXISTS sched (cell TEXT NOT NULL, key TEXT NOT NULL, due INTEGER NOT NULL,
           every INTEGER, method TEXT NOT NULL, args TEXT NOT NULL, cron TEXT, PRIMARY KEY (cell, key));
         CREATE TABLE IF NOT EXISTS ops (cell TEXT NOT NULL, op TEXT NOT NULL, seq INTEGER NOT NULL,
           PRIMARY KEY (cell, op));",
    )?;
    // 1 → 2: one added column, in the same transaction as the stamp.
    if v == 1 {
        tx.execute_batch("ALTER TABLE sched ADD COLUMN cron TEXT")?;
    }
    if aio_file {
        tx.execute_batch(crate::compat::data::KV_TABLE)?;
    }
    tx.execute_batch(&format!("PRAGMA user_version = {SCHEMA}"))?;
    tx.commit()?;
    Ok(())
}

/// Read only: the file's layout stamp, when it is one to set up (new, aio-rs's,
/// or with `aio` an aio `state.db`), else why it isn't.
fn ours(db: &Connection, aio_file: bool) -> Result<i64, Error> {
    let v: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let tables: i64 = db.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'", [], |r| r.get(0))?;
    if v > SCHEMA {
        return Err(Error(format!("store from a newer aio-rs (schema {v} > {SCHEMA})")));
    }
    let has = |t: &str| -> Result<bool, Error> {
        Ok(db.query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            [t],
            |r| r.get(0),
        )?)
    };
    if v == 0 && tables > 0 {
        if !aio_file || !has("aio_kv")? {
            return Err(Error("not an aio-rs store (it has other tables): import it with `ax port --data`".into()));
        }
        for t in TABLES {
            if has(t)? {
                return Err(Error(format!(
                    "this aio store has a table `{t}` (an app table?), a name aio-rs keeps its history in; it is kept as is"
                )));
            }
        }
    }
    Ok(v)
}

/// A file opened for a cell: set aside first if damaged, and aio's slice
/// of it (its state and version).
struct Opened {
    db: Db,
    recovered: Option<Recovered>,
    aio_kv: Option<crate::compat::data::Kv>,
    seed: Option<(Value, u32)>,
}

/// A cell's snapshot: seq, version, state.
type Snap = (i64, u32, String);

/// A boot's first half, the same for every cell type (one copy, not one
/// per type): the file of cell `id` at `version` guarded, swept, set aside
/// when damaged (D4: never deleted; the good copy boots in its place,
/// without one the boot is refused) and opened, and aio's slice of it read
/// (or refused) before anything is written, the file kept before an upgrade.
fn open(
    path: &Path,
    app: Option<&str>,
    id: &str,
    version: u32,
    tables: &[crate::compat::data::Bound],
    slices: &[crate::compat::data::Slice],
) -> Result<Opened, Error> {
    crate::paths::guard(path).map_err(|e| Error(e.to_string()))?;
    // This build reads this cell's data: part of its contract.
    crate::update::opened(id, version);
    slices.iter().for_each(|s| crate::update::opened(s.cell, s.version));
    sweep(path);
    let recovered = match healthy(path) {
        Ok(()) => None,
        Err(why) => Some(recover(path, why)?),
    };
    let file = Db::open(path)?;
    let db = file.conn();
    let aio_kv = app.map(|a| crate::compat::data::Kv::find(&db, a)).transpose().map_err(Error)?;
    // Its declared `db:` tables (aio-rs's own file: once it is set up, `bind`),
    // on a file known as aio's: a new one gets aio's `aio_kv` first, and one
    // the boot would refuse gets nothing.
    if aio_kv.is_some() {
        ours(&db, true)?;
        db.execute_batch(crate::compat::data::KV_TABLE)?;
        let own = tables.iter().map(|t| (id, t));
        declare(&db, own.chain(slices.iter().flat_map(|s| s.tables.iter().map(|t| (s.cell, t)))))?;
    }
    // One cell, aio's: its slice; many (`#[aio_cell]` fields): each slice by field,
    // at this version, the first older one naming the backup.
    let (seed, older) = match &aio_kv {
        Some(kv) if !slices.is_empty() => match kv.slices(&db, slices).map_err(Error)? {
            Some((got, older)) => (Some((Value::Object(got), version)), older.map(|(c, v)| format!("v1-{c}-v{v}"))),
            None => (None, None),
        },
        Some(kv) => {
            let seed = kv.slice(&db, id, tables).map_err(Error)?;
            let older = seed.as_ref().filter(|(_, v)| *v < version).map(|(_, v)| format!("v1-v{v}"));
            (seed, older)
        }
        None => (None, None),
    };
    if let Some(older) = older {
        let bak = format!("{}.{older}.bak", path.display());
        if !Path::new(&bak).exists() {
            db.execute("VACUUM INTO ?1", [&bak])?;
        }
    }
    drop(db);
    Ok(Opened { db: file, recovered, aio_kv, seed })
}

/// Bound tables `(cell, binding)` made or brought to their declared shape
/// (aio's `initSchema`); never one of aio-rs's own tables.
fn declare<'a>(
    db: &Connection,
    tables: impl Iterator<Item = (&'a str, &'a crate::compat::data::Bound)>,
) -> Result<(), Error> {
    for (cell, &(field, table, _, cols)) in tables {
        if TABLES.contains(&table) {
            return Err(Error(format!("{cell}.{field}: table {table} is aio-rs's own; bind the field to another")));
        }
        if let Some(cols) = cols {
            let cols = cols.sql().map_err(|e| Error(format!("{cell}.{field}: table {table}: {e}")))?;
            crate::compat::data::declare(db, table, &cols).map_err(|e| Error(format!("{cell}.{field}: {e}")))?;
        }
    }
    Ok(())
}

/// aio-rs's own file, set up: cell `id`'s bound tables declared, and their rows
/// by field (changed by the app's own SQL, they come in as aio's changes do).
fn bind(
    db: &Connection,
    id: &str,
    version: u32,
    tables: &[crate::compat::data::Bound],
) -> Result<Option<(Value, u32)>, Error> {
    declare(db, tables.iter().map(|t| (id, t)))?;
    let got = crate::compat::data::bound(db, tables).map_err(Error)?;
    Ok((!got.is_empty()).then_some((Value::Object(got), version)))
}

/// A boot's second half, after aio's slice was read: the file set up, and
/// cell `id`'s newest snapshot (the file kept before an upgrade) and its
/// last logged step.
fn newest(db: &Connection, path: &Path, id: &str, version: u32, aio_file: bool) -> Result<(Option<Snap>, u64), Error> {
    init(db, aio_file)?;
    let base: Option<Snap> = db
        .query_row("SELECT seq, v, state FROM snap WHERE cell = ?1 ORDER BY seq DESC LIMIT 1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()?;
    if let Some((seq, v, _)) = &base
        && *v < version
    {
        let bak = format!("{}.v{v}-{seq}.bak", path.display());
        if !Path::new(&bak).exists() {
            db.execute("VACUUM INTO ?1", [&bak])?;
        }
    }
    let last: i64 = db.query_row("SELECT COALESCE(MAX(seq), 0) FROM log WHERE cell = ?1", [id], |r| r.get(0))?;
    Ok((base, last as u64))
}

/// The cells `T` retires (`#[cell(retire = [...])]`) gone from the file, in
/// one transaction: their history here and, in an aio file, their slices and
/// stamps. Zeroed, not only unlinked (`secure_delete`, the WAL emptied), so
/// a secret one held is gone from the file; never a cell still read here.
/// A cell of `#[aio_cell]` fields may retire its own id: aio's slice of that
/// name (an earlier build's, before it read aio's cells by field) and its
/// stamp go, its own history here stays.
fn retire<T: Cell>(db: &Connection, kv: Option<&crate::compat::data::Kv>) -> Result<(), Error> {
    if T::RETIRED.is_empty() {
        return Ok(());
    }
    let reads = |c: &str| (c == T::ID && T::AIO_CELLS.is_empty()) || T::AIO_CELLS.iter().any(|s| s.cell == c);
    if let Some(c) = T::RETIRED.iter().find(|c| reads(c)) {
        return Err(Error(format!("{}: it retires cell {c}, which it still reads", T::ID)));
    }
    erase(db, kv, T::RETIRED, T::ID).map(|_| ())
}

/// The history of `cells` gone from the file (and, in an aio file, their
/// slices and stamps), zeroed; whether there was any. `own`'s history is kept.
fn erase(db: &Connection, kv: Option<&crate::compat::data::Kv>, cells: &[&str], own: &str) -> Result<bool, Error> {
    let secure: i64 = db.query_row("PRAGMA secure_delete", [], |r| r.get(0))?;
    db.execute_batch("PRAGMA secure_delete = ON")?;
    let tx = Atomic::new(db)?;
    let mut gone = match kv {
        Some(kv) => kv.retire(&tx, cells).map_err(Error)?,
        None => false,
    };
    for t in TABLES {
        for c in cells.iter().filter(|c| **c != own) {
            gone |= tx.execute(&format!("DELETE FROM {t} WHERE cell = ?1"), [c])? > 0;
        }
    }
    tx.commit()?;
    if gone {
        db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
    }
    db.execute_batch(&format!("PRAGMA secure_delete = {secure}"))?;
    Ok(gone)
}

/// The journal beside aio's file at `path` past its watermark, as cell `T`
/// replays it ([`crate::compat::data::journal_tail`]).
fn journal_tail<T: Cell>(
    db: &Connection,
    kv: &crate::compat::data::Kv,
    path: &Path,
) -> Result<Vec<crate::compat::data::Journaled>, Error> {
    let data = crate::paths::parent(path);
    let wm = kv.journal_wm(db).map_err(Error)?;
    crate::compat::data::journal_tail::<T>(data, wm).map_err(|e| {
        Error(format!(
            "{}: {e}: start the aio app once and stop it (it folds its journal into the snapshot), then open it here",
            data.display()
        ))
    })
}

/// Saved state of version `v` as this version of the cell.
fn decode<T: Cell>(text: &str, v: u32) -> Result<T, Error> {
    let mut old: Value = serde_json::from_str(text).map_err(|e| Error(format!("{}: corrupt snapshot: {e}", T::ID)))?;
    // Written by an older build that booted on this data ([`DOWNGRADED`]).
    let downgraded = old.as_object_mut().and_then(|m| m.remove(DOWNGRADED));
    // A newer build's data (a rollback): what this build declares, read as it
    // is; the rest is kept by every write (`Store::ahead`, aio's downgrade).
    if v > T::VERSION {
        return serde_json::from_value(old).map_err(|e| {
            Error(format!(
                "{}: data from a newer build (v{v} > v{}) doesn't fit this build: {e}; run the newer build, or give this one a version past v{v} with a `migrate` that converts it down",
                T::ID,
                T::VERSION
            ))
        });
    }
    if v == T::VERSION {
        let st: T = serde_json::from_value(old.clone())
            .map_err(|e| Error(format!("{}: snapshot doesn't fit v{v}: {e}", T::ID)))?;
        // D9: data this build doesn't declare would be dropped by the next
        // snapshot. Never silently: refuse, and say what and how to fix it.
        let now = serde_json::to_value(&st).map_err(|e| Error(format!("{}: unserializable state: {e}", T::ID)))?;
        let lost = undeclared(&old, &now);
        // Back from a rollback (aio's roll-forward): the older build's own
        // fields go, as aio's restore merge drops them, said; the rest is ours.
        if !lost.is_empty() && downgraded.is_some() {
            eprintln!(
                "[aio] {}: an older build ran on this data and wrote fields this build doesn't declare ({}); they are dropped, as aio drops them",
                T::ID,
                lost.join(", ")
            );
            return Ok(st);
        }
        if !lost.is_empty() {
            return Err(Error(format!(
                "{}: the saved data has fields this build doesn't declare ({}); declare them again, or bump the cell's version with a `migrate` that drops them on purpose",
                T::ID,
                lost.join(", ")
            )));
        }
        return Ok(st);
    }
    T::migrate(old, v).map_err(|e| Error(format!("{}: migrate from v{v} failed: {e}", T::ID)))
}

/// The key a snapshot written by an older build booted on newer data
/// carries (that build's version): the newer build then drops the older
/// one's fields it doesn't declare instead of refusing them, as aio does.
pub(crate) const DOWNGRADED: &str = "__downgraded";

/// The file's data (saved by version `v`) as this cell: its aio slice
/// (`whole`), or only some fields (`#[aio_cell]` slices, bound tables' rows) over
/// `st`, its other fields kept.
fn adopt<T: Cell>(st: &T, slice: &Value, v: u32, whole: bool) -> Result<T, Error> {
    if whole {
        return decode(&slice.to_string(), v);
    }
    let mut all = json(st)?;
    if let (Some(m), Some(s)) = (all.as_object_mut(), slice.as_object()) {
        m.extend(s.clone());
    }
    decode(&all.to_string(), v)
}

/// Paths in `old` that `new` (the same data decoded and encoded again) lost.
/// A `null` that went missing is no loss (an omitted `None`).
fn undeclared(old: &Value, new: &Value) -> Vec<String> {
    let at = |path: &[crate::patch::Step]| {
        path.iter().try_fold(old, |v, s| match s {
            crate::patch::Step::Key(k) => v.get(k),
            crate::patch::Step::Index(i) => v.get(i),
        })
    };
    crate::patch::diff(old, new)
        .into_iter()
        .filter_map(|op| match op {
            crate::patch::Op::Del { path } if !at(&path).is_some_and(Value::is_null) => Some(
                path.iter()
                    .map(|s| match s {
                        crate::patch::Step::Key(k) => k.clone(),
                        crate::patch::Step::Index(i) => i.to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join("."),
            ),
            _ => None,
        })
        .collect()
}

fn json<T: Cell>(st: &T) -> Result<Value, Error> {
    serde_json::to_value(st).map_err(|e| Error(format!("{}: unserializable state: {e}", T::ID)))
}

/// `v` with each value replaced by its JSON type (`"string"`, `"number"`,
/// `"bool"`, `"null"`), a list by the shape of its first item (pure).
pub fn shape_of(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(m.iter().map(|(k, v)| (k.clone(), shape_of(v))).collect()),
        Value::Array(a) => Value::Array(a.first().map(shape_of).into_iter().collect()),
        Value::String(_) => "string".into(),
        Value::Number(_) => "number".into(),
        Value::Bool(_) => "bool".into(),
        Value::Null => "null".into(),
    }
}

/// What is saved of `st` when its cell shapes it: `save` applied, the
/// `#[transient]` fields left out; `None`: all of it, as is. A panicking
/// `save` saves nothing: never a write that quietly didn't happen.
fn shaped<T: Cell>(st: &T) -> Result<Option<Value>, Error> {
    let saved = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| st.save()))
        .map_err(|_| Error(format!("{}: its `save` panicked; nothing was saved", T::ID)))?;
    if saved.is_none() && filters::<T>().next().is_none() {
        return Ok(None);
    }
    let mut v = json(saved.as_ref().unwrap_or(st))?;
    unsaved_out::<T>(&mut v);
    Ok(Some(v))
}

/// What it doesn't save (`#[transient]`, `persist(include, exclude)`): the
/// cell's own (`None`) and its `#[aio_cell]` slices' (`Some(field)`).
fn filters<T: Cell>() -> impl Iterator<Item = (Option<&'static str>, crate::filter::Filter)> {
    let own = crate::filter::Filter { include: T::PERSIST_INCLUDE, exclude: T::TRANSIENT };
    let slices = T::AIO_CELLS.iter().map(|s| (Some(s.field), crate::filter::Filter { include: s.include, exclude: s.transient }));
    std::iter::once((None, own)).chain(slices).filter(|(_, f)| !f.is_all())
}

/// The part of the state `v` a filter's `at` names.
fn part<'a>(v: &'a mut Value, at: Option<&str>) -> Option<&'a mut Value> {
    match at {
        None => Some(v),
        Some(f) => v.get_mut(f),
    }
}

/// `v` without what it doesn't save.
fn unsaved_out<T: Cell>(v: &mut Value) {
    for (at, f) in filters::<T>() {
        if let Some(p) = part(v, at) {
            crate::filter::strip(p, f);
        }
    }
}

/// `v` (saved) with what it doesn't save taken from `was` (a state).
fn unsaved_from<T: Cell>(v: &mut Value, was: &Value) {
    for (at, f) in filters::<T>() {
        let from = at.map_or(Some(was), |a| was.get(a));
        if let (Some(p), Some(from)) = (part(v, at), from) {
            *p = crate::filter::graft(p, from, f);
        }
    }
}

/// What a problem report shows of `st`: what is saved of it. During a
/// panic the cell's own `save` is not run (a second panic would abort the
/// process): its `#[transient]` fields are still left out.
/// Its `#[server]` fields are left out: a report may leave the machine.
pub(crate) fn reported<T: Cell>(st: &T) -> Option<Value> {
    let mut v = match std::thread::panicking() {
        false => shaped(st).ok()?.or_else(|| json(st).ok())?,
        true => json(st).ok()?,
    };
    unsaved_out::<T>(&mut v);
    if let Some(m) = v.as_object_mut() {
        T::SERVER.iter().for_each(|k| _ = m.remove(*k));
    }
    Some(v)
}

/// `st` as the debug log may show it: without its `#[server]` fields.
fn logged<T: Cell>(st: &T) -> Result<Value, Error> {
    let mut v = json(st)?;
    if let Some(m) = v.as_object_mut() {
        T::SERVER.iter().for_each(|k| _ = m.remove(*k));
    }
    Ok(v)
}

fn unshaped<T: Cell>(v: Value) -> Result<T, Error> {
    serde_json::from_value(v).map_err(|e| Error(format!("{}: its saved shape doesn't fit: {e}", T::ID)))
}

/// A snapshot of `st`: what is saved of it. `exact`: only when it brings
/// `st` back whole (`None` otherwise), so events after it replay on the
/// state they ran on. The close's snapshot needn't be: the next boot starts
/// from it, so it is what the next events ran on.
fn snapshot<T: Cell>(st: &T, exact: bool) -> Result<Option<String>, Error> {
    Ok(match shaped(st)? {
        None => Some(json(st)?.to_string()),
        Some(v) if !exact || unshaped::<T>(v.clone())? == *st => Some(v.to_string()),
        Some(_) => None,
    })
}

/// `st` repaired by its cell's `restore`. A panicking repair is reported and
/// `st` stands: a failed repair must not cost the app its boot.
fn repaired<T: Cell>(st: T) -> T {
    let keep = st.clone();
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| st.restore())).unwrap_or_else(|_| {
        eprintln!("[aio] {}: `restore` panicked; the state stays as saved", T::ID);
        keep
    })
}

/// `st` as it comes back from storage (every boot): what is saved of it,
/// repaired. However the app stopped, it comes back the same. A `save` that
/// panics is reported and `st` boots as it is: the log still holds it all.
fn restored<T: Cell>(st: &T) -> Result<T, Error> {
    Ok(repaired(match shaped(st) {
        Ok(Some(v)) => unshaped(v)?,
        Ok(None) => st.clone(),
        Err(e) => {
            eprintln!("[aio] {e}; the state boots as it is");
            st.clone()
        }
    }))
}

pub(crate) enum Applied {
    Refused(Rejected),
    Panicked(String),
}

/// `st.apply(…)`, with a panic caught: the caller gets it as a value,
/// and `st` (a copy the caller drops on error) is never kept half-done.
pub(crate) fn apply_guarded<T: Cell>(st: &mut T, cx: &Cx, method: &str, args: Value) -> Result<Value, Applied> {
    // Its state machine first: refused here, or moved to where it is going.
    st.advance(method).map_err(Applied::Refused)?;
    // In `ax dev`, through the hot-patch jump table: an edited method runs
    // its new code at once (a release build calls it directly).
    let run = || {
        #[cfg(feature = "dev")]
        if cfg!(debug_assertions) {
            return subsecond::HotFn::current(<T as Cell>::apply).call((st, cx, method, args));
        }
        st.apply(cx, method, args)
    };
    let run = || crate::report::during(|| format!("{}.{method}", T::ID), run);
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)) {
        Ok(r) => r.map_err(Applied::Refused),
        Err(p) => Err(Applied::Panicked(
            p.downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| p.downcast_ref::<String>().cloned())
                .unwrap_or_default(),
        )),
    }
}

impl From<Applied> for Rejected {
    fn from(a: Applied) -> Self {
        match a {
            Applied::Refused(r) => r,
            Applied::Panicked(why) => Rejected::new(format!("panicked: {why}")),
        }
    }
}

/// `st` after `ev`, computed again with the inputs it recorded: how a log
/// replays and how a synced copy follows its server. The event must commit
/// again with the same inputs, or the log and the code disagree.
pub fn apply_event<T: Cell>(st: &T, ev: &Event) -> Result<T, String> {
    if let Some(made) = made(&ev.args) {
        return unredacted(st, made, ev.v);
    }
    if ev.method == REWIND {
        return decode(&ev.args["state"].to_string(), ev.v).map(repaired).map_err(|e| e.0);
    }
    if ev.v != T::VERSION {
        // Old code wrote it; new code may compute something else.
        return Err(format!("it is from v{}, before the upgrade to v{}", ev.v, T::VERSION));
    }
    if ev.method == RESTORE {
        return restored(st).map_err(|e| e.0);
    }
    let cx = Cx::replay(ev.at, ev.inputs.clone(), &ev.by);
    let mut next = st.clone();
    apply_guarded(&mut next, &cx, &ev.method, ev.args.clone())
        .map_err(Rejected::from)
        .and_then(|_| next.check())
        .map_err(|e| format!("refused: {e}"))?;
    if cx.into_asks().inputs != ev.inputs {
        return Err("used other ids or random numbers than it recorded".into());
    }
    Ok(next)
}

/// A cell's machine fits its methods and every state is reachable (else no
/// store opens); a state with no way out is said.
pub(crate) fn machine_ok<T: Cell>() -> Result<(), Error> {
    let bad = T::machine_errors(T::METHODS);
    if !bad.is_empty() {
        return Err(Error(format!("{}: its machine is wrong: {}", T::ID, bad.join("; "))));
    }
    static SAID: std::sync::Mutex<Vec<&str>> = std::sync::Mutex::new(Vec::new());
    let ends = T::machine_ends();
    let mut said = SAID.lock().unwrap_or_else(|e| e.into_inner());
    if !ends.is_empty() && !said.contains(&T::ID) {
        said.push(T::ID);
        eprintln!("[aio] {}: its machine can't leave {} (an end state?)", T::ID, ends.join(", "));
    }
    Ok(())
}

/// What a `#[redact]` method's event keeps instead of its arguments: the
/// state it left, without its `#[transient]` fields, which it may therefore
/// not change (replay could not bring them back). `None`: not redacted.
fn redacted<T: Cell>(method: &str, before: &T, after: &T) -> Result<Option<Value>> {
    // An async method's steps (`unlock.1`) carry what it awaited: theirs too.
    if !T::redacts(method) {
        return Ok(None);
    }
    let (was, left) = (crate::json(before)?, crate::json(after)?);
    let mut kept = left.clone();
    unsaved_out::<T>(&mut kept);
    let mut back = kept.clone();
    unsaved_from::<T>(&mut back, &was);
    crate::ensure!(
        back == left,
        "{}.{method} is #[redact] and can't change what isn't saved (#[transient], persist(include, exclude)): its arguments aren't kept, so replay couldn't bring it back",
        T::ID
    );
    Ok(Some(serde_json::json!({ REDACTED: kept })))
}

/// The state a `#[redact]` event's args keep (see [`REDACTED`]).
fn made(args: &Value) -> Option<&Value> {
    let a = args.as_object()?;
    a.get(REDACTED).or_else(|| a.get(TX))
}

/// A `#[redact]` event replayed: the state it left, with the `#[transient]`
/// fields it could not change.
fn unredacted<T: Cell>(st: &T, made: &Value, v: u32) -> Result<T, String> {
    let left: T = decode(&made.to_string(), v).map_err(|e| e.0)?;
    let (mut v, was) = (json(&left).map_err(|e| e.0)?, json(st).map_err(|e| e.0)?);
    unsaved_from::<T>(&mut v, &was);
    unshaped(v).map_err(|e| e.0)
}

/// Apply logged events after `from` up to `to` onto `st`. Each must commit
/// again with the same inputs, or the log and the code disagree.
/// Apply the log's events `from..=to` to `st`. `upgrading` (boot after a
/// version bump): events an older build wrote after its last snapshot (a
/// crash left them unsnapped) run with this build's code on the migrated
/// state; one that no longer applies refuses the boot (D10).
fn replay<T: Cell>(db: &Connection, st: &mut T, from: u64, to: u64, upgrading: bool) -> Result<(), Error> {
    let mut q = db.prepare(
        "SELECT seq, method, args, inputs, at, v, by FROM log WHERE cell = ?1 AND seq > ?2 AND seq <= ?3 ORDER BY seq",
    )?;
    let rows = q.query_map(params![T::ID, from as i64, to as i64], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get(4)?,
            r.get(5)?,
            r.get::<_, String>(6)?,
        ))
    })?;
    for row in rows {
        let (seq, method, args, inputs, at, v, by): (i64, String, String, String, i64, u32, String) = row?;
        let bad = |what: String| Error(format!("{}: replay of event {seq} ({method}): {what}", T::ID));
        let args: Value = serde_json::from_str(&args).map_err(|e| bad(e.to_string()))?;
        let inputs: Inputs = serde_json::from_str(&inputs).map_err(|e| bad(e.to_string()))?;
        // The caller, as the method saw it (`Cx::by`).
        let by: By = serde_json::from_str(&by).map_err(|e| bad(e.to_string()))?;
        let older = upgrading && v < T::VERSION && method != REWIND && made(&args).is_none();
        let ev = Event {
            seq: seq as u64,
            method: method.clone(),
            args,
            inputs,
            by,
            at,
            v: if older { T::VERSION } else { v },
        };
        *st = apply_event(st, &ev).map_err(|e| {
            if older {
                bad(format!("written by v{v}, it doesn't apply to v{}: {e}; open the data once with the v{v} build to finish its log, then upgrade", T::VERSION))
            } else {
                bad(e)
            }
        })?;
    }
    Ok(())
}

impl<T: Cell> Store<T> {
    /// A store kept in memory only: for tests and cells without `persist`.
    pub fn memory() -> Self {
        Self::with(T::default())
    }

    /// An in-memory store starting from `state`.
    pub fn with(state: T) -> Self {
        // A wrong machine is a bug in the cell's code, said before any call.
        if let Err(e) = machine_ok::<T>() {
            panic!("{e}");
        }
        let db = Db::memory().expect("in-memory SQLite");
        init(&db.conn(), false).expect("in-memory schema");
        let mut s = Self {
            state: Arc::new(state),
            seq: 0,
            db,
            unsnapped: 0,
            effects: vec![],
            foreign: vec![],
            txs: Default::default(),
            refused: Default::default(),
            committed: vec![],
            to_react: None,
            mark: None,
            file: None,
            recovered: None,
            panics: vec![],
            failures: vec![],
            clock: None,
            aio_kv: None,
            took: Default::default(),
            held: None,
            saved: None,
            keep: None,
            coalesce: PERSIST_DEBOUNCE,
            journal_wm: None,
            ahead: None,
        };
        s.snap(true).expect("in-memory snapshot");
        s
    }

    #[cfg(feature = "testing")]
    /// A memory store booted from data saved by version `v` of the cell,
    /// as `open` boots it: refused when it doesn't fit (D9, D10), else
    /// migrated and restored (`#[transient]` fields back to their defaults,
    /// `restore` run). For tests (`TestCells::saved`).
    pub(crate) fn from_saved(text: &str, v: u32) -> Result<Self, Error> {
        Ok(Self::with(restored(&decode::<T>(text, v)?)?))
    }

    /// Open (or create) the store in the SQLite file at `path`. A version
    /// change backs the file up first; data that doesn't fit refuses to boot.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        Self::boot(path.as_ref(), None)
    }

    /// Open (or create) an aio app's `state.db` (docs/migration/data.md) for app
    /// `app`: aio-rs's tables go beside aio's, which stay in aio's shapes. The cell
    /// starts from aio's snapshot (either layout) at its `__versions` stamp,
    /// migrated like any older data, and each commit writes it back in aio's
    /// layout in the same transaction, so aio can open the file after. aio's
    /// data that changed since (aio ran) comes in as a new event.
    pub fn open_aio(path: impl AsRef<Path>, app: &str) -> Result<Self, Error> {
        Self::boot(path.as_ref(), Some(app))
    }

    fn boot(path: &Path, app: Option<&str>) -> Result<Self, Error> {
        machine_ok::<T>()?;
        // One boot at a time: an app's cells share a file, and one cell's
        // sweep must not take another's half-made good copy.
        static BOOT: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _one = BOOT.lock().unwrap_or_else(|e| e.into_inner());
        let Opened { db, recovered, aio_kv, seed } = open(path, app, T::ID, T::VERSION, T::TABLES, T::AIO_CELLS)?;
        if let Some(r) = &recovered {
            told_recovered(T::ID, r);
        }
        let c = db.conn();
        // aio's journal past its save, read (or refused) before anything is written.
        let tail = aio_kv.as_ref().map(|kv| journal_tail::<T>(&c, kv, path)).transpose()?.unwrap_or_default();
        // aio's data that doesn't fit refuses the boot before anything is written.
        let whole = aio_kv.is_some() && T::AIO_CELLS.is_empty();
        let seed = match aio_kv.is_some() {
            true => seed
                .map(|(raw, v)| Ok::<_, Error>((Some(adopt(&T::default(), &raw, v, whole)?), raw, v)))
                .transpose()?,
            false => seed.map(|(raw, v)| (None, raw, v)),
        };
        let (base, last) = newest(&c, path, T::ID, T::VERSION, aio_kv.is_some())?;
        retire::<T>(&c, aio_kv.as_ref())?;
        let seed = match (&aio_kv, T::TABLES.is_empty()) {
            (None, false) => bind(&c, T::ID, T::VERSION, T::TABLES)?.map(|(raw, v)| (None, raw, v)),
            _ => seed,
        };
        let (from, mut st, migrated) = match &base {
            None => (0, T::default(), false),
            Some((seq, v, text)) => (*seq as u64, decode::<T>(text, *v)?, *v < T::VERSION),
        };
        let ahead = base.filter(|(_, v, _)| *v > T::VERSION).map(|(_, v, text)| {
            eprintln!(
                "[aio] {}: its data is from a newer build (v{v} > v{}): it boots, and keeps that build's fields it doesn't declare; fields it declares may be misread. Roll forward, or give this build a version past v{v} with a `migrate` that converts it down",
                T::ID,
                T::VERSION
            );
            (v, serde_json::from_str(&text).unwrap_or(Value::Null))
        });
        let last = last.max(from);
        replay(&c, &mut st, from, last, true)?;
        drop(c);
        let back = restored(&st)?;
        let file = file_id(path).map(|id| (path.to_path_buf(), id));
        let mut s = Self {
            state: Arc::new(st),
            seq: last,
            db,
            unsnapped: 0,
            effects: vec![],
            foreign: vec![],
            txs: Default::default(),
            refused: Default::default(),
            committed: vec![],
            to_react: None,
            mark: None,
            file,
            recovered,
            panics: vec![],
            failures: vec![],
            clock: None,
            aio_kv,
            took: Default::default(),
            held: None,
            saved: None,
            keep: None,
            coalesce: PERSIST_DEBOUNCE,
            journal_wm: None,
            ahead,
        };
        // A failed snapshot only costs time: the next boot replays the log.
        if (migrated || last > from)
            && let Err(e) = s.snap(true)
        {
            eprintln!("[aio] {}: snapshot at boot failed: {e}", T::ID);
        }
        // aio's data differs (the first open, or aio ran since): a new event,
        // so the history before it stays. Repaired as its replay will be.
        // aio-rs's own file, migrated: its tables hold the older shape, so they
        // are made again from the migrated state instead of read.
        let native = s.aio_kv.is_none() && !T::TABLES.is_empty();
        if native && migrated {
            let mut st = match shaped(&*s.state)? {
                Some(v) => v,
                None => json(&*s.state)?,
            };
            let c = s.db.conn();
            let tx = Atomic::new(&c)?;
            crate::compat::data::write_bound(&tx, T::ID, &mut st, None, T::TABLES).map_err(Error)?;
            tx.commit()?;
        }
        let adopted = match seed.filter(|_| !(native && migrated)) {
            Some((st, raw, v)) => {
                // Some fields only: over the state as it stands, the rest kept.
                let st = match (whole, st) {
                    (true, Some(st)) => st,
                    _ => adopt(&back, &raw, v, whole)?,
                };
                let new = repaired(st.clone()) != back;
                if new {
                    // Settled by `#[on_aio]`, checked as a call's result is:
                    // a state the invariants refuse never boots.
                    let refused = |e: Rejected| Error(format!("{}: adopting aio's data: {e}", T::ID));
                    let (st, asks) = match T::ON_AIO.filter(|_| s.aio_kv.is_some()) {
                        None => (st, Asks::default()),
                        Some(m) => {
                            let (mut st, cx) = (repaired(st), Cx::live(s.now(), &By::System));
                            apply_guarded(&mut st, &cx, m, Value::Null).map_err(|e| refused(e.into()))?;
                            (st, cx.into_asks())
                        }
                    };
                    let args = serde_json::json!({ "v1": v, "state": crate::json(&st).map_err(|e| Error(e.why))? });
                    let st = repaired(st);
                    st.check().map_err(refused)?;
                    s.commit(&By::System, REWIND, &args, &asks, now_ms(), Arc::new(st), None, &[]).map_err(|e| Error(e.why))?;
                    s.effects.extend(asks.effects);
                }
                new
            }
            None => false,
        };
        // Unsaved fields back to their defaults, the repair applied: the
        // same state whether the app stopped cleanly or crashed.
        if !adopted && back != *s.state {
            s.commit(&By::System, RESTORE, &Value::Null, &Asks::default(), now_ms(), Arc::new(back), None, &[])
                .map_err(|e| Error(format!("{}: restore: {e}", T::ID)))?;
        }
        s.replay_journal(tail)?;
        refresh_good(&s.db.conn(), path)?;
        Ok(s)
    }

    /// An aio file's journal entries past its watermark (a crash before aio's
    /// save) run again as aio's next boot would run them: each call through
    /// this build's method, at its time, as its caller, its effects dropped
    /// (replay never runs I/O again), the watermark moved in the transaction
    /// of each. A tail aio-rs can't replay refuses the boot, the file kept.
    fn replay_journal(&mut self, tail: Vec<crate::compat::data::Journaled>) -> Result<(), Error> {
        let Some(kv) = self.aio_kv.clone() else { return Ok(()) };
        let Some(last) = tail.last().map(|j| match j {
            crate::compat::data::Journaled::Call { seq, .. } | crate::compat::data::Journaled::Skip { seq, .. } => *seq,
        }) else {
            return Ok(());
        };
        let effects = self.effects.len();
        for j in &tail {
            match j {
                crate::compat::data::Journaled::Skip { why, .. } => {
                    if let Some(why) = why {
                        eprintln!("[aio] {}: {why}", T::ID);
                    }
                }
                crate::compat::data::Journaled::Call { seq, method, args, by, at } => {
                    self.flush().map_err(|e| Error(e.why))?;
                    self.journal_wm = Some(*seq);
                    self.run(by, method, args.clone(), *at, None).map_err(|e| {
                        Error(format!(
                            "{}: aio's journal entry {seq} ({method}) doesn't run again here: {e}; start the aio app once and stop it, then open it here",
                            T::ID
                        ))
                    })?;
                }
            }
        }
        self.flush().map_err(|e| Error(e.why))?;
        self.effects.truncate(effects);
        // Entries past the last call (skipped ones): the watermark alone.
        self.journal_wm = None;
        let c = self.db.conn();
        if kv.journal_wm(&c).map_err(Error)? < last {
            kv.set_journal_wm(&c, last).map_err(Error)?;
        }
        drop(c);
        let calls = tail.iter().filter(|j| matches!(j, crate::compat::data::Journaled::Call { .. })).count();
        eprintln!("[aio] {}: replayed {calls} calls of aio's journal (up to entry {last})", T::ID);
        Ok(())
    }

    /// The time this store's calls see: the wall clock, or a test's.
    pub(crate) fn now(&self) -> i64 {
        self.clock.as_ref().map_or_else(now_ms, |c| c.load(std::sync::atomic::Ordering::Relaxed))
    }

    #[cfg(feature = "testing")]
    /// Run on `clock` instead of the wall clock (tests that move time).
    pub(crate) fn set_clock(&mut self, clock: std::sync::Arc<std::sync::atomic::AtomicI64>) {
        self.clock = Some(clock);
    }

    pub fn state(&self) -> &T {
        &self.state
    }

    /// The state, cheap to keep (shared, not copied).
    pub fn shared(&self) -> Arc<T> {
        self.state.clone()
    }

    /// The last committed event's number (0: none yet).
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// The event the synced call `op` made, while its id is kept (see
    /// [`Self::call_from`]); `None`: never taken here.
    #[cfg(all(feature = "compat", feature = "server"))]
    pub(crate) fn op_seq(&self, op: &str) -> Option<u64> {
        self.db
            .conn()
            .query_row("SELECT seq FROM ops WHERE cell = ?1 AND op = ?2", params![T::ID, op], |r| r.get::<_, i64>(0))
            .ok()
            .map(|s| s as u64)
    }

    /// Run one method atomically. The state changes only when the method
    /// returns `Ok`, the invariants hold and the event is in the log.
    pub fn call(&mut self, by: &By, method: &str, args: Value) -> Result<Value> {
        self.run(by, method, args, self.now(), None)
    }

    /// A call a synced client already made on its copy: `inputs` are the ids
    /// and random numbers it used (so its prediction and this event agree),
    /// `op` its id. A call with an `op` seen before is not applied again:
    /// `Ok(None)`.
    pub fn call_from(&mut self, by: &By, method: &str, args: Value, op: &str) -> Result<Option<Value>> {
        self.call_as(by, None, method, args, Some(op), None)
    }

    /// A network caller's call, with the role its gate proved ([`Cx::role`]);
    /// with an `op`, as [`Self::call_from`].
    pub(crate) fn call_as(
        &mut self,
        by: &By,
        role: Option<&str>,
        method: &str,
        args: Value,
        op: Option<&str>,
        reply: Option<crate::Reply>,
    ) -> Result<Option<Value>> {
        let now = self.now();
        let Some(op) = op else {
            let cx = Cx::live(now, by).with_role(role).with_reply(reply);
            let out = self.apply_and_commit(by, method, args, cx, None, None).map(Some);
            if let Some(sweep) = tx::On::<T>::SWEEP {
                sweep(self);
            }
            return out;
        };
        let op = crate::cx::Op::check(op, now).map_err(Rejected::new)?;
        let seen: bool = self
            .db
            .conn()
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM ops WHERE cell = ?1 AND op = ?2)",
                params![T::ID, op.to_string()],
                |r| r.get(0),
            )
            .map_err(|e| Rejected::new(format!("storage: {e}")))?;
        if seen {
            return Ok(None);
        }
        if let Some(e) = self.refused.0.get(&op.to_string()) {
            return Err(e.clone());
        }
        let cx = Cx::synced(now, T::ID, by, &op).with_role(role).with_reply(reply);
        let out = self.apply_and_commit(by, method, args, cx, None, Some(&op.to_string())).map(Some);
        if let Some(sweep) = tx::On::<T>::SWEEP {
            sweep(self);
        }
        out
    }

    /// A failure of the cell not made by a call (an async method's awaited `?`).
    pub(crate) fn failed(&mut self, by: &By, why: &str) {
        crate::report::tell("refused", &format!("{}.{why}", T::ID));
        self.failures.push((by.clone(), why.into()));
    }

    fn run(&mut self, by: &By, method: &str, args: Value, at: i64, fired: Option<&Due>) -> Result<Value> {
        self.apply_and_commit(by, method, args, Cx::live(at, by), fired, None)
    }

    /// A refusal of a person's call is an answer; one of the cell's own
    /// schedules or effects, or a panic, is a failure (`Cells` health).
    fn apply_and_commit(
        &mut self,
        by: &By,
        method: &str,
        args: Value,
        cx: Cx,
        fired: Option<&Due>,
        op: Option<&str>,
    ) -> Result<Value> {
        crate::log::in_call(|| self.apply_and_commit_in_call(by, method, args, cx, fired, op))
    }

    fn apply_and_commit_in_call(
        &mut self,
        by: &By,
        method: &str,
        args: Value,
        cx: Cx,
        fired: Option<&Due>,
        op: Option<&str>,
    ) -> Result<Value> {
        let (panics, t) = (self.panics.last().copied(), std::time::Instant::now());
        let out = self.apply_and_commit_once(by, method, args, cx, fired, op);
        let took = t.elapsed();
        crate::perf::called(took);
        let refused = out.as_ref().err().map(|e| e.why.as_str());
        if T::DIAGNOSTICS {
            crate::log::called(T::ID, method, took, refused);
            crate::report::called(T::ID, method, self.now(), took, refused);
        }
        let panicked = self.panics.last().copied() != panics;
        if let Err(e) = &out
            && (matches!(by, By::Schedule(_) | By::Effect(_)) || panicked)
        {
            // A panic was reported as it happened; a refusal is told here.
            if !panicked {
                crate::report::tell("refused", &format!("{}.{method}: {e}", T::ID));
            }
            self.failures.push((by.clone(), format!("{method}: {e}")));
        }
        out
    }

    fn apply_and_commit_once(
        &mut self,
        by: &By,
        method: &str,
        args: Value,
        cx: Cx,
        fired: Option<&Due>,
        op: Option<&str>,
    ) -> Result<Value> {
        let at = cx.now();
        crate::perf::storm(T::ID, method)?;
        let rewritten = crate::ask_before(&crate::Call { cell: T::ID, method, args: &args, by, role: cx.caller_role() })?;
        let args = rewritten.unwrap_or(args);
        if self.panics.len() >= PANICS_MAX {
            crate::reject!("{}: stopped after {PANICS_MAX} panics within a minute; restart the app", T::ID);
        }
        let base = method.split_once('.').map_or(method, |(m, _)| m);
        if let (Some(call), Some(cfg), Some(_)) = (tx::On::<T>::CALL, T::TRANSACTION, T::async_mode(base))
            && let Some(reply) = cx.reply()
        {
            return call(self, cfg, by, method, args, cx, reply, op);
        }
        let cx = cx.with_live(self.state.clone());
        let t0 = std::time::Instant::now();
        let mut next = (*self.state).clone();
        let t1 = std::time::Instant::now();
        // a panic is this call's refusal; the state (only a copy was
        // touched) stays as it was.
        let out = match apply_guarded(&mut next, &cx, method, args.clone()) {
            Err(Applied::Panicked(why)) => {
                let now = std::time::Instant::now();
                self.panics.retain(|t| now.duration_since(*t) < PANIC_WINDOW);
                self.panics.push(now);
                eprintln!("[aio] {}.{method} panicked: {why} ({} within a minute)", T::ID, self.panics.len());
                crate::reject!("{}.{method} panicked: {why}", T::ID);
            }
            Err(Applied::Refused(e)) => return Err(self.refused_op(op, e)),
            Ok(out) => out,
        };
        let t2 = std::time::Instant::now();
        next.check().map_err(|e| self.refused_op(op, e))?;
        let t3 = std::time::Instant::now();
        let mut asks = cx.into_asks();
        asks.op = op.map(String::from);
        let quiet = asks.effects.is_empty() && asks.plans.is_empty() && asks.inputs == Default::default();
        let coalesced = T::COALESCED.contains(&method) || (T::COALESCE && !T::redacts(method));
        if coalesced && fired.is_none() && asks.op.is_none() && quiet && !self.coalesce.is_zero() {
            self.hold(by, method, args, at, next)?;
            let p = crate::perf::Phases { clone: t1 - t0, apply: t2 - t1, check: t3 - t2, commit: t3.elapsed() };
            crate::perf::method(T::ID, method, p);
            return Ok(out);
        }
        // A held run goes into the log first: this call ran after it.
        self.flush()?;
        self.commit(by, method, &args, &asks, at, Arc::new(next), fired, &[])?;
        let p = crate::perf::Phases { clone: t1 - t0, apply: t2 - t1, check: t3 - t2, commit: t3.elapsed() };
        if self.took.len() == TOOK {
            self.took.pop_front();
        }
        self.took.push_back((self.seq, p.own()));
        crate::perf::method(T::ID, method, p);
        self.effects.extend(asks.effects);
        Ok(out)
    }

    /// Remember a synced client's call (`op`) refused as `e`.
    fn refused_op(&mut self, op: Option<&str>, e: Rejected) -> Rejected {
        if let Some(op) = op {
            let (by_op, order) = &mut self.refused;
            if order.len() == REFUSED_OPS
                && let Some(old) = order.pop_front()
            {
                by_op.remove(&old);
            }
            by_op.insert(op.into(), e.clone());
            order.push_back(op.into());
        }
        e
    }

    /// A coalesced call: its state is shown at once, its event waits (see
    /// [`Self::flush`]). One that changes nothing is not even held.
    fn hold(&mut self, by: &By, method: &str, args: Value, at: i64, next: T) -> Result {
        if next == *self.state {
            return Ok(());
        }
        let mut h = match self.held.take() {
            Some(h) if h.by == *by && h.calls.len() < HELD_MAX => h,
            other => {
                self.held = other;
                self.flush()?;
                let base = self.state.clone();
                Held { by: by.clone(), calls: vec![], base, since: std::time::Instant::now() }
            }
        };
        self.state = Arc::new(next);
        h.calls.push((method.into(), args, at));
        self.held = Some(h);
        Ok(())
    }

    /// Put a held run into the log, in one write (aio wrote its state once
    /// per window): one event, its last call, when the run is one method's
    /// and that call alone ends where the run did from where it began (a
    /// cursor's steps); else each call, in order (a step in one panel, then
    /// in the other; `down` after `down`). A run that came back to where it
    /// began logs nothing. Refused (storage), the run stays held.
    pub fn flush(&mut self) -> Result {
        if self.held.is_none() {
            return Ok(());
        }
        self.joining().map_err(|e| Rejected::new(format!("storage: {e}")))?;
        let Some(h) = self.held.take() else { return Ok(()) };
        let now = std::mem::replace(&mut self.state, h.base.clone());
        if *now == *h.base {
            return Ok(());
        }
        let last = h.calls.len() - 1;
        let (method, args, at) = &h.calls[last];
        let alone = h.calls.iter().all(|c| c.0 == *method) && {
            let mut st = (*h.base).clone();
            apply_guarded(&mut st, &Cx::live(*at, &h.by), method, args.clone()).is_ok() && st == *now
        };
        let earlier = if alone { &[][..] } else { &h.calls[..last] };
        // The state shown all along stays the same `Arc`: the views have
        // nothing new to draw, the caches keyed on it hold.
        if let Err(e) = self.commit(&h.by, method, args, &Asks::default(), *at, now.clone(), None, earlier) {
            self.state = now;
            self.held = Some(h);
            return Err(e);
        }
        Ok(())
    }

    /// Flush a held run [`Self::set_coalesce`] (default [`PERSIST_DEBOUNCE`])
    /// after its first call; else how long until then.
    pub(crate) fn flush_idle(&mut self) -> Option<std::time::Duration> {
        let left = self.coalesce.checked_sub(self.held.as_ref()?.since.elapsed());
        match left {
            Some(d) if !d.is_zero() => Some(d),
            _ => {
                if let Err(e) = self.flush() {
                    eprintln!("[aio] {}: a held run could not be saved: {e}", T::ID);
                    return Some(self.coalesce.max(PERSIST_DEBOUNCE));
                }
                None
            }
        }
    }

    /// Is a run of coalesced calls waiting to go into the log?
    pub fn holds(&self) -> bool {
        self.held.is_some()
    }

    /// Hold a run of `#[coalesce]` calls `d` before it goes into the log
    /// (`[data] coalesce_ms`, aio's `persistDebounceMs`); zero: each call at once.
    pub fn set_coalesce(&mut self, d: std::time::Duration) -> Result {
        self.coalesce = d;
        if d.is_zero() { self.flush() } else { Ok(()) }
    }

    /// The event, its schedule changes and the fired schedule's re-arm go in
    /// one transaction: all of them or none. `earlier`: a held run's calls
    /// before this one (its last), logged first in the same transaction.
    #[allow(clippy::too_many_arguments)]
    fn commit(
        &mut self,
        by: &By,
        method: &str,
        args: &Value,
        asks: &Asks,
        at: i64,
        next: Arc<T>,
        fired: Option<&Due>,
        earlier: &[(String, Value, i64)],
    ) -> Result {
        // A schedule names a method by its name: a typo is refused now, by the
        // call that set it, not found when it fires.
        for p in &asks.plans {
            if let Plan::Set { key, method: m, .. } = p
                && !m.contains(':')
                && !T::METHODS.contains(&m.as_str())
            {
                crate::reject!("schedule `{key}`: {} has no method `{m}` (its methods: {})", T::ID, T::METHODS.join(", "));
            }
        }
        // A #[redact] call keeps the state it made, never its arguments.
        let kept = redacted(method, &*self.state, &next)?;
        let args = kept.as_ref().unwrap_or(args);
        let (inputs_text, by_text) = (crate::json(&asks.inputs)?.to_string(), crate::json(by)?.to_string());
        // aio's slice is what is saved of it: no `#[transient]` field on disk.
        let saved = |st: &T| match shaped(st)? {
            Some(v) => Ok(v),
            None => json(st),
        };
        // aio-rs's own file keeps its bound tables too.
        let bound = self.aio_kv.is_some() || !T::TABLES.is_empty();
        let kv_state = bound.then(|| saved(&next)).transpose().map_err(|e| Rejected::new(e.0))?;
        // What its tables hold now: only the rows that changed are written.
        // With `#[aio_cell]` fields, only the slices that changed are written.
        // The committed state's saved form is kept from its own commit.
        let kv_prev = match bound && !(T::TABLES.is_empty() && T::AIO_CELLS.is_empty()) {
            true => Some(match self.saved.take() {
                Some(v) => v,
                None => saved(&self.state).map_err(|e| Rejected::new(e.0))?,
            }),
            false => None,
        };
        let db = |e: rusqlite::Error| Rejected::new(format!("storage: {e}"));
        if let Some((path, id)) = &self.file
            && file_id(path) != Some(*id)
        {
            eprintln!("[aio] storage: {} was deleted or replaced while the app ran; refusing writes", path.display());
            crate::reject!(
                "storage: {} was deleted or replaced while the app ran; nothing more can be saved: restart the app",
                path.display()
            );
        }
        // D6: another writer (a second process, a tool) changed the log
        // under us: our state no longer follows it, so stop instead of forking.
        let newer: bool = self
            .db
            .conn()
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM log WHERE cell = ?1 AND seq > ?2)",
                params![T::ID, self.seq as i64],
                |r| r.get(0),
            )
            .map_err(db)?;
        if newer {
            eprintln!(
                "[aio] storage: another writer added to {}'s log after event {}; refusing writes",
                T::ID,
                self.seq
            );
            crate::reject!("storage: something else wrote to this app's data; restart the app to see it");
        }
        self.joining().map_err(db)?;
        let c = self.db.conn();
        let tx = Atomic::new(&c).map_err(db)?;
        let mut logged_seqs = Vec::with_capacity(earlier.len());
        let calls = earlier.iter().map(|(m, a, t)| (m.as_str(), a, *t, "{}")).chain([(method, args, at, &*inputs_text)]);
        for (m, a, t, inputs) in calls {
            tx.execute(
                "INSERT INTO log (cell, method, args, inputs, by, at, v) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![T::ID, m, a.to_string(), inputs, by_text, t, T::VERSION],
            )
            .map_err(db)?;
            logged_seqs.push(tx.last_insert_rowid() as u64);
        }
        let seq = logged_seqs.pop().expect("this call's event");
        let keep = kv_prev.is_some();
        let kept_saved = match (&self.aio_kv, kv_state, &kv_prev) {
            (Some(kv), Some(st), Some(prev)) if !T::AIO_CELLS.is_empty() => kv.write_slices(&tx, T::AIO_CELLS, &st, prev).map(|_| st),
            (Some(kv), Some(st), prev) => {
                let copy = st.clone();
                kv.write(&tx, T::ID, T::VERSION, st, prev.as_ref(), T::TABLES).map(|_| copy)
            }
            (None, Some(st), prev) => {
                let mut rows = st.clone();
                crate::compat::data::write_bound(&tx, T::ID, &mut rows, prev.as_ref(), T::TABLES).map(|_| st)
            }
            _ => Ok(Value::Null),
        }
        .map_err(Rejected::new)?;
        if let (Some(kv), Some(wm)) = (&self.aio_kv, self.journal_wm) {
            kv.set_journal_wm(&tx, wm).map_err(Rejected::new)?;
        }
        if let Some(op) = &asks.op {
            tx.execute("INSERT INTO ops (cell, op, seq) VALUES (?1, ?2, ?3)", params![T::ID, op, seq as i64])
                .map_err(db)?;
        }
        if let Some(d) = fired {
            rearm::<T>(&tx, d, at).map_err(db)?;
        }
        for p in &asks.plans {
            match p {
                Plan::Set { key, due, every, cron, method, args } => tx.execute(
                    "INSERT OR REPLACE INTO sched (cell, key, due, every, method, args, cron) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![T::ID, key, due, every, method, args.to_string(), cron],
                ),
                Plan::Cancel(key) => tx.execute("DELETE FROM sched WHERE cell = ?1 AND key = ?2", params![T::ID, key]),
            }
            .map_err(db)?;
        }
        tx.commit().map_err(db)?;
        drop(c);
        self.journal_wm = None;
        for ((m, a, t), s) in earlier.iter().zip(logged_seqs) {
            let ev = Event { seq: s, method: m.clone(), args: a.clone(), inputs: Default::default(), by: by.clone(), at: *t, v: T::VERSION };
            if let Some(r) = &mut self.to_react {
                r.push(ev.clone());
            }
            self.committed.push((self.seq, ev));
            self.seq = s;
        }
        let ev = Event {
            seq,
            method: method.into(),
            args: args.clone(),
            inputs: asks.inputs.clone(),
            by: by.clone(),
            at,
            v: T::VERSION,
        };
        if let Some(r) = &mut self.to_react {
            r.push(ev.clone());
        }
        self.committed.push((self.seq, ev));
        self.seq = seq;
        // A dev build at `debug` says what each call changed, key by key.
        if cfg!(debug_assertions)
            && crate::log::enabled(crate::log::Level::Debug)
            && !crate::log::suppressed(T::ID, method)
            && kept.is_none()
            && T::DIAGNOSTICS
            && let (Ok(a), Ok(b)) = (logged(&*self.state), logged(&*next))
        {
            let ops = crate::patch::diff(&a, &b);
            if !ops.is_empty() {
                crate::debug!(cat: "diff", "{}.{method}: {}", T::ID, crate::patch::said(&a, &ops));
            }
        }
        self.state = next;
        self.saved = keep.then_some(kept_saved);
        let before = self.unsnapped;
        self.unsnapped += 1 + earlier.len() as u32;
        if before / SNAP_EVERY != self.unsnapped / SNAP_EVERY {
            if let Err(e) = self.snap(true) {
                // The log already holds the event; the next boot replays it.
                eprintln!("[aio] {}: snapshot failed: {e}", T::ID);
            }
            if let Err(e) = self.fold_kept() {
                eprintln!("[aio] {}: [history] keep: folding failed: {e}", T::ID);
            }
        }
        Ok(())
    }

    /// Events committed since the last take, each with the event before it.
    pub(crate) fn take_committed(&mut self) -> Vec<(u64, Event)> {
        std::mem::take(&mut self.committed)
    }

    /// Other cells listen to it: from now on its events wait for their reactions.
    pub(crate) fn listened(&mut self) {
        self.to_react.get_or_insert_default();
    }

    /// Events committed since the cells listening to it last reacted.
    pub(crate) fn unreacted(&mut self) -> Vec<Event> {
        self.to_react.as_mut().map(std::mem::take).unwrap_or_default()
    }

    /// Its file's connection, shared with the other cells kept in it.
    pub(crate) fn file(&self) -> Shared {
        self.db.file().clone()
    }

    /// Before a write: in a joint commit, the store as it was before its
    /// first write there is kept, to put back should the commit be refused.
    fn joining(&mut self) -> rusqlite::Result<()> {
        if let Some(id) = self.db.begin()?
            && self.mark.as_ref().is_none_or(|m| m.0 != id)
        {
            self.mark = Some((id, self.marked()));
        }
        Ok(())
    }

    /// A joint commit it wrote in ended: `kept`, or refused and put back.
    pub(crate) fn joint_end(&mut self, joint: u64, kept: bool) {
        if let Some((_, m)) = self.mark.take_if(|m| m.0 == joint)
            && !kept
        {
            self.undo(m);
        }
    }

    fn marked(&self) -> Mark<T> {
        Mark {
            state: self.state.clone(),
            seq: self.seq,
            unsnapped: self.unsnapped,
            effects: self.effects.len(),
            foreign: self.foreign.len(),
            committed: self.committed.len(),
            to_react: self.to_react.as_ref().map_or(0, Vec::len),
            held: self.held.clone(),
            journal_wm: self.journal_wm,
        }
    }

    /// Back to `m`: a joint commit its calls went into was refused, so
    /// none of them are on disk.
    fn undo(&mut self, m: Mark<T>) {
        self.state = m.state;
        self.seq = m.seq;
        self.unsnapped = m.unsnapped;
        self.effects.truncate(m.effects);
        self.foreign.truncate(m.foreign);
        self.committed.truncate(m.committed);
        if let Some(r) = &mut self.to_react {
            r.truncate(m.to_react);
        }
        self.took.retain(|(s, _)| *s <= m.seq);
        self.held = m.held;
        self.journal_wm = m.journal_wm;
        self.saved = None;
    }

    /// Failures since the last take (see `apply_and_commit`).
    pub(crate) fn take_failures(&mut self) -> Vec<(By, String)> {
        std::mem::take(&mut self.failures)
    }

    /// The last committed event's method and time (ms since the epoch).
    pub(crate) fn last_call(&self) -> Option<(String, i64)> {
        self.db
            .conn()
            .query_row("SELECT method, at FROM log WHERE cell = ?1 ORDER BY seq DESC LIMIT 1", [T::ID], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()
            .ok()
            .flatten()
    }

    /// The schedules due on other cells since the last take.
    pub(crate) fn take_foreign(&mut self) -> Vec<Due> {
        std::mem::take(&mut self.foreign)
    }

    /// Effects asked for by committed calls, to be started by the runtime.
    pub(crate) fn take_effects(&mut self) -> Vec<Box<dyn std::any::Any + Send>> {
        std::mem::take(&mut self.effects)
    }

    /// Every saved schedule, soonest first.
    pub fn schedules(&self) -> Result<Vec<Due>, Error> {
        let c = self.db.conn();
        let mut q = c
            .prepare("SELECT key, due, every, method, args, cron FROM sched WHERE cell = ?1 ORDER BY due, key")?;
        let rows = q.query_map([T::ID], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get::<_, String>(4)?, r.get(5)?))
        })?;
        rows.map(|row| {
            let (key, due, every, method, args, cron) = row?;
            let args =
                serde_json::from_str(&args).map_err(|e| Error(format!("{}: corrupt schedule {key}: {e}", T::ID)))?;
            Ok(Due { key, due, every, cron, method, args })
        })
        .collect()
    }

    /// Run every schedule due at `now`, in time order. A refused call is
    /// reported and still re-armed (or removed), so it can't fire in a loop.
    /// Returns how many fired.
    pub fn fire_due(&mut self, now: i64) -> Result<usize, Error> {
        self.fire_due_but(now, |_| false)
    }

    /// [`Self::fire_due`], re-arming without a call the schedules `skip` names.
    pub(crate) fn fire_due_but(&mut self, now: i64, skip: impl Fn(&Due) -> bool) -> Result<usize, Error> {
        let due: Vec<Due> = self.schedules()?.into_iter().filter(|d| d.due <= now).collect();
        for d in &due {
            if skip(d) {
                rearm::<T>(&self.db.conn(), d, now)?;
                continue;
            }
            // Another cell's method (`cell:method`): its handle calls it.
            if d.method.contains(':') {
                rearm::<T>(&self.db.conn(), d, now)?;
                self.foreign.push(d.clone());
                continue;
            }
            let by = By::Schedule(d.key.clone());
            if let Err(e) = self.run(&by, &d.method, d.args.clone(), now, Some(d)) {
                eprintln!("[aio] {}: schedule `{}` ({}) refused: {e}", T::ID, d.key, d.method);
                rearm::<T>(&self.db.conn(), d, now)?;
            }
        }
        Ok(due.len())
    }

    /// Arm the cell's declared schedules (`Cell::SCHEDULES`) as `@<method>`:
    /// one already saved with the same timing keeps its next run, a changed
    /// one starts over from now, and an `@` one no longer declared goes.
    pub(crate) fn declare(&mut self) -> Result<(), Error> {
        let (now, have) = (self.now(), self.schedules()?);
        let c = self.db.conn();
        let tx = Atomic::new(&c)?;
        for d in &have {
            if d.key.strip_prefix('@').is_some_and(|m| !T::SCHEDULES.iter().any(|s| s.method == m)) {
                tx.execute("DELETE FROM sched WHERE cell = ?1 AND key = ?2", params![T::ID, d.key])?;
            }
        }
        for s in T::SCHEDULES {
            let key = format!("@{}", s.method);
            if have.iter().any(|d| d.key == key && d.every == s.every && d.cron.as_deref() == s.cron) {
                continue;
            }
            let due = match (s.every, s.cron) {
                (Some(ms), _) => now + ms,
                (None, Some(c)) => {
                    let c = crate::cron::Cron::parse(c)
                        .map_err(|e| Error(format!("{}.{}: #[cron]: {e}", T::ID, s.method)))?;
                    c.next(now).ok_or_else(|| Error(format!("{}.{}: #[cron] never runs", T::ID, s.method)))?
                }
                (None, None) => continue,
            };
            tx.execute(
                "INSERT OR REPLACE INTO sched (cell, key, due, every, method, args, cron) VALUES (?1, ?2, ?3, ?4, ?5, 'null', ?6)",
                params![T::ID, key, due, s.every, s.method, s.cron],
            )?;
        }
        Ok(tx.commit()?)
    }

    /// Set when this open recovered from a damaged file (tell the user).
    pub fn recovered(&self) -> Option<&Recovered> {
        self.recovered.as_ref()
    }

    /// How hard each commit is saved from now on.
    pub fn set_durability(&mut self, d: crate::config::Durability) -> Result<(), Error> {
        let mode = match d {
            crate::config::Durability::Normal => "NORMAL",
            crate::config::Durability::Full => "FULL",
        };
        self.db.pragmas(&[format!("PRAGMA synchronous = {mode}")]).map_err(Error)
    }

    /// Set the app's own pragmas (`[data] pragmas`) on the store's file;
    /// one SQLite ignores is refused, never left looking set.
    pub fn set_pragmas(&mut self, ps: &[String]) -> Result<(), Error> {
        self.db.pragmas(ps).map_err(Error)
    }

    /// How hard commits are saved, as SQLite says (not a remembered setting).
    pub fn durability(&self) -> Result<crate::config::Durability, Error> {
        let n: i64 = self.db.conn().query_row("PRAGMA synchronous", [], |r| r.get(0))?;
        Ok(if n >= 2 { crate::config::Durability::Full } else { crate::config::Durability::Normal })
    }

    /// Snapshot the state (`exact`: only if it comes back whole, else
    /// `SNAP_EVERY` events later).
    fn snap(&mut self, exact: bool) -> Result<(), Error> {
        let Some(text) = snapshot(&*self.state, exact)? else { return Ok(()) };
        let (v, text) = match &self.ahead {
            None => (T::VERSION, text),
            Some((v, newer)) => {
                let mut mine: Value = serde_json::from_str(&text).map_err(|e| Error(e.to_string()))?;
                crate::filter::reattach(&mut mine, newer);
                if let Some(m) = mine.as_object_mut() {
                    m.insert(DOWNGRADED.into(), T::VERSION.into());
                }
                (*v, mine.to_string())
            }
        };
        self.db.conn().execute(
            "INSERT OR REPLACE INTO snap (cell, seq, v, state) VALUES (?1, ?2, ?3, ?4)",
            params![T::ID, self.seq as i64, v, text],
        )?;
        self.unsnapped = 0;
        Ok(())
    }

    /// Every kept event, oldest first.
    pub fn events(&self) -> Result<Vec<Event>, Error> {
        self.newest(usize::MAX)
    }

    /// The newest `n` events, oldest first.
    pub fn newest(&self, n: usize) -> Result<Vec<Event>, Error> {
        let c = self.db.conn();
        let mut q =
            c.prepare("SELECT seq, method, args, inputs, by, at, v FROM (SELECT * FROM log WHERE cell = ?1 ORDER BY seq DESC LIMIT ?2) ORDER BY seq")?;
        let rows = q.query_map(params![T::ID, i64::try_from(n).unwrap_or(-1)], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get(5)?,
                r.get(6)?,
            ))
        })?;
        rows.map(|row| {
            let (seq, method, args, inputs, by, at, v) = row?;
            let bad = |e: serde_json::Error| Error(format!("{}: corrupt event {seq}: {e}", T::ID));
            Ok(Event {
                seq: seq as u64,
                method,
                args: serde_json::from_str(&args).map_err(bad)?,
                inputs: serde_json::from_str(&inputs).map_err(bad)?,
                by: serde_json::from_str(&by).map_err(bad)?,
                at,
                v,
            })
        })
        .collect()
    }

    fn floor(&self) -> Result<u64, Error> {
        let f: Option<i64> =
            self.db.conn().query_row("SELECT seq FROM floor WHERE cell = ?1", [T::ID], |r| r.get(0)).optional()?;
        Ok(f.unwrap_or(0) as u64)
    }

    /// Another cell's history gone from this store's file (`ax persist
    /// --drop`: one no build declares any more, as `#[cell(retire)]` does),
    /// never this cell's or one it reads; whether there was any.
    pub fn drop_cell(&self, cell: &str) -> Result<bool, Error> {
        if cell == T::ID || T::AIO_CELLS.iter().any(|s| s.cell == cell) {
            return Err(Error(format!("{}: it reads {cell}: a declared cell is never dropped", T::ID)));
        }
        erase(&self.db.conn(), self.aio_kv.as_ref(), &[cell], "")
    }

    /// The state right after event `seq` (0: the start).
    pub fn state_at(&self, seq: u64) -> Result<T, Error> {
        if seq > self.seq {
            return Err(Error(format!("{}: no event {seq} yet (last is {})", T::ID, self.seq)));
        }
        let floor = self.floor()?;
        if seq < floor {
            return Err(Error(format!("{}: history before event {floor} was folded", T::ID)));
        }
        let base: Option<(i64, u32, String)> = self
            .db
            .conn()
            .query_row(
                "SELECT seq, v, state FROM snap WHERE cell = ?1 AND seq <= ?2 ORDER BY seq DESC LIMIT 1",
                params![T::ID, seq as i64],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let (from, mut st) = match base {
            Some((s, v, text)) => {
                if v != T::VERSION && self.has_events(s as u64, seq)? {
                    return Err(Error(format!("{}: event {seq} is from before the upgrade from v{v}", T::ID)));
                }
                (s as u64, decode(&text, v)?)
            }
            None => (0, T::default()),
        };
        replay(&self.db.conn(), &mut st, from, seq, false)?;
        Ok(st)
    }

    fn has_events(&self, from: u64, to: u64) -> Result<bool, Error> {
        Ok(self.db.conn().query_row(
            "SELECT EXISTS (SELECT 1 FROM log WHERE cell = ?1 AND seq > ?2 AND seq <= ?3)",
            params![T::ID, from as i64, to as i64],
            |r| r.get(0),
        )?)
    }

    /// How long the method of event `seq` took, if it ran in this run
    /// (one of the newest `TOOK`).
    pub fn took(&self, seq: u64) -> Option<std::time::Duration> {
        self.took.iter().rev().find(|(s, _)| *s == seq).map(|(_, d)| *d)
    }

    /// The cell's log as time travel reads it ([`Steps`]).
    fn steps(&self) -> Result<Vec<(u64, Option<u64>)>, Error> {
        let c = self.db.conn();
        let mut q = c.prepare("SELECT seq, method, args FROM log WHERE cell = ?1 ORDER BY seq")?;
        let rows =
            q.query_map([T::ID], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?;
        rows.map(|r| {
            let (seq, method, args) = r?;
            let to = (method == REWIND).then(|| serde_json::from_str::<Value>(&args).ok()?["to"].as_u64()).flatten();
            Ok((seq as u64, to))
        })
        .collect()
    }

    /// Where the cell is in its history ([`Position`]).
    pub fn position(&self) -> Result<Position, Error> {
        Ok(Position::of(&self.steps()?, self.floor()?))
    }

    /// Step through the history ([`Travel`]): returns the event the state is
    /// right after now.
    pub fn travel(&mut self, by: &By, t: Travel) -> Result<u64> {
        self.flush()?;
        let (log, floor) = (self.steps().map_err(|e| Rejected::new(e.0))?, self.floor().map_err(|e| Rejected::new(e.0))?);
        let to = Position::target(&log, floor, t).map_err(Rejected::new)?;
        self.rewind(by, to)?;
        Ok(to)
    }

    /// Go back to the state after event `seq`. This is a new event, so the
    /// steps in between stay in the history (and can be returned to). Like
    /// a boot, it brings back what was saved of that state, repaired.
    pub fn rewind(&mut self, by: &By, seq: u64) -> Result {
        let st = self.state_at(seq).map_err(|e| Rejected::new(e.0))?;
        self.put(by, st, ("to", seq.into()))
    }

    /// Replace the state with `state` (this version's JSON, as `aio
    /// snapshot` writes it): a new event like a rewind, so the state before
    /// it stays in the history. One that doesn't fit the type, or that the
    /// cell's `check` refuses, changes nothing.
    pub fn load(&mut self, by: &By, state: Value) -> Result {
        let st: T = unshaped(state).map_err(|e| Rejected::new(e.0))?;
        st.check()?;
        self.put(by, st, ("load", true.into()))
    }

    /// Commit `st` as a rewind event (`why` says which kind), as a boot
    /// brings it back: what is saved of it, repaired.
    fn put(&mut self, by: &By, st: T, why: (&str, Value)) -> Result {
        self.flush()?;
        let (saved, back) = match shaped(&st).map_err(|e| Rejected::new(e.0))? {
            Some(v) => (v.clone(), unshaped(v).map_err(|e| Rejected::new(e.0))?),
            None => (crate::json(&st)?, st),
        };
        let args = serde_json::json!({ why.0: why.1, "state": saved });
        self.commit(by, REWIND, &args, &Asks::default(), self.now(), Arc::new(repaired(back)), None, &[])
    }

    /// What this version saves of the state now, as a shape: each value
    /// replaced by its JSON type (`ax migrations` compares a stored
    /// snapshot against it).
    pub fn shape(&self) -> Result<Value, Error> {
        let v = match shaped(&*self.state)? {
            Some(v) => v,
            None => json(&*self.state)?,
        };
        Ok(shape_of(&v))
    }

    /// Keep the history `keep` long (`[history] keep`): the events older
    /// are folded now, then every `SNAP_EVERY` commits. Returns how many
    /// were folded now.
    pub fn keep_history(&mut self, keep: std::time::Duration) -> Result<usize, Error> {
        self.keep = Some(i64::try_from(keep.as_millis()).unwrap_or(i64::MAX));
        self.fold_kept()
    }

    fn fold_kept(&mut self) -> Result<usize, Error> {
        let Some(keep) = self.keep else { return Ok(0) };
        self.fold_before(self.now().saturating_sub(keep))
    }

    /// Fold the events older than `before` (ms since the epoch) into a
    /// snapshot: time travel then reaches back to that snapshot only.
    /// Returns how many events were folded.
    pub fn fold_before(&mut self, before: i64) -> Result<usize, Error> {
        self.flush().map_err(|e| Error(e.why))?;
        let cut: Option<i64> =
            self.db.conn().query_row("SELECT MAX(seq) FROM log WHERE cell = ?1 AND at < ?2", params![T::ID, before], |r| {
                r.get(0)
            })?;
        let Some(cut) = cut else { return Ok(0) };
        let text = snapshot(&self.state_at(cut as u64)?, true)?;
        // What is saved of that state can't bring it back whole: fold at the
        // newest snapshot before it instead, which the events after it follow.
        let cut = match text {
            Some(_) => cut,
            None => {
                let snap: Option<i64> = self.db.conn().query_row(
                    "SELECT MAX(seq) FROM snap WHERE cell = ?1 AND seq <= ?2",
                    params![T::ID, cut],
                    |r| r.get(0),
                )?;
                match snap {
                    Some(s) if s as u64 > self.floor()? => s,
                    _ => return Ok(0),
                }
            }
        };
        let c = self.db.conn();
        let tx = Atomic::new(&c)?;
        if let Some(text) = text {
            tx.execute(
                "INSERT OR REPLACE INTO snap (cell, seq, v, state) VALUES (?1, ?2, ?3, ?4)",
                params![T::ID, cut, T::VERSION, text],
            )?;
        }
        let n = tx.execute("DELETE FROM log WHERE cell = ?1 AND seq <= ?2", params![T::ID, cut])?;
        tx.execute("DELETE FROM snap WHERE cell = ?1 AND seq < ?2", params![T::ID, cut])?;
        // Call ids expire with the folded history, but never before a re-send
        // of them would be refused anyway (`Op::MAX_AGE_MS`).
        let oldest = self.now() - crate::cx::Op::MAX_AGE_MS;
        let old: Vec<String> = tx
            .prepare("SELECT op FROM ops WHERE cell = ?1 AND seq <= ?2")?
            .query_map(params![T::ID, cut], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for op in old.iter().filter(|op| crate::cx::Op::parse(op).is_none_or(|o| o.ms() < oldest)) {
            tx.execute("DELETE FROM ops WHERE cell = ?1 AND op = ?2", params![T::ID, op])?;
        }
        tx.execute("INSERT OR REPLACE INTO floor (cell, seq) VALUES (?1, ?2)", params![T::ID, cut])?;
        tx.commit()?;
        Ok(n)
    }
}

/// After a fire: a repeating schedule moves to its next slot after `now`
/// (missed slots fire once, not one by one); a one-shot is removed. Only the
/// row that fired: a method that set the same key meanwhile keeps its row.
fn rearm<T: Cell>(db: &Connection, d: &Due, now: i64) -> rusqlite::Result<()> {
    // A cron schedule: its next slot after now (a missed one fired once).
    if let Some(next) = d.cron.as_deref().and_then(|c| crate::cron::Cron::parse(c).ok()?.next(now.max(d.due))) {
        return db
            .execute(
                "UPDATE sched SET due = ?3 WHERE cell = ?1 AND key = ?2 AND due = ?4",
                params![T::ID, d.key, next, d.due],
            )
            .map(|_| ());
    }
    match d.every {
        Some(ms) => {
            let mut next = d.due + ms;
            if next <= now {
                next = now + ms;
            }
            db.execute(
                "UPDATE sched SET due = ?3 WHERE cell = ?1 AND key = ?2 AND due = ?4",
                params![T::ID, d.key, next, d.due],
            )
        }
        None => db.execute("DELETE FROM sched WHERE cell = ?1 AND key = ?2 AND due = ?3", params![T::ID, d.key, d.due]),
    }
    .map(|_| ())
}

impl<T: Cell> Drop for Store<T> {
    fn drop(&mut self) {
        if let Err(e) = self.flush() {
            eprintln!("[aio] {}: a held run could not be saved at close: {e}", T::ID);
        }
        if self.unsnapped > 0
            && let Err(e) = self.snap(false)
        {
            eprintln!("[aio] {}: snapshot at close failed: {e}", T::ID);
        }
    }
}

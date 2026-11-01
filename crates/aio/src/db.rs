//! An app's own SQLite file (aio's `createDB`): raw SQL for what no cell holds
//! (logs, caches, imported tables). One writer thread runs the writes in
//! order; `readers` threads answer reads beside it on the WAL file, so a long
//! read never waits for a write. Every request is async and bounded
//! ([`Opts::timeout`]: a statement that runs longer is interrupted and
//! rolled back, never left running). Each call runs exactly one statement,
//! and transaction control is the API's (`batch`, `transaction`), never a
//! statement's: a stray `BEGIN` can't swallow the next caller's writes.
//! Pragmas are checked after they're set: one SQLite ignored refuses the open.
//! A live query ([`Db::live`]) runs again when a write changes a table it
//! reads; both sides are known from SQLite itself, not from parsing SQL.
use crate::Error;
use rusqlite::Connection;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A value SQLite stores: `Null`, `Integer`, `Real`, `Text`, `Blob`
/// (`5.into()`, `"a".to_string().into()`, `None::<i64>.into()`).
pub use rusqlite::types::Value as Sql;

/// What every connection sets first (aio's `DEFAULT_PRAGMAS`).
pub const PRAGMAS: [&str; 5] = [
    "PRAGMA journal_mode = WAL",
    "PRAGMA synchronous = NORMAL",
    "PRAGMA cache_size = -64000",
    "PRAGMA busy_timeout = 5000",
    "PRAGMA foreign_keys = ON",
];

/// How long one request may run, unless [`Opts::timeout`] says otherwise.
pub const TIMEOUT: Duration = Duration::from_secs(120);

/// How a database opens.
#[derive(Clone, Debug)]
pub struct Opts {
    /// Read threads beside the writer (none for `:memory:`): `query` takes
    /// turns over them.
    pub readers: usize,
    /// Pragmas over [`PRAGMAS`], by name (`PRAGMA synchronous = FULL`
    /// replaces only `synchronous`).
    pub pragmas: Vec<String>,
    /// How long one request may run (`None`: as long as it takes).
    pub timeout: Option<Duration>,
    /// D4 for this file: checked at open, a damaged one is set aside (never
    /// deleted) and its good copy, made daily at open, takes its place.
    pub guard: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Self { readers: 0, pragmas: vec![], timeout: Some(TIMEOUT), guard: false }
    }
}

/// A query's answer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rows {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Sql>>,
}

impl Rows {
    /// Each row as `T` (by column name; a blob is a list of bytes).
    pub fn to<T: serde::de::DeserializeOwned>(&self) -> Result<Vec<T>, Error> {
        let cell = |v: &Sql| match v {
            Sql::Null => serde_json::Value::Null,
            Sql::Integer(i) => (*i).into(),
            Sql::Real(f) => (*f).into(),
            Sql::Text(s) => s.clone().into(),
            Sql::Blob(b) => b.clone().into(),
        };
        let row = |r: &Vec<Sql>| self.columns.iter().cloned().zip(r.iter().map(cell)).collect();
        let rows = self.rows.iter().map(|r| serde_json::Value::Object(row(r)));
        rows.map(|r| serde_json::from_value(r).map_err(|e| Error(format!("db: a row doesn't fit: {e}")))).collect()
    }
}

/// What a write did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Done {
    pub changes: usize,
    /// The rowid of the last insert on this connection.
    pub last_id: i64,
}

/// The pragmas a connection sets: [`PRAGMAS`] with `over` replacing the
/// ones of the same name, then the rest of `over`; a read-only connection
/// leaves `journal_mode` to the writer.
pub fn pragmas(over: &[String], readonly: bool) -> Vec<String> {
    let name = |p: &str| pragma_name(p).unwrap_or_default();
    let mine = |d: &str| over.iter().find(|p| name(p) == name(d)).cloned().unwrap_or_else(|| d.into());
    let all = PRAGMAS
        .iter()
        .map(|d| mine(d))
        .chain(over.iter().filter(|p| !PRAGMAS.iter().any(|d| name(d) == name(p))).cloned());
    all.filter(|p| !(readonly && name(p) == "journal_mode")).collect()
}

pub use aio_config::db::pragma_name;
/// Why `pragma` didn't take on `c` (SQLite ignores a name it doesn't know
/// and falls back on a value it doesn't know, without an error), or `None`.
pub(crate) fn not_taken(c: &Connection, pragma: &str) -> Option<String> {
    let body = pragma.trim().trim_end_matches(';').trim();
    let body = body.get(6..).filter(|_| body[..6].eq_ignore_ascii_case("pragma"))?.trim();
    let (name, asked) = match body.split_once('=') {
        Some((n, v)) => (n.trim(), Some(v.trim().trim_matches(['\'', '"']).to_ascii_lowercase())),
        None => (body, None),
    };
    let name = name.rsplit('.').next().unwrap_or_default().trim().to_ascii_lowercase();
    let known: bool = c
        .query_row("SELECT count(*) > 0 FROM pragma_pragma_list WHERE name = ?1", [&name], |r| r.get(0))
        .unwrap_or(true);
    if !known {
        return Some(format!(
            "\"{pragma}\": SQLite has no pragma \"{name}\"; it ignores the statement, so this setting is not in effect"
        ));
    }
    let spell: &[(&str, &str)] = match name.as_str() {
        "synchronous" => &[("off", "0"), ("normal", "1"), ("full", "2"), ("extra", "3")],
        "foreign_keys" => &[("off", "0"), ("false", "0"), ("no", "0"), ("on", "1"), ("true", "1"), ("yes", "1")],
        "temp_store" => &[("default", "0"), ("file", "1"), ("memory", "2")],
        "journal_mode" => &[],
        _ => return None,
    };
    let asked = asked?;
    let got: String = c
        .query_row(&format!("PRAGMA {name}"), [], |r| r.get::<_, Sql>(0))
        .map(|v| match v {
            Sql::Integer(i) => i.to_string(),
            Sql::Text(s) => s.to_ascii_lowercase(),
            _ => String::new(),
        })
        .unwrap_or_default();
    // An in-memory database has no journal file to put in another mode.
    let want = spell.iter().find(|(k, _)| *k == asked).map_or(asked.as_str(), |(_, v)| v);
    (want != got && !(name == "journal_mode" && got == "memory")).then(|| {
        format!("\"{pragma}\": SQLite reads back {name} = {got}; the value was not taken (one it doesn't know falls back without an error)")
    })
}

/// Does `sql` begin, end or roll back the connection's transaction? (A
/// `SAVEPOINT`, `RELEASE` or `ROLLBACK TO` nests inside one and is fine.)
pub fn controls_tx(sql: &str) -> bool {
    let mut s = sql.trim_start();
    // Comments first.
    loop {
        if let Some(r) = s.strip_prefix("--") {
            s = r.split_once('\n').map_or("", |(_, r)| r).trim_start();
        } else if let Some(r) = s.strip_prefix("/*") {
            s = r.split_once("*/").map_or("", |(_, r)| r).trim_start();
        } else {
            break;
        }
    }
    let words: Vec<String> =
        s.split_whitespace().take(3).map(|w| w.trim_end_matches(';').to_ascii_uppercase()).collect();
    match words.first().map(String::as_str) {
        Some("BEGIN" | "COMMIT" | "END") => true,
        Some("ROLLBACK") => {
            let next = words.get(1).map(String::as_str);
            !(next == Some("TO") || (next == Some("TRANSACTION") && words.get(2).map(String::as_str) == Some("TO")))
        }
        _ => false,
    }
}

/// One statement, prepared on `c`: refused when it is none, several, or
/// controls the transaction.
fn one<'c>(c: &'c Connection, sql: &str) -> Result<rusqlite::Statement<'c>, Error> {
    if controls_tx(sql) {
        return Err(Error(format!(
            "db: \"{}\" is refused: transactions are the API's (db.batch, db.transaction), so one can't swallow \
             another caller's writes. Nothing was run.",
            sql.trim()
        )));
    }
    match c.prepare(sql) {
        Err(rusqlite::Error::MultipleStatement) => Err(Error(
            "db: one statement per call, and this SQL has several (SQLite would run the first and drop the rest). \
             Run them together with db.batch. Nothing was run."
                .into(),
        )),
        Ok(s) if s.column_count() == 0 && sql.trim().trim_matches(';').trim().is_empty() => {
            Err(Error("db: no statement (the SQL is empty). Nothing was run.".into()))
        }
        r => r.map_err(|e| Error(format!("db: {e}"))),
    }
}

fn rows(c: &Connection, sql: &str, params: &[Sql]) -> Result<Rows, Error> {
    let mut s = one(c, sql)?;
    let columns: Vec<String> = s.column_names().into_iter().map(String::from).collect();
    let n = columns.len();
    let rows = s
        .query_map(rusqlite::params_from_iter(params), |r| (0..n).map(|i| r.get::<_, Sql>(i)).collect())
        .and_then(|it| it.collect::<rusqlite::Result<Vec<Vec<Sql>>>>())
        .map_err(said)?;
    Ok(Rows { columns, rows })
}

fn exec(c: &Connection, sql: &str, params: &[Sql]) -> Result<Done, Error> {
    let mut s = one(c, sql)?;
    // A statement that returns rows (`INSERT … RETURNING`, a pragma) still runs to the end.
    let changes = if s.column_count() > 0 {
        let mut r = s.query(rusqlite::params_from_iter(params)).map_err(said)?;
        while r.next().map_err(said)?.is_some() {}
        c.changes() as usize
    } else {
        s.execute(rusqlite::params_from_iter(params)).map_err(said)?
    };
    Ok(Done { changes, last_id: c.last_insert_rowid() })
}

/// SQLite's error, said plainly; an interrupt is the timeout.
fn said(e: rusqlite::Error) -> Error {
    match e.sqlite_error_code() {
        Some(rusqlite::ErrorCode::OperationInterrupted) => {
            Error("db: the request ran past its timeout and was interrupted: nothing of it was written".into())
        }
        _ => Error(format!("db: {e}")),
    }
}

/// A transaction's statements (`Db::transaction`): the closure's `Ok`
/// commits, its `Err` rolls back.
pub struct Tx<'a>(&'a rusqlite::Transaction<'a>);

impl Tx<'_> {
    pub fn query(&self, sql: &str, params: &[Sql]) -> Result<Rows, Error> {
        rows(self.0, sql, params)
    }

    pub fn execute(&self, sql: &str, params: &[Sql]) -> Result<Done, Error> {
        exec(self.0, sql, params)
    }
}

type Job = Box<dyn FnOnce(&mut Connection) + Send>;

/// Tables, lowercased.
pub type Tables = BTreeSet<String>;

/// The tables the statements of one request read and may write, as SQLite
/// itself names them while it prepares them (its authorizer): views,
/// triggers, `WITH`, quoted and qualified names included, no SQL parsing.
#[derive(Default)]
struct Seen {
    reads: Tables,
    writes: Tables,
}

/// A connection on its own thread. The writer's (`feed`) tells, after each
/// request that changed rows, which tables it may have written.
struct Worker {
    jobs: Option<std::sync::mpsc::Sender<Job>>,
    thread: Option<std::thread::JoinHandle<()>>,
    seen: Arc<Mutex<Seen>>,
    feed: Option<tokio::sync::broadcast::Sender<Arc<Tables>>>,
}

fn lock(m: &Mutex<Seen>) -> std::sync::MutexGuard<'_, Seen> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Worker {
    fn start(
        name: String,
        c: Connection,
        feed: Option<tokio::sync::broadcast::Sender<Arc<Tables>>>,
    ) -> Result<Self, Error> {
        use rusqlite::hooks::{AuthAction as A, Authorization};
        let seen = Arc::new(Mutex::new(Seen::default()));
        let s = seen.clone();
        c.authorizer(Some(move |ctx: rusqlite::hooks::AuthContext<'_>| {
            let mut s = lock(&s);
            match ctx.action {
                A::Read { table_name: t, .. } => _ = s.reads.insert(t.to_lowercase()),
                A::Insert { table_name: t }
                | A::Delete { table_name: t }
                | A::Update { table_name: t, .. }
                | A::DropTable { table_name: t }
                | A::DropTempTable { table_name: t }
                | A::AlterTable { table_name: t, .. } => _ = s.writes.insert(t.to_lowercase()),
                _ => {}
            }
            Authorization::Allow
        }))
        .map_err(said)?;
        let (tx, rx) = std::sync::mpsc::channel::<Job>();
        let thread = std::thread::Builder::new()
            .name(name)
            .spawn(move || {
                let mut c = c;
                for job in rx {
                    // A job that panics fails its caller only.
                    _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(&mut c)));
                }
            })
            .map_err(|e| Error(format!("db: no thread: {e}")))?;
        Ok(Self { jobs: Some(tx), thread: Some(thread), seen, feed })
    }

    /// Run `f` on the connection, within `timeout`; with the tables it read.
    async fn run<R: Send + 'static>(
        &self,
        timeout: Option<Duration>,
        f: impl FnOnce(&mut Connection) -> Result<R, Error> + Send + 'static,
    ) -> Result<(R, Tables), Error> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let (seen, feed) = (self.seen.clone(), self.feed.clone());
        let job: Job = Box::new(move |c| {
            *lock(&seen) = Seen::default();
            let before = c.total_changes();
            let bounded = match timeout {
                Some(t) => {
                    let end = Instant::now() + t;
                    c.progress_handler(1000, Some(move || Instant::now() > end)).map_err(said)
                }
                None => Ok(()),
            };
            let r = bounded.and_then(|()| f(c));
            _ = c.progress_handler(0, None::<fn() -> bool>);
            let Seen { reads, writes } = std::mem::take(&mut *lock(&seen));
            // Committed (a failed request rolled back): the live queries reading these re-run.
            if let Some(feed) = feed.filter(|_| r.is_ok() && c.total_changes() != before && !writes.is_empty()) {
                _ = feed.send(Arc::new(writes));
            }
            _ = tx.send(r.map(|r| (r, reads)));
        });
        let sent = self.jobs.as_ref().is_some_and(|j| j.send(job).is_ok());
        if !sent {
            return Err(Error("db: closed".into()));
        }
        rx.await.map_err(|_| Error("db: the request panicked; nothing of it was committed".into()))?
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Queued requests finish, then the connection closes.
        self.jobs.take();
    }
}

/// An app's own SQLite database. Dropped (or [`Db::close`]d), it finishes
/// what was asked and closes the file.
pub struct Db {
    writer: Worker,
    readers: Vec<Worker>,
    turn: AtomicUsize,
    timeout: Option<Duration>,
    path: PathBuf,
    recovered: Option<crate::Recovered>,
    feed: tokio::sync::broadcast::Sender<Arc<Tables>>,
}

impl Db {
    /// Open (or create, with its folder) the database at `path` (or
    /// `:memory:`). A pragma SQLite didn't take refuses the open.
    pub fn open(path: impl AsRef<Path>, opts: Opts) -> Result<Self, Error> {
        let path = path.as_ref().to_path_buf();
        let memory = path.as_os_str() == ":memory:";
        let mut recovered = None;
        if !memory {
            std::fs::create_dir_all(crate::paths::parent(&path)).map_err(|e| Error(format!("db: {e}")))?;
            if opts.guard {
                crate::store::sweep(&path);
                if let Err(why) = crate::store::healthy(&path) {
                    recovered = Some(crate::store::recover(&path, why)?);
                }
            }
        }
        let connect = |readonly: bool| -> Result<Connection, Error> {
            let c = match readonly {
                true => Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY),
                false => Connection::open(&path),
            }
            .map_err(|e| Error(format!("db: can't open {}: {e}", path.display())))?;
            let set = pragmas(&opts.pragmas, readonly);
            for p in &set {
                c.prepare(p).and_then(|mut s| s.query([]).and_then(|mut r| r.next().map(drop))).map_err(said)?;
            }
            let refused: Vec<String> = set.iter().filter_map(|p| not_taken(&c, p)).collect();
            if !refused.is_empty() {
                return Err(Error(format!("db {}: {}", path.display(), refused.join("; "))));
            }
            Ok(c)
        };
        let c = connect(false)?;
        if opts.guard && !memory {
            crate::store::refresh_good(&c, &path)?;
        }
        let name = path.file_name().map_or("db".into(), |n| n.to_string_lossy().into_owned());
        let (feed, _) = tokio::sync::broadcast::channel(256);
        let writer = Worker::start(format!("aio-db-{name}"), c, Some(feed.clone()))?;
        let readers = (0..if memory { 0 } else { opts.readers })
            .map(|i| connect(true).and_then(|c| Worker::start(format!("aio-db-{name}-r{i}"), c, None)))
            .collect::<Result<_, _>>()?;
        Ok(Self { writer, readers, turn: AtomicUsize::new(0), timeout: opts.timeout, path, recovered, feed })
    }

    /// How the file came back from damage at open ([`Opts::guard`]).
    pub fn recovered(&self) -> Option<&crate::Recovered> {
        self.recovered.as_ref()
    }

    /// The next reader in turn, or the writer when there are none.
    fn reader(&self) -> &Worker {
        match self.readers.len() {
            0 => &self.writer,
            n => &self.readers[self.turn.fetch_add(1, Ordering::Relaxed) % n],
        }
    }

    /// Run one statement and return its rows: a reader answers, unless the
    /// statement writes (then the writer runs it, in order with the rest).
    pub async fn query(&self, sql: &str, params: Vec<Sql>) -> Result<Rows, Error> {
        self.read(sql, params).await.map(|(rows, _)| rows)
    }

    /// `query`, with the tables it read.
    async fn read(&self, sql: &str, params: Vec<Sql>) -> Result<(Rows, Tables), Error> {
        if !self.readers.is_empty() {
            let (s, p) = (sql.to_string(), params.clone());
            let read = self
                .reader()
                .run(self.timeout, move |c| {
                    let reads = one(c, &s)?.readonly();
                    reads.then(|| rows(c, &s, &p)).transpose()
                })
                .await?;
            if let (Some(rows), tables) = read {
                return Ok((rows, tables));
            }
        }
        let s = sql.to_string();
        self.writer.run(self.timeout, move |c| rows(c, &s, &params)).await
    }

    /// Run one write statement.
    pub async fn execute(&self, sql: &str, params: Vec<Sql>) -> Result<Done, Error> {
        let s = sql.to_string();
        self.writer.run(self.timeout, move |c| exec(c, &s, &params)).await.map(|(d, _)| d)
    }

    /// Run these statements in one transaction: all of them, or (on the
    /// first error) none.
    pub async fn batch(&self, stmts: Vec<(String, Vec<Sql>)>) -> Result<Vec<Done>, Error> {
        self.transaction(move |tx| stmts.iter().map(|(s, p)| tx.execute(s, p)).collect()).await
    }

    /// Run `f` in one transaction on the writer: its `Ok` commits, its `Err`
    /// (or a panic) rolls back. Nothing else runs on the writer meanwhile,
    /// so it reads only its own writes and those committed before it.
    pub async fn transaction<R: Send + 'static>(
        &self,
        f: impl FnOnce(&Tx) -> Result<R, Error> + Send + 'static,
    ) -> Result<R, Error> {
        let run = self.writer.run(self.timeout, move |c| {
            let t = c.transaction().map_err(said)?;
            let r = f(&Tx(&t))?;
            t.commit().map_err(said)?;
            Ok(r)
        });
        run.await.map(|(r, _)| r)
    }

    /// A live query (aio's `reactiveDB().select`): its rows now, then again
    /// each time a write through this `Db` changes a table it reads.
    pub async fn live(&self, sql: &str, params: Vec<Sql>) -> Result<Live<'_>, Error> {
        // Listening before the first run: no write between them is missed.
        let feed = self.feed.subscribe();
        let (rows, tables) = self.read(sql, params.clone()).await?;
        if tables.is_empty() {
            eprintln!("[aio] db: this live query reads no table, so nothing will refresh it: {}", sql.trim());
        }
        Ok(Live { db: self, sql: sql.into(), params, rows, tables, feed })
    }

    /// A consistent copy of the live database in `to` (SQLite `VACUUM
    /// INTO`, compacted), flushed to disk; an existing `to` is refused.
    pub async fn snapshot(&self, to: impl AsRef<Path>) -> Result<(), Error> {
        let to = to.as_ref().to_path_buf();
        if to.exists() {
            return Err(Error(format!("db: {} exists; a snapshot never overwrites", to.display())));
        }
        let run = self.writer.run(self.timeout, move |c| {
            c.execute("VACUUM INTO ?1", [to.to_string_lossy()]).map_err(said)?;
            crate::paths::sync_file(&to)
                .and_then(|()| crate::paths::sync_dir(crate::paths::parent(&to)))
                .map_err(|e| Error(format!("db: snapshot {}: {e}", to.display())))
        });
        run.await.map(|(r, _)| r)
    }

    /// SQLite's `quick_check`: what it found wrong, nothing when sound.
    pub async fn check(&self) -> Result<Vec<String>, Error> {
        let (r, _) = self.writer.run(self.timeout, |c| rows(c, "PRAGMA quick_check", &[])).await?;
        let found = r.rows.into_iter().filter_map(|r| match r.into_iter().next() {
            Some(Sql::Text(t)) if t == "ok" => None,
            Some(Sql::Text(t)) => Some(t),
            _ => None,
        });
        Ok(found.collect())
    }

    /// Finish what was asked, then close the file (every thread is done
    /// with it when this returns).
    pub async fn close(mut self) {
        let mut threads: Vec<_> = std::iter::once(&mut self.writer)
            .chain(&mut self.readers)
            .filter_map(|w| {
                w.jobs.take();
                w.thread.take()
            })
            .collect();
        _ = tokio::task::spawn_blocking(move || threads.drain(..).for_each(|t| _ = t.join())).await;
    }

    /// The file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// A live query ([`Db::live`]).
pub struct Live<'a> {
    db: &'a Db,
    sql: String,
    params: Vec<Sql>,
    rows: Rows,
    tables: Tables,
    feed: tokio::sync::broadcast::Receiver<Arc<Tables>>,
}

impl Live<'_> {
    /// Its rows, as of the last run.
    pub fn rows(&self) -> &Rows {
        &self.rows
    }

    /// The tables it reads (a write to one re-runs it).
    pub fn tables(&self) -> &Tables {
        &self.tables
    }

    /// Wait until a write changes its rows: the new rows. A write that leaves
    /// them as they were wakes nobody; writes it fell behind on re-run it once.
    pub async fn changed(&mut self) -> Result<&Rows, Error> {
        use tokio::sync::broadcast::error::RecvError;
        loop {
            match self.feed.recv().await {
                Ok(t) if t.is_disjoint(&self.tables) => continue,
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return Err(Error("db: closed".into())),
            }
            let (rows, tables) = self.db.read(&self.sql, self.params.clone()).await?;
            self.tables = tables;
            if rows != self.rows {
                self.rows = rows;
                return Ok(&self.rows);
            }
        }
    }
}

//! A call on a cell others react to opens a joint commit: one SQLite
//! transaction that its own writes and those of the reactions it causes go
//! into, committed once they all ran, as aio runs a call and its `listensTo`
//! reactions in one reduce (`state/cell-compose-reduce.ts:941-1010`) and
//! persists every cell in one transaction (`server/persistence.ts:1263-1276`).
//!
//! Each cell's store has a connection of its own, as before; a file has one
//! more in this process, shared by every cell kept in it, that a joint
//! commit writes through, used by one thread at a time (its turn, which the
//! thread may take again). Once cells of a file react to each other, a call
//! on any of them takes the file's turn before its cell's lock and keeps it
//! to its end (that file's cells take calls one at a time, as aio's one
//! reduce does): a joint commit holds the turn while it takes the locks of
//! the cells it reaches, so no thread may wait for the turn holding one.
//! A file without reactions never takes the turn: its calls run side by side.
use super::{FileId, file_id};
use rusqlite::{Connection, Statement};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::thread::ThreadId;

/// A file's connection, and whose turn it is.
#[derive(Clone)]
pub(crate) struct Shared(Arc<Inner>);

struct Inner {
    conn: Mutex<Kept>,
    /// Its checkpoints, off the calls' thread (none in memory).
    checkpoints: Option<Arc<Checkpoints>>,
    /// The thread whose turn it is, and how many times it took it.
    turn: Mutex<(Option<ThreadId>, usize)>,
    free: Condvar,
    /// Its cells react to each other: each call takes the turn first.
    reacting: AtomicBool,
    /// Calls running side by side, taking the turn only for the connection.
    loose: AtomicUsize,
}

/// A connection and the statements its commits run, each prepared on its
/// first run and kept for the connection's life: a commit prepared 4 or
/// more from scratch (~3 µs of a call's 7).
pub(crate) struct Kept {
    /// Point into `*conn`; cleared before it is freed ([`Drop`]).
    stmts: RefCell<Vec<(&'static str, Statement<'static>)>>,
    /// Owned (from `Box::into_raw`), held as a pointer, not a `Box`: moving
    /// a `Box` asserts unique access to its target, which the statements'
    /// shared borrows of it would break.
    conn: NonNull<Connection>,
}

// SAFETY: the statements are only ever used through `&Kept`, so on the
// thread that has the connection, and they move with it; a `Connection`
// may move between threads, and `Kept` owns the one `conn` points to.
unsafe impl Send for Kept {}

impl Kept {
    pub(crate) fn new(conn: Connection) -> Self {
        Self { stmts: RefCell::default(), conn: NonNull::from(Box::leak(Box::new(conn))) }
    }

    /// `f` on `sql`'s statement, prepared on its first use and kept. Asked
    /// again from inside `f`, the inner call runs a statement of its own,
    /// prepared for it and not kept.
    pub(crate) fn with<R>(
        &self,
        sql: &'static str,
        f: impl FnOnce(&mut Statement<'_>) -> rusqlite::Result<R>,
    ) -> rusqlite::Result<R> {
        let Ok(mut stmts) = self.stmts.try_borrow_mut() else {
            return f(&mut self.prepare(sql)?);
        };
        let i = match stmts.iter().position(|(s, _)| *s == sql) {
            Some(i) => i,
            None => {
                let s = self.prepare(sql)?;
                // SAFETY: `s` borrows `*self.conn`, which stays where it is
                // and alive until `Drop` has dropped every statement; it is
                // never lent out past `&self`.
                let s = unsafe { std::mem::transmute::<Statement<'_>, Statement<'static>>(s) };
                stmts.push((sql, s));
                stmts.len() - 1
            }
        };
        f(&mut stmts[i].1)
    }
}

impl std::ops::Deref for Kept {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        // SAFETY: `conn` is the live connection `Kept` owns (freed only in
        // `Drop`), and it is only ever lent out shared.
        unsafe { self.conn.as_ref() }
    }
}

impl Drop for Kept {
    fn drop(&mut self) {
        // The statements first: they point into the connection.
        self.stmts.get_mut().clear();
        // SAFETY: `conn` came from `Box::leak` in `new`, is freed only here,
        // and nothing borrows it any more.
        drop(unsafe { Box::from_raw(self.conn.as_ptr()) });
    }
}

/// A file open in this process: its path, identity and shared connection.
type Open = (PathBuf, Option<FileId>, Weak<Inner>);

/// The files open in this process: one shared connection each.
static OPEN: Mutex<Vec<Open>> = Mutex::new(Vec::new());

/// Numbers each joint commit, so a store marks itself once per joint commit.
static JOINTS: AtomicU64 = AtomicU64::new(1);

fn lock<X>(m: &Mutex<X>) -> MutexGuard<'_, X> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(c) = &self.checkpoints {
            c.stop();
        }
    }
}

/// WAL pages that make a checkpoint due (SQLite's own mark).
const CHECKPOINT_PAGES: std::os::raw::c_int = 1000;

/// WAL pages past which a commit checkpoints itself, as SQLite would: the
/// thread can't keep up (writes without a pause, its WAL never restarts).
const CHECKPOINT_LAGGING: std::os::raw::c_int = 4 * CHECKPOINT_PAGES;

/// A file's checkpoints (the WAL copied into the file: two disk syncs) on
/// a thread of their own. SQLite's own ran in the commit that passed 1000
/// pages: every 150 or so calls, one call (and the views waiting on it)
/// took the disk's syncs, 10-30 ms on an SSD.
struct Checkpoints {
    path: PathBuf,
    /// Due, stopped, the thread (started at the first due).
    state: Mutex<(bool, bool, Option<std::thread::JoinHandle<()>>)>,
    due: Condvar,
}

impl Checkpoints {
    /// `c`'s commits make a checkpoint due past [`CHECKPOINT_PAGES`]; it
    /// runs none itself, unless no thread can be had or the WAL is past
    /// [`CHECKPOINT_LAGGING`].
    fn watch(self: &Arc<Self>, c: &mut Connection) -> rusqlite::Result<()> {
        let me = self.clone();
        c.wal_hook(Some(move |wal: &mut rusqlite::hooks::Wal, pages| {
            // Its error would be the commit's, which is made: ignored, as SQLite's own.
            if pages >= CHECKPOINT_LAGGING || (pages >= CHECKPOINT_PAGES && !me.wake()) {
                _ = wal.checkpoint();
            }
            Ok(())
        }))
    }

    /// Make a checkpoint due; false: no thread to run it.
    fn wake(self: &Arc<Self>) -> bool {
        let mut s = lock(&self.state);
        if s.2.is_none() && !s.1 {
            let me = self.clone();
            match std::thread::Builder::new().name("aio-checkpoint".into()).spawn(move || me.run()) {
                Ok(t) => s.2 = Some(t),
                Err(_) => return false,
            }
        }
        s.0 = true;
        self.due.notify_one();
        true
    }

    fn run(&self) {
        // Never made here: a file gone stays gone.
        let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let mut db = None;
        loop {
            {
                let mut s = lock(&self.state);
                while !s.0 && !s.1 {
                    s = self.due.wait(s).unwrap_or_else(|e| e.into_inner());
                }
                if s.1 {
                    return;
                }
                s.0 = false;
            }
            if db.is_none() {
                db = Connection::open_with_flags(&self.path, flags).ok();
            }
            // PASSIVE: the calls' writers never wait for it.
            if let Some(c) = &db
                && let Err(e) = c.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(()))
            {
                eprintln!("[aio] storage: checkpoint of {}: {e}", self.path.display());
            }
        }
    }

    /// End its thread (after the checkpoint it may be running).
    fn stop(&self) {
        let t = {
            let mut s = lock(&self.state);
            s.1 = true;
            self.due.notify_one();
            s.2.take()
        };
        if let Some(t) = t {
            _ = t.join();
        }
    }
}

impl Shared {
    fn new(conn: Connection, checkpoints: Option<Arc<Checkpoints>>) -> Self {
        Self(Arc::new(Inner {
            conn: Mutex::new(Kept::new(conn)),
            checkpoints,
            turn: Mutex::new((None, 0)),
            free: Condvar::new(),
            reacting: AtomicBool::new(false),
            loose: AtomicUsize::new(0),
        }))
    }

    /// The file at `path` (it exists) as the other cells kept in it share
    /// it, else newly: its shared connection opened.
    fn open(path: &Path) -> rusqlite::Result<Self> {
        let mut open = lock(&OPEN);
        open.retain(|(_, _, w)| w.strong_count() > 0);
        let key = (std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()), file_id(path));
        if let Some(i) = open.iter().find(|(p, id, _)| (p, id) == (&key.0, &key.1)).and_then(|(_, _, w)| w.upgrade()) {
            return Ok(Self(i));
        }
        let mut c = Connection::open(path)?;
        c.execute_batch("PRAGMA synchronous = NORMAL")?;
        let checkpoints = Arc::new(Checkpoints { path: key.0.clone(), state: Mutex::new((false, false, None)), due: Condvar::new() });
        checkpoints.watch(&mut c)?;
        let s = Self::new(c, Some(checkpoints));
        open.push((key.0, key.1, Arc::downgrade(&s.0)));
        Ok(s)
    }

    fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// This thread's turn at the file, until the guard drops: no other
    /// thread uses the connection meanwhile.
    pub(crate) fn turn(&self) -> Turn {
        let me = std::thread::current().id();
        let mut t = lock(&self.0.turn);
        while t.0.is_some_and(|o| o != me) {
            t = self.0.free.wait(t).unwrap_or_else(|e| e.into_inner());
        }
        *t = (Some(me), t.1 + 1);
        Turn(self.0.clone())
    }

    /// A call's hold on the file: its turn, taken first and kept, once the
    /// file's cells react to each other; else none (`Err`), the call side
    /// by side with the others, counted until the guard drops.
    pub(crate) fn call(&self) -> Result<Turn, Loose> {
        self.0.loose.fetch_add(1, Ordering::SeqCst);
        let loose = Loose(self.0.clone());
        match self.0.reacting.load(Ordering::SeqCst) {
            true => {
                drop(loose);
                Ok(self.turn())
            }
            false => Err(loose),
        }
    }

    /// From now on its cells react to each other: once the calls already
    /// running side by side end, every call takes the turn first.
    pub(crate) fn react(&self) {
        if self.0.reacting.swap(true, Ordering::SeqCst) {
            return;
        }
        while self.0.loose.load(Ordering::SeqCst) > 0 {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// The shared connection, in this thread's turn. Never twice at once on one thread.
    fn conn(&self) -> FileConn<'_> {
        let turn = self.turn();
        let c = self.0.conn.try_lock().unwrap_or_else(|e| match e {
            std::sync::TryLockError::Poisoned(p) => p.into_inner(),
            std::sync::TryLockError::WouldBlock => panic!("[aio] a store's connection used twice at once (a bug in aio-rs)"),
        });
        FileConn { c, _turn: turn }
    }

    /// Is a joint commit on this file open on this thread?
    fn joined(&self) -> bool {
        JOINT.with(|j| j.borrow().as_ref().is_some_and(|j| j.file.same(self)))
    }
}

/// A store's connections: its own (none in memory: one connection is the
/// whole database there) and its file's shared one, for joint commits.
pub(crate) struct Db {
    own: Option<Kept>,
    file: Shared,
}

impl Db {
    /// The file at `path`, made if missing.
    pub(crate) fn open(path: &Path) -> rusqlite::Result<Self> {
        let mut own = Connection::open(path)?;
        let file = Shared::open(path)?;
        if let Some(c) = &file.0.checkpoints {
            c.watch(&mut own)?;
        }
        Ok(Self { own: Some(Kept::new(own)), file })
    }

    /// A database in memory, its own.
    pub(crate) fn memory() -> rusqlite::Result<Self> {
        Ok(Self { own: None, file: Shared::new(Connection::open_in_memory()?, None) })
    }

    pub(crate) fn file(&self) -> &Shared {
        &self.file
    }

    /// The connection to use: inside this file's joint commit (or in
    /// memory) the shared one, else the store's own.
    pub(crate) fn conn(&self) -> Conn<'_> {
        match &self.own {
            Some(c) if !self.file.joined() => Conn::Own(c),
            _ => Conn::File(self.file.conn()),
        }
    }

    /// Set pragmas `ps` on each of its connections (a pragma is per
    /// connection); one SQLite ignores is refused, never left looking set.
    pub(crate) fn pragmas(&self, ps: &[String]) -> Result<(), String> {
        let file = self.file.conn();
        for c in self.own.iter().chain([&*file]) {
            for p in ps {
                c.execute_batch(p).map_err(|e| format!("[data] pragmas {p:?}: {e}"))?;
                if let Some(why) = crate::db::not_taken(c, p) {
                    return Err(format!("[data] pragmas {why}"));
                }
            }
        }
        Ok(())
    }

    /// Before a write: the joint commit this thread has open on this file
    /// begins (once), and is named (`None`: none open, the write commits on its own).
    pub(crate) fn begin(&self) -> rusqlite::Result<Option<u64>> {
        JOINT.with(|j| {
            let mut j = j.borrow_mut();
            let Some(j) = j.as_mut().filter(|j| j.file.same(&self.file)) else { return Ok(None) };
            if !j.begun {
                self.file.conn().execute_batch("BEGIN IMMEDIATE")?;
                j.begun = true;
            }
            Ok(Some(j.id))
        })
    }
}

/// A thread's turn at a file ([`Shared::turn`]).
pub(crate) struct Turn(Arc<Inner>);

impl Drop for Turn {
    fn drop(&mut self) {
        let mut t = lock(&self.0.turn);
        t.1 -= 1;
        if t.1 == 0 {
            t.0 = None;
            self.0.free.notify_one();
        }
    }
}

/// A call running side by side with the others ([`Shared::call`]).
pub(crate) struct Loose(Arc<Inner>);

impl Drop for Loose {
    fn drop(&mut self) {
        self.0.loose.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The shared connection, while its thread's turn lasts.
pub(crate) struct FileConn<'a> {
    c: MutexGuard<'a, Kept>,
    _turn: Turn,
}

impl std::ops::Deref for FileConn<'_> {
    type Target = Kept;
    fn deref(&self) -> &Kept {
        &self.c
    }
}

/// A store's connection ([`Db::conn`]).
pub(crate) enum Conn<'a> {
    Own(&'a Kept),
    File(FileConn<'a>),
}

impl std::ops::Deref for Conn<'_> {
    type Target = Kept;
    fn deref(&self) -> &Kept {
        match self {
            Conn::Own(c) => c,
            Conn::File(f) => f,
        }
    }
}

/// What follows a call's commits ([`release`]).
#[derive(Clone, Copy)]
pub(crate) enum Then {
    Nothing,
    /// A refused call: its failures counted.
    Failures,
    /// Listeners told, effects started; `wake`: the scheduler too.
    Told { wake: bool },
    /// The schedules fired: told, then those due on other cells called.
    Fired,
}

/// A cell, as a call's hold and a joint commit see it.
pub(crate) trait Member {
    /// The joint commit it wrote in ended: `kept`, or refused (then it is
    /// put back as it was before it wrote there).
    fn end(&self, joint: u64, kept: bool);
    /// Run the reactions to its calls committed since they last ran.
    fn react(&self);
    /// Show what follows its commits (`then`) now; what it returns tells it.
    fn show(self: Arc<Self>, then: Then) -> Box<dyn FnOnce()>;
}

/// What follows a commit, shown then told at the joint commit's end.
type After = (Arc<dyn Member>, Then);

/// A call's hold on its cell's file ([`hold`]), and the joint commit it opened.
pub(crate) struct Hold {
    file: Result<Turn, Loose>,
    owner: Option<Owner>,
}

/// A call's hold on `cell`'s file, taken before its lock: once the file's
/// cells react to each other, its turn (a joint commit holds the turn while
/// it takes the locks of the cells it reaches, so nobody waits for the turn
/// holding one). When other cells react to it (`reacts`), the joint commit
/// its calls and their reactions go into: one this thread has open,
/// joined, or a new one.
pub(crate) fn hold(file: &Shared, reacts: bool, cell: Arc<dyn Member>) -> Hold {
    let turn = file.call();
    let owner = match turn.is_ok() && reacts {
        true => open_joint(file),
        false => None,
    };
    if turn.is_ok() {
        join(Arc::as_ptr(&cell) as *const () as usize, || cell);
    }
    Hold { file: turn, owner }
}

/// A call's end: the reactions to what it committed run now (inside the
/// joint commit), then what follows (`then`: listeners, effects) at the
/// joint commit's end, or now when there is none. Ending the joint commit
/// it opened commits it: refused, nothing of it is kept, and that is the
/// call's answer.
pub(crate) fn release(h: Hold, cell: Arc<dyn Member>, then: Then) -> Result<(), String> {
    cell.react();
    let Some(owner) = h.owner else {
        drop(h.file);
        after(false, (cell, then));
        return Ok(());
    };
    after(true, (cell, then));
    owner.end(h.file.ok())
}

/// The joint commit this thread has open: begun at its first write.
struct Joint {
    id: u64,
    file: Shared,
    begun: bool,
    /// The cells in it, each once (by key).
    joined: Vec<(usize, Arc<dyn Member>)>,
    /// What follows each commit in it, run at its end: each shows its
    /// cell's new state, then (once all are shown) tells its listeners.
    after: Vec<After>,
}

thread_local! {
    static JOINT: RefCell<Option<Joint>> = const { RefCell::new(None) };
}

/// Open a joint commit on `file` (in this thread's turn at it), unless one
/// is open: its owner, who ends it.
fn open_joint(file: &Shared) -> Option<Owner> {
    JOINT.with(|j| {
        let mut j = j.borrow_mut();
        if j.is_some() {
            return None;
        }
        let id = JOINTS.fetch_add(1, Ordering::Relaxed);
        *j = Some(Joint { id, file: file.clone(), begun: false, joined: vec![], after: vec![] });
        Some(Owner { ended: false })
    })
}

/// Join the open joint commit as `key` (once).
fn join(key: usize, m: impl FnOnce() -> Arc<dyn Member>) {
    JOINT.with(|j| {
        if let Some(j) = j.borrow_mut().as_mut()
            && !j.joined.iter().any(|(k, _)| *k == key)
        {
            j.joined.push((key, m()));
        }
    });
}

/// Run `f` at the open joint commit's end (`first`: before the rest), else now.
fn after(first: bool, f: After) {
    let now = JOINT.with(|j| match j.borrow_mut().as_mut() {
        Some(j) if first => {
            j.after.insert(0, f);
            None
        }
        Some(j) => {
            j.after.push(f);
            None
        }
        None => Some(f),
    });
    if let Some((m, t)) = now {
        m.show(t)()
    }
}

/// The thread's joint commit, to end.
struct Owner {
    ended: bool,
}

/// Commit `j` (begun): all of it, or (refused) none, every cell in it put back.
fn finish(j: &Joint, commit: bool) -> Result<(), String> {
    if !j.begun {
        return Ok(());
    }
    let out = {
        let c = j.file.conn();
        match commit {
            true => c.execute_batch("COMMIT").map_err(|e| {
                // Some errors roll it back already: then this one fails too.
                _ = c.execute_batch("ROLLBACK");
                format!("storage: {e}")
            }),
            false => c.execute_batch("ROLLBACK").map_err(|e| e.to_string()).and(Err("rolled back".into())),
        }
    };
    j.joined.iter().rev().for_each(|(_, m)| m.end(j.id, out.is_ok()));
    out
}

impl Owner {
    /// Commit it, then, out of `turn` (the caller's turn at the file), show
    /// and tell what follows each of its commits.
    fn end(mut self, turn: Option<Turn>) -> Result<(), String> {
        self.ended = true;
        let Some(j) = JOINT.with(|j| j.borrow_mut().take()) else { return Ok(()) };
        let out = finish(&j, true);
        drop(turn);
        let tells: Vec<_> = j.after.into_iter().map(|(m, t)| m.show(t)).collect();
        tells.into_iter().for_each(|t| t());
        out
    }
}

impl Drop for Owner {
    /// Unwinding from a panic in it: nothing of it commits.
    fn drop(&mut self) {
        if !self.ended
            && let Some(j) = JOINT.with(|j| j.borrow_mut().take())
        {
            _ = finish(&j, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Kept;
    use rusqlite::Connection;

    fn kept() -> Kept {
        let k = Kept::new(Connection::open_in_memory().unwrap());
        k.execute_batch("CREATE TABLE t (n INTEGER)").unwrap();
        k
    }

    const ADD: &str = "INSERT INTO t (n) VALUES (?1)";
    const COUNT: &str = "SELECT COUNT(*) FROM t";

    #[test]
    fn a_kept_statement_runs_after_its_kept_moves() {
        let k = kept();
        k.with(ADD, |s| s.execute([1])).unwrap();
        // Moved into a Vec, a Mutex and back, its statements still run.
        let k = std::sync::Mutex::new(vec![k].pop().unwrap()).into_inner().unwrap();
        k.with(ADD, |s| s.execute([2])).unwrap();
        let n: i64 = k.with(COUNT, |s| s.query_row([], |r| r.get(0))).unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn a_statement_asked_for_inside_another_runs_one_of_its_own() {
        let k = kept();
        let n: i64 = k
            .with(ADD, |s| {
                s.execute([1])?;
                k.with(ADD, |inner| inner.execute([2]))?;
                k.with(COUNT, |inner| inner.query_row([], |r| r.get(0)))
            })
            .unwrap();
        assert_eq!(n, 2);
        // The nested ones were not kept: only the outer one is.
        assert_eq!(k.stmts.borrow().len(), 1);
    }
}

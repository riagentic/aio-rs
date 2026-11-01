//! A `listens_to` reaction commits in the same SQLite transaction as the
//! call it reacts to (aio's one reduce, one persist transaction).
use aio::prelude::*;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[cell(persist)]
pub struct Orders {
    pub placed: u64,
    pub at: u32,
}

#[methods]
impl Orders {
    pub fn place(&mut self) {
        self.placed += 1;
    }

    #[coalesce]
    pub fn focus(&mut self, at: u32) {
        self.at = at;
    }
}

#[cell(persist)]
pub struct Stock {
    pub taken: u64,
}

#[methods]
impl Stock {
    pub fn take(&mut self) {
        self.taken += 1;
    }
}

/// Events of `cell` on disk, as another connection (another process) sees them.
fn on_disk(f: &Path, cell: &str) -> i64 {
    let db = rusqlite::Connection::open_with_flags(f, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    db.query_row("SELECT COUNT(*) FROM log WHERE cell = ?1", [cell], |r| r.get(0)).unwrap()
}

fn wired(f: &Path) -> (Handle<Orders>, Handle<Stock>) {
    let (orders, stock) = (Handle::<Orders>::open(f).unwrap(), Handle::<Stock>::open(f).unwrap());
    stock.listens_to(&orders, "place", |s, _| s.take());
    (orders, stock)
}

/// aio runs a call and its `listensTo` reactions in one reduce and saves
/// them in one transaction (`state/cell-compose-reduce.ts:941`,
/// `server/persistence.ts:1263`): the reaction runs before its source's
/// commit, and nobody is told of either before both are on disk.
#[test]
fn a_reaction_commits_in_the_same_transaction_as_its_call() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("state.db");
    let (orders, stock) = (Handle::<Orders>::open(&f).unwrap(), Handle::<Stock>::open(&f).unwrap());
    let seen = Arc::new(Mutex::new(vec![]));
    let (s, file) = (seen.clone(), f.clone());
    stock.listens_to(&orders, "place", move |st, _| {
        // Inside the call's commit: not on disk yet, the call nor this.
        s.lock().unwrap().push(("reacting", on_disk(&file, "orders"), on_disk(&file, "stock")));
        st.take()
    });
    let (s, file, st) = (seen.clone(), f.clone(), stock.clone());
    orders.listen(move |o| {
        // Told after both: the reaction's state is there already.
        s.lock().unwrap().push(("told", o.placed as i64, st.state().taken as i64));
        assert_eq!((on_disk(&file, "orders"), on_disk(&file, "stock")), (1, 1));
    });
    orders.place().unwrap();
    assert_eq!(*seen.lock().unwrap(), [("reacting", 0, 0), ("told", 1, 1)]);
    let (o, s) = (orders.store(|s| s.events().unwrap()), stock.store(|s| s.events().unwrap()));
    assert_eq!((o.len(), s.len(), s[0].seq), (1, 1, o[0].seq + 1), "two events, one commit");
}

/// The commit refused at its end (here a deferred foreign key a trigger
/// breaks): neither call is on disk, neither cell's state moved, nobody is
/// told, and the next call commits as usual.
#[test]
fn a_refused_joint_commit_keeps_neither_call() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("state.db");
    let (orders, stock) = wired(&f);
    orders.store(|s| s.set_pragmas(&["PRAGMA foreign_keys = ON".into()])).unwrap();
    let raw = rusqlite::Connection::open(&f).unwrap();
    raw.execute_batch(
        "CREATE TABLE p (id INTEGER PRIMARY KEY);
         CREATE TABLE c (p INTEGER REFERENCES p (id) DEFERRABLE INITIALLY DEFERRED);
         CREATE TRIGGER boom AFTER INSERT ON log WHEN NEW.cell = 'stock' BEGIN INSERT INTO c VALUES (42); END;",
    )
    .unwrap();
    let told = Arc::new(Mutex::new(0));
    let t = told.clone();
    orders.listen(move |_| *t.lock().unwrap() += 1);

    let e = orders.place().unwrap_err();
    assert!(e.why.contains("FOREIGN KEY"), "{e}");
    assert_eq!((orders.state().placed, stock.state().taken, *told.lock().unwrap()), (0, 0, 0));
    assert_eq!((on_disk(&f, "orders"), on_disk(&f, "stock")), (0, 0));

    raw.execute_batch("DROP TRIGGER boom").unwrap();
    orders.place().unwrap();
    assert_eq!((orders.state().placed, stock.state().taken, *told.lock().unwrap()), (1, 1, 1));
    assert_eq!((on_disk(&f, "orders"), on_disk(&f, "stock")), (1, 1));
    drop((orders, stock));
    let (orders, stock) = wired(&f);
    assert_eq!((orders.state().placed, stock.state().taken), (1, 1), "booted as committed");
}

/// The child [`a_kill_mid_commit_leaves_both_calls_or_neither`] kills:
/// places orders until killed, in an aio-rs file or an aio one.
#[test]
fn joint_child() {
    let Ok(f) = std::env::var("AIO_TEST_JOINT") else { return };
    let (orders, stock) = match std::env::var("AIO_TEST_JOINT_AIO") {
        Ok(app) => (Handle::<Orders>::open_aio(&f, &app).unwrap(), Handle::<Stock>::open_aio(&f, &app).unwrap()),
        Err(_) => (Handle::<Orders>::open(&f).unwrap(), Handle::<Stock>::open(&f).unwrap()),
    };
    stock.listens_to(&orders, "place", |s, _| s.take());
    println!("placing");
    loop {
        orders.place().unwrap();
    }
}

/// `kill -9` at any moment of the joint commit: the file holds both calls
/// or neither, in aio-rs's log and in aio's own snapshot (`aio_kv`).
#[test]
fn a_kill_mid_commit_leaves_both_calls_or_neither() {
    use std::io::BufRead;
    let d = tempfile::tempdir().unwrap();
    for aio_file in [false, true] {
        let f = d.path().join(format!("state-{aio_file}.db"));
        for round in 0..12u64 {
            let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
            cmd.args(["--exact", "joint_child", "--nocapture", "--test-threads=1"]).env("AIO_TEST_JOINT", &f);
            if aio_file {
                cmd.env("AIO_TEST_JOINT_AIO", "shop");
            }
            let mut child = cmd.stdout(std::process::Stdio::piped()).spawn().unwrap();
            let mut out = std::io::BufReader::new(child.stdout.take().unwrap());
            let mut line = String::new();
            while !line.contains("placing") {
                line.clear();
                assert!(out.read_line(&mut line).unwrap() > 0, "the child ended early");
            }
            std::thread::sleep(std::time::Duration::from_millis(5 + round * 7 % 40));
            child.kill().unwrap();
            child.wait().unwrap();
            let (o, s) = (on_disk(&f, "orders"), on_disk(&f, "stock"));
            assert_eq!(o, s, "round {round}: {o} orders, {s} takes on disk");
            if aio_file {
                let db = rusqlite::Connection::open_with_flags(&f, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
                let doc: String = db.query_row("SELECT v FROM aio_kv WHERE k = 'state'", [], |r| r.get(0)).unwrap();
                let doc: serde_json::Value = serde_json::from_str(&doc).unwrap();
                assert_eq!(doc["orders"]["placed"], doc["stock"]["taken"], "round {round}: aio's snapshot");
            }
        }
        assert!(on_disk(&f, "orders") > 0, "the children placed orders");
    }
}

/// Calls on the reacting cell from other threads, while its source's
/// calls react to it: no thread waits on another forever, none is lost.
#[test]
fn reactions_and_direct_calls_on_many_threads_never_deadlock() {
    let d = tempfile::tempdir().unwrap();
    let (orders, stock) = wired(&d.path().join("state.db"));
    std::thread::scope(|sc| {
        for _ in 0..3 {
            sc.spawn(|| (0..300).for_each(|_| orders.place().unwrap()));
            sc.spawn(|| (0..300).for_each(|_| stock.take().unwrap()));
        }
    });
    assert_eq!((orders.state().placed, stock.state().taken), (900, 1800));
}

/// A reaction that panics is said and refused; its source's call stands.
#[test]
fn a_panicking_reaction_never_undoes_its_source() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("state.db");
    let (orders, stock) = (Handle::<Orders>::open(&f).unwrap(), Handle::<Stock>::open(&f).unwrap());
    stock.listens_to(&orders, "place", |_, _| -> Result { panic!("boom") });
    orders.place().unwrap();
    assert_eq!((orders.state().placed, on_disk(&f, "orders"), stock.state().taken), (1, 1, 0));
}

/// A reaction to a `#[coalesce]` call runs when its run is written, in that commit.
#[test]
fn a_reaction_to_a_coalesced_run_commits_with_it() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("state.db");
    let (orders, stock) = (Handle::<Orders>::open(&f).unwrap(), Handle::<Stock>::open(&f).unwrap());
    stock.listens_to(&orders, "focus", |s, _| s.take());
    (1..=5).for_each(|i| orders.focus(i).unwrap());
    assert_eq!((stock.state().taken, on_disk(&f, "orders")), (0, 0), "the run is held");
    orders.store(|s| s.flush()).unwrap();
    assert_eq!((stock.state().taken, on_disk(&f, "orders"), on_disk(&f, "stock")), (1, 1, 1));
}

/// Write throughput, calls per second (`--ignored`, best in `--release`).
#[test]
#[ignore]
fn bench_write_throughput() {
    const N: u64 = 20_000;
    let rate = |n: u64, t: Instant| (n as f64 / t.elapsed().as_secs_f64()) as u64;
    let d = tempfile::tempdir().unwrap();

    let alone = Handle::<Orders>::open(d.path().join("alone.db")).unwrap();
    let t = Instant::now();
    (0..N).for_each(|_| alone.place().unwrap());
    println!("bench one cell: {} calls/s", rate(N, t));

    let f = d.path().join("react.db");
    let (orders, stock) = (Handle::<Orders>::open(&f).unwrap(), Handle::<Stock>::open(&f).unwrap());
    stock.listens_to(&orders, "place", |s, _| s.take());
    let t = Instant::now();
    (0..N).for_each(|_| orders.place().unwrap());
    println!("bench call + reaction: {} calls/s", rate(N, t));
    assert_eq!(stock.state().taken, N);

    let t = Instant::now();
    (0..N).for_each(|i| orders.focus(i as u32).unwrap());
    orders.store(|s| s.flush()).unwrap();
    println!("bench coalesced (reacting cell): {} calls/s", rate(N, t));

    let f = d.path().join("threads.db");
    let (a, b) = (Handle::<Orders>::open(&f).unwrap(), Handle::<Stock>::open(&f).unwrap());
    let t = Instant::now();
    std::thread::scope(|sc| {
        for _ in 0..2 {
            sc.spawn(|| (0..N / 4).for_each(|_| a.place().unwrap()));
            sc.spawn(|| (0..N / 4).for_each(|_| b.take().unwrap()));
        }
    });
    println!("bench 4 threads, 2 cells, one file: {} calls/s", rate(N, t));

    let t = Instant::now();
    std::thread::scope(|sc| {
        for _ in 0..2 {
            sc.spawn(|| (0..N / 4).for_each(|_| orders.place().unwrap()));
            sc.spawn(|| (0..N / 4).for_each(|_| stock.take().unwrap()));
        }
    });
    println!("bench 4 threads, 2 reacting cells, one file: {} calls/s", rate(N, t));
}

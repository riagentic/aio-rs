//! An app's own SQLite file (`aio::db`), as aio's `createDB` tests prove it.
use aio::db::{Db, Done, Opts, Sql, controls_tx, pragmas};
use std::time::Duration;

fn text(s: &str) -> Sql {
    Sql::Text(s.into())
}

async fn notes(db: &Db) -> Vec<String> {
    let r = db.query("SELECT t FROM n ORDER BY id", vec![]).await.unwrap();
    r.rows.into_iter().map(|r| if let Sql::Text(t) = &r[0] { t.clone() } else { panic!("{r:?}") }).collect()
}

async fn with_notes(opts: Opts) -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("sub/app.db"), opts).unwrap();
    db.execute("CREATE TABLE n (id INTEGER PRIMARY KEY, t TEXT NOT NULL, b BLOB)", vec![]).await.unwrap();
    (dir, db)
}

#[tokio::test]
async fn query_execute_and_typed_rows() {
    let (_dir, db) = with_notes(Opts::default()).await;
    let d = db.execute("INSERT INTO n (t, b) VALUES (?, ?)", vec![text("a"), Sql::Blob(vec![0, 255])]).await.unwrap();
    assert_eq!(d, Done { changes: 1, last_id: 1 });
    db.execute("INSERT INTO n (t) VALUES (?)", vec![text("b")]).await.unwrap();
    let r = db.query("SELECT id, t, b FROM n WHERE id >= ? ORDER BY id", vec![1.into()]).await.unwrap();
    assert_eq!(r.columns, ["id", "t", "b"]);
    assert_eq!(r.rows[0], [Sql::Integer(1), text("a"), Sql::Blob(vec![0, 255])]);
    #[derive(serde::Deserialize, Debug, PartialEq)]
    struct Note {
        id: i64,
        t: String,
        b: Option<Vec<u8>>,
    }
    let typed: Vec<Note> = r.to().unwrap();
    assert_eq!(typed[1], Note { id: 2, t: "b".into(), b: None });
    let d = db.execute("UPDATE n SET t = upper(t)", vec![]).await.unwrap();
    assert_eq!(d.changes, 2);
    // A statement that returns rows runs to its end.
    let r = db.query("INSERT INTO n (t) VALUES ('c') RETURNING id", vec![]).await.unwrap();
    assert_eq!(r.rows, [[Sql::Integer(3)]]);
    assert_eq!(notes(&db).await, ["A", "B", "c"]);
    // Parameters that don't fit the statement are an error, never NULLs.
    assert!(db.execute("INSERT INTO n (t) VALUES (?)", vec![]).await.is_err());
    assert!(db.execute("INSERT INTO n (t) VALUES (?)", vec![text("x"), text("y")]).await.is_err());
    assert_eq!(notes(&db).await.len(), 3);
}

#[tokio::test]
async fn one_statement_per_call_and_transactions_are_the_apis() {
    let (_dir, db) = with_notes(Opts::default()).await;
    let two = "INSERT INTO n (t) VALUES ('a'); INSERT INTO n (t) VALUES ('b')";
    assert!(db.execute(two, vec![]).await.unwrap_err().0.contains("one statement per call"));
    assert!(db.query(two, vec![]).await.unwrap_err().0.contains("one statement per call"));
    assert!(db.execute("  ;", vec![]).await.unwrap_err().0.contains("no statement"));
    for tx in ["BEGIN", "begin immediate", "COMMIT", "END", "-- why\nROLLBACK"] {
        assert!(db.execute(tx, vec![]).await.unwrap_err().0.contains("transactions are the API's"), "{tx}");
    }
    assert!(notes(&db).await.is_empty(), "nothing ran");
    // A trigger's body is part of one statement.
    let trigger = "CREATE TRIGGER up AFTER INSERT ON n BEGIN UPDATE n SET t = t || '!' WHERE id = new.id; END";
    db.execute(trigger, vec![]).await.unwrap();
    db.execute("INSERT INTO n (t) VALUES ('a')", vec![]).await.unwrap();
    assert_eq!(notes(&db).await, ["a!"]);
    assert!(controls_tx("rollback transaction") && !controls_tx("ROLLBACK TO sp") && !controls_tx("SAVEPOINT a"));
    assert!(!controls_tx("ROLLBACK TRANSACTION TO sp") && !controls_tx("SELECT 'BEGIN'"));
}

#[tokio::test]
async fn a_transaction_commits_whole_or_not_at_all() {
    let (_dir, db) = with_notes(Opts::default()).await;
    let ins = |t: &str| ("INSERT INTO n (t) VALUES (?)".to_string(), vec![text(t)]);
    let done = db.batch(vec![ins("a"), ins("b")]).await.unwrap();
    assert_eq!(done.iter().map(|d| d.last_id).collect::<Vec<_>>(), [1, 2]);
    // The second fails: neither is there.
    let bad = ("INSERT INTO n (t) VALUES (NULL)".to_string(), vec![]);
    assert!(db.batch(vec![ins("c"), bad]).await.is_err());
    assert_eq!(notes(&db).await, ["a", "b"]);
    // A closure: `Ok` commits, `Err` rolls back, and it sees its own writes.
    let seen = db
        .transaction(|tx| {
            tx.execute("INSERT INTO n (t) VALUES ('c')", &[])?;
            Ok(tx.query("SELECT count(*) FROM n", &[])?.rows[0][0].clone())
        })
        .await
        .unwrap();
    assert_eq!(seen, Sql::Integer(3));
    let e = db
        .transaction(|tx| {
            tx.execute("INSERT INTO n (t) VALUES ('d')", &[])?;
            tx.execute("COMMIT", &[])?;
            Ok(())
        })
        .await
        .unwrap_err();
    assert!(e.0.contains("transactions are the API's"));
    // Savepoints nest inside it.
    db.transaction(|tx| {
        tx.execute("SAVEPOINT s", &[])?;
        tx.execute("INSERT INTO n (t) VALUES ('gone')", &[])?;
        tx.execute("ROLLBACK TO s", &[])?;
        tx.execute("RELEASE s", &[])?;
        tx.execute("INSERT INTO n (t) VALUES ('e')", &[])
    })
    .await
    .unwrap();
    assert_eq!(notes(&db).await, ["a", "b", "c", "e"]);
    // Two at once both commit, one after the other.
    let one = db.transaction(|tx| tx.execute("INSERT INTO n (t) VALUES ('f')", &[]));
    let two = db.transaction(|tx| tx.execute("INSERT INTO n (t) VALUES ('g')", &[]));
    let (a, b) = tokio::join!(one, two);
    assert!(a.is_ok() && b.is_ok());
    assert_eq!(notes(&db).await.len(), 6);
    // A panic is a rollback, and the database keeps working.
    let p: Result<(), _> = db
        .transaction(|tx| {
            tx.execute("INSERT INTO n (t) VALUES ('h')", &[])?;
            panic!("the app's bug")
        })
        .await;
    assert!(p.unwrap_err().0.contains("panicked"));
    assert_eq!(notes(&db).await.len(), 6);
}

#[tokio::test]
async fn readers_answer_reads_and_writes_still_go_in_order() {
    let (_dir, db) = with_notes(Opts { readers: 2, ..Default::default() }).await;
    for i in 0..20 {
        db.execute("INSERT INTO n (t) VALUES (?)", vec![text(&format!("n{i}"))]).await.unwrap();
        // Read your writes: a reader sees what was committed before it.
        assert_eq!(notes(&db).await.len(), i + 1);
    }
    // A write sent through query() takes the writer: it is not lost.
    db.query("INSERT INTO n (t) VALUES ('w') RETURNING id", vec![]).await.unwrap();
    assert_eq!(notes(&db).await.last().unwrap(), "w");
    let reads = (0..8).map(|_| db.query("SELECT count(*) FROM n", vec![]));
    assert!(futures_util::future::join_all(reads).await.into_iter().all(|r| r.unwrap().rows[0][0] == Sql::Integer(21)));
    // In memory there is one connection, so reads see writes there too.
    let m = Db::open(":memory:", Opts { readers: 3, ..Default::default() }).unwrap();
    m.execute("CREATE TABLE n (id INTEGER PRIMARY KEY, t TEXT)", vec![]).await.unwrap();
    m.execute("INSERT INTO n (t) VALUES ('m')", vec![]).await.unwrap();
    assert_eq!(notes(&m).await, ["m"]);
}

#[tokio::test]
async fn a_pragma_sqlite_did_not_take_refuses_the_open() {
    let dir = tempfile::tempdir().unwrap();
    let open = |name: &str, p: &[&str]| {
        Db::open(
            dir.path().join(name),
            Opts { pragmas: p.iter().map(|s| s.to_string()).collect(), ..Default::default() },
        )
    };
    let e = open("a.db", &["PRAGMA synchronus = FULL", "PRAGMA synchronous = FUL", "PRAGMA journal_mode = WALL"])
        .err()
        .unwrap()
        .0;
    assert!(e.contains("no pragma \"synchronus\""), "{e}");
    assert!(e.contains("synchronous = 1"), "{e}");
    assert!(e.contains("journal_mode = delete"), "{e}");
    let fine = [
        "PRAGMA synchronous = FULL",
        "PRAGMA main.foreign_keys = off",
        "PRAGMA cache_size=-2000;",
        "PRAGMA temp_store = MEMORY",
    ];
    let db = open("b.db", &fine).unwrap();
    let read = |p: &'static str| db.query(p, vec![]);
    assert_eq!(read("PRAGMA synchronous").await.unwrap().rows[0][0], Sql::Integer(2));
    assert_eq!(read("PRAGMA foreign_keys").await.unwrap().rows[0][0], Sql::Integer(0));
    // The defaults not named stay.
    assert_eq!(read("PRAGMA journal_mode").await.unwrap().rows[0][0], text("wal"));
    assert_eq!(read("PRAGMA busy_timeout").await.unwrap().rows[0][0], Sql::Integer(5000));
    let merged = pragmas(&["pragma Synchronous=FULL".into(), "PRAGMA mmap_size = 0".into()], true);
    assert_eq!(merged.len(), 5);
    assert!(merged.contains(&"pragma Synchronous=FULL".into()) && !merged.iter().any(|p| p.contains("journal_mode")));
}

#[tokio::test]
async fn a_request_past_its_timeout_is_interrupted_and_rolled_back() {
    let (_dir, db) = with_notes(Opts { timeout: Some(Duration::from_millis(50)), ..Default::default() }).await;
    let forever = "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c)";
    let t = std::time::Instant::now();
    let e = db.query(&format!("{forever} SELECT count(*) FROM c"), vec![]).await.unwrap_err();
    assert!(e.0.contains("past its timeout"), "{e}");
    assert!(t.elapsed() < Duration::from_secs(5));
    let e = db.execute(&format!("INSERT INTO n (t) {forever} SELECT x FROM c"), vec![]).await.unwrap_err();
    assert!(e.0.contains("nothing of it was written"));
    assert!(notes(&db).await.is_empty());
    db.execute("INSERT INTO n (t) VALUES ('after')", vec![]).await.unwrap();
    assert_eq!(notes(&db).await, ["after"]);
}

#[tokio::test]
async fn a_snapshot_is_a_consistent_copy_and_check_finds_nothing_wrong() {
    let (dir, db) = with_notes(Opts { readers: 1, ..Default::default() }).await;
    db.execute("INSERT INTO n (t) VALUES ('kept')", vec![]).await.unwrap();
    let to = dir.path().join("copy.db");
    db.snapshot(&to).await.unwrap();
    // aio's rolling snapshot: the same path again holds the newer copy.
    db.execute("INSERT INTO n (t) VALUES ('later')", vec![]).await.unwrap();
    db.snapshot(&to).await.unwrap();
    assert_eq!(
        std::fs::read_dir(dir.path())
            .unwrap()
            .filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().ends_with(".tmp"))
            .count(),
        0
    );
    assert_eq!(db.check().await.unwrap(), Vec::<String>::new());
    let copy = Db::open(&to, Opts::default()).unwrap();
    assert_eq!(notes(&copy).await, ["kept", "later"]);
    copy.close().await;
    db.close().await;
    // Closed: every thread let go of the file.
    std::fs::rename(&to, dir.path().join("moved.db")).unwrap();
}

#[tokio::test]
async fn a_guarded_file_that_is_damaged_boots_from_its_good_copy() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.db");
    let guard = Opts { guard: true, ..Default::default() };
    let db = Db::open(&path, guard.clone()).unwrap();
    db.execute("CREATE TABLE n (id INTEGER PRIMARY KEY, t TEXT)", vec![]).await.unwrap();
    db.execute("INSERT INTO n (t) VALUES ('saved')", vec![]).await.unwrap();
    db.close().await;
    // The good copy is made at open, at most daily: this one is fresh.
    let good = dir.path().join("app.db.good");
    std::fs::remove_file(&good).unwrap();
    Db::open(&path, guard.clone()).unwrap().close().await;
    assert!(good.exists());
    let damage = |p: &std::path::Path| {
        let mut b = std::fs::read(p).unwrap();
        b[..100].fill(7);
        std::fs::write(p, b).unwrap();
    };
    damage(&path);
    let db = Db::open(&path, guard.clone()).unwrap();
    let r = db.recovered().unwrap().clone();
    assert!(r.quarantined.exists(), "the damaged file is kept");
    assert_eq!(notes(&db).await, ["saved"]);
    db.close().await;
    // Unguarded, or without a good copy: never a silent fresh start.
    damage(&path);
    assert!(Db::open(&path, Opts::default()).is_err());
    std::fs::remove_file(&good).unwrap();
    let e = Db::open(&path, guard).err().unwrap();
    assert!(e.0.contains("no good copy"), "{e}");
    assert!(path.exists());
}

/// A folder reached through a link (macOS's `/var` → `/private/var`).
#[cfg(unix)]
#[tokio::test]
async fn a_database_and_its_copies_work_through_a_linked_folder() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("real")).unwrap();
    std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("link")).unwrap();
    let path = dir.path().join("link/app.db");
    let db = Db::open(&path, Opts { guard: true, readers: 1, ..Default::default() }).unwrap();
    db.execute("CREATE TABLE n (id INTEGER PRIMARY KEY, t TEXT)", vec![]).await.unwrap();
    db.execute("INSERT INTO n (t) VALUES ('x')", vec![]).await.unwrap();
    db.snapshot(dir.path().join("link/copy.db")).await.unwrap();
    db.close().await;
    assert!(dir.path().join("real/copy.db").exists() && dir.path().join("real/app.db.good").exists());
    let copy = Db::open(dir.path().join("real/copy.db"), Opts::default()).unwrap();
    assert_eq!(notes(&copy).await, ["x"]);
}

/// The live query's new rows, if a write wakes it soon.
async fn woke(l: &mut aio::db::Live<'_>) -> Option<aio::db::Rows> {
    let r = tokio::time::timeout(Duration::from_millis(300), l.changed()).await.ok()?;
    Some(r.unwrap().clone())
}

#[tokio::test]
async fn a_live_query_reruns_when_a_write_changes_what_it_reads() {
    for readers in [0, 2] {
        let (_dir, db) = with_notes(Opts { readers, ..Default::default() }).await;
        db.execute("CREATE TABLE other (x)", vec![]).await.unwrap();
        let mut l = db.live("SELECT count(*) FROM main.\"N\"", vec![]).await.unwrap();
        assert_eq!((l.rows().rows[0][0].clone(), l.tables().len()), (Sql::Integer(0), 1));
        let count = |r: Option<aio::db::Rows>| r.map(|r| r.rows[0][0].clone());
        db.execute("INSERT INTO n (t) VALUES ('a')", vec![]).await.unwrap();
        assert_eq!(count(woke(&mut l).await), Some(Sql::Integer(1)));
        // Every way a write lands: upsert, replace, RETURNING through query(), a batch, a truncating delete.
        db.execute("INSERT INTO n (id, t) VALUES (1, 'b') ON CONFLICT (id) DO UPDATE SET t = 'b'", vec![])
            .await
            .unwrap();
        assert_eq!(woke(&mut l).await, None, "the count didn't change: nobody woken");
        db.execute("INSERT OR REPLACE INTO n (id, t) VALUES (2, 'c')", vec![]).await.unwrap();
        assert_eq!(count(woke(&mut l).await), Some(Sql::Integer(2)));
        db.query("INSERT INTO n (t) VALUES ('d') RETURNING id", vec![]).await.unwrap();
        assert_eq!(count(woke(&mut l).await), Some(Sql::Integer(3)));
        db.batch(vec![("INSERT INTO n (t) VALUES ('e')".into(), vec![])]).await.unwrap();
        assert_eq!(count(woke(&mut l).await), Some(Sql::Integer(4)));
        db.execute("DELETE FROM n", vec![]).await.unwrap();
        assert_eq!(count(woke(&mut l).await), Some(Sql::Integer(0)));
        // Another table, or a write that rolled back: nothing.
        db.execute("INSERT INTO other VALUES (1)", vec![]).await.unwrap();
        assert!(
            db.batch(vec![("INSERT INTO n (t) VALUES ('x')".into(), vec![]), ("bad".into(), vec![])]).await.is_err()
        );
        assert_eq!(woke(&mut l).await, None);
    }
}

#[tokio::test]
async fn a_live_query_wakes_on_writes_through_any_db_of_its_file() {
    let (dir, db) = with_notes(Opts::default()).await;
    let mut l = db.live("SELECT count(*) FROM n", vec![]).await.unwrap();
    // Another `Db` on the same file, named another way: one feed.
    std::fs::create_dir(dir.path().join("sub/x")).unwrap();
    let other = Db::open(dir.path().join("sub/x/../app.db"), Opts::default()).unwrap();
    other.execute("INSERT INTO n (t) VALUES ('a')", vec![]).await.unwrap();
    assert_eq!(woke(&mut l).await.map(|r| r.rows[0][0].clone()), Some(Sql::Integer(1)));
    // Closed, it leaves the feed to the rest; another file's writes wake nobody.
    other.close().await;
    db.execute("INSERT INTO n (t) VALUES ('b')", vec![]).await.unwrap();
    assert_eq!(woke(&mut l).await.map(|r| r.rows[0][0].clone()), Some(Sql::Integer(2)));
    let (_d2, elsewhere) = with_notes(Opts::default()).await;
    elsewhere.execute("INSERT INTO n (t) VALUES ('c')", vec![]).await.unwrap();
    assert_eq!(woke(&mut l).await, None);
}

#[tokio::test]
async fn a_live_query_knows_its_tables_through_views_joins_and_triggers() {
    let (_dir, db) = with_notes(Opts { readers: 1, ..Default::default() }).await;
    for s in [
        "CREATE TABLE tag (note INTEGER, name TEXT)",
        "CREATE TABLE log (what TEXT)",
        "CREATE VIEW tagged AS SELECT n.t, tag.name FROM n JOIN tag ON tag.note = n.id",
        "CREATE TRIGGER logged AFTER INSERT ON n BEGIN INSERT INTO log VALUES (new.t); END",
    ] {
        db.execute(s, vec![]).await.unwrap();
    }
    let mut view = db.live("WITH v AS (SELECT * FROM tagged) SELECT count(*) FROM v", vec![]).await.unwrap();
    assert_eq!(view.tables().iter().map(String::as_str).collect::<Vec<_>>(), ["n", "tag", "tagged"]);
    let mut log = db.live("SELECT what FROM [log]", vec![]).await.unwrap();
    db.execute("INSERT INTO n (t) VALUES ('a')", vec![]).await.unwrap();
    // The trigger wrote the log; the view's join has no tag yet.
    assert_eq!(woke(&mut log).await.unwrap().rows, [[text("a")]]);
    assert_eq!(woke(&mut view).await, None);
    db.execute("INSERT INTO tag VALUES (1, 'x')", vec![]).await.unwrap();
    assert_eq!(woke(&mut view).await.unwrap().rows, [[Sql::Integer(1)]]);
}

/// aio's `createDB({ readonly })`: reads what the app wrote, writes nothing.
#[tokio::test]
async fn a_read_only_database_reads_and_refuses_every_write() {
    let (dir, db) = with_notes(Opts::default()).await;
    db.execute("INSERT INTO n (t) VALUES ('kept')", vec![]).await.unwrap();
    let path = dir.path().join("sub/app.db");
    let ro = Db::open(&path, Opts { readonly: true, readers: 1, ..Default::default() }).unwrap();
    assert_eq!(notes(&ro).await, ["kept"]);
    let e = ro.execute("INSERT INTO n (t) VALUES ('no')", vec![]).await.unwrap_err();
    assert!(e.0.contains("readonly"), "{e}");
    assert!(Db::open(dir.path().join("none.db"), Opts { readonly: true, ..Default::default() }).is_err());
    assert_eq!(notes(&db).await, ["kept"]);
}

/// aio's `where` objects: a filter, an order and a page, values always
/// parameters, names always plain.
#[tokio::test]
async fn a_where_selects_by_parameters_and_refuses_what_could_inject() {
    use aio::db::{Order, Select, Where};
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("shop.db"), Opts::default()).unwrap();
    db.execute("CREATE TABLE orders (id INTEGER PRIMARY KEY, who TEXT, total REAL, note TEXT)", vec![]).await.unwrap();
    let evil = "x'); DROP TABLE orders; --";
    for (who, total, note) in
        [("ana", 5.0, Some("gift")), ("bo", 20.0, None), ("cy", 50.0, Some(evil)), ("di", 8.0, None)]
    {
        let note: Sql = note.map(String::from).into();
        let row = vec![text(who), Sql::Real(total), note];
        db.execute("INSERT INTO orders (who, total, note) VALUES (?, ?, ?)", row).await.unwrap();
    }
    let whos = |r: aio::db::Rows| r.rows.iter().map(|r| r[1].clone()).collect::<Vec<_>>();
    let q = |w: Where| Select::from("orders").filter(w).order_by("total", Order::Asc);
    let (sql, params) =
        q(Where::gt("total", 6.0).and(Where::ne("who", "bo".to_string()))).limit(5).offset(1).sql().unwrap();
    assert_eq!(sql, "SELECT * FROM orders WHERE (total > ?) AND (who != ?) ORDER BY total ASC LIMIT 5 OFFSET 1");
    assert_eq!(params, [Sql::Real(6.0), text("bo")]);
    let got = |w| {
        let db = &db;
        async move { whos(db.select(&q(w)).await.unwrap()) }
    };
    assert_eq!(got(Where::lt("total", 10.0)).await, [text("ana"), text("di")]);
    assert_eq!(got(Where::is_in("who", ["bo", "cy"].map(String::from))).await, [text("bo"), text("cy")]);
    assert_eq!(got(Where::is_in("who", Vec::<String>::new())).await, Vec::<Sql>::new(), "none in: no row");
    assert_eq!(got(Where::eq("note", Sql::Null)).await, [text("di"), text("bo")]);
    assert_eq!(got(Where::like("note", "g%").or(Where::gte("total", 50))).await, [text("ana"), text("cy")]);
    // A value is data, whatever it holds: matched as text, never run.
    assert_eq!(got(Where::eq("note", evil.to_string())).await, [text("cy")]);
    assert_eq!(got(Where::eq("who", "' OR 1=1 --".to_string())).await, Vec::<Sql>::new());
    // aio's object form: fields all, a list any, operators by name.
    let w = Where::object(&json!({ "total": { "gte": 8, "lt": 50 }, "note": null })).unwrap();
    assert_eq!(got(w).await, [text("di"), text("bo")]);
    let w = Where::object(&json!([{ "who": "ana" }, { "who": { "in": ["cy"] } }])).unwrap();
    assert_eq!(got(w).await, [text("ana"), text("cy")]);
    for bad in [json!({ "who": { "regex": "a" } }), json!({ "who": [1] }), json!({ "who": { "like": 1 } }), json!(3)] {
        assert!(Where::object(&bad).is_err(), "{bad}");
    }
    // A name goes into the statement: only a plain one.
    for name in ["who; DROP TABLE orders", "who --", "1who", "", "\"who\""] {
        let e = Select::from("orders").filter(Where::eq(name, 1)).sql().unwrap_err();
        assert!(e.0.contains("not a plain"), "{name}: {e:?}");
        assert!(Select::from("orders").order_by(name, Order::Desc).sql().is_err(), "{name}");
        assert!(Select::from(name).sql().is_err(), "{name}");
    }
    let many = Where::is_in("id", 0..40_000i64);
    assert!(Select::from("orders").filter(many).sql().unwrap_err().0.contains("at most 32766"));
    assert_eq!(db.query("SELECT count(*) FROM orders", vec![]).await.unwrap().rows[0][0], Sql::Integer(4), "all there");
    db.close().await;
}

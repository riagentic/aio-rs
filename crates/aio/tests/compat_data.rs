//! An aio app's data, both ways: aio's own `state.db` (a copy of aio's golden
//! `aio-1.0.9-clean-stop` fixture) opens in aio-rs, and after aio-rs ran it still reads
//! as aio wrote it (docs/migration/data.md).
use aio::compat::data::Home;
use aio::prelude::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const APP: &str = "sync-listener-journal-probe";

/// aio's `tally`: no version (0), its slice holds `__aio_status`.
#[cell(persist, version = 0)]
pub struct Tally {
    n: u32,
}

#[methods]
impl Tally {
    pub fn on_add(&mut self) {
        self.n += 1;
    }
}

/// aio's `inbox`, ported at version 1: its aio data (stamp 0) migrates.
#[cell(persist)]
pub struct Inbox {
    posts: Vec<String>,
}

#[methods]
impl Inbox {
    pub fn post(&mut self, t: String) {
        self.posts.push(t);
    }
}

/// aio's `shaped` stores `{saved}`: a port that declares `n` drops it.
#[cell(persist, id = "shaped", version = 0)]
pub struct Shaped {
    n: u32,
}

#[methods]
impl Shaped {}

/// A copy of aio's fixture home (aio's files are never opened in place).
fn fixture() -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let home = d.path().join("home");
    std::fs::create_dir_all(home.join("data")).unwrap();
    // Built in: the test runs on machines without the source tree.
    for (f, b) in [
        ("state.db", &include_bytes!("fixtures/aio-1.0.9-clean-stop/state.db")[..]),
        ("meta.json", include_bytes!("fixtures/aio-1.0.9-clean-stop/meta.json")),
    ] {
        std::fs::write(home.join("data").join(f), b).unwrap();
    }
    (d, home)
}

/// Every `aio_kv` row, its value parsed.
fn kv(p: &Path) -> BTreeMap<String, Value> {
    let db = rusqlite::Connection::open_with_flags(p, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let mut q = db.prepare("SELECT k, v FROM aio_kv").unwrap();
    q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .map(|(k, v)| (k, serde_json::from_str(&v).unwrap()))
        .collect()
}

/// aio's other tables, as text.
fn aio_tables(p: &Path) -> String {
    let db = rusqlite::Connection::open_with_flags(p, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let mut out = String::new();
    for t in ["aio_schema", "sync_ops", "sync_snapshots", "sync_meta", "sync_compacted_ids"] {
        let mut q = db.prepare(&format!("SELECT * FROM {t} ORDER BY 1")).unwrap();
        let n = q.column_count();
        let rows = q
            .query_map([], |r| {
                (0..n)
                    .map(|i| r.get::<_, rusqlite::types::Value>(i).map(|v| format!("{v:?}")))
                    .collect::<Result<Vec<_>, _>>()
            })
            .unwrap();
        for row in rows {
            out += &format!("{t}: {}\n", row.unwrap().join(" | "));
        }
    }
    out
}

/// Everything in the file, as text: a refused file must stay as it was.
fn dump(p: &Path) -> String {
    let db = rusqlite::Connection::open_with_flags(p, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let schema: Vec<String> = db
        .prepare("SELECT name FROM sqlite_master ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    format!("{schema:?}\n{:?}\n{}", kv(p), aio_tables(p))
}

#[test]
fn an_aio_file_opens_in_aio_rs_and_stays_readable_by_aio() {
    let (_d, home) = fixture();
    let h = Home::open(&home, APP, None).unwrap();
    let db = h.state_db();
    let (before, tables) = (kv(&db), aio_tables(&db));

    // aio → aio-rs: each cell starts from aio's snapshot.
    let mut tally = Store::<Tally>::open_aio(&db, APP).unwrap();
    assert_eq!(tally.state().n, 10);
    let mut inbox = Store::<Inbox>::open_aio(&db, APP).unwrap();
    assert_eq!(inbox.state().posts, ["p16", "p18", "p19", "p21", "p22"]);
    // Stamped 0, ported at 1: migrated, with a backup first.
    assert!(Path::new(&format!("{}.v1-v0.bak", db.display())).exists());
    assert_eq!(tally.events().unwrap().len(), 1, "aio's data is the first event");

    tally.call(&By::Test, "on_add", Value::Null).unwrap();
    inbox.call(&By::Test, "post", json!(["p30"])).unwrap();

    // aio-rs → aio: the same rows, in aio's shapes; nothing else changed.
    let after = kv(&db);
    assert_eq!(
        after["state"],
        json!({
            "tally": { "n": 11, "__aio_status": "active" },
            "inbox": { "posts": ["p16", "p18", "p19", "p21", "p22", "p30"] },
            "shaped": { "saved": 10 },
        })
    );
    assert_eq!(after[&format!("{APP}:__schema")], json!(1));
    assert_eq!(after[&format!("{APP}:__persistKey")], json!("state"));
    // A version 0 cell is not stamped (as in aio); the sync cells' stamps stay.
    assert_eq!(after[&format!("{APP}:__versions")], json!({ "notes": 1, "mirror": 1, "feed": 1, "inbox": 1 }));
    let changed: Vec<&str> = after.keys().filter(|k| before.get(*k) != after.get(*k)).map(String::as_str).collect();
    assert_eq!(changed, ["state", &format!("{APP}:__persistKey"), &format!("{APP}:__versions")]);
    assert_eq!(aio_tables(&db), tables);
    drop((tally, inbox));

    // Reopened: aio-rs's own history, no second adoption.
    let tally = Store::<Tally>::open_aio(&db, APP).unwrap();
    assert_eq!((tally.state().n, tally.events().unwrap().len()), (11, 2));
}

#[test]
fn a_ported_app_boots_on_its_aio_home_and_data() {
    let (_d, home) = fixture();
    let cfg =
        aio::config::Config::parse(&format!("[app]\nid = \"tally-port-{}\"\nport_of = \"{APP}\"", std::process::id()))
            .unwrap();
    let ask = aio::paths::Ask { home: Some(home.display().to_string()), ..Default::default() };
    let dirs = aio::paths::AppDirs::planned(&cfg.app.id, aio::paths::plan_app(&cfg, &ask).unwrap()).unwrap();
    assert_eq!((&dirs.data, &dirs.logs, dirs.port_of.as_deref()), (&home.join("data"), &home.join("logs"), Some(APP)));
    // aio's claim is the lock: aio (or a second aio-rs) can't open the home now.
    assert_eq!(Home::open(&home, APP, None).unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
    assert!(aio::paths::running(&dirs.data));
    let again = aio::paths::AppDirs::planned(&cfg.app.id, aio::paths::plan_app(&cfg, &ask).unwrap());
    assert_eq!(again.unwrap_err().kind(), std::io::ErrorKind::WouldBlock);

    let db = dirs.data.join("state.db");
    let s = aio::Source::<Tally>::for_app(&cfg, &dirs).unwrap();
    assert_eq!(s.state().n, 10, "aio's snapshot");
    s.call("on_add", Value::Null).unwrap();
    assert_eq!(kv(&db)["state"]["tally"], json!({ "n": 11, "__aio_status": "active" }), "written as aio reads it");
    // aio's meta.json is aio's: kept byte for byte.
    let meta = std::fs::read(home.join("data/meta.json")).unwrap();
    assert_eq!(meta, include_bytes!("fixtures/aio-1.0.9-clean-stop/meta.json"));
    _ = std::fs::remove_dir_all(aio::paths::sockets(&cfg.app.id, &dirs.data).unwrap());
}

#[test]
fn a_new_aio_home_says_whose_it_is() {
    let d = tempfile::tempdir().unwrap();
    let home = d.path().join(".notes-dev");
    drop(Home::open(&home, "notes", Some("dev")).unwrap());
    let meta: Value = serde_json::from_slice(&std::fs::read(home.join("data/meta.json")).unwrap()).unwrap();
    assert_eq!((meta["appId"].as_str(), meta["profile"].as_str()), (Some("notes"), Some("dev")));
    assert_eq!(meta["createdAt"], meta["updatedAt"]);
    // So the plain app, or another, never shares its database.
    assert!(Home::open(&home, "notes", None).unwrap_err().to_string().contains("profile"));
    assert!(Home::open(&home, "notes-dev", None).unwrap_err().to_string().contains("belongs to app"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&home).unwrap().permissions().mode() & 0o777, 0o700);
    }
}

#[test]
fn what_aio_wrote_since_comes_in_as_a_new_event() {
    let (_d, home) = fixture();
    let db = Home::open(&home, APP, None).unwrap().state_db();
    let mut s = Store::<Tally>::open_aio(&db, APP).unwrap();
    s.call(&By::Test, "on_add", Value::Null).unwrap();
    drop(s);
    // aio runs and saves its document.
    let c = rusqlite::Connection::open(&db).unwrap();
    c.execute("UPDATE aio_kv SET v = json_set(v, '$.tally.n', 50) WHERE k = 'state'", []).unwrap();
    drop(c);
    let s = Store::<Tally>::open_aio(&db, APP).unwrap();
    assert_eq!(s.state().n, 50);
    assert_eq!(s.events().unwrap().len(), 3);
    // Time travel still reaches what aio-rs had.
    assert_eq!(s.state_at(2).unwrap().n, 11);
}

#[test]
fn undeclared_aio_data_is_refused_and_the_file_kept() {
    let (_d, home) = fixture();
    let db = Home::open(&home, APP, None).unwrap().state_db();
    let before = dump(&db);
    let e = Store::<Shaped>::open_aio(&db, APP).err().unwrap().0;
    assert!(e.contains("doesn't declare (saved)"), "{e}");
    assert_eq!(dump(&db), before);
    // A plain aio-rs open never adopts an aio file.
    assert!(Store::<Tally>::open(&db).err().unwrap().0.contains("not an aio-rs store"));
    assert_eq!(dump(&db), before);
}

/// An aio file made by hand: `aio_kv` with `rows`.
fn aio_file(rows: &[(&str, Value)]) -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.db");
    let c = rusqlite::Connection::open(&p).unwrap();
    c.execute_batch("CREATE TABLE aio_kv (k TEXT PRIMARY KEY, v TEXT NOT NULL)").unwrap();
    for (k, v) in rows {
        c.execute("INSERT INTO aio_kv (k, v) VALUES (?1, ?2)", [*k, &v.to_string()]).unwrap();
    }
    (d, p)
}

#[test]
fn the_multi_layout_and_another_persist_key_are_kept() {
    let (_d, p) = aio_file(&[
        ("app:__persistKey", json!("doc")),
        ("doc\x1ftally", json!({ "n": 3 })),
        ("doc\x1fother", json!({ "x": [1] })),
        ("doc\x1f__aio_parked", json!({ "y": 2 })),
    ]);
    let mut s = Store::<Tally>::open_aio(&p, "app").unwrap();
    assert_eq!(s.state().n, 3);
    s.call(&By::Test, "on_add", Value::Null).unwrap();
    let rows = kv(&p);
    assert_eq!(rows["doc\x1ftally"], json!({ "n": 4 }));
    assert_eq!(rows["doc\x1fother"], json!({ "x": [1] }));
    assert_eq!(rows["doc\x1f__aio_parked"], json!({ "y": 2 }));
    assert!(!rows.contains_key("doc") && !rows.contains_key("state"));
    assert_eq!(rows["app:__persistKey"], json!("doc"));
}

#[test]
fn a_new_home_gets_a_file_aio_can_open() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.db");
    let mut s = Store::<Inbox>::open_aio(&p, "app").unwrap();
    assert!(s.state().posts.is_empty());
    s.call(&By::Test, "post", json!(["a"])).unwrap();
    let rows = kv(&p);
    assert_eq!(rows["state"], json!({ "inbox": { "posts": ["a"] } }));
    assert_eq!(rows["app:__versions"], json!({ "inbox": 1 }));
}

#[test]
fn files_aio_would_refuse_are_refused_and_kept() {
    let cases: [(&[(&str, Value)], &str); 4] = [
        (&[("app:__schema", json!(2))], "newer aio"),
        (&[("state", json!([1]))], "not an object of cells"),
        (
            &[("state", json!({ "inbox": { "posts": "one" } })), ("app:__versions", json!({ "inbox": 2 }))],
            "newer build (v2 > v1) doesn't fit this build",
        ),
        (&[("state", json!({}))], "a table `log`"),
    ];
    for (i, (rows, why)) in cases.into_iter().enumerate() {
        let (_d, p) = aio_file(rows);
        if i == 3 {
            rusqlite::Connection::open(&p).unwrap().execute_batch("CREATE TABLE log (id INTEGER)").unwrap();
        }
        let before = std::fs::read(&p).unwrap();
        let e = Store::<Inbox>::open_aio(&p, "app").err().unwrap().0;
        assert!(e.contains(why), "{e}");
        assert_eq!(std::fs::read(&p).unwrap(), before, "{why}");
    }
    let (_d, p) = aio_file(&[]);
    rusqlite::Connection::open(&p).unwrap().execute_batch("CREATE TABLE aio_schema (id INTEGER PRIMARY KEY, version INTEGER NOT NULL); INSERT INTO aio_schema VALUES (1, 2)").unwrap();
    assert!(Store::<Inbox>::open_aio(&p, "app").err().unwrap().0.contains("aio_schema 2"));
}

#[test]
fn the_home_is_claimed_and_checked_as_aio_does() {
    let (_d, home) = fixture();
    let h = Home::open(&home, APP, None).unwrap();
    let info: Value = serde_json::from_slice(&std::fs::read(home.join(".aio-instance.json")).unwrap()).unwrap();
    assert_eq!((info["appId"].as_str(), info["pid"].as_u64()), (Some(APP), Some(u64::from(std::process::id()))));
    let e = Home::open(&home, APP, None).unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::WouldBlock);
    assert!(e.to_string().contains(&format!("pid {}", std::process::id())), "{e}");
    drop(h);
    assert!(Home::open(&home, "other", None).unwrap_err().to_string().contains("belongs to app"));
    // 1.0.9's meta.json names no profile: a profile boot is not refused.
    drop(Home::open(&home, APP, Some("dev")).unwrap());
    let meta = home.join("data/meta.json");
    std::fs::write(&meta, json!({ "appId": APP, "profile": "dev" }).to_string()).unwrap();
    assert!(Home::open(&home, APP, None).unwrap_err().to_string().contains("profile"));
    drop(Home::open(&home, APP, Some("dev")).unwrap());
    // An aio journal not yet folded is the store's to replay, not the claim's to refuse.
    std::fs::write(home.join("data/journal"), b"{}\n").unwrap();
    drop(Home::open(&home, APP, Some("dev")).unwrap());
}

/// A copy of aio's `v1.0.9-crashed-streaming` home: killed after its save
/// at journal entry 10, two `inbox:post` calls in the journal past it.
fn crashed() -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let data = d.path().join("home/data");
    std::fs::create_dir_all(&data).unwrap();
    for (f, b) in [
        ("state.db", &include_bytes!("fixtures/aio-1.0.9-crashed-streaming/state.db")[..]),
        ("meta.json", include_bytes!("fixtures/aio-1.0.9-crashed-streaming/meta.json")),
        ("journal", include_bytes!("fixtures/aio-1.0.9-crashed-streaming/journal")),
        ("journal.base", include_bytes!("fixtures/aio-1.0.9-crashed-streaming/journal.base")),
    ] {
        std::fs::write(data.join(f), b).unwrap();
    }
    (d, data.join("state.db"))
}

const WM: &str = "sync-listener-journal-probe:__journal_wm";

#[test]
fn an_aio_journal_past_its_save_is_replayed_once_and_aio_finds_it_folded() {
    let (_d, db) = crashed();
    let posts = |s: &Store<Inbox>| s.state().posts.clone();
    let s = Store::<Inbox>::open_aio(&db, APP).unwrap();
    assert_eq!(posts(&s).len(), 12);
    assert_eq!(posts(&s)[10..], ["p54", "p59"], "the two calls the kill left in the journal");
    drop(s);
    // aio's next boot: the snapshot holds them, the watermark says so (nothing replayed twice).
    assert_eq!(kv(&db)[WM], json!(12));
    let cells = aio_reads(&db, APP);
    assert_eq!(cells["inbox"]["posts"].as_array().unwrap().len(), 12);
    assert_eq!(cells["tally"], json!({ "n": 21, "__aio_status": "active" }), "other cells as aio left them");
    let s = Store::<Inbox>::open_aio(&db, APP).unwrap();
    assert_eq!(posts(&s).len(), 12, "aio-rs's own next boot replays nothing again");
    assert_eq!(s.events().unwrap().iter().filter(|e| e.method == "post").count(), 2);
}

#[test]
fn a_journal_of_another_cells_calls_is_left_to_aio() {
    let (_d, db) = crashed();
    let s = Store::<Tally>::open_aio(&db, APP).unwrap();
    assert_eq!(s.state().n, 21);
    drop(s);
    assert_eq!(kv(&db)[WM], json!(10), "the watermark is the inbox's to move");
}

#[test]
fn a_journal_aio_rs_cant_replay_refuses_the_boot_and_keeps_the_file() {
    let line = |l: &str| format!("{l}\n");
    for (tail, why) in [
        (line(r#"{"seq":11,"type":"aio:__timeTravel","payload":{},"ts":1}"#), "carries state"),
        (line(r#"{"seq":11,"type":"inbox:__setPosts","payload":{},"ts":1}"#), "carries state"),
        (line(r#"{"seq":11,"type":"custom","payload":{},"ts":1}"#), "no call aio-rs can replay"),
        (
            line(r#"{"seq":11,"type":"inbox:post","payload":{"args":["a"]},"ts":1}"#)
                + &line(r#"{"seq":12,"type":"tally:onAdd","payload":{"args":[]},"ts":1}"#),
            "and of another cell (tally:onAdd)",
        ),
    ] {
        let (_d, db) = crashed();
        std::fs::write(db.with_file_name("journal"), tail).unwrap();
        let before = dump(&db);
        let e = Store::<Inbox>::open_aio(&db, APP).err().unwrap().0;
        assert!(e.contains(why) && e.contains("start the aio app once"), "{e}");
        assert_eq!(dump(&db), before);
    }
    // The store put back to before what the journal dropped: a gap only aio may judge.
    let (_d, db) = crashed();
    std::fs::write(db.with_file_name("journal.base"), r#"{"wm":11,"cells":{}}"#).unwrap();
    assert!(Store::<Inbox>::open_aio(&db, APP).err().unwrap().0.contains("put back to an older copy"));
}

#[test]
fn torn_withheld_and_bookkeeping_lines_are_skipped_as_aio_skips_them() {
    let (_d, db) = crashed();
    let lines = [
        r#"{"seq":9,"type":"inbox:post","payload":{"args":["old"]},"ts":1}"#,
        r#"{"seq":11,"type":"__aioBatch","entries":[{"seq":11,"type":"inbox:post","payload":{"args":["a"]},"ts":1}]}"#,
        r#"{"seq":12,"type":"inbox:post","payload":"[redacted]","redacted":true,"ts":1}"#,
        r#"{"seq":13,"type":"__aioSyncApplied","ts":1}"#,
        r#"{"seq":14,"type":"inbox:po"#,
    ];
    std::fs::write(db.with_file_name("journal"), lines.join("\n")).unwrap();
    let s = Store::<Inbox>::open_aio(&db, APP).unwrap();
    assert_eq!(s.state().posts.last().map(String::as_str), Some("a"), "the batch's call, not the one before the save");
    assert_eq!(s.state().posts.len(), 11);
    drop(s);
    assert_eq!(kv(&db)[WM], json!(13), "past every entry it read");
}

/// What aio reads of the file at `p` for app `app` (`stored-state.ts`): an
/// intact SQLite file, `aio_kv` rows of JSON, a schema it knows, and every
/// cell's slice in either layout. A file aio would refuse fails the test.
fn aio_reads(p: &Path, app: &str) -> BTreeMap<String, Value> {
    let db = rusqlite::Connection::open_with_flags(p, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let ok: String = db.query_row("PRAGMA integrity_check", [], |r| r.get(0)).unwrap();
    assert_eq!(ok, "ok");
    let rows = kv(p);
    assert!(rows.get(&format!("{app}:__schema")).is_none_or(|v| v.as_i64() <= Some(1)));
    let key = rows.get(&format!("{app}:__persistKey")).and_then(Value::as_str).unwrap_or("state");
    let mut cells: BTreeMap<String, Value> = rows
        .iter()
        .filter_map(|(k, v)| Some((k.strip_prefix(&format!("{key}\x1f"))?.to_string(), v.clone())))
        .collect();
    if let Some(doc) = rows.get(key) {
        let doc = doc.as_object().expect("aio refuses a snapshot that isn't an object");
        cells.extend(doc.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    cells
}

/// A purse's accounts: the balance is never saved (`#[transient]`, aio's
/// `persist: { exclude }`).
#[cell(persist, id = "accounts", version = 0)]
pub struct Accounts {
    names: Vec<String>,
    #[transient]
    balance: u64,
}

#[methods]
impl Accounts {
    pub fn add(&mut self, n: String) {
        self.names.push(n);
        self.balance += 1;
    }
}

/// An aio app's 30 cells in one file, one row each (`multi`, aio's
/// `persistMode`) or one document row; two of them this port's.
fn thirty(multi: bool) -> (tempfile::TempDir, PathBuf) {
    let mut cells: Vec<(String, Value)> =
        (0..28).map(|i| (format!("c{i:02}"), json!({ "i": i, "__aio_status": "active" }))).collect();
    cells.push(("tally".into(), json!({ "n": 7 })));
    cells.push(("accounts".into(), json!({ "names": ["a"] })));
    let mut rows: Vec<(String, Value)> = vec![
        ("w:__schema".into(), json!(1)),
        ("w:__persistKey".into(), json!("state")),
        ("w:__versions".into(), json!({ "c00": 3, "c27": 1 })),
    ];
    match multi {
        true => rows.extend(cells.into_iter().map(|(c, v)| (format!("state\x1f{c}"), v))),
        false => rows.push(("state".into(), Value::Object(cells.into_iter().collect()))),
    }
    let rows: Vec<(&str, Value)> = rows.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    aio_file(&rows)
}

#[test]
fn many_cells_share_one_aio_file_in_either_layout_and_aio_reads_every_one() {
    for multi in [false, true] {
        let (_d, p) = thirty(multi);
        let before = aio_reads(&p, "w");
        let mut tally = Store::<Tally>::open_aio(&p, "w").unwrap();
        let mut accounts = Store::<Accounts>::open_aio(&p, "w").unwrap();
        // Each cell reads its own slice.
        assert_eq!((tally.state().n, &accounts.state().names[..]), (7, &["a".to_string()][..]));
        tally.call(&By::Test, "on_add", Value::Null).unwrap();
        accounts.call(&By::Test, "add", json!(["b"])).unwrap();
        let after = aio_reads(&p, "w");
        assert_eq!(after.len(), 30, "multi = {multi}");
        assert_eq!(after["tally"], json!({ "n": 8 }));
        // What is never saved is never in aio's file either.
        assert_eq!(after["accounts"], json!({ "names": ["a", "b"] }));
        assert_eq!(accounts.state().balance, 1);
        // The 28 cells this port doesn't have are kept as aio wrote them.
        for (c, v) in before.iter().filter(|(c, _)| c.starts_with('c')) {
            assert_eq!(&after[c], v, "{c}");
        }
        let rows = kv(&p);
        assert_eq!(rows["w:__versions"], json!({ "c00": 3, "c27": 1 }), "version 0 cells: unstamped, as in aio");
        assert_eq!(rows.contains_key("state"), !multi, "the file's own layout");
    }
}

#[test]
fn cells_writing_at_once_lose_no_write_in_the_one_document_row() {
    let (_d, p) = thirty(false);
    std::thread::scope(|s| {
        s.spawn(|| {
            let mut t = Store::<Tally>::open_aio(&p, "w").unwrap();
            (0..40).for_each(|_| _ = t.call(&By::Test, "on_add", Value::Null).unwrap());
        });
        s.spawn(|| {
            let mut a = Store::<Accounts>::open_aio(&p, "w").unwrap();
            (0..40).for_each(|i| _ = a.call(&By::Test, "add", json!([format!("n{i}")])).unwrap());
        });
    });
    let cells = aio_reads(&p, "w");
    assert_eq!(cells["tally"]["n"], json!(47));
    assert_eq!(cells["accounts"]["names"].as_array().unwrap().len(), 41);
    assert_eq!(cells.len(), 30);
}

#[test]
fn app_tables_beside_aio_kv_are_the_apps_both_ways() {
    let (_d, p) = thirty(true);
    // aio's app table (a purse's records of inactive profiles), as aio made it.
    let deno = rusqlite::Connection::open(&p).unwrap();
    deno.execute_batch(
        "CREATE TABLE account_store (id INTEGER PRIMARY KEY, profile_id INTEGER NOT NULL, row TEXT NOT NULL);
         CREATE INDEX account_store_profile ON account_store (profile_id);
         INSERT INTO account_store VALUES (1, 2, '{\"name\":\"old\"}');",
    )
    .unwrap();
    drop(deno);
    let mut tally = Store::<Tally>::open_aio(&p, "w").unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
    rt.block_on(async {
        let db = aio::db::Db::open(&p, aio::db::Opts::default()).unwrap();
        let r = db.query("SELECT row FROM account_store WHERE profile_id = ?", vec![2.into()]).await.unwrap();
        assert_eq!(r.rows, [[aio::db::Sql::Text("{\"name\":\"old\"}".into())]]);
        let row = aio::db::Sql::Text("{\"name\":\"new\"}".into());
        db.execute("INSERT INTO account_store VALUES (?, ?, ?)", vec![2.into(), 3.into(), row]).await.unwrap();
        // The cells commit beside it, in the same file.
        tally.call(&By::Test, "on_add", Value::Null).unwrap();
        db.close().await;
    });
    // aio reads its table and its cells.
    let deno = rusqlite::Connection::open_with_flags(&p, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let rows: Vec<(i64, i64, String)> = deno
        .prepare("SELECT id, profile_id, row FROM account_store ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(rows, [(1, 2, "{\"name\":\"old\"}".into()), (2, 3, "{\"name\":\"new\"}".into())]);
    assert_eq!(aio_reads(&p, "w")["tally"], json!({ "n": 8 }));
}

#[test]
fn a_ported_app_opens_its_aio_store_by_name_and_retires_a_stray_one() {
    let (_d, home) = fixture();
    let data = home.join("data");
    // aio's `dbPath: <home>/data/vault.db`; an older build's state.db beside it.
    std::fs::rename(data.join("state.db"), data.join("vault.db")).unwrap();
    std::fs::write(data.join("state.db"), b"stale").unwrap();
    std::fs::write(data.join("state.db-wal"), b"stale wal").unwrap();
    let toml = format!(
        "[app]\nid = \"vault-port-{}\"\nport_of = \"{APP}\"\n[data]\nstore = \"vault.db\"\nretire = [\"state.db\"]",
        std::process::id()
    );
    let cfg = aio::config::Config::parse(&toml).unwrap();
    let ask = aio::paths::Ask { home: Some(home.display().to_string()), ..Default::default() };
    let dirs = aio::paths::AppDirs::planned(&cfg.app.id, aio::paths::plan_app(&cfg, &ask).unwrap()).unwrap();
    assert_eq!(dirs.store, data.join("vault.db"));
    let s = aio::Source::<Tally>::for_app(&cfg, &dirs).unwrap();
    assert_eq!(s.state().n, 10, "aio's snapshot, from vault.db");
    s.call("on_add", Value::Null).unwrap();
    assert_eq!(aio_reads(&data.join("vault.db"), APP)["tally"], json!({ "n": 11, "__aio_status": "active" }));
    // Set aside, never deleted; no new state.db.
    assert_eq!(std::fs::read(data.join("state.db.pre-migration")).unwrap(), b"stale");
    assert_eq!(std::fs::read(data.join("state.db-wal.pre-migration")).unwrap(), b"stale wal");
    assert!(!data.join("state.db").exists());
    // A later stray stays where it is: the first copy is never replaced.
    std::fs::write(data.join("state.db"), b"later").unwrap();
    assert!(aio::compat::data::retire(&data, &["state.db".into()]).unwrap().is_empty());
    assert_eq!(std::fs::read(data.join("state.db.pre-migration")).unwrap(), b"stale");
    drop(s);
    _ = std::fs::remove_dir_all(aio::paths::sockets(&cfg.app.id, &dirs.data).unwrap());
}

#[test]
fn a_store_by_another_name_backs_up_and_restores_once() {
    let d = tempfile::tempdir().unwrap();
    let dirs = aio::paths::AppDirs::at(d.path(), "w").unwrap();
    let vault = dirs.data.join("vault.db");
    let mut s = Store::<Tally>::open_aio(&vault, "w").unwrap();
    s.call(&By::Test, "on_add", Value::Null).unwrap();
    drop(s);
    std::fs::write(dirs.data.join("notes.txt"), b"kept").unwrap();
    let file = d.path().join("w.backup");
    aio::backup::backup_all("w", &vault, &file).unwrap();
    let held = rusqlite::Connection::open(&file).unwrap();
    let paths: Vec<String> = held
        .prepare("SELECT path FROM aio_files")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(paths, ["notes.txt"], "the store is the backup itself, never a file in it");
    drop(held);
    let mut s = Store::<Tally>::open_aio(&vault, "w").unwrap();
    s.call(&By::Test, "on_add", Value::Null).unwrap();
    drop(s);
    aio::backup::restore("w", &file, &vault, &dirs).unwrap().unwrap();
    assert_eq!(Store::<Tally>::open_aio(&vault, "w").unwrap().state().n, 1, "the backed-up store, in place");
    assert_eq!(aio_reads(&vault, "w")["tally"], json!({ "n": 1 }));
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Contact {
    id: i64,
    name: String,
    email: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Tag {
    tag: String,
    n: f64,
}

/// aio's `db: { contacts: table(…), "book.tags": { table, shape: "map" } }`.
#[cell(persist, id = "book", version = 0)]
pub struct Book {
    title: String,
    #[table("contacts")]
    contacts: Vec<Contact>,
    #[table("book_tags", key = "tag")]
    tags: BTreeMap<String, Tag>,
}

#[methods]
impl Book {
    pub fn add(&mut self, id: i64, name: String) {
        self.contacts.push(Contact { id, name, email: None });
    }
    pub fn rename(&mut self, id: i64, name: String) {
        self.contacts.iter_mut().filter(|c| c.id == id).for_each(|c| c.name = name.clone());
    }
    pub fn drop_first(&mut self) {
        self.contacts.remove(0);
    }
    pub fn tag(&mut self, tag: String, n: f64) {
        self.tags.insert(tag.clone(), Tag { tag, n });
    }
    pub fn retitle(&mut self, t: String) {
        self.title = t;
    }
}

/// An aio file of a `db:` app: the bound arrays live in their tables (aio
/// strips them from `aio_kv`), the rest of the slice in `aio_kv`.
fn bound_file(synced: bool) -> (tempfile::TempDir, PathBuf) {
    let (d, p) = aio_file(&[("state", json!({ "book": { "title": "Mine" } }))]);
    let c = rusqlite::Connection::open(&p).unwrap();
    c.execute_batch(
        "CREATE TABLE aio_schema (id INTEGER PRIMARY KEY, version INTEGER NOT NULL); INSERT INTO aio_schema VALUES (1, 1);
         CREATE TABLE contacts (id INTEGER PRIMARY KEY, name TEXT NOT NULL, email TEXT);
         INSERT INTO contacts VALUES (1, 'Ada', 'ada@example.com'), (2, 'Bo', NULL);
         CREATE TABLE book_tags (tag TEXT PRIMARY KEY, n REAL NOT NULL);
         INSERT INTO book_tags VALUES ('red', 1.5);",
    )
    .unwrap();
    if synced {
        c.execute_batch(
            "CREATE TABLE aio_synced_tables (name TEXT PRIMARY KEY NOT NULL); INSERT INTO aio_synced_tables VALUES ('contacts'), ('book_tags');",
        )
        .unwrap();
    }
    (d, p)
}

/// Table `t`'s rows as aio reads them, as text.
fn table(p: &Path, t: &str) -> Vec<String> {
    let c = rusqlite::Connection::open_with_flags(p, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let mut q = c.prepare(&format!("SELECT * FROM {t} ORDER BY 1")).unwrap();
    let n = q.column_count();
    q.query_map([], |r| {
        Ok((0..n)
            .map(|i| format!("{:?}", r.get::<_, rusqlite::types::Value>(i).unwrap()))
            .collect::<Vec<_>>()
            .join("|"))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

#[test]
fn a_cells_db_tables_are_its_fields_both_ways() {
    let (_d, p) = bound_file(true);
    let mut s = Store::<Book>::open_aio(&p, "app").unwrap();
    // The rows are the fields; the rest of the slice is aio_kv's.
    assert_eq!(s.state().title, "Mine");
    let ada = Contact { id: 1, name: "Ada".into(), email: Some("ada@example.com".into()) };
    assert_eq!(s.state().contacts, [ada, Contact { id: 2, name: "Bo".into(), email: None }]);
    assert_eq!(s.state().tags["red"], Tag { tag: "red".into(), n: 1.5 });
    s.call(&By::Test, "add", json!([3, "Cy"])).unwrap();
    s.call(&By::Test, "rename", json!([2, "Bob"])).unwrap();
    s.call(&By::Test, "drop_first", Value::Null).unwrap();
    s.call(&By::Test, "tag", json!(["blue", 2.0])).unwrap();
    // aio reads its rows, and a slice without them (aio strips bound arrays).
    assert_eq!(table(&p, "contacts"), ["Integer(2)|Text(\"Bob\")|Null", "Integer(3)|Text(\"Cy\")|Null"]);
    assert_eq!(table(&p, "book_tags"), ["Text(\"blue\")|Real(2.0)", "Text(\"red\")|Real(1.5)"]);
    assert_eq!(aio_reads(&p, "app")["book"], json!({ "title": "Mine" }));
    // Only what changed is written: a rowid kept is a row not rewritten.
    let rowid = |name: &str| -> i64 {
        rusqlite::Connection::open(&p)
            .unwrap()
            .query_row("SELECT rowid FROM book_tags WHERE tag = ?1", [name], |r| r.get(0))
            .unwrap()
    };
    let red = rowid("red");
    s.call(&By::Test, "retitle", json!(["Ours"])).unwrap();
    s.call(&By::Test, "tag", json!(["green", 3.0])).unwrap();
    assert_eq!(rowid("red"), red);
    drop(s);
    // aio ran: a row it added comes in.
    let c = rusqlite::Connection::open(&p).unwrap();
    c.execute("INSERT INTO contacts VALUES (9, 'Vi', NULL)", []).unwrap();
    drop(c);
    let s = Store::<Book>::open_aio(&p, "app").unwrap();
    assert_eq!(s.state().contacts.iter().map(|c| c.id).collect::<Vec<_>>(), [2, 3, 9]);
    assert_eq!((s.state().title.as_str(), s.state().tags.len()), ("Ours", 3));
}

#[test]
fn an_empty_table_aio_never_synced_adopts_the_array_and_rows_aio_cant_hold_are_refused() {
    let (_d, p) = bound_file(false);
    let c = rusqlite::Connection::open(&p).unwrap();
    c.execute_batch("DELETE FROM contacts").unwrap();
    c.execute(
        "UPDATE aio_kv SET v = ?1 WHERE k = 'state'",
        [json!({ "book": { "title": "T", "contacts": [{ "id": 5, "name": "Seed", "email": null }] } }).to_string()],
    )
    .unwrap();
    drop(c);
    let mut s = Store::<Book>::open_aio(&p, "app").unwrap();
    // aio's rule: an empty table nobody wrote never empties the array.
    assert_eq!(s.state().contacts[0].name, "Seed");
    s.call(&By::Test, "retitle", json!(["U"])).unwrap();
    assert_eq!(table(&p, "contacts"), ["Integer(5)|Text(\"Seed\")|Null"], "written on the first commit");
    assert!(table(&p, "aio_synced_tables").contains(&"Text(\"contacts\")".to_string()));
    // A table without the column refuses the call, nothing written.
    rusqlite::Connection::open(&p).unwrap().execute_batch("ALTER TABLE contacts DROP COLUMN email").unwrap();
    let before = table(&p, "contacts");
    let e = s.call(&By::Test, "add", json!([6, "No"])).unwrap_err().why;
    assert!(e.contains("book.contacts (aio table contacts): the table has no column email"), "{e}");
    assert_eq!(table(&p, "contacts"), before);
    assert_eq!(s.state().contacts.len(), 1);
}

#[test]
fn without_its_table_a_bound_field_stays_in_the_slice_for_aio_to_adopt() {
    let (_d, p) = aio_file(&[("state", json!({ "book": { "title": "T" } }))]);
    let mut s = Store::<Book>::open_aio(&p, "app").unwrap();
    s.call(&By::Test, "add", json!([1, "A"])).unwrap();
    let book = &aio_reads(&p, "app")["book"];
    assert_eq!(book["contacts"], json!([{ "id": 1, "name": "A", "email": null }]));
}

/// aio's `db: { contacts: table({ id: pk(), name: text(), email: text({ nullable: true }) }) }`.
#[cell(persist, id = "shelf", version = 0)]
pub struct Shelf {
    #[table("contacts", columns = "id INTEGER PRIMARY KEY, name TEXT NOT NULL, email TEXT")]
    contacts: Vec<Contact>,
}

#[methods]
impl Shelf {
    pub fn add(&mut self, id: i64, name: String) {
        self.contacts.push(Contact { id, name, email: None });
    }
}

/// The same table, its email now required.
#[cell(persist, id = "shelf", version = 0)]
pub struct Strict {
    #[table("contacts", columns = "id INTEGER PRIMARY KEY, name TEXT NOT NULL, email TEXT NOT NULL")]
    contacts: Vec<Contact>,
}

#[methods]
impl Strict {}

/// An aio file whose `contacts` table was made as `ddl`, with `rows`.
fn shaped_file(ddl: &str, rows: &str) -> (tempfile::TempDir, PathBuf) {
    let (d, p) = aio_file(&[("state", json!({ "shelf": {} }))]);
    let c = rusqlite::Connection::open(&p).unwrap();
    c.execute_batch(&format!("CREATE TABLE contacts ({ddl}); {rows}")).unwrap();
    (d, p)
}

fn ddl(p: &Path) -> String {
    let c = rusqlite::Connection::open(p).unwrap();
    c.query_row("SELECT sql FROM sqlite_master WHERE name = 'contacts'", [], |r| r.get(0)).unwrap()
}

/// aio tests/db-schema-drift.test.ts, db-init-schema.test.ts.
#[test]
fn a_declared_table_is_made_and_a_stored_one_gains_what_it_lacks() {
    // A fresh file: the table is made as declared, and the rows go there.
    let (_d, p) = aio_file(&[]);
    let mut s = Store::<Shelf>::open_aio(&p, "app").unwrap();
    assert!(ddl(&p).contains("name TEXT NOT NULL, email TEXT"), "{}", ddl(&p));
    s.call(&By::Test, "add", json!([1, "Ada"])).unwrap();
    assert_eq!(table(&p, "contacts"), ["Integer(1)|Text(\"Ada\")|Null"]);
    assert!(aio_reads(&p, "app")["shelf"].get("contacts").is_none(), "aio reads them from the table");
    drop(s);
    assert_eq!(Store::<Shelf>::open_aio(&p, "app").unwrap().state().contacts[0].name, "Ada", "and so does aio-rs");

    // A stored table without the nullable column: added, the rows kept.
    let (_d, p) = shaped_file("id INTEGER PRIMARY KEY, NAME TEXT NOT NULL", "INSERT INTO contacts VALUES (1, 'Bo');");
    let s = Store::<Shelf>::open_aio(&p, "app").unwrap();
    assert_eq!(s.state().contacts, [Contact { id: 1, name: "Bo".into(), email: None }], "NAME takes the declared case");
    assert_eq!(table(&p, "contacts"), ["Integer(1)|Text(\"Bo\")|Null"]);
}

#[test]
fn a_column_rows_cant_fill_refuses_the_boot_unless_the_table_is_empty() {
    // Rows, and a NOT NULL column without a default to add: refused, nothing changed.
    let (_d, p) = shaped_file("id INTEGER PRIMARY KEY, name TEXT NOT NULL", "INSERT INTO contacts VALUES (1, 'Bo');");
    let before = ddl(&p);
    let e = Store::<Strict>::open_aio(&p, "app").err().unwrap().0;
    assert!(e.contains("shelf.contacts: aio table contacts: it lacks column(s) email and holds 1 row(s)"), "{e}");
    assert_eq!(ddl(&p), before);
    // Empty: made again in the declared shape, its own index kept.
    let (_d, p) =
        shaped_file("id INTEGER PRIMARY KEY, name TEXT NOT NULL", "CREATE UNIQUE INDEX by_name ON contacts (name);");
    Store::<Strict>::open_aio(&p, "app").unwrap();
    assert!(ddl(&p).contains("email TEXT NOT NULL"), "{}", ddl(&p));
    let c = rusqlite::Connection::open(&p).unwrap();
    assert!(c.execute("INSERT INTO contacts VALUES (1, 'A', 'a'), (2, 'A', 'b')", []).is_err(), "the index is kept");
}

#[test]
fn a_stored_not_null_column_no_longer_declared_refuses_the_boot() {
    let rows = "INSERT INTO contacts VALUES (1, 'Bo', NULL, '555');";
    let (_d, p) = shaped_file("id INTEGER PRIMARY KEY, name TEXT NOT NULL, email TEXT, phone TEXT NOT NULL", rows);
    let e = Store::<Shelf>::open_aio(&p, "app").err().unwrap().0;
    assert!(e.contains("a NOT NULL column phone the app no longer declares"), "{e}");
    // With a default, the schema passes (new rows get the default); its
    // stored values are data no field declares, kept by refusing the boot.
    let (_d, p) =
        shaped_file("id INTEGER PRIMARY KEY, name TEXT NOT NULL, email TEXT, phone TEXT NOT NULL DEFAULT ''", rows);
    let e = Store::<Shelf>::open_aio(&p, "app").err().unwrap().0;
    assert!(e.contains("fields this build doesn't declare (contacts.0.phone)"), "{e}");
    let (_d, p) =
        shaped_file("id INTEGER PRIMARY KEY, name TEXT NOT NULL, email TEXT, phone TEXT NOT NULL DEFAULT ''", "");
    let mut s = Store::<Shelf>::open_aio(&p, "app").unwrap();
    s.call(&By::Test, "add", json!([1, "Cy"])).unwrap();
    assert_eq!(table(&p, "contacts"), ["Integer(1)|Text(\"Cy\")|Null|Text(\"\")"]);
}

#[test]
fn an_aio_rs_stores_bound_tables_are_its_fields_both_ways() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("app.db");
    let mut s = Store::<Shelf>::open(&p).unwrap();
    assert!(ddl(&p).contains("name TEXT NOT NULL, email TEXT"), "made as declared: {}", ddl(&p));
    s.call(&By::Test, "add", json!([1, "Ada"])).unwrap();
    s.call(&By::Test, "add", json!([2, "Bo"])).unwrap();
    assert_eq!(table(&p, "contacts"), ["Integer(1)|Text(\"Ada\")|Null", "Integer(2)|Text(\"Bo\")|Null"]);
    drop(s);
    // The app's own SQL on the table: its rows come in as a new event.
    let c = rusqlite::Connection::open(&p).unwrap();
    c.execute("UPDATE contacts SET email = 'bo@example.com' WHERE id = 2", []).unwrap();
    drop(c);
    let s = Store::<Shelf>::open(&p).unwrap();
    assert_eq!(s.state().contacts[1].email.as_deref(), Some("bo@example.com"));
    assert_eq!(s.events().unwrap().len(), 3);
    drop(s);
    // Unchanged: no new event.
    assert_eq!(Store::<Shelf>::open(&p).unwrap().events().unwrap().len(), 3);
    // A stored NOT NULL column without a default it doesn't declare: refused (aio's rule).
    let c = rusqlite::Connection::open(&p).unwrap();
    c.execute_batch("DELETE FROM contacts; ALTER TABLE contacts ADD COLUMN phone TEXT NOT NULL").unwrap();
    drop(c);
    let e = Store::<Shelf>::open(&p).err().unwrap().0;
    assert!(e.contains("a NOT NULL column phone the app no longer declares"), "{e}");
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Person {
    id: i64,
    name: String,
}

#[cell(persist, id = "roll")]
pub struct Roll {
    #[table("people", columns = "id INTEGER PRIMARY KEY, name TEXT NOT NULL")]
    people: Vec<Person>,
}

#[methods]
impl Roll {
    pub fn add(&mut self, id: i64, name: String) {
        self.people.push(Person { id, name });
    }
}

/// `Roll` at 2: names in capitals.
#[cell(persist, id = "roll", version = 2, migrate = loud)]
pub struct Roll2 {
    #[table("people", columns = "id INTEGER PRIMARY KEY, name TEXT NOT NULL")]
    people: Vec<Person>,
}

fn loud(old: Value, _: u32) -> Result<Roll2, String> {
    let mut r: Roll2 = serde_json::from_value(old).map_err(|e| e.to_string())?;
    r.people.iter_mut().for_each(|p| p.name = p.name.to_uppercase());
    Ok(r)
}

#[methods]
impl Roll2 {}

/// A cell bound to `log`, aio-rs's own table.
#[cell(persist, id = "clash")]
pub struct Clash {
    #[table("log")]
    rows: Vec<Person>,
}

#[methods]
impl Clash {}

#[test]
fn a_migrated_aio_rs_store_writes_its_tables_in_the_new_shape_and_aios_own_are_never_bound() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("app.db");
    let mut s = Store::<Roll>::open(&p).unwrap();
    s.call(&By::Test, "add", json!([1, "ada"])).unwrap();
    drop(s);
    let s = Store::<Roll2>::open(&p).unwrap();
    assert_eq!(s.state().people[0].name, "ADA", "migrated, not the older rows");
    assert_eq!(table(&p, "people"), ["Integer(1)|Text(\"ADA\")"]);
    drop(s);
    assert_eq!(Store::<Roll2>::open(&p).unwrap().state().people[0].name, "ADA");
    let e = Store::<Clash>::open(d.path().join("clash.db")).err().unwrap().0;
    assert!(e.contains("clash.rows: table log is aio-rs's own"), "{e}");
}

/// One aio-rs cell owning two of aio's fixture cells (`#[aio_cell]`), and a field of
/// its own that aio never sees.
#[cell(persist, id = "desk")]
pub struct Desk {
    #[aio_cell]
    tally: Tally,
    #[aio_cell]
    inbox: Inbox,
    note: String,
}

#[methods]
impl Desk {
    pub fn both(&mut self, t: String) {
        self.tally.n += 1;
        self.inbox.posts.push(t);
    }
    pub fn jot(&mut self, t: String) {
        self.note = t;
    }
}

#[test]
fn one_cell_owns_many_aio_cells_of_aios_own_file() {
    let (_d, home) = fixture();
    let h = Home::open(&home, APP, None).unwrap();
    let db = h.state_db();
    let before = kv(&db);
    let mut s = Store::<Desk>::open_aio(&db, APP).unwrap();
    // Each slice at its own version: `inbox` (stamp 0) migrated to 1, backed up first.
    assert_eq!((s.state().tally.n, s.state().inbox.posts.len()), (10, 5));
    assert!(Path::new(&format!("{}.v1-inbox-v0.bak", db.display())).exists());
    s.call(&By::Test, "both", json!(["p30"])).unwrap();
    s.call(&By::Test, "jot", json!(["mine"])).unwrap();
    let after = kv(&db);
    assert_eq!(
        after["state"],
        json!({
            "tally": { "n": 11, "__aio_status": "active" },
            "inbox": { "posts": ["p16", "p18", "p19", "p21", "p22", "p30"] },
            "shaped": { "saved": 10 },
        }),
        "each slice in aio's shape, `__*` and unknown cells kept, aio-rs's own field not there"
    );
    assert_eq!(after[&format!("{APP}:__versions")], json!({ "notes": 1, "mirror": 1, "feed": 1, "inbox": 1 }));
    let changed: Vec<&str> = after.keys().filter(|k| before.get(*k) != after.get(*k)).map(String::as_str).collect();
    assert_eq!(changed, ["state", &format!("{APP}:__persistKey"), &format!("{APP}:__versions")]);
    drop(s);
    // Reopened: aio-rs's history, its own field kept, no second adoption.
    let s = Store::<Desk>::open_aio(&db, APP).unwrap();
    assert_eq!((s.state().tally.n, s.state().note.as_str(), s.events().unwrap().len()), (11, "mine", 3));
    drop(s);
    // The same cells still open one by one, as before.
    assert_eq!(Store::<Tally>::open_aio(&db, APP).unwrap().state().n, 11);
}

/// aio's `settings`, at 8: 7 made `dark` a `theme`.
#[cell(persist, id = "settings", version = 8, migrate = settings_up)]
pub struct Settings {
    theme: String,
}

fn settings_up(mut old: Value, from: u32) -> Result<Settings, String> {
    if from < 7
        && let Some(m) = old.as_object_mut()
    {
        let dark = m.remove("dark").and_then(|d| d.as_bool()).unwrap_or(false);
        m.insert("theme".into(), json!(if dark { "dark" } else { "light" }));
    }
    serde_json::from_value(old).map_err(|e| e.to_string())
}

/// aio's `network`: `persist: { exclude: ["epoch"] }`.
#[cell(persist, id = "network", version = 2)]
pub struct Net {
    rpc: String,
    #[transient]
    epoch: u64,
}

/// aio's `accounts` and, bound to its `store`, the `account_store` table.
#[cell(persist, id = "accounts", version = 3)]
pub struct Accts {
    active: Option<i64>,
    #[table("account_store", key = "id")]
    store: BTreeMap<String, Acct>,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Acct {
    id: String,
    label: String,
}

/// A purse port: one cell for aio's three, its own `locked` beside them.
#[cell(persist, id = "purse", version = 2)]
pub struct Purse {
    #[aio_cell]
    settings: Settings,
    #[aio_cell]
    network: Net,
    #[aio_cell]
    accounts: Accts,
    locked: bool,
}

#[methods]
impl Purse {
    pub fn theme(&mut self, t: String) {
        self.settings.theme = t;
    }
    pub fn tick(&mut self) {
        self.network.epoch += 1;
    }
    /// Two slices and a table row in one call.
    pub fn open(&mut self, id: String, label: String) {
        self.accounts.active = Some(1);
        self.accounts.store.insert(id.clone(), Acct { id, label });
        self.settings.theme = "opened".into();
    }
    pub fn lock(&mut self) {
        self.locked = true;
    }
}

/// A purse's aio file in either layout: `settings` saved at 6, the account
/// rows in their table, and 26 cells this port doesn't have.
fn purse_file(multi: bool) -> (tempfile::TempDir, PathBuf) {
    let mut cells: Vec<(String, Value)> = (0..26).map(|i| (format!("c{i:02}"), json!({ "i": i }))).collect();
    cells.push(("settings".into(), json!({ "dark": true, "__aio_status": "active" })));
    cells.push(("network".into(), json!({ "rpc": "https://rpc.example" })));
    cells.push(("accounts".into(), json!({ "active": null })));
    let mut rows: Vec<(String, Value)> = vec![
        ("w:__schema".into(), json!(1)),
        ("w:__versions".into(), json!({ "settings": 6, "network": 2, "accounts": 3, "c00": 4 })),
    ];
    match multi {
        true => rows.extend(cells.into_iter().map(|(c, v)| (format!("state\x1f{c}"), v))),
        false => rows.push(("state".into(), Value::Object(cells.into_iter().collect()))),
    }
    let rows: Vec<(&str, Value)> = rows.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    let (d, p) = aio_file(&rows);
    rusqlite::Connection::open(&p)
        .unwrap()
        .execute_batch(
            "CREATE TABLE account_store (id TEXT PRIMARY KEY, label TEXT NOT NULL);
             INSERT INTO account_store VALUES ('a1', 'Main');
             CREATE TABLE aio_synced_tables (name TEXT PRIMARY KEY NOT NULL);
             INSERT INTO aio_synced_tables VALUES ('account_store');",
        )
        .unwrap();
    (d, p)
}

#[test]
fn a_cell_of_many_aio_slices_reads_and_writes_each_in_either_layout() {
    for multi in [false, true] {
        let (_d, p) = purse_file(multi);
        let before = aio_reads(&p, "w");
        let mut w = Store::<Purse>::open_aio(&p, "w").unwrap();
        let st = w.state();
        assert_eq!(st.settings.theme, "dark", "settings migrated from 6 to 8");
        assert!(Path::new(&format!("{}.v1-settings-v6.bak", p.display())).exists());
        assert_eq!((st.network.rpc.as_str(), st.accounts.store["a1"].label.as_str()), ("https://rpc.example", "Main"));
        w.call(&By::Test, "tick", Value::Null).unwrap();
        w.call(&By::Test, "lock", Value::Null).unwrap();
        w.call(&By::Test, "open", json!(["a2", "Savings"])).unwrap();
        let after = aio_reads(&p, "w");
        assert_eq!(after.len(), 29, "multi = {multi}");
        assert_eq!(after["settings"], json!({ "theme": "opened", "__aio_status": "active" }));
        assert_eq!(after["network"], json!({ "rpc": "https://rpc.example" }), "never saved: the epoch");
        assert_eq!(after["accounts"], json!({ "active": 1 }), "its rows are the table's");
        assert_eq!(table(&p, "account_store"), ["Text(\"a1\")|Text(\"Main\")", "Text(\"a2\")|Text(\"Savings\")"]);
        for (c, v) in before.iter().filter(|(c, _)| c.starts_with('c')) {
            assert_eq!(&after[c], v, "{c}");
        }
        let rows = kv(&p);
        assert_eq!(rows["w:__versions"], json!({ "settings": 8, "network": 2, "accounts": 3, "c00": 4 }));
        assert_eq!(rows.contains_key("state"), !multi, "the file's own layout");
        assert!(!rows.keys().any(|k| k.contains("purse")), "aio-rs's own cell is not in aio_kv");
        drop(w);

        // aio ran: what it changed comes in; aio-rs's own field as it was, the epoch unsaved.
        let c = rusqlite::Connection::open(&p).unwrap();
        c.execute("INSERT INTO account_store VALUES ('a3', 'Cold')", []).unwrap();
        drop(c);
        let w = Store::<Purse>::open_aio(&p, "w").unwrap();
        assert_eq!((w.state().accounts.store.len(), w.state().locked, w.state().network.epoch), (3, true, 0));
        assert_eq!(w.state().settings.theme, "opened");
    }
}

#[test]
fn a_cell_of_many_slices_writes_them_all_or_none() {
    let (_d, p) = purse_file(false);
    let mut w = Store::<Purse>::open_aio(&p, "w").unwrap();
    w.call(&By::Test, "theme", json!(["calm"])).unwrap();
    // A row the table can't hold: the call is refused, no slice written.
    rusqlite::Connection::open(&p).unwrap().execute_batch("ALTER TABLE account_store DROP COLUMN label").unwrap();
    let before = kv(&p);
    let e = w.call(&By::Test, "open", json!(["a2", "Savings"])).unwrap_err().why;
    assert!(e.contains("accounts.store (aio table account_store)"), "{e}");
    assert_eq!(kv(&p), before);
    assert_eq!(w.state().settings.theme, "calm");
}

#[test]
fn a_slice_from_a_newer_aio_boots_and_keeps_its_fields_one_that_doesnt_fit_is_refused() {
    let (_d, p) = purse_file(true);
    let c = rusqlite::Connection::open(&p).unwrap();
    c.execute("UPDATE aio_kv SET v = ?1 WHERE k = 'w:__versions'", [json!({ "network": 3 }).to_string()]).unwrap();
    c.execute("UPDATE aio_kv SET v = ?1 WHERE k = 'state\x1fnetwork'", [json!({ "rpc": "r", "later": 1 }).to_string()])
        .unwrap();
    drop(c);
    let mut w = Store::<Purse>::open_aio(&p, "w").unwrap();
    assert_eq!(w.state().network.rpc, "r");
    w.call(&By::Test, "theme", json!(["calm"])).unwrap();
    drop(w);
    let rows = kv(&p);
    assert_eq!(rows["state\x1fnetwork"], json!({ "rpc": "r", "later": 1 }), "the newer build's field kept");
    assert_eq!(rows["w:__versions"]["network"], json!(3), "its stamp never lowered");
    let (_d, p) = purse_file(true);
    let c = rusqlite::Connection::open(&p).unwrap();
    c.execute("UPDATE aio_kv SET v = ?1 WHERE k = 'w:__versions'", [json!({ "network": 3 }).to_string()]).unwrap();
    c.execute("UPDATE aio_kv SET v = ?1 WHERE k = 'state\x1fnetwork'", [json!({ "rpc": [1] }).to_string()]).unwrap();
    drop(c);
    let before = kv(&p);
    let e = Store::<Purse>::open_aio(&p, "w").err().unwrap().0;
    assert!(e.contains("network: aio data from a newer build (v3 > v2) doesn't fit"), "{e}");
    assert_eq!(kv(&p), before);
    assert!(table(&p, "sqlite_master").iter().all(|t| !t.contains("\"log\"")), "no aio-rs table made");
}

/// aio's `vault`, its rows in the `vault_rows` table it declares.
#[cell(persist, id = "vault", version = 1)]
pub struct VaultRows {
    #[table("vault_rows", key = "id", columns = "id TEXT PRIMARY KEY, label TEXT NOT NULL")]
    rows: BTreeMap<String, Acct>,
}

/// A port owning it, on a file aio never made.
#[cell(persist, id = "keeper")]
pub struct Keeper {
    #[aio_cell]
    vault: VaultRows,
}

#[methods]
impl Keeper {
    pub fn add(&mut self, id: String, label: String) {
        self.vault.rows.insert(id.clone(), Acct { id, label });
    }
}

#[test]
fn a_new_file_is_aios_before_its_declared_tables_are_made() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("vault.db");
    let mut k = Store::<Keeper>::open_aio(&p, "w").unwrap();
    k.call(&By::Test, "add", json!(["a1", "Main"])).unwrap();
    drop(k);
    assert_eq!(table(&p, "vault_rows"), ["Text(\"a1\")|Text(\"Main\")"]);
    assert_eq!(aio_reads(&p, "w")["vault"], json!({}), "its rows are the table's");
    assert_eq!(Store::<Keeper>::open_aio(&p, "w").unwrap().state().vault.rows["a1"].label, "Main");
    // A file that is no store of aio's gets no declared table either.
    let other = d.path().join("other.db");
    rusqlite::Connection::open(&other).unwrap().execute_batch("CREATE TABLE notes (t TEXT)").unwrap();
    let before = table(&other, "sqlite_master");
    let e = Store::<Keeper>::open_aio(&other, "w").err().unwrap().0;
    assert!(e.contains("not an aio-rs store"), "{e}");
    assert_eq!(table(&other, "sqlite_master"), before);
}

/// A port's next build: `purse` (its own, from before it read aio's data) and
/// `purseSeen` retired, gone from the file.
#[cell(persist, id = "vault2", retire = ["purse", "purseSeen"])]
pub struct Vault2 {
    #[aio_cell]
    settings: Settings,
    #[aio_cell]
    network: Net,
}

#[methods]
impl Vault2 {}

/// A cell can't retire a slice it reads.
#[cell(persist, id = "selfish", retire = ["settings"])]
pub struct Selfish {
    #[aio_cell]
    settings: Settings,
}

#[methods]
impl Selfish {}

/// Everything on disk of the store at `p`: the file and its WAL.
fn bytes(p: &Path) -> Vec<u8> {
    let wal = PathBuf::from(format!("{}-wal", p.display()));
    [std::fs::read(p).unwrap(), std::fs::read(wal).unwrap_or_default()].concat()
}

fn holds(hay: &[u8], needle: &str) -> bool {
    hay.windows(needle.len()).any(|w| w == needle.as_bytes())
}

#[test]
fn a_retired_cell_is_gone_from_the_file_in_either_layout() {
    for multi in [false, true] {
        let (_d, p) = purse_file(multi);
        let c = rusqlite::Connection::open(&p).unwrap();
        // Long enough to need pages of its own, which SQLite only unlinks.
        let seal = json!({ "sealed": format!("SEALED-SECRET-1{}", "s".repeat(9000)), "__aio_status": "active" });
        match multi {
            true => {
                for (k, v) in [("state\x1fpurse", seal), ("state\x1fpurseSeen", json!({ "at": 1 }))] {
                    c.execute("INSERT INTO aio_kv (k, v) VALUES (?1, ?2)", [k, &v.to_string()]).unwrap();
                }
            }
            false => {
                let doc: String = c.query_row("SELECT v FROM aio_kv WHERE k = 'state'", [], |r| r.get(0)).unwrap();
                let mut doc: Value = serde_json::from_str(&doc).unwrap();
                doc["purse"] = seal;
                doc["purseSeen"] = json!({ "at": 1 });
                c.execute("UPDATE aio_kv SET v = ?1 WHERE k = 'state'", [doc.to_string()]).unwrap();
            }
        }
        let versions = json!({ "settings": 6, "network": 2, "accounts": 3, "c00": 4, "purse": 2 });
        c.execute("UPDATE aio_kv SET v = ?1 WHERE k = 'w:__versions'", [versions.to_string()]).unwrap();
        drop(c);
        // aio-rs's `purse` ran here: its history is in the file too.
        let mut w = Store::<Purse>::open_aio(&p, "w").unwrap();
        w.call(&By::Test, "lock", Value::Null).unwrap();
        drop(w);
        assert!(holds(&bytes(&p), "SEALED-SECRET-1"));
        let before = aio_reads(&p, "w");

        let v = Store::<Vault2>::open_aio(&p, "w").unwrap();
        assert_eq!(v.state().settings.theme, "dark");
        drop(v);
        let after = aio_reads(&p, "w");
        assert!(!after.contains_key("purse") && !after.contains_key("purseSeen"), "multi = {multi}");
        assert_eq!(after.len(), before.len() - 2);
        for (c, v) in before.iter().filter(|(c, _)| c.starts_with('c') || *c == "accounts") {
            assert_eq!(&after[c], v, "{c}");
        }
        assert_eq!(kv(&p)["w:__versions"], json!({ "settings": 8, "network": 2, "accounts": 3, "c00": 4 }));
        let c = rusqlite::Connection::open(&p).unwrap();
        let left: i64 = c
            .query_row("SELECT (SELECT COUNT(*) FROM log WHERE cell = 'purse') + (SELECT COUNT(*) FROM snap WHERE cell = 'purse')", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0, "its history is gone too");
        drop(c);
        assert!(!holds(&bytes(&p), "SEALED-SECRET-1"), "zeroed, not only unlinked");
        // Again: nothing left to do, the boot as ever.
        assert_eq!(Store::<Vault2>::open_aio(&p, "w").unwrap().state().network.rpc, "https://rpc.example");
    }
    let (_d, p) = purse_file(false);
    let e = Store::<Selfish>::open_aio(&p, "w").err().unwrap().0;
    assert!(e.contains("selfish: it retires cell settings, which it still reads"), "{e}");
}

/// A port whose bridge-era build kept a slice under its own id (`state\x1fpurse3`)
/// before it read aio's cells by field: it retires that slice, its history kept.
#[cell(persist, id = "purse3", retire = ["purse3"])]
pub struct Purse3 {
    #[aio_cell]
    settings: Settings,
    #[aio_cell]
    network: Net,
    locked: bool,
}

#[methods]
impl Purse3 {
    pub fn lock(&mut self) {
        self.locked = true;
    }
}

/// A one-slice cell reads aio's slice of its own id: it can't retire it.
#[cell(persist, id = "settings", retire = ["settings"])]
pub struct SelfRetired {
    dark: bool,
}

#[methods]
impl SelfRetired {}

#[test]
fn a_cell_of_aio_cells_retires_aios_slice_of_its_own_id_and_keeps_its_history() {
    for multi in [false, true] {
        let (_d, p) = purse_file(multi);
        let mut w = Store::<Purse3>::open_aio(&p, "w").unwrap();
        w.call(&By::Test, "lock", Value::Null).unwrap();
        drop(w);
        // An older build's slice of its own id, and its stamp.
        let c = rusqlite::Connection::open(&p).unwrap();
        let bridge = json!({ "seen": "BRIDGE-ERA-SLICE" });
        match multi {
            true => _ = c.execute("INSERT INTO aio_kv (k, v) VALUES ('state\x1fpurse3', ?1)", [bridge.to_string()]).unwrap(),
            false => {
                let doc: String = c.query_row("SELECT v FROM aio_kv WHERE k = 'state'", [], |r| r.get(0)).unwrap();
                let mut doc: Value = serde_json::from_str(&doc).unwrap();
                doc["purse3"] = bridge;
                c.execute("UPDATE aio_kv SET v = ?1 WHERE k = 'state'", [doc.to_string()]).unwrap();
            }
        }
        let mut versions = kv(&p)["w:__versions"].clone();
        versions["purse3"] = json!(1);
        c.execute("UPDATE aio_kv SET v = ?1 WHERE k = 'w:__versions'", [versions.to_string()]).unwrap();
        drop(c);
        assert!(aio_reads(&p, "w").contains_key("purse3"));

        let w = Store::<Purse3>::open_aio(&p, "w").unwrap();
        assert!(w.state().locked, "its own history stays");
        assert!(w.events().unwrap().iter().any(|e| e.method == "lock"));
        drop(w);
        let after = aio_reads(&p, "w");
        assert!(!after.contains_key("purse3"), "multi = {multi}");
        assert!(after.contains_key("settings") && after.contains_key("c00"));
        assert!(kv(&p)["w:__versions"].get("purse3").is_none());
        assert!(!holds(&bytes(&p), "BRIDGE-ERA-SLICE"), "zeroed");
    }
    let (_d, p) = purse_file(false);
    let e = Store::<SelfRetired>::open_aio(&p, "w").err().unwrap().0;
    assert!(e.contains("settings: it retires cell settings, which it still reads"), "{e}");
}

/// aio's `counts`: ids handed out, and the next one.
#[cell(persist, id = "counts")]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    next_id: u32,
    ids: Vec<u32>,
}

/// A port whose invariant aio's own boot settles (`nextId` past every id):
/// its `#[on_aio]` does it, on whatever aio left (feature `compat`).
#[cfg(feature = "compat")]
#[cell(persist, id = "ledger")]
pub struct Ledger {
    #[aio_cell]
    counts: Counts,
    settled: u32,
}

#[cfg(feature = "compat")]
#[methods]
impl Ledger {
    #[on_aio]
    pub fn settle(&mut self) {
        self.counts.next_id = self.counts.next_id.max(self.counts.ids.iter().max().map_or(1, |m| m + 1));
        self.settled += 1;
    }
    #[invariant]
    fn next_past_every_id(&self) -> Result {
        ensure!(self.counts.ids.iter().all(|i| *i < self.counts.next_id), "an id at or past nextId");
        Ok(())
    }
}

/// The same invariant, nothing to settle it.
#[cell(persist, id = "strict_ledger")]
pub struct StrictLedger {
    #[aio_cell]
    counts: Counts,
}

#[methods]
impl StrictLedger {
    #[invariant]
    fn next_past_every_id(&self) -> Result {
        ensure!(self.counts.ids.iter().all(|i| *i < self.counts.next_id), "an id at or past nextId");
        Ok(())
    }
}

#[cfg(feature = "compat")]
fn set_counts(p: &Path, counts: Value) {
    let c = rusqlite::Connection::open(p).unwrap();
    c.execute("UPDATE aio_kv SET v = ?1 WHERE k = 'state'", [json!({ "counts": counts }).to_string()]).unwrap();
}

#[cfg(feature = "compat")]
#[test]
fn what_aio_changed_is_settled_by_on_aio_and_checked_by_the_invariants() {
    let (_d, p) = aio_file(&[
        ("w:__versions", json!({ "counts": 1 })),
        ("state", json!({ "counts": { "nextId": 1, "ids": [1, 5] } })),
    ]);
    // Nothing settles it: the boot is refused, aio's data kept.
    let before = kv(&p);
    let e = Store::<StrictLedger>::open_aio(&p, "w").err().unwrap().0;
    assert!(e.contains("strict_ledger: adopting aio's data: an id at or past nextId"), "{e}");
    assert_eq!(kv(&p), before);

    let l = Store::<Ledger>::open_aio(&p, "w").unwrap();
    assert_eq!((l.state().counts.next_id, l.state().settled), (6, 1));
    assert_eq!(l.events().unwrap().len(), 1, "one adoption, settled");
    drop(l);
    assert_eq!(aio_reads(&p, "w")["counts"], json!({ "nextId": 6, "ids": [1, 5] }), "settled for aio too");
    // Nothing new from aio: not run again.
    assert_eq!(Store::<Ledger>::open_aio(&p, "w").unwrap().state().settled, 1);
    // aio ran and handed out 9: settled again, on the next boot.
    set_counts(&p, json!({ "nextId": 6, "ids": [1, 5, 9] }));
    let l = Store::<Ledger>::open_aio(&p, "w").unwrap();
    assert_eq!((l.state().counts.next_id, l.state().settled), (10, 2));
    drop(l);
    // Replayed from its log, the same state.
    let db = rusqlite::Connection::open(&p).unwrap();
    db.execute("DELETE FROM snap WHERE cell = 'ledger'", []).unwrap();
    drop(db);
    assert_eq!(Store::<Ledger>::open_aio(&p, "w").unwrap().state().settled, 2);
}

#[test]
fn aio_apks_state_comes_in_once_on_the_ports_first_android_start() {
    // aio android-template MainActivity.kt `AioNativeStore.fileFor`: Java's
    // String.hashCode, in hex; aio's standalone runtime keys `aio:<appId>`.
    let d = tempfile::tempdir().unwrap();
    let files = d.path();
    let file = aio::compat::data::android_store_file(&files.join("aio-store"), "aio:notes");
    assert_eq!(file.file_name().unwrap(), "aio_notes.22588ace.json", "as the APK names it (checked with Java)");
    let odd = aio::compat::data::android_store_file(Path::new("s"), &format!("aio:{}/é", "x".repeat(70)));
    let name = odd.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with(&format!("aio_{}", "x".repeat(60))) && name.split('.').next().unwrap().len() == 64);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, r#"{"tally":{"n":4,"__aio_status":"active"},"__versions":{"tally":0}}"#).unwrap();
    let store = files.join("data/state.db");
    std::fs::create_dir_all(store.parent().unwrap()).unwrap();
    assert!(aio::compat::data::import_android(files, "notes", &store).unwrap());
    assert_eq!(aio_reads(&store, "notes")["tally"]["n"], 4);
    let mut s = Store::<Tally>::open_aio(&store, "notes").unwrap();
    assert_eq!(s.state().n, 4, "the cell starts from aio's state");
    s.call(&By::Test, "on_add", Value::Null).unwrap();
    drop(s);
    // Once: the port's own store from then on; aio's file kept as it was.
    assert!(!aio::compat::data::import_android(files, "notes", &store).unwrap());
    assert_eq!(Store::<Tally>::open_aio(&store, "notes").unwrap().state().n, 5);
    assert!(std::fs::read_to_string(&file).unwrap().contains("\"n\":4"));
    // Nothing of aio's: nothing made.
    let none = d.path().join("none");
    assert!(!aio::compat::data::import_android(&none, "notes", &none.join("state.db")).unwrap());
    // Not aio's document: refused, never dropped or half made.
    std::fs::write(&file, "[1]").unwrap();
    let other = files.join("data/other.db");
    let e = aio::compat::data::import_android(files, "notes", &other).unwrap_err();
    assert!(e.to_string().contains("not an object of cells"), "{e}");
    assert!(!other.exists() && !other.with_extension("importing").exists());
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Task {
    id: i64,
    title: String,
    done: bool,
    tags: Vec<String>,
    meta: Option<BTreeMap<String, Value>>,
}

/// Typed columns (aio's `table({ id: pk(), … })`), a bool, a list and a nested value among them.
const TASKS: &[aio::columns::Col] = &[
    aio::columns::pk("id"),
    aio::columns::text("title"),
    aio::columns::boolean("done").default_int(0),
    aio::columns::json("tags"),
    aio::columns::json("meta").nullable(),
];

#[cell(persist, id = "board")]
pub struct Board {
    #[table("tasks", columns = TASKS)]
    tasks: Vec<Task>,
}

#[methods]
impl Board {
    pub fn add(&mut self, id: i64, title: String, tags: Vec<String>, meta: Option<BTreeMap<String, Value>>) {
        self.tasks.push(Task { id, title, done: false, tags, meta });
    }
    pub fn finish(&mut self, id: i64) {
        self.tasks.iter_mut().filter(|t| t.id == id).for_each(|t| t.done = true);
    }
}

#[test]
fn typed_columns_hold_bools_lists_and_nested_values_both_ways() {
    let meta = json!({ "due": { "day": 3 }, "who": ["ana"] });
    for aio_file in [false, true] {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("state.db");
        let open = || if aio_file { Store::<Board>::open_aio(&p, APP) } else { Store::<Board>::open(&p) };
        let mut s = open().unwrap();
        let ddl: String = rusqlite::Connection::open(&p)
            .unwrap()
            .query_row("SELECT sql FROM sqlite_master WHERE name = 'tasks'", [], |r| r.get(0))
            .unwrap();
        assert!(ddl.contains(r#""done" BOOLEAN NOT NULL DEFAULT 0, "tags" JSON NOT NULL, "meta" JSON)"#), "{ddl}");
        s.call(&By::Test, "add", json!([1, "plan", ["a", "b"], meta])).unwrap();
        s.call(&By::Test, "add", json!([2, "[not a list]", [], null])).unwrap();
        s.call(&By::Test, "finish", json!([1])).unwrap();
        assert_eq!(
            table(&p, "tasks"),
            [
                r#"Integer(1)|Text("plan")|Integer(1)|Text("[\"a\",\"b\"]")|Text("{\"due\":{\"day\":3},\"who\":[\"ana\"]}")"#,
                r#"Integer(2)|Text("[not a list]")|Integer(0)|Text("[]")|Null"#,
            ],
            "0/1 and JSON text, as aio (and any SQL tool) can read them"
        );
        let want = s.state().tasks.clone();
        drop(s);
        // Back as they were, from the rows: the app's own SQL changes them too.
        let c = rusqlite::Connection::open(&p).unwrap();
        c.execute(r#"UPDATE tasks SET done = 0, tags = '["x"]' WHERE id = 1"#, []).unwrap();
        drop(c);
        let s = open().unwrap();
        assert_eq!(s.state().tasks[1], want[1]);
        assert_eq!((s.state().tasks[0].done, s.state().tasks[0].tags.clone()), (false, vec!["x".to_string()]));
        assert_eq!(s.state().tasks[0].meta.as_ref().map(|m| json!(m)), Some(meta.clone()));
    }
}

#[test]
fn a_bool_or_a_list_in_a_plain_column_is_refused_naming_the_column_kind() {
    let (_d, p) = bound_file(true);
    let c = rusqlite::Connection::open(&p).unwrap();
    c.execute_batch(
        "CREATE TABLE tasks (id INTEGER PRIMARY KEY, title TEXT NOT NULL, done INTEGER, tags TEXT, meta TEXT)",
    )
    .unwrap();
    drop(c);
    let mut s = Store::<Board>::open_aio(&p, APP).unwrap();
    let e = s.call(&By::Test, "add", json!([1, "plan", [], null])).unwrap_err().why;
    assert!(e.contains("done = false") && e.contains("a bool needs a BOOLEAN column"), "{e}");
}

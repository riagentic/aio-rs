//! What is saved and when (aio's `persist` filter, its downgrade boot, `persist: false`).
use aio::prelude::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Feed {
    url: String,
    busy: bool,
}

/// aio's `persist: { exclude: ["feeds.*.busy"] }`.
#[cell(persist(exclude = ["feeds.*.busy"]))]
pub struct Reader {
    feeds: BTreeMap<String, Feed>,
}

#[methods]
impl Reader {
    pub fn fetch(&mut self, id: String, url: String) {
        self.feeds.insert(id, Feed { url, busy: true });
    }
}

/// aio's `persist: { include: ["name", "profile.email"] }`.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Profile {
    email: String,
    token: String,
}

#[cell(persist(include = ["name", "profile.email"]))]
pub struct Account {
    name: String,
    profile: Profile,
    seen: u32,
}

#[methods]
impl Account {
    pub fn set(&mut self, name: String, email: String, token: String) {
        (self.name, self.profile, self.seen) = (name, Profile { email, token }, 7);
    }
}

fn saved_state(p: &std::path::Path, cell: &str) -> Value {
    let c = rusqlite::Connection::open(p).unwrap();
    let t: String =
        c.query_row("SELECT state FROM snap WHERE cell = ?1 ORDER BY seq DESC LIMIT 1", [cell], |r| r.get(0)).unwrap();
    serde_json::from_str(&t).unwrap()
}

#[test]
fn nested_exclude_and_include_paths_are_what_a_restart_brings_back() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.db");
    let mut s = Store::<Reader>::open(&p).unwrap();
    s.call(&By::Test, "fetch", json!(["a", "https://a"])).unwrap();
    assert!(s.state().feeds["a"].busy, "kept in memory");
    drop(s);
    assert_eq!(saved_state(&p, "reader"), json!({ "feeds": { "a": { "url": "https://a" } } }), "never on disk");
    let s = Store::<Reader>::open(&p).unwrap();
    assert_eq!(s.state().feeds["a"], Feed { url: "https://a".into(), busy: false });
    drop(s);

    let p = d.path().join("account.db");
    let mut s = Store::<Account>::open(&p).unwrap();
    s.call(&By::Test, "set", json!(["ana", "a@x", "secret"])).unwrap();
    drop(s);
    assert_eq!(saved_state(&p, "account"), json!({ "name": "ana", "profile": { "email": "a@x" } }));
    let s = Store::<Account>::open(&p).unwrap();
    assert_eq!((s.state().name.as_str(), s.state().profile.email.as_str()), ("ana", "a@x"));
    assert_eq!((s.state().profile.token.as_str(), s.state().seen), ("", 0));
}

#[test]
fn an_aio_slice_leaves_out_what_its_persist_filter_excludes() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.db");
    let mut s = Store::<Reader>::open_aio(&p, "reader-app").unwrap();
    s.call(&By::Test, "fetch", json!(["a", "u"])).unwrap();
    drop(s);
    let c = rusqlite::Connection::open(&p).unwrap();
    let doc: String = c.query_row("SELECT v FROM aio_kv WHERE k = 'state'", [], |r| r.get(0)).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&doc).unwrap()["reader"], json!({ "feeds": { "a": { "url": "u" } } }));
}

#[cell(persist, id = "note", version = 1)]
pub struct NoteV1 {
    text: String,
}

#[methods]
impl NoteV1 {
    pub fn edit(&mut self, text: String) {
        self.text = text;
    }
}

fn never(_: Value, from: u32) -> Result<NoteV2, String> {
    Err(format!("migrated again from v{from}"))
}

#[cell(persist, id = "note", version = 2, migrate = never)]
pub struct NoteV2 {
    text: String,
    color: String,
}

#[methods]
impl NoteV2 {
    pub fn paint(&mut self, text: String, color: String) {
        (self.text, self.color) = (text, color);
    }
}

#[cell(persist, id = "note", version = 9)]
pub struct NoteV9 {
    text: Vec<u32>,
}

#[methods]
impl NoteV9 {
    pub fn put(&mut self, n: u32) {
        self.text.push(n);
    }
}

#[test]
fn an_older_build_boots_on_newer_data_and_keeps_what_it_doesnt_know() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.db");
    let mut s = Store::<NoteV2>::open(&p).unwrap();
    s.call(&By::Test, "paint", json!(["a", "red"])).unwrap();
    drop(s);
    // The rollback: v1 boots (aio's downgrade), and its writes keep `color`.
    let mut s = Store::<NoteV1>::open(&p).unwrap();
    assert_eq!(s.state().text, "a");
    s.call(&By::Test, "edit", json!(["b"])).unwrap();
    drop(s);
    let c = rusqlite::Connection::open(&p).unwrap();
    let v: u32 =
        c.query_row("SELECT v FROM snap WHERE cell = 'note' ORDER BY seq DESC LIMIT 1", [], |r| r.get(0)).unwrap();
    assert_eq!(v, 2, "the stamp is never lowered");
    drop(c);
    // Rolled forward: nothing is migrated again, nothing was lost.
    let s = Store::<NoteV2>::open(&p).unwrap();
    assert_eq!((s.state().text.as_str(), s.state().color.as_str()), ("b", "red"));
    drop(s);
    // Data this build can't read as its own: refused, the file kept.
    let p = d.path().join("nine.db");
    let mut s = Store::<NoteV9>::open(&p).unwrap();
    s.call(&By::Test, "put", json!([1])).unwrap();
    drop(s);
    let e = Store::<NoteV1>::open(&p).err().unwrap().0;
    assert!(e.contains("data from a newer build (v9 > v1) doesn't fit this build"), "{e}");
}

#[test]
fn an_older_build_on_an_aio_slice_from_a_newer_one_parks_it_as_aio_does() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.db");
    let c = rusqlite::Connection::open(&p).unwrap();
    c.execute_batch(
        r#"CREATE TABLE aio_kv (k TEXT PRIMARY KEY, v TEXT NOT NULL);
           INSERT INTO aio_kv VALUES ('state', '{"note":{"text":"x","later":{"a":1}}}');
           INSERT INTO aio_kv VALUES ('notes:__versions', '{"note":5}');"#,
    )
    .unwrap();
    drop(c);
    let mut s = Store::<NoteV1>::open_aio(&p, "notes").unwrap();
    assert_eq!(s.state().text, "x");
    s.call(&By::Test, "edit", json!(["y"])).unwrap();
    s.call(&By::Test, "edit", json!(["z"])).unwrap();
    drop(s);
    let c = rusqlite::Connection::open(&p).unwrap();
    let get = |k: &str| -> Value {
        serde_json::from_str(&c.query_row("SELECT v FROM aio_kv WHERE k = ?1", [k], |r| r.get::<_, String>(0)).unwrap())
            .unwrap()
    };
    let doc = get("state");
    assert_eq!(doc["note"], json!({ "text": "z", "later": { "a": 1 } }), "the newer build's field kept");
    assert_eq!(doc["__downgraded:note"], json!({ "text": "x", "later": { "a": 1 } }), "parked once, verbatim");
    assert_eq!(get("notes:__versions"), json!({ "note": 5 }));
}

#[cell(persist, id = "memo", version = 1)]
pub struct MemoV1 {
    text: String,
    pin: bool,
}

#[methods]
impl MemoV1 {
    pub fn edit(&mut self, text: String, pin: bool) {
        (self.text, self.pin) = (text, pin);
    }
}

fn memo_up(old: Value, from: u32) -> Result<MemoV2, String> {
    assert_eq!(from, 1);
    Ok(MemoV2 { text: old["text"].as_str().ok_or("no text")?.into(), color: "none".into() })
}

/// v2 drops v1's `pin` and adds `color`.
#[cell(persist, id = "memo", version = 2, migrate = memo_up)]
pub struct MemoV2 {
    text: String,
    color: String,
}

#[methods]
impl MemoV2 {
    pub fn paint(&mut self, text: String, color: String) {
        (self.text, self.color) = (text, color);
    }
}

/// old -> new -> old -> new, with fields only the old build declares: every
/// boot succeeds as in aio (upgrade migrates, downgrade keeps, roll-forward
/// drops the old build's fields), and nothing the new build wrote is lost.
fn round_trip(open1: impl Fn() -> Store<MemoV1>, open2: impl Fn() -> Store<MemoV2>) {
    let mut s = open1();
    s.call(&By::Test, "edit", json!(["a", true])).unwrap();
    drop(s);
    let mut s = open2();
    assert_eq!((s.state().text.as_str(), s.state().color.as_str()), ("a", "none"), "the upgrade migrates");
    s.call(&By::Test, "paint", json!(["b", "red"])).unwrap();
    drop(s);
    let mut s = open1();
    assert_eq!(s.state().text, "b", "the downgrade boots");
    s.call(&By::Test, "edit", json!(["c", true])).unwrap();
    drop(s);
    let mut s = open2();
    assert_eq!((s.state().text.as_str(), s.state().color.as_str()), ("c", "red"), "the roll-forward boots");
    s.call(&By::Test, "paint", json!(["d", "blue"])).unwrap();
    drop(s);
    let s = open2();
    assert_eq!((s.state().text.as_str(), s.state().color.as_str()), ("d", "blue"));
    drop(s);
    // And down again: still boots.
    assert_eq!(open1().state().text, "d");
}

#[test]
fn a_down_and_up_round_trip_boots_every_time_as_in_aio() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.db");
    round_trip(|| Store::<MemoV1>::open(&p).unwrap(), || Store::<MemoV2>::open(&p).unwrap());
    let p = d.path().join("aio.db");
    round_trip(|| Store::<MemoV1>::open_aio(&p, "memos").unwrap(), || Store::<MemoV2>::open_aio(&p, "memos").unwrap());
}

#[test]
fn a_file_aio_rolled_back_and_forward_boots() {
    // What aio leaves after new -> old: the slice widened with the old build's
    // `pin`, the newer stamp, and the parked copy.
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.db");
    let c = rusqlite::Connection::open(&p).unwrap();
    c.execute_batch(
        r#"CREATE TABLE aio_kv (k TEXT PRIMARY KEY, v TEXT NOT NULL);
           INSERT INTO aio_kv VALUES ('state', '{"memo":{"text":"c","color":"red","pin":true},"__downgraded:memo":{"text":"b","color":"red"}}');
           INSERT INTO aio_kv VALUES ('memos:__versions', '{"memo":2}');"#,
    )
    .unwrap();
    drop(c);
    let mut s = Store::<MemoV2>::open_aio(&p, "memos").unwrap();
    assert_eq!((s.state().text.as_str(), s.state().color.as_str()), ("c", "red"));
    s.call(&By::Test, "paint", json!(["d", "blue"])).unwrap();
    drop(s);
    assert_eq!(Store::<MemoV2>::open_aio(&p, "memos").unwrap().state().text, "d");
}

#[cell(persist, id = "kept")]
pub struct Kept {
    n: u32,
}

#[methods]
impl Kept {
    pub fn add(&mut self) {
        self.n += 1;
    }
}

#[test]
fn persist_false_keeps_the_state_in_memory_only() {
    let d = tempfile::tempdir().unwrap();
    let id = format!("kept-{}", std::process::id());
    let cfg = aio::config::Config::parse(&format!("[app]\nid = \"{id}\"\n[data]\npersist = false")).unwrap();
    let ask = aio::paths::Ask { home: Some(d.path().join("home").display().to_string()), ..Default::default() };
    let dirs = aio::paths::AppDirs::planned(&cfg.app.id, aio::paths::plan_app(&cfg, &ask).unwrap()).unwrap();
    let s = aio::Source::<Kept>::for_app(&cfg, &dirs).unwrap();
    s.call("add", json!([])).unwrap();
    assert_eq!(s.state().n, 1);
    assert!(!dirs.store.exists(), "no store: {}", dirs.store.display());
    _ = std::fs::remove_dir_all(aio::paths::sockets(&cfg.app.id, &dirs.data).unwrap());
    // Checked as aio.toml is read.
    let bad = |t: &str| aio::config::Config::parse(&format!("[app]\nid = \"x\"\n[data]\n{t}")).unwrap_err();
    assert!(bad("backup_keep = 0").contains("at least 1"));
    assert!(bad("coalesce_ms = 120000").contains("over a minute"));
    assert!(!bad("backup_every = \"often\"").is_empty());
}

/// aio's `persist: "none"`: a cell that starts from its defaults each launch.
#[cell(persist = false)]
pub struct Draft {
    pub text: String,
}

#[methods]
impl Draft {
    pub fn type_in(&mut self, t: String) {
        self.text = t;
    }
}

#[test]
fn a_cell_with_persist_false_is_never_saved_even_on_a_file() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.db");
    let h = Handle::<Draft>::open(&p).unwrap();
    h.type_in("half a thought".into()).unwrap();
    drop(h);
    assert_eq!(Handle::<Draft>::open(&p).unwrap().state().text, "", "the next launch starts empty");
    assert!(!p.exists(), "nothing written for it");
}

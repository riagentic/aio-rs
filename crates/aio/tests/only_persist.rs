//! `[data] only_persist` (aio's `cellDefaults: { persist: "none" }`). Its own
//! test binary: the setting is the process's.
use aio::prelude::*;

#[cell(persist)]
pub struct Kept {
    pub n: u32,
}

#[methods]
impl Kept {
    pub fn bump(&mut self) {
        self.n += 1;
    }
}

#[cell]
pub struct Scratch {
    pub n: u32,
}

#[methods]
impl Scratch {
    pub fn bump(&mut self) {
        self.n += 1;
    }
}

#[test]
fn only_persist_cells_are_saved_when_the_app_asks() {
    let d = tempfile::tempdir().unwrap();
    let id = format!("only-persist-{}", std::process::id());
    let cfg = aio::config::Config::parse(&format!("[app]\nid = \"{id}\"\n[data]\nonly_persist = true")).unwrap();
    let dirs = aio::paths::AppDirs::at(d.path(), &id).unwrap();
    let s = aio::Source::<Kept>::for_app(&cfg, &dirs).unwrap();
    s.call("bump", serde_json::Value::Null).unwrap();
    let scratch = Handle::<Scratch>::open(&dirs.store).unwrap();
    scratch.bump().unwrap();
    drop(scratch);
    assert_eq!(Handle::<Scratch>::open(&dirs.store).unwrap().state().n, 0, "a cell without persist starts empty");
    let events = rusqlite::Connection::open(&dirs.store)
        .unwrap()
        .query_row("SELECT count(*) FROM log WHERE cell = 'scratch'", [], |r| r.get::<_, i64>(0))
        .unwrap();
    assert_eq!(events, 0, "nothing of it written");
    assert_eq!(s.state().n, 1);
    _ = std::fs::remove_dir_all(aio::paths::sockets(&id, &dirs.data).unwrap());
}

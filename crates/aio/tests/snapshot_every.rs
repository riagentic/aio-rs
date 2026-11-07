//! `[data] snapshot_every` (aio's rolling snapshot that
//! `checkIntegrityOnBoot` restores from): a damaged store comes back with at
//! most that much lost, not a day's.
use aio::prelude::*;

#[cell(persist)]
pub struct Notes {
    pub items: Vec<String>,
}

#[methods]
impl Notes {
    pub fn add(&mut self, text: String) {
        self.items.push(text);
    }
}

#[test]
fn a_damaged_store_comes_back_from_the_frequent_snapshot() {
    let d = tempfile::tempdir().unwrap();
    let id = format!("snapshot-every-{}", std::process::id());
    let cfg = aio::config::Config::parse(&format!("[app]\nid = \"{id}\"\n[data]\nsnapshot_every = \"1s\"")).unwrap();
    assert_eq!(cfg.data.snapshot_every, Some(std::time::Duration::from_secs(1)));
    let dirs = aio::paths::AppDirs::at(d.path(), &id).unwrap();
    let s = aio::Source::<Notes>::for_app(&cfg, &dirs).unwrap();
    s.call("add", serde_json::json!(["before"])).unwrap();
    let good = dirs.store.with_extension("db.good");
    let booted = std::fs::metadata(&good).unwrap().modified().unwrap();
    s.call("add", serde_json::json!(["since boot"])).unwrap();
    let made = || std::fs::metadata(&good).unwrap().modified().unwrap() > booted;
    for _ in 0..300 {
        if made() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(made(), "made again while the app runs");
    // What a damaged store boots from (the good copy, as `recover` puts it in place) has it.
    let copy = d.path().join("copy.db");
    std::fs::copy(&good, &copy).unwrap();
    let h = Handle::<Notes>::open(&copy).unwrap();
    assert_eq!(h.state().items, ["before", "since boot"], "what came after boot is in it");
    _ = std::fs::remove_dir_all(aio::paths::sockets(&id, &dirs.data).unwrap());
}

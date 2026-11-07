//! `[history] travel = false` (aio's `diagnostics.dev.timeTravel: false`).
//! Its own test binary: the setting is the process's.
use aio::prelude::*;

#[cell(persist)]
pub struct Score {
    pub n: u32,
}

#[methods]
impl Score {
    pub fn bump(&mut self) {
        self.n += 1;
    }
}

#[test]
fn time_travel_is_refused_when_the_app_turns_it_off() {
    let d = tempfile::tempdir().unwrap();
    let id = format!("travel-off-{}", std::process::id());
    let cfg = aio::config::Config::parse(&format!("[app]\nid = \"{id}\"\n[history]\ntravel = false")).unwrap();
    let dirs = aio::paths::AppDirs::at(d.path(), &id).unwrap();
    let s = aio::Source::<Score>::for_app(&cfg, &dirs).unwrap();
    s.call("bump", serde_json::Value::Null).unwrap();
    s.call("bump", serde_json::Value::Null).unwrap();
    let e = s.travel(Some(aio::TimeAsk::Undo)).unwrap_err();
    assert_eq!(e.why, "time travel is off ([history] travel = false)");
    assert_eq!(s.state().n, 2, "nothing moved");
    assert_eq!(s.travel(None).unwrap().head, 2, "where it stands is still said");
    _ = std::fs::remove_dir_all(aio::paths::sockets(&id, &dirs.data).unwrap());
}

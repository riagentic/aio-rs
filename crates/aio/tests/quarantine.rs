//! aio's quarantine: a cell whose log no longer replays runs at its last
//! snapshot in a release run, refusing calls and keeping its log for a
//! fixed build; a dev build refuses the boot. Its own test binary: the
//! setting is the process's.
use aio::prelude::*;

#[cell(persist, restore = calm)]
pub struct Tally {
    pub n: u32,
    /// Cleared by every boot's repair (`restore`).
    pub busy: bool,
}

fn calm(t: Tally) -> Tally {
    Tally { busy: false, ..t }
}

#[methods]
impl Tally {
    pub fn add(&mut self, by: u32) {
        self.n += by;
        self.busy = true;
    }
}

#[test]
fn a_cell_whose_log_no_longer_replays_is_quarantined_not_reset() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.db");
    Handle::<Tally>::open(&p).unwrap().add(1).unwrap();
    // Booted again: replayed, snapshotted at n = 1; then one more event.
    Handle::<Tally>::open(&p).unwrap().add(2).unwrap();
    let db = rusqlite::Connection::open(&p).unwrap();
    // The later event no longer applies (as after a method changed its arguments), and only the
    // first boot's snapshot (n = 1) is before it.
    db.execute("UPDATE log SET args = '[\"two\"]' WHERE cell = 'tally' AND method = 'add' AND args = '[2]'", [])
        .unwrap();
    db.execute("DELETE FROM snap WHERE cell = 'tally' AND seq > 1", []).unwrap();
    assert_eq!(db.query_row("SELECT max(seq) FROM snap WHERE cell = 'tally'", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
    let rows = || db.query_row("SELECT count(*) FROM log WHERE cell = 'tally'", [], |r| r.get::<_, i64>(0)).unwrap();
    let before = rows();
    aio::testing::quarantine(false);
    let e = Handle::<Tally>::open(&p).err().unwrap();
    assert!(e.0.contains("replay of event"), "a dev build refuses the boot: {e}");
    aio::testing::quarantine(true);
    let h = Handle::<Tally>::open(&p).unwrap();
    assert_eq!(h.state().n, 1, "its last snapshot, never the initial state");
    assert!(!h.state().busy, "repaired as every boot is");
    assert_eq!(h.store(|s| s.seq()), 1, "labelled with its snapshot's event, not the log's last");
    assert!(h.store(|s| s.quarantined().map(String::from)).unwrap().contains("replay of event"));
    let refused = h.add(5).unwrap_err();
    assert_eq!(refused.code, Some(aio::Code::PersistError));
    assert!(refused.why.contains("tally is quarantined"), "{refused}");
    drop(h);
    assert_eq!(rows(), before, "its log kept as it is, for a fixed build");
    assert_eq!(Handle::<Tally>::open(&p).unwrap().state().n, 1, "and still quarantined at the next start");
}

//! `#[on_start]` / `#[on_stop]` (aio's `onInit` / `onDestroy`): run by the
//! runtime as a cell goes live and stops, in dependency order and back.
use aio::Cells;
use aio::prelude::*;
use std::sync::Mutex;
use std::time::Duration;

/// What the hooks did, in order (this file's tests take turns on it).
static SAID: Mutex<Vec<String>> = Mutex::new(vec![]);
static TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn said() -> Vec<String> {
    std::mem::take(&mut *SAID.lock().unwrap())
}

fn say(s: &str) {
    SAID.lock().unwrap().push(s.into());
}

#[cell]
pub struct Db {
    pub opened: u32,
}

#[methods]
impl Db {
    /// Open what the cell keeps: an owned resource, dropped when it stops.
    #[on_start]
    pub fn open(&mut self, cx: &Cx) {
        self.opened += 1;
        cx.run(Effect::new("open", |h: Handle<Db>| async move { h.own("conn", Some(Box::new(Conn))) }));
        say("db start");
    }

    #[on_stop]
    pub fn close(&mut self) {
        say("db stop");
    }
}

struct Conn;

impl Drop for Conn {
    fn drop(&mut self) {
        say("conn dropped");
    }
}

#[cell]
pub struct Log {
    pub n: u32,
}

#[methods]
impl Log {
    #[on_start]
    pub fn begin(&mut self) {
        self.n += 1;
        say("log start");
    }

    #[on_stop]
    pub fn end(&mut self) {
        say("log stop");
    }
}

#[cell]
pub struct Mail {
    pub n: u32,
}

#[methods]
impl Mail {
    #[on_start]
    pub fn begin(&mut self) {
        say("mail start");
    }

    #[on_stop]
    pub fn end(&mut self) -> Result {
        say("mail stop");
        reject!("a refusing hook stops nothing")
    }
}

#[tokio::test]
async fn hooks_run_in_dependency_order_at_start_and_back_at_shutdown() {
    let _t = TURN.lock().await;
    said();
    let (log, mail) = (Handle::<Log>::memory(), Handle::<Mail>::memory());
    let cells = Cells::default().with(mail.clone(), &[Log::ID]).with(log.clone(), &[]).start().unwrap();
    assert_eq!(said(), ["log start", "mail start"], "a cell starts after the ones it depends on");
    assert!(cells.shutdown(Duration::from_secs(1)).await.is_empty());
    assert_eq!(said(), ["mail stop", "log stop"], "and stops before them; mail's refusal stopped nothing");
    assert_eq!(log.state().n, 1);
}

#[tokio::test]
async fn disable_stops_it_and_enable_starts_it_again_once_each_way() {
    let _t = TURN.lock().await;
    said();
    let db = Handle::<Db>::memory();
    let cells = Cells::default().with(db.clone(), &[]).start().unwrap();
    db.done("open").await;
    cells.disable(Db::ID).unwrap();
    cells.disable(Db::ID).unwrap();
    assert_eq!(said(), ["db start", "db stop", "conn dropped"], "a second disable runs nothing");
    cells.enable(Db::ID).unwrap();
    cells.enable(Db::ID).unwrap();
    assert_eq!(said(), ["db start"]);
    assert_eq!(db.state().opened, 2);
    db.done("open").await;
    assert!(db.owns("conn"), "reopened");
    cells.disable(Db::ID).unwrap();
    said();
}

#[cfg(feature = "testing")]
#[test]
fn a_test_cell_starts_live() {
    let _t = TURN.blocking_lock();
    said();
    let mut t = aio::testing::TestCells::new();
    let db = t.cell::<Db>();
    assert_eq!(db.state().opened, 1);
    assert_eq!(said(), ["db start"]);
}

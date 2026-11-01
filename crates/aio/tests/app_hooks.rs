//! The app's own hooks on every cell: `aio::before_call` (aio's `beforeReduce`)
//! and `aio::on_effect` (aio's `onEffect`). A process has one of each, so this
//! file holds one test each.
use aio::prelude::*;
use std::sync::Mutex;
use std::time::Duration;

#[cell(persist)]
pub struct Tally {
    pub n: i64,
}

#[methods]
impl Tally {
    pub fn add(&mut self, n: i64) {
        self.n += n;
    }

    pub fn reset(&mut self) {
        self.n = 0;
    }
}

#[cell]
pub struct Fetcher {
    pub done: u32,
}

#[methods]
impl Fetcher {
    pub fn fetch(&mut self, cx: &Cx) {
        let slow = |h: Handle<Fetcher>| async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            _ = h.fetched();
        };
        cx.run(Effect::new("fetch", slow).mode(Mode::First));
    }

    pub fn boom(&mut self, cx: &Cx) {
        cx.run(Effect::new("boom", |h: Handle<Fetcher>| async move { _ = h.fetched() }));
    }

    pub fn fetched(&mut self) {
        self.done += 1;
    }
}

static SEEN: Mutex<Vec<String>> = Mutex::new(vec![]);
static STARTED: Mutex<Vec<String>> = Mutex::new(vec![]);

#[tokio::test(flavor = "multi_thread")]
async fn the_app_hears_every_effect_that_starts() {
    aio::on_effect(|cell, key| {
        assert_ne!(key, "boom", "a hook that fails takes nothing down");
        STARTED.lock().unwrap().push(format!("{cell} {key}"));
    });
    let h = Handle::<Fetcher>::memory();
    h.fetch().unwrap();
    // A second while the first runs: `First` drops it, so it never starts.
    h.fetch().unwrap();
    for _ in 0..200 {
        if h.state().done == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!((h.state().done, STARTED.lock().unwrap().clone()), (1, vec!["fetcher fetch".to_string()]));
    // A hook that panics: logged, and the effect runs all the same.
    h.boom().unwrap();
    for _ in 0..200 {
        if h.state().done == 2 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the effect did not run past a failing hook");
}

#[test]
fn the_app_gate_sees_every_call_and_refuses_what_it_will() {
    aio::before_call(|c| {
        if c.cell != "tally" {
            return Ok(()); // the other test's
        }
        SEEN.lock().unwrap().push(format!("{}.{} {} {:?} {:?}", c.cell, c.method, c.args, c.by, c.role));
        ensure!(c.method != "reset", "resets are off in this app");
        ensure!(c.args[0].as_i64().is_none_or(|n| n < 100), "too much at once");
        assert!(c.args[0] != 13, "unlucky");
        Ok(())
    });
    let d = tempfile::tempdir().unwrap();
    let db = d.path().join("state.db");
    let h = Handle::<Tally>::open(&db).unwrap();
    h.add(2).unwrap();
    assert_eq!(*SEEN.lock().unwrap(), [r#"tally.add [2] System None"#]);
    // Refused: said to the caller, nothing ran, nothing saved.
    assert_eq!(h.reset().unwrap_err().why, "resets are off in this app");
    assert_eq!(h.add(500).unwrap_err().why, "too much at once");
    // A gate that panics refuses (fail closed).
    assert!(h.add(13).unwrap_err().why.contains("the before_call hook panicked"), "a panic is a no");
    assert_eq!((h.state().n, h.store(|s| s.events().unwrap()).len()), (2, 1));
    drop(h);
    // Replay never asks it: what it let in is in the log.
    SEEN.lock().unwrap().clear();
    let h = Handle::<Tally>::open(&db).unwrap();
    assert_eq!((h.state().n, SEEN.lock().unwrap().len()), (2, 0));
}

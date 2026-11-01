//! `Handle::pending` (aio's `cell.$pending("scan")`): how many calls of an
//! async method are in flight, as a count the framework owns, told to
//! `on_pending` watchers so a spinner follows it.
use aio::prelude::*;
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

static GATE: Semaphore = Semaphore::const_new(0);

async fn gate() -> u32 {
    GATE.acquire().await.expect("open").forget();
    1
}

#[cell]
pub struct Jobs {
    pub done: u32,
}

#[methods]
impl Jobs {
    pub async fn scan(&mut self, path: String) -> Result {
        let n: u32 = gate().await;
        ensure!(!path.is_empty(), "no path");
        self.done += n;
        Ok(())
    }

    pub async fn other(&mut self) -> Result {
        let n: u32 = gate().await;
        self.done += n;
        Ok(())
    }

    /// Named like `scan` up to a point: not one of its calls.
    pub async fn scanner(&mut self) -> Result {
        let n: u32 = gate().await;
        self.done += n;
        Ok(())
    }
}

#[tokio::test]
async fn a_count_per_method_and_for_the_cell_told_to_watchers_and_released_on_refusal() {
    let h = Handle::<Jobs>::memory();
    let seen = Arc::new(Mutex::new(vec![]));
    let (s, h2) = (seen.clone(), h.clone());
    h.on_pending(move || s.lock().unwrap().push(h2.pending(Some("scan"))));
    assert_eq!(h.pending(Some("scan")), 0);

    h.scan("/x".into()).unwrap();
    assert_eq!(h.pending(Some("scan")), 1);
    h.scan(String::new()).unwrap();
    assert_eq!(h.pending(Some("scan")), 2, "two overlapping calls are two, never a flag");
    h.other().unwrap();
    h.scanner().unwrap();
    assert_eq!(h.pending(Some("other")), 1);
    assert_eq!(h.pending(Some("scan")), 2, "`scanner` is not `scan`");
    assert_eq!(h.pending(None), 4, "the cell's count is the sum");
    assert_eq!(seen.lock().unwrap()[..2], [1, 2], "a watcher sees each call start");

    GATE.add_permits(4);
    for k in ["scan", "other", "scanner"] {
        h.done(k).await;
    }
    assert_eq!(h.pending(None), 0, "the refused call released its count too");
    assert_eq!(h.state().done, 3);
    assert_eq!(seen.lock().unwrap().last(), Some(&0), "and the watcher saw it fall");
}

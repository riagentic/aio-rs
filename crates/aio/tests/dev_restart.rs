//! A stop (`ax stop`, a closed window, SIGTERM, `ax dev`'s restart) keeps
//! every call as it ran: the next start reads the state the calls left,
//! even when the code changed meanwhile (an `ax dev` hot-patch, then a
//! rebuild). The log alone would replay them with the new code.
use aio::prelude::*;
use before::CounterCalls;
use std::time::Duration;

/// The counter as it was built first.
mod before {
    #[aio::cell(persist, coalesce, id = "counter")]
    pub struct Counter {
        pub count: i64,
    }

    #[aio::methods]
    impl Counter {
        pub fn add(&mut self, by: i64) {
            self.count += by;
        }
    }
}

/// The same cell after an edit of its method (as a hot-patch made it).
mod after {
    #[aio::cell(persist, coalesce, id = "counter")]
    pub struct Counter {
        pub count: i64,
    }

    #[aio::methods]
    impl Counter {
        pub fn add(&mut self, by: i64) {
            self.count += by * 2;
        }
    }
}

const CALLS: i64 = 37;

#[tokio::test]
async fn a_stop_keeps_every_call_as_it_ran_though_the_code_changed() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("app.db");
    let h = Handle::<before::Counter>::open(&db).unwrap();
    for _ in 0..CALLS {
        h.add(1).unwrap();
    }
    assert_eq!(h.state().count, CALLS);
    assert!(h.shutdown(Duration::from_secs(1)).await.aborted.is_empty());
    // A process ends without dropping its cells (they live in a static).
    std::mem::forget(h);
    assert_eq!(Handle::<after::Counter>::open(&db).unwrap().state().count, CALLS, "the next start");
}

#[tokio::test]
async fn a_checkpoint_keeps_the_calls_before_it_as_they_ran_through_a_crash() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("app.db");
    let h = Handle::<before::Counter>::open(&db).unwrap();
    for _ in 0..CALLS {
        h.add(1).unwrap();
    }
    // `ax dev` hot-patches the method: what ran before is checkpointed first.
    h.checkpoint().unwrap();
    // Killed: no stop, no close.
    std::mem::forget(h);
    assert_eq!(Handle::<after::Counter>::open(&db).unwrap().state().count, CALLS, "after the crash");
}

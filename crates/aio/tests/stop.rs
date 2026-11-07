//! The app's stop hooks around the drain (aio's `onStopping` / `onStop`),
//! and its doors unlinked. One test: the hooks are the process's.
use aio::Source;
use aio::prelude::*;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cell(persist)]
pub struct Tally {
    n: u64,
}

#[methods]
impl Tally {
    pub fn bump(&mut self) {
        self.n += 1;
    }
}

/// The app's own store at `db`, opened as an app opens it.
fn local(db: &std::path::Path) -> Handle<Tally> {
    match Source::<Tally>::open(db).unwrap() {
        Source::Local(h) => h,
        #[cfg(feature = "server")]
        Source::Remote(_) => unreachable!("a store is local"),
    }
}

#[test]
fn stopping_runs_while_methods_commit_stop_runs_after_and_a_hung_hook_is_left_behind() {
    let t = tempfile::tempdir().unwrap();
    let db = t.path().join("state.db");
    let h = local(&db);
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let (log, hs) = (seen.clone(), h.clone());
    aio::on_stopping(move || {
        // Still open: the last write of a producer the app stops lands.
        hs.bump().unwrap();
        log.lock().unwrap().push(format!("stopping n={}", hs.state().n));
    });
    aio::on_stopping(|| panic!("a broken hook"));
    let (log, hs) = (seen.clone(), h.clone());
    aio::on_stop(move || {
        let refused = hs.call("bump", serde_json::json!([])).is_err();
        log.lock().unwrap().push(format!("stop refused={refused}"));
    });
    // A door served on the runtime, as a cell's is: never dropped.
    let door = aio::ipc::addr(t.path(), "cell-tally");
    let l = {
        let _in = aio::rt().enter();
        aio::ipc::Listener::bind(&door).unwrap()
    };
    drop(aio::rt().spawn(async move {
        std::future::pending::<()>().await;
        drop(l)
    }));
    assert_eq!(aio::shutdown(Duration::from_secs(5)), Vec::<String>::new());
    assert_eq!(*seen.lock().unwrap(), ["stopping n=1", "stop refused=true"]);
    // (A Windows pipe goes with its process.)
    assert!(cfg!(windows) || !std::path::Path::new(&door).exists(), "a clean stop leaves no socket behind");
    drop(h);
    let again = local(&db);
    assert_eq!(again.state().n, 1, "on_stopping's write was saved");
    assert!(aio::shutdown(Duration::from_secs(5)).is_empty(), "the hooks ran once");
    // A hook that never returns can't hold the stop.
    aio::on_stop(|| std::thread::sleep(Duration::from_secs(60)));
    let at = Instant::now();
    assert_eq!(aio::shutdown(Duration::from_secs(1)), ["on_stop"]);
    assert!(at.elapsed() < Duration::from_secs(20), "given up at its budget: {:?}", at.elapsed());
}

#[test]
fn a_paused_cell_and_a_panic_refuse_with_aios_codes() {
    #[aio::cell]
    pub struct Probe {
        pub n: u32,
    }
    #[aio::methods]
    impl Probe {
        pub fn bug(&mut self) {
            panic!("a bug");
        }
        pub fn ok(&mut self) {
            self.n += 1;
        }
    }
    let h = aio::Handle::<Probe>::memory();
    assert_eq!(h.bug().unwrap_err().code(), Some(aio::Code::ReduceError));
    h.pause(true);
    let e = h.ok().unwrap_err();
    assert_eq!(e.code(), Some(aio::Code::DispatchClosed), "{e}");
    assert!(aio::error_summary(&e).starts_with("DISPATCH_CLOSED: "), "{}", aio::error_summary(&e));
}

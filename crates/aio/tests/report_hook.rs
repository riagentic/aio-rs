// Its own test binary: the reporter, the hook and the panic hook are global.
use std::sync::{Arc, Mutex};

#[test]
fn the_hook_sees_every_problem_and_its_own_panic_is_only_reported() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("reports");
    aio::report::init(aio::report::About { id: "notes".into(), dir: dir.clone(), ..Default::default() });
    aio::report::on_panic();
    let seen = Arc::new(Mutex::new(vec![]));
    let s = seen.clone();
    aio::report::on_problem(move |kind, message| {
        s.lock().unwrap().push(format!("{kind}: {message}"));
        assert_ne!(message, "bad", "the hook itself fails");
    });
    assert!(aio::report::problem("window:crashed", "gone").is_some());
    // A repeat is not reported again, but the hook still hears of it.
    assert!(aio::report::problem("window:crashed", "gone").is_none());
    // The hook panics: the caller goes on; the panic is a report of its own,
    // never handed back to the hook (that would loop).
    aio::report::problem("update:failed", "bad");
    aio::report::problem("degraded:sync", "offline");
    assert_eq!(
        *seen.lock().unwrap(),
        ["window:crashed: gone", "window:crashed: gone", "update:failed: bad", "degraded:sync: offline"]
    );
    let kinds: Vec<_> = aio::report::list(&dir).iter().map(|(_, v)| v["kind"].as_str().unwrap().to_string()).collect();
    assert_eq!(kinds, ["degraded:sync", "update:failed", "panic", "window:crashed"]);
}

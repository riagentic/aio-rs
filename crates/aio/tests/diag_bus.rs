// Its own test binary: the bus is global.
use std::sync::{Arc, Mutex};

#[test]
fn subscribers_hear_problems_and_warnings_until_dropped() {
    let heard = Arc::new(Mutex::new(vec![]));
    let h = heard.clone();
    let sub = aio::report::subscribe(move |d| h.lock().unwrap().push(format!("{} {} +{}", d.kind, d.message, d.held)));
    // A broken subscriber is logged; the others still hear.
    let broken = aio::report::subscribe(|_| panic!("subscriber bug"));
    aio::report::problem("window:crashed", "gone");
    aio::report::problem("window:crashed", "gone again");
    aio::health::part("bus:net").after(1).fail("offline");
    drop(broken);
    drop(sub);
    aio::report::problem("update:failed", "not heard");
    assert_eq!(*heard.lock().unwrap(), ["window:crashed gone +0", "degraded:bus:net offline +0"]);
    let recent: Vec<_> = aio::report::recent().into_iter().map(|d| d.kind).collect();
    assert_eq!(recent, ["window:crashed", "degraded:bus:net", "update:failed"], "recent keeps what was said");
}

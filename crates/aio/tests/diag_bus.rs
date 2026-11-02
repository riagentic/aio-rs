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
    diagnostics_carry_severity_a_tip_and_a_code_and_recoveries_are_said();
}

/// aio's bus events: each carries its severity and the fix; its kind is
/// typed (`code`); a degraded part that works again is said as recovered.
/// (Run by the test above: the bus is global.)
fn diagnostics_carry_severity_a_tip_and_a_code_and_recoveries_are_said() {
    use aio::report::{Code, Severity};
    let heard = Arc::new(Mutex::new(vec![]));
    let h = heard.clone();
    let _sub = aio::report::subscribe(move |d| {
        if d.kind.contains("sev:") {
            h.lock().unwrap().push((d.kind.clone(), d.severity, d.tip, d.code()));
        }
    });
    aio::report::problem("panic", "sev: a bug");
    let part = aio::health::part("sev:disk").after(1);
    part.fail("full");
    part.ok();
    let got = heard.lock().unwrap().clone();
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!((got[0].0.as_str(), got[0].1, got[0].3), ("degraded:sev:disk", Severity::Error, Code::Degraded));
    assert_eq!(
        (got[1].0.as_str(), got[1].1, got[1].3),
        ("degraded-recovered:sev:disk", Severity::Info, Code::Recovered)
    );
    let panic = aio::report::recent().into_iter().find(|d| d.kind == "panic").unwrap();
    assert_eq!((panic.severity, panic.tip, panic.code()), (Severity::Error, Code::Panic.tip(), Code::Panic));
    assert!(panic.tip.unwrap().contains("aio::reject!"));
}

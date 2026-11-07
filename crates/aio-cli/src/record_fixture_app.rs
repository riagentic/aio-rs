//! Recorded by `ax record` from the events 8..10 of notes: the calls
//! they took, on the state before them, and the state they left.
use aio::prelude::*;
use aio::serde_json::json;
use aio::testing::TestCells;
use super::tests::Steps;
use super::tests::Tally;

#[test]
fn a_session() {
    let mut t = TestCells::new();
    let steps = t.seeded::<Steps>(json!({"log":[],"n":0}));
    let tally = t.seeded::<Tally>(json!({"n":4}));
    steps.by(by(json!({"kind":"user","name":"ana"}))).call("add", json!([2,"first"])).unwrap(); // 8
    tally.by(by(json!({"kind":"user","name":"ana"}))).call("bump", json!([])).unwrap(); // 9
    steps.by(by(json!({"kind":"user","name":"ana"}))).call("add", json!([1,"second"])).unwrap(); // 10
    t.settle();
    assert_eq!(aio::serde_json::to_value(&*steps.state()).unwrap(), json!({"log":["first","second"],"n":3}));
    assert_eq!(aio::serde_json::to_value(&*tally.state()).unwrap(), json!({"n":5}));
}

/// Who made a call, as the log has it.
fn by(v: aio::serde_json::Value) -> aio::By {
    aio::serde_json::from_value(v).unwrap_or_default()
}

//! Recorded by `ax record` from the events 4..6 of notes: the calls
//! they took, on the state before them, and the state they left.
use aio::prelude::*;
use aio::serde_json::json;
use aio::testing::TestCells;
use super::tests::Steps;

#[test]
fn adds_two_notes() {
    let mut t = TestCells::new();
    let h = t.seeded::<Steps>(json!({"log":["seed"],"n":1}));
    h.by(by(json!({"kind":"user","name":"ana"}))).call("add", json!([2,"first"])).unwrap(); // 4
    // 5 $rewind: not called again: time travel or a restore: a state put back, not a call
    h.by(by(json!({"kind":"agent","name":"bot"}))).call("add", json!([3,"second"])).unwrap(); // 6
    t.settle();
    assert_eq!(aio::serde_json::to_value(&*h.state()).unwrap(), json!({"log":["seed","first","second"],"n":6}));
}

/// Who made a call, as the log has it.
fn by(v: aio::serde_json::Value) -> aio::By {
    aio::serde_json::from_value(v).unwrap_or_default()
}

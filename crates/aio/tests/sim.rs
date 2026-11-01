//! Simulation tests (`aio::testing::Sim`): a real server and real copies over
//! lines a seeded script cuts, heals and slows; every copy settles on the
//! server's state, and each call counts once however its line fared.
use aio::prelude::*;
use aio::testing::{Rng, Sim};
use serde_json::json;

#[cell(shared)]
pub struct Tally {
    /// Each call's own id, once per call that ran.
    pub ids: Vec<u64>,
    pub total: i64,
}

#[methods]
impl Tally {
    #[client]
    pub fn add(&mut self, id: u64, n: i64) {
        self.ids.push(id);
        self.total += n;
    }

    #[client]
    pub fn undo_last(&mut self) -> Result {
        ensure!(!self.ids.is_empty(), "nothing to undo");
        self.ids.pop();
        Ok(())
    }
}

fn add(r: &mut Rng) -> serde_json::Value {
    json!([r.number(), r.below(10) as i64 - 3])
}

fn none(_: &mut Rng) -> serde_json::Value {
    json!([])
}

/// No call ran twice: each id is there once.
fn once(t: &Tally) -> bool {
    let mut ids = t.ids.clone();
    ids.sort_unstable();
    ids.windows(2).all(|w| w[0] != w[1])
}

#[tokio::test(flavor = "multi_thread")]
async fn copies_settle_on_the_server_and_each_call_counts_once_through_cuts() {
    let calls: [aio::testing::RandomCall; 2] = [("add", &add), ("undo_last", &none)];
    let mut sim = Sim::start(Handle::<Tally>::memory(), 3, 7).await.unwrap();
    let script = sim.run(300, &calls, once).await;
    assert!(
        script.iter().any(|s| s.starts_with("cut")) && script.iter().any(|s| s.starts_with("c1.add")),
        "{script:?}"
    );
    let s = sim.server.state();
    assert!(!s.ids.is_empty() && once(&s));
    for c in sim.clients() {
        assert_eq!(*c.state(), *s, "every copy shows the server's state");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_same_seed_is_the_same_script() {
    let calls: [aio::testing::RandomCall; 2] = [("add", &add), ("undo_last", &none)];
    let run = async |seed| Sim::start(Handle::<Tally>::memory(), 2, seed).await.unwrap().run(60, &calls, once).await;
    let a = run(11).await;
    assert_eq!(run(11).await, a);
    assert_ne!(run(12).await, a);
}

#[tokio::test(flavor = "multi_thread")]
#[should_panic(expected = "sim seed 5: the server's state no longer holds")]
async fn a_failure_names_its_seed_and_script() {
    let calls: [aio::testing::RandomCall; 1] = [("add", &add)];
    let mut sim = Sim::start(Handle::<Tally>::memory(), 2, 5).await.unwrap();
    sim.run(200, &calls, |t| t.ids.len() < 4).await;
}

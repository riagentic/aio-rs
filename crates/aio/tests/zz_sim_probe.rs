// TEMP probe, never committed.
use aio::prelude::*;
use aio::testing::{RandomCall, Rng, Sim};
use serde_json::{Value, json};

#[cell(shared)]
pub struct Ledger {
    pub ids: Vec<u64>,
}

#[methods]
impl Ledger {
    #[client]
    pub fn add(&mut self, id: u64) {
        self.ids.push(id);
    }

    #[client]
    pub fn undo_last(&mut self) -> Result {
        ensure!(self.ids.pop().is_some(), "nothing to undo");
        Ok(())
    }
}

fn add(r: &mut Rng) -> Value {
    json!([r.number()])
}

fn none(_: &mut Rng) -> Value {
    Value::Null
}

fn once(l: &Ledger) -> bool {
    let mut ids = l.ids.clone();
    ids.sort_unstable();
    ids.windows(2).all(|w| w[0] != w[1])
}

#[tokio::test(flavor = "multi_thread")]
async fn probe() {
    use std::sync::{Arc, Mutex};
    for round in 0..40 {
        let calls: [RandomCall; 2] = [("add", &add), ("undo_last", &none)];
        let mut sim = Sim::start(Handle::<Ledger>::memory(), 3, 7).await.unwrap();
        let log: Arc<Mutex<Vec<String>>> = Arc::default();
        let t0 = std::time::Instant::now();
        {
            let l = log.clone();
            sim.server.handle.listen(move |s| {
                l.lock().unwrap().push(format!(
                    "{:?} server n{} last{:?}",
                    t0.elapsed(),
                    s.ids.len(),
                    s.ids.last().map(|x| x % 10000)
                ))
            });
        }
        for (i, c) in sim.clients().iter().enumerate() {
            let l = log.clone();
            c.on_patch(move |m| {
                l.lock().unwrap().push(format!("{:?} c{i} {} prev{} seq{}", t0.elapsed(), m["t"], m["prev"], m["seq"]))
            });
            let (l, c2) = (log.clone(), c.clone());
            c.listen(move |s| {
                l.lock().unwrap().push(format!(
                    "{:?} c{i} shown n{} last{:?} pend{} online{}",
                    t0.elapsed(),
                    s.ids.len(),
                    s.ids.last().map(|x| x % 10000),
                    c2.pending(),
                    c2.online()
                ))
            });
        }
        let script = sim.run(120, &calls, once).await;
        log.lock().unwrap().push(format!("{:?} SETTLED server seq {}", t0.elapsed(), sim.server.handle.snapshot().0));
        let bad: Vec<usize> = (0..3).filter(|&i| *sim.clients()[i].state() != *sim.server.state()).collect();
        if !bad.is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let after: Vec<bool> = (0..3).map(|i| *sim.clients()[i].state() == *sim.server.state()).collect();
            let l = log.lock().unwrap();
            panic!(
                "PROBE round {round} bad {bad:?} equal-after-500ms {after:?} server {:?} seqs {:?}\nLOG:\n{}\nSCRIPT {script:?}",
                sim.server.state().ids.iter().map(|x| x % 10000).collect::<Vec<_>>(),
                sim.clients().iter().map(|c| c.seq()).collect::<Vec<_>>(),
                l[l.len().saturating_sub(150)..].join("\n")
            );
        }
    }
}

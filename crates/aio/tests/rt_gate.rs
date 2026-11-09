//! The runtime's benchmark gate (aio's D12 `check:bench`): a cell started in
//! memory, a store opened, a call in memory, a saved call. Each is measured
//! many times and its p10 (the floor of the samples, steady where a median
//! swings with the machine's other work) must stay under a ceiling of twice
//! the most it measured: correct but slower fails like a broken test. The
//! ceilings are the debug build's (`cargo test`) and the release build's
//! (`cargo test --release`, as `./ci.sh` runs it), on Linux x86_64, the
//! machine they were measured on (as aio's gate is one machine's); a
//! ceiling moves only with the numbers that justify it.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
use aio::prelude::*;
use std::time::{Duration, Instant};

#[cell(persist)]
pub struct Tally {
    n: u64,
    names: Vec<String>,
}

#[methods]
impl Tally {
    pub fn bump(&mut self, by: u64) {
        self.n += by;
    }

    pub fn name(&mut self, s: String) {
        self.names.push(s);
    }
}

/// The p10 of `runs` samples of `f` (after one to warm up), each what `f`
/// says it took per operation.
fn p10(runs: usize, mut f: impl FnMut() -> Duration) -> Duration {
    f();
    let mut s: Vec<Duration> = (0..runs).map(|_| f()).collect();
    s.sort();
    s[runs / 10]
}

/// (metric, debug ceiling, release ceiling) in µs per operation: twice the
/// most p10 measured in two runs each on a Linux x86_64 desktop with other
/// builds beside it (debug: boot 129, call 13.4, saved call 202; release:
/// 76, 7.4, 137). `open` is bound by disk syncs, so its time swings with
/// the disk's other work (13 ms quiet, 54 ms under a parallel build):
/// its ceilings stay the pre-0.4 ones (twice 45 676 / 31 086 µs), loose
/// enough for a loaded disk, and its sync count is gated exactly instead
/// (`a_new_store_opens_and_closes_with_four_disk_syncs`).
const CEILINGS: [(&str, f64, f64); 4] =
    [("boot", 258.0, 152.0), ("open", 91_352.0, 62_172.0), ("call", 26.8, 14.8), ("saved-call", 404.0, 274.0)];

fn gate(metric: &str, took: Duration) {
    let (_, debug, release) = CEILINGS.iter().find(|c| c.0 == metric).expect("a ceiling");
    let ceiling = if cfg!(debug_assertions) { *debug } else { *release };
    let us = took.as_secs_f64() * 1e6;
    eprintln!("rt-gate {metric}: p10 {us:.2} µs/op, ceiling {ceiling} µs");
    assert!(us <= ceiling, "{metric}: p10 {us:.2} µs per op is over its ceiling of {ceiling} µs: correct but slower");
}

#[test]
fn a_start_a_call_and_a_saved_call_stay_under_their_ceilings() {
    gate(
        "boot",
        p10(100, || {
            let t = Instant::now();
            let h = Handle::<Tally>::memory();
            let took = t.elapsed();
            drop(h);
            took
        }),
    );
    let d = tempfile::tempdir().unwrap();
    let mut i = 0;
    gate(
        "open",
        p10(20, || {
            i += 1;
            let p = d.path().join(format!("open-{i}.db"));
            let t = Instant::now();
            let h = Handle::<Tally>::open(&p).unwrap();
            let took = t.elapsed();
            drop(h);
            took
        }),
    );
    let h = Handle::<Tally>::memory();
    gate(
        "call",
        p10(30, || {
            let t = Instant::now();
            (0..10_000).for_each(|_| _ = h.bump(1));
            t.elapsed() / 10_000
        }),
    );
    let h = Handle::<Tally>::open(d.path().join("saved.db")).unwrap();
    gate(
        "saved-call",
        p10(20, || {
            let t = Instant::now();
            (0..500).for_each(|i| _ = h.name(format!("n{i}")));
            t.elapsed() / 500
        }),
    );
}

// Its own test binary: the budgets, perf.log and the problem hook are global.
use aio::prelude::*;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cell]
pub struct Slow {
    pub got: u32,
}

#[methods]
impl Slow {
    pub fn crunch(&mut self, ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }

    /// Slow on purpose, with a budget of its own.
    pub fn heavy(&mut self) {
        std::thread::sleep(Duration::from_millis(30));
    }

    pub fn got(&mut self) {
        self.got += 1;
    }

    pub fn nope(&mut self) -> Result {
        reject!("not now")
    }

    /// A step that blocks its worker thread.
    pub fn fetch(&mut self, cx: &Cx) {
        cx.run(Effect::new("fetch", |h: Handle<Slow>| async move {
            std::thread::sleep(Duration::from_millis(20));
            _ = h.got();
        }));
    }

    /// A quick step whose call is slow: the call is the method's, not the step's.
    pub fn relay(&mut self, cx: &Cx) {
        cx.run(Effect::new("relay", |h: Handle<Slow>| async move {
            _ = h.heavy();
            _ = h.got();
        }));
    }

    pub fn refuse_later(&mut self, cx: &Cx) {
        cx.run(Effect::new("later", |h: Handle<Slow>| async move { _ = h.nope() }));
    }

    pub fn hang(&mut self, cx: &Cx) {
        cx.run(
            Effect::new("hang", |_: Handle<Slow>| async move { tokio::time::sleep(Duration::from_secs(5)).await })
                .timeout(Duration::from_millis(20)),
        );
    }
}

async fn until(what: &str, f: impl Fn() -> bool) {
    for _ in 0..300 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out: {what}");
}

#[tokio::test(flavor = "multi_thread")]
async fn slow_methods_and_effect_steps_reach_perf_log_and_the_hook() {
    let d = tempfile::tempdir().unwrap();
    aio::perf::start(d.path()).unwrap();
    aio::perf::set(toml::from_str("method = 5\neffect = 5\n[methods]\n\"slow.heavy\" = 1000").unwrap());
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let s = seen.clone();
    aio::report::on_problem(move |kind, message| s.lock().unwrap().push(format!("{kind}: {message}")));
    let h = Handle::<Slow>::memory();
    h.crunch(0).unwrap();
    h.crunch(20).unwrap();
    h.fetch().unwrap();
    h.relay().unwrap();
    h.refuse_later().unwrap();
    h.hang().unwrap();
    until("effects ran", || h.state().got == 2 && !h.busy("hang") && !h.busy("later")).await;
    // A step is timed once its poll returns: after the call it made.
    until("the slow step said", || {
        seen.lock().unwrap().iter().any(|l| l.starts_with("budget:effect: effect slow.fetch "))
    })
    .await;

    let seen = seen.lock().unwrap().clone();
    let has = |p: &str| seen.iter().any(|l| l.starts_with(p));
    assert!(has("budget:method: method slow.crunch "), "{seen:?}");
    assert_eq!(seen.iter().filter(|l| l.contains("slow.crunch")).count(), 1, "the quick call is within budget");
    assert!(has("budget:effect: effect slow.fetch "), "{seen:?}");
    assert!(!seen.iter().any(|l| l.contains("slow.relay") || l.contains("slow.heavy")), "{seen:?}");
    assert!(has("refused: slow.nope: not now"), "{seen:?}");
    assert!(has("effect:timeout: slow: effect `hang` timed out"), "{seen:?}");

    let log = std::fs::read_to_string(d.path().join("perf.log")).unwrap();
    let crunch = log.lines().find(|l| l.contains("method slow.crunch")).unwrap();
    assert!(crunch[25..].starts_with("PERF  [perf] method slow.crunch "), "{crunch}");
    assert!(crunch.contains(" > 5ms (clone=") && crunch.contains(" commit="), "{crunch}");
    assert!(log.contains("PERF  [perf] effect slow.fetch "), "{log}");
}

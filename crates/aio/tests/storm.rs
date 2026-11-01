// Its own test binary: [perf] and the problem hook are global.
use aio::prelude::*;
use std::sync::{Arc, Mutex};

#[cell]
pub struct Watcher {
    pub seen: u32,
}

#[methods]
impl Watcher {
    pub fn changed(&mut self) {
        self.seen += 1;
    }
}

#[test]
fn a_call_storm_is_said_and_the_breaker_refuses_it() {
    aio::perf::set(toml::from_str("[storm]\nrate = 20\nsecs = 1\nbreaker = true").unwrap());
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let s = seen.clone();
    aio::report::on_problem(move |kind, message| s.lock().unwrap().push(format!("{kind}: {message}")));
    let h = Handle::<Watcher>::memory();
    let t = std::time::Instant::now();
    let refused = loop {
        assert!(t.elapsed().as_secs() < 5, "never refused");
        if let Err(e) = h.changed() {
            break e;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    assert_eq!(refused.why, "watcher.changed: refused during a call storm ([perf.storm] breaker)");
    let seen = seen.lock().unwrap().clone();
    assert!(seen.iter().any(|l| l.starts_with("storm: watcher.changed called ")), "{seen:?}");
    assert_eq!(aio::report::Code::of("storm"), aio::report::Code::Storm);
}

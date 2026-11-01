//! A person is deciding (aio's `pauseCallDeadlines`): a picker is open, and
//! thirty seconds in a dialog is ordinary. What waits on a server fn call
//! ([`crate::fns`]) must not give up meanwhile: each call in flight tells its
//! caller to hold its clock (`sfnh`), and to start a full window again after.
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// How to tell a call's caller to hold its clock (`true`) or start it again.
pub(crate) type Tell = Arc<dyn Fn(bool) + Send + Sync>;

/// Every call in flight, by number.
static HOLDABLE: Mutex<BTreeMap<u64, Tell>> = Mutex::new(BTreeMap::new());

fn holdable() -> std::sync::MutexGuard<'static, BTreeMap<u64, Tell>> {
    HOLDABLE.lock().unwrap_or_else(|e| e.into_inner())
}

/// A call in flight, holdable until it ends (or is cancelled).
pub(crate) struct InFlight(u64);

#[cfg_attr(not(feature = "server"), allow(dead_code))]
impl InFlight {
    pub(crate) fn new(tell: Tell) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        holdable().insert(n, tell);
        Self(n)
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        holdable().remove(&self.0);
    }
}

/// A person is deciding (aio's `pauseCallDeadlines`): while this lives, every
/// server fn call in flight has its caller's clock held, so thirty seconds in
/// a dialog never loses the pick. Dropped, each still running gets a fresh
/// full window: one dialog is no amnesty for a call that really hangs. The
/// pickers take one; so does code that waits on a person some other way.
#[must_use = "the hold ends when this is dropped"]
pub struct Deciding(Vec<(u64, Tell)>);

/// Hold the clock of every server fn call in flight ([`Deciding`]).
pub fn start() -> Deciding {
    let held: Vec<_> = holdable().iter().map(|(n, t)| (*n, t.clone())).collect();
    held.iter().for_each(|(_, t)| t(true));
    Deciding(held)
}

impl Drop for Deciding {
    fn drop(&mut self) {
        let alive: Vec<_> = {
            let now = holdable();
            self.0.drain(..).filter(|(n, _)| now.contains_key(n)).collect()
        };
        alive.iter().for_each(|(_, t)| t(false));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hold_tells_the_calls_in_flight_and_resumes_only_survivors() {
        let told = Arc::new(Mutex::new(vec![]));
        let call = |id: &'static str| {
            let told = told.clone();
            InFlight::new(Arc::new(move |h| told.lock().unwrap().push((id, h))))
        };
        let (a, b) = (call("a"), call("b"));
        let held = start();
        // Started during the hold: it has no clock held, nothing to resume.
        let late = call("late");
        drop(b);
        drop(held);
        let mine: Vec<_> = told.lock().unwrap().iter().filter(|(id, _)| *id != "late").copied().collect();
        assert_eq!(mine, [("a", true), ("b", true), ("a", false)]);
        assert!(!told.lock().unwrap().iter().any(|(id, _)| *id == "late"));
        drop((a, late));
    }
}

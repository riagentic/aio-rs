//! A copy of a big state made ahead, off the caller's thread, so a call
//! starts on it instead of copying the state first: a cell of 1000 rows
//! costs a held key ~1.5 ms per step otherwise (aio's Immer drafts copy
//! only what the method touches; a Rust state is copied whole). Made while
//! copies are slow: [`ON`] of the last 8 took [`SLOW`] or more (one copy a
//! busy machine held up is not enough), until 8 in a row were fast again.
//! The copy waits beside the state it copies and is taken only while that
//! state is still the current one.
use crate::kind::St;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A copy this slow counts towards making them ahead.
pub(crate) const SLOW: Duration = Duration::from_micros(250);

/// How many of the last 8 copies took [`SLOW`] to turn making them ahead on.
const ON: u32 = 3;

/// The state a copy is asked for, and the copy once made, with how long it took.
type Slot = Option<(St, Option<(St, Duration)>)>;

/// A job for the copies' thread.
type Job = Box<dyn FnOnce() + Send>;

/// The copy made ahead of a state.
#[derive(Default)]
pub(crate) struct Spare {
    on: bool,
    /// The last 8 copies, the newest in the lowest bit: 1 took [`SLOW`] or more.
    slow: u8,
    slot: Arc<Mutex<Slot>>,
}

impl Spare {
    /// A copy of `state` to change, alone in its `Arc`: the one made ahead,
    /// else made now. Either counts towards making the next ones ahead.
    pub fn copy(&mut self, state: &St) -> St {
        let ahead = lock(&self.slot).take().filter(|(of, _)| Arc::ptr_eq(of, state));
        let (t, took) = match ahead {
            Some((_, Some(made))) => made,
            _ => {
                let t0 = Instant::now();
                (state.dup(), t0.elapsed())
            }
        };
        self.took(took);
        t
    }

    /// A copy took `took`: copies are made ahead while [`ON`] of the last 8
    /// were slow, and no more once 8 in a row were fast.
    fn took(&mut self, took: Duration) {
        self.slow = self.slow << 1 | u8::from(took >= SLOW);
        let slow = self.slow.count_ones();
        self.on = cfg!(not(target_arch = "wasm32")) && if self.on { slow > 0 } else { slow >= ON };
    }

    /// `state` is the current one: copy it ahead (when on, and not yet asked).
    pub fn ahead(&mut self, state: &St) {
        let mut slot = lock(&self.slot);
        if !self.on || slot.as_ref().is_some_and(|(of, _)| Arc::ptr_eq(of, state)) {
            return;
        }
        *slot = Some((state.clone(), None));
        drop(slot);
        let (slot, of) = (Arc::downgrade(&self.slot), Arc::downgrade(state));
        worker(Box::new(move || {
            // Skipped once its store went or the state moved on.
            let wanted = |s: &Slot, of: &St| s.as_ref().is_some_and(|(w, _)| Arc::ptr_eq(w, of));
            let (Some(slot), Some(of)) = (slot.upgrade(), of.upgrade()) else { return };
            if !wanted(&lock(&slot), &of) {
                return;
            }
            let t0 = Instant::now();
            let t = of.dup();
            let took = t0.elapsed();
            let mut s = lock(&slot);
            if wanted(&s, &of) {
                *s = Some((of, Some((t, took))));
            }
        }));
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// One thread for every store's copies, started at the first.
#[cfg(not(target_arch = "wasm32"))]
fn worker(job: Job) {
    use std::sync::mpsc::{Sender, channel};
    static TX: Mutex<Option<Sender<Job>>> = Mutex::new(None);
    let mut tx = lock(&TX);
    let job = match tx.as_ref() {
        Some(t) => match t.send(job) {
            Ok(()) => return,
            Err(e) => e.0,
        },
        None => job,
    };
    let (t, rx) = channel::<Job>();
    let started = std::thread::Builder::new().name("aio-spare".into()).spawn(move || {
        for job in rx {
            // A copy that panicked (a Clone impl's) leaves only the copy undone.
            _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
        }
    });
    if started.is_ok() && t.send(job).is_ok() {
        *tx = Some(t);
    }
}

#[cfg(target_arch = "wasm32")]
fn worker(_: Job) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kind::typed;

    #[crate::cell(id = "big")]
    pub struct Big {
        rows: Vec<String>,
    }

    #[crate::methods]
    impl Big {}

    fn of(rows: Vec<String>) -> St {
        Arc::new(Big { rows })
    }

    fn big() -> St {
        of((0..200_000).map(|i| i.to_string()).collect())
    }

    fn wait(s: &Spare) {
        let t = std::time::Instant::now();
        while !lock(&s.slot).as_ref().is_some_and(|(_, c)| c.is_some()) && t.elapsed() < std::time::Duration::from_secs(10) {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    #[test]
    fn a_slow_copy_is_made_ahead_for_the_state_it_copies_only() {
        let mut s = Spare::default();
        let a = big();
        let copy = s.copy(&a);
        assert!(copy.same(&*a) && !Arc::ptr_eq(&copy, &a), "the first copy is made on the spot");
        (1..ON).for_each(|_| _ = s.copy(&a));
        assert!(s.on, "they were slow: copies are made ahead from now on");
        s.ahead(&a);
        wait(&s);
        assert!(lock(&s.slot).as_ref().is_some_and(|(of, t)| Arc::ptr_eq(of, &a) && t.as_ref().is_some_and(|(t, _)| t.same(&*a))));
        assert!(s.copy(&a).same(&*a), "taken");
        assert!(lock(&s.slot).is_none());
        // Made for a state that moved on: not taken, a fresh copy instead.
        s.ahead(&a);
        wait(&s);
        let b = of(vec!["b".into()]);
        assert_eq!(typed::<Big>(s.copy(&b)).rows, ["b"]);
        assert!(lock(&s.slot).is_none(), "the stale copy is gone");
    }

    #[test]
    fn copies_are_made_ahead_only_while_they_are_slow() {
        let (mut s, slow, fast) = (Spare::default(), SLOW, Duration::ZERO);
        s.took(slow);
        (0..20).for_each(|_| s.took(fast));
        assert!(!s.on, "one slow copy (a busy machine's) is not enough");
        [slow, fast, slow, fast, fast, fast, fast].into_iter().for_each(|t| s.took(t));
        assert!(!s.on, "two of the last 8");
        s.took(slow);
        assert!(s.on, "three of the last 8");
        (0..7).for_each(|_| s.took(fast));
        assert!(s.on, "on while any of the last 8 was slow");
        s.took(fast);
        assert!(!s.on, "8 fast copies in a row: off again");
        (0..ON).for_each(|_| s.took(slow));
        assert!(s.on, "slow again, on again");
    }

    #[test]
    fn a_fast_copy_is_never_made_ahead() {
        let mut s = Spare::default();
        let a = of(vec!["a".into()]);
        assert!(s.copy(&a).same(&*a));
        s.ahead(&a);
        assert!(!s.on && lock(&s.slot).is_none());
    }
}

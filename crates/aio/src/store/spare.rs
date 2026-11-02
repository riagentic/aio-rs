//! A copy of a big state made ahead, off the caller's thread, so a call
//! starts on it instead of copying the state first: a cell of 1000 rows
//! costs a held key ~1.5 ms per step otherwise (aio's Immer drafts copy
//! only what the method touches; a Rust state is copied whole). Made once
//! copying took [`SLOW`] or more; the copy waits beside the state it copies
//! and is taken only while that state is still the current one.
use std::sync::{Arc, Mutex};

/// A copy this slow is made ahead from then on.
pub(crate) const SLOW: std::time::Duration = std::time::Duration::from_micros(250);

/// The state a copy is asked for, and the copy once made.
type Slot<T> = Option<(Arc<T>, Option<T>)>;

/// A job for the copies' thread.
type Job = Box<dyn FnOnce() + Send>;

/// The copy made ahead of `T`.
pub(crate) struct Spare<T> {
    on: bool,
    slot: Arc<Mutex<Slot<T>>>,
}

impl<T> Default for Spare<T> {
    fn default() -> Self {
        Self { on: false, slot: Arc::default() }
    }
}

impl<T: Clone + Send + Sync + 'static> Spare<T> {
    /// A copy of `state` to change: the one made ahead, else made now
    /// (one that took [`SLOW`] turns making them ahead on).
    pub fn copy(&mut self, state: &Arc<T>) -> T {
        let ahead = lock(&self.slot).take().filter(|(of, _)| Arc::ptr_eq(of, state));
        if let Some((_, Some(t))) = ahead {
            return t;
        }
        let t0 = std::time::Instant::now();
        let t = (**state).clone();
        self.on |= t0.elapsed() >= SLOW && cfg!(not(target_arch = "wasm32"));
        t
    }

    /// `state` is the current one: copy it ahead (when on, and not yet asked).
    pub fn ahead(&mut self, state: &Arc<T>) {
        let mut slot = lock(&self.slot);
        if !self.on || slot.as_ref().is_some_and(|(of, _)| Arc::ptr_eq(of, state)) {
            return;
        }
        *slot = Some((state.clone(), None));
        drop(slot);
        let (slot, of) = (Arc::downgrade(&self.slot), Arc::downgrade(state));
        worker(Box::new(move || {
            // Skipped once its store went or the state moved on.
            let wanted = |s: &Slot<T>, of: &Arc<T>| s.as_ref().is_some_and(|(w, _)| Arc::ptr_eq(w, of));
            let (Some(slot), Some(of)) = (slot.upgrade(), of.upgrade()) else { return };
            if !wanted(&lock(&slot), &of) {
                return;
            }
            let t = (*of).clone();
            let mut s = lock(&slot);
            if wanted(&s, &of) {
                *s = Some((of, Some(t)));
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

    #[derive(Clone, Debug, PartialEq)]
    struct Big(Vec<String>);

    fn big() -> Arc<Big> {
        Arc::new(Big((0..200_000).map(|i| i.to_string()).collect()))
    }

    fn wait(s: &Spare<Big>) {
        let t = std::time::Instant::now();
        while !lock(&s.slot).as_ref().is_some_and(|(_, c)| c.is_some()) && t.elapsed() < std::time::Duration::from_secs(10) {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    #[test]
    fn a_slow_copy_is_made_ahead_for_the_state_it_copies_only() {
        let mut s = Spare::default();
        let a = big();
        assert_eq!(s.copy(&a), *a, "the first copy is made on the spot");
        assert!(s.on, "it was slow: copies are made ahead from now on");
        s.ahead(&a);
        wait(&s);
        assert!(lock(&s.slot).as_ref().is_some_and(|(of, t)| Arc::ptr_eq(of, &a) && t.as_ref() == Some(&*a)));
        assert_eq!(s.copy(&a), *a, "taken");
        assert!(lock(&s.slot).is_none());
        // Made for a state that moved on: not taken, a fresh copy instead.
        s.ahead(&a);
        wait(&s);
        let b = Arc::new(Big(vec!["b".into()]));
        assert_eq!(s.copy(&b), *b);
        assert!(lock(&s.slot).is_none(), "the stale copy is gone");
    }

    #[test]
    fn a_fast_copy_is_never_made_ahead() {
        let mut s = Spare::default();
        let a = Arc::new(Big(vec!["a".into()]));
        assert_eq!(s.copy(&a), *a);
        s.ahead(&a);
        assert!(!s.on && lock(&s.slot).is_none());
    }
}

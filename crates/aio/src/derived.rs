//! Derived values (aio selectors): a pure function of cell states (and
//! arguments), cached until an input changes. A commit makes a new state
//! `Arc`, so "changed" is a pointer test, never a deep compare.
use std::sync::{Arc, Mutex, Weak};

/// A state as a cache key: equal only to the same `Arc`. Weak, so it keeps
/// no old state alive, yet it holds the allocation, so a newer state can't
/// reuse the address and pass for it.
pub struct Ver<T>(Weak<T>);

impl<T> Ver<T> {
    pub fn of(state: &Arc<T>) -> Self {
        Self(Arc::downgrade(state))
    }
}

impl<T> PartialEq for Ver<T> {
    fn eq(&self, other: &Self) -> bool {
        Weak::ptr_eq(&self.0, &other.0)
    }
}

/// A state as a cache key that stays the same while the fields a derived
/// value reads do (`#[derived(reads = [rows])]`; aio's `memoLast`'s deps): the
/// same `Arc`, or one whose read fields equal it. It keeps that state alive
/// to compare with (one state per derived value).
pub struct Reads<T>(Arc<T>, fn(&T, &T) -> bool);

impl<T> Reads<T> {
    /// `same`: are the fields it reads equal in both states?
    pub fn of(state: &Arc<T>, same: fn(&T, &T) -> bool) -> Self {
        Self(state.clone(), same)
    }
}

impl<T> PartialEq for Reads<T> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || (self.1)(&self.0, &other.0)
    }
}

/// The last answer of a derived value and the inputs it was made from
/// (aio's `createSelector`, `memoLast`: same inputs, the same answer, so an
/// `Arc` answer keeps its identity for the caches keyed on it). `#[derived]`
/// keeps one per function.
pub struct Derived<K, R>(Mutex<Option<(K, R)>>);

impl<K: PartialEq, R: Clone> Default for Derived<K, R> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: PartialEq, R: Clone> Derived<K, R> {
    pub const fn new() -> Self {
        Self(Mutex::new(None))
    }

    /// The kept answer if `key` is the last one's, else `f()`, kept. A
    /// panicking `f` keeps nothing.
    pub fn get(&self, key: K, f: impl FnOnce() -> R) -> R {
        if let Some((k, r)) = &*self.0.lock().unwrap_or_else(|e| e.into_inner())
            && *k == key
        {
            return r.clone();
        }
        let r = f();
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some((key, r.clone()));
        r
    }
}

/// Answers kept per key while their source stays the same (aio's caches on
/// an array's identity: `groupBy`, an index by owner, the panel
/// rows of a frame): [`get`](Self::get) gives `key`'s kept answer while
/// `source` (a [`Ver`], a part's stamp `ui::Part::ver`, a tuple of them) is
/// the last one's, and a new `source` drops every answer. A static one is
/// shared by every view and handler (`static ROWS: Cache<u64, Panel,
/// Arc<Vec<Row>>> = Cache::new();`).
pub struct Cache<S, K, R>(Mutex<Option<(S, std::collections::HashMap<K, R>)>>);

impl<S: PartialEq, K: Eq + std::hash::Hash, R: Clone> Default for Cache<S, K, R> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: PartialEq, K: Eq + std::hash::Hash, R: Clone> Cache<S, K, R> {
    pub const fn new() -> Self {
        Self(Mutex::new(None))
    }

    /// `key`'s answer from `source`: the kept one, else `f()`, kept. `f`
    /// runs unlocked (it may ask this cache for another key); a panicking
    /// `f` keeps nothing.
    pub fn get(&self, source: S, key: K, f: impl FnOnce() -> R) -> R {
        if let Some((s, kept)) = &*self.0.lock().unwrap_or_else(|e| e.into_inner())
            && *s == source
            && let Some(r) = kept.get(&key)
        {
            return r.clone();
        }
        let r = f();
        let mut l = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match &mut *l {
            Some((s, kept)) if *s == source => _ = kept.insert(key, r.clone()),
            _ => *l = Some((source, std::collections::HashMap::from([(key, r.clone())]))),
        }
        r
    }
}

/// A listener that calls `on` with `f`'s answer when it differs from the
/// last one (the first state counts as a change): `watch` outside views.
pub(crate) fn changes<T, R: PartialEq + Send + 'static>(
    f: impl Fn(&T) -> R + Send + Sync + 'static,
    on: impl Fn(&R) + Send + Sync + 'static,
) -> impl Fn(&Arc<T>) + Send + Sync + 'static {
    let last = Mutex::new(None);
    move |st| {
        let r = f(st);
        let mut l = last.lock().unwrap_or_else(|e| e.into_inner());
        if l.as_ref() != Some(&r) {
            on(&r);
            *l = Some(r);
        }
    }
}

/// What `on_change` returns: while it lives the reaction runs and what it
/// made last is kept; dropped, both stop.
#[must_use = "dropping it stops the reaction"]
pub struct Watching(#[allow(dead_code)] Arc<dyn std::any::Any + Send + Sync>);

/// The last answer and what the reaction made of it.
type Reacting<R, M> = Mutex<(R, Option<M>)>;

/// A listener that calls `on(new, Some(old))` when `f`'s answer changes
/// (and `on(now, None)` at once if `immediate`), dropping what the last call
/// made before the next runs. `on` runs unlocked: it may call the cell.
pub(crate) fn reaction<T, R, M>(
    now: &T,
    immediate: bool,
    f: impl Fn(&T) -> R + Send + Sync + 'static,
    on: impl Fn(&R, Option<&R>) -> M + Send + Sync + 'static,
) -> (impl Fn(&Arc<T>) + Send + Sync + 'static, Watching)
where
    R: PartialEq + Clone + Send + 'static,
    M: Send + 'static,
{
    let first = f(now);
    let made = immediate.then(|| on(&first, None));
    let live: Arc<Reacting<R, M>> = Arc::new(Mutex::new((first, made)));
    let weak = Arc::downgrade(&live);
    let listener = move |st: &Arc<T>| {
        let Some(live) = weak.upgrade() else { return };
        let r = f(st);
        let (old, made) = {
            let mut l = live.lock().unwrap_or_else(|e| e.into_inner());
            if l.0 == r {
                return;
            }
            (std::mem::replace(&mut l.0, r.clone()), l.1.take())
        };
        drop(made);
        let next = on(&r, Some(&old));
        live.lock().unwrap_or_else(|e| e.into_inner()).1 = Some(next);
    };
    (listener, Watching(live))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn same_inputs_reuse_the_answer_new_ones_recompute() {
        let (d, runs) = (Derived::new(), Cell::new(0));
        let f = |x: u32| {
            d.get(x, || {
                runs.set(runs.get() + 1);
                x * 2
            })
        };
        assert_eq!((f(1), f(1), runs.get()), (2, 2, 1));
        assert_eq!((f(2), runs.get()), (4, 2));
        assert_eq!((f(1), runs.get()), (2, 3), "one slot, like aio");
    }

    #[test]
    fn a_cache_keeps_each_keys_answer_until_its_source_changes() {
        let (c, runs) = (Cache::new(), Cell::new(0));
        let f = |src: u64, k: &str| {
            c.get(src, k.to_string(), || {
                runs.set(runs.get() + 1);
                format!("{src}{k}")
            })
        };
        assert_eq!((f(1, "a"), f(1, "b"), f(1, "a"), runs.get()), ("1a".into(), "1b".into(), "1a".into(), 2));
        assert_eq!((f(2, "a"), runs.get()), ("2a".into(), 3), "a new source drops them");
        assert_eq!((f(2, "b"), runs.get()), ("2b".into(), 4), "all of them");
        let nested = c.get(3, "x".into(), || f(3, "y") + "!");
        assert_eq!(nested, "3y!", "f may ask the cache itself");
    }

    #[test]
    fn reads_hold_while_the_fields_read_are_equal() {
        let same = |a: &(u32, u32), b: &(u32, u32)| a.0 == b.0;
        let a = Arc::new((1, 1));
        assert!(Reads::of(&a, same) == Reads::of(&Arc::new((1, 9)), same), "the other field moved");
        assert!(Reads::of(&a, same) != Reads::of(&Arc::new((2, 1)), same));
    }

    #[test]
    fn a_state_is_its_arc_not_its_value() {
        let (a, b) = (Arc::new(vec![1]), Arc::new(vec![1]));
        assert!(Ver::of(&a) == Ver::of(&a.clone()));
        assert!(Ver::of(&a) != Ver::of(&b), "equal values, another state");
        let old = Ver::of(&a);
        drop(a);
        assert!(old != Ver::of(&Arc::new(vec![1])), "a dropped state's address isn't reused");
    }

    #[test]
    fn a_watch_tells_only_changes() {
        let seen = Arc::new(Mutex::new(vec![]));
        let s = seen.clone();
        let l = changes(|v: &Vec<u32>| v.len(), move |n| s.lock().unwrap().push(*n));
        for v in [vec![], vec![1], vec![2], vec![1, 2], vec![]] {
            l(&Arc::new(v));
        }
        assert_eq!(*seen.lock().unwrap(), [0, 1, 2, 0]);
    }

    #[test]
    fn a_panicking_compute_keeps_nothing() {
        let d = Derived::new();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| d.get(1, || panic!("boom"))));
        assert!(r.is_err());
        assert_eq!(d.get(1, || 7), 7);
    }
}

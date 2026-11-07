//! A big field of a cell, shared between states until written (aio's Immer
//! drafts: what a call didn't touch is the same object before and after).
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

/// `pub rows: Big<Vec<Row>>`: reads and writes as its value
/// (`self.rows.len()`, `self.rows.push(r)`). A call's copy of the state
/// shares it (a pointer, not 1000 rows), and a write copies it once, then
/// only if another state still shares it. Two that share it are equal at
/// once: a step that didn't write it costs no comparison (the views'
/// re-render, a `#[coalesce]` run, your own `==`), where a plain field is
/// compared item by item. Saved and sent as its value.
///
/// `T` is plain data: never one that changes behind `&` (`Cell`,
/// `RefCell`, `Mutex`, an atomic, or a type holding one). A change through
/// it reaches every state sharing the value, the states before the call
/// too, and [`Big::ptr_eq`] still says they are equal, so nothing sees it
/// change. Write through `DerefMut` (`self.rows.push(r)`), which copies
/// first. The type system can't refuse such a `T`: this is the rule.
#[derive(Default)]
pub struct Big<T>(Arc<T>);

impl<T> Big<T> {
    pub fn new(value: T) -> Self {
        Self(Arc::new(value))
    }

    /// Do `a` and `b` share one value (pure)? Sharing is equality only
    /// for plain data (the type's rule above).
    pub fn ptr_eq(a: &Self, b: &Self) -> bool {
        Arc::ptr_eq(&a.0, &b.0)
    }
}

/// A pointer copy: the two share the value until one is written.
impl<T> Clone for Big<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

/// Shared: equal without a look; else compared by value.
impl<T: PartialEq> PartialEq for Big<T> {
    fn eq(&self, other: &Self) -> bool {
        Self::ptr_eq(self, other) || *self.0 == *other.0
    }
}

impl<T: Eq> Eq for Big<T> {}

impl<T> From<T> for Big<T> {
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

impl<T> Deref for Big<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

/// A write: the value copied first while another state shares it.
impl<T: Clone> DerefMut for Big<T> {
    fn deref_mut(&mut self) -> &mut T {
        Arc::make_mut(&mut self.0)
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for Big<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl<T: Serialize> Serialize for Big<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Big<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        T::deserialize(d).map(Self::new)
    }
}

/// How long a field's comparison may take before a dev build says to make
/// it a [`Big`] (a frame is 16 ms; a held key compares each field a few
/// times a step).
pub const SLOW_COMPARE: std::time::Duration = std::time::Duration::from_micros(500);

/// `differs()`, the comparison of `cell`'s field `field` after a call (the
/// views' changed-field mask, a coalesced run's write); a dev build times
/// it and says once per field when it took [`SLOW_COMPARE`] or more. Not
/// in a browser: `std`'s clock isn't there (`Instant::now` panics).
#[doc(hidden)]
#[inline]
pub fn differs(cell: &'static str, field: &'static str, differs: impl FnOnce() -> bool) -> bool {
    if !crate::DEV_BUILD || cfg!(target_arch = "wasm32") {
        return differs();
    }
    let t = std::time::Instant::now();
    let d = differs();
    let took = t.elapsed();
    if took >= SLOW_COMPARE {
        slow(cell, field, took);
    }
    d
}

#[cold]
fn slow(cell: &'static str, field: &'static str, took: std::time::Duration) {
    static SAID: std::sync::Mutex<Vec<(&str, &str)>> = std::sync::Mutex::new(Vec::new());
    let mut said = SAID.lock().unwrap_or_else(|e| e.into_inner());
    if said.contains(&(cell, field)) {
        return;
    }
    said.push((cell, field));
    crate::info!(
        cat: "hint",
        "field `{field}` of cell `{cell}` took {:.1} ms to compare: make it `aio::Big<…>` (states that share it \
         compare at once), and write it only when it changes",
        took.as_secs_f64() * 1e3
    );
}

#[cfg(test)]
mod dev_build {
    /// `ax dev`'s build (aio's `dev`, no debug assertions) is a dev build:
    /// a slow compare is said, once per field.
    #[test]
    fn a_dev_build_says_a_slow_compare_once() {
        let slow = || {
            std::thread::sleep(super::SLOW_COMPARE * 2);
            true
        };
        let (_, said) = crate::log::capture(|| (super::differs("t", "f", slow), super::differs("t", "f", slow)));
        let n = said.iter().filter(|l| l.contains("field `f` of cell `t` took")).count();
        assert_eq!(n, usize::from(cfg!(any(debug_assertions, feature = "dev"))), "{said:?}");
    }
}

//! App-wide state in a `static`, one per app instance (per mounted
//! [`Root`](crate::Root), so per test): made on first use, under the root
//! (its context is the root's), ended with it.
use crate::reactive::{Memo, ReadGuard, Signal, WriteGuard, detached, memo_now, with_owner};
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

thread_local!(static GLOBALS: RefCell<HashMap<usize, Rc<dyn Any>>> = RefCell::default());

/// A new app instance: every global is made again on its next use.
pub(crate) fn reset() {
    let old = GLOBALS.take();
    drop(old);
}

fn get_or<T: Clone + 'static>(at: usize, make: impl FnOnce() -> T) -> T {
    if let Some(v) = GLOBALS.with_borrow(|g| g.get(&at).cloned()) {
        return v.downcast_ref::<T>().cloned().expect("air: a global of another type");
    }
    let v = match crate::dom::root_owner() {
        Some(r) => with_owner(r, make),
        None => detached(make),
    };
    GLOBALS.with_borrow_mut(|g| g.insert(at, Rc::new(v.clone())));
    v
}

/// A signal in a `static`: `static THEME: Global<String> = Global::new(|| "dark".into());`.
pub struct Global<T: 'static> {
    init: fn() -> T,
}

impl<T: 'static> Global<T> {
    /// One per `static`.
    pub const fn new(init: fn() -> T) -> Self {
        Self { init }
    }

    /// Kept for the facade's macros: a `static` is its own key on air.
    #[doc(hidden)]
    pub const fn named(init: fn() -> T, _name: &'static str) -> Self {
        Self { init }
    }

    /// This app instance's signal.
    pub fn signal(&self) -> Signal<T> {
        get_or(self as *const Self as usize, || Signal::new((self.init)()))
    }

    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.signal().get()
    }

    pub fn get_untracked(&self) -> T
    where
        T: Clone,
    {
        self.signal().get_untracked()
    }

    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.signal().with(f)
    }

    pub fn with_untracked<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.signal().with_untracked(f)
    }

    pub fn read(&self) -> ReadGuard<T> {
        self.signal().read()
    }

    pub fn read_untracked(&self) -> ReadGuard<T> {
        self.signal().read_untracked()
    }

    pub fn set(&self, v: T)
    where
        T: PartialEq,
    {
        self.signal().set(v)
    }

    pub fn update<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        self.signal().update(f)
    }

    pub fn write(&self) -> WriteGuard<T> {
        self.signal().write()
    }
}

impl<T: std::fmt::Display + 'static> std::fmt::Display for Global<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.signal().fmt(f)
    }
}

/// A memo in a `static`: `static N: GlobalMemo<usize> = GlobalMemo::new(|| ROWS.with(Vec::len));`.
pub struct GlobalMemo<T: PartialEq + 'static> {
    f: fn() -> T,
}

impl<T: PartialEq + 'static> GlobalMemo<T> {
    pub const fn new(f: fn() -> T) -> Self {
        Self { f }
    }

    pub fn memo(&self) -> Memo<T> {
        let f = self.f;
        get_or(self as *const Self as usize, move || memo_now(f))
    }

    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.memo().get()
    }

    pub fn get_untracked(&self) -> T
    where
        T: Clone,
    {
        self.memo().get_untracked()
    }

    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.memo().with(f)
    }

    pub fn read(&self) -> ReadGuard<T> {
        self.memo().read()
    }
}

impl<T: PartialEq + std::fmt::Display + 'static> std::fmt::Display for GlobalMemo<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.memo().fmt(f)
    }
}

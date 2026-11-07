//! App-wide state in a `static`, one per app instance (per mounted
//! [`Root`](crate::Root), so per test): made on first use, under the root
//! (its context is the root's), ended with it.
use crate::reactive::{Memo, ReadGuard, Signal, WriteGuard, current_root, detached, memo_now, with_owner};
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// Each root's globals (by root, then by the static: its name, else its address).
type Globals = HashMap<(u32, Key), Rc<dyn Any>>;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    At(usize),
    Named(&'static str),
}

thread_local!(static GLOBALS: RefCell<Globals> = RefCell::default());

/// The current root's app instance ends: its globals are made again on their next use.
pub(crate) fn reset() {
    let root = current_root();
    let old: Vec<_> = GLOBALS.with_borrow_mut(|g| {
        let gone: Vec<_> = g.keys().filter(|k| k.0 == root).copied().collect();
        gone.into_iter().filter_map(|k| g.remove(&k)).collect()
    });
    drop(old);
}

fn get_or<T: Clone + 'static>(at: Key, make: impl FnOnce() -> T) -> T {
    let at = (current_root(), at);
    if let Some(v) = GLOBALS.with_borrow(|g| g.get(&at).cloned()) {
        if let Some(v) = v.downcast_ref::<T>() {
            return v.clone();
        }
        // Only a hot-patch gives one name two types (`ax dev` patched code
        // whose global's type changed): the old value can't be read as the
        // new type, so the global starts again from its first value. Any
        // other build has no hot-patch: two types under one name is a bug.
        if !cfg!(feature = "dev") {
            panic!("air: a global of another type");
        }
        eprintln!("[air] a global changed its type (hot-patched code): it starts again from its first value");
        GLOBALS.with_borrow_mut(|g| g.remove(&at));
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
    name: Option<&'static str>,
}

impl<T: 'static> Global<T> {
    /// One per `static`.
    pub const fn new(init: fn() -> T) -> Self {
        Self { init, name: None }
    }

    /// One per `name` (the facade's macros: `module_path!()::NAME`): the
    /// same global for the code a hot-patch brings in, whose `static` is
    /// a copy of its own.
    #[doc(hidden)]
    pub const fn named(init: fn() -> T, name: &'static str) -> Self {
        Self { init, name: Some(name) }
    }

    fn key(&self) -> Key {
        self.name.map_or(Key::At(self as *const Self as usize), Key::Named)
    }

    /// This app instance's signal.
    pub fn signal(&self) -> Signal<T> {
        get_or(self.key(), || Signal::new((self.init)()))
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
        get_or(Key::At(self as *const Self as usize), move || memo_now(f))
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

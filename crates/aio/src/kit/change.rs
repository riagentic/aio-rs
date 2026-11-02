//! A reaction to a change (aio's `onChange`): `f(new, previous)` whenever
//! what the selector gives changes. Only the selector is followed (`f`
//! runs untracked, so the state it reads while it works can't loop it),
//! it waits for a change (`use_on_change_now` runs once at the start too),
//! and what `f` returns may be a cleanup, run before the next change and
//! when the view goes (close the old camera, then open the new one).
use crate::ui::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// What a reaction may return: nothing, or its cleanup.
pub trait Cleanup {
    fn cleanup(self) -> Option<Box<dyn FnOnce()>>;
}

impl Cleanup for () {
    fn cleanup(self) -> Option<Box<dyn FnOnce()>> {
        None
    }
}

impl<F: FnOnce() + 'static> Cleanup for F {
    fn cleanup(self) -> Option<Box<dyn FnOnce()>> {
        Some(Box::new(self))
    }
}

struct Seen<T, F> {
    last: Option<T>,
    cleanup: Option<Box<dyn FnOnce()>>,
    f: F,
}

/// Run `f(new, previous)` each time `selector()` gives another value (a
/// hook); not at the start.
pub fn use_on_change<T, C>(selector: impl Fn() -> T + 'static, f: impl FnMut(&T, Option<&T>) -> C + 'static)
where
    T: Clone + PartialEq + 'static,
    C: Cleanup,
{
    on_change(selector, false, f);
}

/// [`use_on_change`], and once at the start with no previous value.
pub fn use_on_change_now<T, C>(selector: impl Fn() -> T + 'static, f: impl FnMut(&T, Option<&T>) -> C + 'static)
where
    T: Clone + PartialEq + 'static,
    C: Cleanup,
{
    on_change(selector, true, f);
}

fn on_change<T, C, F>(selector: impl Fn() -> T + 'static, now: bool, f: F)
where
    T: Clone + PartialEq + 'static,
    C: Cleanup,
    F: FnMut(&T, Option<&T>) -> C + 'static,
{
    let seen = once(|| Rc::new(RefCell::new(Seen { last: None::<T>, cleanup: None, f })));
    let ran = seen.clone();
    effect(move || {
        let next = selector();
        ::air::untrack(|| {
            let mut s = ran.borrow_mut();
            let first = s.last.is_none();
            if s.last.as_ref() == Some(&next) {
                return;
            }
            let prev = s.last.replace(next.clone());
            if first && !now {
                return;
            }
            if let Some(c) = s.cleanup.take() {
                c();
            }
            let out = (s.f)(&next, prev.as_ref());
            s.cleanup = out.cleanup();
        });
    });
    on_cleanup(move || {
        if let Some(c) = seen.borrow_mut().cleanup.take() {
            c();
        }
    });
}

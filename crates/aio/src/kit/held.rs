//! A held resource in a view (aio's `useResource` in a component): the one
//! for `key` held while the component lives, the old one let go (and its
//! close waited for) when the key changes.
use crate::held::{Held, Hold, State};
use crate::ui::prelude::*;
use Task;
use std::cell::RefCell;
use std::fmt::Debug;
use std::hash::Hash;
use std::rc::Rc;

/// Hold `key`'s resource from `pool` (a hook): `None` while there is no
/// key, else where it stands, as it changes (read it in a hole). A key
/// that follows state (a signal, a [`Prop`]) lets the old one go and holds
/// the new one; the last is let go at unmount.
pub fn use_held<K, T>(pool: &Held<K, T>, key: impl Into<Prop<Option<K>>>) -> Signal<Option<State<T>>>
where
    K: Clone + Eq + Hash + Debug + Send + Sync + 'static,
    T: Send + Sync + 'static,
{
    let (pool, key) = (pool.clone(), key.into());
    let out = signal(|| None::<State<T>>);
    let slot = once(|| Rc::new(RefCell::new(None::<(Hold<K, T>, Task)>)));
    {
        let drop_slot = slot.clone();
        on_cleanup(move || drop(drop_slot.borrow_mut().take()));
    }
    let hold = move |key: Option<K>| {
        let now = slot.borrow().as_ref().map(|(h, _)| h.key().clone());
        if now == key {
            return;
        }
        let after = slot.borrow_mut().take().map(|(h, task)| {
            task.cancel();
            h.release()
        });
        let Some(k) = key else { return *out.write() = None };
        let hold = pool.hold(k, after);
        let mut rx = hold.watch();
        *out.write() = Some(hold.state());
        let task = spawn(async move {
            while rx.changed().await.is_ok() {
                let s = rx.borrow_and_update().clone();
                if out.alive() {
                    *out.write() = Some(s);
                }
            }
        });
        *slot.borrow_mut() = Some((hold, task));
    };
    hold(key.get_untracked());
    if key.is_live() {
        effect(move || hold(key.get()));
    }
    out
}

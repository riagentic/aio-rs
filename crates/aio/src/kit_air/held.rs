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
/// key, else where it stands; the view re-renders as that changes. One
/// open across re-renders; let go at unmount.
pub fn use_held<K, T>(pool: &Held<K, T>, key: Option<K>) -> Option<State<T>>
where
    K: Clone + Eq + Hash + Debug + Send + Sync + 'static,
    T: Send + Sync + 'static,
{
    let mut seen = signal(|| 0u64);
    let slot = once(|| Rc::new(RefCell::new(None::<(Hold<K, T>, Task)>)));
    {
        let drop_slot = slot.clone();
        on_cleanup(move || drop(drop_slot.borrow_mut().take()));
    }
    let now = slot.borrow().as_ref().map(|(h, _)| h.key().clone());
    if now != key {
        let after = slot.borrow_mut().take().map(|(h, task)| {
            task.cancel();
            h.release()
        });
        if let Some(k) = key {
            let hold = pool.hold(k, after);
            let mut rx = hold.watch();
            let task = spawn(async move {
                while rx.changed().await.is_ok() {
                    seen += 1;
                }
            });
            *slot.borrow_mut() = Some((hold, task));
        }
    }
    _ = seen.get();
    slot.borrow().as_ref().map(|(h, _)| h.state())
}

//! The Android Back button in a view (aio's `onBackButton`): a handler per
//! component, asked while it is mounted, the latest first. Inert where no
//! Back button asks ([`crate::ui::back_pressed`]): the desktop, tests.
use dioxus::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// A mounted handler: its id, and the latest `f`.
type Handler = (u64, Callback<(), bool>);

/// The window's handlers, in the order they were mounted.
#[derive(Clone, Default)]
struct Stack(Rc<RefCell<Vec<Handler>>>);

/// Handle Back while this component lives (a hook): `f` returns `true`
/// when it took it (went up a screen, closed a dialog), `false` to let the
/// one mounted before it try; when none takes it the host does its default
/// (the page's history, then leave the app). The latest `f` runs.
pub fn use_back_button(mut f: impl FnMut() -> bool + 'static) {
    let run = use_callback(move |()| f());
    let stack = use_hook(|| {
        try_consume_context::<Stack>().unwrap_or_else(|| {
            let stack = dioxus::core::provide_root_context(Stack::default());
            let s = stack.clone();
            dioxus::core::spawn_forever(async move {
                let mut asks = crate::ui::back_presses();
                while let Some(ask) = asks.recv().await {
                    let hs: Vec<_> = s.0.borrow().iter().map(|(_, h)| *h).collect();
                    let calls: Vec<_> = hs.iter().map(|h| move || h.call(())).collect();
                    let refs: Vec<&dyn Fn() -> bool> = calls.iter().map(|c| c as &dyn Fn() -> bool).collect();
                    _ = ask.send(crate::ui::back_taken(&refs));
                }
            });
            stack
        })
    });
    let id = use_hook(|| {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        stack.0.borrow_mut().push((id, run));
        id
    });
    use_drop(move || stack.0.borrow_mut().retain(|(i, _)| *i != id));
}

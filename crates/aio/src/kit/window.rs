//! Window-level events (aio's `onWindowEvent`): a listener per component,
//! removed with it, that hears what reaches the app root
//! ([`GlobalKeys`](super::GlobalKeys)): every pointer move, press and key
//! anywhere in the window that no handler stopped on its way up, and the
//! window's size. The latest `f` runs, so it sees the latest render.
use crate::ui::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// What reached the window.
#[derive(Clone, Debug)]
pub enum WindowEvent {
    /// The pointer moved, wherever it is: at most once per
    /// [`MOVE_MS`](super::MOVE_MS), the last move too.
    MouseMove(MouseEvent),
    MouseDown(MouseEvent),
    MouseUp(MouseEvent),
    Click(MouseEvent),
    Wheel(WheelEvent),
    /// One no global key binding took.
    KeyDown(KeyboardEvent),
    KeyUp(KeyboardEvent),
    /// The window's new size in px (its first, too, once laid out).
    Resize {
        width: f64,
        height: f64,
    },
}

/// A listener: its id, and the latest `f`.
type Listener = (u64, Callback<WindowEvent>);

/// The window's listeners.
#[derive(Clone, Default)]
pub(super) struct Listeners {
    all: Rc<RefCell<Vec<Listener>>>,
    next: Rc<Cell<u64>>,
}

impl Listeners {
    pub(super) fn tell(&self, e: WindowEvent) {
        let all: Vec<_> = self.all.borrow().iter().map(|(_, f)| *f).collect();
        for f in all {
            f.call(e.clone());
        }
    }
}

/// Hear the window's events while this component lives (a hook). Needs a
/// [`GlobalKeys`](super::GlobalKeys) around the app.
pub fn use_window_event(f: impl FnMut(WindowEvent) + 'static) {
    let run = callback(f);
    let ls = try_context::<Listeners>().expect("use_window_event needs a GlobalKeys around the app");
    let id = once(|| {
        let id = ls.next.get();
        ls.next.set(id + 1);
        ls.all.borrow_mut().push((id, run));
        id
    });
    on_cleanup(move || ls.all.borrow_mut().retain(|(i, _)| *i != id));
}

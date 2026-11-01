//! Portals (aio's `Portal`): content drawn at the end of the app root (a
//! [`PortalHost`] around the app), out of every ancestor's `overflow`,
//! `transform` and stacking order, while its state and handlers stay with
//! the component that made it. Portals show in the order they mounted, so
//! a menu opened inside a portalled dialog is drawn over it, and never
//! inside it. With no host, or `disabled`, the content stays in place.
use crate::ui::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// The host's portals, by id in mount order, and how to redraw it.
#[derive(Clone)]
struct Outlet {
    all: Rc<RefCell<Vec<(u64, View)>>>,
    next: Rc<Cell<u64>>,
    host: std::sync::Arc<dyn Fn() + Send + Sync>,
}

/// Where portals are drawn: put it around the app once.
#[component]
pub fn PortalHost(children: View) -> View {
    let out = provide(|| Outlet { all: Default::default(), next: Default::default(), host: crate::ui::redraw() });
    let shown: Vec<(u64, View)> = out.all.borrow().clone();
    rsx! {
        {children}
        div { class: "aio-portals",
            for (id, el) in shown {
                Fragment { key: "{id}", {el} }
            }
        }
    }
}

/// Draw `children` in the [`PortalHost`] while this component lives.
#[component]
pub fn Portal(#[props(default)] disabled: bool, children: View) -> View {
    let host = try_context::<Outlet>();
    let id = once(|| Rc::new(Cell::new(None::<u64>)));
    {
        let (host, id) = (host.clone(), id.clone());
        on_cleanup(move || {
            if let (Some(h), Some(i)) = (host, id.get()) {
                take(&h, i)
            }
        });
    }
    let Some(out) = host.filter(|_| !disabled) else {
        if let (Some(h), Some(i)) = (try_context::<Outlet>(), id.take()) {
            take(&h, i);
        }
        return children;
    };
    let i = id.get().unwrap_or_else(|| {
        let i = out.next.get();
        out.next.set(i + 1);
        id.set(Some(i));
        i
    });
    {
        let mut all = out.all.borrow_mut();
        match all.iter_mut().find(|(k, _)| *k == i) {
            Some(slot) => slot.1 = children,
            None => all.push((i, children)),
        }
    }
    (out.host)();
    rsx! {}
}

/// Portal `id` leaves the host.
fn take(out: &Outlet, id: u64) {
    out.all.borrow_mut().retain(|(k, _)| *k != id);
    (out.host)();
}

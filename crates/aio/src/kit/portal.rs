//! Portals (aio's `Portal`): content drawn at the end of the app root (a
//! [`PortalHost`] around the app), out of every ancestor's `overflow`,
//! `transform` and stacking order, while its state and handlers stay with
//! the component that made it. Portals show in the order they mounted, so
//! a menu opened inside a portalled dialog is drawn over it, and never
//! inside it. With no host, or `disabled`, the content stays in place.
use dioxus::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// The host's portals, by id in mount order, and how to redraw it.
#[derive(Clone)]
struct Outlet {
    all: Rc<RefCell<Vec<(u64, Element)>>>,
    next: Rc<Cell<u64>>,
    host: ScopeId,
}

/// Where portals are drawn: put it around the app once.
#[component]
pub fn PortalHost(children: Element) -> Element {
    let out = use_context_provider(|| Outlet {
        all: Default::default(),
        next: Default::default(),
        host: dioxus::core::current_scope_id(),
    });
    let shown: Vec<(u64, Element)> = out.all.borrow().clone();
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
pub fn Portal(#[props(default)] disabled: bool, children: Element) -> Element {
    let host = try_use_context::<Outlet>();
    let id = use_hook(|| Rc::new(Cell::new(None::<u64>)));
    {
        let (host, id) = (host.clone(), id.clone());
        use_drop(move || {
            if let (Some(h), Some(i)) = (host, id.get()) {
                take(&h, i)
            }
        });
    }
    let Some(out) = host.filter(|_| !disabled) else {
        if let (Some(h), Some(i)) = (try_consume_context::<Outlet>(), id.take()) {
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
    dioxus::core::needs_update_any(out.host);
    rsx! {}
}

/// Portal `id` leaves the host.
fn take(out: &Outlet, id: u64) {
    out.all.borrow_mut().retain(|(k, _)| *k != id);
    dioxus::core::needs_update_any(out.host);
}

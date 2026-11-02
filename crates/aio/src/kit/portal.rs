//! Portals (aio's `Portal`): content drawn at the end of the app root (a
//! [`PortalHost`] around the app), out of every ancestor's `overflow`,
//! `transform` and stacking order, while its state and handlers stay with
//! the component that made it. Portals show in the order they mounted, so
//! a menu opened inside a portalled dialog is drawn over it, and never
//! inside it. With no host, or `disabled`, the content stays in place.
use crate::ui::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

/// The host's portals, by id in mount order.
#[derive(Clone, Copy)]
struct Outlet {
    all: Signal<Vec<(u64, View)>>,
}

thread_local!(static NEXT: Cell<u64> = const { Cell::new(0) });

/// Where portals are drawn: put it around the app once.
#[component]
pub fn PortalHost(children: View) -> View {
    let out = provide(|| Outlet { all: Signal::new(vec![]) });
    rsx! {
        {children}
        div { class: "aio-portals",
            for (id, el) in out.all.get() {
                Fragment { key: id, {el} }
            }
        }
    }
}

/// Draw `children` in the [`PortalHost`] while this component lives (and
/// isn't `disabled`).
#[component]
pub fn Portal(#[props(default)] disabled: Prop<bool>, children: View) -> View {
    let Some(out) = try_context::<Outlet>() else { return children };
    let id = NEXT.with(|n| n.replace(n.get() + 1));
    let here = memo(move || disabled.get());
    let placed = once(|| Rc::new(Cell::new(false)));
    {
        let (children, placed) = (children.clone(), placed.clone());
        effect(move || {
            let away = !here.get();
            if away != placed.get() {
                placed.set(away);
                if away {
                    out.all.write().push((id, children.clone()));
                } else {
                    out.all.write().retain(|(k, _)| *k != id);
                }
            }
        });
    }
    on_cleanup(move || {
        if out.all.alive() {
            out.all.write().retain(|(k, _)| *k != id);
        }
    });
    rsx! {
        if here.get() {
            {children.clone()}
        }
    }
}

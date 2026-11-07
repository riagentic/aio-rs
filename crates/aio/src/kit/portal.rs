//! Portals (aio's `Portal`): content drawn at the end of the app root (a
//! [`PortalHost`] around the app), out of every ancestor's `overflow`,
//! `transform` and stacking order, while its state and handlers stay with
//! the component that made it. Portals show in the order they mounted, so
//! a menu opened inside a portalled dialog is drawn over it, and never
//! inside it. With no host, or `disabled`, the content stays in place. A
//! host with a `name` anywhere on the page takes the portals that name it
//! as their `target` (aio's `Portal target`).
use crate::ui::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

/// The host's portals, by id in mount order.
#[derive(Clone, Copy)]
struct Outlet {
    all: Signal<Vec<(u64, View)>>,
}

thread_local! {
    static NEXT: Cell<u64> = const { Cell::new(0) };
    /// The named hosts showing now.
    static NAMED: std::cell::RefCell<Vec<(String, Outlet)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Where portals are drawn: put it around the app once; one with a `name`
/// (a sidebar, a status bar) takes the portals whose `target` names it.
#[component]
pub fn PortalHost(#[props(into)] name: Option<String>, children: View) -> View {
    let out = provide(|| Outlet { all: Signal::new(vec![]) });
    if let Some(name) = name {
        once(|| NAMED.with_borrow_mut(|n| n.push((name.clone(), out))));
        on_cleanup(move || NAMED.with_borrow_mut(|n| n.retain(|(k, o)| *k != name || o.all != out.all)));
    }
    rsx! {
        {children}
        div { class: "aio-portals",
            for (id, el) in out.all.get() {
                Fragment { key: id, {el} }
            }
        }
    }
}

/// Draw `children` in the [`PortalHost`] around it, or the one named
/// `target`, while this component lives (and isn't `disabled`).
#[component]
pub fn Portal(#[props(default)] disabled: Prop<bool>, #[props(into)] target: Option<String>, children: View) -> View {
    let named = |t: &String| NAMED.with_borrow(|n| n.iter().rev().find(|(k, _)| k == t).map(|(_, o)| *o));
    let Some(out) = target.as_ref().map_or_else(try_context::<Outlet>, named) else { return children };
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

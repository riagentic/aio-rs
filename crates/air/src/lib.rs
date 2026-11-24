//! air: aio-rs's fine-grained renderer. A view's body runs once: it
//! makes its state and returns a tree; the tree, when placed, clones its
//! templates in the page and wires its holes. Each hole subscribes to
//! exactly what it reads and writes its own DOM node when that changes: no
//! re-run, no VDOM, no diff. Design and numbers: `docs/design/air.md`.
//! Apps use it through `aio::ui::prelude` (feature `air` of aio-rs).
//!
//! ```
//! use air::{View, rsx, signal};
//!
//! fn counter() -> View {
//!     let count = signal(|| 0);
//!     rsx! {
//!         h1 { "data-t": "count", "{count}" }
//!         button { "data-t": "add", onclick: move |_| count += 1, "Add" }
//!     }
//! }
//!
//! let mut page = air::testing::TestPage::mount(counter);
//! page.click("add");
//! assert_eq!(page.text("count"), "1");
//! ```
//!
//! A row of a `for` needs its `key:` (a compile error without one):
//!
//! ```compile_fail
//! use air::{View, rsx};
//! fn list() -> View {
//!     rsx! { ul { for n in 0..3 { li { "{n}" } } } }
//! }
//! ```
//!
//! and so is a `while` or `loop` (a view says what shows: a `for` over the items):
//!
//! ```compile_fail
//! use air::{View, rsx};
//! fn count(mut n: u32) -> View {
//!     rsx! { while n > 0 { p { "{n}" } } }
//! }
//! ```
//! A required prop left out is a compile error, naming it:
//!
//! ```compile_fail
//! use air::{View, component, rsx};
//! #[component]
//! fn Hello(name: String) -> View { rsx! { p { "{name}" } } }
//! fn app() -> View { rsx! { Hello {} } }
//! ```
//!
//! and so is an attribute given to a component that takes none:
//!
//! ```compile_fail
//! use air::{View, component, rsx};
//! #[component]
//! fn Hello(name: String) -> View { rsx! { p { "{name}" } } }
//! fn app() -> View { rsx! { Hello { name: "x", "data-t": "h" } } }
//! ```
//!
//! `#[component]` goes on a fn, and `#[derive(Props)]` on a struct with
//! named fields:
//!
//! ```compile_fail
//! use air::component;
//! #[component]
//! struct Hello { name: String }
//! ```
//!
//! ```compile_fail
//! use air::Props;
//! #[derive(Clone, Props)]
//! struct HelloProps(String);
//! ```
//!
//! ```
//! use air::{Props, View, component, rsx};
//! #[component]
//! fn Hello(name: String) -> View { rsx! { p { "{name}" } } }
//! #[derive(Clone, Props)]
//! struct GreetProps { name: String }
//! ```
extern crate self as air;

mod dom;
mod event;
mod global;
mod h;
pub mod hot;
mod props;
mod reactive;
pub mod ssr;
mod task;
pub mod testing;
mod tree;

pub use air_macro::{Props, component, rsx};
pub use dom::{
    AttrValue, Child, ErrorBoundary, ErrorBoundaryProps, Eval, Fragment, FragmentProps, IntoAttr, IntoChild,
    IntoHandler, MountedData, Retry, Root, Suspense, SuspenseProps, Tree, View, ViewError, eval, exec, is_empty,
    longest_increasing, retry, styled, take_edits, text_view,
};
pub use event::*;
pub use global::{Global, GlobalMemo, TrackedMemo};
pub use h::{H, comp, h};
pub use props::{Attribute, IntoOpt, IntoOptView, IntoOptViewValue, IntoValue, IntoView, IntoViewValue, Prop, Props};
pub use reactive::{
    Callback, Effect, EventHandler, Memo, Optimistic, Owner, ReadGuard, Selector, Signal, WriteGuard, batch, callback,
    context, current_owner, detached, drop_left, effect, effect_quiet, follow, in_body, in_views, memo, on_cleanup,
    once, parent_context, provide, provide_now, provide_root, scope, selector, signal, try_context, untrack,
    use_optimistic, watch, with_owner,
};
pub use task::{Resource, Task, live_tasks, on_wake, resource, run_tasks, seeded_resource, spawn, spawn_forever, task};
pub use tree::{Mounted, RENDERS_KEPT, Render, components, renders};

#[cfg(feature = "fault")]
pub use dom::__fault;

/// What a component is (`TestUi::with` mounts one): a function of its props.
pub trait Component<P, M = ()>: 'static {
    fn call(&self, props: P) -> View;
}

impl<P, F: Fn(P) -> View + 'static> Component<P> for F {
    fn call(&self, props: P) -> View {
        self(props)
    }
}

/// The marker of a component that takes no props.
pub struct NoProps;

impl<F: Fn() -> View + 'static> Component<(), NoProps> for F {
    fn call(&self, _: ()) -> View {
        self()
    }
}

/// What is alive on this thread: a leak shows here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Live {
    /// Signals, effects, holes, memos, owners, callbacks.
    pub nodes: usize,
    /// Of them, the ones that run (effects, holes, memos).
    pub effects: usize,
    /// Subscriptions that still reach a running effect.
    pub subscriptions: usize,
    /// Cleanups waiting for their owner.
    pub cleanups: usize,
    /// Event handlers the page's nodes have.
    pub handlers: usize,
    /// Tasks not ended.
    pub tasks: usize,
    /// Page scripts still answered.
    pub scripts: usize,
}

/// What is alive on this thread now.
pub fn live() -> Live {
    let r = reactive::live();
    let (handlers, scripts) = dom::live_dom();
    Live {
        nodes: r.nodes,
        effects: r.effects,
        subscriptions: r.subscriptions,
        cleanups: r.cleanups,
        handlers,
        tasks: live_tasks(),
        scripts,
    }
}

type Warn = std::sync::Arc<dyn Fn(&str, &str) + Send + Sync>;
static WARN: std::sync::Mutex<Option<Warn>> = std::sync::Mutex::new(None);

/// Where air's dev-build warnings go (`kind`, message): aio-rs puts them on
/// its diagnostic bus; on stderr until a host sets one.
pub fn on_warn(f: impl Fn(&str, &str) + Send + Sync + 'static) {
    *WARN.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::sync::Arc::new(f));
}

/// `v` sorted by `cmp`, stably, through the one sort every caller shares (a
/// sort of `u32` positions): each element type's own sort is ~3 KB of a web
/// build's code.
pub(crate) fn sort_by<T>(v: &mut Vec<T>, cmp: impl Fn(&T, &T) -> std::cmp::Ordering) {
    if v.len() < 2 {
        return;
    }
    let mut order: Vec<u32> = (0..v.len() as u32).collect();
    let by: &mut dyn FnMut(&u32, &u32) -> std::cmp::Ordering = &mut |a, b| cmp(&v[*a as usize], &v[*b as usize]);
    order.sort_by(by);
    let mut old: Vec<Option<T>> = std::mem::take(v).into_iter().map(Some).collect();
    v.extend(order.iter().filter_map(|&i| old[i as usize].take()));
}

pub(crate) fn warn(kind: &str, msg: &str) {
    let hook = WARN.lock().unwrap_or_else(|e| e.into_inner()).clone();
    match hook {
        Some(f) => f(kind, msg),
        None => eprintln!("WARN  [air] {kind}: {msg}"),
    }
}

/// The page's interpreter (after `<div id="main">`, with `window.__aioWire`
/// as the shell's preload gives it).
pub const PAGE_JS: &str = include_str!("page.js");

/// What `rsx!` expands to calls; not for apps.
#[doc(hidden)]
pub mod __rt {
    /// air's `dev` feature (`ax dev`'s builds: markup sent live, renders
    /// logged), as the app's expanded templates see it.
    pub const DEV: bool = cfg!(feature = "dev");
    pub use crate::dom::{
        AttrSlot, Range, Template, attr, attr_of, branch, child, comp, each, handler, handler_fn, mounted, mounted_fn,
        on, show, text, text_of, tpl, view,
    };
    pub use crate::event::{
        DragData, FormData, KeyboardData, MouseData, PlainData, PointerData, ResizeData, ScrollData, VisibleData,
        WheelData,
    };
    pub use crate::hot::At;
    pub use crate::props::{
        ByMemo, ByOpt, ByProp, BySignal, ByText, ByVal, ByView, CallbackOf, GlobalAttributeNames, GlobalAttributes,
        IntoOpt, IntoOptView, IntoOptViewValue, IntoValue, IntoView, IntoViewValue, NoPropsBuilder, Props, Set,
        builder_of, spread, with_attributes,
    };
    pub use crate::tree::{KeyByDebug, KeyByDisplay, KeyByNone, KeyOf};
}

#[cfg(test)]
mod sort_tests {
    #[test]
    fn sort_by_is_stable_and_keeps_every_element() {
        let mut v = vec![(2, 'a'), (1, 'b'), (2, 'c'), (0, 'd'), (1, 'e')];
        super::sort_by(&mut v, |a, b| a.0.cmp(&b.0));
        assert_eq!(v, [(0, 'd'), (1, 'b'), (1, 'e'), (2, 'a'), (2, 'c')]);
        let mut one = vec![String::from("x")];
        super::sort_by(&mut one, Ord::cmp);
        assert_eq!(one, ["x"]);
    }
}

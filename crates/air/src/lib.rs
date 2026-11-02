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
//! and `if let` is refused (make a bool, or a `{child}` of an `Option`):
//!
//! ```compile_fail
//! use air::{View, rsx};
//! fn maybe(n: Option<u32>) -> View {
//!     rsx! { if let Some(n) = n { p { "{n}" } } }
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
extern crate self as air;

mod dom;
mod event;
mod global;
mod props;
mod reactive;
mod task;
pub mod testing;
mod tree;

pub use air_macro::{Props, component, rsx};
pub use dom::{
    AttrValue, Child, ErrorBoundary, ErrorBoundaryProps, Eval, Fragment, FragmentProps, IntoAttr, IntoChild,
    IntoHandler, MountedData, Root, Suspense, SuspenseProps, Tree, View, ViewError, eval, is_empty, longest_increasing,
    take_edits,
};
pub use event::*;
pub use global::{Global, GlobalMemo};
pub use props::{Attribute, IntoOpt, IntoValue, Prop, Props};
pub use reactive::{
    Callback, EventHandler, Memo, Owner, ReadGuard, Selector, Signal, WriteGuard, batch, callback, context,
    current_owner, detached, effect, follow, in_body, in_views, memo, on_cleanup, once, parent_context, provide,
    provide_now, provide_root, scope, selector, signal, try_context, untrack, watch, with_owner,
};
pub use task::{Resource, Task, live_tasks, on_wake, resource, run_tasks, spawn, spawn_forever, task};
pub use tree::{Mounted, components};

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

/// The page's interpreter (after `<div id="main">`, with `window.__aioWire`
/// as the shell's preload gives it).
pub const PAGE_JS: &str = include_str!("page.js");

/// What `rsx!` expands to calls; not for apps.
#[doc(hidden)]
pub mod __rt {
    pub use crate::dom::{
        Range, Template, attr, child, comp, each, handler, handler_fn, mounted, mounted_fn, on, show, text, tpl, view,
    };
    pub use crate::event::{
        DragData, FormData, KeyboardData, MouseData, PlainData, PointerData, ScrollData, WheelData,
    };
    pub use crate::tree::{KeyByDebug, KeyByDisplay, KeyByNone, KeyOf};
    pub use crate::props::{
        ByMemo, ByOpt, ByProp, BySignal, ByVal, CallbackOf, GlobalAttributeNames, GlobalAttributes, IntoOpt, IntoValue,
        NoPropsBuilder, Props, Set, builder_of, spread,
    };
}

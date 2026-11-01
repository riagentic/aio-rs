//! The view API apps and the kit are written against (`aio::ui::prelude`):
//! one surface whatever draws the page. Today it is Dioxus 0.7 underneath;
//! air (docs/design/air.md) takes its place without an app migration, so
//! each name here has air's shape: a view's local state is a `Copy`
//! [`Signal`] read with `.get()`, a value made of state is a [`memo`], work
//! that follows state is an [`effect`], and a view's one-time setup is
//! [`once`].
//!
//! The functions that keep something for a view ([`signal`], [`memo`],
//! [`effect`], [`watch`], [`once`], [`provide`], [`callback`], [`task`],
//! [`resource`], [`on_cleanup`]) are called at a view's top level,
//! unconditionally and in the same order each time (Dioxus's hook rule; air
//! takes the same code, a view running once). Everything else ([`spawn`],
//! [`context`], a signal's methods) works anywhere: in a view, a handler,
//! an effect.

use __rsx::*;
use dioxus::prelude as dx;
use dioxus::prelude::{Key, KeyboardEvent, Modifiers};
use dx::{Readable as _, ReadableExt as _, Writable as _, WritableExt as _};
use std::ops::{Deref, DerefMut};

pub use dioxus::dioxus_core::{Task, spawn, spawn_forever};
pub use dioxus::prelude::{Attribute, Callback, EventHandler, Fragment, Props, component, rsx};

/// A component (`#[component] fn Card(…) -> View`), as `TestUi::with` mounts one.
pub use dioxus::dioxus_core::ComponentFunction as Component;

/// What a view returns: an element tree, made by [`rsx!`]. A `Result`, so a
/// view may `?` an error to the nearest [`ErrorBoundary`] and a
/// [`Resource::suspend`] to the nearest [`Suspense`].
pub type View = dioxus::prelude::Element;

/// Why a view stopped: an error thrown to an [`ErrorBoundary`], or a
/// resource still loading ([`Resource::suspend`]).
pub type ViewError = dioxus::prelude::RenderError;

/// A value that tells the views and effects that read it when it changes.
/// `Copy`: a handle, freed with the view that made it ([`signal`]), so a
/// closure takes it without a clone. Reads in a view or an effect subscribe
/// it (`get`, `with`, `read`, `"{count}"`); the `_untracked` reads don't.
pub struct Signal<T: 'static>(dx::Signal<T>);

impl<T> Clone for Signal<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Signal<T> {}

impl<T> PartialEq for Signal<T> {
    /// The same signal (not the same value): a prop that is a signal never
    /// re-renders its view by itself.
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<T: 'static> Signal<T> {
    /// A signal owned by the current view, or by the app when outside one.
    /// In a view's body use [`signal`] (made once, not on every render).
    #[track_caller]
    pub fn new(v: T) -> Self {
        Self(dx::Signal::new(v))
    }

    /// Is its value still there? A signal ends with the view that made it;
    /// work that may outlive the view (a task, a reply) asks before it
    /// writes. Any other read or write of an ended signal panics.
    pub fn alive(&self) -> bool {
        !matches!(self.0.try_peek(), Err(dx::BorrowError::Dropped(_)))
    }

    /// The value, cloned; subscribes.
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.0.cloned()
    }

    /// The value, cloned, subscribing nothing.
    pub fn get_untracked(&self) -> T
    where
        T: Clone,
    {
        self.0.peek_unchecked().clone()
    }

    /// `f` of the value; subscribes.
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        f(&self.0.read_unchecked())
    }

    /// `f` of the value, subscribing nothing.
    pub fn with_untracked<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        f(&self.0.peek_unchecked())
    }

    /// The value, borrowed until the guard drops; subscribes. A write while
    /// it is held panics.
    pub fn read(&self) -> impl Deref<Target = T> + use<T> {
        self.0.read_unchecked()
    }

    /// The value, borrowed, subscribing nothing.
    pub fn read_untracked(&self) -> impl Deref<Target = T> + use<T> {
        self.0.peek_unchecked()
    }

    /// The new value; readers are told only if it differs (aio's AIR
    /// `set`): setting what is there already re-renders nothing.
    pub fn set(&self, v: T)
    where
        T: PartialEq,
    {
        if *self.0.peek_unchecked() != v {
            *self.0.write_unchecked() = v;
        }
    }

    /// Change the value in place; readers are always told.
    pub fn update<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        f(&mut self.0.write_unchecked())
    }

    /// The value, mutably borrowed; readers are told when the guard drops.
    pub fn write(&self) -> impl DerefMut<Target = T> + use<T> {
        self.0.write_unchecked()
    }

    /// [`write`](Self::write), or `None` once the signal ended (or while it
    /// is borrowed): for work that may outlive its view.
    pub fn try_write(&self) -> Option<impl DerefMut<Target = T> + use<T>> {
        self.0.try_write_unchecked().ok()
    }

    /// [`read_untracked`](Self::read_untracked), or `None` once the signal
    /// ended (or while it is written).
    pub fn try_read_untracked(&self) -> Option<impl Deref<Target = T> + use<T>> {
        self.0.try_peek_unchecked().ok()
    }
}

impl Signal<bool> {
    /// Flip it; readers are told.
    pub fn toggle(&self) {
        self.update(|v| *v = !*v)
    }
}

impl<T: std::fmt::Display + 'static> std::fmt::Display for Signal<T> {
    /// Subscribes: `"{count}"` in a view follows `count`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.with(|v| v.fmt(f))
    }
}

impl<T: std::fmt::Debug + 'static> std::fmt::Debug for Signal<T> {
    /// Subscribes too: `"{pick:?}"` in a view follows `pick`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.with(|v| v.fmt(f))
    }
}

impl<T: Clone + std::ops::Add<Output = T> + 'static> std::ops::AddAssign<T> for Signal<T> {
    /// `count += 1` in a handler.
    fn add_assign(&mut self, by: T) {
        self.update(|v| *v = v.clone() + by)
    }
}

impl<T: Clone + std::ops::Sub<Output = T> + 'static> std::ops::SubAssign<T> for Signal<T> {
    fn sub_assign(&mut self, by: T) {
        self.update(|v| *v = v.clone() - by)
    }
}

impl<T: Clone + dioxus_core::IntoAttributeValue + 'static> dioxus_core::IntoAttributeValue for Signal<T> {
    /// `value: name` in `rsx!`: the attribute follows the signal.
    fn into_value(self) -> dioxus_core::AttributeValue {
        self.get().into_value()
    }
}

/// A view's own state, made the first time the view runs: `let n = signal(|| 0);`.
#[track_caller]
pub fn signal<T: 'static>(init: impl FnOnce() -> T) -> Signal<T> {
    Signal(dx::use_signal(init))
}

/// A value made of signals and cell parts, made again when one it read
/// changes; its readers are told only when the value differs.
pub struct Memo<T: 'static>(dx::Memo<T>);

impl<T> Clone for Memo<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Memo<T> {}

impl<T> PartialEq for Memo<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<T: PartialEq + 'static> Memo<T> {
    /// The value, cloned; subscribes.
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.0.cloned()
    }

    pub fn get_untracked(&self) -> T
    where
        T: Clone,
    {
        self.0.peek_unchecked().clone()
    }

    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        f(&self.0.read_unchecked())
    }

    pub fn with_untracked<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        f(&self.0.peek_unchecked())
    }

    pub fn read(&self) -> impl Deref<Target = T> + use<T> {
        self.0.read_unchecked()
    }

    pub fn read_untracked(&self) -> impl Deref<Target = T> + use<T> {
        self.0.peek_unchecked()
    }
}

impl<T: PartialEq + std::fmt::Display + 'static> std::fmt::Display for Memo<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.with(|v| v.fmt(f))
    }
}

impl<T: PartialEq + std::fmt::Debug + 'static> std::fmt::Debug for Memo<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.with(|v| v.fmt(f))
    }
}

/// `f`'s value, made again when what it read changes.
#[track_caller]
pub fn memo<T: PartialEq + 'static>(f: impl FnMut() -> T + 'static) -> Memo<T> {
    Memo(dx::use_memo(f))
}

/// Run `f` after the view is drawn, and again whenever what it read changes.
#[track_caller]
pub fn effect(f: impl FnMut() + 'static) {
    dx::use_effect(f);
}

/// Run `f(deps)` after the view is drawn, and again when `deps` (values the
/// view made, e.g. of its props) differ from the last run's. On air a view
/// runs once, so `deps` there come from signals or cell parts.
#[track_caller]
pub fn watch<D: Clone + PartialEq + 'static>(deps: D, mut f: impl FnMut(D) + 'static) {
    let mut last = dx::use_signal(|| deps.clone());
    if *last.peek() != deps {
        last.set(deps);
    }
    dx::use_effect(move || f(last()));
}

/// `f`'s value, made the first time the view runs and kept for its life
/// (a channel, a counter, a handle); later runs get a clone.
#[track_caller]
pub fn once<T: Clone + 'static>(f: impl FnOnce() -> T) -> T {
    dx::use_hook(f)
}

/// Run `f` when the view goes away.
#[track_caller]
pub fn on_cleanup(f: impl FnOnce() + 'static) {
    dx::use_drop(f)
}

/// A handler made once, so a prop that takes it doesn't change on every render.
#[track_caller]
pub fn callback<A: 'static, R: 'static>(f: impl FnMut(A) -> R + 'static) -> Callback<A, R> {
    dx::use_callback(f)
}

/// Give the views below this one `f()`, made once; they read it with
/// [`context`]. Returns it.
#[track_caller]
pub fn provide<T: Clone + 'static>(f: impl FnOnce() -> T) -> T {
    dx::use_context_provider(f)
}

/// What a view above provided ([`provide`]); panics if none did.
#[track_caller]
pub fn context<T: Clone + 'static>() -> T {
    dx::consume_context()
}

/// Give every view of the app `v` (the app's root context), now: for a
/// shared service a component sets up on first use.
pub fn provide_root<T: Clone + 'static>(v: T) -> T {
    dioxus_core::provide_root_context(v)
}

/// What a view above provided, if one did.
pub fn try_context<T: Clone + 'static>() -> Option<T> {
    dx::try_consume_context()
}

/// Work for the view's life: `f()`'s future starts when the view is drawn
/// and is cancelled when it goes away; what it ends with is dropped. (In a
/// handler, [`spawn`].)
#[track_caller]
pub fn task<F: std::future::Future + 'static>(mut f: impl FnMut() -> F + 'static) {
    dx::use_future(move || {
        let run = f();
        async move { _ = run.await }
    });
}

/// A value loaded by a future: `f()` runs when the view is drawn and again
/// when a signal it read before its first `.await` changes.
pub struct Resource<T: 'static>(dx::Resource<T>);

impl<T> Clone for Resource<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Resource<T> {}

impl<T> PartialEq for Resource<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<T: 'static> Resource<T> {
    /// The value once loaded, cloned; subscribes.
    pub fn get(&self) -> Option<T>
    where
        T: Clone,
    {
        (*self.0.read_unchecked()).clone()
    }

    pub fn with<R>(&self, f: impl FnOnce(Option<&T>) -> R) -> R {
        f(self.0.read_unchecked().as_ref())
    }

    /// Is it loading?
    pub fn pending(&self) -> bool {
        self.0.pending()
    }

    /// Load it again.
    pub fn restart(&self) {
        let mut r = self.0;
        r.restart()
    }

    /// The value, or the view stops here until it is loaded and the
    /// nearest [`Suspense`] shows its fallback: `let v = res.suspend()?;`.
    pub fn suspend(&self) -> Result<T, ViewError>
    where
        T: Clone,
    {
        self.0.suspend().map(|v| v.cloned())
    }
}

#[track_caller]
pub fn resource<T: 'static, F: std::future::Future<Output = T> + 'static>(
    f: impl FnMut() -> F + 'static,
) -> Resource<T> {
    Resource(dx::use_resource(f))
}

/// App-wide state in a `static`, made on first use per app instance (so per
/// test): `static THEME: Global<String> = Global::new(|| "dark".into());`.
pub struct Global<T: 'static>(dx::GlobalSignal<T>);

impl<T: 'static> Global<T> {
    pub const fn new(init: fn() -> T) -> Self {
        Self(dx::GlobalSignal::new(init))
    }

    /// One keyed by `name`, not by where it is written (a macro's globals
    /// all have the macro's place).
    #[doc(hidden)]
    pub const fn named(init: fn() -> T, name: &'static str) -> Self {
        Self(dx::GlobalSignal::with_name(init, name))
    }

    /// This app instance's signal.
    pub fn signal(&self) -> Signal<T> {
        Signal(self.0.signal())
    }

    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.signal().get()
    }

    pub fn get_untracked(&self) -> T
    where
        T: Clone,
    {
        self.signal().get_untracked()
    }

    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.signal().with(f)
    }

    pub fn with_untracked<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.signal().with_untracked(f)
    }

    pub fn read(&self) -> impl Deref<Target = T> + use<T> {
        self.signal().read()
    }

    pub fn read_untracked(&self) -> impl Deref<Target = T> + use<T> {
        self.signal().read_untracked()
    }

    pub fn set(&self, v: T)
    where
        T: PartialEq,
    {
        self.signal().set(v)
    }

    pub fn update<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        self.signal().update(f)
    }

    pub fn write(&self) -> impl DerefMut<Target = T> + use<T> {
        self.signal().write()
    }
}

impl<T: std::fmt::Display + 'static> std::fmt::Display for Global<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.signal().fmt(f)
    }
}

/// A [`memo`] in a `static`, one per app instance: `static N: GlobalMemo<usize> = GlobalMemo::new(|| rows().len());`.
pub struct GlobalMemo<T: PartialEq + 'static>(dx::GlobalMemo<T>);

impl<T: PartialEq + 'static> GlobalMemo<T> {
    pub const fn new(f: fn() -> T) -> Self {
        Self(dx::Memo::global(f))
    }

    fn memo(&self) -> Memo<T> {
        Memo(self.0.memo())
    }

    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.memo().get()
    }

    pub fn get_untracked(&self) -> T
    where
        T: Clone,
    {
        self.memo().get_untracked()
    }

    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.memo().with(f)
    }

    pub fn read(&self) -> impl Deref<Target = T> + use<T> {
        self.memo().read()
    }
}

/// Are the app's views running on this thread (in a view, a handler, an
/// effect or a task of theirs)? Not in a cell method or a test's own code.
pub fn in_views() -> bool {
    dioxus_core::Runtime::try_current().is_some()
}

/// A key press as the views get one, for a key heard elsewhere (a guest
/// page, the host) and handed to the views' handlers.
pub fn key_event(key: Key, modifiers: Modifiers, repeat: bool) -> KeyboardEvent {
    use dioxus::html::{Code, KeyboardData, Location, SerializedKeyboardData};
    let data = SerializedKeyboardData::new(key, Code::Unidentified, Location::Standard, repeat, modifiers, false);
    dioxus_core::Event::new(std::rc::Rc::new(KeyboardData::new(data)), true)
}

/// A script run in the page (`aio.send(v)` hands a value to the view,
/// `await aio.recv()` takes one from it, `return v` ends it): focus,
/// measuring, the page's own APIs.
pub fn eval(js: &str) -> Eval {
    dioxus_document::eval(js)
}

pub use dioxus_document::{Eval, EvalError};

/// Its children, or `fallback(error)` once one of them throws (`?` in a view).
#[component]
pub fn ErrorBoundary(fallback: Callback<String, View>, children: View) -> View {
    rsx! {
        dx::ErrorBoundary {
            handle_error: move |e: dx::ErrorContext| fallback.call(e.error().map(|e| e.to_string()).unwrap_or_default()),
            {children}
        }
    }
}

/// Its children, or `fallback` while one of them waits for a
/// [`Resource::suspend`].
#[component]
pub fn Suspense(fallback: View, children: View) -> View {
    rsx! {
        dx::SuspenseBoundary { fallback: move |_| fallback.clone(), {children} }
    }
}

/// The names `rsx!` and `#[component]` expand to; not for apps.
#[doc(hidden)]
pub mod __rsx {
    pub use dioxus::prelude::{HasAttributes, dioxus_core, dioxus_elements, dioxus_signals};
}

/// Is a view rendering now (not a handler, an effect or a memo)?
pub(crate) fn in_render() -> bool {
    dioxus_core::ReactiveContext::current().is_some_and(|rc| rc.to_string().contains("for scope"))
}

/// The window's page, kept by a view to talk to it after the view is gone
/// (at unmount no view is current to ask).
#[derive(Clone)]
pub(crate) struct Page(std::rc::Rc<dyn dioxus_document::Document>);

impl Page {
    pub(crate) fn here() -> Self {
        Self(dioxus_document::document())
    }

    pub(crate) fn eval(&self, js: String) -> Eval {
        self.0.eval(js)
    }
}

/// Redraw the current view later, from anywhere (a portal's host when
/// content is put in it).
pub(crate) fn redraw() -> std::sync::Arc<dyn Fn() + Send + Sync> {
    dioxus_core::schedule_update()
}

/// Give the views below `v` now, replacing what this view gave before (a
/// router's place, made again each render). Not for apps: [`provide`].
#[doc(hidden)]
pub fn provide_now<T: Clone + 'static>(v: T) -> T {
    dioxus_core::provide_context(v)
}

/// What the views above this one provided, skipping its own.
#[doc(hidden)]
pub fn parent_context<T: Clone + 'static>() -> Option<T> {
    dioxus_core::parent_scope().and_then(dioxus_core::consume_context_from_scope::<T>)
}

/// Is `v` nothing (`rsx! {}`, no children given, or an error)?
#[doc(hidden)]
pub fn is_empty(v: &View) -> bool {
    // No children is Dioxus's placeholder: one dynamic root, empty.
    !v.as_ref().is_ok_and(|c| {
        use dioxus_core::{DynamicNode, TemplateNode};
        c.template.roots.iter().any(|r| !matches!(r, TemplateNode::Dynamic { .. }))
            || c.dynamic_nodes.iter().any(|n| !matches!(n, DynamicNode::Placeholder(_)))
    })
}

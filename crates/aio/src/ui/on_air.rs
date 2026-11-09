//! The view API (`aio::ui::prelude`) on air, aio-rs's own renderer
//! (feature `kit`, docs/design/air.md). A view's body runs once; its holes
//! follow what they read.
//! Most names are air's own; the page's scripts are typed here (serde).
use serde::Serialize;
use serde::de::DeserializeOwned;

pub use air::{
    Attribute, Callback, Code, Component, Effect, ErrorBoundary, ErrorBoundaryProps, EventHandler, Fragment,
    FragmentProps, Global, GlobalMemo, H, Key, Location, Memo, Modifiers, MouseButton, Optimistic, Prop, Props,
    ReadGuard, Resource, Retry, Signal, Suspense, SuspenseProps, Task, TrackedMemo, View, ViewError, WriteGuard,
    callback, comp, component, context, effect, effect_quiet, geometry, h, in_views, is_empty, key_event, memo,
    on_cleanup, once, parent_context, provide, provide_now, provide_root, resource, retry, rsx, signal, spawn,
    spawn_forever, task, try_context, untrack, use_optimistic, watch,
};

/// [`resource`] whose value a served page carries (`Http::painted`): the
/// server waits for it (up to a deadline) while the page streams, sends
/// the `Suspense` it filled in with the value, and the web build's first
/// frame starts from that value with no load of its own. What the closure
/// reads before its `async` is followed from there:
/// `seeded_resource(move || { let id = id.get(); async move { load(id).await } })`.
#[track_caller]
pub fn seeded_resource<T, F>(f: impl FnMut() -> F + 'static) -> Resource<T>
where
    T: Serialize + DeserializeOwned + 'static,
    F: std::future::Future<Output = T> + 'static,
{
    #[cfg(all(target_arch = "wasm32", feature = "web"))]
    crate::web::seed_resources();
    air::seeded_resource(|v: &T| serde_json::to_string(v).ok(), |s| serde_json::from_str(s).ok(), f)
}

/// Run `f(next, prev)` each time `read()`'s value changes, not at the
/// start (a hook; aio's `watch(source, fn)` and `effect(on(..))`): only
/// what `read` reads is followed, `f` runs untracked.
#[track_caller]
pub fn on_change<T: Clone + PartialEq + 'static>(
    mut read: impl FnMut() -> T + 'static,
    mut f: impl FnMut(&T, &T) + 'static,
) {
    let mut prev: Option<T> = None;
    air::effect(move || {
        let next = read();
        if let Some(p) = prev.replace(next.clone()).filter(|p| *p != next) {
            air::untrack(|| f(&next, &p));
        }
    });
}

/// In a view's body: `f()`'s value, the body running again when it changes
/// (its hooks kept, its view made anew): for a view whose shape follows
/// state its body must read (the router's `Route`). A memo, so the body
/// runs again only for a new value.
/// Read state in a hole where it can be: a hole re-runs alone.
#[doc(hidden)]
pub fn follow<T: Clone + PartialEq + 'static>(f: impl Fn() -> T + 'static) -> T {
    let m = air::memo(f);
    air::follow(|| m.get())
}

/// The event kinds and their payloads.
pub mod events {
    pub use air::{
        AnimationEvent, ClipboardEvent, CompositionEvent, DataTransfer, DragData, DragEvent, Event, FileData,
        FocusData, FocusEvent, FormData, FormEvent, ImageEvent, KeyboardData, KeyboardEvent, MediaEvent, MountedData,
        MountedEvent, MouseData, MouseEvent, PlainData, PointerData, PointerEvent, ResizeData, ResizeEvent,
        ScrollBehavior, ScrollData, ScrollEvent, SelectionEvent, ToggleEvent, TouchEvent, TransitionEvent, VisibleData,
        VisibleEvent, WheelData, WheelEvent,
    };
}

/// Why a page script's value didn't come.
#[derive(Debug, Clone, PartialEq)]
pub enum EvalError {
    /// The script failed, or its view ended.
    Finished(String),
    /// A value that isn't the type asked for.
    Serialization(String),
}

impl std::fmt::Display for EvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Finished(e) => write!(f, "the page script ended: {e}"),
            Self::Serialization(e) => write!(f, "the page script's value: {e}"),
        }
    }
}

impl std::error::Error for EvalError {}

/// A script run in the page (`aio.send(v)` hands a value to the view,
/// `await aio.recv()` takes one from it, `return v` ends it): focus,
/// measuring, the page's own APIs.
#[derive(Clone, Debug)]
pub struct Eval(air::Eval);

pub fn eval(js: &str) -> Eval {
    Eval(air::eval(js))
}

impl Eval {
    /// Hand the script `v` (its `await aio.recv()`).
    pub fn send(&self, v: impl Serialize) -> Result<(), EvalError> {
        let json = serde_json::to_string(&v).map_err(|e| EvalError::Serialization(e.to_string()))?;
        self.0.send_json(&json).map_err(EvalError::Finished)
    }

    /// The next value the script sent (`aio.send(v)`).
    pub async fn recv<T: DeserializeOwned>(&mut self) -> Result<T, EvalError> {
        let json = self.0.recv_json().await.map_err(EvalError::Finished)?;
        serde_json::from_str(&json).map_err(|e| EvalError::Serialization(e.to_string()))
    }

    /// What the script returned.
    pub async fn join<T: DeserializeOwned>(self) -> Result<T, EvalError> {
        let json = self.0.join_json().await.map_err(EvalError::Finished)?;
        serde_json::from_str(&json).map_err(|e| EvalError::Serialization(e.to_string()))
    }
}

/// Is a view's body running now (`client!`'s whole-state hint)?
pub(crate) fn in_render() -> bool {
    air::in_body()
}

//! Deferred content (aio's `Defer`): a placeholder until the trigger fires,
//! then the content, for good (one-shot per mount; a new `key` defers
//! again). What waits is the render, the code of a lazy view
//! ([`lazy!`](crate::lazy!): a web build fetches it), and data the content loads itself
//! (`use_resource`, [`use_held`](super::use_held)) with its own loading
//! and error states, or `load` here (aio's `load`, `loading`, `error`,
//! `loadingMinMs`): the content made once its data came.
use super::Instant;
use super::join;
use crate::ui::prelude::*;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::time::Duration;

/// How [`Defer`] makes its content once triggered (aio's `load`): an
/// async fn whose view (or the failure it says) shows when it is done.
#[derive(Clone)]
pub struct Load(Rc<dyn Fn() -> Loading>);

/// A [`Load`]'s run: the view it made, or what it said.
type Loading = Pin<Box<dyn Future<Output = Result<View, String>>>>;

impl PartialEq for Load {
    fn eq(&self, o: &Self) -> bool {
        Rc::ptr_eq(&self.0, &o.0)
    }
}

/// A [`Load`] from an async fn: `load(|| async { let rows = fetch().await?; Ok(rsx! { … }) })`.
pub fn load<F, Fut>(f: F) -> Load
where
    F: Fn() -> Fut + 'static,
    Fut: Future<Output = Result<View, String>> + 'static,
{
    Load(Rc::new(move || Box::pin(f())))
}

/// A [`Load`] of a lazy view (`aio::lazy!`, its `entry`): in a split web
/// build its code is fetched the first time; elsewhere it is in the binary.
#[doc(hidden)]
pub fn lazy_view(entry: fn() -> View) -> Load {
    load(move || async move {
        // Called by its address, never directly: what only it reaches can be split off.
        let entry = std::hint::black_box(entry);
        #[cfg(all(target_arch = "wasm32", feature = "web"))]
        crate::web::lazy::load(entry as usize as u32).await?;
        Ok(entry())
    })
}

/// A view whose code a web build fetches only when it first shows (aio's
/// `lazy(() => import("./View.tsx"))` in a `Suspense`): a [`Load`] for
/// [`Defer`], which shows `loading` meanwhile and `error` if the fetch
/// fails (offline before the page cached it; the next show tries again).
/// `ax build --web` puts the code only lazy views reach into chunks, one
/// per view (what two share, in a common one); on the desktop and Android
/// it is in the one binary, as is a dev build's. The view captures nothing
/// (it is made by a plain `fn`): it reads its state from cells and hooks.
///
/// ```rust,ignore
/// Defer { trigger: Trigger::Now, load: aio::lazy!(rsx! { Chart {} }), loading: rsx! { p { "Loading…" } } }
/// ```
#[macro_export]
macro_rules! lazy {
    ($view:expr) => {{
        #[inline(never)]
        fn __aio_lazy() -> $crate::ui::View {
            $view
        }
        $crate::kit::lazy_view(__aio_lazy)
    }};
}

/// Where a [`Defer`] with a [`Load`] is.
#[derive(Clone, PartialEq)]
enum Stage {
    Waiting,
    Loading,
    Loaded(View),
    Failed(String),
}

/// When [`Defer`] shows its content.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Trigger {
    Now,
    After(Duration),
    /// Once the first frame is drawn and the view has nothing else to do.
    Idle,
    /// When the placeholder comes into the window.
    Viewport,
    /// When the pointer comes over the placeholder.
    Hover,
    /// At a click or a key on the placeholder.
    Interaction,
}

/// `placeholder` (in a box of its own: `class`, attributes) until
/// `trigger` fires, then `children`; or, with `load`, `loading` while it
/// runs (at least `loading_min_ms`, so it doesn't flash), then what it
/// made, or `error` of what it said.
#[component]
pub fn Defer(
    trigger: Trigger,
    placeholder: Option<View>,
    load: Option<Load>,
    loading: Option<View>,
    error: Option<Callback<String, View>>,
    #[props(default)] loading_min_ms: u64,
    #[props(default, into)] class: String,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: View,
) -> View {
    let on = signal(|| trigger == Trigger::Now);
    let stage = signal(|| Stage::Waiting);
    effect(move || {
        if !on.get() {
            return;
        }
        let Some(load) = load.clone() else { return };
        if stage.with_untracked(|s| *s != Stage::Waiting) {
            return;
        }
        stage.set(Stage::Loading);
        let at = Instant::now();
        spawn(async move {
            let got = (load.0)().await;
            if let Some(rest) = Duration::from_millis(loading_min_ms).checked_sub(Instant::now() - at) {
                super::sleep(rest).await;
            }
            stage.set(match got {
                Ok(v) => Stage::Loaded(v),
                Err(e) => Stage::Failed(e),
            });
        });
    });
    once(|| {
        let wait = match trigger {
            Trigger::After(d) => Some(d),
            Trigger::Idle => Some(super::FRAME),
            _ => None,
        };
        if let Some(d) = wait {
            spawn(async move {
                super::sleep(d).await;
                tokio::task::yield_now().await;
                on.set(true);
            });
        }
    });
    let fire = move |when: Trigger| {
        if when == trigger {
            on.set(true)
        }
    };
    let class = join("aio-defer", &class);
    rsx! {
        if on.get() {
            {
                match stage.get() {
                    Stage::Waiting => Some(children.clone()),
                    Stage::Loading => loading.clone(),
                    Stage::Loaded(v) => Some(v),
                    Stage::Failed(e) => error.map(|f| f.call(e)),
                }
            }
        } else {
            div {
                class,
                onvisible: move |e| {
                    if e.is_intersecting().unwrap_or(false) {
                        fire(Trigger::Viewport)
                    }
                },
                onmousemove: move |_| fire(Trigger::Hover),
                onclick: move |_| fire(Trigger::Interaction),
                onkeydown: move |_| fire(Trigger::Interaction),
                ..attributes,
                {placeholder}
            }
        }
    }
}

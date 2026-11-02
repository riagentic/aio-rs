//! A window's views on the build's one renderer, over the window's wire
//! (the desktop runtime's socket, Android's port): Dioxus's liveview
//! (feature `dioxus`, the default) or air (feature `air` alone, no Dioxus
//! in the build). One call per page connected: its views until it goes.
use aio::ui::View;

#[cfg(not(any(feature = "dioxus", feature = "air")))]
compile_error!("aio-rs-shell: no renderer: its default features (`dioxus`), or `air` with `default-features = false`");

/// The windows' views of this process.
#[cfg(feature = "dioxus")]
#[derive(Clone)]
pub(crate) struct Views(std::sync::Arc<dioxus_liveview::LiveViewPool>);

#[cfg(feature = "dioxus")]
impl Views {
    pub fn new() -> Self {
        Self(std::sync::Arc::new(dioxus_liveview::LiveViewPool::new()))
    }

    /// `app`'s views for the page on `s`, until it goes.
    pub async fn launch(&self, s: aio::ipc::Stream, app: fn() -> View) -> Result<(), String> {
        let dom = move || {
            crate::WINDOW_THREAD.set(true);
            dioxus_core::VirtualDom::new(app)
        };
        self.0.launch_virtualdom(crate::wire(s), dom).await.map_err(|e| e.to_string())
    }
}

/// The windows' views of this process: each page's on a thread of its own
/// (air keeps one root per thread), its tasks on that thread's runtime.
#[cfg(all(feature = "air", not(feature = "dioxus")))]
#[derive(Clone)]
pub(crate) struct Views;

#[cfg(all(feature = "air", not(feature = "dioxus")))]
impl Views {
    pub fn new() -> Self {
        Self
    }

    /// `app`'s views for the page on `s`, until it goes.
    pub async fn launch(&self, s: aio::ipc::Stream, app: fn() -> View) -> Result<(), String> {
        let (done, ended) = tokio::sync::oneshot::channel();
        std::thread::Builder::new()
            .name("aio-window".into())
            .spawn(move || {
                crate::WINDOW_THREAD.set(true);
                let r = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| e.to_string())
                    .and_then(|rt| tokio::task::LocalSet::new().block_on(&rt, on_air::pump(s, app)));
                _ = done.send(r);
            })
            .map_err(|e| e.to_string())?;
        ended.await.unwrap_or_else(|_| Err("the window's views panicked".into()))
    }
}

#[cfg(all(feature = "air", not(feature = "dioxus")))]
const _: () = assert!(aio::ui::ON_AIR, "aio-rs-shell on air needs the views on air: aio-rs without `kit`");

#[cfg(all(feature = "air", not(feature = "dioxus")))]
pub(crate) mod on_air {
    use aio::ui::View;
    use futures_util::{SinkExt, StreamExt};

    /// The page's frames to the root (its events, its scripts' replies),
    /// the views' tasks run when woken, each flush's edits to the page as
    /// one frame; a view's error no boundary caught is said.
    pub async fn pump(s: aio::ipc::Stream, app: fn() -> View) -> Result<(), String> {
        let (mut to_page, mut from_page) = crate::wire(s).split();
        let woke = std::sync::Arc::new(tokio::sync::Notify::new());
        let w = woke.clone();
        air::on_wake(move || w.notify_one());
        let root = air::Root::new();
        root.show(app);
        loop {
            air::run_tasks();
            let edits = root.take_edits();
            if !edits.is_empty() {
                to_page.send(edits).await.map_err(|_| "the page is gone".to_string())?;
            }
            for e in root.uncaught() {
                eprintln!("[aio-shell] window: a view failed and no ErrorBoundary is above it: {e}");
            }
            tokio::select! {
                f = from_page.next() => match f {
                    Some(Ok(f)) => _ = root.event(&f),
                    _ => return Ok(()),
                },
                _ = woke.notified() => {}
            }
        }
    }
}

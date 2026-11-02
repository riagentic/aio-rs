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

/// `ax dev`'s hot reload (the door's `Aio.hot`): `markup` for the
/// templates of `files`, then every window's views built again in place,
/// their state kept. How many windows were told.
#[cfg(all(feature = "air", not(feature = "dioxus")))]
pub(crate) fn hot(files: &[String], markup: Vec<air::hot::Markup>) -> usize {
    air::hot::set_markup(files, markup);
    on_air::rebuild()
}

#[cfg(all(feature = "air", not(feature = "dioxus")))]
pub(crate) mod on_air {
    use aio::ui::View;
    use futures_util::{SinkExt, StreamExt};
    use std::sync::Mutex;
    use tokio::sync::mpsc::UnboundedSender;

    /// Each open window's way to be told to build its views again.
    static WINDOWS: Mutex<Vec<UnboundedSender<()>>> = Mutex::new(Vec::new());

    /// Tell every open window to build its views again; how many there are.
    pub fn rebuild() -> usize {
        let mut w = WINDOWS.lock().unwrap_or_else(|e| e.into_inner());
        w.retain(|tx| tx.send(()).is_ok());
        w.len()
    }

    /// The app's view, its body through the patched code when a patch came
    /// (`ax dev`'s hot-patch, a dev build's).
    fn current(app: fn() -> View) -> View {
        #[cfg(all(debug_assertions, feature = "dev"))]
        return subsecond::HotFn::current(app).call(());
        #[cfg(not(all(debug_assertions, feature = "dev")))]
        app()
    }

    /// After a patch: the markup sent before is in the code now, and the
    /// views run the new code from their root.
    #[cfg(all(debug_assertions, feature = "dev"))]
    pub fn patched() {
        air::hot::clear_markup();
        rebuild();
    }

    /// The page's frames to the root (its events, its scripts' replies),
    /// the views' tasks run when woken, each flush's edits to the page as
    /// one frame; a view's error no boundary caught is said. Told to (a hot
    /// reload), its views are built again in place, their state kept.
    pub async fn pump(s: aio::ipc::Stream, app: fn() -> View) -> Result<(), String> {
        let (mut to_page, mut from_page) = crate::wire(s).split();
        let woke = std::sync::Arc::new(tokio::sync::Notify::new());
        let w = woke.clone();
        air::on_wake(move || w.notify_one());
        let (tx, mut hot) = tokio::sync::mpsc::unbounded_channel();
        WINDOWS.lock().unwrap_or_else(|e| e.into_inner()).push(tx);
        let root = air::Root::new();
        root.show(move || current(app));
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
                Some(()) = hot.recv() => root.hot(),
            }
        }
    }
}

#[cfg(all(test, feature = "air", not(feature = "dioxus")))]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use tokio_util::codec::{Framed, LengthDelimitedCodec};

    fn app() -> View {
        use aio::ui::prelude::*;
        let n = signal(|| 0u32);
        // The call's place for the reload below: the next line, column 9.
        AT.set(line!() + 1);
        rsx! { button { onclick: move |_| n += 1, "Add" } p { "{n}" } }
    }

    thread_local!(static AT: std::cell::Cell<u32> = const { std::cell::Cell::new(0) });
    static LINE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    /// A frame's text: what the page is told, readable enough to look for.
    fn said(f: &[u8]) -> String {
        String::from_utf8_lossy(f).into_owned()
    }

    #[tokio::test]
    async fn the_doors_hot_reload_redraws_the_window_with_the_new_markup() {
        let (ours, page) = tokio::io::duplex(1 << 20);
        let views = Views::new();
        let window = tokio::spawn(async move {
            views
                .launch(Box::new(ours), || {
                    let v = app();
                    LINE.store(AT.get(), std::sync::atomic::Ordering::Relaxed);
                    v
                })
                .await
        });
        let codec = LengthDelimitedCodec::builder().max_frame_length(256 << 20).new_codec();
        let mut page = Framed::new(page, codec);
        let first = page.next().await.unwrap().unwrap();
        assert!(said(&first).contains("Add"), "the first frame draws the view");
        // `ax dev`'s markup for that call: "Add" now "Plus" (its template: button, its text; p, a hole).
        let s = |b: &mut Vec<u8>, v: &str| {
            b.extend_from_slice(&(v.len() as u32).to_le_bytes());
            b.extend_from_slice(v.as_bytes());
        };
        let mut nodes = vec![1];
        s(&mut nodes, "button");
        nodes.push(0);
        nodes.extend_from_slice(&1u16.to_le_bytes());
        nodes.push(1);
        nodes.push(2);
        s(&mut nodes, "Plus");
        nodes.push(1);
        s(&mut nodes, "p");
        nodes.push(0);
        nodes.extend_from_slice(&1u16.to_le_bytes());
        nodes.push(0);
        nodes.push(3);
        use base64::Engine;
        let line = LINE.load(std::sync::atomic::Ordering::Relaxed);
        let t = aio::serde_json::json!({
            "file": file!(), "line": line, "col": 9, "idx": 0, "count": 4,
            "nodes": base64::engine::general_purpose::STANDARD.encode(&nodes),
        });
        let ask = aio::serde_json::json!({ "id": 7, "method": crate::inspect::HOT, "params": { "files": [file!()], "templates": [t] } });
        let mut relay = crate::inspect::Relay::default();
        let me = relay.join();
        let answer = relay.ask(me, &ask.to_string()).unwrap_err();
        assert_eq!(answer, r#"{"id":7,"result":{"windows":1}}"#, "answered at once, one window told");
        let next =
            tokio::time::timeout(std::time::Duration::from_secs(10), page.next()).await.unwrap().unwrap().unwrap();
        assert!(said(&next).contains("Plus") && !said(&next).contains("Add"), "redrawn with the new markup");
        air::hot::clear_markup();
        drop(page);
        window.await.unwrap().unwrap();
    }
}

//! A window's views, drawn by air over the window's wire (the desktop
//! runtime's socket, Android's port, a browser tab's WebSocket). One call
//! per page connected: its views until it goes.
use aio::ui::View;

/// The windows' views of this process: each page's on a thread of its own
/// (air keeps one root per thread), its tasks on that thread's runtime.
#[derive(Clone)]
pub(crate) struct Views;

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

/// A dev build's mounted components of every open window (`Aio.components`,
/// `ax components`): each one's name, key, body runs (`renders`), its own
/// holes' rewrites (`updates`) and its children, as of the last flush; the
/// windows' last renders, oldest first (`renders`, `ax components --renders`),
/// and how many they rendered in all (`total`).
pub(crate) fn components() -> aio::serde_json::Value {
    let t = on_air::TREES.lock().unwrap_or_else(|e| e.into_inner());
    let all: Vec<_> = t.iter().flat_map(|t| t.1.clone()).collect();
    let mut renders: Vec<_> = t.iter().flat_map(|t| t.2.clone()).collect();
    renders.sort_by_key(|r| r["at"].as_u64());
    let cut = renders.len().saturating_sub(air::RENDERS_KEPT);
    let total: u64 = t.iter().map(|t| t.3).sum();
    aio::serde_json::json!({ "components": all, "renders": &renders[cut..], "total": total })
}

/// `ax dev`'s hot reload (the door's `Aio.hot`): `markup` for the
/// templates of `files`, then every window's views built again in place,
/// their state kept. How many windows were told.
pub(crate) fn hot(files: &[String], markup: Vec<air::hot::Markup>) -> usize {
    air::hot::set_markup(files, markup);
    on_air::rebuild()
}

pub(crate) mod on_air {
    use aio::serde_json::Value;
    use aio::ui::View;
    use futures_util::{SinkExt, StreamExt};
    use std::sync::Mutex;
    use tokio::sync::mpsc::UnboundedSender;

    /// Each open window's way to be told to build its views again.
    static WINDOWS: Mutex<Vec<UnboundedSender<()>>> = Mutex::new(Vec::new());

    /// What a dev build keeps of each open window (by its thread's id): its
    /// component tree, its last renders and how many it rendered in all.
    pub(super) static TREES: Mutex<Vec<(std::thread::ThreadId, Vec<Value>, Vec<Value>, u64)>> = Mutex::new(Vec::new());

    /// One component and its children as `Aio.components` says them (pure).
    fn json(c: air::Mounted) -> Value {
        let children: Vec<Value> = c.children.into_iter().map(json).collect();
        aio::serde_json::json!({ "name": c.name, "key": c.key, "renders": c.renders, "updates": c.updates, "children": children })
    }

    /// One render as `Aio.components` says it (pure); the root's own view is "root".
    fn render(r: air::Render) -> Value {
        let component = if r.component.is_empty() { "root" } else { r.component };
        aio::serde_json::json!({ "component": component, "key": r.key, "at": r.at, "ms": r.ms, "trigger": r.trigger })
    }

    /// This window's tree and renders, kept for the door (a dev build's), or dropped as it goes.
    fn keep_tree(kept: Option<(Vec<Value>, Vec<Value>, u64)>) {
        let me = std::thread::current().id();
        let mut t = TREES.lock().unwrap_or_else(|e| e.into_inner());
        t.retain(|k| k.0 != me);
        t.extend(kept.map(|(tree, renders, total)| (me, tree, renders, total)));
    }

    /// Drops this window's tree when its pump ends, however it ends.
    struct Kept;

    impl Drop for Kept {
        fn drop(&mut self) {
            keep_tree(None);
        }
    }

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
        return aio_hotpatch::HotFn::current(app).call(());
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
        let _kept = Kept;
        loop {
            air::run_tasks();
            let edits = root.take_edits();
            if !edits.is_empty() {
                if cfg!(debug_assertions) {
                    let (tree, (renders, total)) = root.within(|| (air::components(), air::renders()));
                    keep_tree(Some((
                        tree.into_iter().map(json).collect(),
                        renders.into_iter().map(render).collect(),
                        total,
                    )));
                }
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

#[cfg(test)]
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
        // At once; this window among those told (the tests beside it in this process hold theirs).
        let told: aio::serde_json::Value = aio::serde_json::from_str(&answer).unwrap();
        assert!(told["id"] == 7 && told["result"]["windows"].as_u64() >= Some(1), "{answer}");
        let next =
            tokio::time::timeout(std::time::Duration::from_secs(10), page.next()).await.unwrap().unwrap().unwrap();
        assert!(said(&next).contains("Plus") && !said(&next).contains("Add"), "redrawn with the new markup");
        air::hot::clear_markup();
        drop(page);
        window.await.unwrap().unwrap();
    }

    #[aio::ui::prelude::component]
    fn Logged() -> View {
        use aio::ui::prelude::*;
        let n = signal(|| 0u32);
        task(move || async move { n.set(1) });
        rsx! { p { "logged {n}" } }
    }

    #[tokio::test]
    async fn the_door_says_the_windows_last_renders_as_they_happened() {
        use aio::ui::prelude::*;
        let (ours, page) = tokio::io::duplex(1 << 20);
        let window = tokio::spawn(async move { Views::new().launch(Box::new(ours), || rsx! { Logged {} }).await });
        let codec = LengthDelimitedCodec::builder().max_frame_length(256 << 20).new_codec();
        let mut page = Framed::new(page, codec);
        let ours = || -> Vec<String> {
            let c = components();
            let r = c["renders"].as_array().unwrap().iter().filter(|r| r["component"] == "Logged");
            r.map(|r| format!("{} {}", r["trigger"].as_str().unwrap(), r["ms"].is_f64() && r["at"].as_u64() > Some(0)))
                .collect()
        };
        let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while ours().len() < 2 {
            assert!(std::time::Instant::now() < end, "no renders logged: {}", components());
            _ = tokio::time::timeout(std::time::Duration::from_millis(100), page.next()).await;
        }
        assert_eq!(ours(), ["mount true", "signal true"], "its body's run, then its hole's rewrite");
        assert!(components()["total"].as_u64() >= Some(2));
        drop(page);
        window.await.unwrap().unwrap();
        assert!(ours().is_empty(), "a closed window's renders go with it");
    }
}

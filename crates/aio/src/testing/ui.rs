// UI tests (aio's `testUI`): the real app in the page the app's window
// shows, drawn by the engine the window runs (the pinned Electron, with no
// window on screen). The views run here, in the test's thread, and reach
// the page over the same wire the window uses: DOM edits out, the page's
// events back. The test acts as a person does: elements found by their
// `data-t` handle or auto name, clicked at a point of their box with the
// mouse, typed into with the keyboard; what the page does then (focus,
// submit, scroll, observers) is the browser's own.
use super::electron::{Answered, Asked, Page};
use super::{PATIENCE, TRACES, UNSEEN, dump, failing_on_purpose, wire_refusals};
use crate::{Cell, Door};
use base64::Engine;
use crate::ui::{Component, View};
pub use crate::ui::prelude::{Key, Modifiers};
use dioxus::dioxus_core::{ScopeId, VirtualDom};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// Tall enough that every pane fits, so nothing needs scrolling to be clicked.
const VIEWPORT: (u32, u32) = (1180, 6000);

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// The test page's `window.__aioWire` (where the window's preload puts its
/// own): frames in by `__aioStep(frames)`, which hands back the frames the
/// page sent and the form submits the browser refused; `__aioFrame()` the
/// same once the page's queued work and the app's queries ran, and when one
/// ran or anything watches its size or visibility, once it has drawn twice
/// (its scroll events and observers have run).
pub(crate) const BRIDGE: &str = r#"window.__aioWire = (() => {
  let on = null;
  const early = [], out = [], invalid = [];
  const bytes = (s) => { const b = atob(s), u = new Uint8Array(b.length); for (let i = 0; i < b.length; i++) u[i] = b.charCodeAt(i); return u; };
  const text = (u) => { let s = ''; for (let i = 0; i < u.length; i += 0x8000) s += String.fromCharCode.apply(null, u.subarray(i, i + 0x8000)); return btoa(s); };
  // A script asking (the interpreter checks each form it reads) is no submit.
  let asked = 0;
  for (const C of [HTMLFormElement, HTMLInputElement, HTMLSelectElement, HTMLTextAreaElement]) {
    const check = C.prototype.checkValidity;
    C.prototype.checkValidity = function () { asked++; try { return check.call(this); } finally { asked--; } };
  }
  document.addEventListener('invalid', (e) => {
    if (asked) return;
    const el = e.target, what = el.getAttribute('data-t') || el.getAttribute('name') || el.localName;
    invalid.push(el.validity.valueMissing ? `required field \`${what}\` is empty` : `field \`${what}\` is invalid: ${el.validationMessage}`);
  }, true);
  const take = () => ({ out: out.splice(0), invalid: invalid.splice(0) });
  window.__aioStep = (frames) => { for (const s of frames) { const f = bytes(s); on ? on(f) : early.push(f); } return take(); };
  window.__aioFrame = () => new Promise((r) => {
    const it = window.interpreter, done = () => r(take());
    // The app's queries (`__aioRun`) load as scripts: each has run first,
    // and what one did (a scroll, a focus) has drawn and told its events.
    const ran = () => {
      if (window.__aioRunning) return setTimeout(ran);
      const drawn = window.__aioRan;
      window.__aioRan = false;
      if (drawn || (it && (it.resizeObserver || it.intersectionObserver))) requestAnimationFrame(() => requestAnimationFrame(() => setTimeout(done)));
      else setTimeout(done);
    };
    ran();
  });
  return { send: (u) => out.push(text(u)), listen: (f) => { on = f; early.splice(0).forEach(f); } };
})();"#;

/// The views' end of the wire: frames from the page in, edits out.
struct Wire {
    from_page: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
    to_page: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
}

impl futures_util::Stream for Wire {
    type Item = Result<Vec<u8>, dioxus_liveview::LiveViewError>;

    fn poll_next(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Option<Self::Item>> {
        let got = self.from_page.poll_recv(cx);
        if let std::task::Poll::Ready(Some(_)) = got {
            // The page said something: renders from here on answer it.
            renders::reset();
        }
        got.map(|f| f.map(Ok))
    }
}

impl futures_util::Sink<Vec<u8>> for Wire {
    type Error = dioxus_liveview::LiveViewError;

    fn poll_ready(self: std::pin::Pin<&mut Self>, _: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn start_send(self: std::pin::Pin<&mut Self>, f: Vec<u8>) -> Result<(), Self::Error> {
        self.to_page.send(f).map_err(|_| dioxus_liveview::LiveViewError::SendingFailed)
    }

    fn poll_flush(self: std::pin::Pin<&mut Self>, _: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_close(self: std::pin::Pin<&mut Self>, _: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }
}

/// Who rendered, from the span Dioxus opens for each render (`render`, its
/// component as `scope`): each view's mounted components and their renders
/// (aio DevTools' `tree`), and the tripwire for a view that renders forever.
mod renders {
    use super::MountedComponent;
    use std::cell::RefCell;
    use std::collections::{BTreeMap, HashMap};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Metadata, field::Field, subscriber::Interest};

    /// Renders of one component with nothing from the page or the test in
    /// between that make a render loop (aio's tripwire).
    const LOOP: u32 = 100;

    /// A component in a view: its runtime and its scope.
    type Key = (usize, usize);

    struct Node {
        name: &'static str,
        parent: Option<usize>,
        renders: u32,
    }

    thread_local! {
        static TREE: RefCell<BTreeMap<Key, Node>> = const { RefCell::new(BTreeMap::new()) };
        static SINCE: RefCell<HashMap<Key, u32>> = RefCell::new(HashMap::new());
        static ON: RefCell<Option<tracing::subscriber::DefaultGuard>> = const { RefCell::new(None) };
    }

    /// Listen on this thread (the views render where their test runs).
    pub(super) fn install() {
        ON.with_borrow_mut(|g| {
            g.get_or_insert_with(|| {
                // With one listener alive, tracing-core asks only the
                // registering thread's own: a render callsite first met on a
                // thread with none (a view off any test) is cached as of no
                // interest for every thread, and renders go uncounted. A
                // second one, kept for good, has it ask them all.
                static KEPT: std::sync::OnceLock<tracing::Dispatch> = std::sync::OnceLock::new();
                KEPT.get_or_init(|| tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default()));
                tracing::subscriber::set_default(Spans)
            });
        });
    }

    /// The test, or the page, had its turn: the loop count starts over.
    pub(super) fn reset() {
        SINCE.with_borrow_mut(HashMap::clear);
    }

    /// The mounted components of view `rt`, as a tree; Dioxus's own
    /// boundaries and the root's wrapper are no component of the app's.
    pub(super) fn tree(rt: usize) -> Vec<MountedComponent> {
        fn under(t: &BTreeMap<Key, Node>, rt: usize, parent: Option<usize>) -> Vec<MountedComponent> {
            let mut out = vec![];
            for (&(v, scope), n) in t.range((rt, 0)..=(rt, usize::MAX)) {
                if n.parent != parent || v != rt {
                    continue;
                }
                let children = under(t, rt, Some(scope));
                if n.parent.is_none() || n.name.starts_with("dioxus_core::") {
                    out.extend(children);
                    continue;
                }
                let name = n.name.rsplit("::").next().unwrap_or(n.name);
                out.push(MountedComponent { name, renders: n.renders, children });
            }
            out
        }
        TREE.with_borrow(|t| under(t, rt, None))
    }

    /// Kept in each component's own context: dropped with it, it takes the
    /// component out of the tree.
    #[derive(Clone)]
    struct Alive(#[allow(dead_code)] std::rc::Rc<Gone>);

    struct Gone(Key);

    impl Drop for Gone {
        fn drop(&mut self) {
            _ = TREE.try_with(|t| t.borrow_mut().remove(&self.0));
        }
    }

    fn is_render(m: &Metadata<'_>) -> bool {
        m.is_span() && m.name() == "render" && m.target().starts_with("dioxus_core")
    }

    /// The `scope` field of a render span.
    struct Scope(String);

    impl tracing::field::Visit for Scope {
        fn record_debug(&mut self, f: &Field, v: &dyn std::fmt::Debug) {
            if f.name() == "scope" {
                self.0 = format!("{v:?}");
            }
        }
    }

    struct Spans;

    impl tracing::Subscriber for Spans {
        fn register_callsite(&self, m: &'static Metadata<'static>) -> Interest {
            if is_render(m) { Interest::always() } else { Interest::never() }
        }

        fn enabled(&self, m: &Metadata<'_>) -> bool {
            is_render(m)
        }

        fn new_span(&self, a: &Attributes<'_>) -> Id {
            let id = Id::from_u64(1);
            // The render runs in its component's scope, on its view's thread.
            let Some((rt, scope)) = dioxus::dioxus_core::Runtime::try_current()
                .and_then(|rt| rt.try_current_scope_id().map(|s| (rt, s)))
            else {
                return id;
            };
            let mut name = Scope(String::new());
            a.record(&mut name);
            let key = (std::rc::Rc::as_ptr(&rt) as *const () as usize, scope.0);
            let looping = SINCE.with_borrow_mut(|s| {
                let n = s.entry(key).or_default();
                *n += 1;
                *n > LOOP
            });
            if looping {
                panic!("still rendering after {LOOP} passes: a view writes state it reads (a render loop)");
            }
            TREE.with_borrow_mut(|t| {
                // Its nearest ancestor that rendered (a suspense boundary renders unnamed).
                let mut up = rt.parent_scope(scope);
                while let Some(p) = up.filter(|p| !t.contains_key(&(key.0, p.0))) {
                    up = rt.parent_scope(p);
                }
                let parent = up.map(|p| p.0);
                let n = t.entry(key).or_insert_with(|| Node { name: crate::ui::sym(&name.0), parent, renders: 0 });
                if n.name != name.0 || n.parent != parent {
                    *n = Node { name: crate::ui::sym(&name.0), parent, renders: 0 };
                }
                n.renders += 1;
            });
            if rt.has_context::<Alive>(scope).is_none() {
                rt.provide_context(scope, Alive(std::rc::Rc::new(Gone(key))));
            }
            id
        }

        fn record(&self, _: &Id, _: &Record<'_>) {}
        fn record_follows_from(&self, _: &Id, _: &Id) {}
        fn event(&self, _: &Event<'_>) {}
        fn enter(&self, _: &Id) {}
        fn exit(&self, _: &Id) {}
    }
}

/// How a `TestUi` mounts (aio's `testUI` options): the window size, light or
/// dark (`prefers-color-scheme`), scrollbars or none, what runs in the
/// app's root scope before its first render (a seed, a route), and the
/// hosts it may reach.
pub struct Mount {
    size: (u32, u32),
    dark: bool,
    scrollbars: bool,
    before: Vec<Box<dyn FnOnce()>>,
    network: Vec<String>,
    stubs: Vec<(String, super::electron::Stub)>,
}

impl Default for Mount {
    fn default() -> Self {
        Self { size: VIEWPORT, dark: false, scrollbars: true, before: vec![], network: vec![], stubs: vec![] }
    }
}

impl Mount {
    pub fn size(self, width: u32, height: u32) -> Self {
        Self { size: (width, height), ..self }
    }

    pub fn dark(self) -> Self {
        Self { dark: true, ..self }
    }

    /// No scrollbars, and no room kept for them, as headless Chromium draws
    /// a page (aio's shots); by default they show as in the app's window.
    pub fn no_scrollbars(self) -> Self {
        Self { scrollbars: false, ..self }
    }

    pub fn before(mut self, f: impl FnOnce() + 'static) -> Self {
        self.before.push(Box::new(f));
        self
    }

    /// Let the test reach `hosts` (`api.example.org`, `*` for any) besides
    /// this machine. By default a UI test refuses the real network (aio
    /// `testUI`): the page's requests elsewhere are cancelled and fail the
    /// test (take them with `failures`), and aio's own outbound requests in
    /// this test binary are held to this machine and the hosts its mounts
    /// allowed ([`crate::capabilities`]).
    pub fn network(mut self, hosts: &[&str]) -> Self {
        self.network.extend(hosts.iter().map(|h| h.to_string()));
        self
    }

    /// Answer the page's requests to `host` with `answer` (aio's `testUI({
    /// fetch })`): from before the first render, each request the page
    /// makes there (a picture, a video: what the window's policy lets a
    /// page reach) is handed to it, and never reaches the network.
    /// `Mount::fetch("cdn.example.org", |a| Answered::new(200, "image/svg+xml", SVG))`
    pub fn fetch(mut self, host: &str, answer: impl Fn(&Asked) -> Answered + Send + Sync + 'static) -> Self {
        self.stubs.push((host.into(), std::sync::Arc::new(answer)));
        self
    }

    pub fn app(self, app: fn() -> View) -> TestUi {
        self.mount(VirtualDom::new(app))
    }

    /// Mount one component alone, with its props (aio's `testComponent`).
    pub fn component<P: Clone + 'static, M: 'static>(self, c: impl Component<P, M>, props: P) -> TestUi {
        self.mount(VirtualDom::new_with_props(c, props))
    }

    fn mount(self, vdom: VirtualDom) -> TestUi {
        // The page's events as the window's views get them (and the
        // elements `onmounted` hands out, answered by the page).
        static EVENTS: std::sync::Once = std::sync::Once::new();
        EVENTS.call_once(|| drop(dioxus_liveview::LiveViewPool::new()));
        renders::install();
        let sandbox = crate::paths::sandbox().expect("a test sandbox");
        UNSEEN.with_borrow_mut(|u| _ = u.get_or_insert_default());
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
        crate::capabilities::hold_for_tests(&self.network);
        // An answered host is reached (the network rule lets it by), never the network.
        let reach: Vec<String> = self.network.iter().cloned().chain(self.stubs.iter().map(|(h, _)| h.clone())).collect();
        let page = Page::open(self.size.0, self.size.1, &reach);
        page.answer(self.stubs);
        page.cmd("Emulation.setDeviceMetricsOverride", metrics(self.size));
        // A window on no screen has no focus of its own: the page behaves as
        // the focused window a person types into (`:focus`, Tab, focus events).
        page.cmd("Emulation.setFocusEmulationEnabled", json!({ "enabled": true }));
        let scheme = if self.dark { "dark" } else { "light" };
        page.cmd("Emulation.setEmulatedMedia", json!({ "features": [{ "name": "prefers-color-scheme", "value": scheme }] }));
        if !self.scrollbars {
            page.cmd("Emulation.setScrollbarsHidden", json!({ "hidden": true }));
        }
        let view = vdom.runtime();
        let (to_view, from_page) = tokio::sync::mpsc::unbounded_channel();
        let (to_page, from_view) = tokio::sync::mpsc::unbounded_channel();
        let local = tokio::task::LocalSet::new();
        let task = {
            let _in = rt.enter();
            for f in self.before {
                view.in_scope(ScopeId::ROOT, f);
            }
            local.spawn_local(dioxus_liveview::run(vdom, Wire { from_page, to_page }))
        };
        let mut ui = TestUi {
            task,
            local,
            view,
            to_view,
            from_view,
            video: super::video::Recorder::asked(),
            page,
            rt,
            hovering: false,
            sent: 0,
            _sandbox: sandbox,
        };
        ui.settle();
        // Every mount is audited, as aio did: said, not failed (assert on `contrast()` for that).
        for f in ui.contrast() {
            eprintln!("aio: text a person can't read: {f}");
        }
        ui
    }
}

/// Wait (up to `PATIENCE`) until every commit whose state can be read has
/// told its listeners: a view hears of a change made off the UI thread
/// only after the new state is readable, so a test could see the state,
/// act, and hit the screen from before it.
fn quiet() {
    let end = Instant::now() + PATIENCE;
    while crate::handle::TELLING.load(std::sync::atomic::Ordering::SeqCst) > 0 && Instant::now() < end {
        std::thread::yield_now();
    }
}

fn metrics((width, height): (u32, u32)) -> Value {
    json!({ "width": width, "height": height, "deviceScaleFactor": 1, "mobile": false })
}

/// An element as a person or a screen reader meets it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    pub tag: String,
    /// Its ARIA role: `role`, else the one its tag implies (`button`, `link`, `textbox`…).
    pub role: String,
    /// Its accessible name: `aria-label`, its `<label>`, its text, its `placeholder`.
    pub name: String,
}

/// An element's box in the window, CSS px ([`TestUi::rect`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Two animation frames: the second runs once a frame was drawn (what
/// observers and `scroll` saw is told).
const FRAMES: &str = "new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)))";

/// Every running CSS transition and animation that ends has ended (one
/// that ends starting another waits for that too; 5 s at most), then
/// [`FRAMES`]: the page as it rests.
const STILL: &str = "new Promise((r) => {
  const end = performance.now() + 5000;
  const step = () => {
    const ending = document.getAnimations().filter((a) => a.playState === 'running' && a.effect && Number.isFinite(a.effect.getComputedTiming().endTime));
    const left = end - performance.now();
    if (!ending.length || left <= 0) return requestAnimationFrame(() => requestAnimationFrame(r));
    const ended = Promise.all(ending.map((a) => a.finished.catch(() => {})));
    Promise.race([ended, new Promise((t) => setTimeout(t, left))]).then(() => requestAnimationFrame(step));
  };
  step();
})";

/// Text `s` put in at the caret of the focused field as a paste puts it
/// (the field's own rules apply: a line input drops a line break), told
/// by `input`; a focused editable element takes it as `insertText`.
const INSERT: &str = "(s) => {
  const el = document.activeElement;
  if (el && typeof el.setRangeText === 'function' && typeof el.selectionStart === 'number') {
    el.setRangeText(s, el.selectionStart, el.selectionEnd, 'end');
    el.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertText', data: s }));
  } else document.execCommand('insertText', false, s);
}";

/// A paste of text `s` on the focused element, as the browser makes one:
/// `paste` (its `clipboardData` holds `s`), then, when no listener took
/// it, `beforeinput` and the text put in ([`INSERT`]'s way, as `insertFromPaste`).
const PASTE: &str = "(s) => {
  const el = document.activeElement || document.body, dt = new DataTransfer();
  dt.setData('text/plain', s);
  const go = el.dispatchEvent(new ClipboardEvent('paste', { bubbles: true, cancelable: true, composed: true, clipboardData: dt }));
  if (!go || !el.dispatchEvent(new InputEvent('beforeinput', { bubbles: true, cancelable: true, inputType: 'insertFromPaste', data: s }))) return;
  if (typeof el.setRangeText === 'function' && typeof el.selectionStart === 'number') {
    el.setRangeText(s, el.selectionStart, el.selectionEnd, 'end');
    el.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertFromPaste', data: s }));
  } else document.execCommand('insertText', false, s);
}";

/// One mounted component ([`TestUi::components`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountedComponent {
    /// Its name without its module (the mounted app is `root`).
    pub name: &'static str,
    /// Times it rendered since it mounted, the first included.
    pub renders: u32,
    pub children: Vec<MountedComponent>,
}

/// The app in a headless window, driven as a person drives it. Each `t`
/// names an element: its `data-t` handle, else its auto name
/// ([`names`](Self::names): `SaveButton`), `outer/t` one inside another,
/// `text=Skip this version` the one showing that text, or
/// `css=#cmd-0` / `css=.parts li:nth-child(2)` the one a CSS selector
/// matches (two showing that text, or matching, fail, saying so).
pub struct TestUi {
    /// The views' task (the app's VirtualDom, on the window's wire).
    task: tokio::task::JoinHandle<Result<(), dioxus_liveview::LiveViewError>>,
    local: tokio::task::LocalSet,
    /// The views' runtime: the app's root scope, where its globals live.
    view: std::rc::Rc<dioxus::dioxus_core::Runtime>,
    to_view: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    from_view: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
    /// Its steps as a video, when `AIO_TEST_VIDEO` asks.
    video: Option<super::video::Recorder>,
    page: Page,
    rt: tokio::runtime::Runtime,
    /// The pointer rests where `hover` put it (not where a click left it).
    hovering: bool,
    /// Bytes the views sent the page (their DOM edits and queries).
    sent: usize,
    _sandbox: crate::paths::Sandbox,
}

impl Drop for TestUi {
    fn drop(&mut self) {
        // Passed: its traces (of a failure it caught) are no longer news.
        if !std::thread::panicking() {
            let dir = trace_dir();
            _ = std::fs::remove_dir_all(&dir);
            dir.ancestors().skip(1).take(2).for_each(|d| _ = std::fs::remove_dir(d));
        }
        let unseen = [UNSEEN.take().unwrap_or_default(), wire_refusals(), self.offline()].concat();
        if !unseen.is_empty() && !std::thread::panicking() {
            panic!("refused and never looked at (take them with `failures`): {}", unseen.join("; "));
        }
    }
}

/// The modifier keys as the DevTools protocol counts them (Alt 1, Ctrl 2, Meta 4, Shift 8).
fn bits(m: Modifiers) -> u32 {
    [(Modifiers::ALT, 1), (Modifiers::CONTROL, 2), (Modifiers::META, 4), (Modifiers::SHIFT, 8)]
        .iter()
        .filter(|(k, _)| m.contains(*k))
        .map(|(_, b)| b)
        .sum()
}

impl TestUi {
    /// Mount `app` (its globals are fresh: one app instance per test).
    pub fn new(app: fn() -> View) -> Self {
        Mount::default().app(app)
    }

    /// Mount component `c` alone with `props` (aio's `testComponent`).
    pub fn with<P: Clone + 'static, M: 'static>(c: impl Component<P, M>, props: P) -> Self {
        Mount::default().component(c, props)
    }

    /// Take the refusals the view's calls met so far (`method: why`), so
    /// the test's end doesn't fail on them.
    pub fn failures(&mut self) -> Vec<String> {
        let mine = UNSEEN.with_borrow_mut(|u| u.as_mut().map(std::mem::take).unwrap_or_default());
        [mine, wire_refusals(), self.offline()].concat()
    }

    /// The page's requests the network rule refused, taken.
    fn offline(&self) -> Vec<String> {
        let say = |u: String| {
            format!("network: the page asked for {u}: a UI test refuses the real network (allow its host with Mount::network)")
        };
        self.page.refused().into_iter().map(say).collect()
    }

    /// Run the views until `until` is done; a view that panicked fails the test with its panic.
    fn drive(&mut self, until: impl std::future::Future) {
        renders::reset();
        self.rt.block_on(self.local.run_until(until));
        if self.task.is_finished() {
            match self.rt.block_on(&mut self.task) {
                Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
                Err(e) => panic!("the views' task ended: {e}"),
                Ok(Err(e)) => panic!("the views' wire failed: {e}"),
                Ok(Ok(())) => panic!("the views ended"),
            }
        }
    }

    /// Run the views for `d` of real time.
    fn pause(&mut self, d: Duration) {
        let sleep = {
            let _in = self.rt.enter();
            tokio::time::sleep(d)
        };
        self.drive(sleep);
    }

    /// Hand the page's frames to the views; a submit the browser refused
    /// (a required field left empty) fails the test. How many there were.
    fn heard(&mut self, back: Value) -> usize {
        if let Some(why) = back["invalid"].as_array().and_then(|i| i.first()).and_then(Value::as_str) {
            panic!("the form can't be submitted: {why}");
        }
        let frames = back["out"].as_array().cloned().unwrap_or_default();
        for f in &frames {
            let bytes = f.as_str().and_then(|s| B64.decode(s).ok()).expect("a frame from the page");
            _ = self.to_view.send(bytes);
        }
        frames.len()
    }

    /// Run everything that is ready: the views' renders and tasks, their
    /// edits drawn in the page, the page's events back (mounted, focus,
    /// what its observers saw once it drew), until neither has more.
    pub fn settle(&mut self) {
        self.settle_now();
        // A commit made off the UI thread waits up to `ui::BACKGROUND` to be
        // shown: the screen catches up before the test goes on.
        let end = Instant::now() + 2 * crate::ui::BACKGROUND;
        while crate::ui::telling() && Instant::now() < end {
            self.pause(Duration::from_millis(5));
            self.settle_now();
        }
        if let Some(v) = &mut self.video {
            v.snap(&self.page);
        }
    }

    /// [`settle`](Self::settle) without waiting for a commit still to be shown.
    fn settle_now(&mut self) {
        quiet();
        let yields = || async {
            for _ in 0..4 {
                tokio::task::yield_now().await;
            }
        };
        for _ in 0..10 {
            for pass in 0.. {
                assert!(pass < 1000, "the views and their page never settle: each answers the other on and on");
                quiet();
                self.drive(yields());
                let frames: Vec<String> = std::iter::from_fn(|| self.from_view.try_recv().ok())
                    .map(|f| {
                        self.sent += f.len();
                        B64.encode(f)
                    })
                    .collect();
                let back = self.page.eval(&format!("__aioStep({})", json!(frames)));
                if self.heard(back) == 0 && frames.is_empty() {
                    break;
                }
            }
            let back = self.page.eval("__aioFrame()");
            if self.heard(back) == 0 {
                break;
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn page(&self) -> &Page {
        &self.page
    }

    /// The window as a person sees it once it is still, a PNG: the live
    /// page, drawn by the window's engine (`std::fs::write("x.png",
    /// ui.shot())`), after its CSS transitions and animations ended (5 s
    /// at most; an endless one, a spinner, is drawn where it is). The
    /// pointer shows a `:hover` look only where [`hover`](Self::hover) put
    /// it: one a click left is moved off the page first.
    pub fn shot(&self) -> Vec<u8> {
        if !self.hovering {
            self.mouse("mouseMoved", (-1.0, -1.0), false, Modifiers::empty(), 0);
        }
        self.page.eval(STILL);
        self.page.shot()
    }

    /// `js`'s value in the page (a promise awaited; a throw fails the
    /// test), then settle: what a test reads that no other call reads
    /// (`document.documentElement.scrollTop`, a canvas's pixels).
    pub fn eval(&mut self, js: &str) -> Value {
        let v = self.page.eval(js);
        self.settle();
        v
    }

    /// Global `g`'s value, read in the app's root scope (`ui.read(&DIALOG)`).
    pub fn read<T: Clone + 'static>(&self, g: &'static crate::ui::Global<T>) -> T {
        self.in_root(|| g.get_untracked())
    }

    /// `f` in the app's root scope (where its globals live, and its tasks).
    fn in_root<R>(&self, f: impl FnOnce() -> R) -> R {
        let _in = self.rt.enter();
        self.view.in_scope(ScopeId::ROOT, f)
    }

    /// Bytes the views have sent the page so far (their DOM edits and
    /// queries, as the window's wire carries them): a step's cost on the wire.
    pub fn wire_bytes(&self) -> usize {
        self.sent
    }

    /// The mounted components, as a tree (aio DevTools' `tree`): each one's
    /// name and how many times it rendered (re-render tracking).
    pub fn components(&self) -> Vec<MountedComponent> {
        renders::tree(std::rc::Rc::as_ptr(&self.view) as *const () as usize)
    }

    /// `op` of the page's reader (`aio::ui::SURFACE_JS`): its answer, or its refusal.
    fn js(&self, op: &str, arg: Value) -> Result<Value, String> {
        let r = self.page.eval(&format!("__aioTest({}, {arg})", json!(op)));
        match r["error"].as_str() {
            Some(e) => Err(e.to_string()),
            None => Ok(r["ok"].clone()),
        }
    }

    /// [`js`](Self::js); a refusal fails the test with what the page shows.
    fn must(&self, op: &str, arg: Value) -> Value {
        self.js(op, arg).unwrap_or_else(|why| panic!("{why}{}", self.trace(&why)))
    }

    /// Element `t` and its state (`data-t` handle, else auto name; `outer/t`
    /// inside `outer`; `text=Skip this version`: the one element showing that
    /// text, the button around it when it is the button's: see [`TestUi`]).
    fn el(&self, t: &str) -> Value {
        self.must("el", json!({ "path": t }))
    }

    /// The name a test may use for each showing element a person can name,
    /// with no `data-t` needed (aio's auto names): its accessible name, then
    /// its role, as one word: `<button>Save</button>` is `SaveButton`, a
    /// field labelled "Email" is `EmailInput`, an unnamed one `Input`. The
    /// same name again, in document order, is `SaveButton2`, `SaveButton3`.
    pub fn names(&self) -> Vec<String> {
        strings(&self.must("names", json!({})))
    }

    /// Run `f` in the app's root scope (where its globals live, and its tasks), then settle.
    pub fn run<R>(&mut self, f: impl FnOnce() -> R) -> R {
        let r = self.in_root(f);
        self.settle();
        r
    }

    /// Let `d` of real time pass for the app's timers (intervals, springs,
    /// a toast's), then settle.
    pub fn wait(&mut self, d: Duration) {
        let end = Instant::now() + d;
        // In slices: the render-loop tripwire counts within one.
        while let Some(left) = end.checked_duration_since(Instant::now()).filter(|l| !l.is_zero()) {
            self.pause(left.min(Duration::from_millis(10)));
        }
        self.settle();
    }

    /// Settle until `f` holds (async work such as file I/O finishes off-thread); panics after 5 s.
    pub fn until(&mut self, what: &str, f: impl Fn() -> bool) {
        self.until_within(PATIENCE, what, f)
    }

    /// `until`, failing after `patience`; the failure shows the screen and leaves a trace.
    pub fn until_within(&mut self, patience: Duration, what: &str, f: impl Fn() -> bool) {
        let end = Instant::now() + patience;
        loop {
            self.settle();
            if self.in_root(&f) {
                // It may have turned true after that settle (a commit off
                // the UI thread): the screen catches up before the test acts.
                self.settle();
                return;
            }
            if Instant::now() >= end {
                let why = format!("timed out after {patience:?} waiting for: {what}");
                panic!("{why}\n  screen:\n{}{}", self.surface(), self.trace(&why));
            }
            self.pause(Duration::from_millis(10));
        }
    }

    /// Settle until element `t` is there and shows `want` (a change applied
    /// off-thread reaches the screen a render later; an element still to
    /// come is waited for); panics after 5 s with the screen.
    pub fn until_text(&mut self, t: &str, want: &str) {
        self.until_shows(t, &format!("show {want:?}"), |s| s == want);
    }

    /// [`until_text`](Self::until_text) for a part of the text: `t` shows
    /// `part` somewhere in it ("The list did not load." of a longer note).
    pub fn until_contains(&mut self, t: &str, part: &str) {
        self.until_shows(t, &format!("show {part:?} in its text"), |s| s.contains(part));
    }

    fn until_shows(&mut self, t: &str, what: &str, ok: impl Fn(&str) -> bool) {
        let end = Instant::now() + PATIENCE;
        loop {
            let now = self.js("el", json!({ "path": t }));
            if now.as_ref().is_ok_and(|e| ok(e["text"].as_str().unwrap_or_default())) {
                return;
            }
            if Instant::now() >= end {
                let seen = match now {
                    Ok(e) => format!("it shows {:?}", e["text"].as_str().unwrap_or_default()),
                    Err(why) => why,
                };
                let why = format!("timed out after {PATIENCE:?} waiting for {t} to {what}: {seen}");
                panic!("{why}\n  screen:\n{}{}", self.surface(), self.trace(&why));
            }
            self.wait(Duration::from_millis(10));
        }
    }

    /// Wait until the cell's state satisfies `f` (aio's `expectCell`); a
    /// failure after 5 s says `what` and prints the state.
    #[track_caller]
    pub fn expect_cell<T: Cell>(&mut self, door: &impl Door<T>, what: &str, f: impl Fn(&T) -> bool) {
        let end = Instant::now() + PATIENCE;
        loop {
            self.settle();
            if self.in_root(|| door.read(&f)) {
                self.settle();
                return;
            }
            if Instant::now() >= end {
                let state = self.in_root(|| door.read(|s| dump(s, 2000)));
                panic!("{what}\n  state: {state}{}", self.trace(what));
            }
            self.pause(Duration::from_millis(10));
        }
    }

    /// Is an element with this handle or auto name showing: in the document
    /// and not hidden by CSS (`display: none` on it or around it,
    /// `visibility`)? `<button>Lock</button>` is `LockButton` with no
    /// `data-t`: [`has_handle`](Self::has_handle) asks for the handle alone.
    pub fn present(&self, t: &str) -> bool {
        self.must("present", json!({ "path": t })) == true
    }

    /// Is an element with `data-t` handle `t` showing (auto names don't count)?
    pub fn has_handle(&self, t: &str) -> bool {
        self.must("present", json!({ "path": t, "handle": true })) == true
    }

    /// Every `data-t` handle in the page, sorted (showing or not).
    pub fn handles(&self) -> Vec<String> {
        strings(&self.must("handles", json!({})))
    }

    /// Is nothing with this handle showing (never rendered, removed, or hidden)?
    pub fn absent(&self, t: &str) -> bool {
        !self.present(t)
    }

    /// How many elements match a CSS selector.
    pub fn count(&self, selector: &str) -> usize {
        self.must("count", json!({ "selector": selector })).as_u64().unwrap_or(0) as usize
    }

    /// Its text, all descendants joined.
    pub fn text(&self, t: &str) -> String {
        self.el(t)["text"].as_str().unwrap_or_default().into()
    }

    /// A field's current value (a `<select>`'s: its chosen option's).
    pub fn value(&self, t: &str) -> String {
        self.el(t)["value"].as_str().unwrap_or_default().into()
    }

    /// An attribute's value (`aria-pressed`, `href`, …); `None`: not set.
    pub fn attr(&self, t: &str, name: &str) -> Option<String> {
        self.must("attr", json!({ "path": t, "name": name })).as_str().map(String::from)
    }

    /// A CSS property's computed value (`color`, a custom one: `--split-a`), trimmed.
    pub fn style(&self, t: &str, property: &str) -> String {
        self.must("style", json!({ "path": t, "name": property })).as_str().unwrap_or_default().into()
    }

    /// Its box in the window, CSS px (`getBoundingClientRect`).
    pub fn rect(&self, t: &str) -> Rect {
        let r = self.must("rect", json!({ "path": t }));
        let n = |k: &str| r[k].as_f64().unwrap_or(0.0);
        Rect { x: n("x"), y: n("y"), width: n("width"), height: n("height") }
    }

    /// Is the checkbox (or radio) checked, as the person last left it?
    pub fn checked(&self, t: &str) -> bool {
        self.el(t)["checked"].as_bool().unwrap_or_else(|| panic!("data-t=\"{t}\" is no checkbox or radio"))
    }

    /// Is it disabled: by its own `disabled`, or a disabled `<fieldset>` around it?
    pub fn disabled(&self, t: &str) -> bool {
        self.el(t)["disabled"] == true
    }

    /// Is the field read-only (`readonly`)?
    pub fn readonly(&self, t: &str) -> bool {
        self.el(t)["readonly"] == true
    }

    /// Must the field be filled in (`required`)?
    pub fn required(&self, t: &str) -> bool {
        self.el(t)["required"] == true
    }

    /// Is the toggle button pressed (`aria-pressed="true"`)?
    pub fn pressed(&self, t: &str) -> bool {
        self.attr(t, "aria-pressed").as_deref() == Some("true")
    }

    /// Is the disclosure or menu open (`aria-expanded="true"`)?
    pub fn expanded(&self, t: &str) -> bool {
        self.attr(t, "aria-expanded").as_deref() == Some("true")
    }

    /// Its tag, role and accessible name.
    pub fn info(&self, t: &str) -> Info {
        let e = self.el(t);
        let s = |k: &str| e[k].as_str().unwrap_or_default().to_string();
        Info { tag: s("tag"), role: s("role"), name: s("name") }
    }

    /// What a person can see and use, one line each, nested: every showing
    /// element with a handle, a role, or one implied by its tag (controls,
    /// links, headings), as `role "name" [handle] state` (aio's `surface()`,
    /// the digest a failure prints).
    pub fn surface(&self) -> String {
        self.must("surface", json!({ "path": null, "rects": false })).as_str().unwrap_or_default().into()
    }

    /// Text a person can't read on its background (aio's contrast audit,
    /// from the computed styles): every showing element with text of its
    /// own whose colour over its backdrop is under WCAG AA (4.5:1, 3:1 for
    /// large text), as `<tag class="…"> "text" 2.10:1, needs 4.5:1`. A
    /// backdrop with an image can't be judged and is skipped, as is a
    /// subtree marked `data-aio-contrast="skip"` (an overlay on a video).
    pub fn contrast(&self) -> Vec<String> {
        let rgba = |v: &Value| -> Option<Rgba> {
            let c: Vec<f32> = v.as_array()?.iter().filter_map(|n| Some(n.as_f64()? as f32)).collect();
            Some([*c.first()?, *c.get(1)?, *c.get(2)?, *c.get(3)?])
        };
        let all = self.must("contrast", json!({}));
        let judged = all.as_array().into_iter().flatten().filter_map(|e| {
            let layers: Vec<Rgba> = e["layers"].as_array()?.iter().filter_map(rgba).collect();
            let bg = layers.into_iter().rev().fold([1.0, 1.0, 1.0, 1.0], |under, l| over(l, under));
            let fg = rgba(&e["fg"]).filter(|c| c[3] > 0.05)?;
            let need = needed(e["size"].as_f64()? as f32, e["weight"].as_f64()? as f32);
            let ratio = contrast_ratio(over(fg, bg), bg);
            let (text, place) = (cut(e["text"].as_str()?, 30), e["place"].as_str()?);
            (ratio < need).then(|| format!("{place} {text:?} {ratio:.2}:1, needs {need}:1"))
        });
        judged.collect()
    }

    /// The page's HTML, without Dioxus's ids: what the app mounted, then
    /// what it put on `<body>` itself (a tooltip, a portal).
    pub fn html(&self) -> String {
        self.must("html", json!({})).as_str().unwrap_or_default().into()
    }

    /// Write what a failure knows (why, the handles, the screen, the HTML,
    /// the page's warnings and errors) to a trace file in the test's own
    /// folder ([`trace_dir`]), keeping the newest `TRACES`; the line naming it, or "" when it can't be written (the
    /// failure itself is what matters).
    fn trace(&self, why: &str) -> String {
        if failing_on_purpose() {
            return String::new();
        }
        let read = |op: &str, arg: Value| self.js(op, arg).unwrap_or_else(|e| e.into());
        let body = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            json!({
                "why": why, "handles": read("handles", json!({})),
                "surface": read("surface", json!({ "path": null, "rects": false })),
                "html": read("html", json!({})), "console": self.page.console(),
            })
        }));
        let body = body.unwrap_or_else(|_| json!({ "why": why }));
        let dir = trace_dir();
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let file = dir.join(format!("ui-{stamp}.json"));
        let wrote = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&file, body.to_string()));
        if wrote.is_err() {
            return String::new();
        }
        let mut old: Vec<_> = std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).collect();
        old.sort();
        for p in old.iter().take(old.len().saturating_sub(TRACES)) {
            _ = std::fs::remove_file(p);
        }
        format!("\n  trace: {}", file.display())
    }

    /// The point at fractions `(fx, fy)` of the part of element `t`'s box
    /// that shows in the window (scrolled in only when none does: a hover
    /// that scrolled would close what it opened); a hidden element, or one
    /// with no box, can't be `doing`.
    fn point(&self, t: &str, fx: f64, fy: f64, doing: &str) -> (f64, f64) {
        let p = self.must("point", json!({ "path": t, "fx": fx, "fy": fy, "doing": doing }));
        (p["x"].as_f64().unwrap_or(0.0), p["y"].as_f64().unwrap_or(0.0))
    }

    /// One mouse event at `(x, y)`; `held`: the main button is down.
    fn mouse(&self, kind: &str, (x, y): (f64, f64), held: bool, mods: Modifiers, clicks: u32) {
        let button = if kind == "mouseMoved" && !held { "none" } else { "left" };
        let e = json!({ "type": kind, "x": x, "y": y, "button": button, "buttons": u8::from(held), "clickCount": clicks, "modifiers": bits(mods) });
        self.page.cmd("Input.dispatchMouseEvent", e);
    }

    /// Move the mouse over the centre of what shows of the element (shows
    /// what `:hover` reveals; nothing scrolls while part of it shows).
    pub fn hover(&mut self, t: &str) {
        let p = self.point(t, 0.5, 0.5, "hover");
        self.mouse("mouseMoved", p, false, Modifiers::empty(), 0);
        self.hovering = true;
        self.settle();
    }

    /// Click the element's centre, as a mouse would: move, press, release
    /// (pointer and mouse events, then `click`; a submit button submits its form).
    pub fn click(&mut self, t: &str) {
        self.click_with(t, Modifiers::empty());
    }

    /// Click holding modifier keys (Ctrl+click to add, Shift+click to extend).
    /// As on a Mac, Ctrl+click there is the context click (no `click`): use
    /// `Modifiers::META` (Command) for what Ctrl does elsewhere.
    pub fn click_with(&mut self, t: &str, mods: Modifiers) {
        self.click_at_with(t, 0.5, 0.5, mods);
    }

    /// Click at a point of the element, as fractions of its size (0.5, 0.5 = centre).
    /// A hidden element can't be clicked: that panics, saying why.
    pub fn click_at(&mut self, t: &str, fx: f32, fy: f32) {
        self.click_at_with(t, fx, fy, Modifiers::empty());
    }

    /// A click whose element moved between press and release (a late
    /// layout shift) went elsewhere: that fails, saying where it went.
    fn click_at_with(&mut self, t: &str, fx: f32, fy: f32, mods: Modifiers) {
        let p = self.point(t, fx.into(), fy.into(), "click");
        self.hovering = false;
        self.must("watch", json!({ "path": t }));
        self.mouse("mouseMoved", p, false, mods, 0);
        self.mouse("mousePressed", p, true, mods, 1);
        self.mouse("mouseReleased", p, false, mods, 1);
        self.must("pressed", json!({ "path": t }));
        self.settle();
    }

    /// Double click: two clicks, the second also a `dblclick` (as a browser sends them).
    pub fn dblclick(&mut self, t: &str) {
        self.click(t);
        let p = self.point(t, 0.5, 0.5, "click");
        self.mouse("mousePressed", p, true, Modifiers::empty(), 2);
        self.mouse("mouseReleased", p, false, Modifiers::empty(), 2);
        self.settle();
    }

    /// Drag with the main button held from the element's centre by
    /// `(dx, dy)` pixels: down, move, up (pointer and mouse events).
    pub fn drag_by(&mut self, t: &str, dx: f32, dy: f32) {
        self.drag_along(t, &[(dx, dy)]);
    }

    /// [`drag_by`](Self::drag_by) through each offset in turn (from the
    /// centre), released at the last: a drag that leaves the element.
    pub fn drag_along(&mut self, t: &str, path: &[(f32, f32)]) {
        let (x, y) = self.point(t, 0.5, 0.5, "drag");
        self.hovering = false;
        let none = Modifiers::empty();
        self.mouse("mouseMoved", (x, y), false, none, 0);
        self.mouse("mousePressed", (x, y), true, none, 1);
        let mut at = (x, y);
        for (dx, dy) in path {
            at = (x + f64::from(*dx), y + f64::from(*dy));
            self.mouse("mouseMoved", at, true, none, 0);
            self.settle();
        }
        self.mouse("mouseReleased", at, false, none, 1);
        self.settle();
    }

    /// Resize the window to `width`×`height` px: laid out again, and each
    /// element whose box changed size is told (`resize`, its `ResizeObserver`).
    pub fn resize(&mut self, width: u32, height: u32) {
        self.page.cmd("Emulation.setDeviceMetricsOverride", metrics((width, height)));
        // A page's own `ResizeObserver` hears of it with the next frame.
        self.page.eval(FRAMES);
        self.settle();
    }

    /// Turn the wheel over the element's centre by `(dx, dy)` pixels (`wheel`,
    /// and what scrolls under it scrolls).
    pub fn wheel(&mut self, t: &str, dx: f64, dy: f64) {
        let (x, y) = self.point(t, 0.5, 0.5, "turn the wheel over");
        self.hovering = false;
        let e = json!({ "type": "mouseWheel", "x": x, "y": y, "deltaX": dx, "deltaY": dy });
        self.page.cmd("Input.dispatchMouseEvent", e);
        self.settle();
    }

    /// HTML drag and drop of `t` onto `onto`: `dragstart`, `dragenter`,
    /// `dragover`, `drop`, `dragend`, sharing one data transfer.
    pub fn drag_to(&mut self, t: &str, onto: &str) {
        self.must("drag", json!({ "path": t, "onto": onto }));
        self.settle();
    }

    /// Scroll the element to `top`/`left` pixels (`scroll` follows). It must
    /// be able to get there: a box that doesn't scroll (`overflow` visible)
    /// or a point past its end panics, saying where it stopped.
    pub fn scroll(&mut self, t: &str, top: f64, left: f64) {
        let at = self.must("scroll", json!({ "path": t, "top": top, "left": left }));
        let n = |k: &str| at[k].as_f64().unwrap_or(0.0);
        assert!(
            (n("top") - top).abs() < 0.5 && (n("left") - left).abs() < 0.5,
            "data-t=\"{t}\" can't scroll to top {top}, left {left}: it stopped at top {}, left {} (it scrolls {}×{} px)",
            n("top"),
            n("left"),
            n("w"),
            n("h")
        );
        // The browser fires `scroll` with the next frame.
        self.page.eval(FRAMES);
        self.settle();
    }

    /// A key by its DOM name: "Enter", "Escape", "ArrowLeft", "a", " ".
    fn key(name: &str) -> Key {
        name.parse().unwrap_or_else(|_| {
            panic!("no key is named {name:?} (DOM key names: Enter, ArrowLeft, a, …; a chord: \"ctrl+n\", \"mod+shift+k\")")
        })
    }

    /// A chord as `use_global_key` takes it ("ctrl+n", "mod+shift+k", see
    /// [`ChordSpec`](crate::kit::ChordSpec)): its key and modifiers (`mod`:
    /// ⌘ on macOS, else Ctrl). A lone key ("+" too) is itself (pure).
    pub fn chord(spec: &str) -> (String, Modifiers) {
        if spec.chars().count() < 2 || !spec.contains('+') {
            return (spec.into(), Modifiers::empty());
        }
        let c = crate::kit::Chord::parse(spec).unwrap_or_else(|e| panic!("{e}"));
        let cmd = if cfg!(target_os = "macos") { Modifiers::META } else { Modifiers::CONTROL };
        let named = [(c.ctrl, Modifiers::CONTROL), (c.meta, Modifiers::META), (c.alt, Modifiers::ALT)];
        let mods = named
            .into_iter()
            .chain([(c.shift, Modifiers::SHIFT), (c.cmd, cmd)])
            .filter(|(on, _)| *on)
            .fold(Modifiers::empty(), |m, (_, b)| m | b);
        (c.key, mods)
    }

    /// Key `k` down and/or up on the focused element, as the keyboard sends it.
    fn keys(&self, k: &Key, mods: Modifiers, down: bool, up: bool) {
        let name = match k {
            Key::Character(c) if c == "\n" => "Enter".to_string(),
            Key::Character(c) => c.clone(),
            k => k.to_string(),
        };
        let b = bits(mods);
        let mut e = super::cdp_key(&name, b).unwrap_or_else(|| json!({ "key": name, "code": name, "modifiers": b }));
        if down {
            let mut d = e.clone();
            let typed = d["text"].as_str().is_some_and(|t| !t.is_empty());
            d["type"] = if typed { "keyDown" } else { "rawKeyDown" }.into();
            // Editing shortcuts as the OS's own (macOS has no Ctrl+A of its own).
            if b & 6 != 0 && name.eq_ignore_ascii_case("a") {
                d["commands"] = json!(["selectAll"]);
            }
            self.page.cmd("Input.dispatchKeyEvent", d);
        }
        if up {
            e["type"] = "keyUp".into();
            e.as_object_mut().map(|o| o.remove("text"));
            self.page.cmd("Input.dispatchKeyEvent", e);
        }
    }

    /// Press one key on the focused element (`Key::Enter`, `Key::Character("a")`, …).
    pub fn press_key(&mut self, key: Key, mods: Modifiers) {
        self.keys(&key, mods, true, true);
        self.settle();
    }

    /// Press chords one after another on the focused element as fast as a
    /// person types ahead (`["alt+e", "a", "c"]`): no step waits for the app
    /// to answer the one before, then the views settle. For what an app does
    /// with keys that come before its views caught up (aio key-settle).
    pub fn burst(&mut self, chords: &[&str]) {
        for c in chords {
            let (key, mods) = Self::chord(c);
            self.keys(&Self::key(&key), mods, true, true);
        }
        self.settle();
    }

    /// Press a key, named as the DOM names it, on element `t` (focused
    /// first, with no click: see [`focus`](Self::focus)). Enter in a
    /// form's line input submits it (its first submit
    /// button is clicked; with none, the form all the same, as aio: `aio::page::ENTER`).
    /// A chord presses its modifiers too: `press("window", "ctrl+n")`, as
    /// `ax trigger <app> window press ctrl+n` ([`chord`](Self::chord)).
    pub fn press(&mut self, t: &str, key: &str) {
        let (key, mods) = Self::chord(key);
        self.press_with(t, &key, mods);
    }

    /// `press`, holding modifiers (Ctrl+Enter; a modified Enter submits nothing).
    pub fn press_with(&mut self, t: &str, key: &str, mods: Modifiers) {
        let k = Self::key(key);
        self.focus(t);
        self.press_key(k, mods);
    }

    /// Hold a key down on `t` (no keyup until `key_up`).
    pub fn key_down(&mut self, t: &str, key: &str, mods: Modifiers) {
        let k = Self::key(key);
        self.focus(t);
        self.keys(&k, mods, true, false);
        self.settle();
    }

    /// Release a key held by `key_down`.
    pub fn key_up(&mut self, t: &str, key: &str, mods: Modifiers) {
        let k = Self::key(key);
        self.focus(t);
        self.keys(&k, mods, false, true);
        self.settle();
    }

    /// Give `t` focus as Tab would, with no click (`focus` events, and its
    /// `:focus-visible` ring shows; nothing is clicked, chosen or
    /// submitted). One that can't take focus (no
    /// control, no `tabindex`, disabled) panics, saying why; `"window"`:
    /// none has it, keys go to the document.
    pub fn focus(&mut self, t: &str) {
        self.must("focus", json!({ "path": t, "verb": "focus" }));
        self.settle();
    }

    /// The handle of the element with focus (`None`: none has, or it has no handle).
    pub fn focused(&self) -> Option<String> {
        self.must("focused", json!({})).as_str().map(String::from)
    }

    /// Take focus away from `t` (`change` if it was edited, then `blur`).
    pub fn blur(&mut self, t: &str) {
        self.must("blur", json!({ "path": t }));
        self.settle();
    }

    /// Type `text` literally, a character at a time, the app answering each
    /// before the next (as at a person's pace). A control character (`\n`,
    /// `\t`, `\u{7}`) goes in as text, as a paste would put it, never as a
    /// key: `\n` submits nothing (a line input drops it, as the browser
    /// does a pasted one). Keys are [`press`](Self::press)'s.
    fn type_chars(&mut self, text: &str) {
        for c in text.chars() {
            if c.is_control() {
                // Not `Input.insertText`: the browser takes its `\n` for Enter.
                self.page.eval(&format!("({INSERT})({})", json!(c.to_string())));
                self.settle();
            } else {
                self.press_key(Key::Character(c.to_string()), Modifiers::empty());
            }
        }
    }

    /// Type text literally at the end of `t`'s text (focused first, no
    /// click; a control character is text, never a key: see `press`).
    pub fn type_text(&mut self, t: &str, text: &str) {
        self.must("usable", json!({ "path": t, "verb": "type into" }));
        self.focus(t);
        self.must("edit", json!({ "path": t, "verb": "type into", "all": false }));
        self.type_chars(text);
    }

    /// Replace the value of `t`: focus, select all (Ctrl+A), then type
    /// `text` literally (`"a\nb"` types no Enter: `press(t, "Enter")` does).
    pub fn set_value(&mut self, t: &str, text: &str) {
        self.must("usable", json!({ "path": t, "verb": "type into" }));
        self.focus(t);
        self.keys(&Key::Character("a".into()), Modifiers::CONTROL, true, true);
        if text.is_empty() {
            // Settled: the app sees the emptied field before the next step.
            self.press_key(Key::Backspace, Modifiers::empty());
        }
        self.type_chars(text);
    }

    /// Replace the value of `t` with `text` pasted in one go (focus, select
    /// all, paste): one `paste` event carrying it (`text/plain`), then,
    /// unless the app took it (`prevent_default`), one `input`
    /// (`insertFromPaste`) with all of it, where `set_value` types a
    /// character at a time.
    pub fn paste(&mut self, t: &str, text: &str) {
        self.must("usable", json!({ "path": t, "verb": "paste into" }));
        self.focus(t);
        self.keys(&Key::Character("a".into()), Modifiers::CONTROL, true, true);
        self.page.eval(&format!("({PASTE})({})", json!(text)));
        self.settle();
    }

    /// Empty the field `t` (as select all, delete).
    pub fn clear(&mut self, t: &str) {
        self.set_value(t, "");
    }

    /// Pick the option of `<select>` `t` whose value is `value`: `input`,
    /// then `change`. An option that isn't there, or is disabled, can't be
    /// picked: that panics, naming the ones there.
    pub fn select(&mut self, t: &str, value: &str) {
        self.must("select", json!({ "path": t, "verb": "select on", "value": value }));
        self.settle();
    }

    /// Tick box `t` (a click, only when it isn't ticked).
    pub fn check(&mut self, t: &str) {
        self.set_checked(t, true);
    }

    /// Untick box `t` (a click, only when it is ticked).
    pub fn uncheck(&mut self, t: &str) {
        self.set_checked(t, false);
    }

    /// A checkbox, radio or switch (native, or a role with `aria-checked`)
    /// clicked to `on`; anything else has no checked state, and panics.
    fn set_checked(&mut self, t: &str, on: bool) {
        let verb = if on { "check" } else { "uncheck" };
        if self.must("checked", json!({ "path": t, "verb": verb })) != on {
            self.click(t);
        }
    }
}

/// Where this test's failure traces go: `<temp>/aio-traces/<test binary>/<test>`
/// (the test is its thread's name), removed once it passes.
fn trace_dir() -> std::path::PathBuf {
    let word = |s: &str| s.chars().map(|c| if c.is_alphanumeric() || c == '_' { c } else { '-' }).collect::<String>();
    let exe = std::env::current_exe().ok().and_then(|e| e.file_stem().map(|s| s.to_string_lossy().into_owned()));
    let test = std::thread::current().name().map_or_else(|| "test".into(), word);
    std::env::temp_dir().join("aio-traces").join(word(&exe.unwrap_or_default())).join(test)
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().filter_map(|s| s.as_str().map(String::from)).collect()
}

/// `s` cut to `max` chars, with `…`.
fn cut(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &s[..at]),
        None => s.into(),
    }
}

/// An sRGB colour, 0..=1 each, with alpha.
type Rgba = [f32; 4];

/// `fg` drawn over the opaque `bg`.
fn over(fg: Rgba, bg: Rgba) -> Rgba {
    let a = fg[3];
    [fg[0] * a + bg[0] * (1.0 - a), fg[1] * a + bg[1] * (1.0 - a), fg[2] * a + bg[2] * (1.0 - a), 1.0]
}

/// WCAG's contrast ratio of two opaque colours (1 to 21).
fn contrast_ratio(a: Rgba, b: Rgba) -> f32 {
    let lum = |c: Rgba| {
        let f = |v: f32| if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
        0.2126 * f(c[0]) + 0.7152 * f(c[1]) + 0.0722 * f(c[2])
    };
    let (x, y) = (lum(a), lum(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// The ratio WCAG AA asks of text this size (px) and weight: 3 for large
/// (24 px, or 18.66 px bold), else 4.5.
fn needed(size: f32, weight: f32) -> f32 {
    if size >= 24.0 || (size >= 18.66 && weight >= 700.0) { 3.0 } else { 4.5 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contrast_is_wcags_ratio_and_large_text_needs_less() {
        let (black, white) = ([0.0, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0, 1.0]);
        assert!((contrast_ratio(black, white) - 21.0).abs() < 0.01);
        assert!((contrast_ratio(white, white) - 1.0).abs() < 0.01);
        let grey = 0x77 as f32 / 255.0;
        assert!((contrast_ratio([grey, grey, grey, 1.0], white) - 4.48).abs() < 0.01, "#777 just fails AA");
        assert_eq!(over([0.0, 0.0, 0.0, 0.5], white), [0.5, 0.5, 0.5, 1.0]);
        assert_eq!(
            (needed(16.0, 400.0), needed(24.0, 400.0), needed(19.0, 700.0), needed(19.0, 400.0)),
            (4.5, 3.0, 3.0, 4.5)
        );
    }

    #[test]
    fn modifiers_are_counted_as_the_devtools_protocol_counts_them() {
        assert_eq!(bits(Modifiers::empty()), 0);
        assert_eq!(bits(Modifiers::CONTROL | Modifiers::SHIFT), 10);
        assert_eq!(bits(Modifiers::ALT | Modifiers::META), 5);
    }
}

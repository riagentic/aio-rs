//! The kit's islands (`aio::kit::WebIsland`) in the real page: the window's
//! own HTML, CSP and air's interpreter, in a headless Chromium, its frames
//! carried over the DevTools protocol where the preload would carry them.
use crate::{Window, page_with};
use aio::kit::{Island, WebIsland};
use aio::serde_json::{Value, json};
use aio::ui::prelude::*;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

/// `window.__aioWire` for the test: frames in by `__aioIn(base64)`, out
/// queued for `__aioOut()`.
const BRIDGE: &str = "window.__aioWire = (() => {
  let on = null;
  const early = [], out = [];
  const bytes = (s) => { const b = atob(s), u = new Uint8Array(b.length); for (let i = 0; i < b.length; i++) u[i] = b.charCodeAt(i); return u; };
  const text = (u) => { let s = ''; for (let i = 0; i < u.length; i++) s += String.fromCharCode(u[i]); return btoa(s); };
  window.__aioIn = (s) => { const f = bytes(s); on ? on(f) : early.push(f); };
  window.__aioOut = () => out.splice(0);
  return { send: (u) => out.push(text(u)), listen: (f) => { on = f; early.splice(0).forEach(f); } };
})();";

/// A counter drawn by plain JS: a button showing `n`, which emits a bump.
/// `window.__t` counts what the page did.
const COUNTER: Island = Island::js(
    "const t = window.__t = { loads: (window.__t ? window.__t.loads : 0) + 1, mounts: 0, updates: 0, unmounts: 0 };
     return (el, p, emit) => {
       t.mounts++;
       const b = document.createElement('button');
       b.className = 'n';
       b.textContent = 'n=' + p.n;
       b.onclick = () => emit({ by: 2 });
       el.appendChild(b);
       return { update: (q) => { t.updates++; b.textContent = 'n=' + q.n; }, unmount: () => { t.unmounts++; } };
     };",
);

/// An action on its box (aio's `use=`): a title, its cleanup the function it
/// returns; no `update`, so a prop change cleans up and applies it anew.
const TIP: Island = Island::js(
    "const t = window.__tip = { on: 0, off: 0 };
     return (el, p) => { t.on++; el.title = p.label; return () => { t.off++; el.title = ''; }; };",
);

/// Another action, for a box whose script changes.
const MARK: Island = Island::js("return (el) => { el.dataset.mark = 'b'; };");

/// What an island can reach: the boundary. It runs in the page itself, as
/// the app's own code, so it shares the page's DOM and its wire to the app
/// (`__aioWire`); the page's policy holds for it as for the page: no
/// `eval`, no `Function`, no connection, no script from elsewhere. An
/// island is the app's trust, never a third party's: a foreign page is a
/// `Browser`, in a process and partition of its own.
const BOUNDARY: Island = Island::js(
    "const can = (f) => { try { f(); return true; } catch (e) { return false; } };
     const b = window.__boundary = { eval: can(() => eval('1')), fn: can(() => new Function('return 1')()),
       wire: typeof window.__aioWire === 'object', dom: !!document.getElementById('main') };
     fetch('https://example.com/').then(() => { b.fetch = true; }, () => { b.fetch = false; });
     const s = document.createElement('script');
     s.src = 'https://example.com/x.js';
     s.onload = () => { b.remote = true; };
     s.onerror = () => { b.remote = false; };
     document.head.appendChild(s);
     return () => {};",
);

/// A library the app doesn't vouch for, in a sandboxed frame: it draws in
/// its own document and says what it can reach (nothing of the page's).
const SANDBOXED: Island = Island::js(
    "const can = (f) => { try { f(); return true; } catch (e) { return false; } };
     return (el, p, emit) => {
       const b = document.createElement('b');
       b.textContent = 'n=' + p.n;
       el.appendChild(b);
       emit({ n: p.n, dom: can(() => parent.document.body.id), wire: can(() => parent.__aioWire.send),
         ipc: can(() => parent.ipc.postMessage('x')), store: can(() => localStorage.length), drawn: el.textContent });
       return { update: (q) => emit({ n: q.n }) };
     };",
)
.sandboxed();

/// Its `mount` throws: the box keeps its placeholder.
const BROKEN: Island = Island::js("return () => { throw new Error('no canvas'); };");

/// Its script throws: nothing mounts.
const UNLOADED: Island = Island::js("throw new Error('bad bundle');");

/// React as aio tests it: stand-ins for React and `react-dom/client` (an app
/// bundles the real ones), the component a module with `default`.
const REACT: Island = Island::react(
    "const react = { createElement: (type, props) => ({ type, props }) };
     const reactDomClient = { createRoot: (el) => ({
       render: (node) => { const p = node.type(node.props); el.textContent = 'react:' + p.label; el.onclick = () => p.emit({ by: 10 }); },
       unmount: () => { window.__reactGone = true; } }) };
     return { react, reactDomClient, component: { default: (p) => p } };",
);

#[derive(serde::Serialize, Clone, PartialEq)]
struct Count {
    n: i64,
}

#[derive(serde::Deserialize)]
struct Bump {
    by: i64,
}

#[derive(serde::Serialize, Clone, PartialEq)]
struct Label {
    label: String,
}

fn app() -> View {
    let mut n = signal(|| 0i64);
    let on = signal(|| true);
    let probe = signal(String::new);
    rsx! {
        button { id: "hide", onclick: move |_| on.toggle(), "hide" }
        if on.get() {
            WebIsland {
                id: "counter",
                island: COUNTER,
                props: Count { n: n.get() },
                onevent: move |b: Bump| n += b.by,
                loading: rsx! { i { "loading" } },
            }
            WebIsland::<Count, Bump> { id: "fixed", island: COUNTER, props: Count { n: 100 } }
            WebIsland {
                id: "react",
                island: REACT,
                props: Label { label: format!("hi {}", n.get()) },
                onevent: move |b: Bump| n += b.by,
            }
            // Actions (aio's `use=`): the view's own children inside.
            WebIsland::<Label, ()> {
                id: "tip",
                island: TIP,
                props: Label { label: format!("n is {}", n.get()) },
                span { id: "inside", "{n}" }
            }
            WebIsland::<Label, ()> {
                id: "swap",
                island: if n.get() == 0 { TIP } else { MARK },
                props: Label { label: "swap".into() },
            }
            WebIsland::<Count, Bump> { id: "boundary", island: BOUNDARY, props: Count { n: 0 } }
            WebIsland {
                id: "sandboxed",
                island: SANDBOXED,
                props: Count { n: n.get() },
                onevent: move |v: Value| probe.set(v.to_string()),
            }
        }
        p { id: "probe", "{probe}" }
        WebIsland::<Count, Bump> {
            id: "broken",
            island: BROKEN,
            props: Count { n: 0 },
            loading: rsx! { i { "still" } },
        }
        WebIsland::<Count, Bump> { id: "unloaded", island: UNLOADED, props: Count { n: 0 } }
    }
}

/// A headless Chromium showing `file` (`aio::testing::TestBrowser`: a
/// profile of its own, off the person's display and bus), ended when dropped.
struct Chromium(aio::testing::TestBrowser);

impl Chromium {
    fn open(file: &std::path::Path) -> Option<Self> {
        aio::testing::TestBrowser::open(&format!("file://{}", file.display()))
            .map(Self)
            .map_err(|e| eprintln!("{e}"))
            .ok()
    }

    /// The page's DevTools socket, once it shows the file.
    fn page(&self) -> String {
        for _ in 0..200 {
            let list = targets(self.0.port()).unwrap_or_default();
            let page =
                list.iter().find(|t| t["type"] == "page" && t["url"].as_str().is_some_and(|u| u.starts_with("file:")));
            if let Some(ws) = page.and_then(|p| p["webSocketDebuggerUrl"].as_str()) {
                return ws.to_string();
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("Chromium shows no page")
    }
}

/// The browser's targets (`/json/list`).
fn targets(port: u16) -> Option<Vec<Value>> {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    // HTTP/1.1 (Chromium answers 1.0 with nothing), read to its length (it
    // keeps the connection open).
    write!(s, "GET /json/list HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n").ok()?;
    let (mut got, mut buf) = (Vec::new(), [0; 8192]);
    loop {
        let n = s.read(&mut buf).ok()?;
        got.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&got);
        let (head, body) = text.split_once("\r\n\r\n").unwrap_or_default();
        let len =
            head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:")?.trim().parse().ok());
        if n == 0 || len.is_some_and(|l: usize| body.len() >= l) {
            return aio::serde_json::from_str(body).ok();
        }
    }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// `expr`'s value in the page.
async fn eval(ws: &mut Ws, id: &mut u64, expr: &str) -> Value {
    *id += 1;
    let m = json!({ "id": *id, "method": "Runtime.evaluate", "params": { "expression": expr, "returnByValue": true } });
    ws.send(Message::text(m.to_string())).await.unwrap();
    while let Some(Ok(m)) = ws.next().await {
        let Message::Text(t) = m else { continue };
        let v: Value = aio::serde_json::from_str(&t).unwrap();
        if v["id"] == *id {
            assert!(v["result"]["exceptionDetails"].is_null(), "{expr}: {v}");
            return v["result"]["result"]["value"].clone();
        }
    }
    panic!("the DevTools socket closed")
}

/// The page's islands through a view and back: mounted with props, updated
/// in place on a change, events typed into the view, unmounted with the box;
/// a script loaded once per page; a `mount` that throws keeps the placeholder.
#[tokio::test(flavor = "multi_thread")]
async fn an_island_mounts_updates_emits_and_unmounts_in_the_real_page() {
    let said = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let keep = said.clone();
    let before = aio::log::set_sink(Some(std::sync::Arc::new(move |_, l: &str| keep.lock().unwrap().push(l.into()))));
    let d = tempfile::tempdir().unwrap();
    let file = d.path().join("index.html");
    std::fs::write(&file, page_with(&Window { title: "islands", ..Default::default() }, BRIDGE)).unwrap();
    let Some(chromium) = Chromium::open(&file) else { return eprintln!("skipped: no Chromium") };
    let (mut ws, _) = tokio_tungstenite::connect_async(chromium.page()).await.unwrap();
    let mut id = 0;
    for _ in 0..200 {
        if eval(&mut ws, &mut id, "!!window.__aioIn").await == json!(true) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let (view, ours) = tokio::io::duplex(1 << 20);
    tokio::spawn(async move { _ = crate::views::Views::new().launch(Box::new(view), app).await });
    let codec = LengthDelimitedCodec::builder().max_frame_length(256 << 20).new_codec();
    let (mut to_view, mut from_view) = StreamExt::split::<bytes::Bytes>(Framed::new(ours, codec));
    let b64 = base64::engine::general_purpose::STANDARD;
    // Carry frames both ways until `expr` is true (by time: a busy machine
    // takes longer per round).
    let mut until = async |expr: &str| {
        let by = std::time::Instant::now() + Duration::from_secs(20);
        while std::time::Instant::now() < by {
            while let Ok(Some(Ok(f))) = tokio::time::timeout(Duration::from_millis(5), from_view.next()).await {
                eval(&mut ws, &mut id, &format!("__aioIn({})", json!(b64.encode(&f)))).await;
            }
            for f in eval(&mut ws, &mut id, "__aioOut()").await.as_array().unwrap() {
                to_view.send(bytes::Bytes::from(b64.decode(f.as_str().unwrap()).unwrap())).await.unwrap();
            }
            if eval(&mut ws, &mut id, expr).await == json!(true) {
                return;
            }
        }
        panic!("never true: {expr}");
    };
    let text = |box_id: &str| format!("document.getElementById('{box_id}')?.textContent");
    until(&format!("{} === 'n=0' && {} === 'n=100'", text("counter"), text("fixed"))).await;
    until("window.__t.mounts === 2 && window.__t.loads === 1").await;
    until(&format!("{} === 'react:hi 0'", text("react"))).await;
    let tip = "document.getElementById('tip').title";
    until(&format!("{tip} === 'n is 0' && {} === '0' && window.__tip.on === 2", text("inside"))).await;
    until("document.getElementById('swap').title === 'swap'").await;
    // A sandboxed island: its own frame and origin, none of the page's
    // DOM, wire, bridge or storage; it draws, and its events flow.
    until("document.querySelector('#sandboxed iframe')?.getAttribute('sandbox') === 'allow-scripts'").await;
    let probe = "document.getElementById('probe').textContent";
    until(&format!(
        "{probe} === JSON.stringify({{ dom: false, drawn: 'n=0', ipc: false, n: 0, store: false, wire: false }})"
    ))
    .await;
    // An event from the JS, typed in the view: its prop changes in place.
    until("document.querySelector('#counter .n').click() || true").await;
    until(&format!("{} === 'n=2' && {} === 'react:hi 2'", text("counter"), text("react"))).await;
    until("window.__t.updates === 1 && window.__t.mounts === 2 && window.__t.unmounts === 0").await;
    // An action with no update: cleaned up and applied anew, the view's
    // children kept; a new script: the old one's cleanup, then the new one.
    until(&format!("{tip} === 'n is 2' && {} === '2'", text("inside"))).await;
    until("document.getElementById('swap').dataset.mark === 'b' && document.getElementById('swap').title === ''").await;
    until("window.__tip.on === 3 && window.__tip.off === 2").await;
    until("document.getElementById('react').click() || true").await;
    until(&format!("{} === 'n=12' && window.__tip.on === 4 && window.__tip.off === 3", text("counter"))).await;
    until(&format!("{probe} === '{{\"n\":12}}'")).await;
    // The placeholder went once mounted; a mount that threw keeps its own.
    until(&format!("!document.querySelector('#counter i') && {} === 'still'", text("broken"))).await;
    // The boundary: the page's DOM and wire, under the page's policy.
    let b = "window.__boundary";
    until(&format!("{b} && {b}.fetch === false && {b}.remote === false")).await;
    until(&format!("!{b}.eval && !{b}.fn && {b}.wire && {b}.dom")).await;
    // Only a sandboxed island's own frame is heard: a message from elsewhere is no event.
    let seen = "(window.__probes = [], new MutationObserver(() => __probes.push(document.getElementById('probe').textContent))\
                .observe(document.getElementById('probe'), { childList: true, subtree: true, characterData: true }), true)";
    until(seen).await;
    until("window.postMessage({ t: 'event', v: { n: 99 } }, '*') || true").await;
    until("document.querySelector('#counter .n').click() || true").await;
    until(&format!("{probe} === '{{\"n\":14}}' && __probes.length > 0 && !__probes.some((t) => t.includes('99'))"))
        .await;
    // The boxes go: each island's unmount.
    until("document.getElementById('hide').click() || true").await;
    until("window.__t.unmounts === 2 && window.__reactGone === true && !document.getElementById('counter')").await;
    until("window.__tip.off === 5").await;
    until("Object.keys(window.__aioIslands.live).length === 0 && !document.querySelector('iframe')").await;
    // Which half failed, in the app's log.
    aio::log::set_sink(before);
    let said = said.lock().unwrap().join("\n");
    assert!(said.contains("[island] its script loaded but mount() threw: Error: no canvas"), "{said}");
    assert!(said.contains("[island] the script failed to load: Error: bad bundle"), "{said}");
}

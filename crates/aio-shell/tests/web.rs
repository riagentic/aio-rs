//! A view served to browser tabs from the process (aio's tools: `ax ui
//! --browser`), in a real browser (a headless Chromium; skipped without
//! one): each tab its own view, under the page's strict policy. And a
//! server's first paint hydrated by air's interpreter (`air::ssr`).
#![cfg(unix)]
use aio::prelude::*;
use aio::serde_json::{Value, json};
use aio::sync::{Options, serve_with};
use aio::ui::prelude::*;
use futures_util::{SinkExt, StreamExt};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

/// Which view this is: each tab's view (a reconnect's too) gets the next.
static VIEWS: AtomicU32 = AtomicU32::new(0);

/// A view of aio's tools in a tab: its own state, a field the view focuses
/// (a query the page runs), and markup a sanitizer missed.
fn app() -> View {
    let view = once(|| VIEWS.fetch_add(1, Ordering::SeqCst) + 1);
    let n = signal(|| 0);
    rsx! {
        p { id: "view", "{view}" }
        p { id: "n", "{n}" }
        button { id: "add", onclick: move |_| n += 1, "+" }
        input { id: "field", onmounted: move |e| async move { _ = e.set_focus(true).await } }
        div { dangerous_inner_html: "<img id=\"bad\" src=\"data:x\" onerror=\"window.ran = 1\">" }
        a { id: "go", href: "/elsewhere", "go" }
    }
}

/// `f`, failed after a minute: a stuck test still ends its Chromium.
async fn within(f: impl Future<Output = ()>) {
    tokio::time::timeout(Duration::from_secs(60), f).await.expect("the test took over a minute");
}

const KEY: &str = "a-long-random-app-key-for-tests";

/// The view served to tabs on a free port.
async fn serve() -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let http =
        aio_shell::web::tabs(Default::default(), app, aio_shell::Window { title: "Tools", ..Default::default() });
    tokio::spawn(serve_with(Handle::<Desk>::memory(), l, Options { token: KEY.into(), http, ..Default::default() }));
    addr
}

#[cell(shared)]
pub struct Desk {}

#[methods]
impl Desk {}

/// One plain HTTP/1.1 request: the status, the head, the body.
async fn get(addr: &str, path: &str, cookie: Option<&str>) -> (u16, String, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    let cookie = cookie.map(|c| format!("Cookie: {c}\r\n")).unwrap_or_default();
    let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{cookie}Connection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    // To its length: Chromium keeps the connection open.
    let (mut got, mut buf) = (Vec::new(), [0; 8192]);
    loop {
        let n = s.read(&mut buf).await.unwrap();
        got.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&got).into_owned();
        let whole = text.split_once("\r\n\r\n").is_some_and(|(head, body)| {
            let len = head.lines().find_map(|l| {
                let (k, v) = l.split_once(':')?;
                k.eq_ignore_ascii_case("content-length").then(|| v.trim().parse::<usize>().ok())?
            });
            len.is_some_and(|len| body.len() >= len)
        });
        if n == 0 || whole {
            let (head, body) = text.split_once("\r\n\r\n").unwrap();
            return (head[9..12].parse().unwrap(), head.to_ascii_lowercase(), body.into());
        }
    }
}

/// A headless Chromium on `url` (`aio::testing::TestBrowser`: a profile of
/// its own, off the person's display and bus), ended when dropped; `None`
/// without one.
struct Chromium(aio::testing::TestBrowser);

impl Chromium {
    fn open(url: &str) -> Option<Self> {
        aio::testing::TestBrowser::open(url).map(Self).map_err(|e| eprintln!("{e}")).ok()
    }

    /// Its page's DevTools session.
    async fn page(&self) -> Page {
        for _ in 0..200 {
            let (_, _, body) = get(&format!("127.0.0.1:{}", self.0.port()), "/json/list", None).await;
            let list: Vec<Value> = aio::serde_json::from_str(&body).unwrap_or_default();
            if let Some(p) = list.iter().find(|t| t["type"] == "page") {
                // Its URL names the host as asked (no port): the path is what counts.
                let at = p["webSocketDebuggerUrl"].as_str().unwrap();
                let path = &at[at.find("/devtools/").unwrap()..];
                let (ws, _) =
                    tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{}{path}", self.0.port())).await.unwrap();
                return Page { ws, id: 0 };
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("Chromium shows no page");
    }
}

struct Page {
    ws: tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    id: u64,
}

impl Page {
    /// `expr`'s value in the page (`null` on an exception).
    async fn eval(&mut self, expr: &str) -> Value {
        let r = self.call("Runtime.evaluate", json!({ "expression": expr, "returnByValue": true })).await;
        r["result"]["value"].clone()
    }

    /// DevTools `method`'s result.
    async fn call(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        let m = json!({ "id": self.id, "method": method, "params": params });
        self.ws.send(Message::text(m.to_string())).await.unwrap();
        loop {
            let Message::Text(t) = self.ws.next().await.unwrap().unwrap() else { continue };
            let v: Value = aio::serde_json::from_str(&t).unwrap();
            if v["id"] == self.id {
                return v["result"].clone();
            }
        }
    }

    /// Waits until `expr` is `want` (10 s at most).
    async fn until(&mut self, expr: &str, want: Value) {
        let mut last = Value::Null;
        for _ in 0..400 {
            last = self.eval(expr).await;
            if last == want {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let page = self.eval("document.body.innerHTML").await;
        panic!("{expr}: {last} (wanted {want}); the page: {page}");
    }

    async fn text(&mut self, id: &str, want: &str) {
        self.until(&format!("document.getElementById('{id}')?.textContent"), json!(want)).await;
    }
}

/// Counts the page's policy refusals, from its first script on.
const WATCH: &str = "window.refused = []; document.addEventListener('securitypolicyviolation', \
                     (e) => window.refused.push(e.violatedDirective + ' ' + e.blockedURI))";

#[tokio::test(flavor = "multi_thread")]
async fn a_tab_shows_the_view_runs_its_queries_and_nothing_injected() {
    within(async {
        let addr = serve().await;
        // Behind the key: a link with it sets the cookie the page's socket shows.
        let (status, ..) = get(&addr, "/", None).await;
        assert_eq!(status, 401);
        let (status, head, body) = get(&addr, &format!("/deep/link?token={KEY}"), None).await;
        assert_eq!(status, 200, "a deep link is the page too");
        assert!(head.contains("set-cookie: aio_key_desk=") && head.contains("frame-ancestors 'self'"), "{head}");
        assert!(body.contains("/__aio/ui") && body.contains("connect-src 'self'"), "{body}");
        assert!(!body.contains("'unsafe-eval'") && !body.contains("script-src 'unsafe-inline'"), "{body}");
        let Some(c) = Chromium::open("about:blank") else {
            return eprintln!("no Chromium: skipped");
        };
        let mut p = c.page().await;
        p.call("Page.enable", json!({})).await;
        p.call("Page.addScriptToEvaluateOnNewDocument", json!({ "source": WATCH })).await;
        p.call("Page.navigate", json!({ "url": format!("http://{addr}/?token={KEY}") })).await;
        p.text("n", "0").await;
        p.eval("document.getElementById('add').click()").await;
        p.text("n", "1").await;
        // The view's focus is a query: the page ran it as a script of its own.
        p.until("document.activeElement && document.activeElement.id", json!("field")).await;
        // Markup a sanitizer missed runs nothing: its handler is refused.
        p.until("window.refused.length > 0", json!(true)).await;
        assert_eq!(p.eval("window.ran").await, Value::Null);
        let refused = p.eval("window.refused").await;
        let refused = refused.as_array().unwrap();
        assert!(refused.iter().all(|r| r == "script-src-attr inline"), "only the injected handler: {refused:?}");
        p.until("document.documentElement.dataset.aioLink", json!("online")).await;
        // A link is a link: its deep link is the app again, as the cookie lets it.
        p.eval("document.getElementById('go').click()").await;
        p.until("location.pathname", json!("/elsewhere")).await;
        p.until("!!document.getElementById('n')", json!(true)).await;
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tab_reconnects_and_starts_again_from_the_server() {
    within(async {
        let server = serve().await;
        // The network between: dropped (every connection cut), then back on its port.
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let net = |l: tokio::net::TcpListener, to: String| {
            tokio::spawn(async move {
                let mut links = tokio::task::JoinSet::new();
                while let Ok((mut a, _)) = l.accept().await {
                    let to = to.clone();
                    links.spawn(async move {
                        let mut b = tokio::net::TcpStream::connect(to).await.unwrap();
                        _ = tokio::io::copy_bidirectional(&mut a, &mut b).await;
                    });
                }
            })
        };
        let up = net(l, server.clone());
        let Some(c) = Chromium::open(&format!("http://{addr}/?token={KEY}")) else {
            return eprintln!("no Chromium: skipped");
        };
        let mut p = c.page().await;
        p.until("!!document.getElementById('view')", json!(true)).await;
        let first = p.eval("document.getElementById('view').textContent").await;
        up.abort();
        p.until("document.documentElement.dataset.aioLink", json!("offline")).await;
        let _back = net(tokio::net::TcpListener::bind(&addr).await.unwrap(), server);
        p.until("document.documentElement.dataset.aioLink", json!("online")).await;
        // A new view, the server's.
        let other =
            format!("document.getElementById('view') && document.getElementById('view').textContent !== {first}");
        p.until(&other, json!(true)).await;
    })
    .await
}

/// What a page's first paint and its view's first frame are made of: the
/// server's (`painted`) or the client's, which may differ.
fn shown(painted: bool) -> View {
    let n = signal(move || if painted { 3 } else { 5 });
    let typed = signal(String::new);
    // A motion that measures its element (`--aio-w`, `--aio-h`).
    let motion = signal(|| "display: block; width: 40px; height: 10px; animation: grow 1s".to_string());
    rsx! {
        h1 { id: "n", "{n} items" }
        button { id: "add", onclick: move |_| n += 1, "+" }
        input { id: "name", oninput: move |e| typed.set(e.value()) }
        p { id: "typed", "{typed}" }
        p { "a" {typed.get()} }
        if painted { p { id: "differs", "server" } } else { div { id: "differs", "client" } }
        div { id: "moves", style: "{motion}" }
    }
}

/// The page's frames (events) for the view, from its wire.
async fn sent(p: &mut Page) -> Vec<Vec<u8>> {
    let frames = p.eval("window.sent.splice(0)").await;
    aio::serde_json::from_value(frames).unwrap()
}

/// `frame` applied by the page's interpreter.
async fn apply(p: &mut Page, frame: &[u8]) {
    p.eval(&format!("window.__airApply(new Uint8Array({})); true", aio::serde_json::to_string(frame).unwrap())).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_painted_page_is_hydrated_in_place_its_nodes_kept_and_wired() {
    within(async {
        let dir = tempfile::tempdir().unwrap();
        let painted = air::ssr::render_to_string(|| shown(true)).unwrap();
        let page = format!(
            "<!doctype html><html><body><div id=\"main\" data-aio-painted>{painted}</div>\
             <script src=\"wire.js\"></script><script src=\"page.js\"></script></body></html>"
        );
        std::fs::write(dir.path().join("index.html"), page).unwrap();
        let wire = "window.sent = []; window.__aioWire = { send: (b) => window.sent.push([...b]), listen: () => {} }";
        std::fs::write(dir.path().join("wire.js"), wire).unwrap();
        std::fs::write(dir.path().join("page.js"), air::PAGE_JS).unwrap();
        let Some(c) = Chromium::open(&format!("file://{}", dir.path().join("index.html").display())) else {
            return eprintln!("no Chromium: skipped");
        };
        let mut p = c.page().await;
        p.until("typeof window.__airApply", json!("function")).await;
        // The paint as the server sent it; someone types before the view runs.
        p.eval(
            "window.before = [...document.querySelectorAll('#main *')]; document.getElementById('name').value = 'hey'",
        )
        .await;
        let root = air::Root::mount(|| shown(false));
        apply(&mut p, &root.take_edits()).await;
        // Every element kept, but the one of another kind; its text written.
        let kept = "[...document.querySelectorAll('#main *')].map((n, i) => n === window.before[i])";
        assert_eq!(p.eval(kept).await, json!([true, true, true, true, true, false, true]));
        // A motion on a kept node is measured there, before its first frame.
        let measured = "[document.getElementById('moves') === window.before[6], \
                        document.getElementById('moves').style.getPropertyValue('--aio-w')]";
        assert_eq!(p.eval(measured).await, json!([true, "40px"]));
        assert_eq!(p.eval("window.__airHydrated.replaced").await, json!(1));
        p.text("n", "5 items").await;
        p.text("differs", "client").await;
        assert_eq!(p.eval("document.getElementById('main').innerHTML.includes('<!--a-->')").await, json!(false));
        assert_eq!(p.eval("document.getElementById('main').hasAttribute('data-aio-painted')").await, json!(false));
        // What was typed stays, and is told to the view.
        assert_eq!(p.eval("document.getElementById('name').value").await, json!("hey"));
        for f in sent(&mut p).await {
            assert!(root.event(&f));
        }
        apply(&mut p, &root.take_edits()).await;
        p.text("typed", "hey").await;
        assert_eq!(p.eval("window.before[3] === document.getElementById('typed')").await, json!(true));
        // The kept button's click reaches the view, which writes the kept heading.
        p.eval("document.getElementById('add').click()").await;
        let clicks = sent(&mut p).await;
        assert_eq!(clicks.len(), 1);
        assert!(root.event(&clicks[0]));
        apply(&mut p, &root.take_edits()).await;
        p.text("n", "6 items").await;
        assert_eq!(p.eval("window.before[0] === document.getElementById('n')").await, json!(true));
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dev_builds_page_hydrates_with_no_node_replaced_its_tooling_beside_the_app() {
    within(async {
        let dir = tempfile::tempdir().unwrap();
        let painted = air::ssr::render_to_string(|| shown(true)).unwrap();
        let page = format!(
            "<!doctype html><html><body><div id=\"main\" data-aio-painted>{painted}</div>\
             <script src=\"wire.js\"></script><script src=\"page.js\"></script></body></html>"
        );
        std::fs::write(dir.path().join("index.html"), page).unwrap();
        let wire = "window.__aioWire = { send: () => {}, listen: () => {} }";
        std::fs::write(dir.path().join("wire.js"), wire).unwrap();
        std::fs::write(dir.path().join("page.js"), air::PAGE_JS).unwrap();
        let Some(c) = Chromium::open(&format!("file://{}", dir.path().join("index.html").display())) else {
            return eprintln!("no Chromium: skipped");
        };
        let mut p = c.page().await;
        p.until("typeof window.__airApply", json!("function")).await;
        // As `aio::web::launch` mounts a dev build: the app, its tooling beside it.
        let root = air::Root::new();
        root.show(|| shown(true));
        root.beside(|| rsx! { aio::kit::ReduxDevTools {} });
        apply(&mut p, &root.take_edits()).await;
        assert_eq!(p.eval("window.__airHydrated").await, json!({ "kept": 17, "replaced": 0 }));
        // The tooling in the view's own markup (its old place) replaced nodes.
        p.eval("location.reload(); true").await;
        p.until("typeof window.__airApply", json!("function")).await;
        let inside = air::Root::mount(|| {
            let app = shown(true);
            rsx! { {app} aio::kit::ReduxDevTools {} }
        });
        apply(&mut p, &inside.take_edits()).await;
        assert_ne!(p.eval("window.__airHydrated.replaced").await, json!(0));
    })
    .await
}

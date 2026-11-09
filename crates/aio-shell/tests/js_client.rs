//! aio-rs's JS client (`crates/aio/client`) for other view libraries, on
//! real React, Solid, Svelte and Vue, against a real app:
//! - in a browser (a headless Chromium): each an island of a served aio-rs
//!   page, on the cell the page's own view shows; a page of plain DOM, its
//!   calls kept offline across a reload and shared with a twin tab;
//! - in the desktop window's page (`TestUi`, the Electron runtime), whose
//!   page connects nowhere: over the window's own wire (`use_js_client`).
//!
//! And the client's own unit tests and type check (`node --test`, `tsc`).
//! They need the client's dev packages (`npm ci` in `crates/aio/client`)
//! and a Chromium; without them they are skipped, said, unless
//! `AIO_CI_WEB` is set: then they fail.
#![cfg(unix)]
use aio::kit::Island;
use aio::prelude::*;
use aio::serde_json::{Value, json};
use aio::sync::{Options, serve_with};
use futures_util::{SinkExt, StreamExt};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

#[cell(shared)]
pub struct Tally {
    pub n: i64,
    pub label: String,
}

#[methods]
impl Tally {
    #[client]
    pub fn add(&mut self, by: i64) -> Result<i64> {
        self.n += by;
        Ok(self.n)
    }

    #[client]
    pub fn rename(&mut self, label: String) {
        self.label = label;
    }

    /// Not `#[client]`: a page's call is refused.
    pub fn reset(&mut self) {
        self.n = 0;
    }
}

/// The islands' scripts (React, Solid, Svelte, Vue), bundled once.
static ISLANDS: std::sync::OnceLock<Option<[Island; 4]>> = std::sync::OnceLock::new();

fn islands() -> [Island; 4] {
    ISLANDS.get().copied().flatten().expect("bundled")
}

/// The page's own view and the four islands, on the module's cell.
macro_rules! page_view {
    () => {{
        use_live();
        let n = part(|t| &t.n);
        let [react, solid, svelte, vue] = super::islands();
        rsx! {
            p { id: "rs-n", "data-t": "rs-n", "{n}" }
            button { id: "rs-add", "data-t": "rs-add", onclick: |_| _ = aio::act!(add(1)), "+1" }
            WebIsland::<(), ()> { island: react, props: (), id: "react" }
            WebIsland::<(), ()> { island: solid, props: (), id: "solid" }
            WebIsland::<(), ()> { island: svelte, props: (), id: "svelte" }
            WebIsland::<(), ()> { island: vue, props: (), id: "vue" }
        }
    }};
}

/// The served app's view (a browser tab).
mod tab {
    use super::TallyCalls;
    use aio::kit::WebIsland;
    use aio::ui::prelude::*;
    aio::client!(super::Tally, tally);

    pub fn app() -> View {
        page_view!()
    }
}

/// The desktop app's view: the window's page reaches the cell over its wire.
mod window {
    use super::TallyCalls;
    use aio::kit::{JsCell, WebIsland, use_js_client};
    use aio::ui::prelude::*;
    aio::client!(super::Tally, tally);

    pub fn app() -> View {
        use_js_client([JsCell::from(HANDLE.read_untracked().clone())]);
        page_view!()
    }
}

/// A `client` app's window: its cell's copy is its server's, and so is
/// what the page's client reaches.
mod client_window {
    use super::TallyCalls;
    use aio::kit::{JsCell, WebIsland, use_js_client};
    use aio::ui::prelude::*;
    aio::client!(super::Tally, tally);

    pub fn app() -> View {
        use_js_client([JsCell::from(HANDLE.read_untracked().clone())]);
        page_view!()
    }
}

/// A page of no library: the served client, imported by a module script;
/// its copy kept in `localStorage`, its call shown by a predictor;
/// `?offline`: a server that never answers.
const PLAIN: &str = r#"<!doctype html><meta charset="utf-8"><p id="plain"></p><button id="plain-add">+1</button>
<script type="module">
  import { connect } from "/__aio/client.js";
  const offline = new URLSearchParams(location.search).has("offline");
  const c = connect({ app: "plain", ...(offline ? { url: "ws://127.0.0.1:9/ws?wire=2" } : {}) }).cell({ id: "tally", v: 1 });
  c.predict({ add: (s, by) => ({ ...s, n: s.n + by }) });
  c.select((s) => s.n).subscribe((n) => { document.getElementById("plain").textContent = "plain:" + n; });
  document.getElementById("plain-add").onclick = () => c.call("add", [1]);
</script>"#;

const KEY: &str = "a-long-random-app-key-for-tests";

fn client_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../aio/client")
}

/// Skipped, said; a failure under `AIO_CI_WEB`.
fn skip(why: &str) {
    assert!(std::env::var_os("AIO_CI_WEB").is_none(), "{why}");
    eprintln!("{why}: skipped");
}

/// The client's dev packages, or `None` (skipped).
fn packages() -> Option<PathBuf> {
    let dir = client_dir();
    if dir.join("node_modules/.bin/esbuild").exists() {
        return Some(dir);
    }
    skip("no crates/aio/client/node_modules (npm ci there)");
    None
}

/// The islands bundled (`test/e2e/build.mjs`), or `false` (skipped).
fn bundled() -> bool {
    let made = ISLANDS.get_or_init(|| {
        let dir = packages()?;
        let out = tempfile::tempdir().unwrap();
        let s = std::process::Command::new("node")
            .current_dir(&dir)
            .args(["test/e2e/build.mjs", out.path().to_str().unwrap()])
            .status()
            .expect("node runs");
        assert!(s.success(), "test/e2e/build.mjs");
        let read = |n: &str| -> &'static str { String::leak(std::fs::read_to_string(out.path().join(n)).unwrap()) };
        Some([
            Island::react(read("react.js")),
            Island::js(read("solid.js")),
            Island::js(read("svelte.js")),
            Island::js(read("vue.js")),
        ])
    });
    made.is_some()
}

#[test]
fn the_client_s_unit_tests_and_types_pass() {
    let Some(dir) = packages() else { return };
    let run = |cmd: &str, args: &[&str]| {
        let out = std::process::Command::new(cmd).current_dir(&dir).args(args).output().expect(cmd);
        let said = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{cmd} {args:?}:\n{said}");
        said
    };
    let said = run("node", &["--test", "test/client.test.js"]);
    assert!(said.contains("fail 0"), "{said}");
    run(dir.join("node_modules/.bin/tsc").to_str().unwrap(), &["-p", "test"]);
}

/// The desktop window: its page may connect nowhere, so the islands' client
/// rides the window's own wire to the app, with the same API.
#[test]
fn every_library_reads_and_calls_the_cells_in_the_desktop_window() {
    if !bundled() {
        return;
    }
    let mut ui = aio::testing::TestUi::new(window::app);
    let same = |ui: &mut aio::testing::TestUi, n: &str| {
        for t in ["rs-n", "react-n", "solid-n", "svelte-n", "vue-n"] {
            ui.until_text(t, n);
        }
    };
    same(&mut ui, "0");
    ui.until_text("react-conn", "online");
    for (button, n) in
        [("rs-add", "1"), ("react-add", "3"), ("solid-add", "6"), ("svelte-add", "10"), ("vue-add", "15")]
    {
        ui.click(button);
        same(&mut ui, n);
    }
    // A call the app refuses a client (not #[client]): rolled back, said.
    ui.click("react-reset");
    ui.until_text("react-err", "ACCESS_DENIED");
    same(&mut ui, "15");
}

/// A `client` app's window (its cell a copy of a server's): the page's
/// client is carried to that server, the same API.
#[test]
fn in_a_client_app_s_window_the_page_reaches_its_server() {
    if !bundled() {
        return;
    }
    let h = Handle::<Tally>::memory();
    let remote = aio::rt().block_on(async {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        tokio::spawn(aio::sync::serve(h.clone(), l, KEY.into()));
        aio::sync::Remote::<Tally>::connect(&addr, KEY, "ana").await.unwrap()
    });
    client_window::SOURCE.set(aio::Source::Remote(remote)).ok();
    let mut ui = aio::testing::TestUi::new(client_window::app);
    for t in ["rs-n", "react-n", "vue-n"] {
        ui.until_text(t, "0");
    }
    ui.click("react-add");
    for t in ["rs-n", "react-n", "solid-n", "svelte-n", "vue-n"] {
        ui.until_text(t, "2");
    }
    ui.until("the server has it", || h.state().n == 2);
}

/// One plain HTTP/1.1 GET: the status, the head, the body (read to its
/// length: Chromium keeps the connection open).
async fn get(addr: &str, path: &str) -> (u16, String, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
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

struct Page(tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, u64);

impl Page {
    /// The browser's page target `id`, or its first page.
    async fn open(b: &aio::testing::TestBrowser, id: Option<&str>) -> Self {
        for _ in 0..200 {
            let (_, _, body) = get(&format!("127.0.0.1:{}", b.port()), "/json/list").await;
            let list: Vec<Value> = aio::serde_json::from_str(&body).unwrap_or_default();
            if let Some(p) = list.iter().find(|t| t["type"] == "page" && id.is_none_or(|id| t["id"] == id)) {
                let at = p["webSocketDebuggerUrl"].as_str().unwrap();
                let path = &at[at.find("/devtools/").unwrap()..];
                let url = format!("ws://127.0.0.1:{}{path}", b.port());
                return Self(tokio_tungstenite::connect_async(url).await.unwrap().0, 0);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("Chromium shows no page");
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        self.1 += 1;
        let m = json!({ "id": self.1, "method": method, "params": params });
        self.0.send(Message::text(m.to_string())).await.unwrap();
        loop {
            let Message::Text(t) = self.0.next().await.unwrap().unwrap() else { continue };
            let v: Value = aio::serde_json::from_str(&t).unwrap();
            if v["id"] == self.1 {
                return v["result"].clone();
            }
        }
    }

    async fn eval(&mut self, expr: &str) -> Value {
        self.call("Runtime.evaluate", json!({ "expression": expr, "returnByValue": true })).await["result"]["value"]
            .clone()
    }

    async fn go(&mut self, url: &str) {
        self.call("Page.navigate", json!({ "url": url })).await;
    }

    /// Waits (10 s at most) until each element's text is as given.
    async fn texts(&mut self, want: &[(&str, &str)]) {
        let expr = format!(
            "[{}].map((id) => document.getElementById(id)?.textContent ?? null)",
            want.iter().map(|(id, _)| format!("'{id}'")).collect::<Vec<_>>().join(",")
        );
        let want = json!(want.iter().map(|(_, t)| t).collect::<Vec<_>>());
        let mut last = Value::Null;
        for _ in 0..400 {
            last = self.eval(&expr).await;
            if last == want {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("{last} (wanted {want}); the page: {}", self.eval("document.body.outerHTML").await);
    }

    /// Waits until the page's view and each library show `n`.
    async fn same(&mut self, n: &str) {
        self.texts(&[("rs-n", n), ("react-n", n), ("solid-n", n), ("svelte-n", n), ("vue-n", n)]).await;
    }

    /// Waits (4 s at most) until the server's cell holds `n`.
    async fn server(&mut self, h: &Handle<Tally>, n: i64, why: &str) {
        for _ in 0..160 {
            if h.state().n == n {
                return;
            }
            self.eval("1").await;
        }
        assert_eq!(h.state().n, n, "{why}");
    }

    async fn click(&mut self, id: &str) {
        self.eval(&format!("document.getElementById('{id}').click()")).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn every_library_shows_and_calls_the_page_s_own_cells_in_a_browser() {
    if !bundled() {
        return;
    }
    let h = Handle::<Tally>::memory();
    tab::SOURCE.set(aio::Source::Local(h.clone())).ok();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let plain = aio::http::Route::get("/plain", |_| async {
        aio::http::Reply::text(PLAIN).header("content-type", "text/html; charset=utf-8")
    });
    let http = aio_shell::web::tabs(aio::http::Http::default().route(plain), tab::app, aio_shell::Window::default());
    tokio::spawn(serve_with(h.clone(), l, Options { token: KEY.into(), http, ..Default::default() }));

    // Served to anyone, as any library (what it reaches asks the key).
    let (status, head, body) = get(&addr, "/__aio/client.js").await;
    assert_eq!(status, 200);
    assert!(head.contains("content-type: text/javascript"), "{head}");
    assert!(body.contains("export function connect"), "the client itself");

    let b = match aio::testing::TestBrowser::open("about:blank") {
        Ok(b) => b,
        Err(e) => return skip(&e),
    };
    tokio::time::timeout(Duration::from_secs(90), async {
        let mut p = Page::open(&b, None).await;
        p.call("Page.enable", json!({})).await;
        p.go(&format!("http://{addr}/?token={KEY}")).await;
        p.same("0").await;
        p.texts(&[("react-conn", "online")]).await;
        // Each one's call reaches every other one (React's predicted, then confirmed).
        for (button, n) in [("rs-add", "1"), ("react-add", "3"), ("solid-add", "6"), ("svelte-add", "10"), ("vue-add", "15")] {
            p.click(button).await;
            p.same(n).await;
        }
        assert_eq!(h.state().n, 15, "the server's cell");

        // A change of another field: no library runs again.
        let runs = "['react-n', 'solid-n', 'svelte-n', 'vue-n'].map((id) => { const d = document.getElementById(id).dataset; return d.renders ?? d.runs; })";
        let before = p.eval(runs).await;
        h.rename("x".into()).unwrap();
        for _ in 0..40 {
            p.eval("1").await;
        }
        assert_eq!(h.state().label, "x");
        assert_eq!(before, p.eval(runs).await, "a selector follows its part only");

        // A call the server refuses: the prediction rolls back, its code is said.
        p.click("react-reset").await;
        p.texts(&[("react-err", "ACCESS_DENIED"), ("react-n", "15"), ("rs-n", "15")]).await;
        assert_eq!(h.state().n, 15);

        // A page of plain DOM: the served client, the same cell; it keeps its copy.
        p.go(&format!("http://{addr}/plain?token={KEY}")).await;
        p.texts(&[("plain", "plain:15")]).await;
        p.click("plain-add").await;
        p.texts(&[("plain", "plain:16")]).await;
        p.server(&h, 16, "the plain page's call").await;
        p.go("about:blank").await;

        // Offline (its server never answers): started from the kept copy, a
        // call shown by its predictor, and again after a reload.
        let t = p.call("Target.createTarget", json!({ "url": "about:blank" })).await;
        let mut off = Page::open(&b, t["targetId"].as_str()).await;
        let offline = format!("http://{addr}/plain?token={KEY}&offline");
        off.go(&offline).await;
        off.texts(&[("plain", "plain:16")]).await;
        off.click("plain-add").await;
        off.texts(&[("plain", "plain:17")]).await;
        off.go(&offline).await;
        off.texts(&[("plain", "plain:17")]).await;
        assert_eq!(h.state().n, 16, "nothing reached the server");

        // Linked again, a tab sends the kept call, once.
        p.go(&format!("http://{addr}/plain?token={KEY}")).await;
        p.texts(&[("plain", "plain:17")]).await;
        p.server(&h, 17, "the kept call, sent").await;

        // A twin tab's call: made offline, sent by the linked tab.
        off.click("plain-add").await;
        off.texts(&[("plain", "plain:18")]).await;
        p.texts(&[("plain", "plain:18")]).await;
        p.server(&h, 18, "the linked twin sent it").await;
    })
    .await
    .expect("the test took over 90 s");
}

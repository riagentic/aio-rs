//! `ax dev --web`'s hot-patching, end to end: a fixture app served by the
//! real `ax dev`, its page open in a headless Chromium. A view edit and a
//! cell method edit show in the page with no reload, its count kept; an
//! edit that adds a JS import is a full build, the page reloaded. Skipped
//! without the wasm32 target, wasm-bindgen or a Chromium (where
//! `AIO_CI_WEB` is set it fails instead).
#![cfg(unix)]

use aio::serde_json::{Value, json};
use futures_util::{SinkExt, StreamExt};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::Message;

const LIB: &str = r#"use aio::ui::prelude::*;

#[aio::cell(id = "counter")]
pub struct Counter {
    pub count: i64,
}

#[aio::methods]
impl Counter {
    pub fn add(&mut self, by: i64) {
        self.count += by;
    }
}

aio::client!(Counter, counter);

#[component]
pub fn App() -> View {
    use_live();
    let n = part(|c| &c.count);
    let tag = format!("v{}", 0);
    rsx! {
        main {
            h1 { "data-t": "count", "{n}" }
            p { "data-t": "tag", {tag} }
            button { onclick: move |_| _ = act!(add(1)), "Add" }
        }
    }
}

#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn web() {
    _ = SOURCE.set(aio::Source::Local(aio::Handle::memory()));
    aio::web::launch(App);
}
"#;

fn skipped(why: &str) {
    assert!(std::env::var_os("AIO_CI_WEB").is_none(), "{why}: AIO_CI_WEB is set, so this test must run");
    eprintln!("{why}: skipped");
}

/// The fixture app in `dir`: aio-rs as this checkout has it, at its lock's versions.
fn app(dir: &Path) {
    let aio = Path::new(env!("CARGO_MANIFEST_DIR")).join("../aio");
    let manifest = format!(
        "[package]\nname = \"webhot\"\nversion = \"0.1.0\"\nedition = \"2024\"\npublish = false\n\n\
         [lib]\ncrate-type = [\"cdylib\"]\n\n[dependencies]\n\
         aio = {{ package = \"aio-rs\", path = {:?}, default-features = false, features = [\"web\", \"kit\"] }}\n\
         wasm-bindgen = \"0.2\"\n\n[workspace]\n",
        aio.display().to_string()
    );
    std::fs::create_dir_all(dir.join("src")).unwrap();
    for (f, text) in [
        ("Cargo.toml", manifest.as_str()),
        ("Cargo.lock", include_str!("../../../Cargo.lock")),
        ("rust-toolchain.toml", include_str!("../../../rust-toolchain.toml")),
        ("aio.toml", "[app]\nid = \"webhot\"\nname = \"Web hot\"\n"),
        ("src/lib.rs", LIB),
    ] {
        std::fs::write(dir.join(f), text).unwrap();
    }
}

/// `ax dev` killed when dropped.
struct Ax(Child);

impl Drop for Ax {
    fn drop(&mut self) {
        _ = self.0.kill();
        _ = self.0.wait();
    }
}

/// The next line of ax's log that holds `part` (90 s at most: the first build).
fn said(log: &Receiver<String>, part: &str) -> String {
    let end = Instant::now() + Duration::from_secs(90);
    while let Ok(l) = log.recv_timeout(end.saturating_duration_since(Instant::now())) {
        if l.contains(part) {
            return l;
        }
        assert!(!l.contains("a full build") || part.contains("a full build"), "not hot-patched: {l}");
    }
    panic!("ax dev never said {part:?}");
}

/// Chromium's targets on DevTools `port` (HTTP/1.1: it answers 1.0 with
/// nothing, and keeps the connection open: read to the length; the
/// sockets it names are at the `Host` asked).
fn targets(port: u16) -> Value {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(s, "GET /json HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").unwrap();
    let (mut got, mut buf) = (Vec::new(), [0; 8192]);
    loop {
        let n = s.read(&mut buf).unwrap();
        got.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&got).into_owned();
        let (head, body) = text.split_once("\r\n\r\n").unwrap_or_default();
        let len =
            head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:")?.trim().parse().ok());
        if n == 0 || len.is_some_and(|len: usize| body.len() >= len) {
            return aio::serde_json::from_str(body).unwrap_or_default();
        }
    }
}

/// The page's tab on Chromium's DevTools `port`.
async fn tab(port: u16, url: &str) -> Tab {
    for _ in 0..200 {
        let ts = targets(port);
        let ws = ts.as_array().into_iter().flatten().find(|t| t["url"].as_str().is_some_and(|u| u.starts_with(url)));
        if let Some(ws) = ws.and_then(|t| t["webSocketDebuggerUrl"].as_str()) {
            return Tab(tokio_tungstenite::connect_async(ws).await.unwrap().0, 0);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("Chromium shows no tab on {url}");
}

struct Tab(tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, u64);

impl Tab {
    /// `expr`'s value in the page.
    async fn eval(&mut self, expr: &str) -> Value {
        self.1 += 1;
        let params = json!({ "expression": expr, "awaitPromise": true, "returnByValue": true });
        let m = json!({ "id": self.1, "method": "Runtime.evaluate", "params": params });
        self.0.send(Message::text(m.to_string())).await.unwrap();
        while let Some(Ok(m)) = self.0.next().await {
            if let Message::Text(t) = m
                && let Ok(v) = aio::serde_json::from_str::<Value>(&t)
                && v["id"] == self.1
            {
                return v["result"]["result"]["value"].clone();
            }
        }
        panic!("the DevTools socket closed");
    }

    /// Waits until `expr` is `want` (30 s at most).
    async fn until(&mut self, expr: &str, want: Value) {
        let mut last = Value::Null;
        for _ in 0..600 {
            last = self.eval(expr).await;
            if last == want {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("{expr}: {last} (wanted {want})");
    }
}

fn text(t: &str) -> String {
    format!("document.querySelector('[data-t={t}]')?.textContent ?? null")
}

/// The app's source with each `(old, new)` replaced, saved at once.
fn edit(dir: &Path, swaps: &[(&str, &str)]) {
    let f = dir.join("src/lib.rs");
    let s = swaps.iter().fold(std::fs::read_to_string(&f).unwrap(), |s, (old, new)| {
        assert!(s.contains(old), "{old}");
        s.replace(old, new)
    });
    std::fs::write(f, s).unwrap();
}

const CLICK: &str = "[...document.querySelectorAll('button')].find((b) => b.textContent === 'Add').click()";

#[tokio::test(flavor = "multi_thread")]
async fn a_view_and_a_cell_method_edit_are_patched_into_the_page_with_its_state() {
    let sysroot = Command::new("rustc").args(["--print", "sysroot"]).output().unwrap();
    let sysroot = String::from_utf8_lossy(&sysroot.stdout).trim().to_string();
    if !Path::new(&sysroot).join("lib/rustlib/wasm32-unknown-unknown").is_dir() {
        return skipped("no wasm32 target");
    }
    if let Err(e) = aio::testing::TestBrowser::find() {
        return skipped(&e);
    }
    let dir = tempfile::tempdir().unwrap();
    app(dir.path());
    let mut child = Command::new(env!("CARGO_BIN_EXE_ax"))
        .args(["dev", "webhot", "--web", "--port", "0"])
        .current_dir(dir.path())
        .env("AIORT_NO_OPEN", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let err = child.stderr.take().unwrap();
    let _ax = Ax(child);
    let (tx, log) = channel();
    std::thread::spawn(move || BufReader::new(err).lines().map_while(Result::ok).try_for_each(|l| tx.send(l)));
    let mut url = String::new();
    while let Ok(l) = log.recv_timeout(Duration::from_secs(600)) {
        if l.contains("needs wasm-bindgen") {
            return skipped(&l);
        }
        if let Some(u) = l.strip_prefix("ax dev: ").and_then(|l| l.split_once(" (Ctrl-C")) {
            url = u.0.to_string();
            break;
        }
    }
    assert!(url.starts_with("http://127.0.0.1:"), "ax dev never served");
    let c = aio::testing::TestBrowser::open(&url).unwrap();
    let mut t = tab(c.port(), &url).await;
    t.until(&format!("({}) !== null && !!window.__aioPatched", text("count")), json!(true)).await;
    for _ in 0..3 {
        t.eval(CLICK).await;
    }
    t.until(&text("count"), json!("3")).await;
    // Gone if the page reloads.
    t.eval("window.kept = 1").await;

    // A view's logic: patched in, the count kept.
    edit(dir.path(), &[("format!(\"v{}\", 0)", "format!(\"v{}\", 1)")]);
    said(&log, "hot-patched");
    t.until(&text("tag"), json!("v1")).await;
    assert_eq!(t.eval(&text("count")).await, json!("3"));

    // A cell method's: the next call runs the new code on the kept state.
    edit(dir.path(), &[("self.count += by;", "self.count += by * 10;")]);
    said(&log, "hot-patched");
    said(&log, "hot-patch 2 applied");
    t.eval(CLICK).await;
    t.until(&text("count"), json!("13")).await;
    assert_eq!(t.eval("window.kept").await, json!(1), "the page reloaded");

    // A new JS import: the page can't take it as a patch; built and reloaded.
    let import =
        "#[wasm_bindgen::prelude::wasm_bindgen(js_namespace = Date)]\nextern \"C\" {\n    fn now() -> f64;\n}\n\n";
    edit(
        dir.path(),
        &[
            ("format!(\"v{}\", 1)", "format!(\"v{}\", now() as u64 * 0 + 2)"),
            ("#[component]", &format!("{import}#[component]")),
        ],
    );
    said(&log, "a full build");
    t.until(&text("tag"), json!("v2")).await;
    assert_eq!(t.eval("window.kept ?? null").await, Value::Null, "a full build reloads the page");
}

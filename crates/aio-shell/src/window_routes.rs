//! The app's routes in its window, in the real runtime (this crate's
//! main.js, headless): the page's `fetch('/api/…')` goes to `aio://app`,
//! which main.js hands to the app over its private socket (a Unix socket or
//! a named pipe), never a TCP port; the app answers that window and no
//! other process. Skipped where the runtime is not installed.
use aio::http::{Http, Reply, Route};
use aio::serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};

const PATIENCE: Duration = Duration::from_secs(60);

#[tokio::test(flavor = "multi_thread")]
async fn the_window_fetches_the_app_s_routes_over_its_socket_and_nothing_else_is_answered() {
    let rt = crate::runtime::dir();
    let exe = crate::runtime::exe(&rt);
    if !exe.exists() {
        return eprintln!("skipped: no Electron runtime in {}", rt.display());
    }
    let d = tempfile::tempdir_in(aio::paths::short_temp()).unwrap();
    let dir = d.path().join("shell");
    crate::install_with(&dir, "routed", &crate::Window { title: "Routed", ..Default::default() }, true).unwrap();
    let (sock, ctl_sock, http_sock) =
        (aio::ipc::addr(d.path(), "wire"), aio::ipc::addr(d.path(), "ctl"), aio::ipc::addr(d.path(), "http"));
    let mut wires = aio::ipc::Listener::bind(&sock).unwrap();
    let mut ctls = aio::ipc::Listener::bind(&ctl_sock).unwrap();
    let http = Http::default()
        .route(Route::get("/api/hi/:who", |r| async move {
            Reply::text(format!("hi {} {}", r.param("who").unwrap_or(""), r.query("x").unwrap_or("")))
        }))
        .route(Route::post("/api/echo", |r| async move { Reply::text(String::from_utf8_lossy(r.body()).into_owned()) }))
        .route(Route::get("/api/never", |_| std::future::pending()))
        .route(Route::get("/api/feed", |_| async {
            Reply::stream("application/x-ndjson", futures_util::stream::iter(["{\"n\":1}\n", "{\"n\":2}\n"]))
        }));
    tokio::spawn(aio::http::serve_window(http, aio::ipc::Listener::bind(&http_sock).unwrap()));
    let mut child = tokio::process::Command::new(&exe)
        .arg(&dir)
        .args(crate::runtime::sandbox_args(&rt))
        .args(["--ozone-platform=headless", "--aio-title=Routed", "--aio-width=640", "--aio-height=480"])
        .args(["--use-angle=swiftshader", "--enable-unsafe-swiftshader"])
        .arg("--aio-no-open=1")
        .arg(format!("--user-data-dir={}", d.path().join("profile").display()))
        .arg(format!("--aio-sock={sock}"))
        .arg(format!("--aio-ctl={ctl_sock}"))
        .arg(format!("--aio-http={http_sock}"))
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env("TMPDIR", d.path())
        // aio's AIO_SOCKET_TIMEOUT_MS: how long a route may take to its first byte.
        .env("AIORT_SOCKET_TIMEOUT_MS", "1500")
        .env("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={}", d.path().join("no-bus").display()))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    // The window's gate names this runtime, as `run` does.
    aio::ipc::Only::window().arm(child.id().unwrap());
    let said = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    {
        let (said, err) = (said.clone(), child.stderr.take().unwrap());
        tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(err).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                said.lock().unwrap().push(l);
            }
        });
    }
    let s = tokio::time::timeout(PATIENCE, ctls.accept()).await.unwrap().unwrap();
    let (r, mut ctl) = tokio::io::split(s);
    let (tx, mut heard) = tokio::sync::mpsc::unbounded_channel::<Value>();
    tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(r).lines();
        while let Ok(Some(l)) = lines.next_line().await {
            if let Ok(v) = aio::serde_json::from_str(&l) {
                _ = tx.send(v);
            }
        }
    });
    ctl.write_all(b"{\"t\":\"place\",\"width\":640,\"height\":480}\n").await.unwrap();
    let _wire = tokio::time::timeout(PATIENCE, wires.accept()).await.unwrap().unwrap();
    let mut id = 0;
    let mut eval = async |expr: &str| -> Value {
        id += 1;
        let p = json!({ "expression": expr, "returnByValue": true, "awaitPromise": true });
        let m = json!({ "t": "cdp", "id": id, "method": "Runtime.evaluate", "params": p });
        ctl.write_all(format!("{m}\n").as_bytes()).await.unwrap();
        loop {
            let v = tokio::time::timeout(PATIENCE, heard.recv()).await.unwrap().unwrap();
            if v["t"] == "cdp" && v["id"] == id {
                assert!(v["error"].is_null() && v["result"]["exceptionDetails"].is_null(), "{expr}: {v}");
                return v["result"]["result"]["value"].clone();
            }
        }
    };
    // The page is up (its own document, from disk).
    let end = std::time::Instant::now() + PATIENCE;
    while eval("location.href").await != "aio://app/" {
        assert!(std::time::Instant::now() < end, "the page never loaded: {:?}", said.lock().unwrap());
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let text = |js: &str| format!("fetch({js}).then(async (r) => r.status + ' ' + await r.text())");
    assert_eq!(eval(&text("'/api/hi/ann?x=1'")).await, "200 hi ann 1");
    let post = "'/api/echo', { method: 'POST', headers: { 'content-type': 'application/json' }, body: '{\"a\":1}' }";
    assert_eq!(eval(&text(post)).await, "200 {\"a\":1}");
    // Streamed, read as it comes.
    let feed = "fetch('/api/feed').then(async (r) => { const rd = r.body.getReader(); let s = ''; \
                for (;;) { const { done, value } = await rd.read(); if (done) return s; s += new TextDecoder().decode(value) } })";
    assert_eq!(eval(feed).await, "{\"n\":1}\n{\"n\":2}\n");
    assert_eq!(eval(&text("'/nope'")).await, "404 Not Found");
    // A route that never answers: a 504 after AIORT_SOCKET_TIMEOUT_MS, saying so.
    let never = eval(&text("'/api/never'")).await.as_str().unwrap_or_default().to_string();
    assert!(
        never.starts_with("504 ") && never.contains("after 1500 ms") && never.contains("AIORT_SOCKET_TIMEOUT_MS"),
        "{never}"
    );
    // Bodies nobody reads (20 images of a missing path) hold none of the
    // 6 connections: the routes still answer.
    let missing = "Promise.all(Array.from({ length: 20 }, (_, i) => new Promise((ok) => { \
                   const im = new Image(); im.onerror = () => ok(i); im.src = '/missing/' + i }))).then((a) => a.length)";
    assert_eq!(eval(missing).await, 20);
    assert_eq!(eval(&text("'/api/hi/bob'")).await, "200 hi bob ");

    // Any other process, this one too: refused (not the window), never answered.
    let mut s = aio::ipc::connect(&http_sock).await.unwrap();
    let mut got = Vec::new();
    if s.write_all(b"GET /api/hi/x HTTP/1.1\r\nhost: app\r\nconnection: close\r\n\r\n").await.is_ok() {
        _ = tokio::time::timeout(PATIENCE, s.read_to_end(&mut got)).await.unwrap();
    }
    assert!(got.is_empty(), "another process was answered: {}", String::from_utf8_lossy(&got));
    _ = child.kill().await;
}

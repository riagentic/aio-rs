//! The app's routes served to its window alone (aio's zero-port routes,
//! `aio://app/<path>`): HTTP over the private socket, no TCP port, and only
//! the process the window's gate names is answered (unarmed: no one).
use aio::http::{Http, Reply, Route, serve_window};
use aio::ipc::{Listener, Only, addr, connect};
use aio::prelude::*;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// `req` sent over a new connection to `at`; all the app answered.
async fn ask(at: &str, req: &str) -> String {
    let mut s = connect(at).await.unwrap();
    let mut got = Vec::new();
    // Refused: the app drops the connection (a broken pipe or a reset is a refusal too).
    if s.write_all(req.as_bytes()).await.is_err() {
        return String::new();
    }
    let read = tokio::time::timeout(Duration::from_secs(30), s.read_to_end(&mut got)).await.expect("no hang");
    if read.is_err() {
        got.clear();
    }
    String::from_utf8_lossy(&got).into_owned()
}

#[cell(persist)]
pub struct Pad {
    pub n: u32,
}

#[methods]
impl Pad {}

fn get(path: &str) -> String {
    format!("GET {path} HTTP/1.1\r\nhost: app\r\nconnection: close\r\n\r\n")
}

#[tokio::test(flavor = "multi_thread")]
async fn routes_reach_the_window_over_its_socket_and_no_other_process() {
    let d = tempfile::tempdir_in(aio::paths::short_temp()).unwrap();
    // An app that serves no path opens no door for its window.
    let id = format!("wr-{}", std::process::id());
    let cfg = aio::config::Config::parse(&format!("[app]\nid = \"{id}\"")).unwrap();
    let dirs = aio::paths::AppDirs::at(&d.path().join("a"), &id).unwrap();
    drop(aio::Source::<Pad>::for_app(&cfg, &dirs).unwrap());
    assert_eq!(aio::ipc::window_http(), None);
    let at = addr(d.path(), "http");
    let http = Http::default()
        .route(Route::get(
            "/api/hi/:who",
            |r| async move { Reply::text(format!("hi {}", r.param("who").unwrap_or(""))) },
        ))
        .route(Route::post("/api/echo", |r| async move { Reply::text(String::from_utf8_lossy(r.body()).into_owned()) }))
        .route(Route::get("/api/feed", |_| async {
            Reply::stream("application/x-ndjson", futures_util::stream::iter(["{\"n\":1}\n", "{\"n\":2}\n"]))
        }));
    let l = Listener::bind(&at).unwrap();
    tokio::spawn(serve_window(http, l));

    // Unarmed (no window started): refused once the gate stops waiting for one.
    assert_eq!(ask(&at, &get("/api/hi/x")).await, "", "fail closed: no window, no answer");
    // Armed for another process: refused at once.
    Only::window().arm(u32::MAX);
    assert_eq!(ask(&at, &get("/api/hi/x")).await, "", "not the window: no answer");

    // This process as the window: its routes answer, with no key.
    Only::window().arm(std::process::id());
    let r = ask(&at, &get("/api/hi/ann?x=1")).await;
    assert!(r.starts_with("HTTP/1.1 200") && r.ends_with("hi ann"), "{r}");
    let body = "{\"a\":1}";
    let post = format!(
        "POST /api/echo HTTP/1.1\r\nhost: app\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let r = ask(&at, &post).await;
    assert!(r.starts_with("HTTP/1.1 200") && r.ends_with(body), "{r}");
    // Streamed as it comes (chunked), never buffered whole.
    let r = ask(&at, &get("/api/feed")).await;
    assert!(r.to_ascii_lowercase().contains("transfer-encoding: chunked"), "{r}");
    assert!(r.contains("{\"n\":1}") && r.contains("{\"n\":2}"), "{r}");
    // Not a route: 404; the browser shell is the network server's, never here.
    assert!(ask(&at, &get("/nope")).await.starts_with("HTTP/1.1 404"));
    // Keep-alive: two requests on one connection.
    let mut s = connect(&at).await.unwrap();
    let two = format!("{}{}", get("/api/hi/a").replace("connection: close\r\n", ""), get("/api/hi/b"));
    s.write_all(two.as_bytes()).await.unwrap();
    let mut got = String::new();
    tokio::time::timeout(Duration::from_secs(30), s.read_to_string(&mut got)).await.unwrap().unwrap();
    assert!(got.contains("hi a") && got.ends_with("hi b") && got.matches("HTTP/1.1 200").count() == 2, "{got}");

    // A standalone app with a route: served to its window over its own
    // socket, and on no TCP port (no `serving.json`).
    let dirs = aio::paths::AppDirs::at(&d.path().join("b"), &id).unwrap();
    let routed = Http::default().route(Route::get("/api/n", |_| async { Reply::text("7") }));
    let _s = aio::Source::<Pad>::for_app_with(&cfg, &dirs, routed).unwrap();
    let door = aio::ipc::window_http().expect("the window's route door");
    assert_eq!(door, addr(&aio::paths::sockets(&id, &dirs.data).unwrap(), "http"));
    assert!(ask(door, &get("/api/n")).await.ends_with("\r\n\r\n7"));
    assert!(!dirs.data.join("serving.json").exists(), "no TCP port");

    // The window gone: no one again.
    Only::window().disarm(std::process::id());
    assert_eq!(ask(&at, &get("/api/hi/x")).await, "", "window gone: no answer");
    assert_eq!(ask(door, &get("/api/n")).await, "", "window gone: no answer");
    _ = std::fs::remove_dir_all(aio::paths::sockets(&id, &dirs.data).unwrap());
}

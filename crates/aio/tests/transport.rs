//! `[client] transport`: a copy's links over plain HTTP requests when a
//! WebSocket doesn't get through, on a real server. One test: the
//! transport is the process's.
use aio::prelude::*;
use aio::sync::{Link, Remote, Transport, serve, set_transport};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[cell(shared)]
pub struct Chat {
    pub lines: Vec<String>,
}

#[methods]
impl Chat {
    #[client]
    pub fn say(&mut self, line: String) -> Result {
        ensure!(!line.is_empty(), "say something");
        self.lines.push(line);
        Ok(())
    }
}

async fn until(what: &str, f: impl Fn() -> bool) {
    for _ in 0..500 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out: {what}");
}

/// A proxy in front of `to` that refuses every WebSocket upgrade (as some
/// corporate proxies and CDNs do) and passes plain HTTP on; its address.
async fn blocking_proxy(to: String) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        while let Ok((mut c, _)) = l.accept().await {
            let to = to.clone();
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut b = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    if c.read(&mut b).await.unwrap_or(0) == 0 {
                        return;
                    }
                    head.push(b[0]);
                }
                if String::from_utf8_lossy(&head).to_ascii_lowercase().contains("upgrade: websocket") {
                    _ = c.write_all(b"HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await;
                    return;
                }
                let mut up = tokio::net::TcpStream::connect(&to).await.unwrap();
                up.write_all(&head).await.unwrap();
                _ = tokio::io::copy_bidirectional(&mut c, &mut up).await;
            });
        }
    });
    addr
}

#[tokio::test(flavor = "multi_thread")]
async fn a_copy_syncs_over_plain_http_when_websockets_are_blocked() {
    let h = Handle::<Chat>::memory();
    h.say("hello".into()).unwrap();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(serve(h.clone(), l, "secret".into()));
    let proxied = blocking_proxy(addr.clone()).await;
    let link = |addr: &str, token: &str| Link { addr: addr.into(), token: token.into(), root: None };

    // `ws`: through the proxy, no link at all.
    set_transport(Transport::Ws);
    assert!(Remote::<Chat>::connect_to(link(&proxied, "secret"), "ana").await.is_err());

    // `auto` (the default): the upgrade is refused, so plain HTTP.
    set_transport(Transport::Auto);
    let a = Remote::<Chat>::connect_to(link(&proxied, "secret"), "ana").await.unwrap();
    let b = Remote::<Chat>::connect_to(link(&proxied, "secret"), "bo").await.unwrap();
    assert_eq!(a.state().lines, ["hello"]);
    // Predicted, confirmed, and seen by the other copy and the server.
    a.say("from a".into()).unwrap();
    assert_eq!(a.state().lines.last().map(String::as_str), Some("from a"));
    until("confirmed", || a.pending() == 0).await;
    until("b follows", || b.state().lines == h.state().lines).await;
    // A server change reaches both; many of them, in order.
    for i in 0..50 {
        h.say(format!("server {i}")).unwrap();
    }
    until("all of them", || a.state().lines == h.state().lines && b.state().lines == h.state().lines).await;
    // Its pings ride it as on a WebSocket.
    until("a round trip", || a.rtt().is_some()).await;

    // `poll`: the same, straight to the server; a wrong key is refused as on `/ws`.
    set_transport(Transport::Poll);
    let wrong = Remote::<Chat>::connect_to(link(&addr, "nope"), "cy").await.err().unwrap();
    assert_eq!(wrong.kind(), std::io::ErrorKind::PermissionDenied);
    let c = Remote::<Chat>::connect_to(link(&addr, "secret"), "cy").await.unwrap();
    c.say("from c".into()).unwrap();
    until("c confirmed", || c.pending() == 0).await;
    until("a sees c", || a.state().lines == h.state().lines).await;
}

/// One plain HTTP request: the status and the body as JSON.
async fn http(addr: &str, method: &str, path: &str, body: &str) -> (u16, serde_json::Value) {
    let mut c = tokio::net::TcpStream::connect(addr).await.unwrap();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nhost: {addr}\r\nauthorization: Bearer secret\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    c.write_all(req.as_bytes()).await.unwrap();
    let mut got = String::new();
    c.read_to_string(&mut got).await.unwrap();
    let code = got.split(' ').nth(1).unwrap().parse().unwrap();
    let body = got.split_once("\r\n\r\n").map_or("", |(_, b)| b);
    (code, serde_json::from_str(body).unwrap_or_default())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_polled_socket_takes_a_resend_once_and_gives_a_lost_answer_again() {
    use serde_json::json;
    let h = Handle::<Chat>::memory();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(serve(h.clone(), l, "secret".into()));
    // Opened only with the key.
    let mut c = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let req = format!(
        "POST /__aio/poll HTTP/1.1\r\nhost: {addr}\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{{}}"
    );
    c.write_all(req.as_bytes()).await.unwrap();
    let mut got = String::new();
    c.read_to_string(&mut got).await.unwrap();
    assert!(got.starts_with("HTTP/1.1 401"), "{got}");
    let (code, open) = http(&addr, "POST", "/__aio/poll", "{}").await;
    assert_eq!(code, 200);
    let id = open["id"].as_str().unwrap().to_string();
    // A hello and a call on link 1 (mux's frames), sent twice (its answer lost).
    let hello = json!({ "t": "hello", "cell": <Chat as aio::CellData>::ID, "v": <Chat as aio::CellData>::VERSION, "user": "ana" });
    let call = json!({ "t": "call", "method": "say", "args": ["once"] });
    let frames = json!([format!("1:{hello}"), format!("1:{call}")]).to_string();
    assert_eq!(http(&addr, "POST", &format!("/__aio/poll?id={id}&n=0"), &frames).await.0, 200);
    assert_eq!(http(&addr, "POST", &format!("/__aio/poll?id={id}&n=0"), &frames).await.0, 200);
    // A batch past what it has is refused: frames would be missing.
    assert_eq!(http(&addr, "POST", &format!("/__aio/poll?id={id}&n=9"), "[]").await.0, 409);
    until("said once", || h.state().lines == ["once"]).await;
    // Its state, the call's patch and reply; asked again from 0, the same again.
    let mut taken = vec![];
    while taken.len() < 3 {
        let (code, got) = http(&addr, "GET", &format!("/__aio/poll?id={id}&ack=0"), "").await;
        assert_eq!((code, got["from"].as_u64()), (200, Some(0)));
        taken = got["frames"].as_array().unwrap().clone();
    }
    assert!(taken[0].as_str().unwrap().starts_with("1:{"), "{taken:?}");
    let (_, again) = http(&addr, "GET", &format!("/__aio/poll?id={id}&ack=0"), "").await;
    assert_eq!(again["frames"].as_array().unwrap()[..3], taken[..3]);
    // Acked: let go.
    let (_, rest) = http(&addr, "GET", &format!("/__aio/poll?id={id}&ack=1"), "").await;
    assert_eq!(rest["from"], 1);
    // Acking more than was sent, or a socket that isn't: refused.
    assert_eq!(http(&addr, "GET", &format!("/__aio/poll?id={id}&ack=999"), "").await.0, 400);
    assert_eq!(http(&addr, "GET", "/__aio/poll?id=nope&ack=0", "").await.0, 404);
}

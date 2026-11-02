use aio::prelude::*;
use aio::sync::{Remote, serve};
use futures_util::{SinkExt, StreamExt};
use std::collections::BTreeMap;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

#[cell(persist, shared)]
pub struct Board {
    pub notes: BTreeMap<Uuid, String>,
    pub luck: u64,
}

#[methods]
impl Board {
    #[client]
    pub fn add(&mut self, cx: &Cx, text: String) -> Result<Uuid> {
        ensure!(!text.is_empty(), "empty note");
        let id = cx.id();
        self.notes.insert(id, text);
        self.luck = cx.rand();
        Ok(id)
    }

    #[derived]
    pub fn longest(&self) -> usize {
        LONGEST.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.notes.values().map(String::len).max().unwrap_or(0)
    }
}

static LONGEST: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

async fn server() -> (Handle<Board>, String) {
    let h = Handle::<Board>::memory();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(serve(h.clone(), l, "secret".into()));
    (h, addr)
}

/// A fake server's next text message from the client (pings skipped, and
/// health reports: a test here may degrade a part of this process).
async fn next_text<S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin>(
    w: &mut S,
) -> serde_json::Value {
    loop {
        if let Message::Text(t) = w.next().await.unwrap().unwrap() {
            let m: serde_json::Value = serde_json::from_str(&t).unwrap();
            if m["t"] != "diag" && m["t"] != "heard" {
                return m;
            }
        }
    }
}

async fn until(what: &str, f: impl Fn() -> bool) {
    for _ in 0..300 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out: {what}");
}

#[tokio::test(flavor = "multi_thread")]
async fn copies_follow_the_server_bit_for_bit() {
    let (h, addr) = server().await;
    h.add("before".into()).unwrap();
    let a = Remote::<Board>::connect(&addr, "secret", "ana").await.unwrap();
    let b = Remote::<Board>::connect(&addr, "secret", "bo").await.unwrap();
    assert_eq!(*a.state(), *h.state());
    // Predicted: shown at once, before the server answers.
    let id = a.add("from a".into()).unwrap();
    assert_eq!(a.state().notes[&id], "from a");
    until("confirmed", || a.pending() == 0).await;
    until("b follows", || *b.state() == *h.state()).await;
    // Same ids on both sides; the random number is the server's.
    assert_eq!(*a.state(), *h.state());
    assert_eq!(*a.confirmed(), *h.state());
    let by = h.store(|s| s.events().unwrap()).last().unwrap().by.clone();
    assert_eq!(by, By::User("ana".into()));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_copy_knows_its_links_round_trip() {
    // aio vitals' RTT probe: the pings that watch the link measure it too.
    let (_h, addr) = server().await;
    let a = Remote::<Board>::connect(&addr, "secret", "ana").await.unwrap();
    until("a ping answered", || a.rtt().is_some()).await;
    assert!(a.rtt().unwrap() < Duration::from_secs(1), "{:?}", a.rtt());
    assert_eq!(aio::Source::Local(Handle::<Board>::memory()).status().rtt, None, "nothing to measure here");
}

#[tokio::test(flavor = "multi_thread")]
async fn refusals_and_wrong_tokens() {
    let (h, addr) = server().await;
    let a = Remote::<Board>::connect(&addr, "secret", "ana").await.unwrap();
    // Refused on the copy already: nothing is sent.
    assert_eq!(a.add(String::new()), Err(Rejected::new("empty note")));
    assert_eq!(a.pending(), 0);
    assert_eq!(h.store(|s| s.seq()), 0);
    assert!(Remote::<Board>::connect(&addr, "wrong", "eve").await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_gap_takes_the_full_state() {
    // A fake server: a state at event 5, then an event that follows event 7.
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let fake = tokio::spawn(async move {
        let (s, _) = l.accept().await.unwrap();
        let mut w = tokio_tungstenite::accept_async(s).await.unwrap();
        let send = |v: serde_json::Value| Message::text(v.to_string());
        next_text(&mut w).await;
        w.send(send(serde_json::json!({ "t": "state", "seq": 5, "state": Board::default() }))).await.unwrap();
        w.send(send(serde_json::json!({ "t": "patch", "prev": 7, "seq": 9, "ops": [] }))).await.unwrap();
        let asked: serde_json::Value = next_text(&mut w).await;
        assert_eq!(asked["t"], "resync");
        let st = Board { luck: 42, ..Default::default() };
        w.send(send(serde_json::json!({ "t": "state", "seq": 9, "state": st }))).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
    });
    let r = Remote::<Board>::connect(&addr, "t", "u").await.unwrap();
    until("resynced", || r.seq() == 9 && r.state().luck == 42).await;
    fake.await.unwrap();
    until("offline", || !r.online()).await;
    // Offline calls wait for the server.
    assert!(r.add("x".into()).is_ok());
    assert_eq!(r.pending(), 1);
    assert_eq!((r.status().conn.key(), r.status().online), ("offline", false));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_prediction_rolls_back() {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        let (s, _) = l.accept().await.unwrap();
        let mut w = tokio_tungstenite::accept_async(s).await.unwrap();
        let send = |v: serde_json::Value| Message::text(v.to_string());
        next_text(&mut w).await;
        w.send(send(serde_json::json!({ "t": "state", "seq": 0, "state": Board::default() }))).await.unwrap();
        let call: serde_json::Value = next_text(&mut w).await;
        // Late enough that the test sees the prediction first.
        tokio::time::sleep(Duration::from_millis(300)).await;
        w.send(send(serde_json::json!({ "t": "reply", "op": call["op"], "err": "the board is locked" })))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
    });
    let r = Remote::<Board>::connect(&addr, "t", "u").await.unwrap();
    let said = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let s2 = said.clone();
    r.on_refused(move |m, e| *s2.lock().unwrap() = format!("{m}: {e}"));
    let id = r.add("hi".into()).unwrap();
    assert!(r.state().notes.contains_key(&id), "shown at once");
    until("rolled back", || r.pending() == 0).await;
    assert!(r.state().notes.is_empty());
    assert_eq!(*said.lock().unwrap(), "add: the board is locked");
}

#[test]
fn a_resent_call_applies_once() {
    let h = Handle::<Board>::memory().by(By::User("ana".into()));
    let op = aio::uuid_v7().to_string();
    let first = h.call_from("add", serde_json::json!(["a"]), &op).unwrap().unwrap();
    assert_eq!(h.call_from("add", serde_json::json!(["a"]), &op).unwrap(), None);
    assert_eq!(h.store(|s| s.events().unwrap().len()), 1);
    // Folding the history keeps recent call ids: a late re-send still applies once.
    std::thread::sleep(std::time::Duration::from_millis(5));
    h.call("add", serde_json::json!(["b"])).unwrap();
    let later = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64 + 1;
    assert_eq!(h.store(|s| s.fold_before(later)).unwrap(), 2, "both events folded");
    assert_eq!(h.call_from("add", serde_json::json!(["a"]), &op).unwrap(), None);
    // Ids come from the call id and the caller: another user gets other ids.
    let other = Handle::<Board>::memory().by(By::User("eve".into()));
    assert_ne!(other.call_from("add", serde_json::json!(["a"]), &op).unwrap().unwrap(), first);
}

/// A count that can't go below zero: a call refused on one state and not another.
#[cell(shared)]
pub struct Stock {
    pub n: u32,
}

#[methods]
impl Stock {
    #[client]
    pub fn put(&mut self) {
        self.n += 1;
    }

    #[client]
    pub fn take(&mut self) -> Result {
        ensure!(self.n > 0, "none left");
        self.n -= 1;
        Ok(())
    }
}

#[test]
fn a_resent_call_refused_once_stays_refused() {
    let h = Handle::<Stock>::memory().by(By::User("ana".into()));
    let op = aio::uuid_v7().to_string();
    assert_eq!(h.call_from("take", serde_json::Value::Null, &op).unwrap_err().why, "none left");
    h.put().unwrap();
    // Re-sent (its answer went with a cut link): refused as it was, never
    // applied on the newer state its client was never told of.
    assert_eq!(h.call_from("take", serde_json::Value::Null, &op).unwrap_err().why, "none left");
    assert_eq!(h.state().n, 1);
}

#[test]
fn call_ids_from_a_clock_far_ahead_or_too_old_are_refused() {
    let h = Handle::<Board>::memory();
    let at = |ms: i64| uuid::Builder::from_unix_timestamp_millis(ms as u64, &[7; 10]).into_uuid().to_string();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
    let ahead = h.call_from("add", serde_json::json!(["a"]), &at(now + 3_600_000)).unwrap_err();
    assert!(ahead.why.contains("clock is 3600 s ahead"), "{ahead:?}");
    assert!(
        h.call_from("add", serde_json::json!(["a"]), &at(now - 31 * 86_400_000))
            .unwrap_err()
            .why
            .contains("offline too long")
    );
    assert!(h.call_from("add", serde_json::json!(["a"]), "op-1").unwrap_err().why.contains("UUIDv7"));
    assert!(h.call_from("add", serde_json::json!(["a"]), &at(now + 60_000)).is_ok(), "a little skew is fine");
    assert_eq!(h.store(|s| s.events().unwrap().len()), 1);
}

/// A TCP relay the test can cut and pause (paused: new connections are dropped).
struct Relay {
    addr: String,
    live: std::sync::Arc<std::sync::Mutex<Vec<tokio::task::AbortHandle>>>,
    paused: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

async fn relay(to: String) -> Relay {
    use std::sync::atomic::Ordering;
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let r = Relay { addr, live: Default::default(), paused: Default::default() };
    let (live, paused) = (r.live.clone(), r.paused.clone());
    tokio::spawn(async move {
        while let Ok((mut a, _)) = l.accept().await {
            if paused.load(Ordering::SeqCst) {
                continue; // dropped: the client sees the connection fail
            }
            let to = to.clone();
            let t = tokio::spawn(async move {
                let mut b = tokio::net::TcpStream::connect(to).await.unwrap();
                _ = tokio::io::copy_bidirectional(&mut a, &mut b).await;
            });
            live.lock().unwrap().push(t.abort_handle());
        }
    });
    r
}

#[tokio::test(flavor = "multi_thread")]
async fn reconnects_and_sends_what_waited() {
    use std::sync::atomic::Ordering;
    let (h, addr) = server().await;
    let relay = relay(addr).await;
    let r = Remote::<Board>::connect(&relay.addr, "secret", "ana").await.unwrap();
    relay.paused.store(true, Ordering::SeqCst);
    relay.live.lock().unwrap().drain(..).for_each(|t| t.abort());
    until("offline", || !r.online()).await;
    h.add("meanwhile".into()).unwrap();
    let id = r.add("while offline".into()).unwrap();
    assert_eq!(r.pending(), 1);
    relay.paused.store(false, Ordering::SeqCst);
    until("back and sent", || r.online() && r.pending() == 0).await;
    assert_eq!(h.state().notes[&id], "while offline");
    until("caught up", || *r.state() == *h.state()).await;
    assert_eq!(h.state().notes.len(), 2);
}

fn mine(b: &Diary, viewer: &Caller) -> Diary {
    let name = match &viewer.by {
        By::User(u) => u.as_str(),
        _ => "",
    };
    Diary {
        entries: b.entries.iter().filter(|(_, (who, _))| who == name).map(|(k, v)| (*k, v.clone())).collect(),
        ..b.clone()
    }
}

#[cell(persist, shared, visible = mine)]
pub struct Diary {
    /// id → (author, text)
    pub entries: BTreeMap<Uuid, (String, String)>,
    #[server]
    pub audit: Hidden<Vec<String>>,
}

#[methods]
impl Diary {
    /// It writes a `#[server]` field, which a copy never has: only the server runs it.
    #[client(predict = false)]
    pub fn write(&mut self, cx: &Cx, who: String, text: String) -> Result {
        let id = cx.id();
        self.audit.push(format!("{who} wrote"));
        self.entries.insert(id, (who, text));
        Ok(())
    }

    /// Predicted, it would read the audit on the copy: it fails loudly there.
    #[client]
    pub fn audited(&mut self) -> Result<usize> {
        Ok(self.audit.len())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn each_user_sees_only_what_they_may() {
    let h = Handle::<Diary>::memory();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(serve(h.clone(), l, "secret".into()));
    let ana = Remote::<Diary>::connect(&addr, "secret", "ana").await.unwrap();
    let bo = Remote::<Diary>::connect(&addr, "secret", "bo").await.unwrap();
    ana.write("ana".into(), "ana's".into()).unwrap();
    bo.write("bo".into(), "bo's".into()).unwrap();
    until("confirmed", || ana.pending() == 0 && bo.pending() == 0).await;
    until("ana sees hers", || ana.confirmed().entries.len() == 1).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(h.state().entries.len(), 2);
    assert_eq!(h.state().audit.len(), 2);
    let hers: Vec<_> = h.state().entries.iter().filter(|(_, (w, _))| w == "ana").map(|(id, _)| *id).collect();
    assert_eq!(ana.confirmed().entries.keys().copied().collect::<Vec<_>>(), hers);
    assert_eq!(bo.confirmed().entries.len(), 1);
    assert!(
        ana.confirmed().audit.get().is_none() && bo.confirmed().audit.get().is_none(),
        "#[server] stays on the server"
    );
    let e = ana.audited().unwrap_err();
    assert!(
        e.why.contains("Diary.audit: a #[server] field, never sent to a synced copy"),
        "read on a copy fails loudly: {e}"
    );
    let p = h.state().project(&Caller { by: By::User("ana".into()), role: None });
    assert!(p.get("audit").is_none());
}

/// One plain HTTP/1.1 POST; the status and the JSON body.
async fn post(addr: &str, path: &str, body: &str) -> (u16, String, serde_json::Value) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    let (head, body) = out.split_once("\r\n\r\n").unwrap();
    let status = head[9..12].parse().unwrap();
    (status, head.to_ascii_lowercase(), serde_json::from_str(body).unwrap_or_default())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pin_pairs_a_new_device() {
    let h = Handle::<Board>::memory();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let pairing = std::sync::Arc::new(aio::pairing::Pairing::default());
    tokio::spawn(aio::sync::serve_with(
        h,
        l,
        aio::sync::Options { token: "secret".into(), pairing: Some(pairing.clone()), tls: None, ..Default::default() },
    ));
    // No PIN shown yet, a bad body, a too-big body.
    assert_eq!(post(&addr, "/__aio/pair", r#"{"pin":"123456"}"#).await.0, 401);
    assert_eq!(post(&addr, "/__aio/pair", "nope").await.0, 400);
    assert_eq!(post(&addr, "/__aio/pair", &format!(r#"{{"pin":"{}"}}"#, "1".repeat(5000))).await.0, 413);
    let pin = pairing.new_pin();
    let (status, head, profile) = post(&addr, "/__aio/pair", &format!(r#"{{"pin":"{pin}"}}"#)).await;
    assert_eq!(status, 200);
    assert!(head.contains("cache-control: no-store"));
    assert_eq!(profile["key"], "secret");
    assert_eq!(profile["name"], "board");
    assert_eq!(profile["aio"], 1);
    // The key works; the PIN is used up.
    let key = profile["key"].as_str().unwrap();
    assert!(Remote::<Board>::connect(&addr, key, "new").await.is_ok());
    assert_eq!(post(&addr, "/__aio/pair", &format!(r#"{{"pin":"{pin}"}}"#)).await.0, 401);
    // Without pairing the route isn't there.
    let (_, plain) = server().await;
    assert_eq!(post(&plain, "/__aio/pair", r#"{"pin":"1"}"#).await.0, 404);
}

type Sock = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn recv(w: &mut Sock) -> serde_json::Value {
    loop {
        if let Message::Text(t) =
            tokio::time::timeout(Duration::from_secs(5), w.next()).await.unwrap().unwrap().unwrap()
        {
            return serde_json::from_str(&t).unwrap();
        }
    }
}

async fn send(w: &mut Sock, v: serde_json::Value) {
    w.send(Message::text(v.to_string())).await.unwrap();
}

/// The next ack, skipping other frames.
#[cfg(feature = "compat")]
async fn ack(w: &mut Sock) -> serde_json::Value {
    loop {
        let v = recv(w).await;
        if v["t"] == "ack" {
            return v["d"].clone();
        }
    }
}

/// Without feature `compat`, a socket that doesn't say wire 2 is refused before
/// the upgrade (426), and one that does is served.
#[cfg(not(feature = "compat"))]
#[tokio::test(flavor = "multi_thread")]
async fn without_compat_only_wire_2_is_spoken() {
    use tokio_tungstenite::tungstenite::Error;
    let (_h, addr) = server().await;
    match tokio_tungstenite::connect_async(format!("ws://{addr}/ws?token=secret")).await {
        Err(Error::Http(r)) => {
            assert_eq!(r.status(), 426);
            let why = String::from_utf8(r.body().clone().unwrap_or_default()).unwrap();
            assert!(why.contains("aio-rs's wire 2 only"), "{why}");
        }
        other => panic!("{:?}", other.map(|_| ())),
    }
    assert!(tokio_tungstenite::connect_async(format!("ws://{addr}/ws?token=secret&wire=2")).await.is_ok());
}

#[cfg(feature = "compat")]
#[tokio::test(flavor = "multi_thread")]
async fn an_aio_client_speaks_protocol_v3() {
    use serde_json::json;
    let (h, addr) = server().await;
    h.add("before".into()).unwrap();
    let (mut w, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws?token=secret")).await.unwrap();
    let proto = recv(&mut w).await;
    assert_eq!(
        (&proto["v"], &proto["t"], &proto["d"]["v"], &proto["d"]["min"]),
        (&json!(2), &json!("proto"), &json!(3), &json!(3))
    );
    let state = recv(&mut w).await;
    assert_eq!(state["t"], "state");
    assert_eq!(state["d"]["board"]["notes"].as_object().unwrap().len(), 1);
    assert_eq!(recv(&mut w).await["t"], "boot");
    send(&mut w, json!({ "v": 2, "t": "proto", "d": { "v": 3, "min": 3 } })).await;
    send(
        &mut w,
        json!({ "v": 2, "t": "action", "d": { "type": "board:add", "payload": { "args": ["from aio"] }, "cid": "c1" } }),
    )
    .await;
    // The change first, then the ack with the method's value.
    let patches = recv(&mut w).await;
    assert_eq!(patches["t"], "patches");
    assert!(patches["d"].as_array().unwrap().iter().all(|p| p["path"][0] == "board"));
    let got = ack(&mut w).await;
    assert_eq!((&got["cid"], &got["ok"]), (&json!("c1"), &json!(true)));
    let id: Uuid = serde_json::from_value(got["value"].clone()).unwrap();
    assert_eq!(h.state().notes[&id], "from aio");
    // A refused call is an ack with the reason; internals can't be called.
    send(
        &mut w,
        json!({ "v": 2, "t": "action", "d": { "type": "board:add", "payload": { "args": [""] }, "cid": "c2" } }),
    )
    .await;
    assert_eq!(ack(&mut w).await, json!({ "cid": "c2", "ok": false, "error": "empty note" }));
    send(
        &mut w,
        json!({ "v": 2, "t": "action", "d": { "type": "board:__x", "payload": { "args": [] }, "cid": "c3" } }),
    )
    .await;
    assert_eq!(ack(&mut w).await["ok"], false);
    // A client that needs a newer protocol is told, then closed with 4505.
    send(&mut w, json!({ "v": 2, "t": "proto", "d": { "v": 4, "min": 4 } })).await;
    assert_eq!(recv(&mut w).await["t"], "proto-err");
    match tokio::time::timeout(Duration::from_secs(5), w.next()).await.unwrap() {
        Some(Ok(Message::Close(Some(f)))) => assert_eq!(u16::from(f.code), 4505),
        other => panic!("{other:?}"),
    }
}

/// aio-rs as a client of an aio app: aio's protocol v3, against the v3 server
/// (`an_aio_client_speaks_protocol_v3` pins it to aio's frames).
#[cfg(feature = "compat")]
#[tokio::test(flavor = "multi_thread")]
async fn an_aio_rs_app_reads_and_calls_an_aio_app() {
    use aio::{compat::AioLink, sync::Link};
    let (h, addr) = server().await;
    h.add("before".into()).unwrap();
    let wrong = Link { addr: addr.clone(), token: "nope".into(), root: None };
    assert_eq!(AioLink::connect(&wrong).await.err().unwrap().kind(), std::io::ErrorKind::PermissionDenied);
    let mut deno = AioLink::connect(&Link { addr, token: "secret".into(), root: None }).await.unwrap();
    assert!(!deno.boot().is_empty());
    assert_eq!(deno.cell::<Board>("board").unwrap(), *h.state(), "the cell as its own type");
    // A call: its value, its change already in.
    let id: Uuid =
        serde_json::from_value(deno.call("board", "add", serde_json::json!(["from aio-rs"])).await.unwrap()).unwrap();
    assert_eq!(deno.cells()["board"]["notes"][id.to_string()], "from aio-rs");
    assert_eq!(h.state().notes[&id], "from aio-rs");
    assert_eq!(deno.call("board", "add", serde_json::json!([""])).await.unwrap_err().why, "empty note");
    assert!(deno.call("board", "__x", serde_json::json!([])).await.is_err());
    // Its changes come in as aio's patches.
    let other = h.add("theirs".into()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), deno.changed()).await.unwrap().unwrap();
    assert_eq!(deno.cell::<Board>("board").unwrap().notes[&other], "theirs");
    // Only the cells it asks for.
    deno.subscribe(&["other"]).await.unwrap();
    assert!(deno.cell::<Board>("board").is_err());
    deno.subscribe(&["*"]).await.unwrap();
    assert_eq!(deno.cell::<Board>("board").unwrap(), *h.state());
    assert!(deno.subscribe(&[]).await.unwrap_err().why.contains("refused the subscription"));
}

#[cfg(feature = "compat")]
#[tokio::test(flavor = "multi_thread")]
async fn an_aio_client_gets_only_the_cells_it_subscribed_to() {
    use serde_json::json;
    let (h, addr) = server().await;
    let (mut w, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws?token=secret")).await.unwrap();
    for _ in 0..3 {
        recv(&mut w).await; // proto, state, boot
    }
    let subs = |s: serde_json::Value| json!({ "v": 2, "t": "subs", "d": { "subs": s } });
    send(&mut w, subs(json!(["other"]))).await;
    let st = recv(&mut w).await;
    assert_eq!((&st["t"], &st["d"], &st["s"]), (&json!("state"), &json!({}), &json!(["other"])));
    h.add("unseen".into()).unwrap();
    let quiet = tokio::time::timeout(Duration::from_millis(300), recv(&mut w)).await;
    assert!(quiet.is_err(), "no patches for a cell it doesn't show: {quiet:?}");
    // A path inside the cell shows the whole cell, as aio's server does.
    send(&mut w, subs(json!(["board.notes"]))).await;
    let st = recv(&mut w).await;
    assert_eq!(
        (st["d"]["board"]["notes"].as_object().unwrap().len(), &st["d"]["board"]["luck"]),
        (1, &json!(h.state().luck))
    );
    h.add("seen".into()).unwrap();
    assert_eq!(recv(&mut w).await["t"], "patches");
    // Nothing (or too much) is refused out loud, and the set stays.
    send(&mut w, subs(json!([]))).await;
    let d = recv(&mut w).await;
    assert_eq!((&d["t"], &d["d"]["type"]), (&json!("diag"), &json!("subs-refused")));
    h.add("still".into()).unwrap();
    assert_eq!(recv(&mut w).await["t"], "patches");
}

#[cfg(feature = "compat")]
#[tokio::test(flavor = "multi_thread")]
async fn an_aio_page_gets_its_vitals_pong_and_reports_what_is_degraded() {
    use serde_json::json;
    let (_h, addr) = server().await;
    let (mut w, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws?token=secret")).await.unwrap();
    for _ in 0..3 {
        recv(&mut w).await;
    }
    send(&mut w, json!({ "v": 2, "t": "vitals-ping", "d": { "t1": 1234, "ms": 3 } })).await;
    let pong = recv(&mut w).await;
    assert_eq!((&pong["t"], &pong["d"]["t1"]), (&json!("vitals-pong"), &json!(1234)));
    assert!(pong["d"]["t2"].as_i64().unwrap() > 0);
    let said = || aio::health::client_degraded().into_iter().find(|(n, ..)| n == "v1page:storage");
    let cdiag = |kind: &str| json!({ "v": 2, "t": "cdiag", "d": { "name": "v1page:storage", "kind": kind, "failures": 6, "since": 1, "lastError": "quota" } });
    send(&mut w, cdiag("down")).await;
    until("the server's health has it", || said().is_some_and(|(_, n, e)| n == 1 && e.failures == 6)).await;
    send(&mut w, cdiag("up")).await;
    until("recovered", || said().is_none()).await;
    send(&mut w, cdiag("down")).await;
    until("down again", || said().is_some()).await;
    drop(w);
    until("gone with the page", || said().is_none()).await;
}

/// The next frame of kind `t`, skipping others.
#[cfg(feature = "compat")]
async fn recv_kind(w: &mut Sock, t: &str) -> serde_json::Value {
    loop {
        let v = recv(w).await;
        if v["t"] == t {
            return v["d"].clone();
        }
    }
}

/// aio's time-travel panel over the v3 wire: an admin is greeted with where
/// the cell is, moves it and is shown each change; anyone else, neither.
#[cfg(feature = "compat")]
#[tokio::test(flavor = "multi_thread")]
async fn an_aio_admin_drives_time_travel_and_a_user_may_not() {
    use serde_json::json;
    const ANN: &str = "ann-0123456789abcdef0123456789abcdef";
    const ROOT: &str = "root-0123456789abcdef0123456789abcdef";
    let users = aio::auth::Users::map([
        (ANN.to_string(), ("ann".to_string(), "user".to_string())),
        (ROOT.to_string(), ("root".to_string(), "admin".to_string())),
    ]);
    let h = Handle::<Board>::memory();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let opts = aio::sync::Options {
        token: "secret".into(),
        http: aio::http::Http::default().users(users),
        ..Default::default()
    };
    tokio::spawn(aio::sync::serve_with(h.clone(), l, opts));
    h.add("one".into()).unwrap();
    h.add("two".into()).unwrap();
    let open = |token: &str| {
        let url = format!("ws://{addr}/ws?token={token}");
        async move {
            let (mut w, _) = tokio_tungstenite::connect_async(url).await.unwrap();
            for _ in 0..3 {
                recv(&mut w).await; // proto, state, boot
            }
            send(&mut w, json!({ "v": 2, "t": "type", "d": { "kind": "browser" } })).await;
            w
        }
    };
    let tt = |cmd: &str| json!({ "v": 2, "t": "tt-cmd", "d": { "cmd": cmd } });
    let mut root = open(ROOT).await;
    let first = recv_kind(&mut root, "tt-state").await;
    let entries = first["entries"].as_array().unwrap();
    assert_eq!(entries.iter().map(|e| e["type"].as_str().unwrap()).collect::<Vec<_>>(), ["board:add", "board:add"]);
    assert_eq!((&first["index"], &first["paused"]), (&json!(1), &json!(false)), "at the newest");
    let one = entries[0]["id"].as_u64().unwrap();
    // A user is never greeted, and its steps move nothing.
    let mut ann = open(ANN).await;
    send(&mut ann, tt("undo")).await;
    send(&mut ann, json!({ "v": 2, "t": "resync" })).await;
    assert_eq!(recv(&mut ann).await["t"], "state", "no tt-state for a user");
    assert_eq!(h.state().notes.len(), 2);
    // The admin's step: the change, and where the cell is then (paused).
    send(&mut root, tt(&format!("goto:{one}"))).await;
    let at = recv_kind(&mut root, "tt-state").await;
    assert_eq!(at["paused"], true);
    assert_eq!(at["entries"][at["index"].as_u64().unwrap() as usize]["id"], one, "{at}");
    assert_eq!(h.state().notes.len(), 1);
    assert!(h.add("held".into()).is_err(), "paused by travel");
    send(&mut root, tt("resume")).await;
    assert_eq!(recv_kind(&mut root, "tt-state").await["paused"], false);
    // Driving it subscribed it: a change elsewhere is shown, until `unsub`.
    h.add("three".into()).unwrap();
    assert_eq!(
        recv_kind(&mut root, "tt-state").await["entries"].as_array().unwrap().last().unwrap()["type"],
        "board:add"
    );
    send(&mut root, tt("unsub")).await;
    send(&mut root, tt("goto:x")).await;
    // Answered after those two: the change below comes once they are in.
    send(&mut root, json!({ "v": 2, "t": "resync" })).await;
    assert_eq!(recv(&mut root).await["t"], "state", "none for a bad goto");
    h.add("four".into()).unwrap();
    assert_eq!(recv(&mut root).await["t"], "patches");
    send(&mut root, json!({ "v": 2, "t": "resync" })).await;
    assert_eq!(recv(&mut root).await["t"], "state", "no tt-state once unsubscribed");
}

/// With users, a tab's signed-in cookie outranks a `?token=` in the URL: a
/// link someone sends can't make the tab act as the link's user.
#[cfg(feature = "compat")]
#[tokio::test(flavor = "multi_thread")]
async fn with_users_the_cookie_outranks_a_token_in_the_url() {
    use serde_json::json;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    const ANN: &str = "ann-0123456789abcdef0123456789abcdef";
    const ROOT: &str = "root-0123456789abcdef0123456789abcdef";
    let users = aio::auth::Users::map([
        (ANN.to_string(), ("ann".to_string(), "user".to_string())),
        (ROOT.to_string(), ("root".to_string(), "admin".to_string())),
    ]);
    let h = Handle::<Board>::memory();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let opts = aio::sync::Options {
        token: "secret".into(),
        http: aio::http::Http::default().users(users),
        ..Default::default()
    };
    tokio::spawn(aio::sync::serve_with(h.clone(), l, opts));
    h.add("one".into()).unwrap();
    // An admin is greeted with time travel; a user never is.
    let greeted = |cookie: Option<&str>, token: &str| {
        let mut req = format!("ws://{addr}/ws?token={token}").into_client_request().unwrap();
        if let Some(c) = cookie {
            req.headers_mut().insert("cookie", format!("aio_session_board={c}").parse().unwrap());
        }
        async move {
            let (mut w, _) = tokio_tungstenite::connect_async(req).await.unwrap();
            for _ in 0..3 {
                recv(&mut w).await; // proto, state, boot
            }
            send(&mut w, json!({ "v": 2, "t": "type", "d": { "kind": "browser" } })).await;
            send(&mut w, json!({ "v": 2, "t": "resync" })).await;
            recv(&mut w).await["t"] == "tt-state"
        }
    };
    assert!(greeted(None, ROOT).await, "the URL's token alone signs in");
    assert!(!greeted(Some(ANN), ROOT).await, "the tab's own cookie wins over the link's token");
}

/// The same cell, one version on: a newer app.
#[cell(persist, shared, version = 2, id = "board")]
pub struct BoardV2 {
    pub notes: BTreeMap<Uuid, String>,
    pub luck: u64,
}

#[methods]
impl BoardV2 {
    #[client]
    pub fn add(&mut self, text: String) -> Result<()> {
        ensure!(!text.is_empty(), "empty note");
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn copies_of_another_cell_version_are_refused() {
    let (_h, addr) = server().await;
    let e = Remote::<BoardV2>::connect(&addr, "secret", "ana").await.err().unwrap();
    assert_eq!(e.kind(), std::io::ErrorKind::Unsupported);
    assert!(e.to_string().contains("board v1, the app has v2 — update the server"), "{e}");
}

#[tokio::test(flavor = "multi_thread")]
async fn tls_pins_the_paired_root() {
    use aio::sync::Link;
    let roots = tempfile::tempdir().unwrap();
    let tls = aio::tls::server(&roots.path().join("a"), "board").unwrap();
    let root = tls.root_pem.clone();
    let h = Handle::<Board>::memory();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let pairing = std::sync::Arc::new(aio::pairing::Pairing::default());
    tokio::spawn(aio::sync::serve_with(
        h.clone(),
        l,
        aio::sync::Options {
            token: "secret".into(),
            pairing: Some(pairing.clone()),
            tls: Some(tls),
            ..Default::default()
        },
    ));
    // Pairing over TLS hands out the root to pin.
    let pin = pairing.new_pin();
    let (profile, _) = tls_post(&addr, &root, "/__aio/pair", &format!(r#"{{"pin":"{pin}"}}"#)).await;
    assert_eq!((profile["tls"].clone(), profile["cert"].clone()), (true.into(), root.clone().into()));
    // An aio client whose pin misses asks for proof of the key on the cert it sees.
    let nonce = "0f".repeat(16);
    let tag = aio::pairing::pin_proof_tag("secret", &nonce);
    let (v, leaf) =
        tls_post(&addr, &root, "/__aio/pin-proof", &format!(r#"{{"nonce":"{nonce}","tag":"{tag}"}}"#)).await;
    let cert = aio::pairing::served_cert(&leaf, &root);
    assert_eq!(
        (v["cert"].as_str(), v["proof"].as_str()),
        (Some(cert.as_str()), Some(&*aio::pairing::pin_proof_for("secret", &nonce, &cert)))
    );
    let wrong = aio::pairing::pin_proof_tag("guess", &nonce);
    let (v, _) = tls_post(&addr, &root, "/__aio/pin-proof", &format!(r#"{{"nonce":"{nonce}","tag":"{wrong}"}}"#)).await;
    assert!(v["proof"].is_null() && v["error"].as_str().is_some_and(|e| e.contains("not this app's key")), "{v}");
    // The pinned root works; copies follow as over plain sockets.
    let link = Link { addr: addr.clone(), token: "secret".into(), root: Some(root) };
    let a = Remote::<Board>::connect_to(link.clone(), "ana").await.unwrap();
    a.add("over tls".into()).unwrap();
    until("confirmed", || a.pending() == 0).await;
    assert_eq!(*a.state(), *h.state());
    // Another machine's root, or no TLS, can't connect.
    let other = aio::tls::server(&roots.path().join("b"), "board").unwrap().root_pem;
    assert!(Remote::<Board>::connect_to(Link { root: Some(other), ..link }, "eve").await.is_err());
    assert!(Remote::<Board>::connect(&addr, "secret", "eve").await.is_err());
}

/// One JSON POST over TLS pinned to `root`: the answer, and the leaf the server showed.
async fn tls_post(addr: &str, root: &str, path: &str, body: &str) -> (serde_json::Value, Vec<u8>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
    let cfg = aio::tls::client(root).unwrap();
    let name = rustls::pki_types::ServerName::try_from("127.0.0.1").unwrap();
    let mut s = tokio_rustls::TlsConnector::from(cfg).connect(name, tcp).await.unwrap();
    let leaf = s.get_ref().1.peer_certificates().unwrap()[0].to_vec();
    s.write_all(
        format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let mut out = String::new();
    _ = s.read_to_string(&mut out).await;
    (serde_json::from_str(out.split_once("\r\n\r\n").unwrap().1).unwrap(), leaf)
}

async fn pairing_server(tls: Option<aio::tls::Tls>) -> (String, std::sync::Arc<aio::pairing::Pairing>, Handle<Board>) {
    let h = Handle::<Board>::memory();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let pairing = std::sync::Arc::new(aio::pairing::Pairing::default());
    tokio::spawn(aio::sync::serve_with(
        h.clone(),
        l,
        aio::sync::Options { token: "secret".into(), pairing: Some(pairing.clone()), tls, ..Default::default() },
    ));
    (addr, pairing, h)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_device_pairs_by_pake_then_syncs_over_pinned_tls() {
    use aio::pairing::pair;
    let roots = tempfile::tempdir().unwrap();
    let (addr, pairing, h) = pairing_server(Some(aio::tls::server(roots.path(), "board").unwrap())).await;
    // A wrong code, then the right one.
    let pin = pairing.new_pake_pin();
    let wrong = if pin == "00000000" { "00000001" } else { "00000000" };
    assert_eq!(pair(&addr, wrong).await.unwrap_err().kind(), std::io::ErrorKind::PermissionDenied);
    let profile = pair(&addr, &pin).await.unwrap();
    assert_eq!((profile.name.as_str(), profile.key.as_str(), profile.tls), ("board", "secret", true));
    // Used up.
    assert!(pair(&addr, &pin).await.is_err());
    // The profile is all a device needs: a pinned TLS link.
    let link = aio::sync::Link { addr: addr.clone(), token: profile.key, root: profile.cert };
    let a = Remote::<Board>::connect_to(link, "new").await.unwrap();
    a.add("paired".into()).unwrap();
    until("synced", || a.pending() == 0).await;
    assert_eq!(*a.state(), *h.state());
}

#[tokio::test(flavor = "multi_thread")]
async fn pairing_works_plain_and_without_tls() {
    use aio::pairing::pair;
    // PAKE over plain http (binds to no certificate), and a 6-digit code.
    let (addr, pairing, _h) = pairing_server(None).await;
    let pin = pairing.new_pake_pin();
    let p = pair(&addr, &pin).await.unwrap();
    assert_eq!((p.tls, p.cert), (false, None));
    let pin = pairing.new_pin();
    assert_eq!(pair(&addr, &pin).await.unwrap().key, "secret");
    // A PAKE code is never sent, and a bad code is refused before any network use.
    assert_eq!(pair(&addr, "12345").await.unwrap_err().kind(), std::io::ErrorKind::InvalidInput);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_relay_in_the_middle_cant_pair() {
    let roots = tempfile::tempdir().unwrap();
    let real = aio::tls::server(&roots.path().join("real"), "board").unwrap();
    let real_root = real.root_pem.clone();
    let (addr, pairing, _h) = pairing_server(Some(real)).await;
    // The relay ends TLS with its own certificate and talks to the real server.
    let fake = aio::tls::server(&roots.path().join("fake"), "board").unwrap();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay = l.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        loop {
            let (tcp, _) = l.accept().await.unwrap();
            let (fake, real_root, addr) = (fake.server.clone(), real_root.clone(), addr.clone());
            tokio::spawn(async move {
                let Ok(mut client) = tokio_rustls::TlsAcceptor::from(fake).accept(tcp).await else {
                    return;
                };
                let up = tokio::net::TcpStream::connect(&addr).await.unwrap();
                let name = rustls::pki_types::ServerName::try_from("127.0.0.1").unwrap();
                let mut up = tokio_rustls::TlsConnector::from(aio::tls::client(&real_root).unwrap())
                    .connect(name, up)
                    .await
                    .unwrap();
                _ = tokio::io::copy_bidirectional(&mut client, &mut up).await;
            });
        }
    });
    let pin = pairing.new_pake_pin();
    let e = aio::pairing::pair(&relay, &pin).await.unwrap_err();
    assert!(e.to_string().contains("doesn't verify"), "{e}");
}

#[tokio::test(flavor = "multi_thread")]
async fn same_machine_tools_mint_codes_and_pairing_is_saved() {
    let data = tempfile::tempdir().unwrap();
    let key = aio::config::token(data.path()).unwrap();
    let h = Handle::<Board>::memory();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let pairing = std::sync::Arc::new(aio::pairing::Pairing::default());
    tokio::spawn(aio::sync::serve_with(
        h,
        l,
        aio::sync::Options { token: key.clone(), pairing: Some(pairing), tls: None, ..Default::default() },
    ));
    // Not serving yet (no serving.json): a clear error.
    assert_eq!(aio::pairing::mint(data.path(), false).await.unwrap_err().kind(), std::io::ErrorKind::NotFound);
    std::fs::write(
        data.path().join("serving.json"),
        serde_json::to_string(&aio::pairing::Serving { port, tls: false, host: None }).unwrap(),
    )
    .unwrap();
    let code = aio::pairing::mint(data.path(), false).await.unwrap();
    assert_eq!(code.len(), 8);
    assert_eq!(aio::pairing::mint(data.path(), true).await.unwrap().len(), 6);
    // The new device pairs with the latest code and keeps the link.
    let code = aio::pairing::mint(data.path(), false).await.unwrap();
    let addr = format!("127.0.0.1:{port}");
    let profile = aio::pairing::pair(&addr, &code).await.unwrap();
    let device = tempfile::tempdir().unwrap();
    aio::pairing::save(device.path(), &addr, &profile).unwrap();
    aio::pairing::save(device.path(), &addr, &profile).unwrap();
    let link = aio::pairing::saved(device.path()).unwrap();
    assert_eq!((link.addr.as_str(), link.token.as_str(), link.root.as_deref()), (addr.as_str(), key.as_str(), None));
    assert!(Remote::<Board>::connect_to(link, "new").await.is_ok());
    // A request a proxy on this machine relayed is from elsewhere, key or not.
    {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut s = tokio::net::TcpStream::connect(&addr).await.unwrap();
        let req = format!(
            "POST /__aio/pin HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {key}\r\nX-Forwarded-For: 203.0.113.7\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
        );
        s.write_all(req.as_bytes()).await.unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        assert!(out.starts_with("HTTP/1.1 403"), "{out}");
    }
    // Without the key, no codes.
    std::fs::write(data.path().join("sync.token"), "wrong").unwrap();
    assert_eq!(aio::pairing::mint(data.path(), false).await.unwrap_err().kind(), std::io::ErrorKind::PermissionDenied);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_closing_server_leaves_calls_queued_for_the_next_run() {
    let (h, addr) = server().await;
    let a = Remote::<Board>::connect(&addr, "secret", "ana").await.unwrap();
    _ = h.shutdown(Duration::from_millis(10)).await;
    let id = a.add("while closing".into()).unwrap();
    // Not refused and not rolled back: it waits to be re-sent.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(a.pending(), 1);
    assert_eq!(a.state().notes[&id], "while closing");
    assert!(h.state().notes.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_saved_copy_starts_offline_and_sends_its_queue_later() {
    use std::sync::atomic::Ordering;
    let (h, addr) = server().await;
    let r = relay(addr).await;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("copy.json");
    let link = aio::sync::Link { addr: r.addr.clone(), token: "secret".into(), root: None };
    let saved = |f: &std::path::Path| std::fs::read_to_string(f).unwrap_or_default();
    // Online: a confirmed note, saved.
    let a = Remote::<Board>::connect_saved(link.clone(), "ana", &file).await.unwrap();
    a.add("online".into()).unwrap();
    until("confirmed", || a.pending() == 0).await;
    until("saved", || saved(&file).contains("online") && !saved(&file).contains("\"method\"")).await;
    drop(a);
    // The server can't be reached: the app still starts, from the saved copy.
    r.paused.store(true, Ordering::SeqCst);
    for t in r.live.lock().unwrap().drain(..) {
        t.abort();
    }
    let b = Remote::<Board>::connect_saved(link.clone(), "ana", &file).await.unwrap();
    assert!(!b.online());
    assert_eq!(b.state().notes.len(), 1);
    let id = b.add("offline".into()).unwrap();
    until("queued on disk", || saved(&file).contains("\"method\":\"add\"")).await;
    drop(b);
    // Restarted, still offline: the queued call is still there.
    let c = Remote::<Board>::connect_saved(link, "ana", &file).await.unwrap();
    assert_eq!((c.pending(), c.state().notes[&id].as_str()), (1, "offline"));
    assert!(!c.online());
    assert!(!h.state().notes.contains_key(&id));
    // Back online: it reaches the server, once.
    r.paused.store(false, Ordering::SeqCst);
    until("sent", || c.pending() == 0).await;
    assert_eq!(h.state().notes[&id], "offline");
    assert_eq!(*c.state(), *h.state());
}

/// A relay whose live connections can be frozen: open, but nothing passes
/// (a dead Wi-Fi link that never says goodbye). New connections still work.
async fn freezer(to: String) -> (String, std::sync::Arc<tokio::sync::Notify>) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let freeze = std::sync::Arc::new(tokio::sync::Notify::new());
    let f = freeze.clone();
    tokio::spawn(async move {
        while let Ok((mut a, _)) = l.accept().await {
            let (to, f) = (to.clone(), f.clone());
            tokio::spawn(async move {
                let mut b = tokio::net::TcpStream::connect(to).await.unwrap();
                tokio::select! {
                    _ = tokio::io::copy_bidirectional(&mut a, &mut b) => {}
                    _ = f.notified() => std::future::pending::<()>().await,
                }
            });
        }
    });
    (addr, freeze)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_silent_link_is_found_dead_within_seconds() {
    let (h, addr) = server().await;
    let (via, freeze) = freezer(addr).await;
    let a = Remote::<Board>::connect(&via, "secret", "ana").await.unwrap();
    assert!(a.online());
    // The link goes silent; the server changes meanwhile.
    freeze.notify_waiters();
    let id = h.add("while silent".into()).unwrap();
    let t = std::time::Instant::now();
    // The copy finds the link dead, reconnects (new links pass) and catches up.
    while !a.state().notes.contains_key(&id) {
        assert!(t.elapsed() < aio::sync::DEAD + Duration::from_secs(2), "not caught up after {:?}", t.elapsed());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(t.elapsed() > Duration::from_secs(1), "caught up too soon: the link wasn't frozen");
    assert_eq!(*a.state(), *h.state());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_stops_reading_is_dropped_not_waited_for() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let (h, addr) = server().await;
    let mut req = format!("ws://{addr}/ws?token=secret").into_client_request().unwrap();
    req.headers_mut().insert("x-aio-wire", "2".parse().unwrap());
    let (mut w, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    send(&mut w, serde_json::json!({ "t": "hello", "cell": "board", "v": 1, "user": "stuck" })).await;
    // It stops reading (and so never answers pings) while the server keeps changing.
    let big = "x".repeat(64 * 1024);
    for _ in 0..32 {
        h.add(big.clone()).unwrap();
    }
    tokio::time::sleep(aio::sync::DEAD + Duration::from_secs(2)).await;
    // Reading again: what was buffered, then the end. The server let it go.
    let t = std::time::Instant::now();
    loop {
        match tokio::time::timeout(Duration::from_secs(10), w.next()).await.expect("the server still holds the link") {
            None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
            Some(Ok(_)) => assert!(t.elapsed() < Duration::from_secs(10)),
        }
    }
    // Others are unaffected.
    let a = Remote::<Board>::connect(&addr, "secret", "ana").await.unwrap();
    assert_eq!(a.state().notes.len(), 32);
}

#[tokio::test(flavor = "multi_thread")]
async fn clients_get_small_patches_and_their_own_reply_at_once() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let (h, addr) = server().await;
    for i in 0..50 {
        h.add(format!("note {i} {}", "x".repeat(200))).unwrap();
    }
    let mut req = format!("ws://{addr}/ws?token=secret").into_client_request().unwrap();
    req.headers_mut().insert("x-aio-wire", "2".parse().unwrap());
    let (mut w, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    send(&mut w, serde_json::json!({ "t": "hello", "cell": "board", "v": 1, "user": "ana" })).await;
    let full = recv(&mut w).await;
    assert_eq!(full["t"], "state");
    let full_size = full.to_string().len();
    // Someone else's change: a patch of a few hundred bytes, not the whole view again.
    h.add("small".into()).unwrap();
    let p = recv(&mut w).await;
    assert_eq!(p["t"], "patch");
    assert!(p.to_string().len() < full_size / 20, "{} vs {full_size}", p.to_string().len());
    // It names what it makes: the client checks it got there (aio's digest).
    let mut view = full["state"].clone();
    aio::patch::apply(&mut view, &serde_json::from_value::<Vec<aio::patch::Op>>(p["ops"].clone()).unwrap()).unwrap();
    assert_eq!(p["digest"], aio::patch::digest(&view));
    assert_eq!(view, serde_json::to_value(h.state().as_ref()).unwrap());
    // My own call: its change, then the reply, at once (never batched).
    let t = std::time::Instant::now();
    let op = aio::uuid_v7().to_string();
    send(&mut w, serde_json::json!({ "t": "call", "op": op, "method": "add", "args": ["mine"] })).await;
    assert_eq!(recv(&mut w).await["t"], "patch");
    let reply = recv(&mut w).await;
    assert_eq!((reply["t"].as_str(), reply["op"].as_str()), (Some("reply"), Some(op.as_str())));
    assert!(t.elapsed() < Duration::from_millis(200), "{:?}", t.elapsed());
}

async fn limited(limits: aio::sync::Limits) -> (Handle<Board>, String) {
    let h = Handle::<Board>::memory();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let opts = aio::sync::Options { token: "secret".into(), limits, ..Default::default() };
    tokio::spawn(aio::sync::serve_with(h.clone(), l, opts));
    (h, addr)
}

#[tokio::test(flavor = "multi_thread")]
async fn limits_tell_the_sender_and_never_drop() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let limits = aio::sync::Limits { clients: 1, max_message: 2048, calls_per_sec: 100, ..Default::default() };
    let (h, addr) = limited(limits).await;
    let a = Remote::<Board>::connect(&addr, "secret", "ana").await.unwrap();
    // One client at most: the next is told why.
    let e = Remote::<Board>::connect(&addr, "secret", "bo").await.err().unwrap();
    assert!(e.to_string().contains("too many clients (the limit is 1)"), "{e}");
    // Too big: the copy refuses it at once (it learned the limit), nothing is sent.
    let e = a.add("x".repeat(4096)).unwrap_err();
    assert!(e.why.contains("its limit is 2048"), "{e:?}");
    assert_eq!(a.pending(), 0);
    // A flood is paced, not dropped: all 250 land, at about the rate.
    let t = std::time::Instant::now();
    for i in 0..250 {
        a.add(format!("{i}")).unwrap();
    }
    for _ in 0..100 {
        if a.pending() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(h.state().notes.len(), 250);
    assert!(t.elapsed() > Duration::from_millis(400), "not paced: {:?}", t.elapsed());
    // Dropping the copy closes its link: the slot is free again.
    drop(a);
    tokio::time::sleep(Duration::from_millis(1000)).await;
    // A client that ignores the size limit gets a close naming it.
    let mut req = format!("ws://{addr}/ws?token=secret").into_client_request().unwrap();
    req.headers_mut().insert("x-aio-wire", "2".parse().unwrap());
    let (mut w, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    send(&mut w, serde_json::json!({ "t": "hello", "cell": "board", "v": 1, "user": "raw", "pad": "x".repeat(4096) }))
        .await;
    loop {
        match w.next().await {
            Some(Ok(Message::Close(Some(f)))) => {
                assert_eq!(u16::from(f.code), 1009);
                assert!(f.reason.contains("the limit is 2048"), "{}", f.reason);
                break;
            }
            Some(Ok(_)) => {}
            other => panic!("no close frame: {other:?}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn internal_methods_are_refused_and_the_caller_is_set_by_the_server() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let (h, addr) = server().await;
    h.add("kept".into()).unwrap();
    let mut req = format!("ws://{addr}/ws?token=secret").into_client_request().unwrap();
    req.headers_mut().insert("x-aio-wire", "2".parse().unwrap());
    let (mut w, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    send(&mut w, serde_json::json!({ "t": "hello", "cell": "board", "v": 1, "user": "eve" })).await;
    assert_eq!(recv(&mut w).await["t"], "state");
    // The log's own rewind, sent as a call with a made-up state: refused, nothing changes.
    let forged = serde_json::json!({ "to": 0, "state": { "notes": {}, "luck": 7 } });
    for method in ["$rewind", "__rewind", "apply", ""] {
        let op = aio::uuid_v7().to_string();
        send(&mut w, serde_json::json!({ "t": "call", "op": op, "method": method, "args": forged })).await;
        let r = recv(&mut w).await;
        assert!(r["err"].is_string(), "{method}: {r}");
    }
    assert_eq!(h.state().notes.len(), 1);
    // A call that names its own caller is recorded as the link's user, not as the system.
    let op = aio::uuid_v7().to_string();
    let msg = serde_json::json!({ "t": "call", "op": op, "method": "add", "args": ["x"], "by": "System" });
    send(&mut w, msg).await;
    assert_eq!(recv(&mut w).await["t"], "patch");
    assert!(recv(&mut w).await["ok"].is_string());
    assert_eq!(h.store(|s| s.events().unwrap()).last().unwrap().by, By::User("eve".into()));
}

/// One raw HTTP/1.1 request; the status and the lower-cased head.
async fn raw(addr: &str, head: &str) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(format!("{head}Connection: close\r\n\r\n").as_bytes()).await.unwrap();
    // The head only: after a 101 come WebSocket frames.
    let mut out = Vec::new();
    while !out.windows(4).any(|w| w == b"\r\n\r\n") {
        let mut b = [0; 512];
        let n = s.read(&mut b).await.unwrap();
        assert!(n > 0, "closed before the head");
        out.extend_from_slice(&b[..n]);
    }
    let out = String::from_utf8_lossy(&out).to_ascii_lowercase();
    (out[9..12].parse().unwrap(), out.split_once("\r\n\r\n").unwrap().0.to_string())
}

#[tokio::test(flavor = "multi_thread")]
async fn rebinding_cross_site_and_form_requests_are_refused() {
    let (_h, addr) = server().await;
    let ws = "GET /ws?token=secret&wire=2 HTTP/1.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n";
    // DNS rebinding: a page on another name that now points here.
    let (code, head) = raw(&addr, &format!("{ws}Host: evil.example:{}\r\n", &addr[10..])).await;
    assert_eq!(code, 403);
    assert!(
        head.contains("x-content-type-options: nosniff") && head.contains("referrer-policy: no-referrer"),
        "{head}"
    );
    // A page on another site opening the socket with a stolen key.
    let (code, _) = raw(&addr, &format!("{ws}Host: {addr}\r\nOrigin: http://evil.example\r\n")).await;
    assert_eq!(code, 403);
    // The same app's own page, and a native client (no Origin), get in.
    assert_eq!(raw(&addr, &format!("{ws}Host: {addr}\r\nOrigin: http://{addr}\r\n")).await.0, 101);
    assert_eq!(raw(&addr, &format!("{ws}Host: {addr}\r\n")).await.0, 101);
    // A cross-site form posts text/plain: refused before any route.
    let form = format!(
        "POST /__aio/pair HTTP/1.1\r\nHost: {addr}\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\n{{}}"
    );
    assert_eq!(raw(&addr, &form.replace("\r\n\r\n{}", "\r\n")).await.0, 415);
}

#[tokio::test(flavor = "multi_thread")]
async fn derived_values_work_on_a_synced_copy() {
    use std::sync::atomic::Ordering::SeqCst;
    let (h, addr) = server().await;
    let a = Remote::<Board>::connect(&addr, "secret", "ana").await.unwrap();
    let runs = LONGEST.load(SeqCst);
    assert_eq!((a.state().longest(), a.state().longest(), LONGEST.load(SeqCst) - runs), (0, 0, 1), "cached");
    // A prediction is a new state, and so is the server's change arriving.
    a.add("four".into()).unwrap();
    assert_eq!((a.state().longest(), LONGEST.load(SeqCst) - runs), (4, 2));
    h.add("longer".into()).unwrap();
    until("b follows", || a.state().notes.len() == 2).await;
    assert_eq!(a.state().longest(), 6);
    assert_eq!(a.state().longest(), h.state().longest());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_patch_that_makes_another_state_takes_the_full_one() {
    // A fake server: a patch whose ops fit but make something else than its digest says.
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    // The patches wait until the test listens: a fast resync can't be missed.
    let (go, ready) = tokio::sync::oneshot::channel::<()>();
    let fake = tokio::spawn(async move {
        let (s, _) = l.accept().await.unwrap();
        let mut w = tokio_tungstenite::accept_async(s).await.unwrap();
        let send = |v: serde_json::Value| Message::text(v.to_string());
        next_text(&mut w).await;
        w.send(send(serde_json::json!({ "t": "state", "seq": 5, "state": Board::default() }))).await.unwrap();
        ready.await.unwrap();
        let luck = |n: u64| serde_json::json!([{ "op": "set", "path": ["luck"], "value": n }]);
        let good = aio::patch::digest(&serde_json::to_value(Board { luck: 1, ..Default::default() }).unwrap());
        let p = serde_json::json!({ "t": "patch", "prev": 5, "seq": 6, "ops": luck(1), "digest": good });
        w.send(send(p)).await.unwrap();
        let p = serde_json::json!({ "t": "patch", "prev": 6, "seq": 7, "ops": luck(2), "digest": good });
        w.send(send(p)).await.unwrap();
        let asked: serde_json::Value = next_text(&mut w).await;
        assert_eq!(asked["t"], "resync");
        let st = Board { luck: 3, ..Default::default() };
        w.send(send(serde_json::json!({ "t": "state", "seq": 8, "state": st }))).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
    });
    let r = Remote::<Board>::connect(&addr, "t", "u").await.unwrap();
    let synced = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let s2 = synced.clone();
    r.on_sync(move |s| s2.lock().unwrap().push(*s));
    go.send(()).unwrap();
    until("resynced, never the wrong state", || r.seq() == 8).await;
    assert_eq!(r.state().luck, 3);
    // The catch-up is told (aio onSync): what it brought and how the copy stands.
    until("the catch-up is told", || !synced.lock().unwrap().is_empty()).await;
    let s = synced.lock().unwrap()[0];
    assert_eq!((s.events, s.status.conn, s.status.pending), (2, aio::Conn::Online, 0));
    assert!(s.elapsed < Duration::from_secs(5));
    fake.await.unwrap();
}

/// A fake server that sends a state, takes calls and never answers them.
async fn deaf() -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        let (s, _) = l.accept().await.unwrap();
        let mut w = tokio_tungstenite::accept_async(s).await.unwrap();
        next_text(&mut w).await;
        let first = serde_json::json!({ "t": "state", "seq": 0, "state": Board::default() });
        w.send(Message::text(first.to_string())).await.unwrap();
        while w.next().await.is_some() {}
    });
    addr
}

#[tokio::test(flavor = "multi_thread")]
async fn the_status_says_online_syncing_and_per_method_counts() {
    let (_h, addr) = server().await;
    let now = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
    let before = now();
    let r = Remote::<Board>::connect(&addr, "secret", "ana").await.unwrap();
    let st = r.status();
    assert_eq!((st.conn, st.pending, st.online, st.degraded), (aio::Conn::Online, 0, true, false));
    assert!(st.last_sync.unwrap() >= before);
    // Calls on their way: syncing, counted per method (a spinner on its button).
    let d = Remote::<Board>::connect(&deaf().await, "t", "u").await.unwrap();
    d.add("a".into()).unwrap();
    d.add("b".into()).unwrap();
    assert_eq!((d.status().conn, d.pending_of("add"), d.pending_of("other")), (aio::Conn::Syncing, 2, 0));
    // Confirmed: online again, the server's answer just reached the copy.
    r.add("c".into()).unwrap();
    until("confirmed", || r.pending_of("add") == 0).await;
    assert_eq!(r.status().conn, aio::Conn::Online);
    assert!(r.status().last_sync >= st.last_sync);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_confirmed_call_waits_as_long_as_its_method_says() {
    let r = Remote::<Board>::connect(&deaf().await, "t", "u").await.unwrap();
    r.timeout("add", Some(Duration::from_millis(100)));
    let t = std::time::Instant::now();
    let r2 = r.clone();
    let e = tokio::task::spawn_blocking(move || r2.call_confirmed("add", serde_json::json!(["x"]))).await.unwrap();
    assert!(e.unwrap_err().why.contains("no answer in 100ms (it stays queued)"));
    assert!(t.elapsed() < Duration::from_secs(5), "not the default 15 s");
    assert_eq!(r.pending(), 1, "still sent again later");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_full_queue_blocks_unless_a_retention_lets_the_oldest_go() {
    let r = Remote::<Board>::connect(&deaf().await, "t", "u").await.unwrap();
    for i in 0..aio::sync::PENDING_MAX {
        r.add(format!("n{i}")).unwrap();
    }
    let said = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let s2 = said.clone();
    r.on_refused(move |m, e| s2.lock().unwrap().push(format!("{m}: {e}")));
    // No retention (the default): nothing is ever dropped, new calls are refused.
    assert_eq!(r.status().conn, aio::Conn::Blocked);
    assert!(r.add("new".into()).unwrap_err().why.starts_with("blocked"));
    // A retention the oldest hasn't waited out: still blocked.
    r.retain(Some(Duration::from_secs(3600)));
    assert_eq!(r.status().conn, aio::Conn::Blocked);
    assert!(r.add("new".into()).is_err());
    // Past it: a call that applies drops the oldest, once, and says what is known.
    tokio::time::sleep(Duration::from_millis(5)).await;
    r.retain(Some(Duration::from_millis(1)));
    assert_eq!(r.status().conn, aio::Conn::Syncing);
    assert!(r.add(String::new()).is_err());
    assert!(said.lock().unwrap().is_empty(), "a refused call drops nothing");
    r.add("new".into()).unwrap();
    assert_eq!(r.pending(), aio::sync::PENDING_MAX);
    let notes: Vec<String> = r.state().notes.values().cloned().collect();
    assert!(notes.contains(&"new".into()) && notes.contains(&"n1".into()) && !notes.contains(&"n0".into()));
    let said = said.lock().unwrap();
    assert_eq!(said.len(), 1);
    assert!(said[0].starts_with("add: dropped after waiting") && said[0].contains("may or may not"), "{}", said[0]);
}

#[tokio::test(flavor = "multi_thread")]
async fn what_is_degraded_on_a_copy_shows_in_its_servers_health() {
    let (_h, addr) = server().await;
    let r = Remote::<Board>::connect(&addr, "secret", "ana").await.unwrap();
    let said = || aio::health::client_degraded().into_iter().find(|(n, ..)| n == "copy:disk");
    aio::health::degrade("copy:disk", "disk full");
    until_long("the server heard", || said().is_some_and(|(_, n, e)| n == 1 && e.last_error == "disk full")).await;
    aio::health::ok("copy:disk");
    until_long("recovered", || said().is_none()).await;
    aio::health::degrade("copy:disk", "disk full");
    until_long("heard again", || said().is_some()).await;
    // A link that closes takes what its client said with it.
    drop(r);
    until_long("gone with the link", || said().is_none()).await;
    aio::health::ok("copy:disk");
}

#[tokio::test(flavor = "multi_thread")]
async fn what_a_copys_bus_hears_reaches_its_servers_bus() {
    let (_h, addr) = server().await;
    let r = Remote::<Board>::connect(&addr, "secret", "ana").await.unwrap();
    let heard = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    let h = heard.clone();
    let _sub = aio::report::subscribe(move |d| h.lock().unwrap().push((d.kind.clone(), d.message.clone())));
    // One process plays both sides here: the copy's problem is on the bus
    // once as its own, and once as the client's, relayed (and not again).
    // The other tests' copies, linked in this process too, relay it as
    // well: the first to arrive is said, the rest held (one kind).
    // Online first: `connect` waits for that only so long (a loaded machine).
    until_long("online", || r.online()).await;
    aio::report::problem("window:frozen", "copy stuck token=abc");
    until_long("relayed", || heard.lock().unwrap().iter().any(|(k, _)| k == "client:window:frozen")).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let heard = heard.lock().unwrap();
    // Only this kind: the other tests' copies relay what else their bus
    // hears meanwhile (a refused call is `client:refused`).
    let relayed: Vec<_> =
        heard.iter().filter(|(k, _)| k == "client:window:frozen" || k.starts_with("client:client:")).collect();
    assert_eq!(relayed.len(), 1, "never echoed back: {heard:?}");
    assert!(
        relayed[0].1.contains(" from 127.0.0.1:") && relayed[0].1.ends_with(": copy stuck token=…"),
        "{}",
        relayed[0].1
    );
    drop(r);
}

async fn until_long(what: &str, f: impl Fn() -> bool) {
    for _ in 0..1000 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out: {what}");
}

/// What aio's merge strategies merged, as methods: replayed in order on the
/// server, they merge themselves; prose merges with `aio::merge::text3`.
#[cell(persist, shared)]
pub struct Doc {
    pub views: u64,
    pub tags: std::collections::BTreeSet<String>,
    pub text: String,
    pub conflicts: u32,
}

#[methods]
impl Doc {
    #[client]
    pub fn view(&mut self) {
        self.views += 1;
    }

    #[client]
    pub fn tag(&mut self, t: String, on: bool) {
        if on {
            self.tags.insert(t);
        } else {
            self.tags.remove(&t);
        }
    }

    /// `new` was made from `base`: merged with what is there now.
    #[client]
    pub fn edit(&mut self, base: String, new: String) {
        let m = aio::merge::text3(&base, &new, &self.text);
        self.text = m.text;
        self.conflicts += u32::from(m.conflict);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_edits_merge_counters_sets_and_text() {
    let h = Handle::<Doc>::memory();
    h.tag("old".into(), true).unwrap();
    h.edit(String::new(), "Title\n\nfirst draft\n".into()).unwrap();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(serve(h.clone(), l, "secret".into()));
    let a = Remote::<Doc>::connect(&addr, "secret", "ana").await.unwrap();
    let b = Remote::<Doc>::connect(&addr, "secret", "bo").await.unwrap();
    // Both start from the same text and edit it without seeing the other.
    let base = b.state().text.clone();
    a.view().unwrap();
    a.tag("new".into(), true).unwrap();
    a.edit(base.clone(), "Title\n\nfirst draft, longer\n".into()).unwrap();
    b.view().unwrap();
    b.tag("old".into(), false).unwrap();
    b.edit(base, "My Title\n\nfirst draft\n".into()).unwrap();
    until("confirmed", || a.pending() == 0 && b.pending() == 0).await;
    until("both follow", || *a.state() == *h.state() && *b.state() == *h.state()).await;
    let d = h.state();
    assert_eq!(d.views, 2, "a counter counts both");
    assert_eq!(d.tags.iter().map(String::as_str).collect::<Vec<_>>(), ["new"], "an add and a remove both stay");
    assert_eq!((d.text.as_str(), d.conflicts), ("My Title\n\nfirst draft, longer\n", 0), "both edits survive");
}

/// A payment: a preview would lie, so copies wait for the server.
#[cell(persist, shared)]
pub struct Till {
    pub balance: i64,
    #[server]
    pub ledger: Hidden<Vec<i64>>,
}

#[methods]
impl Till {
    #[client(predict = false)]
    pub fn pay(&mut self, amount: i64) -> Result<()> {
        ensure!(self.balance >= amount, "not enough money");
        self.balance -= amount;
        self.ledger.push(-amount);
        Ok(())
    }

    #[client]
    pub fn note(&mut self) {}
}

/// `deaf()` for a till holding 10.
async fn deaf_till() -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        let (s, _) = l.accept().await.unwrap();
        let mut w = tokio_tungstenite::accept_async(s).await.unwrap();
        next_text(&mut w).await;
        let first = serde_json::json!({ "t": "state", "seq": 0, "state": { "balance": 10, "ledger": [] } });
        w.send(Message::text(first.to_string())).await.unwrap();
        while w.next().await.is_some() {}
    });
    addr
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unpredicted_call_shows_only_what_the_server_made_of_it() {
    let h = Handle::<Till>::memory();
    h.pay(-10).unwrap();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(serve(h.clone(), l, "secret".into()));
    let deaf = Remote::<Till>::connect(&deaf_till().await, "t", "u").await.unwrap();
    // Not shown while it waits: the copy can't know the ledger, the server can.
    deaf.pay(3).unwrap();
    assert_eq!((deaf.state().balance, deaf.pending_of("pay"), deaf.status().conn), (10, 1, aio::Conn::Syncing));
    deaf.note().unwrap();
    assert_eq!(deaf.state().balance, 10, "a later prediction doesn't apply it either");
    let r = Remote::<Till>::connect(&addr, "secret", "ana").await.unwrap();
    let said = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let s2 = said.clone();
    r.on_refused(move |m, e| s2.lock().unwrap().push(format!("{m}: {e}")));
    r.pay(4).unwrap();
    until("the server ran it", || r.pending() == 0).await;
    assert_eq!((r.state().balance, (*h.state().ledger).clone()), (6, vec![10, -4]));
    // The server's refusal: nothing was ever shown, and it is said.
    r.pay(100).unwrap();
    until("refused", || r.pending() == 0).await;
    assert_eq!(r.state().balance, 6);
    assert_eq!(*said.lock().unwrap(), ["pay: not enough money"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_watch_on_a_synced_copy_tells_only_changes() {
    let (h, addr) = server().await;
    let a = aio::Source::Remote(Remote::<Board>::connect(&addr, "secret", "ana").await.unwrap());
    let seen = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    let s = seen.clone();
    // Not `longest`: another test counts its runs.
    a.watch(|b: &Board| b.notes.values().map(String::len).max().unwrap_or(0), move |n| s.lock().unwrap().push(*n));
    a.add("four".into()).unwrap();
    h.add("five!".into()).unwrap();
    until("the copy follows", || a.state().notes.len() == 2).await;
    h.add("tiny".into()).unwrap();
    until("the copy follows", || a.state().notes.len() == 3).await;
    assert_eq!(*seen.lock().unwrap(), [0, 4, 5], "a note no longer than the longest changes nothing");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_copys_listeners_never_hear_an_older_state_after_a_newer_one() {
    use std::sync::{Arc, Mutex};
    let (_h, addr) = server().await;
    let a = Remote::<Board>::connect(&addr, "secret", "ana").await.unwrap();
    // The first listener changes the copy as it is told (a tell within a
    // tell); the second must still hear the states in the order they were.
    let once = Mutex::new(Some(a.clone()));
    a.listen(move |_| {
        let b = once.lock().unwrap().take();
        if let Some(b) = b {
            b.add("from a listener".into()).unwrap();
        }
    });
    let seen = Arc::new(Mutex::new(vec![]));
    let s = seen.clone();
    a.listen(move |b: &Arc<Board>| s.lock().unwrap().push(b.notes.len()));
    a.add("first".into()).unwrap();
    until("both shown", || seen.lock().unwrap().last() == Some(&2)).await;
    let seen = seen.lock().unwrap();
    assert!(seen.is_sorted(), "an older state told after a newer one: {seen:?}");
}

#[test]
fn a_cells_listeners_never_hear_an_older_state_after_a_newer_one() {
    use std::sync::{Arc, Mutex};
    let h = Handle::<Board>::memory();
    let once = Mutex::new(Some(h.clone()));
    h.listen(move |_| {
        let b = once.lock().unwrap().take();
        if let Some(b) = b {
            b.add("from a listener".into()).unwrap();
        }
    });
    let seen = Arc::new(Mutex::new(vec![]));
    let s = seen.clone();
    h.listen(move |b: &Arc<Board>| s.lock().unwrap().push(b.notes.len()));
    h.add("first".into()).unwrap();
    assert_eq!(*seen.lock().unwrap(), [1, 2], "told in order, each once");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_app_hears_client_links_open_and_close_but_not_tools() {
    use aio::http::{Http, Presence};
    use std::sync::{Arc, Mutex};
    let heard: Arc<Mutex<Vec<(bool, String)>>> = Arc::default();
    let log = heard.clone();
    let panics = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let boom = panics.clone();
    let http = Http::default().on_link(move |p: &Presence| {
        log.lock().unwrap().push((p.online, p.user.clone()));
        assert!(p.from.starts_with("127.0.0.1:"), "{}", p.from);
        // A hook that panics never ends the link.
        if boom.swap(false, std::sync::atomic::Ordering::SeqCst) {
            panic!("hook exploded");
        }
    });
    let h = Handle::<Board>::memory();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let opts = aio::sync::Options { token: "secret".into(), http, ..Default::default() };
    tokio::spawn(aio::sync::serve_with(h.clone(), l, opts));
    let ann = Remote::<Board>::connect(&addr, "secret", "ann").await.unwrap();
    until("ann online", || heard.lock().unwrap().as_slice() == [(true, "ann".to_string())]).await;
    ann.add("still linked".into()).unwrap();
    // A tool is no client link.
    let link = aio::sync::Link { addr: addr.clone(), token: "secret".into(), root: None };
    let mut tool = aio::sync::Live::connect(&link, "board", "ops").await.unwrap();
    assert!(tool.call("add", serde_json::json!(["by a tool"])).await.is_ok());
    drop(tool);
    drop(ann);
    until("ann offline", || heard.lock().unwrap().len() == 2).await;
    assert_eq!(heard.lock().unwrap().as_slice(), [(true, "ann".to_string()), (false, "ann".to_string())]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_link_sending_more_bytes_than_its_share_is_read_slower_never_dropped() {
    let limits = aio::sync::Limits { bytes_per_sec: 2000, calls_per_sec: 1000, ..Default::default() };
    let (h, addr) = limited(limits).await;
    let a = Remote::<Board>::connect(&addr, "secret", "ana").await.unwrap();
    let t = std::time::Instant::now();
    // ~6 KB against a 4 KB burst at 2 KB/s: about 1 s more.
    for i in 0..6 {
        a.add(format!("{i}{}", "x".repeat(1000))).unwrap();
    }
    until("all landed", || h.state().notes.len() == 6).await;
    let took = t.elapsed();
    assert!(took >= Duration::from_millis(700), "{took:?}");
}

/// A server cell that tells the people using the app (aio's `notify()`, `appIcon()`).
#[cell(shared)]
pub struct Exports {
    pub done: u32,
}

#[methods]
impl Exports {
    #[client]
    pub fn finish(&mut self, cx: &Cx, title: String) -> Result {
        self.done += 1;
        cx.tell(aio::tell::Tell::notify(&title, &format!("{} done", self.done)))?;
        cx.tell(aio::tell::Tell::Icon { window: None, tray: Some(vec![]), tooltip: None, fps: 1 })
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn what_a_server_call_tells_reaches_each_client_once() {
    let h = Handle::<Exports>::memory();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(serve(h.clone(), l, "secret".into()));
    let (ana, bo) = (
        Remote::<Exports>::connect(&addr, "secret", "ana").await.unwrap(),
        Remote::<Exports>::connect(&addr, "secret", "bo").await.unwrap(),
    );
    let said = |r: &Remote<Exports>| {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
        let s = seen.clone();
        r.on_tell(move |t| s.lock().unwrap().push(t.clone()));
        seen
    };
    let (to_ana, to_bo) = (said(&ana), said(&bo));
    h.finish("Export".into()).unwrap();
    // A client's own call: told once, by the server, never by its prediction.
    ana.finish("Again".into()).unwrap();
    until("both told twice", || to_ana.lock().unwrap().len() == 4 && to_bo.lock().unwrap().len() == 4).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let got = to_bo.lock().unwrap().clone();
    assert_eq!(*to_ana.lock().unwrap(), got);
    assert_eq!(got[0], aio::tell::Tell::notify("Export", "1 done"));
    assert_eq!(got[2], aio::tell::Tell::notify("Again", "2 done"));
    assert!(matches!(&got[1], aio::tell::Tell::Icon { tray: Some(f), .. } if f.is_empty()));
    // A blank card is refused where it is asked, and nothing commits.
    assert!(h.finish(" ".into()).unwrap_err().why.contains("title is empty"));
    assert_eq!(h.state().done, 2);
}

/// A raw aio-rs link to `addr`, past its first state.
async fn raw_link(addr: &str) -> (Sock, serde_json::Value) {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut req = format!("ws://{addr}/ws?token=secret").into_client_request().unwrap();
    req.headers_mut().insert("x-aio-wire", "2".parse().unwrap());
    let (mut w, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    send(&mut w, serde_json::json!({ "t": "hello", "cell": "board", "v": 1, "user": "ana" })).await;
    let full = recv(&mut w).await;
    assert_eq!(full["t"], "state");
    (w, full)
}

/// The views a link is sent within `wait`.
async fn views(w: &mut Sock, wait: Duration) -> Vec<serde_json::Value> {
    let mut got = Vec::new();
    while let Ok(Some(Ok(m))) = tokio::time::timeout(wait, w.next()).await {
        if let Message::Text(t) = m {
            let v: serde_json::Value = serde_json::from_str(&t).unwrap();
            if matches!(v["t"].as_str(), Some("state" | "patch")) {
                got.push(v);
            }
        }
    }
    got
}

#[tokio::test(flavor = "multi_thread")]
async fn background_changes_are_paced_and_a_persons_go_at_once() {
    let (h, addr) = limited(aio::sync::Limits { sync_interval_ms: 200, ..Default::default() }).await;
    let (mut w, _) = raw_link(&addr).await;
    // A schedule's burst: the first at once, the rest folded into one patch.
    let tick = h.by(By::Schedule("tick".into()));
    for i in 0..20 {
        tick.add(format!("t{i}")).unwrap();
    }
    let got = views(&mut w, Duration::from_millis(600)).await;
    assert!((1..=3).contains(&got.len()), "{} views for 20 changes", got.len());
    assert_eq!(got.last().unwrap()["seq"], h.snapshot().0);
    // A person's changes skip the window, each one.
    let ana = h.by(By::User("ana".into()));
    for i in 0..5 {
        ana.add(format!("a{i}")).unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(views(&mut w, Duration::from_millis(300)).await.len(), 5);
    // 0: every change at once.
    let (h, addr) = limited(aio::sync::Limits { sync_interval_ms: 0, ..Default::default() }).await;
    let (mut w, _) = raw_link(&addr).await;
    let tick = h.by(By::Schedule("tick".into()));
    for i in 0..10 {
        tick.add(format!("t{i}")).unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(views(&mut w, Duration::from_millis(300)).await.len(), 10);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_patch_bigger_than_its_share_of_the_view_goes_as_the_view() {
    for (percent, want) in [(1, "state"), (50, "patch")] {
        let (h, addr) = limited(aio::sync::Limits { full_state_percent: percent, ..Default::default() }).await;
        for i in 0..50 {
            h.add(format!("note {i} {}", "x".repeat(200))).unwrap();
        }
        let (mut w, _) = raw_link(&addr).await;
        h.by(By::User("bo".into())).add("y".repeat(300)).unwrap();
        let got = views(&mut w, Duration::from_millis(300)).await;
        assert_eq!(got.len(), 1);
        assert_eq!(got[0]["t"], want, "at {percent}%");
        assert_eq!(got[0]["seq"], h.snapshot().0);
    }
    // A copy follows either way.
    let (h, addr) = limited(aio::sync::Limits { full_state_percent: 0, ..Default::default() }).await;
    let a = Remote::<Board>::connect(&addr, "secret", "ana").await.unwrap();
    h.add("whole".into()).unwrap();
    until("the copy follows a whole view", || *a.state() == *h.state()).await;
}

#[cell(shared)]
pub struct Tally {
    pub n: u64,
}

#[methods]
impl Tally {
    #[client]
    pub fn bump(&mut self) -> Result<u64> {
        self.n += 1;
        Ok(self.n)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn every_shared_cell_on_one_port_and_socket_path() {
    let (board, tally) = (Handle::<Board>::memory(), Handle::<Tally>::memory());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let http = aio::http::Http::default().cell(tally.clone());
    let opts = aio::sync::Options { token: "secret".into(), http, ..Default::default() };
    tokio::spawn(aio::sync::serve_with(board.clone(), l, opts));
    let b = Remote::<Board>::connect(&addr, "secret", "ana").await.unwrap();
    let t = Remote::<Tally>::connect(&addr, "secret", "ana").await.unwrap();
    assert_eq!(t.bump().unwrap(), 1);
    b.add("x".into()).unwrap();
    until("both confirmed", || t.pending() == 0 && b.pending() == 0).await;
    assert_eq!((tally.state().n, board.state().notes.len()), (1, 1));
    tally.bump().unwrap();
    until("the second cell's copy follows", || t.state().n == 2).await;
    // The key holds for every cell; a cell nobody serves is refused.
    assert!(Remote::<Tally>::connect(&addr, "nope", "ana").await.is_err());
    let link = aio::sync::Link { addr: addr.clone(), token: "secret".into(), root: None };
    let e = aio::sync::Live::connect(&link, "ghost", "ana").await.err().expect("no such cell");
    assert!(e.to_string().contains("this server has cell board"), "{e}");
    let mut tool = aio::sync::Live::connect(&link, "tally", "ops").await.unwrap();
    assert_eq!(tool.view()["n"], 2);
    assert_eq!(tool.call("bump", serde_json::json!([])).await, Ok(serde_json::json!(3)));
}

#[test]
#[should_panic(expected = "isn't #[cell(shared)]")]
fn a_cell_that_isnt_shared_is_never_served_beside() {
    #[cell]
    pub struct Mine {
        pub n: u8,
    }
    #[methods]
    impl Mine {}
    let _ = aio::http::Http::default().cell(Handle::<Mine>::memory());
}

#[cell(shared)]
pub struct Memo {
    pub title: String,
    pub n: i64,
}

#[methods]
impl Memo {
    #[client]
    pub fn set_title(&mut self, t: String) -> Result {
        self.title = t;
        Ok(())
    }

    #[client]
    pub fn set_n(&mut self, n: i64) -> Result {
        self.n = n;
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_change_over_a_waiting_call_is_said_and_a_merged_field_shows_both() {
    use aio::merge::{Conflict, Merge};
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let fake = tokio::spawn(async move {
        let (s, _) = l.accept().await.unwrap();
        let mut w = tokio_tungstenite::accept_async(s).await.unwrap();
        let send = |v: serde_json::Value| Message::text(v.to_string());
        let patch = |prev: u64, path: &str, v: serde_json::Value, st: &Memo, op: Option<&serde_json::Value>| {
            let digest = aio::patch::digest(&serde_json::to_value(st).unwrap());
            let mut p = serde_json::json!({ "t": "patch", "prev": prev, "seq": prev + 1,
                "ops": [{ "op": "set", "path": [path], "value": v }], "digest": digest });
            if let Some(op) = op {
                p["op"] = op.clone();
            }
            send(p)
        };
        next_text(&mut w).await;
        let mut st = Memo { title: "a".into(), n: 1 };
        w.send(send(serde_json::json!({ "t": "state", "seq": 0, "state": st }))).await.unwrap();
        let call = next_text(&mut w).await;
        assert_eq!(call["method"], "set_title");
        // Someone else's title first: it meets the waiting call.
        st.title = "theirs".into();
        w.send(patch(0, "title", "theirs".into(), &st, None)).await.unwrap();
        // Then the call itself lands: its own change, never a conflict.
        st.title = "mine".into();
        w.send(patch(1, "title", "mine".into(), &st, Some(&call["op"]))).await.unwrap();
        w.send(send(serde_json::json!({ "t": "reply", "op": call["op"], "ok": null }))).await.unwrap();
        // A counter both changed: shown as both changes.
        let call = next_text(&mut w).await;
        assert_eq!(call["method"], "set_n");
        st.n = 3;
        w.send(patch(2, "n", 3.into(), &st, None)).await.unwrap();
        next_text(&mut w).await;
    });
    let r = Remote::<Memo>::connect(&addr, "t", "u").await.unwrap();
    let met: std::sync::Arc<std::sync::Mutex<Vec<Conflict>>> = Default::default();
    let m2 = met.clone();
    r.on_conflict(move |c| m2.lock().unwrap().extend_from_slice(c));
    r.set_title("mine".into()).unwrap();
    until("its call answered", || r.pending() == 0).await;
    assert_eq!(
        *met.lock().unwrap(),
        [Conflict { field: "title".into(), local: "mine".into(), remote: "theirs".into() }],
        "once, for the other's change"
    );
    assert_eq!(r.state().title, "mine");
    r.merge("n", Merge::Counter);
    r.set_n(5).unwrap();
    until("both changes shown", || r.state().n == 7).await;
    assert_eq!((r.confirmed().n, met.lock().unwrap().len()), (3, 1), "a counter never conflicts");
    r.set_title("bye".into()).unwrap();
    fake.await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_copys_own_calls_never_conflict_with_themselves() {
    let (h, addr) = server().await;
    let a = Remote::<Board>::connect(&addr, "secret", "ana").await.unwrap();
    let n = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let n2 = n.clone();
    a.on_conflict(move |c| _ = n2.fetch_add(c.len(), std::sync::atomic::Ordering::SeqCst));
    // `luck` is the server's random number, never the predicted one: its own change still.
    for i in 0..20 {
        a.add(format!("n{i}")).unwrap();
    }
    until("all confirmed", || a.pending() == 0 && a.state().notes.len() == 20).await;
    assert_eq!(*a.state(), *h.state());
    assert_eq!(n.load(std::sync::atomic::Ordering::SeqCst), 0);
}

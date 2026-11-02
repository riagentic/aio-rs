//! Every cell of an app on one socket (aio serves them all on one `/ws`):
//! the copies of several cells share one connection, each cell keeping its
//! own rules, and an app's whole set of cells runs on one `TestServer`.
use aio::auth::{Auth, Policy};
use aio::prelude::*;
use aio::sync::{Link, Live, Options, Remote, serve_with};
use aio::testing::TestServer;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::Message;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The next channel frame but the server's pings to its links, which are
/// answered as a client answers them.
async fn next(w: &mut Ws) -> String {
    loop {
        let m = tokio::time::timeout(Duration::from_secs(5), w.next()).await.unwrap().unwrap().unwrap();
        let Message::Text(t) = m else { continue };
        let t = t.to_string();
        let at = t.find(|c: char| !c.is_ascii_digit()).unwrap();
        if t[at..].starts_with('?') {
            w.send(Message::text(format!("{}.{}", &t[..at], &t[at + 1..]))).await.unwrap();
            continue;
        }
        return t;
    }
}

#[cell(shared)]
pub struct Alpha {
    pub n: u64,
}

#[methods]
impl Alpha {
    #[client]
    pub fn bump(&mut self) -> Result<u64> {
        self.n += 1;
        Ok(self.n)
    }
}

#[cell(shared)]
pub struct Beta {
    pub n: u64,
}

#[methods]
impl Beta {
    #[client]
    pub fn bump(&mut self) -> Result<u64> {
        self.n += 1;
        Ok(self.n)
    }
}

#[cell(shared)]
pub struct Gamma {
    pub n: u64,
}

#[methods]
impl Gamma {
    #[client]
    pub fn bump(&mut self) -> Result<u64> {
        self.n += 1;
        Ok(self.n)
    }

    /// The admin's only.
    #[client(role = "admin")]
    pub fn reset(&mut self) {
        self.n = 0;
    }

    /// Never a client's.
    pub fn wipe(&mut self) {
        self.n = 0;
    }
}

/// What a viewer is shown by their role.
#[cell(shared, visible = by_role)]
pub struct Payroll {
    pub names: Vec<String>,
    pub salaries: Vec<u32>,
}

fn by_role(p: &Payroll, who: &Caller) -> Payroll {
    match who.role.as_deref() {
        Some("admin") => p.clone(),
        _ => Payroll { salaries: vec![], ..p.clone() },
    }
}

#[methods]
impl Payroll {
    pub fn hire(&mut self, name: String, salary: u32) {
        self.names.push(name);
        self.salaries.push(salary);
    }
}

/// Alpha served with Beta and Gamma beside it, `limits` its, behind a relay
/// that counts connections: (handles, the relay's address, the count).
async fn app(limits: aio::sync::Limits) -> ((Handle<Alpha>, Handle<Beta>, Handle<Gamma>), String, Arc<AtomicUsize>) {
    let (a, b, c) = (Handle::<Alpha>::memory(), Handle::<Beta>::memory(), Handle::<Gamma>::memory());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let http = aio::http::Http::default().cell(b.clone()).cell(c.clone());
    let opts = Options { token: "secret".into(), http, limits, ..Default::default() };
    tokio::spawn(serve_with(a.clone(), l, opts));
    let (relay, conns) = counted(addr).await;
    ((a, b, c), relay, conns)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_clients_copies_of_every_cell_share_one_socket() {
    let ((a, b, c), addr, conns) = app(Default::default()).await;
    let ra = Remote::<Alpha>::connect(&addr, "secret", "ana").await.unwrap();
    let rb = Remote::<Beta>::connect(&addr, "secret", "ana").await.unwrap();
    let rc = Remote::<Gamma>::connect(&addr, "secret", "bo").await.unwrap();
    assert_eq!((ra.bump().unwrap(), rb.bump().unwrap(), rc.bump().unwrap()), (1, 1, 1), "predicted at once");
    until("each confirmed", || ra.pending() + rb.pending() + rc.pending() == 0).await;
    assert_eq!((a.state().n, b.state().n, c.state().n), (1, 1, 1));
    b.bump().unwrap();
    c.bump().unwrap();
    until("each copy follows its cell", || rb.state().n == 2 && rc.state().n == 2 && ra.state().n == 1).await;
    // Each cell's own rules on the shared socket.
    let said = refusals(&rc);
    rc.reset().unwrap();
    rc.wipe().unwrap();
    until("both refused", || said.lock().unwrap().len() == 2).await;
    let said = said.lock().unwrap().clone();
    assert!(said[0].starts_with("reset: ") && said[0].contains("access denied"), "{said:?}");
    assert!(said[1].starts_with("wipe: "), "{said:?}");
    until("taken back", || rc.state().n == 2).await;
    // A tool and a cell nobody serves: on the same socket, the others unharmed.
    let link = Link { addr: addr.clone(), token: "secret".into(), root: None };
    let e = Live::connect(&link, "ghost", "ana").await.err().expect("no such cell");
    assert!(e.to_string().contains("this server has cell alpha"), "{e}");
    let mut tool = Live::connect(&link, "beta", "ops").await.unwrap();
    assert_eq!(tool.call("bump", json!([])).await, Ok(json!(3)));
    drop(tool);
    // A copy gone: its link closes, the others keep the socket.
    drop(rb);
    a.bump().unwrap();
    until("alpha's copy follows", || ra.state().n == 2).await;
    assert_eq!(rc.bump().unwrap(), 3);
    until("gamma confirmed", || rc.pending() == 0).await;
    assert_eq!(conns.load(Ordering::SeqCst), 1, "one socket for every link");
    // The key holds for the socket.
    assert!(Remote::<Beta>::connect(&addr, "nope", "ana").await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn every_link_on_the_socket_is_a_client_of_the_limit() {
    let limits = aio::sync::Limits { clients: 2, ..Default::default() };
    let (_, addr, conns) = app(limits).await;
    let ra = Remote::<Alpha>::connect(&addr, "secret", "ana").await.unwrap();
    let rb = Remote::<Beta>::connect(&addr, "secret", "ana").await.unwrap();
    let e = Remote::<Gamma>::connect(&addr, "secret", "ana").await.err().expect("over the limit");
    assert!(e.to_string().contains("too many clients (the limit is 2)"), "{e}");
    drop(rb);
    let mut got = None;
    for _ in 0..100 {
        if let Ok(r) = Remote::<Gamma>::connect(&addr, "secret", "ana").await {
            got = Some(r);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let rc = got.expect("a slot freed by the copy gone");
    assert_eq!((ra.bump().unwrap(), rc.bump().unwrap()), (1, 1));
    until("confirmed", || ra.pending() + rc.pending() == 0).await;
    assert_eq!(conns.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn each_cell_shows_and_lets_a_user_what_their_role_says_on_one_socket() {
    const PW: &str = "correct horse battery";
    let (pay, gamma) = (Handle::<Payroll>::memory(), Handle::<Gamma>::memory());
    pay.hire("ann".into(), 100).unwrap();
    let auth = Arc::new(Auth::memory(Policy::default()).unwrap());
    auth.create("bob", PW, None).unwrap();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let http = aio::http::Http::default().cell(gamma.clone());
    let opts = Options { token: "secret".into(), auth: Some(auth.clone()), http, ..Default::default() };
    tokio::spawn(serve_with(pay.clone(), l, opts));
    let (relay, conns) = counted(addr).await;
    let aio::auth::Login::Signed { token, .. } = auth.login("bob", PW).unwrap() else { panic!("no session") };
    let link = Link { addr: relay, token, root: None };
    let p = Remote::<Payroll>::connect_to(link.clone(), "bob").await.unwrap();
    let g = Remote::<Gamma>::connect_to(link, "bob").await.unwrap();
    assert_eq!((p.confirmed().names.len(), p.confirmed().salaries.len()), (1, 0), "names only");
    let said = refusals(&g);
    g.reset().unwrap();
    until("refused", || said.lock().unwrap().len() == 1).await;
    assert!(said.lock().unwrap()[0].contains("access denied"), "{:?}", said.lock().unwrap());
    // A new role holds for every cell of the socket, from the next change on.
    assert!(auth.set_role("bob", "admin").unwrap());
    pay.hire("bob".into(), 90).unwrap();
    until("bob sees salaries", || p.confirmed().salaries == [100, 90]).await;
    g.bump().unwrap();
    g.reset().unwrap();
    until("gamma confirmed", || g.pending() == 0).await;
    assert_eq!(gamma.state().n, 0, "the admin's reset ran");
    assert_eq!(conns.load(Ordering::SeqCst), 1);
}

/// An older aio-rs server: it doesn't say yes to one socket, so each copy
/// gets a socket of its own, as before.
#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_doesnt_say_yes_gets_a_socket_per_cell() {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let accepted = Arc::new(AtomicUsize::new(0));
    let count = accepted.clone();
    tokio::spawn(async move {
        while let Ok((s, _)) = l.accept().await {
            count.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut w = tokio_tungstenite::accept_async(s).await.unwrap();
                let Some(Ok(Message::Text(t))) = w.next().await else { return };
                let hello: Value = serde_json::from_str(&t).unwrap();
                assert!(hello["t"] == "hello", "a plain frame, no channel: {t}");
                let st = json!({ "t": "state", "seq": 0, "state": { "n": 7 } });
                w.send(Message::text(st.to_string())).await.unwrap();
                while let Some(Ok(_)) = w.next().await {}
            });
        }
    });
    let ra = Remote::<Alpha>::connect(&addr, "t", "u").await.unwrap();
    let rb = Remote::<Beta>::connect(&addr, "t", "u").await.unwrap();
    assert_eq!((ra.state().n, rb.state().n), (7, 7));
    assert_eq!(accepted.load(Ordering::SeqCst), 2);
}

/// The channels by hand, as a client of another language would speak them.
#[tokio::test(flavor = "multi_thread")]
async fn the_channels_on_the_wire() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let (_, addr, _) = app(Default::default()).await;
    let mut req = format!("ws://{addr}/ws").into_client_request().unwrap();
    for (k, v) in [("authorization", "Bearer secret"), ("x-aio-wire", "2"), ("x-aio-mux", "1")] {
        req.headers_mut().insert(k, v.parse().unwrap());
    }
    let (mut w, r) = tokio_tungstenite::connect_async(req).await.unwrap();
    assert_eq!(r.headers().get("x-aio-mux").unwrap(), "1", "the server says yes");
    let text = |s: String| Message::text(s);
    let hello = |cell: &str| json!({ "t": "hello", "cell": cell, "v": 1, "user": "ana" }).to_string();
    w.send(text(format!("1:{}", hello("beta")))).await.unwrap();
    w.send(text(format!("3:{}", hello("gamma")))).await.unwrap();
    let mut firsts: Vec<(String, Value)> = vec![];
    for _ in 0..2 {
        let t = next(&mut w).await;
        let (id, m) = t.split_once(':').unwrap();
        firsts.push((id.into(), serde_json::from_str(m).unwrap()));
    }
    firsts.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!((firsts[0].0.as_str(), &firsts[0].1["t"]), ("1", &json!("state")));
    assert_eq!((firsts[1].0.as_str(), &firsts[1].1["state"]), ("3", &json!({ "n": 0 })));
    // A client's ping is answered on its channel.
    w.send(text("3?00ff".into())).await.unwrap();
    assert_eq!(next(&mut w).await, "3.00ff");
    // A call on one channel, answered there.
    let op = aio::uuid_v7().to_string();
    let call = json!({ "t": "call", "op": op, "method": "bump", "args": [] });
    w.send(text(format!("3:{call}"))).await.unwrap();
    loop {
        let t = next(&mut w).await;
        let m: Value = serde_json::from_str(t.strip_prefix("3:").expect("on its channel")).unwrap();
        if m["t"] == "reply" {
            assert_eq!(m["ok"], 1);
            break;
        }
    }
    // A channel number not above the last is a link gone: ignored. Closed: its end said.
    w.send(text(format!("2:{}", hello("alpha")))).await.unwrap();
    w.send(text("1!1000 ".into())).await.unwrap();
    assert_eq!(next(&mut w).await, "1!1000 ");
    // A cell nobody serves: refused on its channel, then closed.
    w.send(text(format!("4:{}", hello("ghost")))).await.unwrap();
    assert!(next(&mut w).await.starts_with("4:{\"t\":\"refused\""));
    assert!(next(&mut w).await.starts_with("4!1011 "));
    // Not a channel's frame: the socket is closed.
    w.send(text("hello".into())).await.unwrap();
    let end = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(Ok(m)) = w.next().await {
            if matches!(m, Message::Close(_)) {
                break;
            }
        }
    });
    assert!(end.await.is_ok(), "closed");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_whole_app_on_one_test_server() {
    let (b, c) = (Handle::<Beta>::memory(), Handle::<Gamma>::memory());
    let http = aio::http::Http::default().cell(b).cell(c);
    let s = TestServer::start(Handle::<Alpha>::memory(), http).await.unwrap();
    let (ra, rb, rc) = (
        s.client("ana").await.unwrap(),
        s.client_of::<Beta>("ana").await.unwrap(),
        s.client_of::<Gamma>("bo").await.unwrap(),
    );
    ra.bump().unwrap();
    rb.bump().unwrap();
    rb.bump().unwrap();
    rc.bump().unwrap();
    s.converged(&[&ra]).await;
    s.converged(&[&rb]).await;
    s.converged(&[&rc]).await;
    assert_eq!((s.state().n, s.cell::<Beta>().state().n, s.cell::<Gamma>().state().n), (1, 2, 1));
    s.cell::<Gamma>().bump().unwrap();
    s.converged(&[&rc]).await;
    assert_eq!(rc.state().n, 2);
}

#[tokio::test(flavor = "multi_thread")]
#[should_panic(expected = "TestServer: Payroll isn't served here (it serves alpha, beta)")]
async fn a_cell_a_test_server_doesnt_serve_is_named() {
    let http = aio::http::Http::default().cell(Handle::<Beta>::memory());
    let s = TestServer::start(Handle::<Alpha>::memory(), http).await.unwrap();
    s.cell::<Payroll>();
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

/// What the server refused of `r`'s calls, from now on.
fn refusals<U: aio::Cell>(r: &Remote<U>) -> Arc<std::sync::Mutex<Vec<String>>> {
    let said = Arc::new(std::sync::Mutex::new(vec![]));
    let to = said.clone();
    r.on_refused(move |m, e| to.lock().unwrap().push(format!("{m}: {}", e.why)));
    said
}

/// A relay to `to` that counts the TCP connections made through it.
async fn counted(to: String) -> (String, Arc<AtomicUsize>) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (addr, n) = (l.local_addr().unwrap().to_string(), Arc::new(AtomicUsize::new(0)));
    let count = n.clone();
    tokio::spawn(async move {
        while let Ok((mut a, _)) = l.accept().await {
            count.fetch_add(1, Ordering::SeqCst);
            let to = to.clone();
            tokio::spawn(async move {
                if let Ok(mut b) = tokio::net::TcpStream::connect(&to).await {
                    _ = tokio::io::copy_bidirectional(&mut a, &mut b).await;
                }
            });
        }
    });
    (addr, n)
}

/// What a client of three cells costs: its connections, and how long a
/// server change takes to show in its copy (p50, p99 of 300). Run with
/// `--ignored --nocapture`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement"]
async fn measure_three_cells_connections_and_patch_latency() {
    for pace in [50, 0] {
        measure(pace).await;
    }
}

async fn measure(sync_interval_ms: u64) {
    let (a, b, c) = (Handle::<Alpha>::memory(), Handle::<Beta>::memory(), Handle::<Gamma>::memory());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let http = aio::http::Http::default().cell(b.clone()).cell(c.clone());
    let limits = aio::sync::Limits { sync_interval_ms, ..Default::default() };
    let opts = aio::sync::Options { token: "secret".into(), http, limits, ..Default::default() };
    tokio::spawn(aio::sync::serve_with(a.clone(), l, opts));
    let (relay, conns) = counted(addr).await;
    let ra = Remote::<Alpha>::connect(&relay, "secret", "ana").await.unwrap();
    let rb = Remote::<Beta>::connect(&relay, "secret", "ana").await.unwrap();
    let rc = Remote::<Gamma>::connect(&relay, "secret", "ana").await.unwrap();
    let mut took = Vec::new();
    for i in 1..=300u64 {
        let t = Instant::now();
        match i % 3 {
            0 => _ = a.bump().unwrap(),
            1 => _ = b.bump().unwrap(),
            _ => _ = c.bump().unwrap(),
        }
        let want = i.div_ceil(3);
        loop {
            let got = match i % 3 {
                0 => ra.state().n,
                1 => rb.state().n,
                _ => rc.state().n,
            };
            if got == want {
                break;
            }
            tokio::task::yield_now().await;
        }
        took.push(t.elapsed());
    }
    took.sort();
    let us = |d: Duration| d.as_micros();
    println!(
        "three cells, sync_interval_ms {sync_interval_ms}: {} connections; patch latency p50 {} us, p99 {} us",
        conns.load(Ordering::SeqCst),
        us(took[took.len() / 2]),
        us(took[took.len() * 99 / 100])
    );
}

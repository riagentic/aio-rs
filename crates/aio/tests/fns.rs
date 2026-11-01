use aio::auth::{Auth, Policy};
use aio::fns::{Access, Req, ServerFn};
use aio::http::Http;
use aio::prelude::*;
use aio::sync::{Options, Remote, serve_with};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

#[cell(shared)]
pub struct Board {
    pub n: u32,
}

#[methods]
impl Board {
    pub fn bump(&mut self) -> Result {
        self.n += 1;
        Ok(())
    }
}

const ADD: ServerFn<(i64, i64), i64> = ServerFn::new("math.add");
const WHO: ServerFn<(), (Option<String>, Option<String>)> = ServerFn::new("math.who");
const ADMIN: ServerFn<(), String> = ServerFn::new("math.admin");
const DECLINE: ServerFn<(u64,), ()> = ServerFn::new("pay.charge");
const HANG: ServerFn<(), ()> = ServerFn::new("math.hang");
const MISSING: ServerFn<(), ()> = ServerFn::new("math.missing");
const SAY: ServerFn<(u32, u64), String> = ServerFn::new("llm.say");
/// A picker the person takes 400 ms over, then an answer.
const PICK: ServerFn<(), String> = ServerFn::new("files.pick");
/// A picker closed at once, then a hang.
const PICKED: ServerFn<(), ()> = ServerFn::new("files.picked");
/// Where the caller is: its address, its user agent, and whether it was handed a credential.
const FROM: ServerFn<(), (Option<String>, Option<String>, bool)> = ServerFn::new("math.from");
const PW: &str = "correct horse battery";

/// Set when it is dropped: a cancelled call drops its future.
struct Flag(Arc<AtomicBool>);

impl Drop for Flag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

fn fns(cancelled: Arc<AtomicBool>) -> Http {
    Http::default()
        .server_fn(ADD, Access::Open, |_, (a, b)| async move { Ok(a + b) })
        .server_fn(WHO, Access::Open, |by: Req, ()| async move { Ok((by.who.user().map(String::from), by.who.role)) })
        .server_fn(FROM, Access::Open, |by: Req, ()| async move {
            let ua = by.headers.get("user-agent").and_then(|v| v.to_str().ok()).map(String::from);
            let held = by.headers.contains_key("authorization") || by.headers.contains_key("cookie");
            Ok((by.ip.map(|ip| ip.to_string()), ua, held))
        })
        .server_fn(ADMIN, Access::Role("admin"), |by: Req, ()| async move {
            Ok(by.who.user().unwrap_or_default().to_string())
        })
        .server_fn(DECLINE, Access::Signed, |_, (cents,)| async move {
            Err::<(), _>(Rejected::new(format!("card declined for {cents}")))
        })
        // Parts every `gap` ms; the answer says which parts were taken.
        .server_fn_streamed(SAY, Access::Open, |_, (n, gap), parts: aio::fns::Chunks| async move {
            let mut took = String::new();
            for i in 0..n {
                tokio::time::sleep(Duration::from_millis(gap)).await;
                took.push(if parts.send(&format!("t{i}")).await { 'y' } else { 'n' });
            }
            Ok(took)
        })
        .server_fn(PICK, Access::Open, |_, ()| async {
            let _deciding = aio::deciding::start();
            tokio::time::sleep(Duration::from_millis(400)).await;
            Ok("/home/me/video.mp4".to_string())
        })
        .server_fn(PICKED, Access::Open, |_, ()| async {
            drop(aio::deciding::start());
            std::future::pending::<Result>().await
        })
        .server_fn(HANG, Access::Open, move |_, ()| {
            let flag = Flag(cancelled.clone());
            async move {
                let _flag = flag;
                std::future::pending::<Result>().await
            }
        })
}

async fn server(auth: Option<Arc<Auth>>) -> (String, Arc<AtomicBool>) {
    let cancelled = Arc::new(AtomicBool::new(false));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let opts = Options { token: "secret".into(), http: fns(cancelled.clone()), auth, ..Default::default() };
    tokio::spawn(serve_with(Handle::<Board>::memory(), l, opts));
    (addr, cancelled)
}

/// A TCP relay; `cut` ends it: its links drop and it takes no new ones.
async fn relay(to: String) -> (String, tokio::task::AbortHandle) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let cut = tokio::spawn(async move {
        let mut links = tokio::task::JoinSet::new();
        while let Ok((mut a, _)) = l.accept().await {
            let to = to.clone();
            links.spawn(async move {
                let mut b = tokio::net::TcpStream::connect(to).await.unwrap();
                _ = tokio::io::copy_bidirectional(&mut a, &mut b).await;
            });
        }
    });
    (addr, cut.abort_handle())
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

#[tokio::test(flavor = "multi_thread")]
async fn a_client_runs_typed_server_fns() {
    let (addr, _) = server(None).await;
    let r = Remote::<Board>::connect(&addr, "secret", "ana").await.unwrap();
    assert_eq!(r.run(ADD, (2, 40)).await, Ok(42));
    // Calls run side by side: a hung one holds up no other.
    let hung = tokio::spawn({
        let r = r.clone();
        async move { r.run(HANG, ()).await }
    });
    assert_eq!(r.run(ADD, (1, 1)).await, Ok(2));
    hung.abort();
    // The shared key proves no one: the hello's user is not the caller.
    assert_eq!(r.run(WHO, ()).await, Ok((None, None)));
    assert!(r.run(ADMIN, ()).await.unwrap_err().why.contains("access denied"));
    let declined = r.run(DECLINE, (5,)).await.unwrap_err();
    assert!(declined.why.contains("access denied"));
    assert_eq!(declined.code(), Some(aio::Code::AccessDenied), "the code crosses the wire");
    assert!(r.run(MISSING, ()).await.unwrap_err().why.contains("not served"));
    let wrong: ServerFn<(String,), i64> = ServerFn::new("math.add");
    assert!(r.run(wrong, ("2".into(),)).await.unwrap_err().why.contains("bad arguments"));
    let short = HANG.timeout(Duration::from_millis(200));
    let e = r.run(short, ()).await.unwrap_err().why;
    assert!(e.contains("timed out after 200ms") && e.contains("may still be running"), "{e}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_streamed_fn_sends_its_caller_parts_in_order_then_the_answer() {
    let (addr, _) = server(None).await;
    let r = Remote::<Board>::connect(&addr, "secret", "ana").await.unwrap();
    let mut got = vec![];
    assert_eq!(r.run_streamed(SAY, (3, 0), |p: String| got.push(p)).await, Ok("yyy".into()));
    assert_eq!(got, ["t0", "t1", "t2"]);
    // Two streamed calls on one link never see each other's parts.
    let (mut a, mut b) = (vec![], vec![]);
    let (ra, rb) = tokio::join!(
        r.run_streamed(SAY, (4, 5), |p: String| a.push(p)),
        r.run_streamed(SAY, (2, 7), |p: String| b.push(p))
    );
    assert_eq!((ra, rb), (Ok("yyyy".into()), Ok("yy".into())));
    assert!(a == ["t0", "t1", "t2", "t3"] && b == ["t0", "t1"], "{a:?} {b:?}");
    // A plain run gets the answer only; the function hears nobody asked.
    assert_eq!(r.run(SAY, (2, 0)).await, Ok("nn".into()));
    // No parts is fine; a part of the wrong type fails the call.
    assert_eq!(r.run_streamed(SAY, (0, 0), |_: String| panic!("no parts")).await, Ok(String::new()));
    let e = r.run_streamed(SAY, (1, 0), |_: u8| {}).await.unwrap_err().why;
    assert!(e.contains("a part doesn't fit its type"), "{e}");
    // The timeout counts from the last part: 5 parts 120 ms apart outlast a 300 ms one.
    let mut n = 0;
    assert_eq!(
        r.run_streamed(SAY.timeout(Duration::from_millis(300)), (5, 120), |_: String| n += 1).await,
        Ok("yyyyy".into())
    );
    assert_eq!(n, 5);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_person_at_a_picker_holds_the_callers_clock() {
    let (addr, _) = server(None).await;
    let r = Remote::<Board>::connect(&addr, "secret", "ana").await.unwrap();
    let short = Duration::from_millis(200);
    // Twice the window at the picker, and the pick still arrives.
    assert_eq!(r.run(PICK.timeout(short), ()).await.as_deref(), Ok("/home/me/video.mp4"));
    // A call that hangs after the dialog closed gets a full window, no more.
    let t = std::time::Instant::now();
    let e = r.run(PICKED.timeout(short), ()).await.unwrap_err().why;
    assert!(e.contains("timed out after 200ms") && t.elapsed() < Duration::from_secs(10), "{e}");
}

#[tokio::test(flavor = "multi_thread")]
async fn offline_fails_at_once_and_a_dropped_link_cancels_its_calls() {
    let (addr, cancelled) = server(None).await;
    let (via, cut) = relay(addr).await;
    let r = Remote::<Board>::connect(&via, "secret", "ana").await.unwrap();
    let hung = tokio::spawn({
        let r = r.clone();
        async move { r.run(HANG, ()).await }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let t = std::time::Instant::now();
    cut.abort();
    let e = hung.await.unwrap().unwrap_err().why;
    // Bounds well short of the 15 s call timeout: at once, not at its end.
    assert!(e.contains("dropped before it answered") && t.elapsed() < Duration::from_secs(10), "{e}");
    until("the server cancelled the call", || cancelled.load(Ordering::SeqCst)).await;
    assert!(!r.online());
    let t = std::time::Instant::now();
    assert!(r.run(ADD, (1, 2)).await.unwrap_err().why.contains("offline"));
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
}

#[tokio::test(flavor = "multi_thread")]
async fn with_accounts_the_role_is_checked_at_every_call() {
    let auth = Arc::new(Auth::memory(Policy::default()).unwrap());
    auth.create("ann", PW, Some("admin")).unwrap();
    auth.create("bob", PW, None).unwrap();
    let (addr, _) = server(Some(auth.clone())).await;
    let token = |id: &str| auth.issue(id, "user").unwrap();
    let ann = Remote::<Board>::connect(&addr, &token("ann"), "ann").await.unwrap();
    let bob = Remote::<Board>::connect(&addr, &token("bob"), "bob").await.unwrap();
    assert_eq!(ann.run(WHO, ()).await, Ok((Some("ann".into()), Some("admin".into()))));
    assert_eq!(ann.run(ADMIN, ()).await, Ok("ann".into()));
    assert!(bob.run(ADMIN, ()).await.unwrap_err().why.contains("access denied"));
    // Signed in, the function itself answers.
    let declined = bob.run(DECLINE, (5,)).await.unwrap_err();
    assert_eq!((declined.why.as_str(), declined.code()), ("card declined for 5", None), "the function's own");
    // A new role counts from the next call.
    auth.set_role("bob", "admin").unwrap();
    assert_eq!(bob.run(ADMIN, ()).await, Ok("bob".into()));
    auth.set_role("ann", "user").unwrap();
    assert!(ann.run(ADMIN, ()).await.unwrap_err().why.contains("access denied"));
}

#[tokio::test(flavor = "multi_thread")]
async fn users_known_by_token_are_who_they_say_everywhere_and_the_key_opens_nothing() {
    use aio::http::{Reply, Route};
    const ANN: &str = "ann-0123456789abcdef0123456789abcdef";
    const ROOT: &str = "root-0123456789abcdef0123456789abcdef";
    let users = aio::auth::Users::map([
        (ANN.to_string(), ("ann".to_string(), "user".to_string())),
        (ROOT.to_string(), ("root".to_string(), "admin".to_string())),
    ]);
    let http = fns(Arc::new(AtomicBool::new(false))).users(users).route(Route::get("/me", |r| async move {
        Reply::text(format!("{}/{}", r.user().unwrap_or("-"), r.role().unwrap_or("-")))
    }));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(serve_with(
        Handle::<Board>::memory(),
        l,
        Options { token: "secret".into(), http, ..Default::default() },
    ));
    // Links: the token's user and role, on every server fn call.
    let ann = Remote::<Board>::connect(&addr, ANN, "ann").await.unwrap();
    let root = Remote::<Board>::connect(&addr, ROOT, "root").await.unwrap();
    assert_eq!(ann.run(WHO, ()).await, Ok((Some("ann".into()), Some("user".into()))));
    assert!(ann.run(ADMIN, ()).await.unwrap_err().why.contains("access denied"));
    assert_eq!(root.run(ADMIN, ()).await, Ok("root".into()));
    // A token is one user: it can't speak as another; the key and a wrong token get nowhere.
    assert!(Remote::<Board>::connect(&addr, ANN, "root").await.is_err());
    assert!(Remote::<Board>::connect(&addr, "secret", "ann").await.is_err());
    assert!(Remote::<Board>::connect(&addr, "nobody-0123456789abcdef", "ann").await.is_err());
    // Routes see the same user.
    let get = |token: &'static str| {
        let addr = addr.clone();
        async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut s = tokio::net::TcpStream::connect(&addr).await.unwrap();
            let req = format!(
                "GET /me HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
            );
            s.write_all(req.as_bytes()).await.unwrap();
            let mut out = String::new();
            s.read_to_string(&mut out).await.unwrap();
            (out[9..12].to_string(), out.split_once("\r\n\r\n").unwrap().1.to_string())
        }
    };
    assert_eq!(get(ROOT).await, ("200".into(), "root/admin".into()));
    assert_eq!(get("secret").await.0, "401", "the key opens nothing");
    // A resolver that panics resolves no one; an empty token is no one.
    let hook =
        aio::auth::Users::resolve(|t| if t == "boom" { panic!("hook bug") } else { Some((t.into(), "user".into())) });
    assert_eq!((hook.who("boom"), hook.who("")), (None, None));
    assert_eq!(hook.who("eve").map(|s| (s.user, s.role)), Some(("eve".into(), "user".into())));
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The next `t` frame's `d`, skipping others.
async fn next(w: &mut Ws, t: &str) -> Value {
    loop {
        let m = tokio::time::timeout(Duration::from_secs(5), w.next()).await.unwrap().unwrap().unwrap();
        if let Message::Text(s) = m {
            let v: Value = serde_json::from_str(&s).unwrap();
            if v["t"] == t {
                return v["d"].clone();
            }
        }
    }
}

#[cfg(feature = "compat")]
#[tokio::test(flavor = "multi_thread")]
async fn a_server_fn_sees_where_its_caller_is_but_never_its_credentials() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let (addr, _) = server(None).await;
    let mut req = format!("ws://{addr}/ws").into_client_request().unwrap();
    for (k, v) in [("authorization", "Bearer secret"), ("cookie", "a=b"), ("user-agent", "pager/1.0")] {
        req.headers_mut().insert(k, v.parse().unwrap());
    }
    let (mut w, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    let f = json!({ "v": 2, "t": "sfn", "d": { "cid": "1", "ns": "math", "name": "from", "args": [] } });
    w.send(Message::text(f.to_string())).await.unwrap();
    assert_eq!(next(&mut w, "sfnr").await["value"], json!(["127.0.0.1", "pager/1.0", false]));
    // An aio-rs client too.
    let r = Remote::<Board>::connect(&addr, "secret", "ana").await.unwrap();
    assert_eq!(r.run(FROM, ()).await.map(|(ip, _, held)| (ip, held)), Ok((Some("127.0.0.1".into()), false)));
}

/// An aio-rs app as a client of an aio app runs its server fns too.
#[cfg(feature = "compat")]
#[tokio::test(flavor = "multi_thread")]
async fn an_aio_rs_app_runs_an_aio_apps_server_fns() {
    let (addr, _) = server(None).await;
    let mut deno =
        aio::compat::AioLink::connect(&aio::sync::Link { addr, token: "secret".into(), root: None }).await.unwrap();
    assert_eq!(deno.run("math", "add", json!([20, 22])).await, Ok(json!(42)));
    let denied = deno.run("math", "admin", json!([])).await.unwrap_err();
    assert_eq!(denied.code(), Some(aio::Code::AccessDenied), "{denied}");
    assert!(deno.run("math", "constructor", json!([])).await.unwrap_err().why.contains("not served"));
}

#[cfg(feature = "compat")]
#[tokio::test(flavor = "multi_thread")]
async fn an_aio_page_calls_them_with_sfn_frames() {
    let (addr, _) = server(None).await;
    let (mut w, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws?token=secret")).await.unwrap();
    let w = &mut w;
    let sfn = |cid: &str, name: &str, args: Value| {
        let f = json!({ "v": 2, "t": "sfn", "d": { "cid": cid, "ns": "math", "name": name, "args": args } });
        Message::text(f.to_string())
    };
    w.send(sfn("1", "add", json!([20, 22]))).await.unwrap();
    assert_eq!(next(w, "sfnr").await, json!({ "cid": "1", "ok": true, "value": 42 }));
    w.send(sfn("2", "admin", json!([]))).await.unwrap();
    let denied = next(w, "sfnr").await;
    assert_eq!((&denied["ok"], &denied["code"]), (&json!(false), &json!("ACCESS_DENIED")));
    w.send(sfn("3", "constructor", json!([]))).await.unwrap();
    let unknown = next(w, "sfnr").await;
    assert!(unknown["error"].as_str().unwrap().contains("not served"), "{unknown}");
    // A malformed frame is dropped; the link goes on.
    w.send(sfn("4", "add", json!(1))).await.unwrap();
    w.send(sfn("5", "add", json!([1, 2]))).await.unwrap();
    assert_eq!(next(w, "sfnr").await, json!({ "cid": "5", "ok": true, "value": 3 }));
    // A picker inside: the page is told to hold its clock, then to start it.
    w.send(Message::text(
        json!({ "v": 2, "t": "sfn", "d": { "cid": "6", "ns": "files", "name": "pick", "args": [] } }).to_string(),
    ))
    .await
    .unwrap();
    // (A picker of a test beside this one holds it too: every call in flight is.)
    let mut holds = vec![];
    let answer = loop {
        let m = tokio::time::timeout(Duration::from_secs(5), w.next()).await.unwrap().unwrap().unwrap();
        let v: Value = serde_json::from_str(m.to_text().unwrap()).unwrap();
        match v["t"].as_str() {
            Some("sfnh") if v["d"]["cid"] == "6" => holds.push(v["d"]["hold"].as_bool().unwrap()),
            Some("sfnr") => break v["d"].clone(),
            _ => {}
        }
    };
    // The clock starts once no call holds it: a neighbour's picker can keep
    // it held a little past this answer.
    while holds.last() != Some(&false) {
        let m = tokio::time::timeout(Duration::from_secs(5), w.next()).await.unwrap().unwrap().unwrap();
        let v: Value = serde_json::from_str(m.to_text().unwrap()).unwrap();
        if v["t"] == "sfnh" && v["d"]["cid"] == "6" {
            holds.push(v["d"]["hold"].as_bool().unwrap());
        }
    }
    assert_eq!(holds.first(), Some(&true), "{holds:?}");
    assert_eq!(answer, json!({ "cid": "6", "ok": true, "value": "/home/me/video.mp4" }));
}

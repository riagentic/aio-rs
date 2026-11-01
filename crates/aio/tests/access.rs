//! Who may call a cell's methods from the network (aio's `access`): only
//! `#[client]` methods, each as its `role` / `allow` rule says. Code in the
//! app and the local door's tools never ask.
use aio::auth::{Auth, Policy};
use aio::prelude::*;
use aio::sync::{Link, Live, Options, Remote, serve, serve_with};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

#[cell(persist, shared)]
pub struct Vault {
    pub writes: u32,
    /// Doc → its owner.
    pub owners: BTreeMap<String, String>,
    pub edited: Vec<String>,
}

/// Row-level: only a doc's owner may edit it, as the state says now.
fn own_doc(v: &Vault, who: &Caller, doc: &str) -> bool {
    matches!(&who.by, By::User(u) if v.owners.get(doc) == Some(u))
}

/// A rule that can't answer for big numbers.
fn small(_: &Vault, _: &Caller, n: u32) -> bool {
    assert!(n < 10, "too big to judge");
    true
}

#[methods]
impl Vault {
    #[client]
    pub fn note(&mut self) {
        self.writes += 1;
    }

    #[client(role = "admin")]
    pub fn bump(&mut self) {
        self.writes += 1;
    }

    #[client(allow = own_doc)]
    pub fn edit(&mut self, doc: &str) {
        self.edited.push(doc.into());
    }

    #[client(role = "admin", allow = small)]
    pub fn add(&mut self, n: u32) {
        self.writes += n;
    }

    /// Not `#[client]`: the server's own.
    pub fn give(&mut self, doc: String, to: String) {
        self.owners.insert(doc, to);
    }

    #[client]
    pub async fn fetch(&mut self, n: u32) -> Result {
        let got: u32 = async move { n }.await;
        self.writes += got;
        Ok(())
    }
}

fn who(user: &str, role: Option<&str>) -> Caller {
    Caller { by: By::User(user.into()), role: role.map(Into::into) }
}

fn denied(r: Result, why: &str) {
    match r {
        Err(e) => assert!(e.code() == Some(aio::Code::AccessDenied) && e.why.contains(why), "{e}"),
        Ok(()) => panic!("allowed, expected: {why}"),
    }
}

#[test]
fn the_rules() {
    let mut v = Vault::default();
    v.owners.insert("d1".into(), "ana".into());
    let (ana, admin, none) = (who("ana", Some("user")), who("root", Some("admin")), who("eve", None));
    // Plain #[client]: anyone who got in.
    assert_eq!(v.allow(&none, "note", &Value::Null), Ok(()));
    // Not #[client] (aio's `access: false`), unknown, or an async method's later step: no one.
    for m in ["give", "nope", "fetch.1", "$rewind"] {
        denied(v.allow(&admin, m, &json!([])), "is not a #[client] method");
    }
    assert_eq!(v.allow(&ana, "fetch", &json!([1])), Ok(()));
    // A role is that exact role, read from a session: no role, no entry.
    assert_eq!(v.allow(&admin, "bump", &Value::Null), Ok(()));
    denied(v.allow(&ana, "bump", &Value::Null), "needs the role admin");
    denied(v.allow(&none, "bump", &Value::Null), "needs the role admin");
    // A predicate sees the caller, the arguments and the state.
    assert_eq!(v.allow(&ana, "edit", &json!(["d1"])), Ok(()));
    denied(v.allow(&ana, "edit", &json!(["d2"])), "refused by own_doc");
    denied(v.allow(&admin, "edit", &json!(["d1"])), "refused by own_doc");
    // Arguments the method couldn't take are refused before the rule runs.
    assert!(v.allow(&ana, "edit", &json!([7])).unwrap_err().why.contains("bad arguments"));
    // Both: both must pass; a rule that panics is a refusal, never permission.
    assert_eq!(v.allow(&admin, "add", &json!([3])), Ok(()));
    denied(v.allow(&ana, "add", &json!([3])), "needs the role admin");
    denied(v.allow(&admin, "add", &json!([30])), "small panicked");
    assert_eq!(<Vault as Cell>::CLIENT, ["note", "bump", "edit", "add", "fetch"]);
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

async fn keyed() -> (Handle<Vault>, String) {
    let h = Handle::<Vault>::memory();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(serve(h.clone(), l, "secret".into()));
    (h, addr)
}

fn err(r: Result<Value>) -> String {
    r.unwrap_err().why
}

#[tokio::test(flavor = "multi_thread")]
async fn network_callers_are_gated_and_the_server_is_not() {
    let (h, addr) = keyed().await;
    let net = Link { addr: addr.clone(), token: "secret".into(), root: None };
    let mut ana = Live::connect(&net, "vault", "ana").await.unwrap();
    assert_eq!(ana.call("note", Value::Null).await, Ok(Value::Null));
    // Told why, and nothing changed.
    assert!(err(ana.call("give", json!(["d1", "ana"])).await).contains("`give` is not a #[client] method"));
    assert!(err(ana.call("bump", Value::Null).await).contains("needs the role admin"));
    assert_eq!((h.state().writes, h.state().owners.len()), (1, 0));
    // A forged step of an async method can't skip its first part.
    assert!(err(ana.call("fetch.1", json!([[99]])).await).contains("not a #[client] method"));
    assert_eq!(ana.call("fetch", json!([3])).await, Ok(Value::Null));
    until("fetched", || h.state().writes == 4).await;
    // Server code never asks; the row rule reads the state it set.
    h.give("d1".into(), "ana".into()).unwrap();
    h.bump().unwrap();
    assert_eq!(ana.call("edit", json!(["d1"])).await, Ok(Value::Null));
    assert!(err(ana.call("edit", json!(["d2"])).await).contains("refused by own_doc"));
    assert_eq!(h.state().edited, ["d1"]);
    // A synced copy's prediction of a refused call rolls back.
    let copy = Remote::<Vault>::connect(&addr, "secret", "bo").await.unwrap();
    copy.give("d2".into(), "bo".into()).unwrap();
    until("refused", || copy.pending() == 0).await;
    assert_eq!(*copy.state(), *h.state());
    assert!(!h.state().owners.contains_key("d2"));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_local_door_may_call_anything() {
    let d = tempfile::tempdir().unwrap();
    let h = Handle::<Vault>::memory();
    let addr = aio::ipc::addr(d.path(), "cell-vault");
    let l = aio::ipc::Listener::bind(&addr).unwrap();
    tokio::spawn(aio::sync::serve_door(h.clone(), l, Default::default()));
    let mut ops = Live::connect(&Link { addr, token: String::new(), root: None }, "vault", "ops").await.unwrap();
    assert_eq!(ops.call("give", json!(["d1", "ana"])).await, Ok(Value::Null));
    assert_eq!(ops.call("bump", Value::Null).await, Ok(Value::Null));
    assert_eq!((h.state().writes, &h.state().owners["d1"]), (1, &"ana".to_string()));
}

const PW: &str = "correct horse battery";

#[tokio::test(flavor = "multi_thread")]
async fn a_role_is_the_sessions_and_counts_at_once() {
    let h = Handle::<Vault>::memory();
    let auth = Arc::new(Auth::memory(Policy::default()).unwrap());
    auth.create("ann", PW, Some("admin")).unwrap();
    auth.create("bob", PW, None).unwrap();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let opts = Options { token: "secret".into(), auth: Some(auth.clone()), ..Default::default() };
    tokio::spawn(serve_with(h.clone(), l, opts));
    let link = |user: &str| {
        let aio::auth::Login::Signed { token, .. } = auth.login(user, PW).unwrap() else { panic!("no session") };
        Link { addr: addr.clone(), token, root: None }
    };
    let mut ann = Live::connect(&link("ann"), "vault", "ann").await.unwrap();
    let mut bob = Live::connect(&link("bob"), "vault", "bob").await.unwrap();
    assert_eq!(ann.call("bump", Value::Null).await, Ok(Value::Null));
    assert!(err(bob.call("bump", Value::Null).await).contains("needs the role admin"));
    assert_eq!(bob.call("note", Value::Null).await, Ok(Value::Null));
    assert_eq!(h.state().writes, 2);
    // A new role counts on the next call, on the same link.
    assert!(auth.set_role("bob", "admin").unwrap());
    assert_eq!(bob.call("bump", Value::Null).await, Ok(Value::Null));
    assert!(auth.set_role("ann", "user").unwrap());
    assert!(err(ann.call("bump", Value::Null).await).contains("needs the role admin"));
    assert_eq!(h.state().writes, 3);
}

#[cfg(feature = "compat")]
#[tokio::test(flavor = "multi_thread")]
async fn an_aio_client_is_told_access_denied() {
    let (h, addr) = keyed().await;
    let (mut w, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws?token=secret")).await.unwrap();
    let action = |m: &str, cid: &str| json!({ "v": 2, "t": "action", "d": { "type": format!("vault:{m}"), "payload": { "args": [] }, "cid": cid } });
    w.send(Message::text(action("bump", "c1").to_string())).await.unwrap();
    w.send(Message::text(action("note", "c2").to_string())).await.unwrap();
    let mut acks = vec![];
    while acks.len() < 2 {
        let Message::Text(t) = tokio::time::timeout(Duration::from_secs(5), w.next()).await.unwrap().unwrap().unwrap()
        else {
            continue;
        };
        let v: Value = serde_json::from_str(&t).unwrap();
        if v["t"] == "ack" {
            acks.push(v["d"].clone());
        }
    }
    assert_eq!((&acks[0]["cid"], &acks[0]["ok"]), (&json!("c1"), &json!(false)));
    assert!(acks[0]["error"].as_str().unwrap().starts_with("access denied"), "{}", acks[0]);
    assert_eq!((&acks[1]["cid"], &acks[1]["ok"]), (&json!("c2"), &json!(true)));
    assert_eq!(h.state().writes, 1);
}

#[cell]
pub struct Local {
    pub n: u32,
}

#[methods]
impl Local {
    #[client]
    pub fn bump(&mut self) {
        self.n += 1;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cell_without_shared_serves_its_routes_never_its_state() {
    let http =
        aio::http::Http::default().route(aio::http::Route::get("/ok", |_| async { aio::http::Reply::text("fine") }));
    let s = aio::testing::TestServer::start(Handle::<Local>::memory(), http).await.unwrap();
    let ok = s.fetch("GET", "/ok", "").await.unwrap();
    assert_eq!((ok.status, ok.body.as_str()), (200, "fine"));
    let (url, key) = (format!("ws://{}/ws", s.addr), format!("Bearer {}", s.token));
    let mut req = tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(url).unwrap();
    req.headers_mut().insert("Authorization", key.parse().unwrap());
    assert!(tokio_tungstenite::connect_async(req).await.is_err(), "no sync socket");
}

/// What a viewer is shown by their role (aio's `visible.forUser(s, user)` with `user.role`).
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

#[tokio::test(flavor = "multi_thread")]
async fn what_a_viewer_is_shown_follows_their_role_now() {
    let h = Handle::<Payroll>::memory();
    h.hire("ann".into(), 100).unwrap();
    let auth = Arc::new(Auth::memory(Policy::default()).unwrap());
    auth.create("ann", PW, Some("admin")).unwrap();
    auth.create("bob", PW, None).unwrap();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let opts = Options { token: "secret".into(), auth: Some(auth.clone()), ..Default::default() };
    tokio::spawn(serve_with(h.clone(), l, opts));
    let link = |user: &str| {
        let aio::auth::Login::Signed { token, .. } = auth.login(user, PW).unwrap() else { panic!("no session") };
        Link { addr: addr.clone(), token, root: None }
    };
    let ann = Remote::<Payroll>::connect_to(link("ann"), "ann").await.unwrap();
    let bob = Remote::<Payroll>::connect_to(link("bob"), "bob").await.unwrap();
    assert_eq!(ann.confirmed().salaries, [100]);
    assert_eq!((bob.confirmed().names.len(), bob.confirmed().salaries.len()), (1, 0), "bob sees names only");
    // A new role counts from the next change on, on the same link.
    assert!(auth.set_role("bob", "admin").unwrap());
    assert!(auth.set_role("ann", "user").unwrap());
    h.hire("bob".into(), 90).unwrap();
    until("bob sees salaries", || bob.confirmed().salaries == [100, 90]).await;
    until("ann no longer does", || ann.confirmed().names.len() == 2 && ann.confirmed().salaries.is_empty()).await;
    let p = h.state().project(&Caller { by: By::User("x".into()), role: Some("admin".into()) });
    assert_eq!(p["salaries"], json!([100, 90]));
}

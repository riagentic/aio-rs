//! One app using another (`aio::peer`, aio's `peer()`): found by its id, called
//! through its local door as a client, typed or by name, its blobs read.
use aio::blobs::Blobs;
use aio::fns::{Access, Chunks, ServerFn};
use aio::http::Http;
use aio::peer::Peer;
use aio::prelude::*;
use serde_json::json;
use std::time::Duration;

#[cell(shared)]
pub struct Ledger {
    pub total: i64,
}

#[methods]
impl Ledger {
    #[client]
    pub fn credit(&mut self, n: i64) -> Result<i64> {
        ensure!(n > 0, "nothing to credit");
        self.total += n;
        Ok(self.total)
    }

    /// An app the ledger knows as its auditor's only.
    #[client(role = "auditor")]
    pub fn audit(&mut self, cx: &Cx) -> Result<String> {
        Ok(format!("{:?}", cx.by()))
    }

    /// The operator's only: never a peer's.
    pub fn reset(&mut self) -> Result {
        self.total = 0;
        Ok(())
    }
}

/// A second cell on the app's doors.
#[cell(shared)]
pub struct Tips {
    pub n: u32,
}

#[methods]
impl Tips {
    #[client]
    pub fn tip(&mut self) {
        self.n += 1;
    }
}

const COUNT: ServerFn<(u32,), String> = ServerFn::new("ledger.count");

/// The app `billing` running in `root` (locked), its ledger on its door.
fn billing(root: &std::path::Path) -> (aio::paths::AppDirs, Handle<Ledger>, tokio::task::JoinHandle<()>) {
    billing_with(root, Http::default())
}

fn billing_with(
    root: &std::path::Path,
    http: Http,
) -> (aio::paths::AppDirs, Handle<Ledger>, tokio::task::JoinHandle<()>) {
    let dirs = aio::paths::AppDirs::at(root, "billing").unwrap();
    let h = Handle::<Ledger>::memory();
    let http = http.server_fn_streamed(COUNT, Access::Open, |_, (n,), parts: Chunks| async move {
        for i in 0..n {
            parts.send(&i).await;
        }
        Ok(format!("{n} counted"))
    });
    let addr = aio::sync::door(&dirs.id, &dirs.data, "ledger").unwrap();
    let l = aio::ipc::Listener::bind(&addr).unwrap();
    let hh = h.clone();
    let door = tokio::spawn(async move { _ = aio::sync::serve_door_with(hh, l, Default::default(), &http).await });
    (dirs, h, door)
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

/// One test, so the sockets' folder (process-wide) is this test's own, and goes with it.
#[tokio::test(flavor = "multi_thread")]
async fn peers() {
    let sockets = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("XDG_RUNTIME_DIR", sockets.path()) };
    a_peer_calls_by_name_what_a_client_may_and_nothing_more().await;
    a_peer_cell_is_typed_and_its_server_functions_stream().await;
    a_peer_reads_and_puts_blobs_here_and_fetches_them_from_a_server().await;
    a_peer_is_who_resolve_peer_says_and_door_links_are_heard().await;
}

async fn a_peer_is_who_resolve_peer_says_and_door_links_are_heard() {
    use std::sync::{Arc, Mutex};
    let heard: Arc<Mutex<Vec<(bool, String, String)>>> = Arc::default();
    let log = heard.clone();
    let http = Http::default()
        .door_links(true)
        .on_link(move |p: &aio::http::Presence| log.lock().unwrap().push((p.online, p.user.clone(), p.from.clone())))
        .resolve_peer(|app| match app {
            "shop" => Some(Caller { by: By::User("app:shop".into()), role: Some("auditor".into()) }),
            "boom" => panic!("hook exploded"),
            _ => None,
        });
    let t = tempfile::tempdir().unwrap();
    let (dirs, _h, _door) = billing_with(t.path(), http);
    let shop = Peer::at("billing", &dirs.data, "shop").unwrap().unwrap();
    assert_eq!(
        shop.call("ledger", "audit", json!([])).await,
        Ok(json!("User(\"app:shop\")")),
        "the user and role it maps to"
    );
    for other in ["mail", "boom"] {
        let p = Peer::at("billing", &dirs.data, other).unwrap().unwrap();
        let e = p.call("ledger", "audit", json!([])).await.unwrap_err();
        assert!(e.why.contains("access denied"), "{other}: no one in particular: {e:?}");
        assert_eq!(p.call("ledger", "credit", json!([1])).await, Ok(json!(1 + i64::from(other == "boom"))));
    }
    // A call by name is a tool, never a link; a typed copy on the door is one, heard.
    assert!(heard.lock().unwrap().is_empty(), "{:?}", heard.lock().unwrap());
    let ledger = shop.cell::<Ledger>().await.unwrap();
    until("heard on the door", || heard.lock().unwrap().len() == 1).await;
    assert_eq!(heard.lock().unwrap()[0], (true, "app:shop".into(), "door".into()));
    drop(ledger);
    until("gone", || heard.lock().unwrap().len() == 2).await;
    // Off by default: the door's links aren't heard.
    let log = Arc::new(Mutex::new(0));
    let l2 = log.clone();
    let t = tempfile::tempdir().unwrap();
    let (dirs, _h, _door) = billing_with(t.path(), Http::default().on_link(move |_| *l2.lock().unwrap() += 1));
    let copy = Peer::at("billing", &dirs.data, "shop").unwrap().unwrap().cell::<Ledger>().await.unwrap();
    copy.credit(1).unwrap();
    until("its call went", || copy.pending() == 0).await;
    assert_eq!(*log.lock().unwrap(), 0);
}

async fn a_peer_calls_by_name_what_a_client_may_and_nothing_more() {
    let t = tempfile::tempdir().unwrap();
    assert!(Peer::at("billing", t.path().join("data"), "shop").unwrap().is_none(), "not running: none");
    let (dirs, h, door) = billing(t.path());
    let p = Peer::at("billing", &dirs.data, "shop").unwrap().expect("running");
    assert_eq!(p.call("ledger", "credit", json!([40])).await, Ok(json!(40)));
    assert_eq!(p.call("ledger", "credit", json!([0])).await, Err(Rejected::new("nothing to credit")), "its words");
    let e = p.call("ledger", "reset", json!([])).await.unwrap_err();
    assert!(e.why.contains("access denied"), "a peer is a client, not the operator: {e:?}");
    assert_eq!(h.state().total, 40);
    // What it may call, asked live.
    let d = p.describe("ledger").await.unwrap();
    let may = |m: &str| d["methods"].as_array().unwrap().iter().find(|x| x["method"] == m).unwrap()["may"].clone();
    assert_eq!(may("credit"), "yes");
    assert!(may("reset").as_str().unwrap().starts_with("no:"), "{d}");
    // Every cell at once (aio's app-wide describe()), the same answers.
    let tips = aio::ipc::Listener::bind(&aio::sync::door(&dirs.id, &dirs.data, "tips").unwrap()).unwrap();
    let tips = tokio::spawn(async move {
        _ = aio::sync::serve_door_with(Handle::<Tips>::memory(), tips, Default::default(), &Http::default()).await
    });
    let all = p.describe_all().await.unwrap();
    assert_eq!(all.keys().collect::<Vec<_>>(), ["ledger", "tips"]);
    assert_eq!((&all["ledger"], all["tips"]["methods"][0]["may"].as_str()), (&d, Some("yes")));
    tips.abort();
    // Gone: refused at once, never queued.
    door.abort();
    drop(dirs);
    let e = tokio::time::timeout(Duration::from_secs(5), p.call("ledger", "credit", json!([1]))).await.unwrap();
    assert!(e.unwrap_err().why.starts_with("billing: ledger.credit:"));
    assert!(Peer::at("billing", t.path().join("data"), "shop").unwrap().is_none(), "stopped: none");
}

async fn a_peer_cell_is_typed_and_its_server_functions_stream() {
    let t = tempfile::tempdir().unwrap();
    let (dirs, h, _door) = billing(t.path());
    let p = Peer::at("billing", &dirs.data, "shop").unwrap().unwrap();
    let ledger = p.cell::<Ledger>().await.unwrap();
    ledger.credit(2).unwrap();
    until("the call reached billing", || h.state().total == 2).await;
    h.credit(5).unwrap();
    until("its state follows", || ledger.state().total == 7).await;
    let mut parts = vec![];
    assert_eq!(ledger.run_streamed(COUNT, (3,), |i: u32| parts.push(i)).await, Ok("3 counted".to_string()));
    assert_eq!(parts, [0, 1, 2]);
}

async fn a_peer_reads_and_puts_blobs_here_and_fetches_them_from_a_server() {
    let t = tempfile::tempdir().unwrap();
    let (dirs, h, _door) = billing(t.path());
    let p = Peer::at("billing", &dirs.data, "shop").unwrap().unwrap();
    let theirs = Blobs::of(&dirs);
    let b = theirs.put(&b"0123456789"[..], Some("r.txt")).unwrap();
    assert_eq!(p.blobs().read(&b.id, 2, 5).unwrap(), b"234");
    // Put by the peer, then passed to the other app by id.
    let mine = p.blobs().put(&b"receipt"[..], None).unwrap();
    assert_eq!(theirs.read(&mine.id, 0, u64::MAX).unwrap(), b"receipt");

    // On another machine: its served blobs, with its key, ranges too.
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let opts = aio::sync::Options { token: "secret".into(), http: Http::default().blobs(theirs), ..Default::default() };
    tokio::spawn(aio::sync::serve_with(h, l, opts));
    until("served", || std::net::TcpStream::connect(&addr).is_ok()).await;
    let link = aio::sync::Link { addr, token: "secret".into(), root: None };
    assert_eq!(aio::peer::blob(&link, &b.id, 2, 5).await.unwrap(), b"234");
    assert_eq!(aio::peer::blob(&link, &b.id, 0, u64::MAX).await.unwrap(), b"0123456789");
    assert_eq!(aio::peer::blob(&link, &b.id, 8, 99).await.unwrap(), b"89", "clamped to its end");
    assert_eq!(aio::peer::blob(&link, &b.id, 3, 3).await.unwrap(), b"", "an empty range");
    let none = "0".repeat(64);
    assert_eq!(aio::peer::blob(&link, &none, 0, 1).await.unwrap_err().kind(), std::io::ErrorKind::NotFound);
    let wrong = aio::sync::Link { token: "nope".into(), ..link.clone() };
    assert_eq!(aio::peer::blob(&wrong, &b.id, 0, 1).await.unwrap_err().kind(), std::io::ErrorKind::PermissionDenied);
    assert!(aio::peer::blob(&link, "../etc", 0, 1).await.is_err(), "never a path");
}

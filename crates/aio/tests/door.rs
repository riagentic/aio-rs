//! Tools and scripts reach a running app: its local door (`sync::serve_door`)
//! and `Live`, the untyped client the `ax` command uses.
use aio::prelude::*;
use aio::sync::{Link, Live, Remote};
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
    pub fn add(&mut self, cx: &Cx, text: String) -> Result<Uuid> {
        ensure!(!text.is_empty(), "empty note");
        let id = cx.id();
        self.notes.insert(id, text);
        Ok(id)
    }
}

/// Only `the_door_lists_its_links_and_the_network_does_not` serves it: the
/// open-link list is per cell, and other tests link to `Board`.
#[cell(shared)]
pub struct Tally {
    pub n: u32,
}

#[methods]
impl Tally {
    #[client]
    pub fn bump(&mut self) -> Result<u32> {
        self.n += 1;
        Ok(self.n)
    }
}

/// Only `each_links_round_trip_is_timed_by_the_servers_pings` serves it.
#[cell(shared)]
pub struct Pinged {
    pub n: u32,
}

#[methods]
impl Pinged {}

/// `h` served on a fresh door; the link to it.
fn door<T: Cell>(h: &Handle<T>, dir: &std::path::Path, name: &str) -> Link {
    let addr = aio::ipc::addr(dir, name);
    let l = aio::ipc::Listener::bind(&addr).unwrap();
    tokio::spawn(aio::sync::serve_door(h.clone(), l, Default::default()));
    Link { addr, token: String::new(), root: None }
}

/// A fake server's next text message from the client (pings skipped).
async fn next_text<S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin>(
    w: &mut S,
) -> serde_json::Value {
    loop {
        if let Message::Text(t) = w.next().await.unwrap().unwrap() {
            return serde_json::from_str(&t).unwrap();
        }
    }
}

async fn until(what: &str, f: impl Fn() -> bool) {
    for _ in 0..1000 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out: {what}");
}

#[test]
fn doors_are_told_from_network_addresses() {
    assert!(aio::sync::is_door("/run/user/1/aio-rt/notes-0a1b2c3d/cell-notes.sock"));
    assert!(aio::sync::is_door(r"\\.\pipe\aio-rt-notes-0a1b2c3d-cell-notes"));
    assert!(!aio::sync::is_door("127.0.0.1:7700"));
    assert!(!aio::sync::is_door("[::1]:7700"));
    assert!(!aio::sync::is_door("notes.local:7700"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tool_reads_and_calls_a_running_app_on_its_door() {
    let d = tempfile::tempdir().unwrap();
    let h = Handle::<Board>::memory();
    h.add("before".into()).unwrap();
    let link = door(&h, d.path(), "cell-board");
    // No key on the door: the OS admits only this user.
    let mut t = Live::connect(&link, "board", "ops").await.unwrap();
    assert_eq!(t.view(), &serde_json::to_value(&*h.state()).unwrap());
    assert_eq!((t.seq(), t.version()), (1, 1));
    // A call waits for the server; its change is in the view by then.
    let id = t.call("add", serde_json::json!(["from the shell"])).await.unwrap();
    let id: Uuid = serde_json::from_value(id).unwrap();
    assert_eq!(h.state().notes[&id], "from the shell");
    assert_eq!(t.view()["notes"][id.to_string()], "from the shell");
    assert_eq!(h.store(|s| s.events().unwrap()).last().unwrap().by, By::User("ops".into()));
    // Refusals and unknown methods come back as the server's words.
    assert_eq!(t.call("add", serde_json::json!([""])).await, Err(Rejected::new("empty note")));
    let nope = t.call("nope", serde_json::json!([])).await.unwrap_err();
    assert!(nope.why.contains("nope"));
    // Told apart by code, not words (aio's `errorCode`): aio's refusal, or the method's own.
    assert_eq!((nope.code(), Rejected::new("empty note").code()), (Some(aio::Code::ActionRefused), None));
    // Live: another caller's change reaches the tool.
    h.add("elsewhere".into()).unwrap();
    t.changed().await.unwrap();
    assert_eq!(t.view(), &serde_json::to_value(&*h.state()).unwrap());
    // Another cell on this door is refused, by name.
    let e = Live::connect(&link, "todo", "ops").await.err().unwrap();
    assert!(e.to_string().contains("this server has cell board"), "{e}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tool_with_no_app_running_is_told_at_once() {
    let d = tempfile::tempdir().unwrap();
    let link = Link { addr: aio::ipc::addr(d.path(), "cell-board"), token: String::new(), root: None };
    let e = Live::connect(&link, "board", "ops").await.err().unwrap();
    assert!(matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused), "{e:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_typed_copy_syncs_over_the_door_and_queues_while_the_app_is_away() {
    let d = tempfile::tempdir().unwrap();
    let file = d.path().join("copy.json");
    let h = Handle::<Board>::memory();
    let first = door(&h, d.path(), "cell-board");
    let a = Remote::<Board>::connect_saved(first, "ana", &file).await.unwrap();
    a.add("online".into()).unwrap();
    until("confirmed", || a.pending() == 0).await;
    let saved = || std::fs::read_to_string(&file).unwrap_or_default();
    until("saved", || saved().contains("online") && !saved().contains("\"method\"")).await;
    drop(a);
    // The app is away (no door yet): the copy starts from disk and queues.
    let later = Link { addr: aio::ipc::addr(d.path(), "cell-board-2"), token: String::new(), root: None };
    let b = Remote::<Board>::connect_saved(later, "ana", &file).await.unwrap();
    assert!(!b.online());
    let id = b.add("offline".into()).unwrap();
    assert_eq!((b.pending(), b.state().notes.len()), (1, 2));
    // The app comes back: the queued call reaches it, once.
    door(&h, d.path(), "cell-board-2");
    until("sent", || b.pending() == 0).await;
    assert_eq!(h.state().notes[&id], "offline");
    assert_eq!(*b.state(), *h.state());
}

/// aio's transport probe: each link's round trip, timed by the server's pings.
#[tokio::test(flavor = "multi_thread")]
async fn each_links_round_trip_is_timed_by_the_servers_pings() {
    let d = tempfile::tempdir().unwrap();
    let h = Handle::<Pinged>::memory();
    let at_door = door(&h, d.path(), "cell-pinged");
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(aio::sync::serve(h.clone(), l, "secret".into()));
    let _ana = Remote::<Pinged>::connect(&addr, "secret", "ana").await.unwrap();
    let mut ops = Live::connect(&at_door, "pinged", "ops").await.unwrap();
    let rtt = |list: &[serde_json::Value]| list.iter().find(|c| c["user"] == "ana").and_then(|c| c["rtt"].as_u64());
    assert_eq!(rtt(&ops.clients().await.unwrap()), None, "no ping yet");
    let end = std::time::Instant::now() + Duration::from_secs(8);
    loop {
        if let Some(ms) = rtt(&ops.clients().await.unwrap()) {
            assert!(ms < aio::vitals::get().transport.warning, "a loopback link: {ms} ms");
            break;
        }
        assert!(std::time::Instant::now() < end, "no round trip timed");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let tool = ops.clients().await.unwrap().into_iter().find(|c| c["user"] == "ops").unwrap();
    assert!(tool["rtt"].is_null(), "a tool is no client link");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_door_lists_its_links_and_the_network_does_not() {
    let d = tempfile::tempdir().unwrap();
    let h = Handle::<Tally>::memory();
    let at_door = door(&h, d.path(), "cell-tally");
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(aio::sync::serve(h.clone(), l, "secret".into()));
    let ana = Remote::<Tally>::connect(&addr, "secret", "ana").await.unwrap();
    let mut ops = Live::connect(&at_door, "tally", "ops").await.unwrap();
    let list = ops.clients().await.unwrap();
    let rows: Vec<(&str, &str, bool)> =
        list.iter().map(|c| (c["user"].as_str().unwrap(), c["wire"].as_str().unwrap(), c["from"] == "door")).collect();
    assert_eq!(rows, [("ana", "aio-rs", false), ("ops", "tool", true)]);
    assert!(list[0]["from"].as_str().unwrap().starts_with("127.0.0.1:"));
    // Over the network a tool (with the key) reads and calls, but who else
    // is linked stays the operator's to know.
    let net = Link { addr: addr.clone(), token: "secret".into(), root: None };
    let mut script = Live::connect(&net, "tally", "script").await.unwrap();
    assert_eq!(script.call("bump", serde_json::Value::Null).await, Ok(serde_json::json!(1)));
    assert_eq!(script.view()["n"], 1);
    let e = script.clients().await.unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied);
    // A wrong key is refused before any state.
    let wrong = Link { token: "guess".into(), ..net };
    assert_eq!(Live::connect(&wrong, "tally", "eve").await.err().unwrap().kind(), std::io::ErrorKind::PermissionDenied);
    // A link that closes leaves the list.
    drop((ana, script));
    for _ in 0..300 {
        if ops.clients().await.unwrap().len() == 1 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("closed links stayed listed");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tool_at_the_door_travels_the_history_and_nothing_is_lost() {
    let d = tempfile::tempdir().unwrap();
    let h = Handle::<Board>::open(d.path().join("board.db")).unwrap();
    for t in ["a", "b", "c"] {
        h.add(t.into()).unwrap();
    }
    let mut ops = Live::connect(&door(&h, d.path(), "cell-travel"), "board", "ops").await.unwrap();
    let r = ops.travel("where", None).await.unwrap();
    assert_eq!((r["at"].as_u64(), r["head"].as_u64(), &r["paused"]), (Some(3), Some(3), &serde_json::json!(false)));
    let steps = r["steps"].as_array().unwrap();
    assert_eq!(steps.iter().map(|s| s["method"].as_str().unwrap()).collect::<Vec<_>>(), ["add", "add", "add"]);
    assert!(steps.iter().all(|s| s["ms"].as_f64().is_some()), "each call's own time: {steps:?}");
    // A step back pauses: the state stays where travel put it.
    let r = ops.travel("undo", None).await.unwrap();
    assert_eq!((r["at"].as_u64(), &r["paused"]), (Some(2), &serde_json::json!(true)));
    assert_eq!(h.state().notes.len(), 2);
    assert!(h.add("while paused".into()).is_err(), "a paused cell takes no calls");
    ops.travel("undo", None).await.unwrap();
    assert_eq!(h.state().notes.len(), 1);
    let r = ops.travel("redo", None).await.unwrap();
    assert_eq!((r["at"].as_u64(), h.state().notes.len()), (Some(2), 2));
    assert_eq!(ops.travel("goto", Some(3)).await.unwrap()["at"], 3);
    assert!(ops.travel("redo", None).await.unwrap_err().why.contains("nothing to redo"));
    assert!(ops.travel("goto", Some(99)).await.unwrap_err().why.contains("no event 99"));
    assert!(ops.travel("sideways", None).await.unwrap_err().why.contains("unknown time travel"));
    // Back to the start, resume, a new call: every step is in the log.
    ops.travel("goto", Some(0)).await.unwrap();
    assert!(ops.travel("undo", None).await.unwrap_err().why.contains("nothing to undo"));
    assert_eq!(ops.travel("resume", None).await.unwrap()["paused"], false);
    h.add("d".into()).unwrap();
    assert_eq!(h.state().notes.values().collect::<Vec<_>>(), ["d"]);
    let methods: Vec<String> = h.store(|s| s.events().unwrap()).into_iter().map(|e| e.method).collect();
    assert_eq!(methods, ["add", "add", "add", "$rewind", "$rewind", "$rewind", "$rewind", "$rewind", "add"]);
    // Over the network: never.
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let net = Link { addr: l.local_addr().unwrap().to_string(), token: "secret".into(), root: None };
    tokio::spawn(aio::sync::serve(h.clone(), l, "secret".into()));
    let mut script = Live::connect(&net, "board", "script").await.unwrap();
    assert!(script.travel("undo", None).await.unwrap_err().why.contains("only on the app's local door"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tool_with_a_gap_takes_the_full_view() {
    // A fake server: a view at event 5, then a patch that follows event 7.
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let fake = tokio::spawn(async move {
        let (s, _) = l.accept().await.unwrap();
        let mut w = tokio_tungstenite::accept_async(s).await.unwrap();
        let send = |v: serde_json::Value| Message::text(v.to_string());
        assert_eq!(next_text(&mut w).await["tool"], true);
        w.send(send(serde_json::json!({ "t": "state", "seq": 5, "v": 1, "state": { "luck": 0 } }))).await.unwrap();
        w.send(send(serde_json::json!({ "t": "patch", "prev": 7, "seq": 9, "ops": [] }))).await.unwrap();
        assert_eq!(next_text(&mut w).await["t"], "resync");
        w.send(send(serde_json::json!({ "t": "state", "seq": 9, "state": { "luck": 42 } }))).await.unwrap();
    });
    let mut t = Live::connect(&Link { addr, token: "t".into(), root: None }, "board", "u").await.unwrap();
    t.changed().await.unwrap();
    assert_eq!((t.seq(), &t.view()["luck"]), (9, &serde_json::json!(42)));
    fake.await.unwrap();
}

#[test]
fn an_app_whose_cell_lives_here_opens_its_door() {
    let d = tempfile::tempdir().unwrap();
    let id = format!("door-test-{}", std::process::id());
    let cfg = aio::config::Config::parse(&format!("[app]\nid = \"{id}\"")).unwrap();
    let dirs = aio::paths::AppDirs::at(d.path(), &id).unwrap();
    let s = aio::Source::<Board>::for_app(&cfg, &dirs).unwrap();
    s.call("add", serde_json::json!(["standalone"])).unwrap();
    let addr = aio::sync::door(&id, &dirs.data, "board").unwrap();
    let link = Link { addr: addr.clone(), token: String::new(), root: None };
    let t = aio::rt().block_on(Live::connect(&link, "board", "ops")).unwrap();
    assert_eq!(t.view()["notes"].as_object().unwrap().values().next().unwrap(), "standalone");
    // Nothing left behind in this user's socket dir.
    _ = std::fs::remove_dir_all(aio::paths::sockets(&id, &dirs.data).unwrap());
}

#[test]
fn a_server_binary_opens_its_store_and_refuses_client_mode() {
    let d = tempfile::tempdir().unwrap();
    let id = format!("door-server-{}", std::process::id());
    let dirs = aio::paths::AppDirs::at(d.path(), &id).unwrap();
    let client = aio::config::Config::parse(&format!("[app]\nid = \"{id}\"\nmode = \"client\"")).unwrap();
    let e = aio::Source::<Board>::for_server(&client, &dirs).err().unwrap();
    assert_eq!(e.kind(), std::io::ErrorKind::Unsupported);
    assert!(e.to_string().contains("needs aio::Source::for_app"), "{e}");
    let cfg = aio::config::Config::parse(&format!("[app]\nid = \"{id}\"")).unwrap();
    let s = aio::Source::<Board>::for_server(&cfg, &dirs).unwrap();
    s.call("add", serde_json::json!(["kept"])).unwrap();
    assert!(dirs.store.exists());
    _ = std::fs::remove_dir_all(aio::paths::sockets(&id, &dirs.data).unwrap());
}

/// Only `the_operator_moves_loads_pauses_and_shapes_on_the_door_alone` serves it.
#[cell(persist, shared)]
pub struct Steps {
    pub n: u32,
    pub tags: Vec<String>,
}

#[methods]
impl Steps {
    #[client]
    pub fn inc(&mut self) -> Result<u32> {
        self.n += 1;
        Ok(self.n)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_operator_moves_loads_pauses_and_shapes_on_the_door_alone() {
    use serde_json::json;
    let d = tempfile::tempdir().unwrap();
    let h = Handle::<Steps>::memory();
    for _ in 0..3 {
        h.inc().unwrap();
    }
    let link = door(&h, d.path(), "cell-steps");
    let mut t = Live::connect(&link, "steps", "ops").await.unwrap();
    // Back to the state after event 1: a new event, the steps between kept.
    assert_eq!(t.travel("goto", Some(1)).await.unwrap()["at"], 1);
    until("the view follows", || h.state().n == 1).await;
    assert_eq!(h.store(|s| (s.state_at(3).unwrap().n, s.events().unwrap().len())), (3, 4), "nothing is lost");
    assert!(t.travel("goto", Some(99)).await.unwrap_err().why.contains("no event 99"));
    assert_eq!(t.travel("resume", None).await.unwrap()["paused"], false);
    // A state loaded whole, as `ax snapshot` wrote it; one that doesn't fit changes nothing.
    t.ask(json!({ "t": "load", "state": { "n": 42, "tags": ["x"] } })).await.unwrap();
    assert_eq!((h.state().n, t.view()["n"].as_u64()), (42, Some(42)));
    let e = t.ask(json!({ "t": "load", "state": { "n": "many" } })).await.unwrap_err();
    assert!(e.to_string().contains("doesn't fit"), "{e}");
    assert_eq!(h.state().n, 42);
    // Paused: every call is refused, never swallowed; a rewind still moves it.
    assert_eq!(t.travel("pause", None).await.unwrap()["paused"], true);
    assert!(t.call("inc", json!([])).await.unwrap_err().why.contains("paused by time travel"));
    assert!(h.inc().unwrap_err().why.contains("paused"));
    t.travel("goto", Some(2)).await.unwrap();
    assert_eq!(h.state().n, 2);
    assert_eq!(t.travel("resume", None).await.unwrap()["paused"], false);
    assert_eq!(t.call("inc", json!([])).await.unwrap(), 3);
    // The whole state right after any event, for `ax events --diff`.
    assert_eq!(t.ask(json!({ "t": "at", "to": 5 })).await.unwrap(), json!({ "n": 42, "tags": ["x"] }));
    assert_eq!(t.ask(json!({ "t": "at", "to": 0 })).await.unwrap(), json!({ "n": 0, "tags": [] }));
    assert!(t.ask(json!({ "t": "at", "to": 999 })).await.is_err());
    // The shape this version saves, for `ax migrations`.
    let shape = t.ask(json!({ "t": "shape" })).await.unwrap();
    assert_eq!(shape, json!({ "v": 1, "shape": { "n": "number", "tags": [] } }), "an empty list: no item to shape");
    // Over the network: none of it.
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(aio::sync::serve(h.clone(), l, "secret".into()));
    let mut net =
        Live::connect(&Link { addr: addr.clone(), token: "secret".into(), root: None }, "steps", "ops").await.unwrap();
    for m in ["load", "shape", "at"] {
        let e = net.ask(json!({ "t": m, "to": 1, "on": true })).await.unwrap_err();
        assert!(e.to_string().contains("only on the app's local door"), "{m}: {e}");
    }
    assert_eq!((h.state().n, h.paused()), (3, false));
    // What a client is sent counts for `ax cost`; the tools' looks don't.
    assert_eq!(t.ask(json!({ "t": "cost" })).await.unwrap()["pushes"], 0);
    let _ana = Remote::<Steps>::connect(&addr, "secret", "ana").await.unwrap();
    h.inc().unwrap();
    let cost = || aio::cost::report("steps", None);
    until("the patch is counted", || cost()["pushes"] == 2).await;
    let c = t.ask(json!({ "t": "cost" })).await.unwrap();
    assert_eq!((c["fullResends"].as_u64(), c["keys"][0]["key"].as_str()), (Some(1), Some("n")));
    assert!(c["calls"].as_u64().unwrap() >= 4, "its methods are timed: {c}");
}

/// Only `a_tool_asks_what_a_caller_may_call_and_its_signatures` serves it.
#[cell(shared)]
pub struct Gated {
    pub n: u32,
}

fn small(_: &Gated, _: &aio::Caller, by: u32, _: Option<String>) -> bool {
    by < 10
}

#[methods]
impl Gated {
    #[client(role = "admin")]
    pub fn reset(&mut self) -> Result {
        self.n = 0;
        Ok(())
    }

    #[client(allow = small)]
    pub fn add(&mut self, by: u32, why: Option<String>) -> Result<u32> {
        let _ = why;
        self.n += by;
        Ok(self.n)
    }

    pub fn fix(&mut self, to: (u32, String)) -> Result {
        self.n = to.0;
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tool_asks_what_a_caller_may_call_and_its_signatures() {
    use serde_json::json;
    let d = tempfile::tempdir().unwrap();
    let h = Handle::<Gated>::memory();
    let mut t = Live::connect(&door(&h, d.path(), "cell-gated"), "gated", "ops").await.unwrap();
    let ana = t.ask(json!({ "t": "peer", "user": "ana" })).await.unwrap();
    assert_eq!((ana["cell"].as_str(), ana["v"].as_u64()), (Some("gated"), Some(1)));
    let m = |p: &serde_json::Value, i: usize, k: &str| p["methods"][i][k].clone();
    assert_eq!(m(&ana, 0, "sig"), "reset() -> ()");
    assert_eq!(m(&ana, 1, "sig"), "add(by: u32, why: Option<String>) -> u32");
    assert_eq!(m(&ana, 2, "sig"), "fix(to: (u32, String)) -> ()");
    assert!(m(&ana, 0, "may").as_str().unwrap().contains("needs the role admin"), "{ana}");
    assert!(m(&ana, 1, "may").as_str().unwrap().starts_with("by its arguments"), "{ana}");
    assert_eq!(
        (m(&ana, 2, "client"), m(&ana, 2, "may").as_str().unwrap().contains("not a #[client]")),
        (json!(false), true)
    );
    let admin = t.ask(json!({ "t": "peer", "user": "bo", "role": "admin" })).await.unwrap();
    assert_eq!(m(&admin, 0, "may"), "yes");
    // Over the network (aio's `am peer <url>`): the key holder asks about
    // itself; the user and role it claims in the ask are not taken.
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(aio::sync::serve(h.clone(), l, "secret".into()));
    let mut net = Live::connect(&Link { addr, token: "secret".into(), root: None }, "gated", "cy").await.unwrap();
    let cy = net.ask(json!({ "t": "peer", "user": "bo", "role": "admin" })).await.unwrap();
    assert_eq!(m(&cy, 1, "sig"), "add(by: u32, why: Option<String>) -> u32");
    assert!(m(&cy, 0, "may").as_str().unwrap().contains("needs the role admin"), "never the claimed role: {cy}");
    assert!(net.ask(json!({ "t": "fields" })).await.unwrap_err().to_string().contains("only on the app's local door"));
}

/// Only `a_tool_asks_which_fields_are_saved_and_which_are_sent` serves it.
#[cell(persist, shared)]
pub struct Vault {
    pub label: String,
    #[server]
    pub pin: String,
    #[transient]
    pub seen: u32,
}

#[methods]
impl Vault {}

/// Only `a_tool_asks_which_fields_are_saved_and_which_are_sent` serves it: kept in memory.
#[cell(shared)]
pub struct Loose {
    pub n: u32,
}

#[methods]
impl Loose {}

/// aio's trojan `fields` (amui's per-field badges): the door says which
/// fields are saved and which are sent.
#[tokio::test(flavor = "multi_thread")]
async fn a_tool_asks_which_fields_are_saved_and_which_are_sent() {
    use serde_json::json;
    let d = tempfile::tempdir().unwrap();
    let h = Handle::<Vault>::memory();
    let mut t = Live::connect(&door(&h, d.path(), "cell-vault"), "vault", "ops").await.unwrap();
    let f = t.ask(json!({ "t": "fields" })).await.unwrap();
    assert_eq!(f, json!({ "persist": true, "transient": ["seen"], "server": ["pin"] }));
    let mut t = Live::connect(&door(&Handle::<Loose>::memory(), d.path(), "cell-loose"), "loose", "ops").await.unwrap();
    assert_eq!(
        t.ask(json!({ "t": "fields" })).await.unwrap(),
        json!({ "persist": false, "transient": [], "server": [] })
    );
}

/// Only `the_operator_writes_now_and_drops_a_cell_nothing_declares` serves it.
#[cell(persist, shared)]
pub struct Kept {
    pub n: u32,
}

#[methods]
impl Kept {
    pub fn inc(&mut self) {
        self.n += 1;
    }
}

/// aio's `am persist` and `am persist --drop`: the door writes what it
/// holds now, and drops a stored cell this app doesn't declare, never its own.
#[tokio::test(flavor = "multi_thread")]
async fn the_operator_writes_now_and_drops_a_cell_nothing_declares() {
    use serde_json::json;
    let d = tempfile::tempdir().unwrap();
    let file = d.path().join("state.db");
    // A cell an older build stored, then no more.
    let old = Handle::<Steps>::open(&file).unwrap();
    old.inc().unwrap();
    drop(old);
    let h = Handle::<Kept>::open(&file).unwrap();
    h.inc().unwrap();
    let mut t = Live::connect(&door(&h, d.path(), "cell-kept"), "kept", "ops").await.unwrap();
    assert!(t.ask(json!({ "t": "flush" })).await.unwrap().as_u64().is_some());
    let rows = |cell: &str| {
        let db = rusqlite::Connection::open(&file).unwrap();
        db.query_row("SELECT COUNT(*) FROM log WHERE cell = ?1", [cell], |r| r.get::<_, i64>(0)).unwrap()
    };
    assert_eq!(rows("steps"), 1);
    assert_eq!(t.ask(json!({ "t": "drop", "cell": "steps" })).await.unwrap(), true);
    assert_eq!((rows("steps"), rows("kept")), (0, 1), "only the cell nothing declares");
    assert_eq!(t.ask(json!({ "t": "drop", "cell": "steps" })).await.unwrap(), false, "gone already");
    let e = t.ask(json!({ "t": "drop", "cell": "kept" })).await.unwrap_err();
    assert!(e.to_string().contains("never dropped"), "{e}");
}

//! aio pages and tools on `/ws` against an aio-rs app whose one cell holds many
//! of aio's (`#[aio_cell]` fields), and aio's offline CRDT sync (`op`, `sync-req`).
use aio::prelude::*;
use aio::sync::{Remote, serve};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

#[cell(persist, id = "settings", version = 2)]
pub struct Settings {
    pub theme: String,
}

#[cell(persist, id = "purse")]
pub struct Purse {
    pub coins: u64,
}

/// A port of an aio app with cells `settings` and `purse`.
#[cell(persist, shared)]
pub struct Port {
    #[aio_cell]
    pub settings: Settings,
    #[aio_cell]
    pub purse: Purse,
    pub seen: u32,
}

#[methods]
impl Port {
    /// aio's `settings:theme`.
    #[client]
    pub fn theme(&mut self, t: String) -> Result {
        ensure!(!t.is_empty(), "no theme");
        self.settings.theme = t;
        Ok(())
    }

    /// aio's `purse:earn`.
    #[client]
    pub fn earn(&mut self, n: u64) {
        self.purse.coins += n;
    }

    /// aio's `purse:reset` (`port:reset` is the other one).
    #[client]
    pub fn purse_reset(&mut self) {
        self.purse.coins = 0;
    }

    #[client]
    pub fn reset(&mut self) {
        self.seen = 0;
    }

    #[client]
    pub fn see(&mut self) {
        self.seen += 1;
    }
}

async fn server() -> (Handle<Port>, String) {
    let h = Handle::<Port>::memory();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(serve(h.clone(), l, "secret".into()));
    (h, addr)
}

type Sock = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn page(addr: &str) -> Sock {
    let (mut w, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws?token=secret")).await.unwrap();
    for t in ["proto", "state", "boot"] {
        assert_eq!(recv(&mut w).await["t"], t);
    }
    send(&mut w, json!({ "t": "proto", "d": { "v": 3, "min": 3 } })).await;
    w
}

async fn recv(w: &mut Sock) -> Value {
    loop {
        if let Message::Text(t) =
            tokio::time::timeout(Duration::from_secs(5), w.next()).await.unwrap().unwrap().unwrap()
        {
            return serde_json::from_str(&t).unwrap();
        }
    }
}

/// The next frame of kind `t`, skipping others.
async fn recv_kind(w: &mut Sock, t: &str) -> Value {
    loop {
        let v = recv(w).await;
        if v["t"] == t {
            return v["d"].clone();
        }
    }
}

async fn quiet(w: &mut Sock, t: &str) {
    let got = tokio::time::timeout(Duration::from_millis(300), recv_kind(w, t)).await;
    assert!(got.is_err(), "no {t} expected: {got:?}");
}

async fn send(w: &mut Sock, mut v: Value) {
    v["v"] = json!(2);
    w.send(Message::text(v.to_string())).await.unwrap();
}

async fn act(w: &mut Sock, kind: &str, args: Value, cid: &str) -> Value {
    send(w, json!({ "t": "action", "d": { "type": kind, "payload": { "args": args }, "cid": cid } })).await;
    recv_kind(w, "ack").await
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
async fn an_aio_page_sees_and_calls_each_slice_as_a_cell_of_its_own() {
    let (h, addr) = server().await;
    h.earn(3).unwrap();
    let (mut w, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws?token=secret")).await.unwrap();
    assert_eq!(recv(&mut w).await["d"]["appId"], "port");
    let st = recv(&mut w).await;
    assert_eq!(
        st["d"],
        json!({ "settings": { "theme": "" }, "purse": { "coins": 3 }, "port": { "seen": 0 } }),
        "each slice is its aio cell, the app's cell keeps the rest"
    );
    assert_eq!(recv(&mut w).await["t"], "boot");
    // `<slice>:<m>` calls the app cell's `<slice>_<m>`, else `m`.
    assert_eq!(act(&mut w, "settings:theme", json!(["dark"]), "1").await["ok"], true);
    assert_eq!(h.state().settings.theme, "dark");
    h.see().unwrap();
    let p = recv_kind(&mut w, "patches").await;
    assert_eq!(p, json!([{ "op": "replace", "path": ["port", "seen"], "value": 1 }]));
    assert_eq!(act(&mut w, "purse:reset", json!([]), "2").await["ok"], true);
    assert_eq!((h.state().purse.coins, h.state().seen), (0, 1), "purse_reset, not reset");
    assert_eq!(act(&mut w, "port:reset", json!([]), "3").await["ok"], true);
    assert_eq!(h.state().seen, 0);
    let refused = act(&mut w, "settings:theme", json!([""]), "4").await;
    assert_eq!((&refused["ok"], &refused["error"]), (&json!(false), &json!("no theme")));
    for kind in ["nope:theme", "purse:__x", "purse:", "theme"] {
        assert_eq!(act(&mut w, kind, json!([]), "5").await["ok"], false, "{kind}");
    }
    // Subscribed per slice: only its own state and patches.
    send(&mut w, json!({ "t": "subs", "d": { "subs": ["purse.coins"] } })).await;
    let st = recv_kind(&mut w, "state").await;
    assert_eq!(st, json!({ "purse": { "coins": 0 } }));
    h.theme("light".into()).unwrap();
    quiet(&mut w, "patches").await;
    h.earn(2).unwrap();
    assert_eq!(
        recv_kind(&mut w, "patches").await,
        json!([{ "op": "replace", "path": ["purse", "coins"], "value": 2 }])
    );
    send(&mut w, json!({ "t": "resync" })).await;
    assert_eq!(recv_kind(&mut w, "state").await, json!({ "purse": { "coins": 2 } }));
}

/// Both ways across both links: what an aio page does an aio-rs copy sees, and
/// what an aio-rs copy does an aio page and an aio-rs app's `AioLink` see, per slice.
#[tokio::test(flavor = "multi_thread")]
async fn an_aio_rs_copy_and_an_aio_page_share_the_slices_both_ways() {
    use aio::{compat::AioLink, sync::Link};
    let (h, addr) = server().await;
    let copy = Remote::<Port>::connect(&addr, "secret", "ana").await.unwrap();
    let mut w = page(&addr).await;
    assert_eq!(act(&mut w, "purse:earn", json!([4]), "1").await["ok"], true);
    until("the copy sees the aio page's call", || copy.state().purse.coins == 4).await;
    copy.theme("sepia".into()).unwrap();
    let p = recv_kind(&mut w, "patches").await;
    assert_eq!(p, json!([{ "op": "replace", "path": ["settings", "theme"], "value": "sepia" }]));
    let mut tool = AioLink::connect(&Link { addr, token: "secret".into(), root: None }).await.unwrap();
    assert_eq!(tool.cell::<Settings>("settings").unwrap(), h.state().settings);
    assert_eq!(tool.cell::<Purse>("purse").unwrap().coins, 4);
    tool.call("purse", "earn", json!([1])).await.unwrap();
    assert_eq!(tool.cell::<Purse>("purse").unwrap().coins, 5);
    until("the copy sees the tool's call", || copy.state().purse.coins == 5).await;
    tool.subscribe(&["settings"]).await.unwrap();
    assert!(tool.cell::<Purse>("purse").is_err());
    assert_eq!(tool.cell::<Settings>("settings").unwrap().theme, "sepia");
}

fn op(id: &str, cell: &str, action: &str, args: Value, at: i64) -> Value {
    json!({ "id": id, "cell": cell, "action": action, "payload": { "args": args }, "hlc": [at, 0, "client-a"] })
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64
}

#[tokio::test(flavor = "multi_thread")]
async fn aios_crdt_ops_are_acked_once_passed_on_and_caught_up_by_snapshot() {
    let (h, addr) = server().await;
    let (mut a, mut b) = (page(&addr).await, page(&addr).await);
    let req = |pending: Value, cells: Value| json!({ "t": "sync-req", "d": { "clientId": "a", "reqId": 1, "cells": cells, "pendingOps": pending } });
    // Both sync: b's first ask is a snapshot at the cell's seq.
    send(&mut b, req(json!([]), json!({ "purse": { "lastHlc": null } }))).await;
    let res = recv_kind(&mut b, "sync-res").await;
    assert_eq!(
        (&res["mode"], &res["snapshot"], &res["reqId"]),
        (&json!("snapshot"), &json!({ "purse": { "coins": 0 } }), &json!(1))
    );
    send(&mut a, req(json!([]), json!({}))).await;
    assert_eq!(recv_kind(&mut a, "sync-res").await["mode"], "incremental");
    // An op: acked at its event, and passed on to b (not back to a).
    let t = now();
    send(&mut a, json!({ "t": "op", "d": op("a-s-1.x", "purse", "earn", json!([7]), t) })).await;
    let ack = recv_kind(&mut a, "sync-ack").await;
    assert_eq!(
        (&ack["cell"], &ack["opId"], &ack["serverHlc"][2]),
        (&json!("purse"), &json!("a-s-1.x"), &json!("server"))
    );
    let at = ack["serverTs"].as_u64().unwrap();
    assert_eq!(h.state().purse.coins, 7);
    let passed = recv_kind(&mut b, "op").await;
    assert_eq!(
        (&passed["id"], &passed["action"], &passed["payload"]["args"]),
        (&json!("a-s-1.x"), &json!("earn"), &json!([7]))
    );
    assert_eq!(passed["serverTs"], json!(at));
    quiet(&mut a, "op").await;
    // A re-send (a lost ack) is acked again at the same event, never applied twice.
    send(&mut a, json!({ "t": "op", "d": op("a-s-1.x", "purse", "earn", json!([7]), t) })).await;
    assert_eq!(recv_kind(&mut a, "sync-ack").await["serverTs"], json!(at));
    assert_eq!(h.state().purse.coins, 7);
    quiet(&mut b, "op").await;
    // Refusals name the op: the method's own, a cell it doesn't have, a clock far ahead, too old.
    send(&mut a, json!({ "t": "op", "d": op("a-s-2.x", "settings", "theme", json!([""]), t) })).await;
    assert_eq!(
        recv_kind(&mut a, "op-rejected").await,
        json!({ "opId": "a-s-2.x", "cell": "settings", "reason": "no theme" })
    );
    send(&mut a, json!({ "t": "op", "d": op("a-s-3.x", "nope", "theme", json!([]), t) })).await;
    assert!(recv_kind(&mut a, "op-rejected").await["reason"].as_str().unwrap().starts_with("unknown cell \"nope\""));
    send(&mut a, json!({ "t": "op", "d": op("a-s-4.x", "purse", "earn", json!([1]), t + 120_000) })).await;
    let r = recv_kind(&mut a, "op-rejected").await;
    assert!(r["reason"].as_str().unwrap().starts_with("clock drift") && r["serverNow"].is_number(), "{r}");
    send(&mut a, json!({ "t": "op", "d": op("a-s-5.x", "purse", "earn", json!([1]), t - 40 * 86_400_000) })).await;
    assert!(recv_kind(&mut a, "op-rejected").await["reason"].as_str().unwrap().starts_with("stale-beyond-retention"));
    send(
        &mut a,
        json!({ "t": "op", "d": { "id": "a-s-6.x", "cell": "purse", "action": "__set", "hlc": [t, 0, "a"] } }),
    )
    .await;
    assert!(recv_kind(&mut a, "op-rejected").await["reason"].as_str().unwrap().starts_with("malformed"));
    assert_eq!(h.state().purse.coins, 7);
    // Back from offline: its queue is taken in order (the one already in acked again), then where each cell is.
    let queue = json!([
        op("a-s-1.x", "purse", "earn", json!([7]), t),
        op("a-s-7.x", "purse", "earn", json!([1]), t + 1),
        op("a-s-8.x", "settings", "theme", json!(["dark"]), t + 2),
    ]);
    let cells = json!({ "purse": { "lastHlc": [t, 0, "server"], "lastServerTs": at }, "settings": { "lastHlc": null }, "elsewhere": { "lastHlc": null } });
    send(&mut a, req(queue, cells)).await;
    let acks: Vec<Value> =
        [recv_kind(&mut a, "sync-ack").await, recv_kind(&mut a, "sync-ack").await, recv_kind(&mut a, "sync-ack").await]
            .into();
    assert_eq!(
        acks.iter().map(|a| a["opId"].clone()).collect::<Vec<_>>(),
        [json!("a-s-1.x"), json!("a-s-7.x"), json!("a-s-8.x")]
    );
    let res = recv_kind(&mut a, "sync-res").await;
    let seq = h.snapshot().0;
    assert_eq!(res["mode"], "snapshot");
    assert_eq!(
        res["snapshot"],
        json!({ "purse": { "coins": 8 }, "settings": { "theme": "dark" } }),
        "only cells it has"
    );
    assert_eq!(res["lastServerTs"], json!({ "purse": seq, "settings": seq }));
    assert!(acks.iter().all(|a| a["serverTs"].as_u64().unwrap() <= seq), "each ack is in the snapshot");
    assert_eq!(recv_kind(&mut b, "op").await["id"], "a-s-7.x");
    // A cursor this cell never gave is reset; a malformed ask is answered, not dropped.
    send(&mut a, req(json!([]), json!({ "purse": { "lastHlc": null, "lastServerTs": seq + 100 } }))).await;
    assert_eq!(recv_kind(&mut a, "sync-res").await["reset"], json!(["purse"]));
    send(&mut a, json!({ "t": "sync-req", "d": { "cells": {} } })).await;
    assert!(recv_kind(&mut a, "sync-err").await["reason"].as_str().unwrap().contains("clientId"));
    // Paused by time travel: held (`sync-err`), never refused; taken once it resumes.
    h.pause(true);
    send(&mut a, json!({ "t": "op", "d": op("a-s-9.x", "purse", "earn", json!([1]), now()) })).await;
    assert!(recv_kind(&mut a, "sync-err").await["reason"].as_str().unwrap().contains("held"));
    h.pause(false);
    send(&mut a, req(json!([op("a-s-9.x", "purse", "earn", json!([1]), now())]), json!({}))).await;
    assert_eq!(recv_kind(&mut a, "sync-ack").await["opId"], "a-s-9.x");
    assert_eq!(h.state().purse.coins, 9);
}

/// aio's own sync client (its `sync-engine.ts`, two pages, online, offline
/// and refused) against this app: run when `AIO_DENO_SRC` names a copy of
/// aio's `src` and `deno` is on PATH, else it says it skipped.
#[tokio::test(flavor = "multi_thread")]
async fn aios_own_sync_client_converges_with_an_aio_rs_app() {
    let Some(src) = std::env::var_os("AIO_DENO_SRC").map(std::path::PathBuf::from) else {
        eprintln!("skipped: set AIO_DENO_SRC to a copy of aio's src (and have deno) to run aio's own client");
        return;
    };
    let (h, addr) = server().await;
    h.earn(1).unwrap();
    h.theme("light".into()).unwrap();
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/aio_client/sync.ts");
    let mut cmd = std::process::Command::new("deno");
    cmd.args(["run", "--quiet", "--no-lock", "--allow-net=127.0.0.1"])
        .arg(format!("--allow-read={}", src.display()))
        .arg(format!("--config={}", src.join("../deno.json").display()))
        .arg(script)
        .arg(&src)
        .arg(&addr);
    let out = tokio::task::spawn_blocking(move || cmd.output()).await.unwrap().expect("deno runs");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "aio's client failed: {err}");
    let got: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {err}"));
    eprintln!("aio client: {got}");
    let server = json!({ "purse": { "coins": 8 }, "settings": { "theme": "dark" } });
    assert_eq!((h.state().purse.coins, h.state().settings.theme.as_str()), (8, "dark"));
    assert_eq!((&got["start"], &got["optimistic"], &got["queued"]), (&json!(1), &json!(6), &json!(2)), "{got}");
    assert_eq!(got["a"]["confirmed"], server, "{got}");
    assert_eq!(got["a"]["shown"], server, "the refused change is taken back: {got}");
    assert_eq!(got["a"]["rejected"], 1);
    assert_eq!(got["b"]["confirmed"], server, "{got}");
    assert!(got["b"]["ops"].as_u64().unwrap() >= 3, "b got a's ops as ops: {got}");
}

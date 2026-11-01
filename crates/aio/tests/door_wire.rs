//! The local door in every build, `net` or not (`aio::door`): its frames
//! spoken by hand, as any client may.
use aio::prelude::*;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[cell]
pub struct Count {
    pub n: u32,
}

#[methods]
impl Count {
    pub fn bump(&mut self) -> Result<u32> {
        self.n += 1;
        Ok(self.n)
    }
}

async fn put(s: &mut aio::ipc::Stream, kind: u8, body: &[u8]) {
    s.write_u32(1 + body.len() as u32).await.unwrap();
    s.write_u8(kind).await.unwrap();
    s.write_all(body).await.unwrap();
}

async fn get(s: &mut aio::ipc::Stream) -> (u8, Vec<u8>) {
    let n = s.read_u32().await.unwrap() as usize;
    let mut b = vec![0; n];
    s.read_exact(&mut b).await.unwrap();
    (b[0], b[1..].to_vec())
}

async fn say(s: &mut aio::ipc::Stream, v: Value) {
    put(s, 0, v.to_string().as_bytes()).await;
}

async fn heard(s: &mut aio::ipc::Stream) -> Value {
    let (kind, b) = get(s).await;
    assert_eq!(kind, 0, "a JSON message");
    serde_json::from_slice(&b).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn tools_call_and_watch_through_the_door() {
    let d = tempfile::tempdir().unwrap();
    let addr = aio::ipc::addr(d.path(), "cell-count");
    let h = Handle::<Count>::memory();
    tokio::spawn(aio::door::serve(h.clone(), aio::ipc::Listener::bind(&addr).unwrap(), Default::default()));

    let mut s = aio::ipc::connect(&addr).await.unwrap();
    s.write_u8(aio::door::VERSION).await.unwrap();
    say(&mut s, json!({ "t": "hello", "cell": "count", "tool": true, "user": "ops" })).await;
    let first = heard(&mut s).await;
    assert_eq!((first["t"].as_str(), first["state"]["n"].as_u64()), (Some("state"), Some(0)));

    // A call: its change first, then the answer.
    say(&mut s, json!({ "t": "call", "method": "bump", "args": [] })).await;
    assert_eq!(heard(&mut s).await["t"], "patch");
    assert_eq!(heard(&mut s).await["ok"], 1);
    assert_eq!(h.state().n, 1);

    // Operator asks, and a ping is answered with its pong.
    say(&mut s, json!({ "t": "travel", "cmd": "where" })).await;
    assert_eq!(heard(&mut s).await["head"], 1);
    put(&mut s, 1, b"hi").await;
    assert_eq!(get(&mut s).await, (2, b"hi".to_vec()));

    // Server fns need `net`'s servers; without them a call is told so.
    say(&mut s, json!({ "t": "sfn", "id": 7, "name": "x.y", "args": [] })).await;
    let r = heard(&mut s).await;
    assert_eq!((r["t"].as_str(), r["id"].as_u64()), (Some("sfnr"), Some(7)));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_door_refuses_a_wire_it_does_not_speak() {
    let d = tempfile::tempdir().unwrap();
    let addr = aio::ipc::addr(d.path(), "cell-count");
    let l = aio::ipc::Listener::bind(&addr).unwrap();
    tokio::spawn(aio::door::serve(Handle::<Count>::memory(), l, Default::default()));
    let mut s = aio::ipc::connect(&addr).await.unwrap();
    s.write_u8(aio::door::VERSION + 1).await.unwrap();
    let (kind, b) = get(&mut s).await;
    assert_eq!((kind, u16::from_be_bytes([b[0], b[1]])), (3, 1002));
    assert!(String::from_utf8_lossy(&b[2..]).contains("speaks v1"), "{}", String::from_utf8_lossy(&b));
}

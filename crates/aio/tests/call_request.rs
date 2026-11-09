//! `aio::before_call` reads the state the call runs on and, for a network
//! caller, where it came from (aio's `beforeReduce(action, state)` and its
//! request). A process has one gate, so this file holds one test.
use aio::prelude::*;
use std::sync::Mutex;

#[cell(shared)]
pub struct Seats {
    pub taken: u32,
}

#[methods]
impl Seats {
    #[client]
    pub fn take(&mut self) {
        self.taken += 1;
    }
}

static SEEN: Mutex<Vec<String>> = Mutex::new(vec![]);

#[tokio::test(flavor = "multi_thread")]
async fn the_gate_reads_the_state_and_where_a_network_call_came_from() {
    aio::before_call(|c| {
        let st = c.state::<Seats>().expect("the call's own cell");
        let from = c.request.map(|r| {
            let shown = ["host", "authorization", "cookie"].map(|h| r.header(h).is_some());
            format!("{:?} {shown:?}", r.ip)
        });
        SEEN.lock().unwrap().push(format!("{} {from:?}", st.taken));
        ensure!(st.taken < 2, "sold out");
        Ok(())
    });
    let h = Handle::<Seats>::memory();
    h.take().unwrap();
    let s = aio::testing::TestServer::start(h.clone(), aio::http::Http::default()).await.unwrap();
    let ana = s.client("ana").await.unwrap();
    ana.take().unwrap();
    s.converged(&[&ana]).await;
    let refused = tokio::task::spawn_blocking(move || ana.call_confirmed("take", serde_json::json!([]))).await.unwrap();
    assert_eq!(refused.unwrap_err().why, "sold out", "decided on the state it runs on");
    assert_eq!(h.state().taken, 2);
    let seen = SEEN.lock().unwrap().clone();
    assert_eq!(seen[0], "0 None", "the app's own call: no request");
    // Over the network: its address and headers, never its credentials
    // (the link showed its key as `Authorization: Bearer`).
    let net = "Some(\"Some(127.0.0.1) [true, false, false]\")";
    assert_eq!(seen[1..], [format!("1 {net}"), format!("2 {net}")]);
}

// Its own test binary: the declared budgets and their breaches are global.
use aio::budgets::{Budgets, declare};
use aio::prelude::*;
use aio::sync::{Remote, serve};
use std::time::Duration;

#[cell(persist, shared)]
pub struct Feed {
    pub items: Vec<String>,
}

#[methods]
impl Feed {
    #[client]
    pub fn post(&mut self, text: String) {
        self.items.push(text);
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
async fn a_declared_budget_reaches_health_and_degrades_it() {
    let d = tempfile::tempdir().unwrap();
    let src = aio::Source::<Feed>::open(&d.path().join("state.db")).unwrap();
    let h = match &src {
        aio::Source::Local(h) => h.clone(),
        _ => unreachable!(),
    };
    assert_eq!(aio::health::now("feed")["budgets"], serde_json::Value::Null, "none declared: no verdict");
    declare(Budgets { state: Some(2048), payload: Some(1024), broadcast_rate: None });
    h.post("small".into()).unwrap();
    let ok = aio::health::now("feed");
    assert_eq!((ok["status"].as_str(), ok["budgets"]["ok"].as_bool()), (Some("healthy"), Some(true)));

    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(serve(h.clone(), l, "secret".into()));
    let copy = Remote::<Feed>::connect(&addr, "secret", "ana").await.unwrap();
    h.post("x".repeat(1500)).unwrap();
    until("the copy follows", || copy.state().items.len() == 2).await;
    let r = aio::health::now("feed");
    assert_eq!(r["budgets"]["breaches"][0]["budget"], "payload", "the 1.5 KB push: {r}");

    h.post("y".repeat(1000)).unwrap();
    let r = aio::health::now("feed");
    assert_eq!(r["status"], "degraded");
    let state = r["budgets"]["breaches"].as_array().unwrap().iter().find(|b| b["budget"] == "state").cloned().unwrap();
    assert_eq!((state["limit"].as_f64(), state["detail"].as_str()), (Some(2048.0), Some("cell feed")));
    let why = aio::health::judge(&r, r["updatedAt"].as_i64().unwrap(), true).unwrap_err();
    assert!(why.contains("cell feed over [budgets] state:") && why.contains("> 2.0 KB"), "{why}");
}

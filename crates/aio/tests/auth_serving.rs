//! `aio::auth::serving` (aio's `serverAuth()`): a server fn manages the
//! accounts its server signs in with. A process serves one, so this file
//! holds one test.
use aio::auth::{Auth, Policy};
use aio::fns::{Access, Req, ServerFn};
use aio::http::Http;
use aio::prelude::*;
use aio::sync::{Options, Remote, serve_with};
use std::sync::Arc;
use std::time::Duration;

#[cell(shared)]
pub struct Board {
    pub n: u32,
}

#[methods]
impl Board {
    pub fn bump(&mut self) {
        self.n += 1;
    }
}

/// An admin screen's "add an account".
const INVITE: ServerFn<(String,), Vec<String>> = ServerFn::new("admin.invite");
const PW: &str = "correct horse battery";

#[tokio::test(flavor = "multi_thread")]
async fn a_server_fn_manages_the_accounts_its_server_serves() {
    assert!(aio::auth::serving().is_none(), "no server: no accounts");
    let auth = Arc::new(Auth::memory(Policy::default()).unwrap());
    auth.create("ann", PW, Some("admin")).unwrap();
    let http = Http::default().server_fn(INVITE, Access::Role("admin"), |_: Req, (id,): (String,)| async move {
        let a = aio::auth::serving().ok_or_else(|| Rejected::new("no accounts"))?;
        let said = |e: aio::auth::Refusal| Rejected::new(e.to_string());
        a.create(&id, PW, None).map_err(said)?;
        let mut ids: Vec<String> = a.users().map_err(said)?.into_iter().map(|u| u.id).collect();
        ids.sort();
        Ok(ids)
    });
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let opts = Options { token: "secret".into(), http, auth: Some(auth.clone()), ..Default::default() };
    let server = tokio::spawn(serve_with(Handle::<Board>::memory(), l, opts));
    for _ in 0..500 {
        if aio::auth::serving().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(Arc::ptr_eq(&aio::auth::serving().unwrap(), &auth), "the very store the server signs in with");
    let ann = Remote::<Board>::connect(&addr, &auth.issue("ann", "user").unwrap(), "ann").await.unwrap();
    assert_eq!(ann.run(INVITE, ("bob".into(),)).await, Ok(vec!["ann".to_string(), "bob".to_string()]));
    assert_eq!(auth.user("bob").unwrap().map(|u| u.id), Some("bob".into()));
    // Stopped, it is let go of: no fn reaches a store nobody serves.
    drop(ann);
    server.abort();
    drop(auth);
    for _ in 0..500 {
        if aio::auth::serving().is_none() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the stopped server's accounts are still served");
}

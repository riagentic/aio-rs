//! `[capabilities] network` held on aio's own outbound requests, in a process
//! of its own (the list is the process's).
use aio::prelude::*;
use std::io::{Read, Write};

#[cell(shared)]
pub struct Note {
    pub text: String,
}

#[methods]
impl Note {
    pub fn set(&mut self, text: String) -> Result {
        self.text = text;
        Ok(())
    }
}

/// A loopback HTTP server that answers every request with a redirect to `to`.
fn redirecting(to: &'static str) -> String {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        for mut s in l.incoming().flatten() {
            let mut buf = [0; 4096];
            _ = s.read(&mut buf);
            let r = format!("HTTP/1.1 302 Found\r\nLocation: {to}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            _ = s.write_all(r.as_bytes());
        }
    });
    addr
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_outside_the_list_is_refused_before_a_byte_is_sent() {
    aio::capabilities::set(&["api.example.org".into()]);
    // Sync to a server the list leaves out: refused at once, never dialled.
    let e = aio::sync::Remote::<Note>::connect("198.51.100.7:8443", "k", "u").await.err().unwrap();
    assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied);
    assert!(e.to_string().starts_with("sync: 198.51.100.7 is not in this app's [capabilities] network"), "{e}");
    // HTTPS (updates, feedback, OIDC): the first host, and every redirect.
    let agent = || aio::capabilities::agent("updates", ureq::Agent::config_builder().build());
    let e = agent().get("https://dl.example.org/x.json").call().unwrap_err().to_string();
    assert!(e.contains("updates: dl.example.org is not in") && e.contains("add \"dl.example.org\""), "{e}");
    let hop = redirecting("http://dl.example.org/x.json");
    let e = agent().get(&format!("http://{hop}/x.json")).call().unwrap_err().to_string();
    assert!(e.contains("dl.example.org is not in"), "a redirect is held too: {e}");
    // This machine is always reachable.
    let r = agent().get(&format!("http://{}/", redirecting("/"))).call();
    assert!(!r.is_err_and(|e| e.to_string().contains("[capabilities]")), "loopback is never refused");
}

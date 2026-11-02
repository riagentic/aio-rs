//! Every cell of an app on one socket (aio serves them all on one `/ws`):
//! the copies of several cells share one connection, each cell keeping its
//! own rules, and an app's whole set of cells runs on one `TestServer`.
use aio::prelude::*;
use aio::sync::Remote;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

#[cell(shared)]
pub struct Alpha {
    pub n: u64,
}

#[methods]
impl Alpha {
    #[client]
    pub fn bump(&mut self) -> Result<u64> {
        self.n += 1;
        Ok(self.n)
    }
}

#[cell(shared)]
pub struct Beta {
    pub n: u64,
}

#[methods]
impl Beta {
    #[client]
    pub fn bump(&mut self) -> Result<u64> {
        self.n += 1;
        Ok(self.n)
    }
}

#[cell(shared)]
pub struct Gamma {
    pub n: u64,
}

#[methods]
impl Gamma {
    #[client]
    pub fn bump(&mut self) -> Result<u64> {
        self.n += 1;
        Ok(self.n)
    }
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

/// A relay to `to` that counts the TCP connections made through it.
async fn counted(to: String) -> (String, Arc<AtomicUsize>) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (addr, n) = (l.local_addr().unwrap().to_string(), Arc::new(AtomicUsize::new(0)));
    let count = n.clone();
    tokio::spawn(async move {
        while let Ok((mut a, _)) = l.accept().await {
            count.fetch_add(1, Ordering::SeqCst);
            let to = to.clone();
            tokio::spawn(async move {
                if let Ok(mut b) = tokio::net::TcpStream::connect(&to).await {
                    _ = tokio::io::copy_bidirectional(&mut a, &mut b).await;
                }
            });
        }
    });
    (addr, n)
}

/// What a client of three cells costs: its connections, and how long a
/// server change takes to show in its copy (p50, p99 of 300). Run with
/// `--ignored --nocapture`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement"]
async fn measure_three_cells_connections_and_patch_latency() {
    for pace in [50, 0] {
        measure(pace).await;
    }
}

async fn measure(sync_interval_ms: u64) {
    let (a, b, c) = (Handle::<Alpha>::memory(), Handle::<Beta>::memory(), Handle::<Gamma>::memory());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let http = aio::http::Http::default().cell(b.clone()).cell(c.clone());
    let limits = aio::sync::Limits { sync_interval_ms, ..Default::default() };
    let opts = aio::sync::Options { token: "secret".into(), http, limits, ..Default::default() };
    tokio::spawn(aio::sync::serve_with(a.clone(), l, opts));
    let (relay, conns) = counted(addr).await;
    let ra = Remote::<Alpha>::connect(&relay, "secret", "ana").await.unwrap();
    let rb = Remote::<Beta>::connect(&relay, "secret", "ana").await.unwrap();
    let rc = Remote::<Gamma>::connect(&relay, "secret", "ana").await.unwrap();
    let mut took = Vec::new();
    for i in 1..=300u64 {
        let t = Instant::now();
        match i % 3 {
            0 => _ = a.bump().unwrap(),
            1 => _ = b.bump().unwrap(),
            _ => _ = c.bump().unwrap(),
        }
        let want = i.div_ceil(3);
        loop {
            let got = match i % 3 {
                0 => ra.state().n,
                1 => rb.state().n,
                _ => rc.state().n,
            };
            if got == want {
                break;
            }
            tokio::task::yield_now().await;
        }
        took.push(t.elapsed());
    }
    took.sort();
    let us = |d: Duration| d.as_micros();
    println!(
        "three cells, sync_interval_ms {sync_interval_ms}: {} connections; patch latency p50 {} us, p99 {} us",
        conns.load(Ordering::SeqCst),
        us(took[took.len() / 2]),
        us(took[took.len() * 99 / 100])
    );
}

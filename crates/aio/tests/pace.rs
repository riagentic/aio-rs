//! Render-staleness backpressure (aio's), measured on a real server: a
//! copy whose screen lags is sent fewer, folded patches. A process of its
//! own: its copies live for seconds, and what a copy says of its health
//! (the process's) would reach other tests' servers.
//!
//! What is asserted is read off the protocol, not the wall clock: each
//! patch names the events it folds (`prev`..`seq`), so how many a patch
//! folds measures the window the server paced the link with, whatever
//! the machine's load does to the rate of changes.
use aio::prelude::*;
use aio::sync::{Link, Remote, serve};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cell(shared)]
pub struct Ticker {
    pub price: String,
    /// A catalog beside the price: what each patch's view is made of.
    pub rows: Vec<String>,
}

#[methods]
impl Ticker {
    #[client]
    pub fn set(&mut self, price: String) -> Result {
        self.price = price;
        Ok(())
    }
}

/// How a copy's screen goes.
#[derive(Clone, Copy)]
enum Screen {
    /// Shows each state as it comes.
    KeepsUp,
    /// Rendered once, then frozen: its lag only grows.
    Frozen,
    /// Nothing renders it: it never says (as before backpressure).
    Silent,
}

/// What one copy took in 4 s of a steady stream of price changes beside a
/// 5000-row catalog, once its lag was said: the events each patch folded,
/// their bytes, and the process's CPU ms.
struct Took {
    folds: Vec<u64>,
    bytes: usize,
    cpu: u64,
}

impl Took {
    fn median(&self) -> u64 {
        let mut f = self.folds.clone();
        f.sort_unstable();
        f[f.len() / 2]
    }

    fn events(&self) -> u64 {
        self.folds.iter().sum()
    }
}

async fn stream(screen: Screen) -> Took {
    let h = Handle::<Ticker>::memory();
    h.store(|s| s.load(&By::System, serde_json::json!({ "price": "", "rows": vec!["a catalog row"; 5000] }))).unwrap();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let link = Link { addr: l.local_addr().unwrap().to_string(), token: "secret".into(), root: None };
    tokio::spawn(serve(h.clone(), l, "secret".into()));
    let r = Remote::<Ticker>::connect_to(link, "cy").await.unwrap();
    let took = Arc::new(Mutex::new((Vec::new(), 0usize, false)));
    let t = took.clone();
    r.on_patch(move |p| {
        let mut t = t.lock().unwrap();
        if t.2 && p["t"] == "patch" {
            t.0.push(p["seq"].as_u64().unwrap() - p["prev"].as_u64().unwrap());
            t.1 += p.to_string().len();
        }
    });
    match screen {
        // Shown at once, in the copy's own thread: no lag, whatever the load.
        Screen::KeepsUp => {
            let s = r.clone();
            r.listen(move |_| s.rendered());
        }
        Screen::Frozen => r.rendered(),
        Screen::Silent => {}
    }
    let ticker = {
        let h = h.clone();
        tokio::spawn(async move {
            for i in 0..u32::MAX {
                h.set(format!("price {i}")).unwrap();
                tokio::time::sleep(Duration::from_millis(3)).await;
            }
        })
    };
    // Its lag said (each second) before it is measured.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    took.lock().unwrap().2 = true;
    let cpu = aio::health::cpu_ms().unwrap_or(0);
    tokio::time::sleep(Duration::from_secs(4)).await;
    took.lock().unwrap().2 = false;
    ticker.abort();
    // Ended, not just asked to: a tick on another thread could land after "last".
    _ = ticker.await;
    let cpu = aio::health::cpu_ms().unwrap_or(0) - cpu;
    // The newest state always comes, however far apart.
    h.set("last".into()).unwrap();
    for _ in 0..500 {
        if r.state().price == "last" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(r.state().price, "last");
    let (folds, bytes, _) = std::mem::take(&mut *took.lock().unwrap());
    assert!(folds.len() >= 3, "patches came: {folds:?}");
    Took { folds, bytes, cpu }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_screen_that_lags_is_sent_fewer_folded_patches() {
    let up = stream(Screen::KeepsUp).await;
    let frozen = stream(Screen::Frozen).await;
    let silent = stream(Screen::Silent).await;
    let per_event = |t: &Took| t.bytes as f64 / t.events() as f64;
    eprintln!(
        "a change each ~3 ms, 4 s: keeps up {} patches folding {} events (median) {} B; frozen {} patches folding {} \
         {} B {} ms cpu; never says {} patches folding {} {} B {} ms cpu",
        up.folds.len(),
        up.median(),
        up.bytes,
        frozen.folds.len(),
        frozen.median(),
        frozen.bytes,
        frozen.cpu,
        silent.folds.len(),
        silent.median(),
        silent.bytes,
        silent.cpu,
    );
    // A screen that keeps up is paced as before (50 ms windows) ...
    assert!(up.median() * 2 <= silent.median() * 3, "keeps up: {} vs {}", up.median(), silent.median());
    // ... a frozen one 4x: each patch folds about four windows' events.
    assert!(frozen.median() * 2 >= silent.median() * 5, "frozen: {} vs {}", frozen.median(), silent.median());
    // And costs fewer bytes for the same changes.
    assert!(
        per_event(&frozen) * 2.0 <= per_event(&silent),
        "{} vs {} B an event",
        per_event(&frozen),
        per_event(&silent)
    );
}

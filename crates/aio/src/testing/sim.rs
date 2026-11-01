//! Simulation tests: one real server, real synced clients, each over a
//! line of its own that holds every chunk a seeded while and that a script
//! cuts (every connection closed, new ones refused) and heals. The script
//! is drawn from a seed: the same seed is the same script, whatever the
//! timing, and a failure names the seed and the script. Nothing is faked:
//! the copies, the wire and the server are the app's own.
use super::{RandomCall, Rng, TestServer, dump};
use crate::sync::Remote;
use crate::{Cell, Door, Handle};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::watch;

/// How long the end of a run waits for every copy to settle: a cut can
/// leave a copy in its reconnect backoff for some seconds.
const SETTLE: Duration = Duration::from_secs(30);

/// The most a line holds one chunk.
const LATENCY_MS: u64 = 8;

/// One client's line to the server: up or cut.
struct Line {
    addr: String,
    up: watch::Sender<bool>,
}

/// A relay to `to` whose chunks are held `0..=LATENCY_MS` ms (from `seed`),
/// in order, both ways; cut, it closes every connection and refuses new ones.
async fn line(to: String, seed: u64) -> std::io::Result<Line> {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let (up, rx) = watch::channel(true);
    let addr = l.local_addr()?.to_string();
    tokio::spawn(async move {
        let mut n = 0;
        while let Ok((a, _)) = l.accept().await {
            if !*rx.borrow() {
                continue;
            }
            let Ok(b) = tokio::net::TcpStream::connect(&to).await else { continue };
            n += 2;
            let ((ar, aw), (br, bw)) = (a.into_split(), b.into_split());
            tokio::spawn(pipe(ar, bw, rx.clone(), Rng(seed ^ n)));
            tokio::spawn(pipe(br, aw, rx.clone(), Rng(seed ^ (n + 1))));
        }
    });
    Ok(Line { addr, up })
}

/// Pass `r` on to `w`, each chunk held a while, until either ends or the line is cut.
async fn pipe(
    mut r: tokio::net::tcp::OwnedReadHalf,
    mut w: tokio::net::tcp::OwnedWriteHalf,
    mut up: watch::Receiver<bool>,
    mut rng: Rng,
) {
    let mut buf = vec![0; 16 << 10];
    loop {
        let n = tokio::select! {
            n = r.read(&mut buf) => match n {
                Ok(n @ 1..) => n,
                _ => return,
            },
            _ = up.wait_for(|u| !*u) => return,
        };
        tokio::time::sleep(Duration::from_millis(rng.below(LATENCY_MS + 1))).await;
        if !*up.borrow() || w.write_all(&buf[..n]).await.is_err() {
            return;
        }
    }
}

/// A server of `T`, `clients` copies of it and the seed of their script.
pub struct Sim<T: Cell> {
    pub server: TestServer<T>,
    clients: Vec<Remote<T>>,
    lines: Vec<Line>,
    seed: u64,
}

impl<T: Cell> Sim<T> {
    /// Serve `h` and link `clients` copies to it (as `c0`, `c1`…), each
    /// over its own line. Needs a multi-threaded tokio runtime.
    pub async fn start(h: Handle<T>, clients: usize, seed: u64) -> std::io::Result<Self> {
        let server = TestServer::start(h, crate::http::Http::default()).await?;
        let (mut lines, mut copies) = (vec![], vec![]);
        for i in 0..clients {
            let l = line(server.addr.clone(), seed.rotate_left(17) ^ i as u64).await?;
            copies.push(Remote::connect(&l.addr, &server.token, &format!("c{i}")).await?);
            lines.push(l);
        }
        Ok(Self { server, clients: copies, lines, seed })
    }

    pub fn clients(&self) -> &[Remote<T>] {
        &self.clients
    }

    /// `steps` moves of the seed's script: a client calls (a pick from
    /// `calls`, predicted at once, sent when its line is up), the server's
    /// own code calls, a line is cut or healed, or time passes. Then every
    /// line is healed and the run settles: each copy online, nothing
    /// waiting, showing the server's state, which `holds`. A failure panics
    /// naming the seed and the script; the script is returned.
    pub async fn run(&mut self, steps: usize, calls: &[RandomCall<'_>], holds: impl Fn(&T) -> bool) -> Vec<String> {
        assert!(!calls.is_empty() && !self.clients.is_empty(), "sim: no calls or no clients");
        let (seed, mut rng, mut script) = (self.seed, Rng(self.seed), vec![]);
        for _ in 0..steps {
            let c = rng.below(self.clients.len() as u64) as usize;
            match rng.below(20) {
                0..12 => {
                    let (method, args) = &calls[rng.below(calls.len() as u64) as usize];
                    let args = args(&mut rng);
                    script.push(format!("c{c}.{method}({})", dump(&args, 60)));
                    _ = Door::call(&self.clients[c], method, args);
                }
                12..14 => {
                    let (method, args) = &calls[rng.below(calls.len() as u64) as usize];
                    let args = args(&mut rng);
                    script.push(format!("server.{method}({})", dump(&args, 60)));
                    _ = Door::call(&self.server.handle, method, args);
                }
                14..16 => {
                    script.push(format!("cut c{c}"));
                    self.lines[c].up.send_replace(false);
                }
                16..18 => {
                    script.push(format!("heal c{c}"));
                    self.lines[c].up.send_replace(true);
                }
                _ => {
                    let ms = rng.below(20);
                    script.push(format!("wait {ms}ms"));
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                }
            }
            let fail = |why: String| -> ! { panic!("sim seed {seed}: {why}\n  script: {}", script.join(", ")) };
            if !holds(&self.server.state()) {
                fail(format!("the server's state no longer holds: {}", dump(&*self.server.state(), 2000)));
            }
        }
        self.lines.iter().for_each(|l| _ = l.up.send_replace(true));
        script.push("heal all".into());
        self.settled(&script).await;
        if !holds(&self.server.state()) {
            panic!("sim seed {seed}: the settled state no longer holds\n  script: {}", script.join(", "));
        }
        script
    }

    /// Wait until every copy is online, has nothing waiting and shows the
    /// server's state; after [`SETTLE`], panics naming the ones that lag.
    async fn settled(&self, script: &[String]) {
        let end = Instant::now() + SETTLE;
        loop {
            let want = self.server.state();
            let lag: Vec<String> = (self.clients.iter().enumerate())
                .filter(|(_, c)| !c.online() || c.pending() > 0 || *c.state() != *want)
                .map(|(i, c)| {
                    format!("c{i} (online {}, {} waiting): {}", c.online(), c.pending(), dump(&*c.state(), 300))
                })
                .collect();
            if lag.is_empty() {
                return;
            }
            assert!(
                Instant::now() < end,
                "sim seed {}: not settled after {SETTLE:?} on {}: {}\n  script: {}",
                self.seed,
                dump(&*want, 300),
                lag.join("; "),
                script.join(", ")
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

//! The JS client in the desktop window (`crates/aio/client`): a page that
//! may connect nowhere still reaches the app's cells, over the window's own
//! wire to the app's process. [`use_js_client`] in the root view puts a
//! socket in the page (`window.__aioSyncSocket`, which `connect()` takes
//! there); each of its links is served here as a network client's link is
//! (its hello, `#[client]` rules, `visible`, patches, pings), on the
//! window's own copy of the cell: the app's store, or its server's (a
//! `client` app).
use crate::link::{Gate, Hooks, Msg, Sock};
use crate::mux::{Frame, decode, encode};
use crate::ui::prelude::*;
use crate::{Cell, Handle, Source};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

type Serve = Arc<dyn Fn(Sock, Value) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// A cell the window's JS client may reach: `Source<T>` or `Handle<T>` `.into()`.
#[derive(Clone)]
pub struct JsCell {
    id: &'static str,
    serve: Serve,
}

impl<T: Cell> From<Source<T>> for JsCell {
    fn from(s: Source<T>) -> Self {
        let serve: Serve = match s {
            Source::Local(h) => Arc::new(move |sock, hello| {
                let h = h.clone();
                Box::pin(async move {
                    let (limits, calls) = (Default::default(), crate::link::no_calls());
                    let r = crate::link::session(
                        h,
                        sock,
                        limits,
                        "window".into(),
                        Gate::Key,
                        None,
                        calls,
                        Hooks::default(),
                        Some(hello),
                    );
                    if let Err(e) = r.await {
                        crate::warn!(cat: "sync", "[js client] {}: {e}", T::ID);
                    }
                })
            }),
            #[cfg(feature = "server")]
            Source::Remote(r) => Arc::new(move |sock, hello| {
                let link = r.link_now();
                Box::pin(async move {
                    match link {
                        Some(l) => crate::sync::relay(l, sock, hello).await,
                        None => refuse(sock, "this window has no server yet".into()).await,
                    }
                })
            }),
        };
        Self { id: T::ID, serve }
    }
}

impl<T: Cell> From<Handle<T>> for JsCell {
    fn from(h: Handle<T>) -> Self {
        Source::Local(h).into()
    }
}

async fn refuse(mut sock: Sock, why: String) {
    _ = sock.send(Msg::Text(json!({ "t": "refused", "why": why }).to_string())).await;
}

/// The page's side: a WebSocket-shaped socket per `connect()`, its frames
/// (`aio-mux`'s) carried as `[socket, frame]` (`null`: it closed).
const BRIDGE: &str = "const socks = new Map();
let n = 0;
window.__aioSyncSocket = class {
  constructor() {
    this.id = ++n; this.readyState = 0; this.protocol = 'aio-mux';
    socks.set(this.id, this);
    setTimeout(() => { if (this.readyState === 0) { this.readyState = 1; this.onopen && this.onopen(); } });
  }
  send(t) { if (this.readyState === 1) aio.send([this.id, t]); }
  close() {
    if (this.readyState === 3) return;
    this.readyState = 3; socks.delete(this.id); aio.send([this.id, null]);
    this.onclose && this.onclose();
  }
};
for (;;) {
  const [s, t] = await aio.recv();
  const k = socks.get(s);
  if (k && k.onmessage) k.onmessage({ data: t });
}";

/// A link's frame as the page's mux reads it (pure).
fn framed(chan: u32, m: &Msg) -> String {
    match m {
        Msg::Text(t) => encode(chan, &Frame::Text(t)),
        Msg::Ping(p) => encode(chan, &Frame::Ping(p.clone())),
        Msg::Pong(p) => encode(chan, &Frame::Pong(p.clone())),
        Msg::Close(code, why) => encode(chan, &Frame::Close(*code, why)),
    }
}

/// Serve the window page's JS client (`connect()` in an island) on
/// `cells`, for as long as this view lives: call it once, in the root.
pub fn use_js_client(cells: impl IntoIterator<Item = JsCell>) {
    let cells: HashMap<&'static str, JsCell> = cells.into_iter().map(|c| (c.id, c)).collect();
    let ev = eval(BRIDGE);
    let (out, mut to_page) = unbounded_channel::<(u32, String)>();
    let page = ev.clone();
    spawn(async move {
        while let Some((s, f)) = to_page.recv().await {
            if page.send((s, f)).is_err() {
                return;
            }
        }
    });
    let mut ev = ev;
    spawn(async move {
        // Each open link's way in, by socket and channel.
        let mut links: HashMap<(u32, u32), UnboundedSender<Msg>> = HashMap::new();
        while let Ok((s, f)) = ev.recv::<(u32, Option<String>)>().await {
            let Some(f) = f else {
                links.retain(|(sock, _), _| *sock != s);
                continue;
            };
            let Some((c, frame)) = decode(&f) else { continue };
            let msg = match frame {
                Frame::Text(t) => Msg::Text(t.into()),
                Frame::Pong(p) => Msg::Pong(p),
                Frame::Ping(p) => {
                    _ = out.send((s, encode(c, &Frame::Pong(p))));
                    continue;
                }
                Frame::Close(..) => {
                    links.remove(&(s, c));
                    continue;
                }
            };
            if let Some(tx) = links.get(&(s, c)) {
                _ = tx.send(msg);
                continue;
            }
            // A new link: its first message is its hello, naming the cell.
            let Msg::Text(t) = msg else { continue };
            let hello: Value = serde_json::from_str(&t).unwrap_or_default();
            let (in_tx, in_rx) = unbounded_channel();
            let (link_tx, mut link_rx) = unbounded_channel::<Msg>();
            let sock = Sock::Pipe(in_rx, link_tx);
            match hello["cell"].as_str().and_then(|id| cells.get(id)) {
                Some(cell) => _ = crate::rt().spawn((cell.serve)(sock, hello)),
                None => {
                    let why = format!("this window serves no cell {}", hello["cell"]);
                    _ = crate::rt().spawn(refuse(sock, why));
                }
            }
            links.insert((s, c), in_tx);
            let out = out.clone();
            crate::rt().spawn(async move {
                while let Some(m) = link_rx.recv().await {
                    _ = out.send((s, framed(c, &m)));
                }
                _ = out.send((s, encode(c, &Frame::Close(1000, ""))));
            });
        }
    });
}

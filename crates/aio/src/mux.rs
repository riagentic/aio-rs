//! Every link of a client on one socket, as aio serves every cell on one
//! `/ws` (`server/server-ws.ts`, one socket per page; `server-broadcast.ts`
//! `_clientOps` puts the cell first in each op's path). aio-rs keeps one link
//! per cell (its own hello, user, rules, view and pace) and carries the links
//! of one client as channels of one socket: each text frame is
//! `<chan><kind><rest>`, `chan` a decimal number the client picks (each new
//! one higher), and `kind`
//!
//! - `:` the link's message (`rest`, as a link's frame);
//! - `?` a ping, `.` its pong (`rest`: the payload, hex);
//! - `!` the link closes (`rest`: `<code> <why>`).
//!
//! A client asks for it with `x-aio-mux: 1` (a page: the WebSocket protocol
//! `aio-mux`) beside wire 2; the server says yes with the same header (or
//! protocol). A server that doesn't is an older aio-rs: a socket per cell.

/// The header a client asks with and the server answers with.
#[cfg(feature = "server")]
pub(crate) const HEADER: &str = "x-aio-mux";
/// The WebSocket protocol a page asks with (a browser sets no header).
pub(crate) const PROTOCOL: &str = "aio-mux";
/// Links one socket carries at most.
#[cfg(feature = "server")]
pub(crate) const MAX_CHANS: usize = 64;

/// One frame of a channel.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Frame<'a> {
    Text(&'a str),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    Close(u16, &'a str),
}

/// `f` on channel `chan`, as sent (pure).
pub(crate) fn encode(chan: u32, f: &Frame) -> String {
    use std::fmt::Write;
    let hex = |p: &[u8]| {
        p.iter().fold(String::with_capacity(p.len() * 2), |mut s, b| {
            _ = write!(s, "{b:02x}");
            s
        })
    };
    match f {
        Frame::Text(t) => format!("{chan}:{t}"),
        Frame::Ping(p) => format!("{chan}?{}", hex(p)),
        Frame::Pong(p) => format!("{chan}.{}", hex(p)),
        Frame::Close(code, why) => format!("{chan}!{code} {why}"),
    }
}

/// A frame as sent, its channel and what it says (pure); `None`: not one.
pub(crate) fn decode(t: &str) -> Option<(u32, Frame<'_>)> {
    let at = t.find(|c: char| !c.is_ascii_digit()).filter(|at| *at > 0)?;
    let chan = t[..at].parse().ok()?;
    let rest = &t[at + 1..];
    let unhex = |s: &str| -> Option<Vec<u8>> {
        s.len()
            .is_multiple_of(2)
            .then(|| (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect())?
    };
    let f = match t.as_bytes()[at] {
        b':' => Frame::Text(rest),
        b'?' => Frame::Ping(unhex(rest)?),
        b'.' => Frame::Pong(unhex(rest)?),
        b'!' => {
            let (code, why) = rest.split_once(' ').unwrap_or((rest, ""));
            Frame::Close(code.parse().ok()?, why)
        }
        _ => return None,
    };
    Some((chan, f))
}

#[cfg(feature = "server")]
pub(crate) use net::*;

#[cfg(feature = "server")]
mod net {
    use super::{Frame, MAX_CHANS, decode, encode};
    use crate::link::{Msg, Sock, lock};
    use axum::extract::ws::{Message as AxMsg, WebSocket};
    use futures_util::stream::{SplitSink, SplitStream};
    use futures_util::{FutureExt, Sink, SinkExt, Stream, StreamExt};
    use std::collections::HashMap;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, Weak};
    use std::task::{Context, Poll};
    use std::time::Duration;
    use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
    use tokio_tungstenite::tungstenite::{Message as TMsg, protocol::CloseFrame};

    type WsError = tokio_tungstenite::tungstenite::Error;

    /// A socket with no link for this long is closed.
    const IDLE: Duration = Duration::from_secs(15);
    /// The ended links a socket remembers.
    const ENDED: usize = 1024;

    /// One side's queue of a channel's frames not yet read: never blocks the
    /// socket (one slow link must not hold the others); a link that lets
    /// more than `cap` bytes wait is closed.
    struct Inbox<M> {
        tx: UnboundedSender<(M, usize)>,
        queued: Arc<AtomicUsize>,
        cap: usize,
    }

    struct Outbox<M> {
        rx: UnboundedReceiver<(M, usize)>,
        queued: Arc<AtomicUsize>,
    }

    fn inbox<M>(cap: usize) -> (Inbox<M>, Outbox<M>) {
        let (tx, rx) = unbounded_channel();
        let queued = Arc::new(AtomicUsize::new(0));
        (Inbox { tx, queued: queued.clone(), cap }, Outbox { rx, queued })
    }

    impl<M> Inbox<M> {
        /// Queue `m` (`len` bytes); `false` when the reader is gone or over its cap.
        fn put(&self, m: M, len: usize) -> bool {
            if self.queued.fetch_add(len, Ordering::SeqCst) + len > self.cap {
                return false;
            }
            self.tx.send((m, len)).is_ok()
        }
    }

    impl<M> Outbox<M> {
        fn poll(&mut self, cx: &mut Context<'_>) -> Poll<Option<M>> {
            self.rx.poll_recv(cx).map(|m| {
                m.map(|(m, len)| {
                    self.queued.fetch_sub(len, Ordering::SeqCst);
                    m
                })
            })
        }
    }

    type Out = Arc<tokio::sync::Mutex<SplitSink<WebSocket, AxMsg>>>;

    async fn put(out: &Out, text: String) -> std::io::Result<()> {
        out.lock().await.send(AxMsg::Text(text.into())).await.map_err(std::io::Error::other)
    }

    /// The server's end of one channel: a link's [`Sock`].
    pub(crate) struct Chan {
        id: u32,
        rx: Outbox<Msg>,
        out: Out,
    }

    impl Chan {
        pub(crate) async fn recv(&mut self) -> Option<Msg> {
            std::future::poll_fn(|cx| self.rx.poll(cx)).await
        }

        pub(crate) async fn send(&mut self, m: Msg) -> std::io::Result<()> {
            let f = match &m {
                Msg::Text(t) => Frame::Text(t),
                Msg::Ping(p) => Frame::Ping(p.clone()),
                Msg::Pong(p) => Frame::Pong(p.clone()),
                Msg::Close(code, why) => Frame::Close(*code, why),
            };
            put(&self.out, encode(self.id, &f)).await
        }
    }

    /// What runs one link of a socket (the session its hello asks for).
    pub(crate) type Run =
        Arc<dyn Fn(Sock) -> Pin<Box<dyn Future<Output = std::result::Result<(), String>> + Send>> + Send + Sync>;

    /// Serve the links of socket `ws`, each run by `run`, until it closes or
    /// has had none for `IDLE`. The socket's own slot of `open` (of `max`,
    /// taken by the caller) holds its first link; each one more takes one.
    /// `max_message`: a link's largest message; it may let 8 MB wait unread
    /// (what a socket's buffers would hold), or one such message.
    pub(crate) async fn serve(
        ws: WebSocket,
        run: Run,
        open: Arc<AtomicUsize>,
        max: usize,
        max_message: usize,
    ) -> std::result::Result<(), String> {
        let (sink, mut stream) = ws.split();
        let out: Out = Arc::new(tokio::sync::Mutex::new(sink));
        let mut chans: HashMap<u32, Inbox<Msg>> = HashMap::new();
        let mut tasks = tokio::task::JoinSet::new();
        // Links ended lately: their late frames open nothing (a client
        // numbers its links afresh, but their hellos may come in any order).
        let (mut ended, mut held) = (std::collections::BTreeSet::<u32>::new(), 1usize);
        fn end(ended: &mut std::collections::BTreeSet<u32>, id: u32) {
            ended.insert(id);
            if ended.len() > ENDED {
                ended.pop_first();
            }
        }
        let mut quiet = tokio::time::Instant::now();
        let r = loop {
            let idle = tasks.is_empty().then_some(quiet + IDLE);
            tokio::select! {
                m = stream.next() => {
                    let t = match m {
                        Some(Ok(AxMsg::Text(t))) => t,
                        Some(Ok(AxMsg::Ping(_) | AxMsg::Pong(_))) => continue,
                        Some(Ok(AxMsg::Close(_))) | None => break Ok(()),
                        Some(Ok(AxMsg::Binary(_))) => break Err("a binary frame on a mux socket".into()),
                        Some(Err(e)) => break Err(e.to_string()),
                    };
                    let Some((id, f)) = decode(&t) else { break Err("not a mux frame".to_string()) };
                    let closed = |why: &str| encode(id, &Frame::Close(1008, why));
                    match f {
                        Frame::Text(body) => match chans.get(&id) {
                            Some(inbox) => if !inbox.put(Msg::Text(body.into()), body.len()) {
                                chans.remove(&id);
                                end(&mut ended, id);
                                _ = put(&out, closed("sent more than it reads")).await;
                            },
                            // A link gone: its late frames.
                            None if ended.contains(&id) => {}
                            None => {
                                // Past its first link, each takes a client's slot.
                                let full = match tasks.len() {
                                    n if n >= MAX_CHANS => Some(MAX_CHANS),
                                    n if n < held => None,
                                    _ if open.fetch_add(1, Ordering::SeqCst) < max => {
                                        held += 1;
                                        None
                                    }
                                    _ => {
                                        open.fetch_sub(1, Ordering::SeqCst);
                                        Some(max)
                                    }
                                };
                                if let Some(limit) = full {
                                    end(&mut ended, id);
                                    let why = format!("too many clients (the limit is {limit}): try again later");
                                    let said = serde_json::json!({ "t": "refused", "why": why, "retryAfterMs": 5000 });
                                    _ = put(&out, encode(id, &Frame::Text(&said.to_string()))).await;
                                    _ = put(&out, encode(id, &Frame::Close(1013, &why))).await;
                                    continue;
                                }
                                let (inbox, rx) = inbox(max_message.max(8 << 20));
                                inbox.put(Msg::Text(body.into()), body.len());
                                chans.insert(id, inbox);
                                let link = run(Sock::Chan(Chan { id, rx, out: out.clone() }));
                                tasks.spawn(async move {
                                    let r = std::panic::AssertUnwindSafe(link).catch_unwind().await;
                                    (id, r.unwrap_or_else(|_| Err(format!("link {id} panicked"))))
                                });
                            }
                        },
                        Frame::Ping(p) => if put(&out, encode(id, &Frame::Pong(p))).await.is_err() {
                            break Ok(());
                        },
                        Frame::Pong(p) => if let Some(inbox) = chans.get(&id) {
                            inbox.put(Msg::Pong(p), 0);
                        },
                        // Its session sees the end.
                        Frame::Close(..) => {
                            chans.remove(&id);
                            end(&mut ended, id);
                        }
                    }
                }
                Some(done) = tasks.join_next() => {
                    let Ok((id, r)) = done else { continue };
                    chans.remove(&id);
                    end(&mut ended, id);
                    if tasks.is_empty() {
                        quiet = tokio::time::Instant::now();
                    }
                    while held > tasks.len().max(1) {
                        open.fetch_sub(1, Ordering::SeqCst);
                        held -= 1;
                    }
                    let end = match &r {
                        Ok(()) => Frame::Close(1000, ""),
                        Err(e) => {
                            eprintln!("[aio] sync: {e}");
                            Frame::Close(1011, &e[..e.char_indices().nth(120).map_or(e.len(), |(i, _)| i)])
                        }
                    };
                    _ = put(&out, encode(id, &end)).await;
                }
                () = async { tokio::time::sleep_until(idle.unwrap_or(quiet)).await }, if idle.is_some() => break Ok(()),
            }
        };
        // Each link sees its end; one that doesn't finish soon is dropped.
        drop(chans);
        _ = tokio::time::timeout(Duration::from_secs(1), async { while tasks.join_next().await.is_some() {} }).await;
        open.fetch_sub(held - 1, Ordering::SeqCst);
        r
    }

    /// A client's socket that carries its links to one server.
    struct Shared {
        sink: tokio::sync::Mutex<SplitSink<crate::sync::Wire, TMsg>>,
        chans: Mutex<HashMap<u32, Inbox<TMsg>>>,
        next: AtomicU32,
        alive: AtomicBool,
    }

    impl Shared {
        async fn put(&self, text: String) -> std::result::Result<(), WsError> {
            self.sink.lock().await.send(TMsg::text(text)).await
        }
    }

    /// The socket, while a link uses it.
    struct Conn {
        shared: Arc<Shared>,
        reader: tokio::task::AbortHandle,
    }

    impl Drop for Conn {
        fn drop(&mut self) {
            self.reader.abort();
        }
    }

    /// What a client lets wait unread on one link: a server's largest message.
    const CLIENT_CAP: usize = crate::link::MAX_MESSAGE;

    impl Conn {
        fn start(w: crate::sync::Wire) -> Arc<Self> {
            let (sink, stream) = w.split();
            let shared = Arc::new(Shared {
                sink: tokio::sync::Mutex::new(sink),
                chans: Mutex::default(),
                next: AtomicU32::new(1),
                alive: AtomicBool::new(true),
            });
            let reader = tokio::spawn(read(shared.clone(), stream)).abort_handle();
            Arc::new(Self { shared, reader })
        }

        fn open(self: &Arc<Self>) -> crate::sync::Wire {
            let id = self.shared.next.fetch_add(1, Ordering::SeqCst);
            let (inbox, rx) = inbox(CLIENT_CAP);
            lock(&self.shared.chans).insert(id, inbox);
            let sink = futures_util::sink::unfold(self.shared.clone(), move |s, m: TMsg| async move {
                let f = match &m {
                    TMsg::Text(t) => Frame::Text(t.as_str()),
                    TMsg::Ping(p) => Frame::Ping(p.to_vec()),
                    TMsg::Pong(p) => Frame::Pong(p.to_vec()),
                    TMsg::Close(c) => {
                        lock(&s.chans).remove(&id);
                        c.as_ref().map_or(Frame::Close(1000, ""), |c| Frame::Close(c.code.into(), c.reason.as_str()))
                    }
                    TMsg::Binary(_) | TMsg::Frame(_) => {
                        return Err(WsError::Io(std::io::Error::other("a mux link carries no binary")));
                    }
                };
                s.put(encode(id, &f)).await?;
                Ok(s)
            });
            Box::new(Link { id, rx, sink: Box::pin(sink), conn: self.clone() })
        }
    }

    /// The client's socket's frames to its links, until it closes.
    async fn read(shared: Arc<Shared>, mut stream: SplitStream<crate::sync::Wire>) {
        while let Some(Ok(m)) = stream.next().await {
            let t = match m {
                TMsg::Text(t) => t,
                TMsg::Ping(_) | TMsg::Pong(_) | TMsg::Frame(_) => continue,
                TMsg::Binary(_) | TMsg::Close(_) => break,
            };
            let Some((id, f)) = decode(&t) else { break };
            let said = match f {
                Frame::Text(body) => Some((TMsg::text(body), body.len())),
                Frame::Pong(p) => Some((TMsg::Pong(p.into()), 0)),
                // Pinged: answered while the link is open, else said closed.
                Frame::Ping(p) => {
                    let f = match lock(&shared.chans).contains_key(&id) {
                        true => Frame::Pong(p),
                        false => Frame::Close(1000, ""),
                    };
                    if shared.put(encode(id, &f)).await.is_err() {
                        break;
                    }
                    None
                }
                Frame::Close(code, why) => {
                    if let Some(inbox) = lock(&shared.chans).remove(&id) {
                        inbox.put(TMsg::Close(Some(CloseFrame { code: code.into(), reason: why.into() })), 0);
                    }
                    None
                }
            };
            let Some((m, len)) = said else { continue };
            let over = {
                let mut chans = lock(&shared.chans);
                let over = chans.get(&id).is_some_and(|inbox| !inbox.put(m, len));
                if over {
                    chans.remove(&id);
                }
                over
            };
            if over {
                eprintln!("[aio] sync: a link let more than {CLIENT_CAP} bytes wait unread; it is closed");
                _ = shared.put(encode(id, &Frame::Close(1008, "not read"))).await;
            }
        }
        shared.alive.store(false, Ordering::SeqCst);
        lock(&shared.chans).clear();
    }

    /// One link of a client's socket, as a socket of its own.
    struct Link {
        id: u32,
        rx: Outbox<TMsg>,
        sink: Pin<Box<dyn Sink<TMsg, Error = WsError> + Send>>,
        conn: Arc<Conn>,
    }

    impl Stream for Link {
        type Item = std::result::Result<TMsg, WsError>;

        fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            self.get_mut().rx.poll(cx).map(|m| m.map(Ok))
        }
    }

    impl Sink<TMsg> for Link {
        type Error = WsError;

        fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), WsError>> {
            self.get_mut().sink.as_mut().poll_ready(cx)
        }

        fn start_send(self: Pin<&mut Self>, m: TMsg) -> Result<(), WsError> {
            self.get_mut().sink.as_mut().start_send(m)
        }

        fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), WsError>> {
            self.get_mut().sink.as_mut().poll_flush(cx)
        }

        fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), WsError>> {
            self.get_mut().sink.as_mut().poll_close(cx)
        }
    }

    impl Drop for Link {
        fn drop(&mut self) {
            let open = lock(&self.conn.shared.chans).remove(&self.id).is_some();
            // The server ends its session; the last link's socket just closes.
            if open
                && Arc::strong_count(&self.conn) > 1
                && let Ok(rt) = tokio::runtime::Handle::try_current()
            {
                let (s, f) = (self.conn.shared.clone(), encode(self.id, &Frame::Close(1000, "")));
                rt.spawn(async move { _ = s.put(f).await });
            }
        }
    }

    type Key = (String, String, Option<String>);

    /// Each server's socket, while a link uses it; one dial at a time per server.
    fn pool() -> &'static Mutex<HashMap<Key, Arc<tokio::sync::Mutex<Weak<Conn>>>>> {
        static POOL: std::sync::OnceLock<Mutex<HashMap<Key, Arc<tokio::sync::Mutex<Weak<Conn>>>>>> =
            std::sync::OnceLock::new();
        POOL.get_or_init(Mutex::default)
    }

    /// A link to `link`'s server on its one socket, `dial` opening it when
    /// there is none (`true`: it said yes to mux; else that socket is the
    /// link's own, as an older server serves).
    pub(crate) async fn link(
        link: &crate::sync::Link,
        dial: impl Future<Output = std::io::Result<(crate::sync::Wire, bool)>>,
    ) -> std::io::Result<crate::sync::Wire> {
        let slot = {
            let mut p = lock(pool());
            p.retain(|_, s| Arc::strong_count(s) > 1 || s.try_lock().map_or(true, |w| w.strong_count() > 0));
            p.entry((link.addr.clone(), link.token.clone(), link.root.clone())).or_default().clone()
        };
        let mut at = slot.lock().await;
        if let Some(c) = at.upgrade().filter(|c| c.shared.alive.load(Ordering::SeqCst)) {
            return Ok(c.open());
        }
        let (w, mux) = dial.await?;
        if !mux {
            return Ok(w);
        }
        let c = Conn::start(w);
        *at = Arc::downgrade(&c);
        Ok(c.open())
    }
}

#[cfg(test)]
mod tests {
    use super::{Frame, decode, encode};

    #[test]
    fn frames_say_their_channel_and_kind_both_ways() {
        let all = [
            (7, Frame::Text(r#"{"t":"hello","x":"1?2.3!4:5"}"#), r#"7:{"t":"hello","x":"1?2.3!4:5"}"#),
            (12, Frame::Text(""), "12:"),
            (1, Frame::Ping(vec![0, 255, 16]), "1?00ff10"),
            (1, Frame::Ping(vec![]), "1?"),
            (4294967295, Frame::Pong(vec![171]), "4294967295.ab"),
            (3, Frame::Close(1011, "it ended: why"), "3!1011 it ended: why"),
            (3, Frame::Close(1000, ""), "3!1000 "),
        ];
        for (chan, f, text) in all {
            assert_eq!(encode(chan, &f), text);
            assert_eq!(decode(text), Some((chan, f)), "{text}");
        }
        assert_eq!(decode("5!1000"), Some((5, Frame::Close(1000, ""))), "no reason");
    }

    #[test]
    fn what_isnt_a_frame_is_none() {
        for t in ["", ":x", "{\"t\":1}", "1", "1x", "1?0", "1?zz", "1!", "1!code why", "4294967296:x", "-1:x"] {
            assert_eq!(decode(t), None, "{t}");
        }
    }
}

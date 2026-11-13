//! Binary side channels (aio's binary streams): screen frames, audio,
//! telemetry, remote input. The cell carries control (who, which session,
//! start and stop: methods, ordered and acked); a WebSocket route
//! ([`super::Route::socket`]) carries the payload, never persisted, diffed
//! or sent to every client.
//!
//! Each frame is `[kind:u8][seq:u24 little-endian][payload…]`, with one
//! sequence counter per kind (a shared one would make every frame of
//! another kind look like a loss), wrapping at 2^24. A frame is sent as
//! droppable (the next one supersedes it: a video frame, a cursor) or not
//! (a key frame, a keystroke): past [`HIGH_WATER`] bytes waiting for a slow
//! receiver, a droppable one is dropped and any other closes the stream
//! (1011 `backpressure`), never a silent desync. The receiver checks each
//! kind's sequence ([`Seen`]): a gap on a droppable kind is information, on
//! any other a broken stream to tear down.
use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Sequence numbers wrap here (a u24).
pub const MOD: u32 = 1 << 24;
/// Bytes waiting for the receiver past which a frame isn't queued.
pub const HIGH_WATER: usize = 4 << 20;
/// Once its handler is done, how long what is queued has to reach the
/// receiver: one that stopped reading (a frozen tab) is then let go.
const LINGER: std::time::Duration = std::time::Duration::from_secs(if cfg!(test) { 1 } else { 10 });

/// `body` as frame `seq` (wrapped) of `kind`.
pub fn encode(kind: u8, seq: u32, body: &[u8]) -> Vec<u8> {
    let s = (seq % MOD).to_le_bytes();
    [&[kind, s[0], s[1], s[2]][..], body].concat()
}

/// A frame's kind, sequence number and payload; `None`: shorter than its header.
pub fn decode(frame: &[u8]) -> Option<(u8, u32, &[u8])> {
    let [kind, a, b, c, body @ ..] = frame else { return None };
    Some((*kind, u32::from_le_bytes([*a, *b, *c, 0]), body))
}

/// What a frame's sequence number says, against the last of its kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seen {
    /// The next one (or the first).
    Next,
    /// This many frames of its kind were lost before it.
    Gap(u32),
    /// Older than one already seen: a duplicate or reordered.
    Old,
}

/// One sequence counter per kind: a sender's next numbers, or a receiver's
/// expectations.
#[derive(Clone, Debug, Default)]
pub struct Seqs(HashMap<u8, u32>);

impl Seqs {
    /// The sender's number for its next frame of `kind`.
    pub fn next(&mut self, kind: u8) -> u32 {
        let n = self.0.entry(kind).or_insert(0);
        let seq = *n;
        *n = (seq + 1) % MOD;
        seq
    }

    /// The receiver's check of frame `seq` of `kind` (aio's `accept`).
    pub fn accept(&mut self, kind: u8, seq: u32) -> Seen {
        let seq = seq % MOD;
        let want = self.0.insert(kind, (seq + 1) % MOD);
        match want {
            None => Seen::Next,
            Some(w) if w == seq => Seen::Next,
            Some(w) if (seq + MOD - w) % MOD < MOD / 2 => Seen::Gap((seq + MOD - w) % MOD),
            Some(_) => Seen::Old,
        }
    }
}

/// A frame received.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub kind: u8,
    pub seq: u32,
    pub body: Vec<u8>,
    /// Against the last frame of its kind.
    pub seen: Seen,
}

/// The stream is over: it was closed, or a frame that isn't droppable
/// couldn't be queued (closed with 1011 `backpressure`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Closed(pub String);

impl std::fmt::Display for Closed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Closed {}

/// One side channel's socket, the server's side.
pub struct Frames {
    rx: futures_util::stream::SplitStream<WebSocket>,
    tx: tokio::sync::mpsc::UnboundedSender<Message>,
    queued: Arc<AtomicUsize>,
    kill: Arc<(AtomicBool, tokio::sync::Notify)>,
    /// Told when this is dropped (its handler is done).
    done: Arc<tokio::sync::Notify>,
    out: Seqs,
    seen: Seqs,
    high_water: usize,
}

impl Frames {
    pub(crate) fn new(ws: WebSocket) -> Self {
        let (sink, rx) = ws.split();
        let (tx, queue) = tokio::sync::mpsc::unbounded_channel();
        let queued = Arc::new(AtomicUsize::new(0));
        let kill = Arc::new((AtomicBool::new(false), tokio::sync::Notify::new()));
        let done = Arc::new(tokio::sync::Notify::new());
        let lingered = {
            let done = done.clone();
            async move {
                done.notified().await;
                tokio::time::sleep(LINGER).await;
            }
        };
        // Its socket and queue go with the writer.
        let writing = write(sink, queue, queued.clone(), kill.clone());
        tokio::spawn(async move {
            tokio::select! { () = writing => {}, () = lingered => {} }
        });
        Self { rx, tx, queued, kill, done, out: Seqs::default(), seen: Seqs::default(), high_water: HIGH_WATER }
    }

    /// Queue `body` as the next frame of `kind`: `Ok(false)` when it is
    /// `droppable` and the receiver is [`HIGH_WATER`] behind (dropped, its
    /// number unused); a frame that isn't droppable then closes the stream.
    pub fn send(&mut self, kind: u8, body: &[u8], droppable: bool) -> Result<bool, Closed> {
        if self.kill.0.load(Ordering::SeqCst) || self.tx.is_closed() {
            return Err(Closed("the stream is closed".into()));
        }
        if self.queued.load(Ordering::SeqCst) > self.high_water {
            if droppable {
                return Ok(false);
            }
            self.kill.0.store(true, Ordering::SeqCst);
            self.kill.1.notify_one();
            return Err(Closed("backpressure: the receiver fell behind on a frame that can't be dropped".into()));
        }
        let frame = encode(kind, self.out.next(kind), body);
        self.queued.fetch_add(frame.len(), Ordering::SeqCst);
        self.tx.send(Message::Binary(frame.into())).map_err(|_| Closed("the stream is closed".into()))?;
        Ok(true)
    }

    /// The next frame; `None` once the stream is over (closed, or a frame
    /// shorter than its header). Text messages are skipped.
    pub async fn recv(&mut self) -> Option<Frame> {
        loop {
            match self.rx.next().await?.ok()? {
                Message::Binary(b) => {
                    let (kind, seq, body) = decode(&b)?;
                    let seen = self.seen.accept(kind, seq);
                    return Some(Frame { kind, seq, body: body.to_vec(), seen });
                }
                Message::Close(_) => return None,
                _ => {}
            }
        }
    }

    /// Bytes queued and not yet taken by the receiver.
    pub fn queued(&self) -> usize {
        self.queued.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    fn high_water(mut self, n: usize) -> Self {
        self.high_water = n;
        self
    }
}

impl Drop for Frames {
    fn drop(&mut self) {
        self.done.notify_one();
    }
}

/// Send what is queued, as the receiver takes it; on a backpressure kill,
/// close with 1011 at once (no frame of the backlog after it).
async fn write(
    mut sink: futures_util::stream::SplitSink<WebSocket, Message>,
    mut queue: tokio::sync::mpsc::UnboundedReceiver<Message>,
    queued: Arc<AtomicUsize>,
    kill: Arc<(AtomicBool, tokio::sync::Notify)>,
) {
    loop {
        let m = tokio::select! {
            m = queue.recv() => m,
            () = kill.1.notified() => break,
        };
        let Some(m) = m else { return };
        let n = if let Message::Binary(b) = &m { b.len() } else { 0 };
        tokio::select! {
            r = sink.send(m) => {
                if r.is_err() {
                    return;
                }
                queued.fetch_sub(n, Ordering::SeqCst);
            }
            () = kill.1.notified() => break,
        }
    }
    let close = axum::extract::ws::CloseFrame { code: 1011, reason: "backpressure".into() };
    _ = tokio::time::timeout(std::time::Duration::from_secs(5), sink.send(Message::Close(Some(close)))).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_is_its_kind_its_seq_and_its_payload() {
        let f = encode(7, 0x01_02_03, b"hi");
        assert_eq!(f, [7, 3, 2, 1, b'h', b'i']);
        assert_eq!(decode(&f), Some((7, 0x01_02_03, &b"hi"[..])));
        assert_eq!(decode(&encode(1, MOD + 5, b"")), Some((1, 5, &b""[..])), "wrapped");
        assert_eq!(decode(&[1, 2, 3]), None);
    }

    #[test]
    fn each_kind_counts_alone_and_wraps() {
        let (mut tx, mut rx) = (Seqs::default(), Seqs::default());
        for _ in 0..3 {
            let (a, b) = (tx.next(1), tx.next(2));
            assert_eq!((rx.accept(1, a), rx.accept(2, b)), (Seen::Next, Seen::Next), "interleaved kinds are no gap");
        }
        assert_eq!(rx.accept(1, 5), Seen::Gap(2));
        assert_eq!(rx.accept(1, 4), Seen::Old);
        let mut rx = Seqs::default();
        rx.accept(9, MOD - 1);
        assert_eq!(rx.accept(9, 0), Seen::Next, "a wrap is no gap");
        assert_eq!(rx.accept(9, MOD - 3), Seen::Old);
        let mut tx = Seqs::default();
        tx.0.insert(4, MOD - 1);
        assert_eq!((tx.next(4), tx.next(4)), (MOD - 1, 0));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_slow_receiver_drops_droppable_frames_and_ends_the_stream_on_others() {
        use axum::extract::WebSocketUpgrade;
        let (got, mut frames) = tokio::sync::mpsc::unbounded_channel::<Frames>();
        let app = axum::Router::new().route(
            "/s",
            axum::routing::get(move |ws: WebSocketUpgrade| {
                let got = got.clone();
                async move { ws.on_upgrade(move |s| async move { _ = got.send(Frames::new(s)) }) }
            }),
        );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        let (mut client, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/s")).await.unwrap();
        let mut f = frames.recv().await.unwrap().high_water(64 << 10);
        // The receiver doesn't read: the queue fills, droppable frames go first.
        let big = vec![0u8; 16 << 10];
        let mut dropped = 0;
        for _ in 0..4000 {
            if !f.send(1, &big, true).unwrap() {
                dropped += 1;
            }
            if dropped > 10 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(dropped > 10, "dropped {dropped}, queued {}", f.queued());
        let e = f.send(2, b"key frame", false).unwrap_err();
        assert!(e.0.starts_with("backpressure"), "{e}");
        assert!(f.send(1, b"x", true).is_err(), "over");
        // The receiver hears it closed, 1011.
        use tokio_tungstenite::tungstenite::Message as M;
        let closed = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while let Some(Ok(m)) = client.next().await {
                if let M::Close(c) = m {
                    return c.map(|c| u16::from(c.code));
                }
            }
            None
        })
        .await
        .unwrap();
        assert_eq!(closed, Some(1011));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_receiver_that_stopped_reading_is_let_go_once_the_handler_is_done() {
        use axum::extract::WebSocketUpgrade;
        let (got, mut frames) = tokio::sync::mpsc::unbounded_channel::<Frames>();
        let app = axum::Router::new().route(
            "/s",
            axum::routing::get(move |ws: WebSocketUpgrade| {
                let got = got.clone();
                async move { ws.on_upgrade(move |s| async move { _ = got.send(Frames::new(s)) }) }
            }),
        );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        // Connected, and never read: a frozen tab.
        let (_client, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/s")).await.unwrap();
        let mut f = frames.recv().await.unwrap();
        let big = vec![0u8; 64 << 10];
        while f.send(1, &big, true).unwrap() {
            tokio::task::yield_now().await;
        }
        // The handler returns: its writer, socket and queue go, in time.
        let writer = f.kill.clone();
        drop(f);
        let gone = tokio::time::timeout(std::time::Duration::from_secs(15), async {
            while Arc::strong_count(&writer) > 1 {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        })
        .await;
        assert!(gone.is_ok(), "the writer still waits on a receiver that doesn't read");
    }
}

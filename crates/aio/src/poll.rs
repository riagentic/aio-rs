//! A sync socket over plain HTTP requests, for a client whose WebSockets
//! don't get through (a proxy or a CDN in front of the server that drops
//! the upgrade): `[client] transport` (aio's `transport` names the wire;
//! aio has no fallback). It carries what a WebSocket would: mux's frames
//! ([`crate::mux`]), so every link of the client rides one polled socket,
//! with the same hello, rights, pacing and patches.
//!
//! - `POST /__aio/poll` shows what `/ws` asks (the key, a token, a session)
//!   and opens one: `{id}`, a random id only the opener learns.
//! - `POST /__aio/poll?id=&n=` sends frames `["…"]`; `n` is how many it sent
//!   before, so a batch sent again (its answer lost) applies once.
//! - `GET /__aio/poll?id=&ack=` takes the frames after the first `ack`
//!   (those it has), waiting up to 20 s for one: `{from, frames}`. A frame
//!   is kept until acked, so an answer lost on the way is taken again.
//!
//! A socket asked nothing for 30 s, or holding 64 MB unacked, is closed;
//! then each request about it is a 404 and the client opens another.
use crate::link::lock;
use axum::extract::ws::Message as AxMsg;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long a take waits for a frame.
const WAIT: Duration = Duration::from_secs(20);
/// A socket asked nothing for this long is closed.
const IDLE: Duration = Duration::from_secs(30);
/// Unacked bytes a socket may hold.
const HELD: usize = 64 << 20;

/// One polled socket, the server's side.
struct Polled {
    /// Frames sent to it, to its links ([`crate::mux::serve`]); `None` once closed.
    to_links: Mutex<Option<tokio::sync::mpsc::UnboundedSender<String>>>,
    /// How many frames were sent to it.
    took: Mutex<u64>,
    /// Its frames not acked yet: the first one's number, them, their bytes.
    out: Mutex<(u64, VecDeque<String>, usize)>,
    more: tokio::sync::Notify,
    heard: Mutex<tokio::time::Instant>,
}

impl Polled {
    fn closed(&self) -> bool {
        lock(&self.to_links).is_none()
    }

    fn close(&self) {
        lock(&self.to_links).take();
        self.more.notify_waiters();
    }

    /// A frame for the client; refused past [`HELD`] bytes unacked.
    fn put(&self, t: String) -> Result<(), axum::Error> {
        let mut out = lock(&self.out);
        if out.2 + t.len() > HELD {
            drop(out);
            self.close();
            return Err(axum::Error::new(std::io::Error::other("a polled client let 64 MB wait unread")));
        }
        out.2 += t.len();
        out.1.push_back(t);
        drop(out);
        self.more.notify_waiters();
        Ok(())
    }

    /// The frames after the first `ack`, those before it let go: (the
    /// first one's number, them); `None` when `ack` is past what was sent.
    fn after(&self, ack: u64) -> Option<(u64, Vec<String>)> {
        let mut out = lock(&self.out);
        if ack > out.0 + out.1.len() as u64 {
            return None;
        }
        while out.0 < ack {
            let gone = out.1.pop_front().map_or(0, |f| f.len());
            out.0 += 1;
            out.2 -= gone;
        }
        Some((out.0, out.1.iter().cloned().collect()))
    }
}

type Polls = Arc<Mutex<HashMap<String, Arc<Polled>>>>;

/// `/__aio/poll` for the clients [`crate::sync`]'s doors admit.
pub(crate) fn route<T: crate::Cell>(doors: crate::sync::Doors<T>) -> axum::routing::MethodRouter {
    use crate::http::Query;
    use axum::http::{Extensions, HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    let polls = Polls::default();
    let max = doors.max_message().saturating_mul(2).max(1 << 20);
    let (p, d) = (polls.clone(), doors.clone());
    let send = move |headers: HeaderMap, ext: Extensions, Query(q): Query, body: axum::body::Bytes| {
        let (polls, doors) = (p.clone(), d.clone());
        async move {
            let Some(id) = q.get("id") else {
                // Open one, as `/ws` admits.
                let a = match doors.admit(headers, &q, &ext).await {
                    Ok(a) => a,
                    Err(r) => return *r,
                };
                let (id, s, ends) = open(&polls);
                tokio::spawn(async move {
                    doors.serve(crate::sync::Socket::Polled(ends), a, true, true).await;
                    s.close();
                });
                return crate::http::json(json!({ "id": id }));
            };
            let Some(s) = lock(&polls).get(id).cloned() else { return gone() };
            *lock(&s.heard) = tokio::time::Instant::now();
            let (Some(n), Ok(frames)) = (q.get("n").and_then(|n| n.parse::<u64>().ok()), serde_json::from_slice::<Vec<String>>(&body))
            else {
                return (StatusCode::BAD_REQUEST, crate::http::json(json!({ "error": "frames: [\"…\"] with n" }))).into_response();
            };
            let mut took = lock(&s.took);
            if n > *took {
                return (StatusCode::CONFLICT, crate::http::json(json!({ "error": "frames missing before these" })))
                    .into_response();
            }
            let new = frames.into_iter().skip((*took - n) as usize);
            for f in new {
                match lock(&s.to_links).as_ref() {
                    Some(tx) if tx.send(f).is_ok() => *took += 1,
                    _ => return gone(),
                }
            }
            crate::http::json(json!({ "took": *took }))
        }
    };
    let take = move |Query(q): Query| {
        let polls = polls.clone();
        async move {
            let (Some(s), Some(ack)) = (q.get("id").and_then(|id| lock(&polls).get(id).cloned()), q.get("ack")) else {
                return gone();
            };
            let Ok(ack) = ack.parse::<u64>() else { return gone() };
            let until = tokio::time::Instant::now() + WAIT;
            loop {
                *lock(&s.heard) = tokio::time::Instant::now();
                // Asked for before looking: a frame put between them wakes it.
                let more = s.more.notified();
                let Some((from, frames)) = s.after(ack) else {
                    return (StatusCode::BAD_REQUEST, crate::http::json(json!({ "error": "acked more than was sent" })))
                        .into_response();
                };
                if !frames.is_empty() || s.closed() || tokio::time::Instant::now() >= until {
                    if frames.is_empty() && s.closed() {
                        return gone();
                    }
                    return ([(axum::http::header::CACHE_CONTROL, "no-store")], crate::http::json(json!({ "from": from, "frames": frames })))
                        .into_response();
                }
                _ = tokio::time::timeout_at(until, more).await;
            }
        }
    };
    axum::routing::post(send).layer(axum::extract::DefaultBodyLimit::max(max)).get(take)
}

/// A socket polled no more (or never opened).
fn gone() -> axum::response::Response {
    use axum::response::IntoResponse;
    (axum::http::StatusCode::NOT_FOUND, crate::http::json(json!({ "error": "no such socket: open another" }))).into_response()
}

/// A new socket in `polls`: its id, it, and its ends for
/// [`crate::mux::serve`]. It leaves `polls` once closed: idle for [`IDLE`],
/// or its links ended.
fn open(polls: &Polls) -> (String, Arc<Polled>, (crate::mux::Sends, crate::mux::Takes)) {
    let id = crate::uuid_v4().simple().to_string();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let s = Arc::new(Polled {
        to_links: Mutex::new(Some(tx)),
        took: Mutex::new(0),
        out: Mutex::new((0, VecDeque::new(), 0)),
        more: tokio::sync::Notify::new(),
        heard: Mutex::new(tokio::time::Instant::now()),
    });
    lock(polls).insert(id.clone(), s.clone());
    let (all, mine, out, it) = (polls.clone(), id.clone(), s.clone(), s.clone());
    tokio::spawn(async move {
        while !s.closed() && lock(&s.heard).elapsed() < IDLE {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        s.close();
        lock(&all).remove(&mine);
    });
    let sends = futures_util::sink::unfold(out, |s, m: AxMsg| async move {
        match m {
            AxMsg::Text(t) => s.put(t.to_string())?,
            AxMsg::Close(_) => s.close(),
            _ => {}
        }
        Ok::<_, axum::Error>(s)
    });
    let takes = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|t| (Ok(AxMsg::Text(t.into())), rx))
    });
    (id, it, (Box::pin(sends), Box::pin(takes)))
}

/// The client's side: a socket to `link`'s server over polled HTTP, as a
/// WebSocket would be one (its frames mux's). Refused as `/ws` would be:
/// a wrong key is `PermissionDenied`.
pub(crate) async fn dial(link: &crate::sync::Link) -> std::io::Result<crate::sync::Wire> {
    let mut c = Http::to(link).await?;
    let (code, body) = c.ask("POST", "/__aio/poll", "{}").await?;
    let reply: Value = serde_json::from_slice(&body).unwrap_or_default();
    let id = match (code, reply["id"].as_str()) {
        (200, Some(id)) => id.to_string(),
        (401, _) => return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, crate::sync::WrongKey)),
        _ => {
            let why = reply["error"].as_str().map_or_else(|| format!("HTTP {code}"), String::from);
            return Err(std::io::Error::other(format!("refused ({code}): {why}")));
        }
    };
    let (in_tx, in_rx) = tokio::sync::mpsc::unbounded_channel();
    let (out_tx, out_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let tasks = [
        tokio::spawn(takes(link.clone(), id.clone(), c, in_tx)).abort_handle(),
        tokio::spawn(sends(link.clone(), id, out_rx)).abort_handle(),
    ];
    let sink = futures_util::sink::unfold(out_tx, |tx, m: crate::sync::TMsg| async move {
        if let crate::sync::TMsg::Text(t) = m {
            tx.send(t.to_string()).map_err(|_| crate::sync::WsError::ConnectionClosed)?;
        }
        Ok(tx)
    });
    Ok(Box::new(Polling { rx: in_rx, sink: Box::pin(sink), tasks }))
}

/// Ask again after a failed request for this long, then the socket is gone.
const RETRY: Duration = crate::link::DEAD;

/// Take the server's frames into `to` until the socket is gone.
async fn takes(
    link: crate::sync::Link,
    id: String,
    mut c: Http,
    to: tokio::sync::mpsc::UnboundedSender<std::result::Result<crate::sync::TMsg, crate::sync::WsError>>,
) {
    let mut ack = 0u64;
    let mut failing = None::<tokio::time::Instant>;
    loop {
        let r = c.ask("GET", &format!("/__aio/poll?id={id}&ack={ack}"), "").await;
        let frames = match &r {
            Ok((200, body)) => serde_json::from_slice::<Value>(body).ok().and_then(|v| {
                let from = v["from"].as_u64()?;
                let all: Vec<String> = serde_json::from_value(v["frames"].clone()).ok()?;
                Some(all.into_iter().skip(ack.saturating_sub(from) as usize).collect::<Vec<_>>())
            }),
            _ => None,
        };
        match frames {
            Some(frames) => {
                failing = None;
                for f in frames {
                    ack += 1;
                    if to.send(Ok(crate::sync::TMsg::text(f))).is_err() {
                        return;
                    }
                }
            }
            // A 404 (closed) ends it; a network failure is asked again a while.
            None if matches!(r, Ok((404, _))) => break,
            None => {
                if failing.get_or_insert_with(tokio::time::Instant::now).elapsed() > RETRY {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
                if let Ok(n) = Http::to(&link).await {
                    c = n;
                }
            }
        }
    }
    _ = to.send(Err(crate::sync::WsError::ConnectionClosed));
}

/// Send the client's frames, each batch until the server has it.
async fn sends(link: crate::sync::Link, id: String, mut from: tokio::sync::mpsc::UnboundedReceiver<String>) {
    let mut c = None::<Http>;
    let mut sent = 0u64;
    while let Some(f) = from.recv().await {
        let mut batch = vec![f];
        while let Ok(f) = from.try_recv() {
            batch.push(f);
        }
        let body = json!(batch).to_string();
        let started = tokio::time::Instant::now();
        loop {
            let r = match c.as_mut() {
                Some(c) => c.ask("POST", &format!("/__aio/poll?id={id}&n={sent}"), &body).await,
                None => Err(std::io::Error::other("not connected")),
            };
            match r {
                Ok((200, _)) => break,
                Ok(_) => return,
                Err(_) if started.elapsed() > RETRY => return,
                Err(_) => {
                    c = Http::to(&link).await.ok();
                    if c.is_none() {
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                }
            }
        }
        sent += batch.len() as u64;
    }
}

/// A polled socket, as a client's [`crate::sync::Wire`].
struct Polling {
    rx: tokio::sync::mpsc::UnboundedReceiver<std::result::Result<crate::sync::TMsg, crate::sync::WsError>>,
    sink: std::pin::Pin<Box<dyn futures_util::Sink<crate::sync::TMsg, Error = crate::sync::WsError> + Send>>,
    tasks: [tokio::task::AbortHandle; 2],
}

impl Drop for Polling {
    fn drop(&mut self) {
        self.tasks.iter().for_each(tokio::task::AbortHandle::abort);
    }
}

impl futures_util::Stream for Polling {
    type Item = std::result::Result<crate::sync::TMsg, crate::sync::WsError>;

    fn poll_next(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Option<Self::Item>> {
        self.get_mut().rx.poll_recv(cx)
    }
}

impl futures_util::Sink<crate::sync::TMsg> for Polling {
    type Error = crate::sync::WsError;

    fn poll_ready(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), Self::Error>> {
        self.get_mut().sink.as_mut().poll_ready(cx)
    }

    fn start_send(self: std::pin::Pin<&mut Self>, m: crate::sync::TMsg) -> Result<(), Self::Error> {
        self.get_mut().sink.as_mut().start_send(m)
    }

    fn poll_flush(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), Self::Error>> {
        self.get_mut().sink.as_mut().poll_flush(cx)
    }

    fn poll_close(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), Self::Error>> {
        self.get_mut().sink.as_mut().poll_close(cx)
    }
}

trait Io: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> Io for S {}

/// One kept-alive HTTP/1.1 connection to a server (TLS when its link pins a
/// root): only what polling asks of it.
struct Http {
    io: tokio::io::BufReader<Box<dyn Io>>,
    link: crate::sync::Link,
}

impl Http {
    async fn to(link: &crate::sync::Link) -> std::io::Result<Self> {
        crate::capabilities::check("sync", &link.addr)?;
        let tcp = tokio::net::TcpStream::connect(&link.addr).await?;
        _ = tcp.set_nodelay(true);
        let io: Box<dyn Io> = match &link.root {
            #[cfg(feature = "tls")]
            Some(pem) => {
                let (host, _) = crate::capabilities::host_port(&link.addr);
                let name = rustls::pki_types::ServerName::try_from(host.trim_matches(['[', ']']).to_string())
                    .map_err(std::io::Error::other)?;
                Box::new(tokio_rustls::TlsConnector::from(crate::tls::client(pem)?).connect(name, tcp).await?)
            }
            #[cfg(not(feature = "tls"))]
            Some(_) => return Err(crate::left_out("tls", "a link pinned to a root (https)")),
            None => Box::new(tcp),
        };
        Ok(Self { io: tokio::io::BufReader::new(io), link: link.clone() })
    }

    /// `method path` with a JSON `body`: the status and the body.
    async fn ask(&mut self, method: &str, path: &str, body: &str) -> std::io::Result<(u16, Vec<u8>)> {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
        let bad = |m: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("poll: {m}"));
        let key = match self.link.token.is_empty() {
            true => String::new(),
            false => format!("authorization: Bearer {}\r\n", self.link.token),
        };
        let head = format!(
            "{method} {path} HTTP/1.1\r\nhost: {}\r\n{key}x-aio-wire: 2\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
            self.link.addr,
            body.len()
        );
        let io = self.io.get_mut();
        io.write_all(format!("{head}{body}").as_bytes()).await?;
        io.flush().await?;
        let mut line = String::new();
        self.io.read_line(&mut line).await?;
        let code = line.split(' ').nth(1).and_then(|c| c.parse::<u16>().ok()).ok_or_else(|| bad("no status"))?;
        let (mut len, mut chunked) = (None, false);
        loop {
            line.clear();
            if self.io.read_line(&mut line).await? == 0 {
                return Err(bad("cut short"));
            }
            let Some((k, v)) = line.trim_end().split_once(':') else { break };
            match k.trim().to_ascii_lowercase().as_str() {
                "content-length" => len = v.trim().parse::<usize>().ok(),
                "transfer-encoding" => chunked = v.to_ascii_lowercase().contains("chunked"),
                _ => {}
            }
        }
        let mut out = Vec::new();
        match (chunked, len) {
            (true, _) => loop {
                line.clear();
                self.io.read_line(&mut line).await?;
                let n = usize::from_str_radix(line.trim().split(';').next().unwrap_or(""), 16).map_err(|_| bad("chunk size"))?;
                let mut part = vec![0; n + 2];
                self.io.read_exact(&mut part).await?;
                if n == 0 {
                    break;
                }
                out.extend_from_slice(&part[..n]);
            },
            (false, Some(n)) => {
                out.resize(n, 0);
                self.io.read_exact(&mut out).await?;
            }
            (false, None) => return Err(bad("no length")),
        }
        Ok((code, out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn a_kept_alive_connection_reads_chunked_and_sized_answers() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            let (mut c, _) = l.accept().await.unwrap();
            let mut b = [0u8; 4096];
            for reply in [
                "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n5\r\nhello\r\n6;x=1\r\n world\r\n0\r\n\r\n",
                "HTTP/1.1 404 Not Found\r\nContent-Length: 2\r\n\r\n{}",
            ] {
                _ = c.read(&mut b).await.unwrap();
                c.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let mut h = Http::to(&crate::sync::Link { addr, token: "k".into(), root: None }).await.unwrap();
        assert_eq!(h.ask("GET", "/a", "").await.unwrap(), (200, b"hello world".to_vec()));
        assert_eq!(h.ask("POST", "/b", "{}").await.unwrap(), (404, b"{}".to_vec()));
        assert!(h.ask("GET", "/c", "").await.is_err(), "closed");
    }

    #[test]
    fn frames_wait_until_acked_and_a_socket_holds_at_most_64_mb() {
        let s = Polled {
            to_links: Mutex::new(None),
            took: Mutex::new(0),
            out: Mutex::new((0, VecDeque::new(), 0)),
            more: tokio::sync::Notify::new(),
            heard: Mutex::new(tokio::time::Instant::now()),
        };
        for f in ["a", "bb", "ccc"] {
            s.put(f.into()).unwrap();
        }
        assert_eq!(s.after(0), Some((0, vec!["a".into(), "bb".into(), "ccc".into()])));
        assert_eq!(s.after(2), Some((2, vec!["ccc".into()])));
        assert_eq!(s.after(1), Some((2, vec!["ccc".into()])), "an old ack: from what is kept");
        assert_eq!(s.after(4), None, "past what was sent");
        assert_eq!(lock(&s.out).2, 3);
        assert!(s.put("x".repeat(HELD)).is_err());
    }
}

//! Sync: the server is the truth. A client gets what its user may see of the
//! state (`CellData::project`: no `#[server]` fields, rows filtered by
//! `visible`), then patches of that view after each change. A gap or a
//! patch that doesn't fit takes the full view again.
//!
//! Calls are predicted: applied to the client's copy at once (the UI never
//! waits), then sent with the ids and random numbers they used, so the
//! server's event matches. What the client shows = the confirmed copy + its
//! calls still waiting; a refusal drops the call and the view rolls back.
//! Calls made offline wait (at most `PENDING_MAX`) and are sent on
//! reconnect; each has an id, so a re-sent call applies once.
//!
//! Transport: WebSocket at `/ws` (as aio), one JSON text message per frame;
//! the key goes in `Authorization: Bearer` (or `?token=`, as aio accepts).
//! An aio-rs client says so with `x-aio-wire: 2`; any other client gets aio's
//! protocol v3 with feature `compat` (`crate::compat`), so aio pages and tools can
//! use an aio-rs app, and HTTP 426 without it.
//! The same protocol runs on each app's local door (`aio::door`: frames on a private
//! socket, no network, no key: the OS admits only this user), so this
//! user's tools and scripts reach a running app (`Live`, `Link::door`).
//! Server fns ([`crate::fns`]) ride the same link: `sfn {id, name, args}`
//! out, `sfnr {id, ok | err, code?}` back (`Remote::run`); with `stream:
//! true`, its parts `sfnc {id, c}` come first (`Remote::run_streamed`). A copy tells
//! its server what is degraded on its side (`diag {down}`), so the app's
//! health shows a client that keeps failing.
//! Not yet: a binary wire.
use crate::patch::{self, Op};
use crate::{By, Cell, Cx, Door, Handle, Inputs, Rejected, Result};
use crate::link::{Gate, MAX_MESSAGE, Pace, Sock, clients, limits_from_wire, live, session};
#[cfg(feature = "compat")]
use crate::link::{Client, Heard, Listed};
use axum::extract::ws::{Message as AxMsg, WebSocket, WebSocketUpgrade};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::{Message as TMsg, client::IntoClientRequest};

/// The header an aio-rs client sends; without it the server speaks aio's protocol
/// (feature `compat`) or refuses.
const WIRE_HEADER: &str = "x-aio-wire";


/// `call_confirmed` waits this long for its reply (bounded, never forever).
pub const CALL_TIMEOUT: Duration = Duration::from_secs(15);
/// Calls waiting for the server, at most; then calls are refused.
pub const PENDING_MAX: usize = 500;

/// A client's link: a WebSocket, or the local door's frames as the same messages.
pub(crate) type Wire = Box<dyn Duplex>;

pub(crate) trait Duplex:
    futures_util::Stream<Item = std::result::Result<TMsg, WsError>> + futures_util::Sink<TMsg, Error = WsError> + Send + Unpin
{
}
impl<D> Duplex for D where
    D: futures_util::Stream<Item = std::result::Result<TMsg, WsError>>
        + futures_util::Sink<TMsg, Error = WsError>
        + Send
        + Unpin
{
}

type WsError = tokio_tungstenite::tungstenite::Error;

pub use crate::door::{at as door, is_door};
pub use crate::link::{DEAD, PING, SEND_TIMEOUT};

/// Steps of the history a time-travel ask lists, newest last (`Live::travel`).
pub const TRAVEL_STEPS: usize = crate::TIMELINE_STEPS;

fn lock<X>(m: &Mutex<X>) -> MutexGuard<'_, X> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn frame(v: &Value) -> TMsg {
    TMsg::text(v.to_string())
}

/// Equal tokens, in time that doesn't depend on where they differ.
pub(crate) fn same(a: &str, b: &str) -> bool {
    same_bytes(a.as_bytes(), b.as_bytes())
}

pub(crate) fn same_bytes(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The next JSON message from the server (pings and pongs skipped).
pub(crate) async fn next(w: &mut Wire) -> Option<Value> {
    loop {
        match w.next().await?.ok()? {
            TMsg::Text(t) => return serde_json::from_str(&t).ok(),
            TMsg::Ping(_) | TMsg::Pong(_) => continue,
            _ => return None,
        }
    }
}


/// `view` with patch message `m` applied; an error when its ops don't fit or
/// what they make isn't what the server has (its `digest`).
fn patched(view: &Value, m: &Value) -> std::result::Result<Value, String> {
    let ops: Vec<Op> = serde_json::from_value(m["ops"].clone()).map_err(|_| "bad ops".to_string())?;
    let mut view = view.clone();
    patch::apply(&mut view, &ops)?;
    match m["digest"].as_str() {
        Some(d) if d != patch::digest(&view) => Err("the patched copy isn't the server's (digest)".into()),
        _ => Ok(view),
    }
}


/// Key guessing on one server's doors ([`crate::auth::budget::Guesses`]).
pub(crate) type Guessed = Arc<Mutex<crate::auth::budget::Guesses>>;

/// A request without a valid credential: 401 (`why`), or 429 with
/// `Retry-After` once its address `ip` guessed too often. `shown`: it showed
/// one (a missing one is never counted: a probe is not a guess).
pub(crate) fn unauthorized(g: &Guessed, ip: std::net::IpAddr, shown: bool, why: &str) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    let now = crate::store::now_ms();
    let blocked = shown
        .then(|| {
            let mut g = lock(g);
            let new = g.blocked(ip, now).is_none();
            g.wrong(ip, now).map(|ms| (ms, new))
        })
        .flatten();
    match blocked {
        None => (StatusCode::UNAUTHORIZED, why.to_string()).into_response(),
        Some((ms, new)) => {
            let secs = (ms + 999) / 1000;
            if new {
                eprintln!("[aio] security: {ip} showed too many wrong keys: refused for {secs}s (429)");
            }
            let msg = format!("too many wrong keys from this address: try again in {secs}s");
            (StatusCode::TOO_MANY_REQUESTS, [(axum::http::header::RETRY_AFTER, secs.to_string())], msg).into_response()
        }
    }
}

/// The key a request shows: `Authorization: Bearer <key>` or `?token=`.
pub(crate) fn shown_key(headers: &axum::http::HeaderMap, query: &HashMap<String, String>) -> Option<String> {
    let bearer = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.get(..7).filter(|p| p.eq_ignore_ascii_case("bearer ")).map(|_| v[7..].trim().to_string()));
    bearer.or_else(|| query.get("token").cloned())
}

/// The token a `/ws` request shows in users mode (pure): its bearer, else
/// app `id`'s session cookie, and only then a `?token=` (a link).
fn users_token(headers: &axum::http::HeaderMap, query: &HashMap<String, String>, id: &str) -> Option<String> {
    crate::auth::routes::session_token(headers, id).or_else(|| query.get("token").cloned())
}

#[cfg(test)]
#[test]
fn a_phone_never_sends_a_key_in_the_clear() {
    let link = |addr: &str, token: &str, root: Option<&str>| Link {
        addr: addr.into(),
        token: token.into(),
        root: root.map(Into::into),
    };
    let why = cleartext_refusal("android", &link("192.168.1.5:8080", "k", None)).unwrap();
    assert!(why.contains("192.168.1.5:8080") && why.contains("ax pair"), "{why}");
    assert!(cleartext_refusal("android", &link("192.168.1.5:8080", "k", Some("pem"))).is_none(), "TLS, pinned");
    for here in ["127.0.0.1:8080", "localhost:9", "[::1]:9"] {
        assert!(cleartext_refusal("android", &link(here, "k", None)).is_none(), "{here} is the phone itself");
    }
    assert!(cleartext_refusal("android", &link("10.0.0.2:80", "", None)).is_none(), "no key to leak");
    assert!(cleartext_refusal("linux", &link("192.168.1.5:8080", "k", None)).is_none(), "a desktop's choice");
}

/// The headers a link's server fns see ([`crate::fns::Req::headers`]):
/// all but its credentials, which app code never needs to hold (pure).
fn fn_headers(mut h: axum::http::HeaderMap) -> axum::http::HeaderMap {
    for k in [axum::http::header::AUTHORIZATION, axum::http::header::COOKIE] {
        h.remove(k);
    }
    h
}



/// Serve `h` at `/ws` to clients that show `token`, until the listener fails
/// (at once for a cell without `shared`).
pub async fn serve<T: Cell>(h: Handle<T>, listener: TcpListener, token: String) -> std::io::Result<()> {
    serve_with(h, listener, Options { token, ..Default::default() }).await
}

pub use crate::config::Limits;



/// Where a server tells same-machine tools it serves (`data/serving.json`).
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Serving {
    pub port: u16,
    pub tls: bool,
    /// The host part of `[server] listen`.
    #[serde(default)]
    pub host: Option<String>,
}

/// How a server serves.
#[derive(Default)]
pub struct Options {
    /// Its limits.
    pub limits: Limits,
    /// The key clients show.
    pub token: String,
    /// Also `POST /__aio/pair`: a device that sends the PIN gets aio's profile
    /// `{aio:1, name, title, port, tls, cert, key}` (`cert` = the root to pin).
    #[cfg(feature = "tls")]
    pub pairing: Option<Arc<crate::pairing::Pairing>>,
    /// Serve over TLS (`wss://`).
    #[cfg(feature = "tls")]
    pub tls: Option<crate::tls::Tls>,
    /// Names it is reached as besides IPs, `localhost` and `.local`: a
    /// reverse proxy's or a custom domain.
    pub hosts: Vec<String>,
    /// The app's own routes and files, on every path aio doesn't serve.
    pub http: crate::http::Http,
    /// Accounts: clients show a session (`Authorization: Bearer`, from
    /// `POST /__aio/auth/login`) instead of the key, and the user is the
    /// session's, never what the client says. A socket whose session ends
    /// (logout, revoke, expiry, a new password) is closed at once.
    pub auth: Option<Arc<crate::auth::Auth>>,
    /// Served beyond loopback: a socket upgrade must say its Origin.
    pub strict_origin: bool,
    /// The header a reverse proxy overwrites with the client's address: its
    /// last hop is then the client ([`crate::gate`]).
    pub trust_proxy: Option<String>,
    /// The security headers ([`crate::security`]).
    pub security: crate::security::Security,
    /// Open on purpose (`[server] open`): links and routes need no key;
    /// minting a PIN still does. Ignored with `auth` or users.
    pub open: bool,
}

/// A client's address, for `ConnectInfo` on any listener.
#[derive(Clone, Copy)]
pub(crate) struct Peer(pub(crate) std::net::SocketAddr);

impl axum::extract::connect_info::Connected<axum::serve::IncomingStream<'_, TcpListener>> for Peer {
    fn connect_info(s: axum::serve::IncomingStream<'_, TcpListener>) -> Self {
        Peer(*s.remote_addr())
    }
}

#[cfg(feature = "tls")]
impl axum::extract::connect_info::Connected<axum::serve::IncomingStream<'_, TlsListener>> for Peer {
    fn connect_info(s: axum::serve::IncomingStream<'_, TlsListener>) -> Self {
        Peer(*s.remote_addr())
    }
}

/// TCP plus a TLS handshake. Handshakes run apart (at most 10 s each), so a
/// slow client can't hold up the others.
#[cfg(feature = "tls")]
struct TlsListener {
    rx: tokio::sync::mpsc::Receiver<(tokio_rustls::server::TlsStream<tokio::net::TcpStream>, std::net::SocketAddr)>,
    addr: std::net::SocketAddr,
}

#[cfg(feature = "tls")]
impl TlsListener {
    fn new(l: TcpListener, cfg: Arc<rustls::ServerConfig>) -> std::io::Result<Self> {
        let addr = l.local_addr()?;
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let acceptor = tokio_rustls::TlsAcceptor::from(cfg);
        tokio::spawn(async move {
            loop {
                let (tcp, from) = tokio::select! {
                    r = l.accept() => match r {
                        Ok(c) => c,
                        Err(_) => { tokio::time::sleep(Duration::from_millis(50)).await; continue }
                    },
                    () = tx.closed() => return,
                };
                let (acceptor, tx) = (acceptor.clone(), tx.clone());
                tokio::spawn(async move {
                    if let Ok(Ok(s)) = tokio::time::timeout(Duration::from_secs(10), acceptor.accept(tcp)).await {
                        _ = tx.send((s, from)).await;
                    }
                });
            }
        });
        Ok(Self { rx, addr })
    }
}

#[cfg(feature = "tls")]
impl axum::serve::Listener for TlsListener {
    type Io = tokio_rustls::server::TlsStream<tokio::net::TcpStream>;
    type Addr = std::net::SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.rx.recv().await {
            Some(c) => c,
            // The accept task ended: nothing more will come.
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(self.addr)
    }
}

/// What a link must show to get in: the key (`None`: the local door, where
/// the OS admits only this user), or with `users`, a user's own token.
#[derive(Clone, Default)]
struct Creds {
    key: Option<Arc<String>>,
    users: Option<crate::auth::Users>,
    guessed: Guessed,
    /// `[server] open`: the key is not asked.
    open: bool,
}

/// `/ws`: clients that show the key (or with `users`, their token) get a
/// session; with accounts (`auth`), clients show a live session instead,
/// and its socket closes when the session ends.
fn ws<T: Cell>(
    h: Handle<T>,
    creds: Creds,
    auth: Option<Arc<crate::auth::Auth>>,
    limits: Limits,
    fns: Arc<crate::fns::Fns>,
    on_link: Option<crate::http::OnLink>,
) -> axum::routing::MethodRouter {
    use crate::http::Query;
    use axum::extract::ConnectInfo;
    use axum::http::{Extensions, HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    let open_links = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    axum::routing::get(
        move |ws: WebSocketUpgrade, headers: HeaderMap, ext: Extensions, Query(q): Query| {
            let (h, creds, auth, open_links, fns, on_link) =
                (h.clone(), creds.clone(), auth.clone(), open_links.clone(), fns.clone(), on_link.clone());
            async move {
                let ip = ext.get::<ConnectInfo<Peer>>().map(|c| c.0.0.ip());
                let refuse = |shown: bool, why: &str| match ip {
                    Some(ip) => unauthorized(&creds.guessed, ip, shown, why),
                    None => (StatusCode::UNAUTHORIZED, why.to_string()).into_response(),
                };
                let mut known = None;
                // A user known by token: the socket re-checks it while open.
                let mut recheck = None;
                let signed = match &auth {
                    None => {
                        // A page in the browser shows its cookie, as on `/__aio/ui`.
                        // With users, the signed-in cookie outranks a `?token=`:
                        // a link can't make a tab act as the link's user.
                        let shown = match &creds.users {
                            Some(_) => users_token(&headers, &q, T::ID),
                            None => shown_key(&headers, &q).or_else(|| {
                                crate::http::parse_cookies(&headers).remove(&format!("aio_key_{}", T::ID))
                            }),
                        };
                        match (&creds.key, &creds.users) {
                            (None, _) => {}
                            (Some(_), Some(u)) => match shown.as_deref().and_then(|t| u.who(t)) {
                                Some(s) => {
                                    recheck = shown.clone().map(|t| (u.clone(), t, s.clone()));
                                    known = Some((By::User(s.user), s.role));
                                }
                                None => return refuse(shown.is_some(), "wrong or missing token"),
                            },
                            (Some(key), None) if creds.open || shown.as_ref().is_some_and(|k| same(k, key)) => {}
                            (Some(_), None) => return refuse(shown.is_some(), "wrong or missing key"),
                        }
                        None
                    }
                    // A session only in the header: tokens never go in URLs.
                    Some(a) => {
                        let (a, t) =
                            (a.clone(), crate::auth::routes::session_token(&headers, T::ID).unwrap_or_default());
                        match tokio::task::spawn_blocking(move || (live(&a, &t), t)).await {
                            Ok((Some(s), t)) => Some((By::User(s.user), t)),
                            Ok((None, t)) => return refuse(!t.is_empty(), "sign in first"),
                            Err(_) => return refuse(false, "sign in first"),
                        }
                    }
                };
                if open_links.fetch_add(1, Ordering::SeqCst) >= limits.clients {
                    open_links.fetch_sub(1, Ordering::SeqCst);
                    let why = format!("too many clients (the limit is {}): try again later", limits.clients);
                    return (
                        StatusCode::SERVICE_UNAVAILABLE,
                        crate::http::json(json!({ "error": why, "retryAfterMs": 5000 })),
                    )
                        .into_response();
                }
                // A browser can't set a header on its socket: it says `?wire=2`.
                let wire2 = headers.get(WIRE_HEADER).is_some_and(|v| v == "2") || q.get("wire").is_some_and(|v| v == "2");
                if !wire2 && !cfg!(feature = "compat") {
                    open_links.fetch_sub(1, Ordering::SeqCst);
                    let why = "this app speaks aio-rs's wire 2 only: send `x-aio-wire: 2` or `?wire=2` (aio clients need aio-rs's feature `compat`)";
                    return (StatusCode::UPGRADE_REQUIRED, why).into_response();
                }
                let from = ext.get::<ConnectInfo<Peer>>().map_or("door".into(), |c| c.0.0.to_string());
                let gate = match (&auth, &signed, &known, &creds.key) {
                    (Some(a), Some((_, t)), ..) => Gate::Signed(a.clone(), t.clone()),
                    (_, _, Some((_, role)), _) => Gate::Known(role.clone()),
                    // Never the door's rights over the network.
                    _ => Gate::Key,
                };
                ws.max_message_size(MAX_MESSAGE)
                    .on_upgrade(move |s| async move {
                        let viewer = signed.as_ref().or(known.as_ref()).map(|(by, _)| by.clone());
                        let account = auth.clone().zip(signed.as_ref().map(|(_, t)| t.clone()));
                        let caller = known.map(|(by, role)| match by {
                            By::User(_) => crate::Caller { by, role: Some(role) },
                            _ => crate::Caller::default(),
                        });
                        let calls = crate::fns::Served { fns, account, known: caller, from: (ip, fn_headers(headers)) };
                        let run = async {
                            #[cfg(not(feature = "compat"))]
                            let r = session(h, Sock::Ws(s), limits, from, gate, viewer, calls, on_link).await;
                            #[cfg(feature = "compat")]
                            let r = if wire2 {
                                session(h, Sock::Ws(s), limits, from, gate, viewer, calls, on_link).await
                            } else {
                                let user = match &viewer {
                                    Some(By::User(u)) => u.clone(),
                                    _ => "peer".into(),
                                };
                                let _heard = Heard::new(on_link, &user, &from);
                                let who = format!("{user} from {from} (aio)");
                                let listed = Listed::new(Client::new(T::ID, user, "aio", from));
                                let link = (listed.0, who);
                                crate::compat::wire::session(h, Sock::Ws(s), limits, gate, viewer, calls, link).await
                            };
                            r
                        };
                        // Dropping the session closes its socket.
                        let r = match (auth, signed, recheck) {
                            (Some(a), Some((_, t)), _) => tokio::select! {
                                r = run => r,
                                () = ended(a, t) => Err("a session ended: its socket is closed".into()),
                            },
                            (_, _, Some((u, t, was))) => tokio::select! {
                                r = run => r,
                                () = users_ended(u, t, was) => Err("its token no longer resolves: its socket is closed".into()),
                            },
                            _ => run.await,
                        };
                        open_links.fetch_sub(1, Ordering::SeqCst);
                        if let Err(e) = r {
                            eprintln!("[aio] sync: {e}");
                        }
                    })
                    .into_response()
            }
        },
    )
}

/// `/__aio/ui`: one tab of the app's UI page ([`crate::http::Http::ui`]).
/// It shows what `/ws` takes, the browser's way (the key's or the session's
/// cookie): the key, a user known by token, or with accounts a live session
/// (or none: a signed-out tab). Its frames go to the tab handler. A tab
/// whose session ends is closed.
fn ui_socket<T: Cell>(
    ui: crate::http::Ui,
    creds: Creds,
    auth: Option<Arc<crate::auth::Auth>>,
    limits: Limits,
) -> axum::routing::MethodRouter {
    use crate::http::Query;
    use axum::extract::ConnectInfo;
    use axum::http::{Extensions, HeaderMap};
    use axum::response::IntoResponse;
    axum::routing::get(
        move |ws: WebSocketUpgrade, headers: HeaderMap, ext: Extensions, Query(q): Query| {
            let (ui, creds, auth) = (ui.clone(), creds.clone(), auth.clone());
            async move {
                let ip = ext.get::<ConnectInfo<Peer>>().map_or(std::net::Ipv4Addr::LOCALHOST.into(), |c| c.0.0.ip());
                let session = crate::auth::routes::session_token(&headers, T::ID);
                let mut recheck = None;
                // (user, credential): `None` is a signed-out tab.
                let who: Option<(Option<String>, String)> = match (&auth, &creds.users, &creds.key) {
                    (Some(a), ..) => {
                        let (a, t) = (a.clone(), session.unwrap_or_default());
                        let s = tokio::task::spawn_blocking(move || (live(&a, &t), t)).await.ok();
                        s.and_then(|(s, t)| Some((Some(s?.user), t)))
                    }
                    (None, Some(u), _) => match session.as_deref().and_then(|t| u.who(t)) {
                        Some(s) => {
                            let t = session.unwrap_or_default();
                            recheck = Some((u.clone(), t.clone(), s.clone()));
                            Some((Some(s.user), t))
                        }
                        None => return unauthorized(&creds.guessed, ip, session.is_some(), "wrong or missing token"),
                    },
                    (None, None, Some(key)) => {
                        let cookie = crate::http::parse_cookies(&headers).remove(&format!("aio_key_{}", T::ID));
                        match shown_key(&headers, &q).or(cookie) {
                            _ if creds.open => Some((None, key.to_string())),
                            Some(k) if same(&k, key) => Some((None, k)),
                            k => return unauthorized(&creds.guessed, ip, k.is_some(), "wrong or missing key"),
                        }
                    }
                    (None, None, None) => Some((None, String::new())),
                };
                let ended_with = auth.clone().zip(who.as_ref().filter(|_| auth.is_some()).map(|(_, t)| t.clone()));
                ws.max_message_size(limits.max_message)
                    .on_upgrade(move |s| async move {
                        let (ours, wire) = tokio::io::duplex(1 << 20);
                        let user = who.and_then(|(user, _)| user);
                        let tab = tokio::spawn((ui.tab)(crate::http::Tab { user, wire }));
                        let pump = tab_pump(s, ours);
                        match (ended_with, recheck) {
                            (Some((a, t)), _) => tokio::select! { () = pump => {}, () = ended(a, t) => {} },
                            (_, Some((u, t, was))) => tokio::select! { () = pump => {}, () = users_ended(u, t, was) => {} },
                            _ => pump.await,
                        }
                        tab.abort();
                    })
                    .into_response()
            }
        },
    )
}

/// A tab's socket to and from its handler's frames, until either side ends.
async fn tab_pump(s: WebSocket, wire: tokio::io::DuplexStream) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (mut w, mut r) = s.split();
    let (mut from, mut to) = tokio::io::split(wire);
    let page = async {
        while let Some(Ok(m)) = r.next().await {
            let b = match m {
                AxMsg::Binary(b) => b.to_vec(),
                AxMsg::Text(t) => t.as_bytes().to_vec(),
                AxMsg::Close(_) => break,
                _ => continue,
            };
            if to.write_u32(b.len() as u32).await.is_err() || to.write_all(&b).await.is_err() {
                break;
            }
        }
    };
    let view = async {
        while let Ok(n) = from.read_u32().await {
            let mut b = vec![0; (n as usize).min(MAX_MESSAGE)];
            if b.len() < n as usize || from.read_exact(&mut b).await.is_err() {
                break;
            }
            if w.send(AxMsg::Binary(b.into())).await.is_err() {
                break;
            }
        }
        _ = w.send(AxMsg::Close(None)).await;
    };
    tokio::select! { () = page => {}, () = view => {} }
}

/// The local door ([`crate::door::serve`]).
pub use crate::door::serve as serve_door;

/// [`serve_door`], running `http`'s server functions too: what a peer
/// app calls ([`crate::peer`]), as `Access::Open` lets anyone.
pub async fn serve_door_with<T: Cell>(
    h: Handle<T>,
    l: crate::ipc::Listener,
    limits: Limits,
    http: &crate::http::Http,
) -> std::io::Result<()> {
    let calls = crate::fns::Served { fns: Arc::new(http.fns.clone()), ..crate::link::no_calls() };
    crate::door::serve_with(h, l, limits, calls, crate::door::OPERATOR).await
}

/// Serve `h` at `/ws` (and pairing, the app's HTTP, over TLS, as `opts` say)
/// until the listener fails.
pub async fn serve_with<T: Cell>(h: Handle<T>, listener: TcpListener, opts: Options) -> std::io::Result<()> {
    use axum::extract::ConnectInfo;
    #[cfg(feature = "tls")]
    use axum::http::StatusCode;
    use axum::http::HeaderMap;
    use axum::response::IntoResponse;
    // Fail closed: only a cell that says `shared` has its state on the
    // network (`/ws`, `/__aio/ui`); another serves its HTTP routes alone.
    if !T::SHARED {
        eprintln!("[aio] {} isn't #[cell(shared)]: only its HTTP routes are served, never its state", T::ID);
    }
    #[cfg(feature = "tls")]
    let Options { token, pairing, tls, limits, hosts, http, auth, strict_origin, trust_proxy, security, open } = opts;
    #[cfg(not(feature = "tls"))]
    let (Options { token, limits, hosts, http, auth, strict_origin, trust_proxy, security, open }, tls) =
        (opts, None::<std::convert::Infallible>);
    auth.iter().for_each(crate::auth::serve);
    // `[server] hosts` and the plugins' `allowed_origins`, merged (aio's `allowedOrigins`).
    let mut hosts = hosts;
    http.hosts.iter().for_each(|h| if !hosts.contains(h) { hosts.push(h.clone()) });
    // Users known by token count only without accounts (sessions say who then).
    let users = http.users.clone().filter(|_| auth.is_none());
    #[cfg(feature = "tls")]
    let own_cert = tls.as_ref().is_some_and(|t| t.own);
    #[cfg(not(feature = "tls"))]
    let own_cert = false;
    let page = crate::security::Page::new(&security, &hosts, own_cert)
        .map(Arc::new)
        .map_err(std::io::Error::other)?;
    let local = listener.local_addr()?;
    let token = Arc::new(token);
    let fns = Arc::new(http.fns.clone());
    let (watched, started) = (h.clone(), std::time::Instant::now());
    let guessed = Guessed::default();
    let creds = Creds { key: Some(token.clone()), users: users.clone(), guessed: guessed.clone(), open };
    let mut app = axum::Router::new();
    if T::SHARED {
        if let Some(ui) = http.ui.clone() {
            app = app.route("/__aio/ui", ui_socket::<T>(ui, creds.clone(), auth.clone(), limits));
        }
        app = app.route("/ws", ws(h, creds, auth.clone(), limits, fns, http.on_link.clone()));
    }
    #[cfg(feature = "tls")]
    if let Some(pairing) = pairing {
        // Same-machine tools that hold the key mint PINs (`ax pair <app>`); a
        // request a proxy relayed is from elsewhere, whatever the socket says.
        let (minter, key, trust) = (pairing.clone(), token.clone(), Arc::new(trust_proxy.clone()));
        app = app.route(
            "/__aio/pin",
            axum::routing::post(
                move |ConnectInfo(Peer(from)): ConnectInfo<Peer>, headers: HeaderMap, body: axum::body::Bytes| {
                    let (pairing, key, trust) = (minter.clone(), key.clone(), trust.clone());
                    async move {
                        let shown = shown_key(&headers, &HashMap::new());
                        let here = from.ip().is_loopback() && !crate::gate::relayed(&headers, trust.as_deref());
                        if !here || !shown.is_some_and(|k| same(&k, &key)) {
                            return (
                                StatusCode::FORBIDDEN,
                                crate::http::json(json!({ "error": "only this machine's tools with the app's key" })),
                            )
                                .into_response();
                        }
                        let plain = serde_json::from_slice::<Value>(&body).ok().is_some_and(|v| v["plain"] == true);
                        let pin = if plain { pairing.new_pin() } else { pairing.new_pake_pin() };
                        eprintln!("[aio] a pairing code was made (valid 3 minutes)");
                        ([(axum::http::header::CACHE_CONTROL, "no-store")], crate::http::json(json!({ "pin": pin })))
                            .into_response()
                    }
                },
            )
            .layer(axum::extract::DefaultBodyLimit::max(4096)),
        );
        let root = tls.as_ref().map(|t| t.root_pem.clone());
        let profile = Arc::new(
            json!({ "aio": 1, "name": T::ID, "title": T::ID, "port": local.port(), "tls": root.is_some(), "cert": root, "key": *token }),
        );
        let fp = Arc::new(tls.as_ref().map(|t| crate::cpace::fingerprint(&t.leaf_der)).unwrap_or_default());
        app = app.route(
            "/__aio/pair",
            axum::routing::post(move |ConnectInfo(Peer(from)): ConnectInfo<Peer>, body: axum::body::Bytes| {
                let (pairing, profile, fp) = (pairing.clone(), profile.clone(), fp.clone());
                // CPace is a few big-number powers: off the async threads.
                async move {
                    tokio::task::spawn_blocking(move || pair_reply(&pairing, &profile, &fp, from.ip(), &body))
                        .await
                        .unwrap_or_else(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response())
                }
            })
            .layer(axum::extract::DefaultBodyLimit::max(4096)),
        );
    }
    // For monitoring (aio's `/__aio/metrics`, `/__aio/health`, `/__aio/vitals`).
    for path in ["/__aio/health", "/__aio/metrics", "/__aio/vitals"] {
        let (h, guessed, trust) = (watched.clone(), guessed.clone(), Arc::new(trust_proxy.clone()));
        let who = Diagnostics { auth: auth.clone(), users: users.clone(), key: token.clone(), open };
        app = app.route(
            path,
            axum::routing::get(
                move |ConnectInfo(Peer(from)): ConnectInfo<Peer>,
                      headers: HeaderMap,
                      crate::http::Query(q): crate::http::Query| {
                    let (h, who, guessed, trust) = (h.clone(), who.clone(), guessed.clone(), trust.clone());
                    async move {
                        let here = from.ip().is_loopback() && !crate::gate::relayed(&headers, trust.as_deref());
                        if !who.admits(here, &headers, &q, T::ID).await {
                            return unauthorized(
                                &guessed,
                                from.ip(),
                                shown_key(&headers, &q).is_some(),
                                "sign in first",
                            );
                        }
                        let no_store = (axum::http::header::CACHE_CONTROL, "no-store");
                        match path {
                            "/__aio/metrics" => {
                                let text = monitored(&h, started);
                                ([(axum::http::header::CONTENT_TYPE, "text/plain; version=0.0.4"), no_store], text)
                                    .into_response()
                            }
                            "/__aio/vitals" => {
                                let (snap, loopback) = crate::vitals::snapshot();
                                let doc = crate::vitals::doc(&snap, loopback, &crate::vitals::get());
                                ([no_store], crate::http::json(doc)).into_response()
                            }
                            _ => ([no_store], crate::http::json(health_now(&h))).into_response(),
                        }
                    }
                },
            ),
        );
    }
    // aio's `/__aio/snapshot` (a dev build's only): the whole state out, or in.
    if cfg!(debug_assertions) {
        let (h, auth, key, guessed, trust) =
            (watched.clone(), auth.clone(), token.clone(), guessed.clone(), Arc::new(trust_proxy.clone()));
        let snap = move |ConnectInfo(Peer(from)): ConnectInfo<Peer>,
                         method: axum::http::Method,
                         headers: HeaderMap,
                         crate::http::Query(q): crate::http::Query,
                         body: axum::body::Bytes| {
            let (h, auth, key, guessed, trust) = (h.clone(), auth.clone(), key.clone(), guessed.clone(), trust.clone());
            async move {
                let here = from.ip().is_loopback() && !crate::gate::relayed(&headers, trust.as_deref());
                if !snapshot_open(auth, &key, &headers, &q, here).await {
                    let why = "the whole state: this machine's tools with the app's key, or an admin";
                    return unauthorized(&guessed, from.ip(), shown_key(&headers, &q).is_some(), why);
                }
                let (code, text) = snapshot_reply(&h, &method, headers.contains_key("x-aio"), &q, &body).await;
                let ty = if code == 200 { "application/json" } else { "text/plain; charset=utf-8" };
                let code = axum::http::StatusCode::from_u16(code).unwrap_or(axum::http::StatusCode::BAD_REQUEST);
                (code, [(axum::http::header::CONTENT_TYPE, ty), (axum::http::header::CACHE_CONTROL, "no-store")], text)
                    .into_response()
            }
        };
        app = app.route(
            "/__aio/snapshot",
            axum::routing::get(snap.clone())
                .post(snap)
                .layer(axum::extract::DefaultBodyLimit::max(SNAPSHOT_MAX_BODY)),
        );
    }
    #[cfg(feature = "tls")]
    if let Some(t) = &tls {
        // Before the key gate: the caller decides whether this server may be sent the key.
        let (key, cert) = (token.clone(), Arc::new(crate::pairing::served_cert(&t.leaf_der, &t.root_pem)));
        let guessed = guessed.clone();
        app = app.route(
            "/__aio/pin-proof",
            axum::routing::post(move |ConnectInfo(Peer(from)): ConnectInfo<Peer>, body: axum::body::Bytes| {
                let (key, cert, guessed) = (key.clone(), cert.clone(), guessed.clone());
                async move {
                    match crate::pairing::answer_pin_proof(&body, &key, &cert) {
                        Ok(v) => crate::http::json(v).into_response(),
                        // A wrong tag is a guess at the key, like a wrong key.
                        Err((401, why)) => match unauthorized(&guessed, from.ip(), true, &why) {
                            r if r.status() == StatusCode::TOO_MANY_REQUESTS => r,
                            _ => (StatusCode::UNAUTHORIZED, crate::http::json(json!({ "error": why }))).into_response(),
                        },
                        Err((code, why)) => {
                            let code = StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST);
                            (code, crate::http::json(json!({ "error": why }))).into_response()
                        }
                    }
                }
            })
            .layer(axum::extract::DefaultBodyLimit::max(4096)),
        );
    }
    if let Some(a) = auth.clone() {
        app = app.merge(crate::auth::routes::router(Arc::new(crate::auth::routes::Doors::new(
            a,
            T::ID,
            tls.is_some(),
            trust_proxy.clone(),
            http.mail.clone(),
            http.oidc.clone(),
        ))));
    }
    let key = Arc::new(crate::http::Key {
        token: token.clone(),
        cookie: format!("aio_key_{}", T::ID),
        secure: tls.is_some(),
        trust: trust_proxy.clone(),
        auth,
        session_cookie: crate::auth::routes::cookie_name(T::ID),
        page: page.clone(),
        guessed,
        users,
        ancestors: crate::security::frame_ancestors(&hosts),
        open,
    });
    let http = Arc::new(http);
    app = app.fallback(move |ConnectInfo(Peer(from)): ConnectInfo<Peer>, req: axum::extract::Request| {
        let (http, key) = (http.clone(), key.clone());
        async move { crate::http::answer(&http, &key, req, from.ip()).await }
    });
    let guard = Arc::new(Guard { hosts, strict_origin: strict_origin && !local.ip().is_loopback(), trust_proxy, page });
    let app = app
        .layer(axum::middleware::from_fn(move |req: axum::extract::Request, next: axum::middleware::Next| {
            let guard = guard.clone();
            async move { gated(&guard, req, next).await }
        }))
        .into_make_service_with_connect_info::<Peer>();
    match tls {
        None => axum::serve(listener, app).await,
        #[cfg(feature = "tls")]
        Some(t) => axum::serve(TlsListener::new(listener, t.server)?, app).await,
        #[cfg(not(feature = "tls"))]
        Some(t) => match t {},
    }
}

/// The largest snapshot `POST /__aio/snapshot` takes (aio's 10 MB).
const SNAPSHOT_MAX_BODY: usize = 10_000_000;

/// Who may read or replace the whole state: this machine's tools with the
/// app's key (`here`: loopback, not relayed), or an admin's session.
async fn snapshot_open(
    auth: Option<Arc<crate::auth::Auth>>,
    key: &str,
    headers: &axum::http::HeaderMap,
    query: &HashMap<String, String>,
    here: bool,
) -> bool {
    let shown = shown_key(headers, query).unwrap_or_default();
    if here && !key.is_empty() && same(&shown, key) {
        return true;
    }
    let Some(a) = auth else { return false };
    tokio::task::spawn_blocking(move || live(&a, &shown).is_some_and(|s| s.role == "admin")).await.unwrap_or(false)
}

/// aio's `handleSnapshot`: `GET` the snapshot; `POST` one (with `X-AIO`,
/// aio's CSRF guard) to load it, `?force=1` past the cell-set check. The
/// status and the answer's text.
async fn snapshot_reply<T: Cell>(
    h: &Handle<T>,
    method: &axum::http::Method,
    x_aio: bool,
    q: &HashMap<String, String>,
    body: &[u8],
) -> (u16, String) {
    match *method {
        axum::http::Method::GET | axum::http::Method::HEAD => match h.export_snapshot() {
            Ok(v) => (200, v.to_string()),
            Err(e) => (500, format!("snapshot refused: {e}")),
        },
        axum::http::Method::POST if !x_aio => (403, "Missing X-AIO header".into()),
        axum::http::Method::POST => {
            let Ok(snap) = serde_json::from_slice::<Value>(body) else { return (400, "Invalid JSON".into()) };
            let (h, force) = (h.clone(), q.get("force").is_some_and(|f| f == "1"));
            match tokio::task::spawn_blocking(move || h.load_snapshot(&snap, force)).await {
                Ok(Ok(())) => (200, "OK".into()),
                Ok(Err(e)) => (400, format!("snapshot refused: {e}")),
                Err(e) => (500, format!("snapshot refused: {e}")),
            }
        }
        _ => (405, "Method Not Allowed".into()),
    }
}

/// `GET /__aio/metrics`: Prometheus text.
fn monitored<T: Cell>(h: &Handle<T>, started: std::time::Instant) -> String {
    let degraded = crate::health::degraded();
    let m = crate::health::Metrics {
        uptime_s: started.elapsed().as_secs(),
        memory: crate::health::memory(),
        clients: lock(clients()).values().filter(|c| c.cell == T::ID).count(),
        calls: crate::log::calls(),
        cell: (T::ID.into(), h.enabled(), h.failures()),
        up: crate::health::now(T::ID)["status"] == "healthy",
        degraded: degraded.into_iter().map(|(n, e)| (n, e.failures)).collect(),
        gauges: crate::health::gauges(),
    };
    crate::health::prometheus(&m)
}

/// `GET /__aio/health`: what `health.json` says, now, and the served cell.
fn health_now<T: Cell>(h: &Handle<T>) -> Value {
    let mut d = crate::health::now(T::ID);
    d["cell"] = json!({ "id": T::ID, "enabled": h.enabled(), "failures": h.failures() });
    d
}

/// What [`crate::gate`] holds a server's requests to.
#[derive(Default)]
struct Guard {
    hosts: Vec<String>,
    /// `strict_origin`, on a server open beyond loopback.
    strict_origin: bool,
    trust_proxy: Option<String>,
    page: Arc<crate::security::Page>,
}

/// Every request passes [`crate::gate`] first; every answer gets its
/// headers. Behind a trusted proxy, the request's peer is the client.
async fn gated(
    guard: &Guard,
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::extract::ConnectInfo;
    use axum::http::{HeaderValue, StatusCode, header};
    use axum::response::IntoResponse;
    static WARNED: AtomicBool = AtomicBool::new(false);
    let trust = guard.trust_proxy.as_deref();
    if let Some(ConnectInfo(Peer(peer))) = req.extensions().get::<ConnectInfo<Peer>>().copied() {
        if let Some(w) = crate::gate::proxy_warning(peer.ip(), req.headers(), trust)
            && !WARNED.swap(true, Ordering::Relaxed)
        {
            eprintln!("[aio] security: {w}");
        }
        let client = crate::gate::client(peer.ip(), req.headers(), trust);
        req.extensions_mut().insert(ConnectInfo(Peer(std::net::SocketAddr::new(client, peer.port()))));
    }
    let refused = {
        let head = |k: header::HeaderName| req.headers().get(k).and_then(|v| v.to_str().ok());
        // HTTP/1.1 names the host in `Host`; HTTP/2 in the URI's authority.
        let host = head(header::HOST).or_else(|| req.uri().authority().map(|a| a.as_str()));
        let post = req.method() == axum::http::Method::POST;
        let upgrade = head(header::UPGRADE).is_some_and(|u| u.eq_ignore_ascii_case("websocket"));
        let need_origin = guard.strict_origin && upgrade;
        let refused = crate::gate::refusal(
            host,
            head(header::ORIGIN),
            post,
            head(header::CONTENT_TYPE),
            &guard.hosts,
            need_origin,
        );
        // A form navigation reaches the app's HTTP, which gives it the shell or this 415.
        let path = req.uri().path();
        let nav = !crate::http::reserved(path) && crate::gate::navigation(req.headers());
        refused.filter(|(code, _)| !(*code == 415 && nav))
    };
    let mut res = match refused {
        Some((code, why)) => {
            eprintln!("[aio] sync: refused a request: {why}");
            (StatusCode::from_u16(code).unwrap_or(StatusCode::FORBIDDEN), crate::http::json(json!({ "error": why })))
                .into_response()
        }
        None => next.run(req).await,
    };
    // An answer that set its own (a served file's sandbox, a nonce) keeps it.
    for (k, v) in &guard.page.headers {
        if let Ok(v) = HeaderValue::from_str(v) {
            res.headers_mut().entry(*k).or_insert(v);
        }
    }
    res
}

/// `POST /__aio/pair`: `{pin}` (plain) or CPace messages `{cpace, …}`, as aio.
#[cfg(feature = "tls")]
fn pair_reply(
    pairing: &crate::pairing::Pairing,
    profile: &Value,
    fp: &[u8],
    from: std::net::IpAddr,
    body: &[u8],
) -> axum::response::Response {
    use crate::cpace;
    use crate::pairing::Refused;
    use axum::http::{StatusCode, header};
    use axum::response::IntoResponse;
    let reply =
        |status: StatusCode, v: Value| (status, [(header::CACHE_CONTROL, "no-store")], crate::http::json(v)).into_response();
    let refused = |r: Refused| match r {
        Refused::Invalid => reply(
            StatusCode::UNAUTHORIZED,
            json!({ "error": "invalid or expired pairing code", "hint": "codes are single-use and expire after 3 minutes: ask for a new one" }),
        ),
        Refused::TooMany => {
            reply(StatusCode::TOO_MANY_REQUESTS, json!({ "error": "too many tries", "hint": "wait for a new code" }))
        }
        Refused::PakeOnly => reply(
            StatusCode::BAD_REQUEST,
            json!({ "error": "this app pairs by PAKE only and never accepts the code itself; the code sent is burnt: update the client, then ask for a new code", "pake": true }),
        ),
        Refused::Busy => reply(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({ "error": "too many PAKE exchanges are open: try again in 30s" }),
        ),
        Refused::Unknown => reply(
            StatusCode::UNAUTHORIZED,
            json!({ "error": "this PAKE exchange is unknown or expired (30s): start pairing again" }),
        ),
    };
    let Ok(b) = serde_json::from_slice::<Value>(body) else {
        return reply(
            StatusCode::BAD_REQUEST,
            json!({ "error": "bad request", "hint": "send {\"pin\": \"123456\"} or a CPace message" }),
        );
    };
    let now = std::time::Instant::now();
    if b.get("cpace").is_none() {
        let Some(pin) = b["pin"].as_str() else {
            return reply(
                StatusCode::BAD_REQUEST,
                json!({ "error": "bad request", "hint": "send {\"pin\": \"123456\"}" }),
            );
        };
        return match pairing.check(pin, from, now) {
            Ok(()) => reply(StatusCode::OK, profile.clone()),
            Err(r) => refused(r),
        };
    }
    if b["cpace"] != cpace::SUITE {
        return reply(
            StatusCode::BAD_REQUEST,
            json!({ "error": format!("unknown PAKE suite {}: this app speaks {}", b["cpace"], cpace::SUITE), "pake": true }),
        );
    }
    if b.get("id").is_none() {
        let (Some(sid), Some(ya)) = (cpace::unb64::<16>(&b["sid"]), cpace::unb64::<32>(&b["Ya"])) else {
            return reply(
                StatusCode::BAD_REQUEST,
                json!({ "error": "PAKE message 1 is { cpace, sid, Ya }: sid 16 bytes and Ya 32 bytes, base64" }),
            );
        };
        return match pairing.offer(&sid, &ya, fp, from, now) {
            Ok(o) => reply(
                StatusCode::OK,
                json!({ "cpace": cpace::SUITE, "id": o.id, "Yb": cpace::b64(&o.yb), "ADb": cpace::b64(&o.adb) }),
            ),
            Err(r) => refused(r),
        };
    }
    let (Some(id), Some(ta)) = (b["id"].as_str(), cpace::unb64::<64>(&b["Ta"])) else {
        return reply(
            StatusCode::BAD_REQUEST,
            json!({ "error": "PAKE message 2 is { cpace, id, Ta }: Ta 64 bytes, base64" }),
        );
    };
    match pairing.confirm(id, &ta, from, now) {
        Ok(c) => reply(
            StatusCode::OK,
            json!({ "Tb": cpace::b64(&c.tb), "box": cpace::b64(&cpace::seal(&c.isk, &c.sid, &profile.to_string(), cpace::random())) }),
        ),
        Err(r) => refused(r),
    }
}

/// Who may read health and metrics: fail closed. They describe the app (its
/// cells, sizes, clients, errors), so they are never anonymous on the
/// network: the key (a bearer, or `?token=`), a live session, or a user
/// known by token. Anonymous only on this machine (`here`: loopback, not
/// relayed by a proxy) and only on an app without accounts or users (as
/// aio, whose per-user apps answer them to a signed-in user only), or on
/// one that is `[server] open` on purpose.
#[derive(Clone)]
struct Diagnostics {
    auth: Option<Arc<crate::auth::Auth>>,
    users: Option<crate::auth::Users>,
    key: Arc<String>,
    open: bool,
}

impl Diagnostics {
    async fn admits(&self, here: bool, headers: &axum::http::HeaderMap, query: &HashMap<String, String>, id: &str) -> bool {
        if (self.open || here) && self.auth.is_none() && self.users.is_none() {
            return true;
        }
        let shown = shown_key(headers, query).unwrap_or_default();
        if !self.key.is_empty() && same(&shown, &self.key) {
            return true;
        }
        let session = crate::auth::routes::session_token(headers, id).unwrap_or(shown);
        match (&self.auth, &self.users) {
            (Some(a), _) => {
                let a = a.clone();
                tokio::task::spawn_blocking(move || live(&a, &session).is_some()).await.unwrap_or(false)
            }
            (None, Some(u)) => {
                let u = u.clone();
                tokio::task::spawn_blocking(move || u.who(&session).is_some()).await.unwrap_or(false)
            }
            (None, None) => false,
        }
    }
}

/// Returns once the session of `token` is over: re-checked on every account
/// change and every few seconds (expiry). The checks keep a used session alive.
async fn ended(a: Arc<crate::auth::Auth>, token: String) {
    let mut changes = a.changes();
    loop {
        tokio::select! {
            r = changes.changed() => if r.is_err() { return },
            () = tokio::time::sleep(DEAD) => {}
        }
        let (a, t) = (a.clone(), token.clone());
        if !tokio::task::spawn_blocking(move || live(&a, &t).is_some()).await.unwrap_or(false) {
            return;
        }
    }
}

/// Returns once `token` no longer resolves to `was`'s user and role (aio's
/// `users:`/`resolveUser` re-check): asked again every
/// [`crate::auth::USERS_RECHECK`] and on every change its resolver ticks. A
/// resolver that panics ends it (fail closed); one slower than
/// [`crate::auth::USERS_WAIT`] keeps the last verdict, is said, and is not
/// called again until it answers. A changed role ends it too: the client
/// reconnects with the new one.
async fn users_ended(u: crate::auth::Users, token: String, was: crate::auth::Session) {
    let mut changes = u.changes();
    let mut asked: Option<tokio::task::JoinHandle<Option<crate::auth::Session>>> = None;
    loop {
        match changes.as_mut() {
            Some(c) => tokio::select! {
                r = c.changed() => if r.is_err() { changes = None },
                () = tokio::time::sleep(crate::auth::USERS_RECHECK) => {}
            },
            None => tokio::time::sleep(crate::auth::USERS_RECHECK).await,
        }
        let mut call = asked.take().unwrap_or_else(|| {
            let (u, t) = (u.clone(), token.clone());
            tokio::task::spawn_blocking(move || u.who(&t))
        });
        match tokio::time::timeout(crate::auth::USERS_WAIT, &mut call).await {
            Ok(Ok(Some(s))) if s.user == was.user && s.role == was.role => {}
            Ok(_) => return,
            Err(_) => {
                eprintln!("[aio] auth: the users resolver has not answered in {:?}: its socket keeps its last verdict", crate::auth::USERS_WAIT);
                asked = Some(call);
            }
        }
    }
}

type Listener<T> = Arc<dyn Fn(&Arc<T>) + Send + Sync>;
type Refusal = Arc<dyn Fn(&str, &Rejected) + Send + Sync>;
type OnSync = Arc<dyn Fn(&Synced) + Send + Sync>;

/// What one catch-up did (aio's `SyncStats`): the server's whole view reached
/// the copy, on a reconnect or after a gap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Synced {
    /// Server events it brought.
    pub events: u64,
    /// From asking (the link opening, or the resync) to having it.
    pub elapsed: Duration,
    /// How the copy stands with it.
    pub status: crate::Status,
}

/// How the copy stands: `pending` calls waiting, the oldest made at `oldest`
/// (ms), `keep` how long a waiting call is kept when the queue is full.
fn conn(online: bool, pending: usize, oldest: Option<i64>, keep: Option<Duration>, now: i64) -> crate::Conn {
    use crate::Conn;
    match () {
        () if pending >= PENDING_MAX && evictable(oldest, keep, now).is_none() => Conn::Blocked,
        () if !online => Conn::Offline,
        () if pending > 0 => Conn::Syncing,
        () => Conn::Online,
    }
}

/// Whether the oldest waiting call (made at `oldest`) has waited past `keep`
/// and may make room in a full queue: how long it waited.
fn evictable(oldest: Option<i64>, keep: Option<Duration>, now: i64) -> Option<Duration> {
    let waited = Duration::from_millis(u64::try_from(now - oldest?).unwrap_or(0));
    (waited >= keep?).then_some(waited)
}

/// A call made on the copy, waiting for the server.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct Pending {
    op: String,
    method: String,
    args: Value,
    inputs: Inputs,
    at: i64,
    /// `#[client(predict = false)]`: not shown before the server ran it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    waits: bool,
}

impl Pending {
    fn msg(&self) -> Value {
        json!({ "t": "call", "op": self.op, "method": self.method, "args": self.args })
    }
}

struct Copy<T> {
    seq: u64,
    /// The server's view as sent (patches apply to this).
    view: Value,
    confirmed: Arc<T>,
    pending: Vec<Pending>,
    shown: Arc<T>,
    /// Who the waiting calls are by.
    by: By,
}

impl<T: Cell> Copy<T> {
    /// What the client shows: the confirmed copy + the calls still waiting
    /// (one that no longer applies is skipped; the server will refuse it).
    fn reshow(&mut self) {
        let mut st = (*self.confirmed).clone();
        for p in self.pending.iter().filter(|p| !p.waits) {
            let cx = Cx::replay(p.at, p.inputs.clone(), &self.by);
            let mut next = st.clone();
            let applied =
                crate::store::apply_guarded(&mut next, &cx, &p.method, p.args.clone()).map_err(Rejected::from);
            if applied.and_then(|_| next.check()).is_ok() {
                st = next;
            }
        }
        self.shown = Arc::new(st);
    }
}

/// What a copy keeps on disk (`Remote::connect_saved`): the server's view it
/// last had and the calls still waiting for it.
#[derive(serde::Serialize, serde::Deserialize)]
struct Saved {
    cell: String,
    v: u32,
    seq: u64,
    view: Value,
    pending: Vec<Pending>,
}

/// A server fn call's answer to come, and whether a person is deciding.
type Running = (tokio::sync::oneshot::Sender<Result<Value>>, tokio::sync::watch::Sender<bool>);

struct Inner<T: Cell> {
    /// Who this copy calls as (the server sees the same).
    by: By,
    /// Where the copy is kept, newest first (a writer task saves it).
    save: Option<tokio::sync::mpsc::UnboundedSender<Saved>>,
    /// The server's limits, once heard.
    limits: Mutex<Option<Limits>>,
    copy: Mutex<Copy<T>>,
    out: Mutex<Option<tokio::sync::mpsc::UnboundedSender<Value>>>,
    waiting: Mutex<HashMap<String, mpsc::Sender<Result<Value>>>>,
    /// Async methods' callers waiting for their end (`end` frames), by call.
    ends: Mutex<HashMap<String, crate::Reply>>,
    /// Server fn calls waiting for their answer; dropped when the link drops.
    /// Each with whether a person is deciding on the server (`sfnh`).
    running: Mutex<HashMap<String, Running>>,
    /// The parts of streamed server fn calls, by call.
    parts: Mutex<HashMap<String, tokio::sync::mpsc::UnboundedSender<Value>>>,
    online: AtomicBool,
    /// When the server's state last reached the copy (ms).
    last_sync: Mutex<Option<i64>>,
    /// When the copy asked for the whole view (`Synced::elapsed`).
    asked: Mutex<Option<tokio::time::Instant>>,
    /// How long a waiting call is kept once the queue is full (`retain`).
    keep: Mutex<Option<Duration>>,
    /// `call_confirmed`'s wait per method (`None`: as long as it takes).
    timeouts: Mutex<HashMap<String, Option<Duration>>>,
    /// The last ping's round trip, in ms (0: none answered on this link).
    rtt: std::sync::atomic::AtomicU64,
    listeners: Mutex<Vec<Listener<T>>>,
    refusals: Mutex<Vec<Refusal>>,
    tells: Mutex<Vec<crate::tell::Show>>,
    syncs: Mutex<Vec<OnSync>>,
    owned: Mutex<HashMap<String, Box<dyn std::any::Any + Send>>>,
}

/// A synced copy of a cell on a server: reads and calls are local at once,
/// the server confirms. Reconnects on its own.
pub struct Remote<T: Cell> {
    inner: Arc<Inner<T>>,
}

impl<T: Cell> Clone for Remote<T> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

/// Where a copy's server is, and how to trust it.
#[derive(Clone, Debug)]
pub struct Link {
    /// `host:port`, or a local door (`door`, `Link::door`).
    pub addr: String,
    /// The key to show; a local door needs none.
    pub token: String,
    /// The server's root (PEM) to pin: then the link is TLS, else plain.
    pub root: Option<String>,
}

impl Link {
    /// The local door of the running app `app`'s cell `cell` on this
    /// machine (`AIORT_DATA` followed, as the app does).
    pub fn door(app: &str, cell: &str) -> std::io::Result<Self> {
        let (data, _, _) = crate::paths::locate(app)?;
        Ok(Self { addr: door(app, &data, cell)?, token: String::new(), root: None })
    }
}

/// Connect and send `hello`; the link and the server's first message (its
/// state, with its `limits`).
async fn open_raw(link: &Link, hello: &Value) -> std::io::Result<(Wire, Value)> {
    let bad = |m: String| std::io::Error::other(m);
    let mut w = dial(link, true).await?;
    w.send(frame(hello)).await.map_err(|e| bad(e.to_string()))?;
    let first = tokio::time::timeout(Duration::from_secs(15), next(&mut w))
        .await
        .ok()
        .flatten()
        .ok_or_else(|| bad("no answer".into()))?;
    if first["t"] != "state" {
        let kind =
            if first["skew"] == true { std::io::ErrorKind::Unsupported } else { std::io::ErrorKind::PermissionDenied };
        return Err(std::io::Error::new(kind, format!("refused: {}", first["why"].as_str().unwrap_or("?"))));
    }
    Ok((w, first))
}

/// Why `link` may not go out in the clear on `os` (pure): on a phone, a
/// key never crosses a network unencrypted (a phone roams onto networks
/// nobody vouches for), so a plain link with a key to anything but this
/// device is refused; pair (`ax pair`) for a pinned TLS link.
fn cleartext_refusal(os: &str, link: &Link) -> Option<String> {
    let (host, _) = crate::capabilities::host_port(&link.addr);
    let here = host == "localhost" || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback());
    (os == "android" && link.root.is_none() && !link.token.is_empty() && !here).then(|| {
        format!(
            "sync: a link to {} would send its key in the clear (ws://), which a phone never does: pair with the \
             server (ax pair) for a TLS link pinned to its root",
            link.addr
        )
    })
}

/// The socket to `link`'s `/ws`, the key shown; `wire2`: as an aio-rs client
/// (else the server speaks aio's protocol v3, as to an aio page).
pub(crate) async fn dial(link: &Link, wire2: bool) -> std::io::Result<Wire> {
    let bad = |m: String| std::io::Error::other(m);
    if is_door(&link.addr) {
        return match wire2 {
            true => Ok(Box::new(door_wire(crate::door::dial(&link.addr).await?))),
            false => Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "a local door speaks only aio-rs")),
        };
    }
    #[cfg(feature = "tls")]
    let connector = match &link.root {
        Some(pem) => Some(tokio_tungstenite::Connector::Rustls(crate::tls::client(pem)?)),
        None => None,
    };
    #[cfg(not(feature = "tls"))]
    if link.root.is_some() {
        return Err(crate::left_out("tls", "a link pinned to a root (wss)"));
    }
    let scheme = if link.root.is_some() { "wss" } else { "ws" };
    if let Some(why) = cleartext_refusal(std::env::consts::OS, link) {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, why));
    }
    crate::capabilities::check("sync", &link.addr)?;
    let stream = tokio::net::TcpStream::connect(&link.addr).await?;
    let mut req = format!("{scheme}://{}/ws", link.addr).into_client_request().map_err(|e| bad(e.to_string()))?;
    req.headers_mut()
        .insert("authorization", format!("Bearer {}", link.token).parse().map_err(|_| bad("bad token".into()))?);
    if wire2 {
        req.headers_mut().insert(WIRE_HEADER, axum::http::HeaderValue::from_static("2"));
    }
    let cfg = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default().max_message_size(Some(MAX_MESSAGE));
    #[cfg(feature = "tls")]
    let r = tokio_tungstenite::client_async_tls_with_config(req, stream, Some(cfg), connector)
        .await
        .map(|(w, _)| Box::new(w) as Wire);
    #[cfg(not(feature = "tls"))]
    let r = tokio_tungstenite::client_async_with_config(req, stream, Some(cfg)).await.map(|(w, _)| Box::new(w) as Wire);
    match r {
        Ok(w) => Ok(w),
        Err(tokio_tungstenite::tungstenite::Error::Http(r)) if r.status() == 401 => {
            Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "refused: wrong key"))
        }
        Err(tokio_tungstenite::tungstenite::Error::Io(e)) => Err(e),
        // The server says why (a limit): pass its words on.
        Err(tokio_tungstenite::tungstenite::Error::Http(r)) => {
            let why = r.body().as_deref().and_then(|b| serde_json::from_slice::<Value>(b).ok());
            let why = why.as_ref().and_then(|v| v["error"].as_str()).unwrap_or(r.status().as_str()).to_string();
            Err(bad(format!("refused ({}): {why}", r.status().as_u16())))
        }
        Err(e) => Err(bad(e.to_string())),
    }
}

/// The door's frames as WebSocket messages, so a link is one type.
fn door_wire(f: crate::door::Frames) -> impl Duplex {
    use crate::link::Msg;
    use tokio_tungstenite::tungstenite::protocol::CloseFrame;
    let out = |m: TMsg| {
        let m = match m {
            TMsg::Text(t) => Ok(Msg::Text(t.as_str().into())),
            TMsg::Ping(p) => Ok(Msg::Ping(p.to_vec())),
            TMsg::Pong(p) => Ok(Msg::Pong(p.to_vec())),
            TMsg::Close(c) => Ok(c.map_or(Msg::Close(1000, String::new()), |c| Msg::Close(c.code.into(), c.reason.as_str().into()))),
            TMsg::Binary(_) | TMsg::Frame(_) => Err(WsError::Io(std::io::Error::other("the door carries no binary"))),
        };
        futures_util::future::ready(m.map(crate::door::encode))
    };
    f.with(out).map(|r| match crate::door::decode(&r?) {
        Some(Msg::Text(t)) => Ok(TMsg::text(t)),
        Some(Msg::Ping(p)) => Ok(TMsg::Ping(p.into())),
        Some(Msg::Pong(p)) => Ok(TMsg::Pong(p.into())),
        Some(Msg::Close(code, why)) => Ok(TMsg::Close(Some(CloseFrame { code: code.into(), reason: why.into() }))),
        None => Err(WsError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, "not a door frame"))),
    })
}

/// `open_raw` for a copy of `T`: the server's state must fit it. `peer`:
/// as another app on a local door ([`crate::peer`]).
async fn open<T: Cell>(link: &Link, user: &str, peer: bool) -> std::io::Result<(Wire, Value)> {
    let hello = json!({ "t": "hello", "cell": T::ID, "v": T::VERSION, "user": user, "peer": peer });
    let (w, first) = open_raw(link, &hello).await?;
    serde_json::from_value::<T>(first["state"].clone()).map_err(std::io::Error::other)?;
    Ok((w, first))
}

impl<T: Cell> Remote<T> {
    /// Connect, show the token, take the first state; then stay connected
    /// (reconnecting with backoff and jitter) until every clone is dropped.
    pub async fn connect(addr: &str, token: &str, user: &str) -> std::io::Result<Self> {
        Self::connect_to(Link { addr: addr.into(), token: token.into(), root: None }, user).await
    }

    /// `connect`, over TLS when `link` has a root to pin.
    pub async fn connect_to(link: Link, user: &str) -> std::io::Result<Self> {
        Self::start(link, user, None, false).await
    }

    /// `connect_to` another app's local door as a peer ([`crate::peer`]).
    pub(crate) async fn peer(link: Link, user: &str) -> std::io::Result<Self> {
        Self::start(link, user, None, true).await
    }

    /// `connect_to`, keeping the copy in `file` (0600): when the server can't
    /// be reached it starts from the saved copy (offline-first), and calls
    /// still waiting for the server survive a restart and are sent later.
    pub async fn connect_saved(link: Link, user: &str, file: &std::path::Path) -> std::io::Result<Self> {
        Self::start(link, user, Some(file.to_path_buf()), false).await
    }

    async fn start(link: Link, user: &str, file: Option<std::path::PathBuf>, peer: bool) -> std::io::Result<Self> {
        let saved: Option<Saved> = file
            .as_ref()
            .and_then(|f| std::fs::read(f).ok())
            .and_then(|b| serde_json::from_slice::<Saved>(&b).ok())
            .filter(|s| s.cell == T::ID && s.v == T::VERSION);
        let first = match open::<T>(&link, user, peer).await {
            Ok(c) => Some(c),
            // Offline (not refused): start from the saved copy, connect later.
            Err(e)
                if saved.is_some()
                    && !matches!(e.kind(), std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::Unsupported) =>
            {
                eprintln!("[aio] sync {}: {e}; starting offline from the saved copy", T::ID);
                None
            }
            Err(e) => return Err(e),
        };
        let pending = saved.as_ref().map(|s| s.pending.clone()).unwrap_or_default();
        let limits = first.as_ref().map(|(_, m)| m["limits"].clone());
        let (w, seq, view) = match (first, saved) {
            (Some((w, m)), _) => (Some(w), m["seq"].as_u64().unwrap_or(0), m["state"].clone()),
            (None, Some(s)) => (None, s.seq, s.view),
            (None, None) => unreachable!("offline without a saved copy returned above"),
        };
        let st: Arc<T> = Arc::new(crate::copy_of(view.clone()).map_err(std::io::Error::other)?);
        let save = file.map(|f| {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Saved>();
            tokio::spawn(async move {
                while let Some(mut s) = rx.recv().await {
                    // Only the newest matters.
                    while let Ok(newer) = rx.try_recv() {
                        s = newer;
                    }
                    let bytes = serde_json::to_vec(&s).unwrap_or_default();
                    let f = f.clone();
                    let r = tokio::task::spawn_blocking(move || crate::paths::write_atomic(&f, &bytes, true)).await;
                    if let Ok(Err(e)) = r {
                        eprintln!("[aio] sync {}: can't save the copy: {e}", T::ID);
                    }
                }
            });
            tx
        });
        let connected = w.is_some();
        let mut copy = Copy { seq, view, confirmed: st.clone(), pending, shown: st, by: By::User(user.into()) };
        copy.reshow();
        let inner = Arc::new(Inner {
            by: By::User(user.into()),
            save,
            limits: Mutex::new(limits.as_ref().and_then(limits_from_wire)),
            copy: Mutex::new(copy),
            out: Mutex::new(None),
            waiting: Mutex::default(),
            ends: Mutex::default(),
            running: Mutex::default(),
            parts: Mutex::default(),
            online: AtomicBool::new(false),
            last_sync: Mutex::new(connected.then(crate::store::now_ms)),
            asked: Mutex::default(),
            keep: Mutex::default(),
            timeouts: Mutex::default(),
            rtt: Default::default(),
            listeners: Mutex::default(),
            refusals: Mutex::default(),
            tells: Mutex::default(),
            syncs: Mutex::default(),
            owned: Mutex::default(),
        });
        let weak = Arc::downgrade(&inner);
        let user = user.to_string();
        tokio::spawn(async move {
            let mut first = w;
            let mut wait = Duration::from_millis(250);
            let mut told = false;
            loop {
                let w = match first.take() {
                    Some(w) => w,
                    None => match (tokio::time::Instant::now(), open::<T>(&link, &user, peer).await) {
                        (asked, Ok((w, first))) => {
                            let Some(inner) = weak.upgrade() else { return };
                            *lock(&inner.asked) = Some(asked);
                            Remote { inner }.take(first);
                            w
                        }
                        (_, Err(e)) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                            eprintln!("[aio] sync {}: {e}; not retrying", T::ID);
                            return;
                        }
                        (_, Err(e)) => {
                            // A version skew may end (the server is updated or rolled back): say it once, keep trying.
                            if e.kind() == std::io::ErrorKind::Unsupported && !told {
                                eprintln!("[aio] sync {}: {e}", T::ID);
                                told = true;
                            }
                            // Jittered backoff, so many clients don't return at once.
                            let jitter = Duration::from_millis(u64::from(crate::uuid_v4().as_bytes()[0]) * 2);
                            tokio::time::sleep(wait + jitter).await;
                            wait = (wait * 2).min(Duration::from_secs(10));
                            if weak.strong_count() == 0 {
                                return;
                            }
                            continue;
                        }
                    },
                };
                wait = Duration::from_millis(250);
                let Some(inner) = weak.upgrade() else { return };
                let r = Remote { inner };
                r.pump(w).await;
                if weak.strong_count() <= 1 {
                    return;
                }
            }
        });
        // Online once the pump runs.
        let r = Self { inner };
        // Saved from the start, so the next run can open offline.
        r.update(|_| {});
        for _ in 0..if connected { 200 } else { 0 } {
            if r.online() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Ok(r)
    }

    /// Run one connection: send what waits, then move messages both ways
    /// until it drops.
    async fn pump(&self, w: Wire) {
        let (mut sink, mut stream) = w.split();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
        for p in lock(&self.inner.copy).pending.clone() {
            _ = tx.send(p.msg());
        }
        // What this process's bus hears, for the server's (a relayed one is
        // not sent back): heard from before the link is said online, so a
        // problem said once it is (a test's, the app's first) is not missed.
        let (heard_tx, mut bus) = tokio::sync::mpsc::unbounded_channel();
        let _sub = crate::report::subscribe(move |d| {
            if !d.kind.starts_with("client:") {
                _ = heard_tx.send(json!({ "t": "heard", "kind": d.kind, "message": d.message, "held": d.held, "severity": d.severity }));
            }
        });
        *lock(&self.inner.out) = Some(tx);
        self.inner.online.store(true, Ordering::SeqCst);
        self.tell();
        // a link that goes silent (Wi-Fi gone, no close) is dead within
        // `DEAD`: pings ask for an answer, any frame counts as one.
        let start = tokio::time::Instant::now();
        let heard = std::sync::atomic::AtomicU64::new(0);
        let seen = || heard.store(start.elapsed().as_millis() as u64, Ordering::Relaxed);
        let calls_per_sec = lock(&self.inner.limits).map_or(Limits::default().calls_per_sec, |l| l.calls_per_sec);
        let writer = async {
            let mut ping = tokio::time::interval(PING);
            // send calls at the server's advertised rate.
            let mut pace = Pace::new(calls_per_sec);
            let mut told = Vec::new();
            loop {
                let m = tokio::select! {
                    m = rx.recv() => match m {
                        Some(m) => {
                            pace.take().await;
                            frame(&m)
                        }
                        None => break,
                    },
                    Some(d) = bus.recv() => frame(&d),
                    _ = ping.tick() => {
                        // What is degraded here, for the server's health (aio's `cdiag`): said when it changes.
                        let down: Vec<Value> = crate::health::degraded()
                            .into_iter()
                            .map(|(name, e)| json!({ "name": name, "failures": e.failures, "since": e.since, "lastError": e.last_error }))
                            .collect();
                        if down != told {
                            if sink.send(frame(&json!({ "t": "diag", "down": down }))).await.is_err() {
                                break;
                            }
                            told = down;
                        }
                        // Its send time, to read the round trip off the pong.
                        TMsg::Ping((start.elapsed().as_micros() as u64).to_be_bytes().to_vec().into())
                    }
                };
                if sink.send(m).await.is_err() {
                    break;
                }
            }
        };
        let watchdog = async {
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let quiet = start.elapsed().as_millis() as u64 - heard.load(Ordering::Relaxed);
                if quiet > DEAD.as_millis() as u64 {
                    eprintln!("[aio] sync {}: no answer for {} ms; reconnecting", T::ID, quiet);
                    break;
                }
                // Every handle is gone: close the link (don't hold a server slot).
                if Arc::strong_count(&self.inner) == 1 {
                    break;
                }
            }
        };
        let reader = async {
            while let Some(Ok(f)) = stream.next().await {
                seen();
                match f {
                    TMsg::Text(t) => match serde_json::from_str::<Value>(&t) {
                        Ok(m) => self.take(m),
                        Err(_) => break,
                    },
                    TMsg::Pong(p) => {
                        if let Ok(sent) = <[u8; 8]>::try_from(&p[..]) {
                            let us = (start.elapsed().as_micros() as u64).saturating_sub(u64::from_be_bytes(sent));
                            self.inner.rtt.store((us / 1000).max(1), Ordering::Relaxed);
                        }
                    }
                    TMsg::Ping(_) | TMsg::Frame(_) => {}
                    TMsg::Binary(_) | TMsg::Close(_) => break,
                }
            }
        };
        tokio::select! { _ = writer => {}, _ = reader => {}, _ = watchdog => {} }
        self.inner.online.store(false, Ordering::SeqCst);
        self.inner.rtt.store(0, Ordering::Relaxed);
        *lock(&self.inner.out) = None;
        // Never re-sent: what waited is told now (after `out`, so none slips in).
        lock(&self.inner.running).clear();
        self.tell();
    }

    /// One message from the server.
    fn take(&self, m: Value) {
        if matches!(m["t"].as_str(), Some("state" | "patch" | "reply")) {
            *lock(&self.inner.last_sync) = Some(crate::store::now_ms());
        }
        match m["t"].as_str() {
            Some("state") => match crate::copy_of::<T>(m["state"].clone()) {
                Ok(st) => {
                    let mut events = 0;
                    self.update(|c| {
                        if let Some(l) = limits_from_wire(&m["limits"]) {
                            *lock(&self.inner.limits) = Some(l);
                        }
                        let seq = m["seq"].as_u64().unwrap_or(0);
                        events = seq.saturating_sub(c.seq);
                        c.seq = seq;
                        c.view = m["state"].clone();
                        c.confirmed = Arc::new(st);
                    });
                    let elapsed = lock(&self.inner.asked).take().map_or(Duration::ZERO, |t| t.elapsed());
                    let synced = Synced { events, elapsed, status: self.status() };
                    let fs = lock(&self.inner.syncs).clone();
                    fs.iter().for_each(|f| f(&synced));
                }
                Err(e) => eprintln!("[aio] sync {}: bad state: {e}", T::ID),
            },
            Some("patch") => {
                let (prev, seq) = (m["prev"].as_u64().unwrap_or(0), m["seq"].as_u64().unwrap_or(0));
                let (mine, view) = {
                    let c = lock(&self.inner.copy);
                    (c.seq, c.view.clone())
                };
                if seq <= mine {
                    return; // already in the copy
                }
                if prev != mine {
                    return self.resync(); // a gap: never guess
                }
                let st = patched(&view, &m)
                    .and_then(|v| crate::copy_of::<T>(v.clone()).map(|st| (v, st)).map_err(|e| e.to_string()));
                match st {
                    Ok((view, st)) => self.update(|c| {
                        c.seq = seq;
                        c.view = view;
                        c.confirmed = Arc::new(st);
                    }),
                    Err(e) => {
                        eprintln!("[aio] sync {}: patch to {seq} doesn't fit ({e}): resync", T::ID);
                        self.resync();
                    }
                }
            }
            Some("reply") => {
                let op = m["op"].as_str().unwrap_or_default().to_string();
                let r = match m.get("err").and_then(Value::as_str) {
                    Some(e) => Err(Rejected { why: e.into(), code: crate::Code::of(&m["code"]) }),
                    None => Ok(m["ok"].clone()),
                };
                let method = lock(&self.inner.copy).pending.iter().find(|p| p.op == op).map(|p| p.method.clone());
                self.update(|c| c.pending.retain(|p| p.op != op));
                if let (Err(e), Some(method)) = (&r, method) {
                    eprintln!("[aio] sync {}: the server refused {method}: {e}", T::ID);
                    let fs = lock(&self.inner.refusals).clone();
                    fs.iter().for_each(|f| f(&method, e));
                }
                if let Err(e) = &r
                    && let Some(end) = lock(&self.inner.ends).remove(&op)
                {
                    end.send(Err(e.clone()));
                }
                if let Some(tx) = lock(&self.inner.waiting).remove(&op) {
                    _ = tx.send(r);
                }
            }
            // An async method's end, after its reply.
            Some("end") => {
                let r = match m.get("err").and_then(Value::as_str) {
                    Some(e) => Err(Rejected { why: e.into(), code: crate::Code::of(&m["code"]) }),
                    None => Ok(m["ok"].clone()),
                };
                if let Some(end) = m["op"].as_str().and_then(|op| lock(&self.inner.ends).remove(op)) {
                    end.send(r);
                }
            }
            Some("sfnc") => {
                if let Some(tx) = m["id"].as_str().and_then(|id| lock(&self.inner.parts).get(id).cloned()) {
                    _ = tx.send(m["c"].clone());
                }
            }
            Some("sfnr") => {
                let r = match m.get("err").and_then(Value::as_str) {
                    Some(e) => Err(Rejected { why: e.into(), code: crate::Code::of(&m["code"]) }),
                    None => Ok(m["ok"].clone()),
                };
                if let Some(id) = m["id"].as_str() {
                    lock(&self.inner.parts).remove(id);
                }
                if let Some((tx, _)) = m["id"].as_str().and_then(|id| lock(&self.inner.running).remove(id)) {
                    _ = tx.send(r);
                }
            }
            Some("sfnh") => {
                let running = lock(&self.inner.running);
                if let Some((_, held)) = m["id"].as_str().and_then(|id| running.get(id)) {
                    held.send_replace(m["hold"] == true);
                }
            }
            // What a call on the server told the people using the app.
            Some("tell") => match serde_json::from_value::<crate::tell::Tell>(m["d"].clone()) {
                Ok(t) => {
                    let ls = lock(&self.inner.tells).clone();
                    ls.iter().for_each(|f| f(&t));
                    crate::tell::show(&t);
                }
                Err(e) => eprintln!("[aio] sync {}: a tell this build can't read: {e}", T::ID),
            },
            _ => eprintln!("[aio] sync {}: unknown message {m}", T::ID),
        }
    }

    fn send(&self, m: Value) {
        if let Some(tx) = lock(&self.inner.out).as_ref() {
            _ = tx.send(m);
        }
    }

    fn resync(&self) {
        *lock(&self.inner.asked) = Some(tokio::time::Instant::now());
        self.send(json!({ "t": "resync" }));
    }

    /// Change the copy, recompute what's shown, tell the listeners.
    fn update(&self, f: impl FnOnce(&mut Copy<T>)) {
        {
            let mut c = lock(&self.inner.copy);
            f(&mut c);
            c.reshow();
            if let Some(tx) = &self.inner.save {
                let saved = Saved {
                    cell: T::ID.into(),
                    v: T::VERSION,
                    seq: c.seq,
                    view: c.view.clone(),
                    pending: c.pending.clone(),
                };
                _ = tx.send(saved);
            }
        }
        self.tell();
    }

    /// Show the listeners the state: it changed, or the link went up or down.
    fn tell(&self) {
        let shown = self.state();
        let ls = lock(&self.inner.listeners).clone();
        ls.iter().for_each(|f| f(&shown));
    }

    /// What the client shows: confirmed + its calls still waiting.
    pub fn state(&self) -> Arc<T> {
        lock(&self.inner.copy).shown.clone()
    }

    /// The server's state as last heard.
    pub fn confirmed(&self) -> Arc<T> {
        lock(&self.inner.copy).confirmed.clone()
    }

    /// The last server event in the copy.
    pub fn seq(&self) -> u64 {
        lock(&self.inner.copy).seq
    }

    /// Calls waiting for the server (for spinners and "syncing…").
    /// The link's last measured round trip (a ping every [`PING`]), if one
    /// was answered on it.
    pub fn rtt(&self) -> Option<Duration> {
        match self.inner.rtt.load(Ordering::Relaxed) {
            0 => None,
            ms => Some(Duration::from_millis(ms)),
        }
    }

    pub fn pending(&self) -> usize {
        lock(&self.inner.copy).pending.len()
    }

    /// Calls to `method` waiting for the server (a spinner on its button).
    pub fn pending_of(&self, method: &str) -> usize {
        lock(&self.inner.copy).pending.iter().filter(|p| p.method == method).count()
    }

    pub fn online(&self) -> bool {
        self.inner.online.load(Ordering::SeqCst)
    }

    /// How the copy stands, for the UI (`Source::status`).
    pub fn status(&self) -> crate::Status {
        let online = self.online();
        let (pending, oldest) = {
            let c = lock(&self.inner.copy);
            (c.pending.len(), c.pending.first().map(|p| p.at))
        };
        let keep = *lock(&self.inner.keep);
        crate::Status {
            online,
            pending,
            degraded: pending * 5 > PENDING_MAX * 4,
            conn: conn(online, pending, oldest, keep, crate::store::now_ms()),
            last_sync: *lock(&self.inner.last_sync),
            rtt: self.rtt(),
        }
    }

    /// Once `PENDING_MAX` calls wait, a new call makes room by dropping the
    /// oldest one that has waited longer than `keep` (aio's `offline.retention`,
    /// e.g. `[client] retention = "7d"`); the drop is told to `on_refused`.
    /// Unset (the default): nothing is ever dropped, and a full queue refuses
    /// new calls.
    pub fn retain(&self, keep: Option<Duration>) {
        *lock(&self.inner.keep) = keep;
        self.tell();
    }

    /// How long `call_confirmed` waits for `method` (aio's `callTimeouts`):
    /// `None` = as long as it takes. Unset: `CALL_TIMEOUT`.
    pub fn timeout(&self, method: &str, wait: Option<Duration>) {
        lock(&self.inner.timeouts).insert(method.into(), wait);
    }

    /// Call `f` after each catch-up: the server's whole view reached the copy
    /// (on a reconnect, or after a gap).
    pub fn on_sync(&self, f: impl Fn(&Synced) + Send + Sync + 'static) {
        lock(&self.inner.syncs).push(Arc::new(f));
    }

    /// Call `f` with the shown state after each change.
    pub fn listen(&self, f: impl Fn(&Arc<T>) + Send + Sync + 'static) {
        lock(&self.inner.listeners).push(Arc::new(f));
    }

    /// Call `f` with what the server's calls tell the people using the app
    /// (`cx.tell`), as it arrives; it is shown here as well ([`crate::tell::show`]).
    pub fn on_tell(&self, f: impl Fn(&crate::tell::Tell) + Send + Sync + 'static) {
        lock(&self.inner.tells).push(Arc::new(f));
    }

    /// Call `f(method, why)` when the server refuses a predicted call.
    pub fn on_refused(&self, f: impl Fn(&str, &Rejected) + Send + Sync + 'static) {
        lock(&self.inner.refusals).push(Arc::new(f));
    }

    /// Make the call on the copy and send it (after registering `waiter`
    /// for its reply, so a fast reply can't be missed). In a full queue, a
    /// call that applies makes room by dropping the oldest one past `retain`.
    fn predict(
        &self,
        method: &str,
        args: Value,
        waiter: Option<mpsc::Sender<Result<Value>>>,
        end: Option<crate::Reply>,
    ) -> Result<(String, Value)> {
        let (op, out, p, dropped) = {
            let mut c = lock(&self.inner.copy);
            let keep = *lock(&self.inner.keep);
            let room = evictable(c.pending.first().map(|p| p.at), keep, crate::store::now_ms());
            crate::ensure!(
                c.pending.len() < PENDING_MAX || room.is_some(),
                "blocked: {PENDING_MAX} calls wait for the server"
            );
            let op = crate::cx::Op::new();
            let cx = Cx::synced(crate::store::now_ms(), T::ID, &self.inner.by, &op);
            let waits = T::UNPREDICTED.contains(&method);
            let out = match waits {
                // Sent as is: the server alone runs it (and may refuse it).
                true => Value::Null,
                false => {
                    let mut next = (*c.shown).clone();
                    let out =
                        crate::store::apply_guarded(&mut next, &cx, method, args.clone()).map_err(Rejected::from)?;
                    next.check()?;
                    out
                }
            };
            let at = cx.now();
            let (method, inputs) = (method.into(), cx.into_asks().inputs);
            let p = Pending { op: op.to_string(), method, args, inputs, at, waits };
            // Too big for the server: refused here, at once, instead of being
            // sent (and re-sent after every close) forever.
            if let Some(l) = *lock(&self.inner.limits) {
                let size = p.msg().to_string().len();
                crate::ensure!(
                    size <= l.max_message,
                    "too big for the server: {size} bytes, its limit is {}",
                    l.max_message
                );
            }
            let dropped = room.filter(|_| c.pending.len() >= PENDING_MAX).map(|w| (c.pending.remove(0), w));
            c.pending.push(p.clone());
            (p.op.clone(), out, p, dropped)
        };
        if let Some((gone, waited)) = dropped {
            // aio's drop report: what is known, and only that.
            let why = Rejected::new(format!(
                "dropped after waiting {}s unconfirmed, to make room in a full queue: the server may or may not \
                 have it (an answer can be lost with the link), and it isn't sent again",
                waited.as_secs()
            ));
            eprintln!("[aio] sync {}: {}: {}", T::ID, gone.method, why.why);
            if let Some(tx) = lock(&self.inner.waiting).remove(&gone.op) {
                _ = tx.send(Err(why.clone()));
            }
            if let Some(end) = lock(&self.inner.ends).remove(&gone.op) {
                end.send(Err(why.clone()));
            }
            let fs = lock(&self.inner.refusals).clone();
            fs.iter().for_each(|f| f(&gone.method, &why));
        }
        if let Some(tx) = waiter {
            lock(&self.inner.waiting).insert(op.clone(), tx);
        }
        if let Some(end) = end {
            lock(&self.inner.ends).insert(op.clone(), end);
        }
        self.update(|_| {});
        self.send(p.msg());
        Ok((op, out))
    }

    /// Run the server function `f` on the server ([`crate::fns`]): its answer,
    /// within `f`'s timeout, not counting while a person is at a picker the
    /// server opened (then a full window again). Never queued: offline, it
    /// fails at once, and a link that drops fails what was waiting.
    pub async fn run<A: serde::Serialize, R: serde::de::DeserializeOwned>(
        &self,
        f: crate::fns::ServerFn<A, R>,
        args: A,
    ) -> Result<R> {
        self.run_parts(f, args, None::<fn(Value)>).await
    }

    /// [`Self::run`] for a streamed server fn: each part it sends reaches
    /// `on_part` (typed `C`), in order, before the answer. The timeout counts
    /// from the last part, so a long answer that keeps coming never times out.
    /// A part that isn't a `C` fails the call.
    pub async fn run_streamed<A: serde::Serialize, R: serde::de::DeserializeOwned, C: serde::de::DeserializeOwned>(
        &self,
        f: crate::fns::ServerFn<A, R>,
        args: A,
        mut on_part: impl FnMut(C),
    ) -> Result<R> {
        let mut bad = None;
        let r = self
            .run_parts(
                f,
                args,
                Some(|v: Value| match (serde_json::from_value::<C>(v), &bad) {
                    (Ok(c), None) => on_part(c),
                    (Err(e), None) => bad = Some(e.to_string()),
                    _ => {}
                }),
            )
            .await;
        match bad {
            Some(e) => Err(Rejected::new(format!("server fn \"{}\": a part doesn't fit its type ({e})", f.name()))),
            None => r,
        }
    }

    async fn run_parts<A: serde::Serialize, R: serde::de::DeserializeOwned>(
        &self,
        f: crate::fns::ServerFn<A, R>,
        args: A,
        mut on_part: Option<impl FnMut(Value)>,
    ) -> Result<R> {
        let name = f.name();
        let args = serde_json::to_value(args)
            .map_err(|e| Rejected::new(format!("server fn \"{name}\": the arguments can't be sent as JSON ({e})")))?;
        let id = crate::uuid_v4().to_string();
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        let (held_tx, mut held) = tokio::sync::watch::channel(false);
        lock(&self.inner.running).insert(id.clone(), (tx, held_tx));
        let (parts_tx, mut parts) = tokio::sync::mpsc::unbounded_channel();
        let stream = on_part.is_some();
        if stream {
            lock(&self.inner.parts).insert(id.clone(), parts_tx);
        }
        let forget = || {
            lock(&self.inner.running).remove(&id);
            lock(&self.inner.parts).remove(&id);
        };
        let m = json!({ "t": "sfn", "id": id, "name": name, "args": args, "stream": stream });
        if !lock(&self.inner.out).as_ref().is_some_and(|o| o.send(m).is_ok()) {
            forget();
            return Err(Rejected::lost(format!("server fn \"{name}\": offline, nothing was sent")));
        }
        // Each part starts the wait again; while a person is deciding on the
        // server no clock runs, and after it a fresh full window.
        let fresh = || Some(tokio::time::Instant::now() + f.wait());
        let mut deadline = fresh();
        let answer = loop {
            tokio::select! {
                biased;
                Some(p) = parts.recv() => {
                    on_part.as_mut().into_iter().for_each(|f| f(p.clone()));
                    if deadline.is_some() {
                        deadline = fresh();
                    }
                }
                r = &mut rx => break Some(r),
                c = held.changed() => match c {
                    Ok(()) if *held.borrow_and_update() => deadline = None,
                    Ok(()) => deadline = deadline.or_else(fresh),
                    Err(_) => break Some((&mut rx).await),
                },
                _ = tokio::time::sleep_until(deadline.unwrap_or_else(tokio::time::Instant::now)), if deadline.is_some() => break None,
            }
        };
        let answer = match answer {
            Some(Ok(r)) => {
                // Parts that came with the answer, then the answer.
                while let Ok(p) = parts.try_recv() {
                    on_part.as_mut().into_iter().for_each(|f| f(p.clone()));
                }
                r?
            }
            Some(Err(_)) => {
                forget();
                return Err(Rejected::lost(format!(
                    "server fn \"{name}\": the link to the server dropped before it answered"
                )));
            }
            None => {
                forget();
                return Err(Rejected::new(format!(
                    "server fn \"{name}\" timed out after {:?}: the server never answered (it may still be running)",
                    f.wait()
                )));
            }
        };
        serde_json::from_value(answer)
            .map_err(|e| Rejected::new(format!("server fn \"{name}\": the answer doesn't fit its type ({e})")))
    }

    /// Call and wait for the server's answer (up to the method's `timeout`,
    /// else `CALL_TIMEOUT`); for calls that must not be shown before they're
    /// confirmed. A link that drops meanwhile doesn't fail it: the call is
    /// sent again on reconnect and applies once. Don't call it on an async
    /// task's thread without `spawn_blocking`.
    pub fn call_confirmed(&self, method: &str, args: Value) -> Result<Value> {
        let wait = lock(&self.inner.timeouts).get(method).copied().unwrap_or(Some(CALL_TIMEOUT));
        if T::async_mode(method).is_some() {
            // An async method's answer is its end: up to its wait for the
            // reply, and the call ceiling for its awaits.
            let (end, answer) = crate::Reply::new(method);
            let (op, _) = self.predict(method, args, None, Some(end))?;
            let r = match wait {
                Some(w) => crate::rt().block_on(tokio::time::timeout(w + crate::effect::call_ceiling(), answer)).ok(),
                None => Some(crate::rt().block_on(answer)),
            };
            return r.unwrap_or_else(|| {
                lock(&self.inner.ends).remove(&op);
                Err(Rejected::new(format!("{method}: no end in time (it stays queued)")))
            });
        }
        let (tx, rx) = mpsc::channel();
        let (op, _) = self.predict(method, args, Some(tx), None)?;
        let r = match wait {
            Some(w) => rx.recv_timeout(w).ok(),
            None => rx.recv().ok(),
        };
        r.unwrap_or_else(|| {
            lock(&self.inner.waiting).remove(&op);
            let wait = wait.unwrap_or_default();
            Err(Rejected::new(format!("{method}: no answer in {wait:?} (it stays queued)")))
        })
    }
}

impl<T: Cell> Door<T> for Remote<T> {
    /// Predicted: applied to the copy at once and sent; the server confirms
    /// or refuses later (`on_refused`). Works offline (queued). A
    /// `predict = false` method is only sent: it shows once the server ran it
    /// (`pending_of` counts it until then).
    fn call(&self, method: &str, args: Value) -> Result<Value> {
        self.predict(method, args, None, None).map(|(_, out)| out)
    }

    /// Predicted like `call`; an async method's end comes from the server
    /// (a refused reply ends it at once), even across a dropped link.
    fn ask(&self, method: &str, args: Value) -> Result<crate::Answer> {
        if T::async_mode(method).is_none() {
            return self.call(method, args).map(|v| crate::Answer::ready(Ok(v)));
        }
        let (end, answer) = crate::Reply::new(method);
        self.predict(method, args, None, Some(end))?;
        Ok(answer)
    }

    fn read<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        f(&self.state())
    }

    fn own(&self, key: &str, res: Option<Box<dyn std::any::Any + Send>>) {
        let old = match res {
            Some(r) => lock(&self.inner.owned).insert(key.into(), r),
            None => lock(&self.inner.owned).remove(key),
        };
        drop(old);
    }
}

/// A live, untyped look at a served cell, for tools and scripts that don't
/// know its type (`ax state`, `ax call`): the server's view as JSON, kept
/// current. Nothing is predicted or queued: a call waits for the server's
/// answer, and a link that drops says so at once.
pub struct Live {
    w: Wire,
    version: u64,
    seq: u64,
    view: Value,
}

impl Live {
    /// Connect to `link`'s cell `cell` as `user`.
    pub async fn connect(link: &Link, cell: &str, user: &str) -> std::io::Result<Self> {
        Self::open(link, cell, user, false).await
    }

    /// `connect` to another app's local door as a peer ([`crate::peer`]).
    pub(crate) async fn peer(link: &Link, cell: &str, user: &str) -> std::io::Result<Self> {
        Self::open(link, cell, user, true).await
    }

    async fn open(link: &Link, cell: &str, user: &str, peer: bool) -> std::io::Result<Self> {
        let hello = json!({ "t": "hello", "cell": cell, "tool": true, "user": user, "peer": peer });
        let (w, first) = open_raw(link, &hello).await?;
        Ok(Self {
            w,
            version: first["v"].as_u64().unwrap_or(0),
            seq: first["seq"].as_u64().unwrap_or(0),
            view: first["state"].clone(),
        })
    }

    /// What this user may see of the state, as the server last sent it.
    pub fn view(&self) -> &Value {
        &self.view
    }

    /// The last server event in the view.
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// The version of the cell the server runs.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Wait until the view changes.
    pub async fn changed(&mut self) -> std::io::Result<()> {
        loop {
            let m = self.next().await?;
            if self.take(&m).await? {
                return Ok(());
            }
        }
    }

    /// Call `method` with `args` (a JSON list; `null` for none) and wait for the answer (up
    /// to `CALL_TIMEOUT`); its change is in `view` by then.
    pub async fn call(&mut self, method: &str, args: Value) -> Result<Value> {
        let gone = |e: std::io::Error| Rejected::lost(format!("{method}: the link dropped ({e}); it may have run"));
        self.w
            .send(frame(&json!({ "t": "call", "method": method, "args": args })))
            .await
            .map_err(|e| gone(std::io::Error::other(e)))?;
        let answer = async {
            loop {
                let m = self.next().await?;
                if m["t"] == "reply" {
                    return Ok(m);
                }
                self.take(&m).await?;
            }
        };
        let mut m = tokio::time::timeout(CALL_TIMEOUT, answer)
            .await
            .map_err(|_| Rejected::new(format!("{method}: no answer in {CALL_TIMEOUT:?}")))?
            .map_err(gone)?;
        // An async method: its answer is its end, after its last await.
        if m["async"] == true {
            let wait = crate::effect::call_ceiling() + CALL_TIMEOUT;
            let end = async {
                loop {
                    let e = self.next().await?;
                    if e["t"] == "end" {
                        return Ok(e);
                    }
                    self.take(&e).await?;
                }
            };
            m = tokio::time::timeout(wait, end)
                .await
                .map_err(|_| Rejected::new(format!("{method}: no end in {wait:?} (it may still run)")))?
                .map_err(gone)?;
        }
        match m.get("err").and_then(Value::as_str) {
            Some(e) => Err(Rejected { why: e.into(), code: crate::Code::of(&m["code"]) }),
            None => Ok(m["ok"].clone()),
        }
    }

    /// Time travel on the running cell (`cmd`: undo, redo, goto with `to`,
    /// pause, resume, where): where it is then (see `TRAVEL_STEPS`). Only on
    /// the app's local door.
    pub async fn travel(&mut self, cmd: &str, to: Option<u64>) -> Result<Value> {
        let gone = |e: std::io::Error| Rejected::lost(format!("{cmd}: the link dropped ({e})"));
        self.w
            .send(frame(&json!({ "t": "travel", "cmd": cmd, "to": to })))
            .await
            .map_err(|e| gone(std::io::Error::other(e)))?;
        loop {
            let m = self.next().await.map_err(gone)?;
            if m["t"] == "travel" {
                return match m["err"].as_str() {
                    Some(e) => Err(Rejected::new(e)),
                    None => Ok(m),
                };
            }
            self.take(&m).await.map_err(gone)?;
        }
    }

    /// The open links to the cell, oldest first: `{id, user, wire, from,
    /// since}` each. Only on the app's local door.
    pub async fn clients(&mut self) -> std::io::Result<Vec<Value>> {
        self.w.send(frame(&json!({ "t": "clients" }))).await.map_err(std::io::Error::other)?;
        loop {
            let m = self.next().await?;
            if m["t"] == "clients" {
                return match m["err"].as_str() {
                    Some(e) => Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, e)),
                    None => Ok(m["list"].as_array().cloned().unwrap_or_default()),
                };
            }
            self.take(&m).await?;
        }
    }

    /// An operator message of the local door (`{"t":"load","state":…}`,
    /// `shape`, `at` with `to`, `cost`, `peer`) and its answer; the change
    /// it made is in `view` by then. Time travel is [`Self::travel`].
    pub async fn ask(&mut self, msg: Value) -> std::io::Result<Value> {
        self.w.send(frame(&msg)).await.map_err(std::io::Error::other)?;
        loop {
            let m = self.next().await?;
            if m["t"] == msg["t"] {
                return match m["err"].as_str() {
                    Some(e) => Err(std::io::Error::other(e.to_string())),
                    None => Ok(m["ok"].clone()),
                };
            }
            self.take(&m).await?;
        }
    }

    async fn next(&mut self) -> std::io::Result<Value> {
        next(&mut self.w)
            .await
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "the app closed the link"))
    }

    /// Fold a state or patch into the view; did it change? A gap or a
    /// patch that doesn't fit asks for the full view (never a guess).
    async fn take(&mut self, m: &Value) -> std::io::Result<bool> {
        let before = self.seq;
        match m["t"].as_str() {
            Some("state") => {
                self.seq = m["seq"].as_u64().unwrap_or(0);
                let changed = self.view != m["state"];
                self.view = m["state"].clone();
                Ok(changed)
            }
            Some("patch") if m["seq"].as_u64() > Some(before) => {
                let view = (m["prev"].as_u64() == Some(before)).then(|| patched(&self.view, m));
                let Some(Ok(view)) = view else {
                    self.w.send(frame(&json!({ "t": "resync" }))).await.map_err(std::io::Error::other)?;
                    return Ok(false);
                };
                self.seq = m["seq"].as_u64().unwrap_or(before);
                self.view = view;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

//! One link's session, the server's side: a client (a copy, a tool, a peer
//! app) says hello, gets what it may see of the cell and patches after each
//! change, and calls. The same protocol runs over a WebSocket (`sync`, with
//! `net`) and over the app's local door (`door`: frames on a private socket,
//! in every build), so this user's tools reach any running app.
use crate::patch;
use crate::{By, Cell, Door, Handle, Rejected, Result};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;

pub use crate::config::Limits;

pub(crate) fn lock<X>(m: &Mutex<X>) -> MutexGuard<'_, X> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// One frame of a link, whatever carries it.
pub(crate) enum Msg {
    Text(String),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    /// Closing, with a code (WebSocket's) and why.
    Close(u16, String),
}

/// A link's socket: the local door's frames, or (`net`) a WebSocket.
pub(crate) enum Sock {
    Door(crate::door::Frames),
    #[cfg(feature = "server")]
    Ws(axum::extract::ws::WebSocket),
    /// One link of a socket that carries several ([`crate::mux`]).
    #[cfg(feature = "server")]
    Chan(crate::mux::Chan),
}

impl Sock {
    /// The next frame; `None` when the link is gone. Cancel-safe: a frame
    /// is taken whole or not at all.
    async fn recv(&mut self) -> Option<Msg> {
        match self {
            Self::Door(f) => crate::door::recv(f).await,
            #[cfg(feature = "server")]
            Self::Ws(w) => {
                use axum::extract::ws::Message as M;
                match w.recv().await?.ok()? {
                    M::Text(t) => Some(Msg::Text(t.to_string())),
                    M::Ping(p) => Some(Msg::Ping(p.to_vec())),
                    M::Pong(p) => Some(Msg::Pong(p.to_vec())),
                    M::Binary(_) | M::Close(_) => None,
                }
            }
            #[cfg(feature = "server")]
            Self::Chan(c) => c.recv().await,
        }
    }

    pub(crate) async fn send(&mut self, m: Msg) -> std::io::Result<()> {
        match self {
            Self::Door(f) => crate::door::send(f, m).await,
            #[cfg(feature = "server")]
            Self::Ws(w) => {
                use axum::extract::ws::{CloseFrame, Message as M};
                let m = match m {
                    Msg::Text(t) => M::Text(t.into()),
                    Msg::Ping(p) => M::Ping(p.into()),
                    Msg::Pong(p) => M::Pong(p.into()),
                    Msg::Close(code, why) => M::Close(Some(CloseFrame { code, reason: why.into() })),
                };
                w.send(m).await.map_err(std::io::Error::other)
            }
            #[cfg(feature = "server")]
            Self::Chan(c) => c.send(m).await,
        }
    }

    /// Does this side ping the client? A WebSocket does (a silent link is
    /// dead within `DEAD`); the door doesn't need to: a process that goes
    /// away closes its socket, and a copy there pings, answered.
    fn pings(&self) -> bool {
        !matches!(self, Self::Door(_))
    }
}

/// A client's link to the app, opened or closed ([`crate::http::Http::on_link`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Presence {
    /// Opened (true) or closed.
    pub online: bool,
    /// The session's user with accounts; else who the client says (`peer`).
    pub user: String,
    /// The client's address.
    pub from: String,
}

pub(crate) type OnLink = Arc<dyn Fn(&Presence) + Send + Sync>;

/// Who another app on the door is to this one, by its id
/// ([`crate::http::Http::resolve_peer`]); `None`: no one in particular.
pub(crate) type Peers = Arc<dyn Fn(&str) -> Option<crate::Caller> + Send + Sync>;

/// What the app hears of its links, and who a peer app is to it.
#[derive(Clone, Default)]
pub(crate) struct Hooks {
    pub(crate) on_link: Option<OnLink>,
    pub(crate) peers: Option<Peers>,
}

/// A cell served beside the app's own on `/ws` ([`crate::http::Http::cell`]):
/// a link's hello names it.
#[cfg(feature = "server")]
pub(crate) trait Beside: Send + Sync {
    fn id(&self) -> &'static str;
    /// [`session`] on this cell, its hello read.
    #[allow(clippy::too_many_arguments)]
    fn session(
        &self,
        w: Sock,
        limits: Limits,
        from: String,
        gate: Gate,
        signed: Option<By>,
        calls: Calls,
        hooks: Hooks,
        hello: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = std::result::Result<(), String>> + Send>>;
}

#[cfg(feature = "server")]
impl<U: Cell> Beside for Handle<U> {
    fn id(&self) -> &'static str {
        U::ID
    }

    fn session(
        &self,
        w: Sock,
        limits: Limits,
        from: String,
        gate: Gate,
        signed: Option<By>,
        calls: Calls,
        hooks: Hooks,
        hello: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = std::result::Result<(), String>> + Send>> {
        Box::pin(session(self.clone(), w, limits, from, gate, signed, calls, hooks, Some(hello)))
    }
}

/// The caller `f` makes of peer app `app`; a hook that panics is said and
/// makes no one in particular.
fn resolve(f: &Peers, app: &str) -> Option<crate::Caller> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(app))).unwrap_or_else(|_| {
        eprintln!("[aio] sync: the resolve_peer hook panicked for app {app}; it is no one in particular");
        None
    })
}

/// What runs a link's server fns ([`crate::fns`]); a build without `net`
/// has none.
#[cfg(feature = "server")]
pub(crate) type Calls = crate::fns::Served;
#[cfg(not(feature = "server"))]
#[derive(Clone)]
pub(crate) struct Calls;

/// No server fns: a link that calls one is told so.
pub(crate) fn no_calls() -> Calls {
    #[cfg(feature = "server")]
    return crate::fns::Served { fns: Default::default(), account: None, known: None, from: (None, Default::default()) };
    #[cfg(not(feature = "server"))]
    Calls
}

/// A copy pings its server this often…
pub const PING: Duration = Duration::from_secs(2);
/// …and drops a link it has heard nothing on for this long.
pub const DEAD: Duration = Duration::from_secs(5);
/// A server drops a client that takes longer than this to take a message.
pub const SEND_TIMEOUT: Duration = Duration::from_secs(10);

/// Largest message either side accepts.
pub(crate) const MAX_MESSAGE: usize = 64 << 20;

/// A client link's pulse (aio's transport probe): when it was last heard,
/// and the round trip of the server's pings, graded by `[vitals]
/// transport`; a step up to warning or frozen is said in the log with
/// who it is, and the way back.
pub(crate) struct Pulse {
    pub(crate) heard: tokio::time::Instant,
    pinged: Option<tokio::time::Instant>,
    track: crate::vitals::Track,
    /// Its listed id and who it is, once known.
    pub(crate) link: Option<(u64, String)>,
}

impl Pulse {
    pub(crate) fn new() -> Self {
        Self { heard: tokio::time::Instant::now(), pinged: None, track: Default::default(), link: None }
    }

    /// Its pong came, `rtt` after the ping.
    fn pong(&mut self, rtt: Duration) {
        let ms = rtt.as_millis() as u64;
        let Some((id, who)) = &self.link else { return };
        // On loopback a stall is the client's own main thread, not the network.
        let loopback = lock(clients()).get_mut(id).is_some_and(|c| {
            c.rtt = Some(ms);
            c.from.parse::<std::net::SocketAddr>().is_ok_and(|a| a.ip().is_loopback())
        });
        let status = crate::vitals::get().transport.grade(ms);
        let step = self.track.step(status, crate::store::now_ms());
        crate::vitals::linked(*id, loopback, crate::vitals::Layer { status, measured: ms, since: self.track.since });
        if let Some(s) = step {
            crate::vitals::stepped("transport", s, &crate::vitals::transport_says(s, who, ms));
        }
    }
}

/// The next JSON message from a client; `None` when it's gone. Pings it
/// while waiting (its pong times the round trip: [`Pulse`]); a client not
/// heard from (any frame) for `DEAD` is gone.
pub(crate) async fn next_ax(w: &mut Sock, pulse: &mut Pulse, max: usize, bytes: &mut Pace) -> Option<Value> {
    let mut ping = tokio::time::Instant::now() + PING;
    loop {
        tokio::select! {
            m = w.recv() => {
                pulse.heard = tokio::time::Instant::now();
                match m? {
                    // Too big: say so (1009 names the limit), never a silent drop.
                    Msg::Text(t) if t.len() > max => {
                        let why = format!("message too big: {} bytes, the limit is {max}", t.len());
                        _ = w.send(Msg::Close(1009, why)).await;
                        return None;
                    }
                    Msg::Text(t) => {
                        // A fast sender is read more slowly (TCP holds the rest).
                        bytes.take_n(t.len() as f64).await;
                        return serde_json::from_str(&t).ok();
                    }
                    Msg::Pong(_) => {
                        if let Some(at) = pulse.pinged.take() {
                            pulse.pong(at.elapsed());
                        }
                    }
                    // A WebSocket answers pings itself; the door's are answered here.
                    Msg::Ping(p) if !w.pings() => w.send(Msg::Pong(p)).await.ok()?,
                    Msg::Ping(_) => continue,
                    Msg::Close(..) => return None,
                }
            }
            _ = tokio::time::sleep_until(ping), if w.pings() => {
                if pulse.heard.elapsed() > DEAD {
                    return None;
                }
                w.send(Msg::Ping(Vec::new())).await.ok()?;
                pulse.pinged.get_or_insert_with(tokio::time::Instant::now);
                ping += PING;
            }
        }
    }
}

/// Send to a client. One that stopped reading is dropped after
/// `SEND_TIMEOUT` instead of holding its session forever; it gets the
/// full state when it comes back.
pub(crate) async fn send_ax(w: &mut Sock, v: &Value) -> std::result::Result<(), String> {
    send_text(w, v.to_string()).await
}

/// [`send_ax`] of a state or patch, counted for cell `meter`'s cost and
/// its link's bytes (`meter`: the cell and the link).
pub(crate) async fn send_view(
    w: &mut Sock,
    meter: Option<(&'static str, u64)>,
    v: &Value,
) -> std::result::Result<(), String> {
    let text = v.to_string();
    if let Some((cell, link)) = meter {
        crate::cost::sent(cell, v, text.len());
        if let Some(c) = lock(clients()).get_mut(&link) {
            c.sent += text.len() as u64;
        }
    }
    send_text(w, text).await
}

pub(crate) async fn send_text(w: &mut Sock, text: String) -> std::result::Result<(), String> {
    match tokio::time::timeout(SEND_TIMEOUT, w.send(Msg::Text(text))).await {
        Ok(r) => r.map_err(|e| e.to_string()),
        Err(_) => Err(format!("the client stopped reading for {SEND_TIMEOUT:?}; dropped")),
    }
}

/// What one client has been sent: its view and the event it is at.
pub(crate) struct View {
    pub(crate) seq: u64,
    pub(crate) shown: Value,
    /// The view's JSON size when last measured (0: not yet), for [`whole`].
    pub(crate) size: usize,
}

impl View {
    pub(crate) fn of<T: Cell>(h: &Handle<T>, viewer: &crate::Caller) -> Self {
        let (seq, st) = h.snapshot();
        Self { seq, shown: st.project(viewer), size: 0 }
    }

    fn full(&self) -> Value {
        json!({ "t": "state", "seq": self.seq, "state": self.shown })
    }

    /// The patch from what was sent to what the viewer sees now, with the
    /// digest of the view it makes; `None` if nothing they may see changed.
    /// A patch over `full_percent` of the view is the view instead.
    fn advance<T: Cell>(&mut self, h: &Handle<T>, viewer: &crate::Caller, full_percent: u32) -> Option<Value> {
        let now = View::of(h, viewer);
        if now.seq <= self.seq {
            return None;
        }
        let ops = patch::diff(&self.shown, &now.shown);
        if ops.is_empty() {
            // Nothing visible changed: the client's patch chain stays as is.
            return None;
        }
        let mut size = self.size;
        let whole = whole(&ops, full_percent, &mut size, || patch::json_len(&now.shown));
        let msg = match whole {
            true => now.full(),
            false => {
                let digest = patch::digest(&now.shown);
                json!({ "t": "patch", "prev": self.seq, "seq": now.seq, "ops": ops, "digest": digest })
            }
        };
        *self = View { size, ..now };
        Some(msg)
    }
}

/// A patch shorter than this always goes as a patch: the view isn't weighed.
const WEIGH_FROM: usize = 256;

/// Is the view cheaper to send than `ops` (aio's `fullStateThreshold`): the
/// patch is over `percent` of the view's size. The view is measured
/// (`view`) only when the patch may be that big: a patch under half that
/// share of its size as last measured (`size`, kept here) is a patch.
pub(crate) fn whole(ops: &[patch::Op], percent: u32, size: &mut usize, view: impl FnOnce() -> usize) -> bool {
    if percent == 0 {
        return true;
    }
    let (n, p) = (patch::json_len(&ops) as u128, u128::from(percent));
    if n < WEIGH_FROM as u128 || (*size > 0 && n * 200 <= *size as u128 * p) {
        return false;
    }
    *size = view();
    n * 100 > *size as u128 * p
}

/// When a link's next change goes (aio's `syncIntervalMs`): the first of a
/// burst at once, then at most one per `every`, the changes between folded
/// into one patch; a change a person made (a user's or an agent's call)
/// goes at once. A link that is slow to take its messages is sent only the
/// newest view once it takes them again (backpressure: nothing queues).
pub(crate) struct Paced {
    every: Duration,
    last: Option<tokio::time::Instant>,
    due: Option<tokio::time::Instant>,
}

impl Paced {
    pub(crate) fn new(ms: u64) -> Self {
        Self { every: Duration::from_millis(ms), last: None, due: None }
    }

    /// A change came (`urgent`: a person's): send it now, or `false`: when
    /// [`Self::due`] ends.
    pub(crate) fn now(&mut self, urgent: bool) -> bool {
        let now = tokio::time::Instant::now();
        match self.last {
            Some(last) if !urgent && now < last + self.every => {
                self.due.get_or_insert(last + self.every);
                false
            }
            _ => true,
        }
    }

    /// The link is up to date as of now.
    pub(crate) fn sent(&mut self) {
        (self.last, self.due) = (Some(tokio::time::Instant::now()), None);
    }

    /// Ends when a held change is due; never, with none held.
    pub(crate) async fn due(&self) {
        match self.due {
            Some(at) => tokio::time::sleep_until(at).await,
            None => std::future::pending().await,
        }
    }
}

/// Did a person make the event (their change skips the pacing window)?
pub(crate) fn urgent(ev: &std::result::Result<(u64, crate::Event), RecvError>) -> bool {
    matches!(ev, Ok((_, e)) if matches!(e.by, By::User(_) | By::Agent(_)))
}

/// Who may call through one link. A debug build's local door: its tools may
/// call any method (the OS admits only this user); a release build's
/// (`Local`), a client with the key or a session: only `#[client]` methods
/// as their rules say, a session's role read again on each call (a changed
/// role counts at once).
#[derive(Clone)]
#[cfg_attr(not(feature = "server"), allow(dead_code))]
pub(crate) enum Gate {
    Door,
    /// A release build's door: any process of this user may open it, so it
    /// gets what a client gets, never the operator's rights.
    Local,
    Key,
    Signed(Arc<crate::auth::Auth>, String),
    /// A user the app knows by token ([`crate::auth::Users`]), with this role.
    Known(String),
}

impl Gate {
    /// A call by `viewer` through this link, `op` its synced id if any.
    /// Blocking: it may read the session.
    #[cfg_attr(not(all(feature = "compat", feature = "server")), allow(dead_code))]
    pub(crate) fn call<T: Cell>(
        &self,
        h: &Handle<T>,
        viewer: &By,
        method: &str,
        args: Value,
        op: Option<&str>,
    ) -> Result<Option<Value>> {
        let role = match self {
            Gate::Door => {
                return match op {
                    Some(op) => h.call_from(method, args, op),
                    None => h.call(method, args).map(Some),
                };
            }
            Gate::Local | Gate::Key => None,
            Gate::Known(role) => Some(role.clone()),
            Gate::Signed(a, t) => {
                Some(live(a, t).ok_or_else(|| Rejected::coded(crate::Code::AccessDenied, "sign in first: this session ended"))?.role)
            }
        };
        h.call_by(&crate::Caller { by: viewer.clone(), role }, method, args, op)
    }

    /// [`Self::call`], followed to its end: an async method's answer to come.
    pub(crate) fn ask<T: Cell>(
        &self,
        h: &Handle<T>,
        viewer: &By,
        method: &str,
        args: Value,
        op: Option<&str>,
    ) -> Result<(Option<Value>, crate::Answer)> {
        let role = match self {
            Gate::Door => return h.ask_by(None, method, args, op),
            Gate::Local | Gate::Key => None,
            Gate::Known(role) => Some(role.clone()),
            Gate::Signed(a, t) => {
                Some(live(a, t).ok_or_else(|| Rejected::coded(crate::Code::AccessDenied, "sign in first: this session ended"))?.role)
            }
        };
        h.ask_by(Some(&crate::Caller { by: viewer.clone(), role }), method, args, op)
    }

    /// The link's role now, for what it may see (`#[cell(visible = f)]`).
    /// Blocking: it may read the session.
    pub(crate) fn role(&self) -> Option<String> {
        match self {
            Gate::Door | Gate::Local | Gate::Key => None,
            Gate::Known(role) => Some(role.clone()),
            Gate::Signed(a, t) => live(a, t).map(|s| s.role),
        }
    }
}

/// Whether the accounts changed (`false`: they never will again); never,
/// without accounts.
pub(crate) async fn changed(rx: &mut Option<tokio::sync::watch::Receiver<u64>>) -> bool {
    match rx {
        Some(r) => r.changed().await.is_ok(),
        None => std::future::pending().await,
    }
}

/// One open link, for `ax clients`.
pub(crate) struct Client {
    pub(crate) cell: &'static str,
    pub(crate) user: String,
    /// `aio-rs`, `tool` or `aio`.
    pub(crate) wire: &'static str,
    /// The peer's address, or `door`.
    pub(crate) from: String,
    pub(crate) since: i64,
    /// Its last ping's round trip (ms), once one came back.
    pub(crate) rtt: Option<u64>,
    /// The bytes of the states and patches it was sent.
    pub(crate) sent: u64,
    /// Changes it fell behind on, and got as one newer view (its backpressure).
    pub(crate) behind: u64,
}

impl Client {
    /// A link opened now, nothing sent yet.
    pub(crate) fn new(cell: &'static str, user: String, wire: &'static str, from: String) -> Self {
        Self { cell, user, wire, from, since: crate::store::now_ms(), rtt: None, sent: 0, behind: 0 }
    }
}

/// Every open link of this process, by id.
pub(crate) fn clients() -> &'static Mutex<HashMap<u64, Client>> {
    static C: std::sync::OnceLock<Mutex<HashMap<u64, Client>>> = std::sync::OnceLock::new();
    C.get_or_init(Default::default)
}

/// A link listed while it lives.
pub(crate) struct Listed(pub(crate) u64);

impl Listed {
    pub(crate) fn new(c: Client) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        lock(clients()).insert(id, c);
        Self(id)
    }
}

impl Drop for Listed {
    fn drop(&mut self) {
        lock(clients()).remove(&self.0);
        crate::health::client_gone(self.0);
        crate::vitals::unlinked(self.0);
    }
}

/// Tells the app's [`crate::http::Http::on_link`] a link opened, and that
/// it closed when dropped.
pub(crate) struct Heard(Option<(OnLink, Presence)>);

impl Heard {
    pub(crate) fn new(hook: Option<OnLink>, user: &str, from: &str) -> Self {
        let link = Presence { online: true, user: user.into(), from: from.into() };
        Self(hook.inspect(|f| tell(f, &link)).map(|f| (f, link)))
    }
}

impl Drop for Heard {
    fn drop(&mut self) {
        if let Some((f, link)) = self.0.take() {
            tell(&f, &Presence { online: false, ..link });
        }
    }
}

/// Observe-only: a hook that panics is said, the link goes on.
pub(crate) fn tell(f: &OnLink, link: &Presence) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(link))).is_err() {
        eprintln!("[aio] sync: the on_link hook panicked (link {} by {}); the link goes on", link.from, link.user);
    }
}

/// A time-travel ask (`{cmd: undo|redo|goto|pause|resume|where, to}`) on
/// `h`, as `by`, answered with where the cell is then: `{at, head, floor,
/// paused, steps: [{seq, method, at, ms?}]}` (`ms`: the method's own time,
/// for calls of this run). A step pauses the cell, as `pause` does
/// ([`Handle::pause`]: calls refused, schedules held), so it stays where
/// travel put it.
pub(crate) fn travel<T: Cell>(h: &Handle<T>, by: &By, msg: &Value) -> Value {
    use crate::TimeAsk;
    let err = |e: String| json!({ "t": "travel", "err": e });
    let ask = match (msg["cmd"].as_str().unwrap_or("where"), msg["to"].as_u64()) {
        ("undo", _) => Some(TimeAsk::Undo),
        ("redo", _) => Some(TimeAsk::Redo),
        ("goto", Some(n)) => Some(TimeAsk::Goto(n)),
        ("pause", _) => Some(TimeAsk::Pause),
        ("resume", _) => Some(TimeAsk::Resume),
        ("where", _) => None,
        (c, _) => return err(format!("unknown time travel {c:?}: undo, redo, goto <seq>, pause, resume")),
    };
    match h.by(by.clone()).travel(ask) {
        Ok(tl) => {
            let steps: Vec<Value> = tl
                .steps
                .iter()
                .map(|e| {
                    let mut v = json!({ "seq": e.seq, "method": e.method, "at": e.at });
                    if let Some(d) = e.took {
                        v["ms"] = (d.as_secs_f64() * 1000.0).into();
                    }
                    v
                })
                .collect();
            json!({ "t": "travel", "at": tl.at, "head": tl.head, "floor": tl.floor, "paused": tl.paused, "steps": steps })
        }
        Err(e) => err(e.why),
    }
}

/// The open links to cell `cell`, oldest first.
pub(crate) fn listing(cell: &str) -> Value {
    let all = lock(clients());
    let mut ids: Vec<&u64> = all.iter().filter(|(_, c)| c.cell == cell).map(|(id, _)| id).collect();
    ids.sort();
    ids.into_iter()
        .map(|id| {
            let c = &all[id];
            let degraded: Vec<Value> = crate::health::client_parts(*id)
                .into_iter()
                .map(|(name, e)| json!({ "name": name, "failures": e.failures, "since": e.since, "lastError": e.last_error }))
                .collect();
            json!({
                "id": id, "user": c.user, "wire": c.wire, "from": c.from, "since": c.since, "rtt": c.rtt,
                "sent": c.sent, "behind": c.behind, "degraded": degraded,
            })
        })
        .collect()
}

/// `l` as an aio hello says it.
pub(crate) fn limits_wire(l: &Limits) -> Value {
    json!({ "maxMessage": l.max_message, "callsPerSec": l.calls_per_sec })
}

#[cfg(feature = "server")]
pub(crate) fn limits_from_wire(v: &Value) -> Option<Limits> {
    let max_message = usize::try_from(v["maxMessage"].as_u64()?).ok()?;
    let calls_per_sec = u32::try_from(v["callsPerSec"].as_u64()?).ok()?;
    Some(Limits { max_message, calls_per_sec, ..Limits::default() })
}

/// A token bucket: `take` waits for a token (it never refuses).
pub(crate) struct Pace {
    per_sec: f64,
    tokens: f64,
    last: tokio::time::Instant,
}

impl Pace {
    pub(crate) fn new(per_sec: u32) -> Self {
        let per_sec = f64::from(per_sec.max(1));
        Self { per_sec, tokens: per_sec * 2.0, last: tokio::time::Instant::now() }
    }

    pub(crate) async fn take(&mut self) {
        self.take_n(1.0).await;
    }

    /// Take `n` tokens, waiting as long as they take to come (more than a
    /// burst holds too: a big message waits its share).
    pub(crate) async fn take_n(&mut self, n: f64) {
        let now = tokio::time::Instant::now();
        self.tokens =
            (self.tokens + now.duration_since(self.last).as_secs_f64() * self.per_sec).min(self.per_sec * 2.0);
        self.last = now;
        if self.tokens < n {
            let wait = Duration::from_secs_f64((n - self.tokens) / self.per_sec);
            tokio::time::sleep(wait).await;
            self.tokens = n;
            self.last = tokio::time::Instant::now();
        }
        self.tokens -= n;
    }
}

/// The live session of `token`, or `None` (also when storage fails: closed).
pub(crate) fn live(a: &crate::auth::Auth, token: &str) -> Option<crate::auth::Session> {
    a.session(token).inspect_err(|e| eprintln!("[aio] sync: {e}")).ok().flatten()
}

/// A link's first message, its hello (`None`: none came in time).
pub(crate) async fn hello(w: &mut Sock, pulse: &mut Pulse, limits: &Limits, bytes: &mut Pace) -> Option<Value> {
    tokio::time::timeout(Duration::from_secs(15), next_ax(w, pulse, limits.max_message, bytes)).await.ok().flatten()
}

/// One aio-rs client, until it leaves. A tool (`Live`) says `tool: true`: it
/// keeps no copy, so another cell version is fine; on the door it may
/// also ask for the open links. `gate`: what it may call. `signed`: the user
/// its session proved; none: the user its hello names (a shared key proves
/// no one in particular), or for a peer app the caller `hooks` make of it.
/// `said`: its hello, when the server read it to find the cell.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn session<T: Cell>(
    h: Handle<T>,
    mut w: Sock,
    limits: Limits,
    from: String,
    gate: Gate,
    signed: Option<By>,
    #[cfg_attr(not(feature = "server"), allow(unused_mut, unused_variables))] mut calls: Calls,
    hooks: Hooks,
    said: Option<Value>,
) -> std::result::Result<(), String> {
    let mut pulse = Pulse::new();
    let mut pace = Pace::new(limits.calls_per_sec);
    let mut bytes = Pace::new(limits.bytes_per_sec);
    let hello = match said {
        Some(v) => v,
        None => hello(&mut w, &mut pulse, &limits, &mut bytes).await.ok_or("no hello")?,
    };
    if hello["t"] != "hello" || hello["cell"] != T::ID {
        _ = send_ax(&mut w, &json!({ "t": "refused", "why": format!("this server has cell {}", T::ID) })).await;
        return Err("refused a client: wrong cell".into());
    }
    let tool = hello["tool"] == true;
    // Another app on the door ([`crate::peer`]) is a client there: what the
    // key lets in (`#[client]` methods, `visible`), never the operator's.
    let peer = matches!(gate, Gate::Door | Gate::Local) && hello["peer"] == true;
    let mut gate = if peer { Gate::Key } else { gate };
    let mut said = hello["user"].as_str().map(|u| By::User(u.into()));
    // A peer says its app id; the app's hook says who that is here.
    if let (true, Some(f)) = (peer, &hooks.peers) {
        let c = resolve(f, hello["user"].as_str().unwrap_or_default());
        gate = match c.as_ref().and_then(|c| c.role.clone()) {
            Some(role) => Gate::Known(role),
            None => Gate::Key,
        };
        said = Some(c.as_ref().map_or_else(|| By::User("peer".into()), |c| c.by.clone()));
        #[cfg(feature = "server")]
        {
            calls.known = c;
        }
    }
    let door = matches!(gate, Gate::Door);
    let not_here = match gate {
        Gate::Local => "a release build's door serves a client's rights: the operator's tools need a debug build",
        _ => "only on the app's local door",
    };
    // Copies replay events with their own code: both sides must run the same
    // version of the cell, or the copy would drift.
    if !tool && hello["v"] != T::VERSION {
        let why = format!(
            "this server runs {} v{}, the app has v{} — update {}",
            T::ID,
            T::VERSION,
            hello["v"],
            if hello["v"].as_u64() < Some(T::VERSION.into()) { "the app" } else { "the server" }
        );
        _ = send_ax(&mut w, &json!({ "t": "refused", "why": why, "skew": true })).await;
        return Err(format!("refused a client: {why}"));
    }
    let viewer = match signed {
        // A client that predicts as someone else would make other ids: refused.
        Some(by) if said.as_ref().is_some_and(|s| *s != by) => {
            let By::User(me) = &by else { unreachable!("a session is a user") };
            let why = format!("this session is signed in as {me}");
            _ = send_ax(&mut w, &json!({ "t": "refused", "why": why })).await;
            return Err(format!("refused a client: {why}"));
        }
        Some(by) => by,
        None => said.unwrap_or_else(|| By::User("peer".into())),
    };
    let user = match &viewer {
        By::User(u) => u.clone(),
        _ => "peer".into(),
    };
    let wire = if tool { "tool" } else { "aio-rs" };
    // What a client is sent counts for `ax cost`; a tool's look doesn't.
    // A tool (`ax call`, the local door) is no client link.
    let _heard = Heard::new(hooks.on_link.filter(|_| !tool), &user, &from);
    let who = format!("{user} from {from}");
    let listed = Listed::new(Client::new(T::ID, user, wire, from));
    let meter = (!tool).then_some((T::ID, listed.0));
    pulse.link = (!tool).then(|| (listed.0, who.clone()));
    // What it sees goes by its role too, read again on every account change
    // (heard before the read: no change between them is missed).
    let mut accounts = match &gate {
        Gate::Signed(a, _) => Some(a.changes()),
        _ => None,
    };
    let role = |g: &Gate| {
        let g = g.clone();
        tokio::task::spawn_blocking(move || g.role())
    };
    let mut seer = crate::Caller { by: viewer.clone(), role: role(&gate).await.map_err(|e| e.to_string())? };
    // Subscribe before the snapshot: nothing between them is lost.
    let mut events = h.events();
    let mut tells = h.tells();
    let mut view = View::of(&h, &seer);
    let mut first = view.full();
    first["limits"] = limits_wire(&limits);
    first["v"] = T::VERSION.into();
    // Who the copy calls as: a page learns it here (its session's user).
    first["user"] = match &viewer {
        By::User(u) => u.as_str(),
        _ => "peer",
    }
    .into();
    let mut pushes = crate::budgets::Pushes::default();
    let mut paced = Paced::new(limits.sync_interval_ms);
    pushes.push(&first, crate::store::now_ms());
    send_view(&mut w, meter, &first).await?;
    let h = h.by(viewer.clone());
    // Server fns run beside the link; dropped with it, they are cancelled.
    let mut running = tokio::task::JoinSet::<Value>::new();
    let mut relayed = (0, 0);
    // Their parts, in order; a call's parts all go before its answer.
    #[cfg_attr(not(feature = "server"), allow(unused_variables))]
    let (parts_tx, mut parts) = tokio::sync::mpsc::channel::<Value>(64);
    // What they say while running: a person is deciding (`deciding::start`).
    #[cfg_attr(not(feature = "server"), allow(unused_variables))]
    let (held_tx, mut held) = tokio::sync::mpsc::unbounded_channel::<Value>();
    // Async methods' ends (`end {op, ok | err}`), each after its last change.
    let (ends_tx, mut ends) = tokio::sync::mpsc::unbounded_channel::<Value>();
    loop {
        tokio::select! {
            Some(Ok(answer)) = running.join_next(), if !running.is_empty() => {
                while let Ok(p) = parts.try_recv() {
                    send_ax(&mut w, &p).await?;
                }
                send_ax(&mut w, &answer).await?;
            }
            Some(p) = parts.recv() => send_ax(&mut w, &p).await?,
            Some(m) = held.recv() => send_ax(&mut w, &m).await?,
            Some(end) = ends.recv() => {
                while events.try_recv().is_ok() {}
                if let Some(p) = view.advance(&h, &seer, limits.full_state_percent) {
                    pushes.push(&p, crate::store::now_ms());
                    send_view(&mut w, meter, &p).await?;
                }
                send_ax(&mut w, &end).await?;
            }
            Ok(t) = tells.recv(), if !tool => send_ax(&mut w, &json!({ "t": "tell", "d": t })).await?,
            more = changed(&mut accounts) => {
                if !more {
                    accounts = None;
                }
                let now = role(&gate).await.map_err(|e| e.to_string())?;
                // A new role sees another view: sent whole.
                if now != seer.role {
                    seer.role = now;
                    view = View::of(&h, &seer);
                    send_view(&mut w, meter, &view.full()).await?;
                }
            }
            () = paced.due() => {
                paced.sent();
                if let Some(p) = view.advance(&h, &seer, limits.full_state_percent) {
                    pushes.push(&p, crate::store::now_ms());
                    send_view(&mut w, meter, &p).await?;
                }
            }
            ev = events.recv() => match ev {
                Err(RecvError::Closed) => return Ok(()),
                // Held for the window, or now; a slow client just gets the newest view.
                ev => {
                    if let Err(RecvError::Lagged(n)) = &ev
                        && let Some(c) = lock(clients()).get_mut(&listed.0)
                    {
                        c.behind += *n;
                    }
                    if paced.now(urgent(&ev)) {
                        paced.sent();
                        if let Some(p) = view.advance(&h, &seer, limits.full_state_percent) {
                            pushes.push(&p, crate::store::now_ms());
                            send_view(&mut w, meter, &p).await?;
                        }
                    }
                }
            },
            msg = next_ax(&mut w, &mut pulse, limits.max_message, &mut bytes) => {
                let Some(msg) = msg else { return Ok(()) };
                // A fast caller is read more slowly (TCP holds the rest): nothing dropped.
                if msg["t"] == "call" || msg["t"] == "sfn" {
                    pace.take().await;
                }
                match msg["t"].as_str() {
                    #[cfg(feature = "server")]
                    Some("sfn") => {
                        let (id, name) = (msg["id"].clone(), msg["name"].as_str().unwrap_or_default().to_string());
                        let chunks = match msg["stream"] == true {
                            true => crate::fns::Chunks::to(parts_tx.clone(), id.clone()),
                            false => crate::fns::Chunks::default(),
                        };
                        let (held, i) = (held_tx.clone(), id.clone());
                        let tell = move |hold| _ = held.send(json!({ "t": "sfnh", "id": i, "hold": hold }));
                        calls.start(&mut running, name, msg["args"].clone(), chunks, tell, move |r| match r {
                            Ok(v) => json!({ "t": "sfnr", "id": id, "ok": v }),
                            Err(f) => json!({ "t": "sfnr", "id": id, "err": f.why, "code": f.code }),
                        });
                    }
                    #[cfg(not(feature = "server"))]
                    Some("sfn") => {
                        let why = format!("server fn {} is not served by this app", msg["name"]);
                        send_ax(&mut w, &json!({ "t": "sfnr", "id": msg["id"], "err": why, "code": null })).await?;
                    }
                    // Closing: drop the link without an answer, so the client
                    // keeps the call and re-sends it to the next run.
                    Some("call") if h.closing() => return Ok(()),
                    Some("call") => {
                        let (hh, g, by) = (h.clone(), gate.clone(), viewer.clone());
                        let (method, args) = (msg["method"].as_str().unwrap_or_default().to_string(), msg["args"].clone());
                        let op = msg["op"].as_str().map(String::from);
                        let is_async = T::async_mode(&method).is_some();
                        let r = tokio::task::spawn_blocking(move || g.ask(&hh, &by, &method, args, op.as_deref()))
                            .await
                            .map_err(|e| e.to_string())?;
                        // An async method's end goes after its reply, once it comes.
                        let r = match r {
                            Ok((v, answer)) => {
                                if is_async {
                                    let (tx, op) = (ends_tx.clone(), msg["op"].clone());
                                    tokio::spawn(async move {
                                        let end = match answer.await {
                                            Ok(v) => json!({ "t": "end", "op": op, "ok": v }),
                                            Err(e) => json!({ "t": "end", "op": op, "err": e.why, "code": e.code }),
                                        };
                                        _ = tx.send(end);
                                    });
                                }
                                Ok(v.unwrap_or(Value::Null))
                            }
                            Err(e) => Err(e),
                        };
                        // Its change first, then the reply; named, so its copy
                        // knows this change is its own (no conflict with itself).
                        while events.try_recv().is_ok() {}
                        if let Some(mut p) = view.advance(&h, &seer, limits.full_state_percent) {
                            if msg["op"].is_string() {
                                p["op"] = msg["op"].clone();
                            }
                            pushes.push(&p, crate::store::now_ms());
                            send_view(&mut w, meter, &p).await?;
                        }
                        let mut reply = match r {
                            Ok(v) => json!({ "t": "reply", "op": msg["op"], "ok": v }),
                            Err(e) => json!({ "t": "reply", "op": msg["op"], "err": e.why, "code": e.code }),
                        };
                        if is_async && reply.get("ok").is_some() {
                            // Its answer is the `end` that follows.
                            reply["async"] = true.into();
                        }
                        send_ax(&mut w, &reply).await?;
                    }
                    // What is degraded on the client's side, for this app's health (aio's `cdiag`).
                    Some("diag") => crate::health::client_says(listed.0, &who, &msg["down"]),
                    // What the client's own bus heard, onto this one (aio's diagnostic bus).
                    Some("heard") => crate::report::relay(&who, &msg, &mut relayed),
                    Some("resync") => {
                        view = View::of(&h, &seer);
                        send_view(&mut w, meter, &view.full()).await?;
                    }
                    // Time travel (aio's `tt-cmd`), the app's own tools only.
                    Some("travel") => {
                        let reply = if door {
                            let (hh, by) = (h.clone(), viewer.clone());
                            tokio::task::spawn_blocking(move || travel(&hh, &by, &msg)).await.map_err(|e| e.to_string())?
                        } else {
                            json!({ "t": "travel", "err": not_here })
                        };
                        send_ax(&mut w, &reply).await?;
                    }
                    // Who else is linked is the operator's to know: this user's tools only.
                    Some("clients") => {
                        let reply = if door {
                            json!({ "t": "clients", "list": listing(T::ID) })
                        } else {
                            json!({ "t": "clients", "err": not_here })
                        };
                        send_ax(&mut w, &reply).await?;
                    }
                    // A loaded snapshot, the saved shape, diffs, cost, fields, writes: the operator's tools only.
                    // What it may call is any caller's to ask, of itself (aio's `am peer <url>`).
                    Some(t @ ("load" | "shape" | "at" | "cost" | "fields" | "flush" | "drop")) if !door => {
                        send_ax(&mut w, &json!({ "t": t, "err": not_here })).await?;
                    }
                    Some(t @ ("load" | "shape" | "at" | "cost" | "peer" | "fields" | "flush" | "drop")) => {
                        let ask = match door {
                            true => msg.clone(),
                            false => {
                                let me = if let By::User(u) = &seer.by { u.as_str() } else { "peer" };
                                json!({ "t": "peer", "user": me, "role": seer.role })
                            }
                        };
                        let (hh, by) = (h.clone(), viewer.clone());
                        let r = tokio::task::spawn_blocking(move || operate(&hh, &by, &ask))
                            .await
                            .map_err(|e| e.to_string())?;
                        while events.try_recv().is_ok() {}
                        if let Some(p) = view.advance(&h, &seer, limits.full_state_percent) {
                            pushes.push(&p, crate::store::now_ms());
                            send_view(&mut w, meter, &p).await?;
                        }
                        let reply = match r {
                            Ok(v) => json!({ "t": t, "ok": v }),
                            Err(e) => json!({ "t": t, "err": e }),
                        };
                        send_ax(&mut w, &reply).await?;
                    }
                    _ => return Err(format!("unknown message {msg}")),
                }
            }
        }
    }
}

/// What a network caller (`ask`'s `user`, with its `role`) may call of `st`
/// (aio's `peer().describe()`): each method, its signature, and the answer of
/// its `#[client]` rule, asked with no arguments: one that reads them says
/// it depends on them (pure).
pub(crate) fn peer<T: Cell>(st: &T, ask: &Value) -> Value {
    let who = crate::Caller {
        by: By::User(ask["user"].as_str().unwrap_or("peer").into()),
        role: ask["role"].as_str().map(String::from),
    };
    let methods: Vec<Value> = T::METHODS
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let may = match st.allow(&who, m, &Value::Null) {
                Ok(()) => "yes".to_string(),
                Err(e) if e.code == Some(crate::Code::AccessDenied) => format!("no: {}", e.why),
                Err(_) => "by its arguments (its allow rule reads them)".into(),
            };
            json!({ "method": m, "sig": T::SIGNATURES.get(i), "client": T::CLIENT.contains(m), "may": may })
        })
        .collect();
    json!({ "cell": T::ID, "v": T::VERSION, "methods": methods })
}

/// `ax events --json`'s live readings of `h` (an operator's `took`): the
/// time each of events `seqs` took, when it ran in this run (`ms`, by seq),
/// and the cell's refused calls of late (`refused`: `{at, call, ms,
/// refused}`, never their arguments).
fn took<T: Cell>(h: &Handle<T>, ask: &Value) -> Value {
    let seqs: Vec<u64> = ask["seqs"].as_array().into_iter().flatten().filter_map(Value::as_u64).collect();
    let ms: serde_json::Map<String, Value> = h.store(|s| {
        seqs.iter().filter_map(|q| Some((q.to_string(), (s.took(*q)?.as_secs_f64() * 1000.0).into()))).collect()
    });
    let mine = format!("{}.", T::ID);
    let refused: Vec<Value> = crate::report::timeline()
        .into_iter()
        .filter(|e| e["refused"].is_string() && e["call"].as_str().is_some_and(|c| c.starts_with(&mine)))
        .collect();
    json!({ "ms": ms, "refused": refused })
}

/// What a cell saves and sends, field by field (aio's trojan `fields`): saved
/// at all (`persist`), its `#[transient]` fields (kept, never saved) and its
/// `#[server]` ones (saved, never sent to a client) (pure).
pub(crate) fn fields<T: Cell>() -> Value {
    json!({ "persist": T::PERSIST, "transient": T::TRANSIENT, "server": T::SERVER })
}

/// A local door's operator message `ask` on `h`, by `by`: `load` a state,
/// the `shape` this version saves, the state right after an event (`at`, for diffs) or
/// what the cell has sent its clients (`cost`), how long events of this
/// run took and its refused calls (`took`), what a caller may call
/// (`peer`), which fields are saved and which are sent (`fields`), write
/// what is held now (`flush`), `drop` a stored cell nothing declares.
/// The answer, or why not.
pub(crate) fn operate<T: Cell>(h: &Handle<T>, by: &By, ask: &Value) -> std::result::Result<Value, String> {
    match ask["t"].as_str() {
        Some("fields") => Ok(fields::<T>()),
        Some("load") => {
            h.store(|s| s.load(by, ask["state"].clone())).map_err(|e| e.why)?;
            Ok(h.snapshot().0.into())
        }
        Some("cost") => Ok(crate::cost::report(T::ID, ask["window"].as_i64())),
        Some("took") => Ok(took(h, ask)),
        // `ax persist`: what is held (a coalesced run) written now.
        Some("flush") => h.store(|s| s.flush()).map(|_| json!(h.snapshot().0)).map_err(|e| e.why),
        // `ax persist --drop`: a stored cell no cell of this app declares.
        Some("drop") => {
            let cell = ask["cell"].as_str().ok_or("drop: `cell` names the stored cell")?;
            if crate::report::own(cell) {
                return Err(format!("{cell} is a cell of this app: only a cell no build declares is dropped"));
            }
            h.store(|s| s.drop_cell(cell)).map(Value::Bool).map_err(|e| e.0)
        }
        Some("peer") => Ok(peer::<T>(&h.state(), ask)),
        Some("at") => {
            let at = ask["to"].as_u64().ok_or("at: `to` names an event (a number)")?;
            let st = h.store(|s| s.state_at(at)).map_err(|e| e.0)?;
            serde_json::to_value(&st).map_err(|e| e.to_string())
        }
        _ => h.store(|s| s.shape()).map(|shape| json!({ "v": T::VERSION, "shape": shape })).map_err(|e| e.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_view_is_weighed_only_when_a_patch_may_be_its_share() {
        let op = |n: usize| vec![patch::Op::Set { path: vec![], value: json!("x".repeat(n)) }];
        let never = || -> usize { panic!("measured") };
        let mut size = 0;
        assert!(whole(&op(1), 0, &mut size, never), "0: always the view");
        assert!(!whole(&op(10), 50, &mut size, never), "a small patch is a patch, unmeasured");
        assert!(whole(&op(600), 50, &mut size, || 1000) && size == 1000, "over half of 1000");
        assert!(!whole(&op(300), 50, &mut size, || 1000), "under half: measured, a patch");
        size = 100_000;
        assert!(!whole(&op(1000), 50, &mut size, never), "far under its share of the last size");
        assert!(whole(&op(40_000), 50, &mut size, || 50_000), "the view shrank: measured again");
        assert_eq!(size, 50_000);
    }
}

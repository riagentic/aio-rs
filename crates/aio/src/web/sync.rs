//! A synced copy in the page: the native copy's protocol (`/ws`, a snapshot
//! then patches, predicted calls confirmed by the server) over the page's
//! own socket. What it shows is the confirmed copy plus its calls still
//! waiting; those wait in the browser's storage too ([`super::storage`]),
//! so a call made offline survives a reload and is sent once the page is
//! linked again, and a twin tab of the same app shows and sends it as well
//! (the server applies a call id once). A dropped link is retried with
//! backoff and jitter (at most 10 s).
use super::storage::{self, Doc, Queued};
use super::{Local, say};
use crate::patch::{self, Op};
use crate::{By, Cell, Cx, Door, Rejected, Result, Status};
use serde_json::{Value, json};
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};

/// Calls waiting for the server, at most (as a native copy); then calls are refused.
pub const PENDING_MAX: usize = 500;
/// The answered calls a tab remembers, so its writes never put one back.
const DONE_MAX: usize = 1024;

fn lock<X>(m: &Mutex<X>) -> MutexGuard<'_, X> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

type Listener<T> = Arc<dyn Fn(&Arc<T>) + Send + Sync>;
type Refusal = Arc<dyn Fn(&str, &Rejected) + Send + Sync>;
type OnConflict = Arc<dyn Fn(&[crate::merge::Conflict]) + Send + Sync>;

struct Copy<T> {
    seq: u64,
    /// The server's view as sent (patches apply to this).
    view: Value,
    confirmed: Arc<T>,
    pending: Vec<Queued>,
    shown: Arc<T>,
    /// Who it calls as: the server names it on the first state.
    user: String,
    /// The server's view (and state) the oldest waiting call was made on.
    base: Option<(Value, Arc<T>)>,
    /// Fields shown merged ([`Remote::merge`]).
    merges: Vec<(String, crate::merge::Merge)>,
}

/// `method` on `st` as the server runs it: its machine, then the method,
/// then the invariants.
fn apply<T: Cell>(st: &mut T, cx: &Cx, method: &str, args: Value) -> Result<Value> {
    st.advance(method)?;
    let out = st.apply(cx, method, args)?;
    st.check()?;
    Ok(out)
}

impl<T: Cell> Copy<T> {
    /// The confirmed copy + the calls still waiting (one that no longer
    /// applies is skipped; the server will refuse it), the fields of
    /// [`Remote::merge`] both changed shown merged.
    fn reshow(&mut self) {
        match self.pending.is_empty() {
            true => self.base = None,
            false => _ = self.base.get_or_insert_with(|| (self.view.clone(), self.confirmed.clone())),
        }
        let st = self.replayed(&self.confirmed);
        self.shown = Arc::new(match &self.base {
            Some((view, base)) if !self.merges.is_empty() => {
                match (serde_json::to_value(self.replayed(base)), serde_json::to_value(&st)) {
                    (Ok(local), Ok(mut shown)) => {
                        crate::merge::show(&self.merges, view, &local, &self.view, &mut shown);
                        crate::copy_of::<T>(shown).unwrap_or(st)
                    }
                    _ => st,
                }
            }
            _ => st,
        });
    }

    /// The calls still waiting, replayed on `from`.
    fn replayed(&self, from: &T) -> T {
        let by = By::User(self.user.clone());
        let mut st = from.clone();
        for q in self.pending.iter().filter(|q| !q.waits) {
            let mut next = st.clone();
            if apply(&mut next, &Cx::replay(q.at, q.inputs.clone(), &by), &q.method, q.args.clone()).is_ok() {
                st = next;
            }
        }
        st
    }

    /// Before a server view comes: what to check it against for conflicts
    /// (its view, what it shows), unless no call waits or `own` is one of them.
    fn before(&self, own: &Value) -> Option<(Value, Value)> {
        if self.pending.is_empty() || self.pending.iter().any(|q| own.as_str() == Some(q.op.as_str())) {
            return None;
        }
        Some((self.view.clone(), serde_json::to_value(&*self.shown).ok()?))
    }

    fn doc(&self) -> Doc {
        Doc {
            cell: T::ID.into(),
            v: T::VERSION,
            user: self.user.clone(),
            seq: self.seq,
            view: self.view.clone(),
            pending: self.pending.clone(),
            sched: Vec::new(),
        }
    }
}

struct Inner<T: Cell> {
    key: String,
    copy: Mutex<Copy<T>>,
    /// The calls this tab saw answered, newest last.
    done: Mutex<VecDeque<String>>,
    out: Mutex<Option<UnboundedSender<String>>>,
    online: AtomicBool,
    last_sync: Mutex<Option<i64>>,
    /// A refused write was said (once, until one lands again).
    unsaved: AtomicBool,
    listeners: Mutex<Vec<Listener<T>>>,
    refusals: Mutex<Vec<Refusal>>,
    conflicts: Mutex<Vec<OnConflict>>,
    owned: Mutex<std::collections::HashMap<String, Box<dyn std::any::Any + Send>>>,
    /// Async methods' callers waiting for their end (`end` frames), by call.
    ends: Mutex<std::collections::HashMap<String, crate::Reply>>,
}

/// A synced copy of a server's cell in the page. Reads and calls are local
/// at once; the server confirms. Reconnects on its own.
pub struct Remote<T: Cell> {
    inner: Arc<Inner<T>>,
}

impl<T: Cell> Clone for Remote<T> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

/// What the page's socket says.
enum Heard {
    Open,
    Text(String),
    Closed,
}

/// One link to the server: the socket and what it hears, closed when dropped.
struct Link {
    ws: web_sys::WebSocket,
    rx: UnboundedReceiver<Heard>,
    _cbs: [Closure<dyn FnMut(JsValue)>; 3],
}

impl Drop for Link {
    fn drop(&mut self) {
        self.ws.set_onopen(None);
        self.ws.set_onmessage(None);
        self.ws.set_onclose(None);
        _ = self.ws.close();
    }
}

/// Why no link was made.
enum Failed {
    /// Not reachable now: tried again later.
    Offline(String),
    /// The server runs another version of the cell: tried again (it may be updated).
    Skew(String),
    /// The server won't have this copy: not tried again.
    Refused(String),
}

impl Link {
    async fn open(url: &str, hello: &Value) -> std::result::Result<(Self, Value), Failed> {
        let ws = web_sys::WebSocket::new(url).map_err(|e| Failed::Offline(format!("{e:?}")))?;
        let (tx, rx) = unbounded_channel();
        let on = |f: fn(JsValue) -> Heard| {
            let tx = tx.clone();
            Closure::<dyn FnMut(JsValue)>::new(move |e| _ = tx.send(f(e)))
        };
        let cbs = [
            on(|_| Heard::Open),
            on(|e| match e.dyn_into::<web_sys::MessageEvent>().ok().and_then(|m| m.data().as_string()) {
                Some(t) => Heard::Text(t),
                None => Heard::Closed,
            }),
            on(|_| Heard::Closed),
        ];
        ws.set_onopen(Some(cbs[0].as_ref().unchecked_ref()));
        ws.set_onmessage(Some(cbs[1].as_ref().unchecked_ref()));
        ws.set_onclose(Some(cbs[2].as_ref().unchecked_ref()));
        let mut l = Self { ws, rx, _cbs: cbs };
        match l.rx.recv().await {
            Some(Heard::Open) => {}
            _ => return Err(Failed::Offline("the socket closed".into())),
        }
        l.ws.send_with_str(&hello.to_string()).map_err(|e| Failed::Offline(format!("{e:?}")))?;
        let first = match l.rx.recv().await {
            Some(Heard::Text(t)) => serde_json::from_str::<Value>(&t).map_err(|e| Failed::Offline(e.to_string()))?,
            _ => return Err(Failed::Offline("the socket closed".into())),
        };
        let why = || format!("refused: {}", first["why"].as_str().unwrap_or("?"));
        match () {
            () if first["t"] == "state" => Ok((l, first)),
            () if first["skew"] == true => Err(Failed::Skew(why())),
            () => Err(Failed::Refused(why())),
        }
    }
}

impl<T: Cell> Remote<T> {
    /// App `app`'s copy of `T`, synced with the server that served this
    /// page. It starts from the copy this browser kept when there is one
    /// (offline-first: linked later); else it waits for the server's.
    pub async fn open(app: &str) -> std::io::Result<Self> {
        let loc = web_sys::window().ok_or_else(|| std::io::Error::other("no window"))?.location();
        let url = super::ws_url(
            &loc.protocol().unwrap_or_default(),
            &loc.host().unwrap_or_default(),
            &loc.search().unwrap_or_default(),
        );
        Self::open_at(app, &url).await
    }

    /// [`Self::open`] with the server at `url` (`wss://host/ws?wire=2…`).
    pub async fn open_at(app: &str, url: &str) -> std::io::Result<Self> {
        let key = storage::key(app, T::ID);
        super::syncing(app, T::ID);
        let saved = storage::load(&Local, &key, say).filter(|d| d.cell == T::ID && d.v == T::VERSION);
        let hello = json!({ "t": "hello", "cell": T::ID, "v": T::VERSION });
        let first = match Link::open(url, &hello).await {
            Ok(l) => Some(l),
            Err(Failed::Offline(e)) if saved.is_some() => {
                say(&format!("sync {}: {e}; starting offline from the saved copy", T::ID));
                None
            }
            Err(Failed::Offline(e) | Failed::Skew(e) | Failed::Refused(e)) => return Err(std::io::Error::other(e)),
        };
        let (seq, view, user, pending) = match (&first, saved) {
            (Some((_, m)), saved) => {
                let user = m["user"].as_str().unwrap_or("peer").to_string();
                // Another user's calls are never sent as this one (they stay stored).
                let pending = saved.filter(|s| s.user == user).map(|s| s.pending).unwrap_or_default();
                (m["seq"].as_u64().unwrap_or(0), m["state"].clone(), user, pending)
            }
            (None, Some(s)) => (s.seq, s.view, s.user, s.pending),
            (None, None) => unreachable!("offline without a saved copy returned above"),
        };
        let st: Arc<T> = Arc::new(crate::copy_of(view.clone()).map_err(std::io::Error::other)?);
        let mut copy =
            Copy { seq, view, confirmed: st.clone(), pending, shown: st, user, base: None, merges: Vec::new() };
        copy.reshow();
        let connected = first.is_some();
        let inner = Arc::new(Inner {
            key,
            copy: Mutex::new(copy),
            done: Mutex::default(),
            out: Mutex::default(),
            online: AtomicBool::new(false),
            last_sync: Mutex::new(connected.then(super::now_ms)),
            unsaved: AtomicBool::new(false),
            listeners: Mutex::default(),
            refusals: Mutex::default(),
            conflicts: Mutex::default(),
            owned: Mutex::default(),
            ends: Mutex::default(),
        });
        let r = Self { inner };
        r.mirror_twins();
        r.update(|_| {});
        let (weak, url) = (Arc::downgrade(&r.inner), url.to_string());
        wasm_bindgen_futures::spawn_local(run(weak, url, hello, first.map(|(l, _)| l)));
        // Online once the pump runs.
        for _ in 0..if connected { 20 } else { 0 } {
            if r.online() {
                break;
            }
            super::sleep(5).await;
        }
        Ok(r)
    }

    /// A twin tab's write: the calls it queued that this tab lacks are
    /// shown and sent here too.
    fn mirror_twins(&self) {
        let (weak, key) = (Arc::downgrade(&self.inner), self.inner.key.clone());
        let heard = Closure::<dyn FnMut(web_sys::StorageEvent)>::new(move |e: web_sys::StorageEvent| {
            let Some(inner) = weak.upgrade().filter(|_| e.key().as_deref() == Some(&key)) else { return };
            let Some(theirs) = e.new_value().and_then(|v| serde_json::from_str::<Doc>(&v).ok()) else { return };
            let r = Remote { inner };
            let new = {
                let (c, done) = (lock(&r.inner.copy), lock(&r.inner.done));
                storage::adopt(&c.pending, &theirs, &c.user, &done.iter().cloned().collect())
            };
            if new.is_empty() {
                return;
            }
            r.update(|c| {
                c.pending.extend(new.iter().cloned());
                c.pending.sort_by(|a, b| (a.at, &a.op).cmp(&(b.at, &b.op)));
            });
            new.iter().for_each(|q| r.send(q.msg()));
        });
        if let Some(w) = web_sys::window() {
            _ = w.add_event_listener_with_callback("storage", heard.as_ref().unchecked_ref());
        }
        // Heard for the page's life (a dropped copy's is a no-op).
        heard.forget();
    }

    /// One message from the server.
    fn take(&self, m: Value) {
        if matches!(m["t"].as_str(), Some("state" | "patch" | "reply")) {
            *lock(&self.inner.last_sync) = Some(super::now_ms());
        }
        match m["t"].as_str() {
            Some("state") => match crate::copy_of::<T>(m["state"].clone()) {
                Ok(st) => {
                    let before = lock(&self.inner.copy).before(&m["op"]);
                    let met = self.met(before, &m["state"]);
                    self.update(|c| {
                        c.seq = m["seq"].as_u64().unwrap_or(0);
                        c.view = m["state"].clone();
                        c.confirmed = Arc::new(st);
                    });
                    self.conflicted(&met);
                }
                Err(e) => say(&format!("sync {}: bad state: {e}", T::ID)),
            },
            Some("patch") => {
                let (prev, seq) = (m["prev"].as_u64().unwrap_or(0), m["seq"].as_u64().unwrap_or(0));
                let (mine, view, before) = {
                    let c = lock(&self.inner.copy);
                    (c.seq, c.view.clone(), c.before(&m["op"]))
                };
                if seq <= mine {
                    return;
                }
                if prev != mine {
                    return self.send(json!({ "t": "resync" })); // a gap: never guess
                }
                let st = patched(&view, &m)
                    .and_then(|v| crate::copy_of::<T>(v.clone()).map(|st| (v, st)).map_err(|e| e.to_string()));
                match st {
                    Ok((view, st)) => {
                        let met = self.met(before, &view);
                        self.update(|c| {
                            c.seq = seq;
                            c.view = view;
                            c.confirmed = Arc::new(st);
                        });
                        self.conflicted(&met);
                    }
                    Err(e) => {
                        say(&format!("sync {}: patch to {seq} doesn't fit ({e}): resync", T::ID));
                        self.send(json!({ "t": "resync" }));
                    }
                }
            }
            Some("reply") => {
                let op = m["op"].as_str().unwrap_or_default().to_string();
                let method = lock(&self.inner.copy).pending.iter().find(|q| q.op == op).map(|q| q.method.clone());
                {
                    let mut done = lock(&self.inner.done);
                    done.push_back(op.clone());
                    if done.len() > DONE_MAX {
                        done.pop_front();
                    }
                }
                self.update(|c| c.pending.retain(|q| q.op != op));
                if let Some(e) = m.get("err").and_then(Value::as_str) {
                    let e = Rejected { why: e.into(), code: crate::Code::of(&m["code"]) };
                    if let Some(end) = lock(&self.inner.ends).remove(&op) {
                        end.send(Err(e.clone()));
                    }
                    if let Some(method) = method {
                        say(&format!("sync {}: the server refused {method}: {e}", T::ID));
                        let fs = lock(&self.inner.refusals).clone();
                        fs.iter().for_each(|f| f(&method, &e));
                    }
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
            Some("tell") => match serde_json::from_value::<crate::tell::Tell>(m["d"].clone()) {
                Ok(t) => _ = crate::tell::show(&t),
                Err(e) => say(&format!("sync {}: a tell this build can't read: {e}", T::ID)),
            },
            _ => {}
        }
    }

    fn send(&self, m: Value) {
        if let Some(tx) = lock(&self.inner.out).as_ref() {
            _ = tx.send(m.to_string());
        }
    }

    /// Change the copy, recompute what's shown, keep it in the browser,
    /// tell the listeners.
    fn update(&self, f: impl FnOnce(&mut Copy<T>)) {
        let doc = {
            let mut c = lock(&self.inner.copy);
            f(&mut c);
            c.reshow();
            c.doc()
        };
        let done: HashSet<String> = lock(&self.inner.done).iter().cloned().collect();
        match storage::save(&Local, &self.inner.key, doc, &done, say) {
            Ok(()) => self.inner.unsaved.store(false, Ordering::Relaxed),
            Err(e) if !self.inner.unsaved.swap(true, Ordering::Relaxed) => say(&format!(
                "sync {}: the copy can't be kept in this browser ({e}): it goes on in this page, but its unsent \
                 calls won't survive a reload",
                T::ID
            )),
            Err(_) => {}
        }
        self.tell();
    }

    fn tell(&self) {
        let shown = self.state();
        let ls = lock(&self.inner.listeners).clone();
        ls.iter().for_each(|f| f(&shown));
    }

    /// What the page shows: confirmed + its calls still waiting.
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

    /// Calls waiting for the server.
    pub fn pending(&self) -> usize {
        lock(&self.inner.copy).pending.len()
    }

    /// Calls to `method` waiting for the server.
    pub fn pending_of(&self, method: &str) -> usize {
        lock(&self.inner.copy).pending.iter().filter(|q| q.method == method).count()
    }

    pub fn online(&self) -> bool {
        self.inner.online.load(Ordering::SeqCst)
    }

    /// How the copy stands, for the UI.
    pub fn status(&self) -> Status {
        let (online, pending) = (self.online(), self.pending());
        let conn = match () {
            () if pending >= PENDING_MAX => crate::Conn::Blocked,
            () if !online => crate::Conn::Offline,
            () if pending > 0 => crate::Conn::Syncing,
            () => crate::Conn::Online,
        };
        Status {
            online,
            pending,
            degraded: pending * 5 > PENDING_MAX * 4,
            conn,
            last_sync: *lock(&self.inner.last_sync),
            rtt: None,
        }
    }

    /// Call `f` with the shown state after each change.
    pub fn listen(&self, f: impl Fn(&Arc<T>) + Send + Sync + 'static) {
        lock(&self.inner.listeners).push(Arc::new(f));
    }

    /// Call `f(method, why)` when the server refuses a predicted call.
    pub fn on_refused(&self, f: impl Fn(&str, &Rejected) + Send + Sync + 'static) {
        lock(&self.inner.refusals).push(Arc::new(f));
    }

    /// As a native copy's [`crate::sync::Remote::on_conflict`].
    pub fn on_conflict(&self, f: impl Fn(&[crate::merge::Conflict]) + Send + Sync + 'static) {
        lock(&self.inner.conflicts).push(Arc::new(f));
    }

    /// As a native copy's [`crate::sync::Remote::merge`].
    pub fn merge(&self, field: &str, how: crate::merge::Merge) {
        self.update(|c| {
            c.merges.retain(|(f, _)| f != field);
            c.merges.push((field.into(), how));
        });
    }

    /// Where the server's view `now` met the copy's waiting calls.
    fn met(&self, before: Option<(Value, Value)>, now: &Value) -> Vec<crate::merge::Conflict> {
        let merges = lock(&self.inner.copy).merges.clone();
        before.map_or_else(Vec::new, |(base, local)| crate::merge::conflicts(&base, now, &local, &merges))
    }

    fn conflicted(&self, met: &[crate::merge::Conflict]) {
        if met.is_empty() {
            return;
        }
        let fs = lock(&self.inner.conflicts).clone();
        fs.iter().for_each(|f| f(met));
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

/// The copy's link, until every handle is gone: linked, it moves messages
/// both ways; dropped, it is opened again with jittered backoff.
async fn run<T: Cell>(weak: Weak<Inner<T>>, url: String, hello: Value, mut first: Option<Link>) {
    let mut wait = 250u32;
    let mut told = false;
    loop {
        let link = match first.take() {
            Some(l) => l,
            None => match Link::open(&url, &hello).await {
                Ok((l, m)) => {
                    let Some(inner) = weak.upgrade() else { return };
                    Remote { inner }.take(m);
                    l
                }
                Err(Failed::Refused(e)) => return say(&format!("sync {}: {e}; not retrying", T::ID)),
                Err(Failed::Skew(e) | Failed::Offline(e)) => {
                    if !told {
                        say(&format!("sync {}: {e}; retrying", T::ID));
                        told = true;
                    }
                    let jitter = u32::from(crate::uuid_v4().as_bytes()[0]) * 2;
                    super::sleep(wait + jitter).await;
                    wait = (wait * 2).min(10_000);
                    if weak.strong_count() == 0 {
                        return;
                    }
                    continue;
                }
            },
        };
        (wait, told) = (250, false);
        let Some(inner) = weak.upgrade() else { return };
        pump(Remote { inner }, link).await;
        if weak.strong_count() == 0 {
            return;
        }
    }
}

/// One link: send what waits, then move messages both ways until it drops.
async fn pump<T: Cell>(r: Remote<T>, mut link: Link) {
    let (tx, mut out) = unbounded_channel::<String>();
    for q in lock(&r.inner.copy).pending.clone() {
        _ = tx.send(q.msg().to_string());
    }
    *lock(&r.inner.out) = Some(tx);
    r.inner.online.store(true, Ordering::SeqCst);
    r.tell();
    loop {
        tokio::select! {
            h = link.rx.recv() => match h {
                Some(Heard::Text(t)) => match serde_json::from_str::<Value>(&t) {
                    Ok(m) => r.take(m),
                    Err(_) => break,
                },
                Some(Heard::Open) => {}
                Some(Heard::Closed) | None => break,
            },
            Some(m) = out.recv() => {
                if link.ws.send_with_str(&m).is_err() {
                    break;
                }
            }
        }
    }
    r.inner.online.store(false, Ordering::SeqCst);
    *lock(&r.inner.out) = None;
    r.tell();
}

impl<T: Cell> Remote<T> {
    /// Make the call on the copy and send it (an async method's caller waits
    /// at `end` for its end). In a full queue it is refused.
    fn predict(&self, method: &str, args: Value, end: Option<crate::Reply>) -> Result<Value> {
        let (out, q) = {
            let c = lock(&self.inner.copy);
            crate::ensure!(c.pending.len() < PENDING_MAX, "blocked: {PENDING_MAX} calls wait for the server");
            let op = crate::cx::Op::new();
            let cx = Cx::synced(super::now_ms(), T::ID, &By::User(c.user.clone()), &op);
            let waits = T::UNPREDICTED.contains(&method);
            let out = match waits {
                // Sent as is: the server alone runs it (and may refuse it).
                true => Value::Null,
                false => apply(&mut (*c.shown).clone(), &cx, method, args.clone())?,
            };
            let at = cx.now();
            let q =
                Queued { op: op.to_string(), method: method.into(), args, inputs: cx.into_asks().inputs, at, waits };
            (out, q)
        };
        if let Some(end) = end {
            lock(&self.inner.ends).insert(q.op.clone(), end);
        }
        self.update(|c| c.pending.push(q.clone()));
        self.send(q.msg());
        Ok(out)
    }
}

impl<T: Cell> Door<T> for Remote<T> {
    /// Predicted: applied to the copy at once and sent; works offline (queued).
    fn call(&self, method: &str, args: Value) -> Result<Value> {
        self.predict(method, args, None)
    }

    /// Predicted like `call`; an async method's end comes from the server.
    fn ask(&self, method: &str, args: Value) -> Result<crate::Answer> {
        if T::async_mode(method).is_none() {
            return self.call(method, args).map(|v| crate::Answer::ready(Ok(v)));
        }
        let (end, answer) = crate::Reply::new(method);
        self.predict(method, args, Some(end))?;
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

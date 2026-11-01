//! Keyed held resources (aio's `useResource`): a camera, a socket, a file
//! handle, opened for a key and closed when its last holder lets go. One
//! open per key however many hold it. A key opened again waits for its
//! close to finish, and a holder's next key for its last one's, up to
//! [`SETTLE`] (then it opens anyway, loudly). An open that finishes after
//! everyone let go is closed, never handed out. Views hold one with
//! `aio::kit::use_held`; the key decides which one is held, so whatever
//! changes the key (a method, an agent) reopens it.
use futures_util::future::{BoxFuture, FutureExt, Shared, join_all};
use std::collections::HashMap;
use std::fmt::{Debug, Display};
use std::future::Future;
use std::hash::Hash;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;

/// How long an open waits for the close before it: one that never ends
/// holds the device, and the next open goes ahead after this.
pub const SETTLE: Duration = Duration::from_secs(5);

/// Where a held resource stands.
#[derive(Debug)]
pub enum State<T> {
    Opening,
    Open(Arc<T>),
    /// The open failed: why. Nothing is held.
    Failed(String),
}

impl<T> Clone for State<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Opening => Self::Opening,
            Self::Open(v) => Self::Open(v.clone()),
            Self::Failed(e) => Self::Failed(e.clone()),
        }
    }
}

impl<T> State<T> {
    /// The resource, once open.
    pub fn value(&self) -> Option<&Arc<T>> {
        match self {
            Self::Open(v) => Some(v),
            _ => None,
        }
    }
}

/// A release finishing: the close (or the open it has to close) running.
pub type Settled = Shared<BoxFuture<'static, ()>>;

type Open<K, T> = Box<dyn Fn(K) -> BoxFuture<'static, Result<T, String>> + Send + Sync>;
type Close<K, T> = Box<dyn Fn(Arc<T>, K) -> BoxFuture<'static, ()> + Send + Sync>;

struct Pool<K, T> {
    open: Open<K, T>,
    close: Close<K, T>,
    slots: Mutex<Slots<K, T>>,
}

struct Slots<K, T> {
    next: u64,
    live: HashMap<K, Slot<T>>,
    /// Releases still finishing, by key, with the slot they were.
    closing: HashMap<K, (u64, Settled)>,
}

struct Slot<T> {
    id: u64,
    refs: usize,
    state: watch::Sender<State<T>>,
    /// The open: it closes what it made if the slot is gone by then.
    task: Settled,
}

/// A pool of resources by key (one per kind of thing: cameras, sockets).
pub struct Held<K, T>(Arc<Pool<K, T>>);

impl<K, T> Clone for Held<K, T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

fn spawn(f: impl Future<Output = ()> + Send + 'static) {
    match tokio::runtime::Handle::try_current() {
        Ok(rt) => drop(rt.spawn(f)),
        Err(_) => drop(crate::rt().spawn(f)),
    }
}

fn ready() -> Settled {
    futures_util::future::ready(()).boxed().shared()
}

impl<K: Clone + Eq + Hash + Debug + Send + Sync + 'static, T: Send + Sync + 'static> Held<K, T> {
    /// `open` makes the resource for a key; `close` ends it (it may take
    /// its time: the next open of that key waits).
    pub fn new<O, E, C>(
        open: impl Fn(K) -> O + Send + Sync + 'static,
        close: impl Fn(Arc<T>, K) -> C + Send + Sync + 'static,
    ) -> Self
    where
        O: Future<Output = Result<T, E>> + Send + 'static,
        E: Display,
        C: Future<Output = ()> + Send + 'static,
    {
        Self(Arc::new(Pool {
            open: Box::new(move |k| open(k).map(|r| r.map_err(|e| e.to_string())).boxed()),
            close: Box::new(move |v, k| close(v, k).boxed()),
            slots: Mutex::new(Slots { next: 0, live: HashMap::new(), closing: HashMap::new() }),
        }))
    }

    /// Hold `key`'s resource, opening it if nobody holds it; a new open
    /// waits for `after` (this holder's last release) and the key's own close.
    pub fn hold(&self, key: K, after: Option<Settled>) -> Hold<K, T> {
        let pool = &self.0;
        let mut s = pool.slots.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(slot) = s.live.get_mut(&key) {
            slot.refs += 1;
            let (id, rx) = (slot.id, slot.state.subscribe());
            return Hold { pool: pool.clone(), key, id, rx, held: true };
        }
        s.next += 1;
        let id = s.next;
        let before: Vec<Settled> = s.closing.get(&key).map(|(_, d)| d.clone()).into_iter().chain(after).collect();
        let (tx, rx) = watch::channel(State::Opening);
        let task = opening(pool.clone(), key.clone(), id, before).boxed().shared();
        spawn(task.clone());
        s.live.insert(key.clone(), Slot { id, refs: 1, state: tx, task });
        Hold { pool: pool.clone(), key, id, rx, held: true }
    }

    /// Resources open or opening now.
    pub fn len(&self) -> usize {
        self.0.slots.lock().unwrap_or_else(|e| e.into_inner()).live.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Slot `id` of `key`, opened once what was before it settled.
async fn opening<K: Clone + Eq + Hash + Debug, T>(pool: Arc<Pool<K, T>>, key: K, id: u64, before: Vec<Settled>) {
    let live =
        |p: &Pool<K, T>| p.slots.lock().unwrap_or_else(|e| e.into_inner()).live.get(&key).is_some_and(|s| s.id == id);
    if !before.is_empty() {
        if tokio::time::timeout(SETTLE, join_all(before)).await.is_err() {
            crate::warn!(
                cat: "ui",
                "held {key:?}: the close before it has not finished after {SETTLE:?}; opening anyway \
                 (an open or close that never ends holds the device)"
            );
        }
        if !live(&pool) {
            return;
        }
    }
    let made = (pool.open)(key.clone()).await;
    let stale = {
        let s = pool.slots.lock().unwrap_or_else(|e| e.into_inner());
        match s.live.get(&key).filter(|s| s.id == id) {
            Some(slot) => {
                slot.state.send_replace(match made {
                    Ok(v) => State::Open(Arc::new(v)),
                    Err(e) => State::Failed(e),
                });
                None
            }
            None => made.ok(),
        }
    };
    if let Some(v) = stale {
        (pool.close)(Arc::new(v), key).await;
    }
}

/// One holder of a resource; dropping it lets go (the last one closes it).
pub struct Hold<K: Clone + Eq + Hash + Send + Sync + 'static, T: Send + Sync + 'static> {
    pool: Arc<Pool<K, T>>,
    key: K,
    id: u64,
    rx: watch::Receiver<State<T>>,
    held: bool,
}

impl<K: Clone + Eq + Hash + Send + Sync + 'static, T: Send + Sync + 'static> Hold<K, T> {
    pub fn key(&self) -> &K {
        &self.key
    }

    /// Where it stands now.
    pub fn state(&self) -> State<T> {
        self.rx.borrow().clone()
    }

    /// Told on every change of [`state`](Self::state).
    pub fn watch(&self) -> watch::Receiver<State<T>> {
        self.rx.clone()
    }

    /// Let go now; what finishes the release (ready if others still hold it).
    pub fn release(mut self) -> Settled {
        self.let_go()
    }

    fn let_go(&mut self) -> Settled {
        if !std::mem::take(&mut self.held) {
            return ready();
        }
        let pool = &self.pool;
        let mut s = pool.slots.lock().unwrap_or_else(|e| e.into_inner());
        let Some(slot) = s.live.get_mut(&self.key).filter(|s| s.id == self.id) else { return ready() };
        slot.refs -= 1;
        if slot.refs > 0 {
            return ready();
        }
        let slot = s.live.remove(&self.key).expect("the slot just seen");
        let done = match &*slot.state.borrow() {
            State::Open(v) => (pool.close)(v.clone(), self.key.clone()).boxed().shared(),
            State::Opening => slot.task.clone(),
            State::Failed(_) => return ready(),
        };
        s.closing.insert(self.key.clone(), (self.id, done.clone()));
        let (pool, key, id, d) = (pool.clone(), self.key.clone(), self.id, done.clone());
        spawn(async move {
            d.await;
            let mut s = pool.slots.lock().unwrap_or_else(|e| e.into_inner());
            if s.closing.get(&key).is_some_and(|(at, _)| *at == id) {
                s.closing.remove(&key);
            }
        });
        done
    }
}

impl<K: Clone + Eq + Hash + Send + Sync + 'static, T: Send + Sync + 'static> Drop for Hold<K, T> {
    fn drop(&mut self) {
        _ = self.let_go();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Log = Arc<Mutex<Vec<String>>>;

    /// A camera whose open takes `open_ms` and close `close_ms`; "bad" fails.
    fn camera(log: &Log, open_ms: u64, close_ms: u64) -> Held<&'static str, String> {
        let (l, c) = (log.clone(), log.clone());
        Held::new(
            move |k: &'static str| {
                let l = l.clone();
                async move {
                    l.lock().unwrap().push(format!("open {k}"));
                    tokio::time::sleep(Duration::from_millis(open_ms)).await;
                    if k == "bad" { Err("no such camera") } else { Ok(format!("cam {k}")) }
                }
            },
            move |v: Arc<String>, k| {
                let c = c.clone();
                async move {
                    c.lock().unwrap().push(format!("closing {k}"));
                    tokio::time::sleep(Duration::from_millis(close_ms)).await;
                    c.lock().unwrap().push(format!("closed {v}"));
                }
            },
        )
    }

    fn took(log: &Log) -> Vec<String> {
        std::mem::take(&mut *log.lock().unwrap())
    }

    async fn pause(ms: u64) {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }

    #[tokio::test(start_paused = true)]
    async fn one_open_per_key_shared_by_its_holders_and_closed_by_the_last() {
        let log = Log::default();
        let cams = camera(&log, 10, 0);
        let (a, b) = (cams.hold("a", None), cams.hold("a", None));
        assert!(matches!(a.state(), State::Opening));
        pause(20).await;
        assert_eq!(a.state().value().map(|v| v.as_str()), Some("cam a"));
        assert!(Arc::ptr_eq(a.state().value().unwrap(), b.state().value().unwrap()), "the same one");
        drop(a);
        pause(1).await;
        assert_eq!(took(&log), ["open a"], "still held");
        b.release().await;
        assert_eq!(took(&log), ["closing a", "closed cam a"]);
        assert!(cams.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_key_opened_again_waits_for_its_slow_close() {
        let log = Log::default();
        let cams = camera(&log, 0, 50);
        let a = cams.hold("a", None);
        pause(1).await;
        drop(a);
        let again = cams.hold("a", None);
        pause(10).await;
        assert_eq!(took(&log), ["open a", "closing a"], "not reopened while closing");
        pause(50).await;
        assert_eq!(took(&log), ["closed cam a", "open a"]);
        assert!(again.state().value().is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn a_holders_next_key_waits_for_its_last_keys_close() {
        let log = Log::default();
        let cams = camera(&log, 0, 50);
        let a = cams.hold("a", None);
        pause(1).await;
        let b = cams.hold("b", Some(a.release()));
        pause(60).await;
        assert_eq!(took(&log), ["open a", "closing a", "closed cam a", "open b"]);
        assert!(b.state().value().is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn an_open_that_ends_after_everyone_let_go_is_closed_and_the_next_waits_for_it() {
        let log = Log::default();
        let cams = camera(&log, 30, 20);
        let a = cams.hold("a", None);
        pause(5).await;
        let again = cams.hold("b", Some(a.release()));
        pause(100).await;
        assert_eq!(took(&log), ["open a", "closing a", "closed cam a", "open b"], "the stale one closed first");
        assert_eq!(again.state().value().map(|v| v.as_str()), Some("cam b"));
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_open_says_why_and_holds_nothing() {
        let log = Log::default();
        let cams = camera(&log, 0, 0);
        let bad = cams.hold("bad", None);
        pause(1).await;
        assert!(matches!(bad.state(), State::Failed(e) if e == "no such camera"));
        drop(bad);
        pause(1).await;
        assert_eq!(took(&log), ["open bad"], "nothing to close");
        assert!(cams.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_close_that_never_ends_delays_the_next_open_by_the_bound_only() {
        let log = Log::default();
        let cams = camera(&log, 0, 1 << 40);
        let a = cams.hold("a", None);
        pause(1).await;
        drop(a);
        let again = cams.hold("a", None);
        pause(SETTLE.as_millis() as u64 - 10).await;
        assert!(matches!(again.state(), State::Opening));
        pause(20).await;
        assert!(again.state().value().is_some(), "opened anyway");
        assert_eq!(took(&log), ["open a", "closing a", "open a"]);
    }

    #[tokio::test(start_paused = true)]
    async fn pools_are_separate_and_keys_are_typed() {
        // aio's scopes, and its number key vs string key: a pool and a key type each.
        let log = Log::default();
        let (front, back) = (camera(&log, 0, 0), camera(&log, 0, 0));
        let (_f, _b) = (front.hold("a", None), back.hold("a", None));
        pause(1).await;
        assert_eq!(took(&log), ["open a", "open a"]);
        let n: Held<u32, u32> = Held::new(|k| async move { Ok::<_, String>(k) }, |_, _| async {});
        let _one = n.hold(1, None);
        assert_eq!(n.len(), 1);
    }
}

//! A shareable door to one store: one call at a time (the cell is an actor),
//! listeners told after each commit, resources the cell owns, and the
//! runtime part: effects started after commits, schedules fired on time.
use crate::effect::{Abort, Effect, Mode};
use crate::{By, Cell, Door, Error, Result, Store};
use serde_json::Value;
use std::{
    any::Any,
    collections::HashMap,
    path::Path,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    sync::{Notify, oneshot},
    task::AbortHandle,
};

type Listener<T> = Arc<dyn Fn(&Arc<T>) + Send + Sync>;
type Reaction = Arc<dyn Fn(&crate::Event) + Send + Sync>;

/// A caller refused while the app closes may retry after this.
pub const RETRY_AFTER_MS: u64 = 2000;

/// Commits whose new state is readable but whose listeners (a view's
/// re-render among them) are not all told yet, in this process. `TestUi`
/// settles only once it is 0, so a test that saw a state change never acts
/// on the screen from before it.
pub(crate) static TELLING: AtomicUsize = AtomicUsize::new(0);

/// One commit counted in [`TELLING`] until it is dropped (a listener's panic too).
struct Telling;

impl Telling {
    fn new() -> Self {
        TELLING.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for Telling {
    fn drop(&mut self) {
        TELLING.fetch_sub(1, Ordering::SeqCst);
    }
}

/// What a shutdown left undone.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Drained {
    /// Effects still running at the deadline, aborted.
    pub aborted: Vec<String>,
}

/// The newest steps a [`Timeline`] lists.
pub const TIMELINE_STEPS: usize = 20;

/// A time-travel step ([`Handle::travel`]; aio's `useTimeTravel`'s controls).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeAsk {
    Undo,
    Redo,
    /// To the state right after event `seq`.
    Goto(u64),
    Pause,
    Resume,
}

/// Where a cell is in its history ([`crate::Position`]), whether travel
/// holds it, and its newest [`TIMELINE_STEPS`] events, oldest first.
#[derive(Clone, Debug, PartialEq)]
pub struct Timeline {
    pub at: u64,
    pub head: u64,
    pub floor: u64,
    pub paused: bool,
    pub steps: Vec<Moment>,
}

/// One event of a [`Timeline`]: its `method` (`$rewind` for a step), when
/// (ms since the Unix epoch) and, for calls of this run, how long it took.
#[derive(Clone, Debug, PartialEq)]
pub struct Moment {
    pub seq: u64,
    pub method: String,
    pub at: i64,
    pub took: Option<Duration>,
}

struct Inner<T: Cell> {
    store: Mutex<Store<T>>,
    /// The last committed state and its event, readable while a call runs.
    shown: Mutex<(u64, Arc<T>)>,
    listeners: Mutex<Vec<Listener<T>>>,
    /// Other cells' reactions to its committed calls ([`Handle::listens_to`]).
    reactions: Mutex<Vec<Reaction>>,
    watchers: Mutex<Vec<Arc<dyn Fn() + Send + Sync>>>,
    owned: Mutex<HashMap<String, Box<dyn Any + Send>>>,
    /// The effects running (or queued) per key, each with its number.
    running: Mutex<HashMap<String, Vec<(u64, AbortHandle)>>>,
    /// Per key, the end of the last queued effect: the next one waits for it.
    tails: Mutex<HashMap<String, oneshot::Receiver<()>>>,
    /// Held while a commit's events and effects are taken and handed on.
    order: Mutex<()>,
    ran: AtomicU64,
    /// An effect ended (finished, failed, timed out or aborted).
    ended: Notify,
    wake: Notify,
    /// Shutting down: new calls are refused, schedules stop, effects finish.
    closing: AtomicBool,
    /// Each committed event with the cell's event before it, for synced copies.
    events: tokio::sync::broadcast::Sender<(u64, crate::Event)>,
    /// What its calls told the people using the app, for synced clients.
    tells: tokio::sync::broadcast::Sender<crate::tell::Tell>,
    /// Set by a test (`aio::testing`): effects wait here until it starts them.
    held: Mutex<Option<Vec<Effect<T>>>>,
    /// A test's ledger: its schedules' and effects' failures, until it takes them.
    failed: Mutex<Vec<String>>,
    /// Disabled (`Cells::disable`): calls refused, schedules paused.
    off: AtomicBool,
    /// Paused by time travel (`ax timetravel pause`): calls refused.
    paused: AtomicBool,
    /// Its `#[on_start]` ran, and its `#[on_stop]` not since.
    live: AtomicBool,
    /// Its failures, and when they disable it.
    tally: Mutex<crate::cells::Tally>,
    /// `#[ttl]` calls answered.
    answered: Mutex<crate::answer::Cache>,
    /// The `#[mode(first)]` and `#[mode(newest)]` calls in flight.
    inflight: crate::answer::Inflight,
    /// aio pages' CRDT sync of this cell (`compat::crdt`): its clock, the ops
    /// it passes on.
    #[cfg(all(feature = "compat", feature = "server", not(target_arch = "wasm32")))]
    aio_sync: crate::compat::crdt::Shared,
}

/// A cell held in this process: its state ([`state`](Self::state)), its
/// calls, its store. A cell's methods are called on it through the
/// `<Cell>Calls` trait `#[methods]` makes (`h.add(1)`, any [`Door`](crate::Door)
/// has them). The traits listed below under blanket implementations
/// (`FeedbackCalls`, `UpdatesCalls`: `dismiss`, `refresh`, `apply`, …) are
/// those of aio's own cells, for a `Handle` of that cell only.
pub struct Handle<T: Cell> {
    inner: Arc<Inner<T>>,
    by: By,
}

impl<T: Cell> Clone for Handle<T> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone(), by: self.by.clone() }
    }
}

/// A poisoned lock only means a listener panicked; the store itself commits
/// atomically, so its state is still whole.
fn lock<X>(m: &Mutex<X>) -> MutexGuard<'_, X> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl<T: Cell> Handle<T> {
    pub fn new(store: Store<T>) -> Self {
        let inner = Inner {
            shown: Mutex::new((store.seq(), store.shared())),
            store: Mutex::new(store),
            listeners: Mutex::default(),
            reactions: Mutex::default(),
            watchers: Mutex::default(),
            owned: Mutex::default(),
            running: Mutex::default(),
            tails: Mutex::default(),
            order: Mutex::default(),
            ran: AtomicU64::new(0),
            ended: Notify::new(),
            wake: Notify::new(),
            closing: AtomicBool::new(false),
            events: tokio::sync::broadcast::channel(1024).0,
            tells: tokio::sync::broadcast::channel(64).0,
            held: Mutex::default(),
            failed: Mutex::default(),
            off: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            live: AtomicBool::new(false),
            tally: Mutex::default(),
            answered: Mutex::default(),
            inflight: Default::default(),
            #[cfg(all(feature = "compat", feature = "server", not(target_arch = "wasm32")))]
            aio_sync: Default::default(),
        };
        for k in crate::perf::unknown::<T>() {
            crate::warn!(cat: "perf", "aio.toml [perf.methods] \"{k}\" names no method of {}: it has {}", T::ID, T::METHODS.join(", "));
        }
        Self { inner: Arc::new(inner), by: By::System }
    }

    pub fn memory() -> Self {
        Self::new(Store::memory())
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        Store::open(path).map(Self::new)
    }

    /// [`Store::open_aio`]: aio app `app`'s `state.db`, kept readable by aio.
    pub fn open_aio(path: impl AsRef<Path>, app: &str) -> Result<Self, Error> {
        Store::open_aio(path, app).map(Self::new)
    }

    /// The same store, with calls recorded as caused by `by`.
    pub fn by(&self, by: By) -> Self {
        Self { inner: self.inner.clone(), by }
    }

    /// The state now, cheap to keep. Never waits for a running call: it is
    /// the last committed state (a busy cell stays readable).
    pub fn state(&self) -> Arc<T> {
        lock(&self.inner.shown).1.clone()
    }

    /// Run `f` with the store (history, time travel). `f` must not use this handle.
    /// A state it changes (a rewind) forgets the `#[ttl]` answers.
    pub fn store<R>(&self, f: impl FnOnce(&mut Store<T>) -> R) -> R {
        let r = {
            let mut s = lock(&self.inner.store);
            let before = s.shared();
            // What it reads (the log, the history) has the held run in it.
            if let Err(e) = s.flush() {
                eprintln!("[aio] {}: a held run could not be saved: {e}", T::ID);
            }
            let r = f(&mut s);
            if !Arc::ptr_eq(&before, &s.shared()) {
                lock(&self.inner.answered).clear();
            }
            r
        };
        self.after_commit(true);
        r
    }

    /// Call `f` with the new state after each commit.
    pub fn listen(&self, f: impl Fn(&Arc<T>) + Send + Sync + 'static) {
        lock(&self.inner.listeners).push(Arc::new(f));
    }

    /// Call `on` with `f(state)` (a field, a `#[derived]` value) now and
    /// after each commit that changes it: a watcher outside views.
    pub fn watch<R: PartialEq + Send + 'static>(
        &self,
        f: impl Fn(&T) -> R + Send + Sync + 'static,
        on: impl Fn(&R) + Send + Sync + 'static,
    ) {
        let l = Arc::new(crate::derived::changes(f, on));
        let first = l.clone();
        self.listen(move |st| l(st));
        first(&self.state());
    }

    /// Call `on(new, Some(old))` after each commit that changes `f(state)`
    /// (aio's `onChange`; `immediate`: also `on(now, None)` at once). What `on`
    /// returns (an open camera, a socket) is kept until the next change, and
    /// dropped before the next `on` runs; dropping the [`crate::Watching`] stops it.
    pub fn on_change<R, M>(
        &self,
        immediate: bool,
        f: impl Fn(&T) -> R + Send + Sync + 'static,
        on: impl Fn(&R, Option<&R>) -> M + Send + Sync + 'static,
    ) -> crate::Watching
    where
        R: PartialEq + Clone + Send + 'static,
        M: Send + 'static,
    {
        let (l, w) = crate::derived::reaction(&*self.state(), immediate, f, on);
        let l = Arc::new(l);
        let first = l.clone();
        self.listen(move |st| l(st));
        // A commit between the first look and listening isn't missed.
        first(&self.state());
        w
    }

    /// Run `f` on this cell after each committed call of `method` on
    /// `source` (aio's `listensTo`: `source` never knows who listens). It
    /// gets the call's event (its arguments, who made it); its own calls
    /// are made by the same caller. A refusal is said in the log and a
    /// report, and never undoes the source's call. A `method` `source`
    /// doesn't have panics here, as the app wires its cells (a typo would
    /// otherwise never fire).
    pub fn listens_to<S: Cell, R>(
        &self,
        source: &Handle<S>,
        method: &'static str,
        f: impl Fn(&Handle<T>, &crate::Event) -> Result<R> + Send + Sync + 'static,
    ) {
        assert!(
            S::METHODS.contains(&method),
            "{}.listens_to: {} has no method `{method}` (its methods: {})",
            T::ID,
            S::ID,
            S::METHODS.join(", ")
        );
        let me = self.clone();
        let react = move |e: &crate::Event| {
            if e.method != method {
                return;
            }
            if let Err(err) = f(&me.by(e.by.clone()), e) {
                let msg = format!("{}: its reaction to {}.{method} refused: {err}", T::ID, S::ID);
                eprintln!("[aio] {msg}");
                crate::report::problem("reaction", &msg);
            }
        };
        lock(&source.inner.reactions).push(Arc::new(react));
    }

    /// Call `f` when `pending` may have changed (an effect started or ended).
    pub fn on_pending(&self, f: impl Fn() + Send + Sync + 'static) {
        lock(&self.inner.watchers).push(Arc::new(f));
    }

    fn keep(&self, key: &str, res: Option<Box<dyn Any + Send>>) {
        let old = match res {
            Some(r) => lock(&self.inner.owned).insert(key.to_string(), r),
            None => lock(&self.inner.owned).remove(key),
        };
        drop(old);
    }

    pub fn owns(&self, key: &str) -> bool {
        lock(&self.inner.owned).contains_key(key)
    }

    /// Tell the listeners, start the effects the commits asked for, and
    /// let the scheduler see changed schedules.
    fn after_commit(&self, wake: bool) {
        // Before the state is shown: from then until its listeners are told.
        let telling = Telling::new();
        let (started, done, changed) = {
            // Another thread's commit may take these first: whoever takes
            // them sends and starts them before the next taker, so events
            // and queued effects keep commit order.
            let _order = lock(&self.inner.order);
            let (effects, events, changed) = {
                let mut s = lock(&self.inner.store);
                let mut shown = lock(&self.inner.shown);
                let changed = shown.0 != s.seq() || !Arc::ptr_eq(&shown.1, &s.shared());
                *shown = (s.seq(), s.shared());
                (s.take_effects(), s.take_committed(), changed)
            };
            let reacting = !lock(&self.inner.reactions).is_empty();
            let done: Vec<crate::Event> =
                if reacting { events.iter().map(|(_, e)| e.clone()).collect() } else { vec![] };
            for e in events {
                // No subscribers is fine: nobody syncs this cell.
                _ = self.inner.events.send(e);
            }
            let started = !effects.is_empty();
            for e in effects {
                match e.downcast::<Effect<T>>() {
                    Ok(e) => match lock(&self.inner.held).as_mut() {
                        Some(held) => held.push(*e),
                        None => self.spawn(*e),
                    },
                    Err(e) => match e.downcast::<Abort>() {
                        Ok(a) => self.abort(&a.0),
                        Err(e) => match e.downcast::<crate::tell::Tell>() {
                            Ok(t) => self.say(*t),
                            Err(e) => match e.downcast::<crate::answer::Answered>() {
                                Ok(a) => self.answer(*a),
                                Err(_) => eprintln!("[aio] {}: an effect for another cell was dropped", T::ID),
                            },
                        },
                    },
                }
            }
            (started, done, changed)
        };
        // A call that changed nothing (a cursor already at the end) tells nobody.
        if changed {
            let st = self.state();
            let ls = lock(&self.inner.listeners).clone();
            ls.iter().for_each(|f| f(&st));
        }
        drop(telling);
        // After the order lock: a reaction may call back into this cell.
        let rs = lock(&self.inner.reactions).clone();
        done.iter().for_each(|e| rs.iter().for_each(|r| r(e)));
        if started {
            tell(&self.inner);
        }
        if wake {
            self.inner.wake.notify_one();
        }
        self.count_failures();
    }

    /// Count the failures the store saw; the breaker's trip disables the
    /// cell (its state kept), said in the log, health and a report.
    fn count_failures(&self) {
        let fs = lock(&self.inner.store).take_failures();
        let Some((_, last)) = fs.last() else { return };
        if lock(&self.inner.held).is_some() {
            let unseen = fs.iter().filter_map(|(by, why)| match by {
                By::Schedule(k) => Some(format!("{why} (schedule `{k}`)")),
                By::Effect(k) => Some(format!("{why} (effect `{k}`)")),
                _ => None,
            });
            lock(&self.inner.failed).extend(unseen);
        }
        let now = std::time::Instant::now();
        let (tripped, n) = {
            let mut t = lock(&self.inner.tally);
            let tripped = fs.iter().fold(false, |tripped, _| t.add(now) || tripped);
            (tripped, t.count(now))
        };
        if tripped && self.enabled() {
            let msg = format!("{} disabled after {n} failures; the last: {last}", T::ID);
            eprintln!("[aio] {msg}");
            crate::health::degrade(&format!("cell {}", T::ID), &msg);
            crate::report::problem("tripped", &msg);
            self.switch(false);
        }
    }

    /// Is the cell enabled (taking calls, firing schedules)?
    pub fn enabled(&self) -> bool {
        !self.inner.off.load(Ordering::SeqCst)
    }

    /// Disable: calls refused, schedules paused, running effects aborted,
    /// owned resources dropped; the state stays. Enable: the failures count
    /// from zero, and schedules missed meanwhile fire once.
    pub(crate) fn switch(&self, on: bool) {
        if !on {
            _ = self.live(false);
        }
        self.inner.off.store(!on, Ordering::SeqCst);
        if on {
            _ = self.live(true);
            lock(&self.inner.tally).clear();
            crate::health::ok(&format!("cell {}", T::ID));
            self.inner.wake.notify_one();
        } else {
            lock(&self.inner.running).values().flatten().for_each(|(_, a)| a.abort());
            let owned = std::mem::take(&mut *lock(&self.inner.owned));
            drop(owned);
        }
    }

    /// Going live or not: run its `#[on_start]` or `#[on_stop]` once each
    /// way. A refusal (or a panic) is said in the log and a report, never
    /// stops the boot or the shutdown.
    pub(crate) fn live(&self, on: bool) -> Result {
        if self.inner.live.swap(on, Ordering::SeqCst) == on {
            return Ok(());
        }
        if on && !T::SCHEDULES.is_empty() {
            if let Err(e) = lock(&self.inner.store).declare() {
                eprintln!("[aio] {e}");
                crate::report::problem("schedule", &e.0);
            }
            self.inner.wake.notify_one();
        }
        let (Some(m), hook) = (if on { (T::ON_START, "on_start") } else { (T::ON_STOP, "on_stop") }) else {
            return Ok(());
        };
        Door::call(self, m, Value::Null).map(drop).map_err(|e| {
            let msg = format!("{}: #[{hook}] {m} refused: {e}", T::ID);
            eprintln!("[aio] {msg}");
            crate::report::problem("hook", &msg);
            crate::Rejected::new(msg)
        })
    }

    /// Disable the cell after `max` failures (within `window`, if set).
    pub(crate) fn trip_after(&self, max: u32, window: Option<Duration>) {
        lock(&self.inner.tally).trip_after(max, window);
    }

    /// Failures counted (since the last enable).
    pub(crate) fn failures(&self) -> usize {
        lock(&self.inner.tally).count(std::time::Instant::now())
    }

    /// The last committed call's method and time (ms since the epoch).
    pub(crate) fn last_call(&self) -> Option<(String, i64)> {
        lock(&self.inner.store).last_call()
    }

    fn spawn(&self, e: Effect<T>) {
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            eprintln!("[aio] {}: effect `{}` needs a tokio runtime; dropped", T::ID, e.key);
            return;
        };
        let mut running = lock(&self.inner.running);
        let busy = running.contains_key(&e.key);
        match e.mode {
            Mode::First if busy => return,
            Mode::Newest if busy => running[&e.key].iter().for_each(|(_, a)| a.abort()),
            _ => {}
        }
        crate::effect_started(T::ID, &e.key);
        // Queued in call order: each waits for the one asked before it, not
        // for whichever task the runtime happens to poll first.
        let (next, after) = match e.mode {
            Mode::Queue => {
                let (tx, rx) = oneshot::channel();
                (Some(tx), lock(&self.inner.tails).insert(e.key.clone(), rx))
            }
            _ => (None, None),
        };
        let id = self.inner.ran.fetch_add(1, Ordering::Relaxed);
        let end = Ended { inner: self.inner.clone(), key: e.key.clone(), id, _next: next };
        let (h, timeout) = (self.by(By::Effect(e.key.clone())), e.timeout);
        let (mut run, key) = ((e.run)(h), e.key.clone());
        // Each step timed against the effect budget (`perf`).
        let fut = std::future::poll_fn(move |cx| crate::perf::step(T::ID, &key, || run.as_mut().poll(cx)));
        let waiting = after.is_some().then(crate::vitals::Waiting::new);
        let task = rt.spawn(async move {
            let end = end;
            if let Some(rx) = after {
                // The one before ended (or was aborted): its turn is over.
                _ = rx.await;
            }
            drop(waiting);
            let Some(d) = timeout else { return fut.await };
            if tokio::time::timeout(d, fut).await.is_err() {
                let msg = format!("{}: effect `{}` timed out after {d:?}", T::ID, end.key);
                eprintln!("[aio] {msg}");
                crate::report::problem("effect:timeout", &msg);
            }
        });
        running.entry(e.key).or_default().push((id, task.abort_handle()));
    }

    /// An async call ended and committed: tell its caller, and keep a
    /// `#[ttl]` answer for its key.
    fn answer(&self, a: crate::answer::Answered) {
        if let Some(r) = &a.reply {
            r.send(Ok(a.value.clone()));
        }
        let Some((key, ms)) = a.ttl else { return };
        let now = lock(&self.inner.store).now();
        lock(&self.inner.answered).keep(key, now, ms, a.value);
    }

    /// This call's answer, when one within its ttl has it: it needn't run again.
    fn answers(&self, method: &str, args: &Value) -> Option<Value> {
        if lock(&self.inner.answered).is_empty() {
            return None;
        }
        let now = lock(&self.inner.store).now();
        lock(&self.inner.answered).get(method, args, now)
    }

    /// Run a call (by the network caller `who`, with a synced client's
    /// `op`, if any), followed to its end when it is an async method's:
    /// what it returned now (`None`: a re-sent `op`, not applied again), and
    /// its end. `who`'s `#[client]` rule is asked on the very state the
    /// call runs on.
    fn enter(
        &self,
        who: Option<&crate::Caller>,
        method: &str,
        args: Value,
        op: Option<&str>,
    ) -> Result<(Option<Value>, crate::Answer)> {
        self.open_for_calls()?;
        if who.is_none()
            && op.is_none()
            && let Some(v) = self.answers(method, &args)
        {
            return Ok((Some(v.clone()), crate::Answer::ready(Ok(v))));
        }
        let mode = T::async_mode(method);
        let (reply, answer) = match mode {
            Some(_) => {
                let (r, a) = crate::answer::Reply::new(method);
                (Some(r), Some(a))
            }
            None => (None, None),
        };
        if let (Some(mode), Some(r)) = (mode, &reply)
            && let Some(shared) = self.inner.inflight.enter(mode, crate::answer::key_of::<T>(method, &args), r)
        {
            return Ok((Some(Value::Null), shared));
        }
        let out = {
            let mut s = lock(&self.inner.store);
            match who.map_or(Ok(()), |w| s.state().allow(w, method, &args)) {
                Ok(()) => s.call_as(&self.by, who.and_then(|w| w.role.as_deref()), method, args, op, reply.clone()),
                Err(e) => Err(e),
            }
        };
        self.settle(&out);
        match (out, reply, answer) {
            (Err(e), reply, _) => {
                reply.inspect(|r| r.send(Err(e.clone())));
                Err(e)
            }
            (Ok(None), Some(_), _) => {
                let why = format!("{method}: ran before, when this call was first sent; its answer went with that link");
                Ok((None, crate::Answer::ready(Err(crate::Rejected::new(why)))))
            }
            (Ok(v), _, Some(a)) => Ok((v, a)),
            (Ok(v), _, None) => Ok((v.clone(), crate::Answer::ready(Ok(v.unwrap_or_default())))),
        }
    }

    /// An async method's step `method` (`m.1`), run by its effect for the
    /// call whose end goes to `reply`: refused once a newer call of a
    /// `#[mode(newest)]` method came.
    pub(crate) fn resume(&self, method: &str, args: Value, reply: Option<crate::answer::Reply>) -> Result<Value> {
        self.open_for_calls()?;
        let base = method.split_once('.').map_or(method, |(m, _)| m);
        if let (Some(crate::Mode::Newest), Some(r), By::Effect(key)) = (T::async_mode(base), &reply, &self.by)
            && !self.inner.inflight.current(key, r)
        {
            crate::reject!("{}.{base}: superseded by a newer call (#[mode(newest)])", T::ID);
        }
        let out = lock(&self.inner.store).call_as(&self.by, None, method, args, None, reply);
        self.settle(&out);
        out.map(Option::unwrap_or_default)
    }

    /// An async method's await refused (`io.await?`): a failure of the cell, as a refused step is.
    pub(crate) fn failed(&self, why: &str) {
        lock(&self.inner.store).failed(&self.by, why);
        self.count_failures();
    }

    /// Stop the running and queued effects with this key, and drop the
    /// held ones (`TestCells`) unstarted.
    fn abort(&self, key: &str) {
        if let Some(held) = lock(&self.inner.held).as_mut() {
            held.retain(|e| e.key != key);
        }
        if let Some(rs) = lock(&self.inner.running).get(key) {
            rs.iter().for_each(|(_, a)| a.abort());
        }
    }

    /// Wait until no effect with this key runs or waits. A caller whose
    /// `Mode::First` effect was skipped waits here for the running one, and
    /// then reads the state it left: the first's result, shared.
    pub async fn done(&self, key: &str) {
        loop {
            let mut ended = std::pin::pin!(self.inner.ended.notified());
            ended.as_mut().enable();
            if !self.busy(key) {
                return;
            }
            ended.await;
        }
    }

    /// Keep effects from now on, unstarted, until `start_held`.
    pub(crate) fn hold(&self) {
        lock(&self.inner.held).get_or_insert_default();
    }

    /// The keys of the effects held, in the order they were asked for.
    pub(crate) fn held(&self) -> Vec<String> {
        lock(&self.inner.held).iter().flatten().map(|e| e.key.clone()).collect()
    }

    /// The effects held, each as its key, mode and timeout.
    pub(crate) fn held_effects(&self) -> Vec<(String, Mode, Option<Duration>)> {
        lock(&self.inner.held).iter().flatten().map(|e| (e.key.clone(), e.mode, e.timeout)).collect()
    }

    /// Start the held effects (inside a tokio runtime); how many.
    pub(crate) fn start_held(&self) -> usize {
        let es = lock(&self.inner.held).as_mut().map(std::mem::take).unwrap_or_default();
        let n = es.len();
        es.into_iter().for_each(|e| self.spawn(e));
        n
    }

    /// The failures of its schedules and effects since the last take.
    pub(crate) fn take_failed(&self) -> Vec<String> {
        std::mem::take(&mut *lock(&self.inner.failed))
    }

    /// The keys of the effects running now.
    pub(crate) fn running(&self) -> Vec<String> {
        let mut r: Vec<String> = lock(&self.inner.running)
            .iter()
            .filter(|(_, rs)| rs.iter().any(|(_, a)| !a.is_finished()))
            .map(|(k, _)| k.clone())
            .collect();
        r.sort();
        r
    }

    /// Shutting down: only the running effects' own calls still commit.
    pub fn closing(&self) -> bool {
        self.inner.closing.load(Ordering::SeqCst)
    }

    fn open_for_calls(&self) -> Result {
        if self.closing() && !matches!(self.by, By::Effect(_)) {
            crate::reject!("{}: the app is closing; retry after {RETRY_AFTER_MS} ms", T::ID);
        }
        crate::ensure!(self.enabled(), "{} is disabled", T::ID);
        if self.paused() && !matches!(self.by, By::Effect(_)) {
            crate::reject!("{}: paused by time travel (`ax timetravel resume` lets calls in again)", T::ID);
        }
        Ok(())
    }

    /// Pause (`true`) or resume time travel: while paused every call is
    /// refused, never dropped as if it ran, and schedules are held
    /// (those missed fire once on resume); running effects still commit,
    /// and rewinds still move the state.
    pub fn pause(&self, on: bool) {
        self.inner.paused.store(on, Ordering::SeqCst);
        if !on {
            self.inner.wake.notify_one();
        }
    }

    /// Is time travel paused?
    pub fn paused(&self) -> bool {
        self.inner.paused.load(Ordering::SeqCst)
    }

    /// Time travel (aio's `tt-cmd`, `useTimeTravel`): `ask` it a step (`None`:
    /// only where the cell is), then the [`Timeline`]. A step pauses the
    /// cell, as [`Self::pause`] does, so it stays where travel put it.
    pub fn travel(&self, ask: Option<TimeAsk>) -> Result<Timeline> {
        use crate::store::Travel;
        let step = match ask {
            Some(TimeAsk::Undo) => Some(Travel::Undo),
            Some(TimeAsk::Redo) => Some(Travel::Redo),
            Some(TimeAsk::Goto(seq)) => Some(Travel::To(seq)),
            Some(TimeAsk::Pause | TimeAsk::Resume) => {
                self.pause(ask == Some(TimeAsk::Pause));
                None
            }
            None => None,
        };
        if let Some(t) = step {
            self.store(|s| s.travel(&self.by, t))?;
            self.pause(true);
        }
        // Read only: not through `store`, which tells the listeners (a view
        // reading this on every commit would loop). A held run is flushed
        // unseen: its state is the one already shown.
        let mut s = lock(&self.inner.store);
        if let Err(e) = s.flush() {
            eprintln!("[aio] {}: a held run could not be saved: {e}", T::ID);
        }
        let bad = |e: Error| crate::Rejected::new(e.0);
        let (p, newest) = (s.position().map_err(bad)?, s.newest(TIMELINE_STEPS).map_err(bad)?);
        let steps = newest
            .into_iter()
            .map(|e| Moment { took: s.took(e.seq), seq: e.seq, method: e.method, at: e.at })
            .collect();
        Ok(Timeline { at: p.at, head: p.head, floor: p.floor, paused: self.paused(), steps })
    }

    /// Shut down within `budget`: refuse new calls (`retry after`),
    /// stop the schedules, let running effects finish and commit; those
    /// still running at the deadline are aborted and named.
    pub async fn shutdown(&self, budget: Duration) -> Drained {
        _ = self.live(false);
        self.inner.closing.store(true, Ordering::SeqCst);
        // A held `#[coalesce]` run is saved before anything else stops.
        self.store(|s| s.flush()).ok();
        self.inner.wake.notify_one();
        let deadline = tokio::time::Instant::now() + budget;
        loop {
            let mut ended = std::pin::pin!(self.inner.ended.notified());
            ended.as_mut().enable();
            let mut busy: Vec<String> = lock(&self.inner.running).keys().cloned().collect();
            if busy.is_empty() {
                return Drained { aborted: vec![] };
            }
            if tokio::time::Instant::now() >= deadline {
                busy.sort();
                for k in &busy {
                    self.abort(k);
                    eprintln!("[aio] {}: effect `{k}` still ran at shutdown; aborted", T::ID);
                }
                return Drained { aborted: busy };
            }
            _ = tokio::time::timeout_at(deadline, ended).await;
        }
    }

    /// The event the synced call `op` made (see `Store::op_seq`).
    #[cfg(all(feature = "compat", feature = "server", not(target_arch = "wasm32")))]
    pub(crate) fn op_seq(&self, op: &str) -> Option<u64> {
        lock(&self.inner.store).op_seq(op)
    }

    #[cfg(all(feature = "compat", feature = "server", not(target_arch = "wasm32")))]
    pub(crate) fn aio_sync(&self) -> &crate::compat::crdt::Shared {
        &self.inner.aio_sync
    }

    /// A synced client's call (see `Store::call_from`); `Ok(None)`: a re-send, not applied again.
    pub fn call_from(&self, method: &str, args: Value, op: &str) -> Result<Option<Value>> {
        self.enter(None, method, args, Some(op)).map(|(v, _)| v)
    }

    /// A call from the network by `who`, with the synced client's `op` if
    /// any: refused unless the method's `#[client]` rule lets `who` in, asked
    /// on the very state the call then runs on.
    #[cfg_attr(not(feature = "server"), allow(dead_code))]
    pub(crate) fn call_by(
        &self,
        who: &crate::Caller,
        method: &str,
        args: Value,
        op: Option<&str>,
    ) -> Result<Option<Value>> {
        self.enter(Some(who), method, args, op).map(|(v, _)| v)
    }

    /// [`Self::call_by`], followed to its end: an async method's answer comes later.
    pub(crate) fn ask_by(
        &self,
        who: Option<&crate::Caller>,
        method: &str,
        args: Value,
        op: Option<&str>,
    ) -> Result<(Option<Value>, crate::Answer)> {
        self.enter(who, method, args, op)
    }

    /// Committed events from now on, each with the event before it. A
    /// subscriber that falls behind gets `Lagged` and must take a full state.
    pub fn events(&self) -> tokio::sync::broadcast::Receiver<(u64, crate::Event)> {
        self.inner.events.subscribe()
    }

    /// What its calls tell the people using the app (`cx.tell`), from now on.
    pub fn tells(&self) -> tokio::sync::broadcast::Receiver<crate::tell::Tell> {
        self.inner.tells.subscribe()
    }

    /// Show `t` in this process and send it to the synced clients; with
    /// neither, it is said in the log, not lost silently.
    fn say(&self, t: crate::tell::Tell) {
        let shown = crate::tell::show(&t);
        if self.inner.tells.send(t.clone()).is_err() && !shown {
            eprintln!(
                "[aio] {}: nobody to tell (no window, no client): {}",
                T::ID,
                crate::json(&t).unwrap_or_default()
            );
        }
    }

    /// The state and the last event in it, read together.
    pub fn snapshot(&self) -> (u64, Arc<T>) {
        lock(&self.inner.shown).clone()
    }

    /// The whole state as aio's `app.snapshot()` has it: an object of cells,
    /// each its whole state (`#[server]` fields too), a `#[aio_cell]` field
    /// under its aio cell's name. What [`Self::load_snapshot`] takes back.
    pub fn export_snapshot(&self) -> Result<Value> {
        Ok(snapshot_cells::<T>(crate::json(&*self.state())?))
    }

    /// Replace the state with `snap`, as [`Self::export_snapshot`] made it (aio's
    /// `loadSnapshot`): a new event, saved before it returns, sent to every
    /// client. Refused when it isn't an object of objects, or when its cells
    /// aren't this app's (a file from another app would wipe the cells it
    /// lacks); `force` loads it anyway: a missing cell starts from its
    /// defaults, an unknown one is dropped (said).
    pub fn load_snapshot(&self, snap: &Value, force: bool) -> Result {
        let st = snapshot_state::<T>(snap, force)?;
        self.store(|s| s.load(&By::System, st))
    }

    /// How many calls of async method `method` are in flight (its effects
    /// running or queued, under any `#[ttl]` key); `None`: every effect of
    /// the cell. A count, never a flag: two overlapping calls are two.
    /// `on_pending` tells when it changes, so a view re-renders on it.
    pub fn pending(&self, method: Option<&str>) -> usize {
        let of = |k: &str| method.is_none_or(|m| k.strip_prefix(m).is_some_and(|r| r.is_empty() || r.starts_with(':')));
        lock(&self.inner.running).iter().filter(|(k, _)| of(k)).map(|(_, rs)| rs.len()).sum()
    }

    /// Wait until `f` holds on the state (aio's `until`), checked now and after
    /// each commit; refused if it doesn't within `timeout`.
    pub async fn until(&self, timeout: Duration, f: impl Fn(&T) -> bool) -> Result {
        let mut ev = self.events();
        let wait = async {
            // A lagged receiver only skipped events: the state is checked anew.
            while !f(&self.state()) {
                _ = ev.recv().await;
            }
        };
        match tokio::time::timeout(timeout, wait).await {
            Ok(()) => Ok(()),
            Err(_) => crate::reject!("{}: the condition did not hold within {timeout:?}", T::ID),
        }
    }

    /// Is an effect with this key still running?
    pub fn busy(&self, key: &str) -> bool {
        lock(&self.inner.running).contains_key(key)
    }

    /// Is `d` a `skip_if_running` schedule whose last tick's effect (keyed
    /// by the method) still runs?
    pub(crate) fn skips(&self, d: &crate::Due) -> bool {
        let declared = d.key.strip_prefix('@');
        declared.is_some_and(|m| T::SCHEDULES.iter().any(|s| s.skip_if_running && s.method == m && self.busy(m)))
    }

    /// Fire the schedules due now (none while disabled or paused): when the
    /// next one is due, and now.
    pub(crate) fn fire(&self) -> (Option<i64>, i64) {
        let (next, now) = {
            let mut s = lock(&self.inner.store);
            let now = s.now();
            if !self.enabled() || self.paused() {
                return (None, now);
            }
            if let Err(e) = s.fire_due_but(now, |d| self.skips(d)) {
                eprintln!("[aio] {}: schedules: {e}", T::ID);
            }
            // A held run's idle time is a wake-up too.
            let idle = s.flush_idle().map(|d| now + d.as_millis() as i64 + 1);
            let due = s.schedules().ok().and_then(|d| d.first().map(|d| d.due));
            (due.into_iter().chain(idle).min(), now)
        };
        // Its own fires must not wake it again.
        self.after_commit(false);
        (next, now)
    }

    /// After a call: a commit tells everyone; a refusal may be a failure.
    fn settle<R>(&self, out: &Result<R>) {
        match out {
            Ok(_) => self.after_commit(true),
            Err(_) => self.count_failures(),
        }
    }

    /// Fire saved schedules on time, until the task is dropped or aborted.
    /// Schedules missed while the app was off fire once, right away.
    pub fn start(&self) -> tokio::task::JoinHandle<()> {
        _ = self.live(true);
        let h = self.clone();
        tokio::spawn(async move {
            loop {
                // Register before looking, so a change made meanwhile still wakes us.
                let mut woken = std::pin::pin!(h.inner.wake.notified());
                woken.as_mut().enable();
                if h.closing() {
                    return;
                }
                // On the blocking pool: a slow scheduled method holds no
                // runtime thread, so other cells and the sockets go on.
                let f = h.clone();
                let Ok((next, now)) = tokio::task::spawn_blocking(move || f.fire()).await else { return };
                let wait = wait_ms(next, now);
                tokio::select! {
                    _ = woken => {}
                    _ = tokio::time::sleep(std::time::Duration::from_millis(wait)) => {}
                }
            }
        })
    }
}

/// Held by an effect's task and dropped when it ends, however it ends (a
/// panic or an abort drops it too): takes it off `running`, lets the next
/// queued one start, and tells `done` and `shutdown`.
struct Ended<T: Cell> {
    inner: Arc<Inner<T>>,
    key: String,
    id: u64,
    _next: Option<oneshot::Sender<()>>,
}

impl<T: Cell> Drop for Ended<T> {
    fn drop(&mut self) {
        let mut running = lock(&self.inner.running);
        if let Some(rs) = running.get_mut(&self.key) {
            rs.retain(|(id, _)| *id != self.id);
            if rs.is_empty() {
                running.remove(&self.key);
            }
        }
        drop(running);
        tell(&self.inner);
        self.inner.ended.notify_waiters();
    }
}

/// Tell the `on_pending` watchers.
fn tell<T: Cell>(inner: &Inner<T>) {
    let ws = lock(&inner.watchers).clone();
    ws.iter().for_each(|f| f());
}

/// How long the scheduler sleeps: until the next due time on the wall
/// clock, but never over a minute, so a clock step or a machine's sleep is
/// noticed within a minute (the timer itself counts monotonic time).
fn wait_ms(next: Option<i64>, now: i64) -> u64 {
    next.map_or(60_000, |due| (due - now).clamp(0, 60_000)) as u64
}

impl<T: Cell> Door<T> for Handle<T> {
    fn call(&self, method: &str, args: Value) -> Result<Value> {
        self.enter(None, method, args, None).map(|(v, _)| v.unwrap_or_default())
    }

    fn ask(&self, method: &str, args: Value) -> Result<crate::Answer> {
        self.enter(None, method, args, None).map(|(_, a)| a)
    }

    fn read<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        f(&self.state())
    }

    fn own(&self, key: &str, res: Option<Box<dyn Any + Send>>) {
        self.keep(key, res)
    }
}

/// The cells of `T` a snapshot names: its own, then its `#[aio_cell]` slices'.
fn snapshot_names<T: Cell>() -> Vec<&'static str> {
    std::iter::once(T::ID).chain(T::AIO_CELLS.iter().map(|s| s.cell)).collect()
}

/// `T`'s state (`json`) as a snapshot of cells (pure).
fn snapshot_cells<T: Cell>(json: Value) -> Value {
    let mut own = match json {
        Value::Object(m) => m,
        v => return serde_json::json!({ T::ID: v }),
    };
    let mut out = serde_json::Map::new();
    for s in T::AIO_CELLS {
        if let Some(v) = own.remove(s.field) {
            out.insert(s.cell.into(), v);
        }
    }
    out.insert(T::ID.into(), own.into());
    out.into()
}

/// A snapshot of cells back as `T`'s state, checked as aio checks one
/// (`snapshotShapeError`, `snapshotCellsError`): pure but for the warnings.
pub(crate) fn snapshot_state<T: Cell>(snap: &Value, force: bool) -> Result<Value> {
    let Some(cells) = snap.as_object() else {
        crate::reject!("snapshot must be a JSON object: pass the exact snapshot export gave");
    };
    for (c, v) in cells {
        if !v.is_object() {
            let got = match v {
                Value::Null => "null",
                Value::Array(_) => "an array",
                Value::String(_) => "a string",
                Value::Number(_) => "a number",
                _ => "a boolean",
            };
            crate::reject!("snapshot value for cell \"{c}\" must be an object (a cell's state is always an object), got {got}");
        }
    }
    let names = snapshot_names::<T>();
    let missing: Vec<&str> = names.iter().copied().filter(|c| !cells.contains_key(*c)).collect();
    let unknown: Vec<&str> = cells.keys().map(String::as_str).filter(|c| !names.contains(c)).collect();
    let quoted = |cs: &[&str]| cs.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(", ");
    if !force && (!missing.is_empty() || !unknown.is_empty()) {
        let mut parts = vec![];
        if !missing.is_empty() {
            parts.push(format!("it has nothing for {}, whose state would be DESTROYED", quoted(&missing)));
        }
        if !unknown.is_empty() {
            parts.push(format!("it carries cells this app does not declare: {}", quoted(&unknown)));
        }
        crate::reject!(
            "snapshot refused: {}. A snapshot replaces the WHOLE state, so this looks like a file from a different app. This app declares: {}. Load it anyway with force (`?force=1`)",
            parts.join("; and "),
            quoted(&names)
        );
    }
    if !unknown.is_empty() {
        eprintln!("[aio] snapshot: dropped cells this app does not declare: {}", quoted(&unknown));
    }
    if !missing.is_empty() {
        eprintln!("[aio] snapshot: {} start from their defaults", quoted(&missing));
    }
    let mut own = match cells.get(T::ID) {
        Some(Value::Object(m)) => m.clone(),
        _ => serde_json::Map::new(),
    };
    for s in T::AIO_CELLS {
        own.insert(s.field.into(), cells.get(s.cell).cloned().unwrap_or_else(|| serde_json::json!({})));
    }
    Ok(own.into())
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_scheduler_wakes_at_least_every_minute() {
        assert_eq!(super::wait_ms(None, 0), 60_000);
        assert_eq!(super::wait_ms(Some(30 * 86_400_000), 0), 60_000, "a far slot is re-checked");
        assert_eq!(super::wait_ms(Some(500), 0), 500);
        assert_eq!(super::wait_ms(Some(-5), 0), 0, "overdue: now");
    }
}

//! A cell of the page itself: its state in memory, saved to the browser's
//! storage after each call when it persists. A call is atomic as on the
//! native store (its machine, the method, the invariants), then its effects
//! start on the page's tasks, keyed and moded as there. Its schedules run
//! on the page's timer and are kept with its state, so a reload keeps them
//! (missed ones fire once); its `#[ttl]` answers are kept as natively.
use super::storage;
use crate::effect::{Abort, Mode};
use crate::{By, Call, Cell, Cx, Door, Effect, Result};
use futures_util::future::{AbortHandle, Abortable};
use serde_json::Value;
use std::any::Any;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

type Listener<T> = Arc<dyn Fn(&Arc<T>) + Send + Sync>;

struct Inner<T: Cell> {
    state: Mutex<Arc<T>>,
    /// Its document key in the browser's storage; `None`: memory only.
    key: Option<String>,
    listeners: Mutex<Vec<Listener<T>>>,
    watchers: Mutex<Vec<Arc<dyn Fn() + Send + Sync>>>,
    owned: Mutex<HashMap<String, Box<dyn Any + Send>>>,
    /// The effects running (or queued) per key, each with its number.
    running: Mutex<HashMap<String, Vec<(u64, AbortHandle)>>>,
    /// Per key, the end of the last queued effect: the next one waits for it.
    tails: Mutex<HashMap<String, tokio::sync::oneshot::Receiver<()>>>,
    ran: AtomicU64,
    /// The `#[mode(first)]` and `#[mode(newest)]` calls in flight.
    inflight: crate::answer::Inflight,
    /// `#[ttl]` calls answered.
    answered: Mutex<crate::answer::Cache>,
    /// Its schedules, and the page's timer for the next one.
    sched: Mutex<Vec<storage::Due>>,
    timer: Mutex<Option<i32>>,
}

pub struct Handle<T: Cell> {
    inner: Arc<Inner<T>>,
    by: By,
}

impl<T: Cell> Clone for Handle<T> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone(), by: self.by.clone() }
    }
}

fn lock<X>(m: &Mutex<X>) -> MutexGuard<'_, X> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What is kept of `st`: its `save` hook's, without what it doesn't save
/// (`#[transient]`, `persist(include, exclude)`; pure).
fn kept<T: Cell>(st: &T) -> Value {
    let mut v = serde_json::to_value(st.save().as_ref().unwrap_or(st)).unwrap_or(Value::Null);
    crate::filter::strip(&mut v, crate::filter::Filter { include: T::PERSIST_INCLUDE, exclude: T::TRANSIENT });
    v
}

impl<T: Cell> Handle<T> {
    fn new(st: T, key: Option<String>, mut sched: Vec<storage::Due>) -> Self {
        if let Err(e) = storage::declare(&mut sched, T::SCHEDULES, super::now_ms()) {
            super::say(&format!("{}.{e}", T::ID));
        }
        let inner = Inner {
            state: Mutex::new(Arc::new(st)),
            key,
            listeners: Mutex::default(),
            watchers: Mutex::default(),
            owned: Mutex::default(),
            running: Mutex::default(),
            tails: Mutex::default(),
            ran: AtomicU64::new(0),
            inflight: Default::default(),
            answered: Mutex::default(),
            sched: Mutex::new(sched),
            timer: Mutex::default(),
        };
        let h = Self { inner: Arc::new(inner), by: By::System };
        h.arm();
        h
    }

    /// A fresh cell, in memory only.
    pub fn memory() -> Self {
        Self::new(T::default(), None, Vec::new())
    }

    /// App `app`'s cell, back as the browser kept it (`#[cell(persist)]`
    /// is saved after each call; another starts fresh), its `restore` run.
    pub fn saved(app: &str) -> Self {
        let key = T::PERSIST.then(|| storage::key(app, T::ID));
        let doc = key.as_deref().and_then(|k| storage::load(&super::Local, k, super::say));
        let sched = doc.as_ref().filter(|d| d.cell == T::ID).map(|d| d.sched.clone()).unwrap_or_default();
        let st = match doc {
            Some(d) if d.cell == T::ID => match T::migrate(d.view, d.v) {
                Ok(st) => st.restore(),
                Err(e) => {
                    super::say(&format!("{}: the saved state can't be read ({e}); starting fresh", T::ID));
                    T::default()
                }
            },
            _ => T::default(),
        };
        Self::new(st, key, sched)
    }

    /// The same cell, with calls recorded as caused by `by`.
    pub fn by(&self, by: By) -> Self {
        Self { inner: self.inner.clone(), by }
    }

    pub fn state(&self) -> Arc<T> {
        lock(&self.inner.state).clone()
    }

    /// Call `f` with the state after each call.
    pub fn listen(&self, f: impl Fn(&Arc<T>) + Send + Sync + 'static) {
        lock(&self.inner.listeners).push(Arc::new(f));
    }

    /// Call `f` when `pending` may have changed (an effect started or ended).
    pub fn on_pending(&self, f: impl Fn() + Send + Sync + 'static) {
        lock(&self.inner.watchers).push(Arc::new(f));
    }

    /// Effects of async method `method` running or queued (`None`: all).
    pub fn pending(&self, method: Option<&str>) -> usize {
        let of = |k: &str| method.is_none_or(|m| k.strip_prefix(m).is_some_and(|r| r.is_empty() || r.starts_with(':')));
        lock(&self.inner.running).iter().filter(|(k, _)| of(k)).map(|(_, rs)| rs.len()).sum()
    }

    /// Is an effect with this key still running?
    pub fn busy(&self, key: &str) -> bool {
        lock(&self.inner.running).contains_key(key)
    }

    /// Run one call atomically; an async method's end goes to `reply`.
    fn run(&self, method: &str, args: Value, reply: Option<crate::Reply>) -> Result<Value> {
        let args =
            crate::ask_before(&Call { cell: T::ID, method, args: &args, by: &self.by, role: None })?.unwrap_or(args);
        let cx = Cx::live(super::now_ms(), &self.by).with_reply(reply);
        let mut st = (*self.state()).clone();
        st.advance(method)?;
        let out = st.apply(&cx, method, args)?;
        st.check()?;
        let asks = cx.into_asks();
        // A schedule names a method by its name: a typo is refused now, by the
        // call that set it, not found when it fires.
        for p in &asks.plans {
            if let crate::Plan::Set { key, method: m, .. } = p
                && !T::METHODS.contains(&m.as_str())
            {
                crate::reject!(
                    "schedule `{key}`: {} has no method `{m}` (its methods: {})",
                    T::ID,
                    T::METHODS.join(", ")
                );
            }
        }
        let planned = !asks.plans.is_empty();
        {
            let mut sched = lock(&self.inner.sched);
            for p in asks.plans {
                match p {
                    crate::Plan::Set { key, due, every, cron, method, args } => {
                        sched.retain(|d| d.key != key);
                        sched.push(storage::Due { key, due, every, cron, method, args });
                    }
                    crate::Plan::Cancel(key) => sched.retain(|d| d.key != key),
                }
            }
        }
        self.commit(st, asks.effects);
        if planned {
            self.arm();
        }
        Ok(out)
    }

    /// Wake for the next schedule on the page's timer: at its time, but
    /// never over a minute away, so a clock step or a sleeping machine is
    /// noticed within a minute (the native scheduler's rule).
    fn arm(&self) {
        use wasm_bindgen::JsCast;
        let Some(w) = web_sys::window() else { return };
        if let Some(id) = lock(&self.inner.timer).take() {
            w.clear_timeout_with_handle(id);
        }
        let Some(next) = lock(&self.inner.sched).iter().map(|d| d.due).min() else { return };
        let wait = (next - super::now_ms()).clamp(0, 60_000) as i32;
        let h = self.clone();
        let f = wasm_bindgen::closure::Closure::once_into_js(move || h.fire());
        if let Ok(id) = w.set_timeout_with_callback_and_timeout_and_arguments_0(f.unchecked_ref(), wait) {
            *lock(&self.inner.timer) = Some(id);
        }
    }

    /// Run the schedules due now, in time order, each as its own call
    /// (`By::Schedule`): re-armed first, so a refused one can't fire in a
    /// loop, and a `skip_if_running` one whose last tick's effect still runs
    /// only re-armed. Then wait for the next.
    fn fire(&self) {
        lock(&self.inner.timer).take();
        let now = super::now_ms();
        let mut due: Vec<storage::Due> = lock(&self.inner.sched).iter().filter(|d| d.due <= now).cloned().collect();
        due.sort_by_key(|d| d.due);
        for d in &due {
            storage::rearm(&mut lock(&self.inner.sched), d, now);
            let skips = d
                .key
                .strip_prefix('@')
                .is_some_and(|m| T::SCHEDULES.iter().any(|s| s.skip_if_running && s.method == m && self.busy(m)));
            if skips {
                continue;
            }
            if let Err(e) = self.by(By::Schedule(d.key.clone())).run(&d.method, d.args.clone(), None) {
                super::say(&format!("{}: schedule `{}` ({}) refused: {e}", T::ID, d.key, d.method));
            }
        }
        if !due.is_empty() {
            self.save(&self.state());
        }
        self.arm();
    }

    /// A call, followed to its end when it is an async method's; one with
    /// a `#[ttl]` answer still good is answered without running.
    fn enter(&self, method: &str, args: Value) -> Result<(Value, crate::Answer)> {
        if !lock(&self.inner.answered).is_empty()
            && let Some(v) = lock(&self.inner.answered).get(method, &args, super::now_ms())
        {
            return Ok((v.clone(), crate::Answer::ready(Ok(v))));
        }
        let Some(mode) = T::async_mode(method) else {
            let v = self.run(method, args, None)?;
            return Ok((v.clone(), crate::Answer::ready(Ok(v))));
        };
        let (reply, answer) = crate::Reply::new(method);
        let key = crate::answer::key_of::<T>(method, &args);
        if let Some(shared) = self.inner.inflight.enter(mode, key, &reply) {
            return Ok((Value::Null, shared));
        }
        match self.run(method, args, Some(reply.clone())) {
            Ok(v) => Ok((v, answer)),
            Err(e) => {
                reply.send(Err(e.clone()));
                Err(e)
            }
        }
    }

    /// An async method's step, for the call whose end goes to `reply`.
    pub(crate) fn resume(&self, method: &str, args: Value, reply: Option<crate::Reply>) -> Result<Value> {
        let base = method.split_once('.').map_or(method, |(m, _)| m);
        if let (Some(Mode::Newest), Some(r), By::Effect(key)) = (T::async_mode(base), &reply, &self.by)
            && !self.inner.inflight.current(key, r)
        {
            crate::reject!("{}.{base}: superseded by a newer call (#[mode(newest)])", T::ID);
        }
        self.run(method, args, reply)
    }

    /// An async method's await refused (`io.await?`): said in the page's log.
    pub(crate) fn failed(&self, why: &str) {
        super::say(&format!("{}.{why}", T::ID));
    }

    /// Keep `st` and the schedules in the browser's storage, when it persists.
    fn save(&self, st: &T) {
        let Some(key) = &self.inner.key else { return };
        let doc = storage::Doc {
            cell: T::ID.into(),
            v: T::VERSION,
            user: String::new(),
            seq: 0,
            view: kept(st),
            pending: Vec::new(),
            sched: lock(&self.inner.sched).clone(),
        };
        if let Err(e) = storage::save(&super::Local, key, doc, &Default::default(), super::say) {
            super::say(&format!("{}: not saved in this browser ({e}); it lives on in this page", T::ID));
        }
    }

    fn commit(&self, st: T, effects: Vec<crate::cx::Asked>) {
        let st = Arc::new(st);
        *lock(&self.inner.state) = st.clone();
        self.save(&st);
        for e in effects {
            match e.downcast::<Effect<T>>() {
                Ok(e) => self.spawn(*e),
                Err(e) => match e.downcast::<Abort>() {
                    Ok(a) => self.abort(&a.0),
                    Err(e) => match e.downcast::<crate::tell::Tell>() {
                        Ok(t) if !crate::tell::show(&t) => super::say(&format!("{}: nobody to tell: {t:?}", T::ID)),
                        Ok(_) => {}
                        Err(e) => match e.downcast::<crate::answer::Answered>() {
                            Ok(a) => {
                                a.reply.iter().for_each(|r| r.send(Ok(a.value.clone())));
                                if let Some((key, ms)) = a.ttl {
                                    lock(&self.inner.answered).keep(key, super::now_ms(), ms, a.value);
                                }
                            }
                            Err(_) => super::say(&format!("{}: an effect for another cell was dropped", T::ID)),
                        },
                    },
                },
            }
        }
        let ls = lock(&self.inner.listeners).clone();
        ls.iter().for_each(|f| f(&st));
    }

    fn watchers(&self) {
        let ws = lock(&self.inner.watchers).clone();
        ws.iter().for_each(|f| f());
    }

    fn abort(&self, key: &str) {
        if let Some(rs) = lock(&self.inner.running).get(key) {
            rs.iter().for_each(|(_, a)| a.abort());
        }
    }

    /// Start `e` as its mode says: `First` skips while one runs, `Newest`
    /// cancels the running ones, `Queue` waits for the one asked before.
    fn spawn(&self, e: Effect<T>) {
        let (next, after) = {
            let mut running = lock(&self.inner.running);
            let busy = running.contains_key(&e.key);
            match e.mode {
                Mode::First if busy => return,
                Mode::Newest if busy => running[&e.key].iter().for_each(|(_, a)| a.abort()),
                _ => {}
            }
            crate::effect_started(T::ID, &e.key);
            let (next, after) = match e.mode {
                Mode::Queue => {
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    (Some(tx), lock(&self.inner.tails).insert(e.key.clone(), rx))
                }
                _ => (None, None),
            };
            let id = self.inner.ran.fetch_add(1, Ordering::Relaxed);
            let (abort, reg) = AbortHandle::new_pair();
            running.entry(e.key.clone()).or_default().push((id, abort));
            ((next, id, reg), after)
        };
        let ((next, id, reg), key, timeout) = (next, e.key.clone(), e.timeout);
        let run = (e.run)(self.by(By::Effect(key.clone())));
        let h = self.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let _next = next;
            if let Some(rx) = after {
                _ = rx.await;
            }
            let run = Abortable::new(run, reg);
            match timeout {
                None => _ = run.await,
                Some(d) => {
                    if crate::effect::within(d, run).await.is_none() {
                        super::say(&format!("{}: effect `{key}` timed out after {d:?}", T::ID));
                    }
                }
            }
            {
                let mut running = lock(&h.inner.running);
                if let Some(rs) = running.get_mut(&key) {
                    rs.retain(|(i, _)| *i != id);
                    if rs.is_empty() {
                        running.remove(&key);
                    }
                }
            }
            h.watchers();
        });
        self.watchers();
    }
}

impl<T: Cell> Door<T> for Handle<T> {
    fn call(&self, method: &str, args: Value) -> Result<Value> {
        self.enter(method, args).map(|(v, _)| v)
    }

    fn ask(&self, method: &str, args: Value) -> Result<crate::Answer> {
        self.enter(method, args).map(|(_, a)| a)
    }

    fn read<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        f(&self.state())
    }

    fn own(&self, key: &str, res: Option<Box<dyn Any + Send>>) {
        let old = match res {
            Some(r) => lock(&self.inner.owned).insert(key.into(), r),
            None => lock(&self.inner.owned).remove(key),
        };
        drop(old);
    }
}

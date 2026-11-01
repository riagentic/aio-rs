//! An async method's end, for its caller (aio's `await cell.method()`): the
//! value its last part returned, or the refusal that ended it after an
//! await. Each part still commits on its own and the log holds every awaited
//! result, so replay stays exact; the answer itself is never logged.
use crate::{Cx, Rejected, Result};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{
    collections::HashMap,
    future::Future,
    marker::PhantomData,
    pin::Pin,
    sync::{Arc, Mutex, Weak},
    task::{Context, Poll},
};
use tokio::sync::oneshot;

type Tx = oneshot::Sender<Result<Value>>;

/// Where one async call's end goes; every part of the call carries a clone.
/// The first answer sent wins. Dropped unanswered (its I/O aborted, cut off
/// at the call ceiling, superseded, or the app stopped), the caller is told so.
#[doc(hidden)]
#[derive(Clone)]
pub struct Reply(Arc<Slot>);

pub(crate) struct Slot {
    method: String,
    /// The caller, and any `#[mode(first)]` caller that shares its answer.
    to: Mutex<Vec<Tx>>,
    /// Run once the call is gone (a transaction's end, [`Reply::on_drop`]).
    gone: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl Drop for Slot {
    fn drop(&mut self) {
        if let Some(f) = self.gone.get_mut().unwrap_or_else(|e| e.into_inner()).take() {
            f();
        }
        let why = format!(
            "{}: ended before it answered (its await was aborted, superseded, cut off after the call ceiling, \
             or the app stopped)",
            self.method
        );
        for tx in std::mem::take(&mut *self.to.lock().unwrap_or_else(|e| e.into_inner())) {
            _ = tx.send(Err(Rejected::new(why.clone())));
        }
    }
}

impl Reply {
    pub(crate) fn new(method: &str) -> (Self, Answer) {
        let (tx, rx) = oneshot::channel();
        (
            Self(Arc::new(Slot { method: method.into(), to: Mutex::new(vec![tx]), gone: Mutex::default() })),
            Answer::waiting(rx),
        )
    }

    /// This call, among the calls in flight.
    pub(crate) fn id(&self) -> usize {
        Arc::as_ptr(&self.0) as usize
    }

    /// Whether this call is still in flight (anything still holds its reply).
    pub(crate) fn watch(&self) -> Weak<Slot> {
        Arc::downgrade(&self.0)
    }

    /// Run `f` once nothing holds this call any more (its last part ran,
    /// or its await was aborted or cut off).
    pub(crate) fn on_drop(&self, f: impl FnOnce() + Send + 'static) {
        *self.0.gone.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(f));
    }

    /// Send the end to whoever waits (once: later sends are dropped).
    pub(crate) fn send(&self, r: Result<Value>) {
        for tx in std::mem::take(&mut *self.0.to.lock().unwrap_or_else(|e| e.into_inner())) {
            _ = tx.send(r.clone());
        }
    }

    /// Another caller waits for this call's end (`#[mode(first)]`); `None`:
    /// it has already answered.
    pub(crate) fn join(&self) -> Option<Answer> {
        let mut to = self.0.to.lock().unwrap_or_else(|e| e.into_inner());
        if to.is_empty() {
            return None;
        }
        let (tx, rx) = oneshot::channel();
        to.push(tx);
        Some(Answer::waiting(rx))
    }
}

/// The async calls in flight a `#[mode(first)]` or `#[mode(newest)]`
/// method's next call looks at, by effect key (the method, or with a
/// `#[ttl]` the method and its arguments). One per cell.
#[derive(Default)]
pub(crate) struct Inflight {
    first: Mutex<HashMap<String, Weak<Slot>>>,
    newest: Mutex<HashMap<String, Weak<Slot>>>,
}

impl Inflight {
    /// A new call of `method`, whose end goes to `reply`: under `First`,
    /// the running one's answer to share instead (aio's `concurrency:
    /// "first"`: the second caller adopts it, nothing runs); under
    /// `Newest`, it is the one whose steps go on from now on.
    pub(crate) fn enter(&self, mode: crate::Mode, key: String, reply: &Reply) -> Option<Answer> {
        let map = match mode {
            crate::Mode::Queue => return None,
            crate::Mode::First => &self.first,
            crate::Mode::Newest => &self.newest,
        };
        let mut m = map.lock().unwrap_or_else(|e| e.into_inner());
        if mode == crate::Mode::First
            && let Some(a) = m.get(&key).and_then(Weak::upgrade).and_then(|s| Reply(s).join())
        {
            return Some(a);
        }
        m.retain(|_, w| w.strong_count() > 0);
        m.insert(key, Arc::downgrade(&reply.0));
        None
    }

    /// May this step of a `Newest` call go on? Not once a newer call came.
    pub(crate) fn current(&self, key: &str, reply: &Reply) -> bool {
        let m = self.newest.lock().unwrap_or_else(|e| e.into_inner());
        m.get(key).is_none_or(|w| std::ptr::eq(w.as_ptr(), Arc::as_ptr(&reply.0)))
    }
}

/// The effect key a call of async `method` with `args` runs under.
pub(crate) fn key_of<T: crate::Cell>(method: &str, args: &Value) -> String {
    match T::TTL.contains(&method) {
        true => crate::effect::key(method, args).unwrap_or_else(|_| method.into()),
        false => method.into(),
    }
}

/// Asked by a call whose method ended: its answer is sent once the call
/// commits (never if it is refused); with a `#[ttl]`, it is kept under its
/// key for `ms`.
pub(crate) struct Answered {
    pub(crate) reply: Option<Reply>,
    pub(crate) value: Value,
    pub(crate) ttl: Option<(String, i64)>,
}

/// An async method's end, to `.await`: `h.checkout(item)?.await?` is the
/// value its last part returned, or the refusal that ended it after an
/// await (a step's `ensure!`, an awaited `?`, its I/O cut off). The call
/// itself runs whether or not this is awaited: drop it to fire and forget.
pub struct Answer<R = Value> {
    now: Option<Result<Value>>,
    rx: Option<oneshot::Receiver<Result<Value>>>,
    _r: PhantomData<fn() -> R>,
}

impl<R> std::fmt::Debug for Answer<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.now {
            Some(r) => write!(f, "Answer({r:?})"),
            None => write!(f, "Answer(waiting)"),
        }
    }
}

impl Answer {
    pub(crate) fn ready(r: Result<Value>) -> Self {
        Self { now: Some(r), rx: None, _r: PhantomData }
    }

    fn waiting(rx: oneshot::Receiver<Result<Value>>) -> Self {
        Self { now: None, rx: Some(rx), _r: PhantomData }
    }

    /// The typed answer of a `<Cell>Calls` method.
    #[doc(hidden)]
    pub fn typed<R>(self) -> Answer<R> {
        Answer { now: self.now, rx: self.rx, _r: PhantomData }
    }
}

impl<R: DeserializeOwned> Answer<R> {
    /// The end if it came already (a sync test after `TestCells::settle`);
    /// `None` while the method still runs.
    pub fn now(&mut self) -> Option<Result<R>> {
        if self.now.is_none()
            && let Some(rx) = &mut self.rx
        {
            match rx.try_recv() {
                Ok(r) => self.now = Some(r),
                Err(oneshot::error::TryRecvError::Empty) => return None,
                Err(oneshot::error::TryRecvError::Closed) => self.now = Some(Err(lost())),
            }
        }
        self.now.take().map(crate::typed)
    }
}

fn lost() -> Rejected {
    Rejected::new("the async method's answer was lost: its cell was dropped")
}

impl<R: DeserializeOwned> Future for Answer<R> {
    type Output = Result<R>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<R>> {
        if let Some(r) = self.now.take() {
            return Poll::Ready(crate::typed(r));
        }
        let Some(rx) = self.rx.as_mut() else { return Poll::Ready(Err(lost())) };
        Pin::new(rx).poll(cx).map(|r| crate::typed(r.unwrap_or_else(|_| Err(lost()))))
    }
}

/// Where an async method's code goes next.
#[doc(hidden)]
pub enum Step<S, R> {
    /// It returned: the call's answer.
    Done(R),
    /// It awaits: an effect resumes it in a later step.
    Wait,
    /// It goes on in another part of this call (a branch's join, a loop's
    /// next turn), run here: no recursion however long the loop.
    Goto(Next<S, R>),
}

/// The rest of a lowered async method's part, run next in the same call.
#[doc(hidden)]
pub type Next<S, R> = Box<dyn FnOnce(&mut S, &Cx) -> Result<Step<S, R>>>;

/// Run a lowered async method's part to its end: its gotos in turn, then
/// its answer (asked to be sent after the commit) or `null` while it awaits.
#[doc(hidden)]
pub fn finish<S, R: Serialize>(
    s: &mut S,
    cx: &Cx,
    mut r: Result<Step<S, R>>,
    ttl: Option<(String, i64)>,
) -> Result<Value> {
    loop {
        match r? {
            Step::Goto(f) => r = f(s, cx),
            Step::Wait => return Ok(Value::Null),
            Step::Done(v) => {
                let v = crate::json(&v)?;
                cx.answer(v.clone(), ttl);
                return Ok(v);
            }
        }
    }
}

/// A `#[ttl]` step's answer key: its effect's (the method and arguments).
#[doc(hidden)]
pub fn ttl_key(cx: &Cx, method: &str, ms: i64) -> Option<(String, i64)> {
    let key = match cx.by() {
        crate::By::Effect(k) => k.clone(),
        _ => method.to_string(),
    };
    Some((key, ms))
}

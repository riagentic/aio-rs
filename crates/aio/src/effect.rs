//! Effects: async I/O a method asks for. It starts only after the call
//! commits, and hands its result back as another method call, so the log
//! holds results, never I/O, and replay never repeats it.
use crate::{Cell, Handle};
use std::{future::Future, pin::Pin, time::Duration};

/// An effect's run. A page's I/O (its timers, `fetch`) is not `Send`: there the
/// effect runs on the page's one thread, so it needn't be.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) type Fut = Pin<Box<dyn Future<Output = ()> + Send>>;
#[cfg(target_arch = "wasm32")]
pub(crate) type Fut = Pin<Box<dyn Future<Output = ()>>>;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) type Run<T> = Box<dyn FnOnce(Handle<T>) -> Fut + Send>;
#[cfg(target_arch = "wasm32")]
pub(crate) type Run<T> = Box<dyn FnOnce(Handle<T>) -> Fut>;

/// `Send` natively; anything in a page (an effect's future).
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSend: Send {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send> MaybeSend for T {}
#[cfg(target_arch = "wasm32")]
pub trait MaybeSend {}
#[cfg(target_arch = "wasm32")]
impl<T> MaybeSend for T {}

/// How long each await of an async method may take, unless it is `#[long]`:
/// one that never settles is cut off (said in the log and a report) and its
/// later steps never run, instead of hanging its caller forever.
pub const CALL_CEILING: Duration = Duration::from_secs(30);

/// aio's `call({ timeoutMs, retries })`: run `f`, each try cut off after
/// `timeout` (if set), and again up to `retries` more times while it fails;
/// the last failure if every try did.
pub async fn retry<R, E: std::fmt::Display, F: Future<Output = Result<R, E>>>(
    retries: u32,
    timeout: Option<Duration>,
    mut f: impl FnMut() -> F,
) -> crate::Result<R> {
    let mut left = retries;
    loop {
        let out = match timeout {
            Some(d) => match within(d, f()).await {
                Some(r) => r.map_err(|e| e.to_string()),
                None => Err(format!("timed out after {d:?}")),
            },
            None => f().await.map_err(|e| e.to_string()),
        };
        match out {
            Ok(r) => return Ok(r),
            Err(e) if left == 0 => return Err(crate::Rejected::new(e)),
            Err(_) => left -= 1,
        }
    }
}

/// `f`'s output, unless `d` passes first: on tokio's clock natively, on the
/// page's timer in a browser.
pub(crate) async fn within<R>(d: Duration, f: impl Future<Output = R>) -> Option<R> {
    #[cfg(not(target_arch = "wasm32"))]
    return tokio::time::timeout(d, f).await.ok();
    #[cfg(target_arch = "wasm32")]
    {
        let late = std::pin::pin!(crate::web::sleep(d.as_millis().min(u32::MAX.into()) as u32));
        match futures_util::future::select(std::pin::pin!(f), late).await {
            futures_util::future::Either::Left((r, _)) => Some(r),
            futures_util::future::Either::Right(_) => None,
        }
    }
}

/// Two effects with the same key, the second starting while the first runs:
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// the second waits for the first (nothing lost, nothing cancelled).
    #[default]
    Queue,
    /// the first is cancelled (search as you type).
    Newest,
    /// the second is skipped (a refresh already running); its caller may
    /// wait for the first with `Handle::done` and read what it left.
    First,
}

/// Asked by `Cx::abort`: stop the running effects with this key.
pub(crate) struct Abort(pub String);

/// The key of effect `name` run for `args`: `scan` for `"/a"` and for `"/b"`
/// then neither cancel, skip nor wait for each other.
pub fn key(name: &str, args: &impl serde::Serialize) -> crate::Result<String> {
    Ok(format!("{name}:{}", crate::json(args)?))
}

/// Async work a call asks for, keyed: it starts once the call commits,
/// and only when handed to `cx.run`. One built and dropped never runs, so
/// the compiler warns:
///
/// ```compile_fail
/// #![deny(unused_must_use)]
/// use aio::prelude::*;
/// #[cell]
/// pub struct S { n: u32 }
/// #[methods]
/// impl S {
///     pub fn go(&mut self) { Effect::new("k", |_: Handle<S>| async {}); }
/// }
/// # fn main() {}
/// ```
/// ```
/// #![deny(unused_must_use)]
/// use aio::prelude::*;
/// #[cell]
/// pub struct S { n: u32 }
/// #[methods]
/// impl S {
///     pub fn go(&mut self, cx: &Cx) { cx.run(Effect::new("k", |_: Handle<S>| async {})); }
/// }
/// # fn main() {}
/// ```
#[must_use = "an effect runs only when handed to `cx.run(..)`"]
pub struct Effect<T: Cell> {
    pub(crate) key: String,
    pub(crate) mode: Mode,
    pub(crate) timeout: Option<Duration>,
    pub(crate) run: Run<T>,
}

impl<T: Cell> Effect<T> {
    /// `Effect::new("fetch", move |h| async move { let r = get(url).await; _ = h.fetched(r); })`
    pub fn new<F>(key: &str, f: impl FnOnce(Handle<T>) -> F + MaybeSend + 'static) -> Self
    where
        F: Future<Output = ()> + MaybeSend + 'static,
    {
        Self { key: key.into(), mode: Mode::Queue, timeout: None, run: Box::new(move |h| Box::pin(f(h))) }
    }

    pub fn mode(mut self, m: Mode) -> Self {
        self.mode = m;
        self
    }

    /// Key it by `args` too (see `key`).
    pub fn per(mut self, args: &impl serde::Serialize) -> crate::Result<Self> {
        self.key = key(&self.key, args)?;
        Ok(self)
    }

    /// Cancel it if it runs longer than `d`.
    pub fn timeout(mut self, d: Duration) -> Self {
        self.timeout = Some(d);
        self
    }
}

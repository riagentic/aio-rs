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

/// How long each await of an async method may take by default, unless it is
/// `#[long]`: one that never settles is cut off (said in the log and a
/// report) and its later steps never run, its caller told, instead of
/// hanging forever. `[perf] call_ceiling` sets another ([`call_ceiling`]).
pub const CALL_CEILING: Duration = Duration::from_secs(30);

/// The app's await ceiling: `[perf] call_ceiling` (ms), else [`CALL_CEILING`];
/// in a page, always [`CALL_CEILING`].
pub fn call_ceiling() -> Duration {
    #[cfg(not(target_arch = "wasm32"))]
    return crate::perf::ceiling();
    #[cfg(target_arch = "wasm32")]
    CALL_CEILING
}

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

/// Asked to stop: what [`blocking`] work checks between chunks.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Debug, Default)]
pub struct Cancel(std::sync::Arc<std::sync::atomic::AtomicBool>);

#[cfg(not(target_arch = "wasm32"))]
impl Cancel {
    /// Was it cancelled ([`cancel_blocking`], or the effect awaiting it
    /// aborted, timed out or dropped)? Stop soon, then.
    pub fn cancelled(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn set(&self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(not(target_arch = "wasm32"))]
type Tokens = std::sync::Mutex<std::collections::HashMap<String, Vec<Cancel>>>;

#[cfg(not(target_arch = "wasm32"))]
fn tokens() -> &'static Tokens {
    static T: std::sync::OnceLock<Tokens> = std::sync::OnceLock::new();
    T.get_or_init(Default::default)
}

/// aio's `blocking(id, fn)`: run `f` on a worker thread (CPU work, a
/// directory walk, a sync-only API) and await its result, so the effect
/// holds no runtime thread. `f` gets a [`Cancel`] to check between chunks:
/// it is set by [`cancel_blocking`]`(key)` (aio's `blocking.cancel(id)`) and
/// when the effect awaiting it ends early (`cx.abort`, `Mode::Newest`, a
/// timeout, the app stopping). Cancelled, its answer is a refusal even if
/// `f` returned; a panic in `f` is one too.
#[cfg(not(target_arch = "wasm32"))]
pub async fn blocking<R: Send + 'static>(key: &str, f: impl FnOnce(&Cancel) -> R + Send + 'static) -> crate::Result<R> {
    /// Unlisted when done; dropped early, it cancels.
    struct Listed(String, Cancel);
    impl Drop for Listed {
        fn drop(&mut self) {
            self.1.set();
            let mut t = tokens().lock().unwrap_or_else(|e| e.into_inner());
            if let Some(v) = t.get_mut(&self.0) {
                v.retain(|c| !std::sync::Arc::ptr_eq(&c.0, &(self.1).0));
                if v.is_empty() {
                    t.remove(&self.0);
                }
            }
        }
    }
    let c = Cancel::default();
    tokens().lock().unwrap_or_else(|e| e.into_inner()).entry(key.into()).or_default().push(c.clone());
    let listed = Listed(key.into(), c.clone());
    let worker = c.clone();
    let out = tokio::task::spawn_blocking(move || f(&worker)).await;
    let cancelled = c.cancelled();
    drop(listed);
    match out {
        _ if cancelled => Err(crate::Rejected::new(format!("blocking `{key}` was cancelled"))),
        Ok(r) => Ok(r),
        Err(e) => Err(crate::Rejected::new(format!("blocking `{key}` failed: {e}"))),
    }
}

/// Cancel the [`blocking`] work running under `key`: how many were asked.
#[cfg(not(target_arch = "wasm32"))]
pub fn cancel_blocking(key: &str) -> usize {
    let t = tokens().lock().unwrap_or_else(|e| e.into_inner());
    t.get(key).map_or(0, |v| {
        v.iter().for_each(Cancel::set);
        v.len()
    })
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

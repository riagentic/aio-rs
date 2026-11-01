//! Server functions (aio's `serverFns`): typed async code that runs only on the
//! server and answers a client with a value no cell stores (a search, a file
//! listing, a payment). One declaration types both ends:
//!
//! ```ignore
//! const CHARGE: ServerFn<(u64,), Receipt> = ServerFn::new("pay.charge");
//! // the server
//! Http::default().server_fn(CHARGE, Access::Role("billing"), |by, (cents,)| charge(by, cents));
//! // a client
//! let receipt = remote.run(CHARGE, (1250,)).await?;
//! ```
//!
//! - **Arguments** are a tuple, sent as a JSON array: aio's positional ones,
//!   so an aio page's `serverFn("pay").charge(1250)` reaches `pay.charge`.
//! - **Access** is said per function and fails closed: with accounts on, the
//!   caller is the session's user and role, read at every call; without, the
//!   shared key proves no one, so only [`Access::Open`] lets a call in.
//! - A function that fails, panics or answers what JSON can't carry fails its
//!   call, in words; the server goes on. A link that closes cancels its
//!   calls still running.
//! - A client waits [`ServerFn::timeout`] (30 s, as aio), then fails; offline,
//!   or when the link drops, it fails at once: a call is never queued.
//! - **Streamed** ([`crate::http::Http::server_fn_streamed`]): the function
//!   sends parts as it goes ([`Chunks`], aio's `emitChunk`: tokens of an LLM's
//!   answer), each to the one caller that asked (`Remote::run_streamed`), in
//!   order and before its answer; a plain `run` gets the answer only.
//! - While a person is at a picker ([`crate::deciding`]), no caller's clock runs; once
//!   it closes, each call still running gets a fresh full window (aio's `sfnh`).
use futures_util::FutureExt;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

/// How long a client waits for an answer unless the function says otherwise.
pub const TIMEOUT: Duration = Duration::from_secs(30);

/// A server function's name and types: `A` the argument tuple, `R` the answer.
pub struct ServerFn<A, R> {
    name: &'static str,
    timeout: Duration,
    types: PhantomData<fn(A) -> R>,
}

impl<A, R> Clone for ServerFn<A, R> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<A, R> Copy for ServerFn<A, R> {}

impl<A, R> ServerFn<A, R> {
    /// `name` is `namespace.function` (what an aio page calls it by).
    pub const fn new(name: &'static str) -> Self {
        assert!(!name.is_empty(), "a server fn needs a name");
        Self { name, timeout: TIMEOUT, types: PhantomData }
    }

    /// How long a client waits for the answer (not zero).
    pub const fn timeout(self, t: Duration) -> Self {
        assert!(!t.is_zero(), "a server fn's timeout must be more than zero");
        Self { timeout: t, ..self }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub(crate) fn wait(&self) -> Duration {
        self.timeout
    }
}

/// A server fn call as its function sees it (aio's `serverRequest()`): who
/// calls, as a cell's `#[client]` rule sees them ([`crate::Caller`]: with
/// accounts on, `By::User` and their role now; a user known by token, the
/// same; otherwise `Caller::default()`, no one in particular), and from where.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Req {
    pub who: crate::Caller,
    /// The client's address: the TCP peer, or a trusted proxy's last hop
    /// ([`crate::gate`]); `None` on the local door.
    pub ip: Option<std::net::IpAddr>,
    /// The headers its link opened with (`user-agent`, `accept-language`…).
    pub headers: axum::http::HeaderMap,
}

/// An app's own access rule: the call and its arguments → may it run.
pub type Rule = Arc<dyn Fn(&Req, &Value) -> bool + Send + Sync>;

/// Who may call a server function.
#[derive(Clone)]
pub enum Access {
    /// Anyone the app lets connect (its key, or any session): aio's no rule.
    Open,
    /// A signed-in user.
    Signed,
    /// A user with this role.
    Role(&'static str),
    /// The app's own rule, on the caller and the arguments (per row); one
    /// that panics is a no.
    Check(Rule),
}

impl Access {
    fn allows(&self, req: &Req, args: &Value) -> bool {
        let signed = req.who.user().is_some();
        match self {
            Self::Open => true,
            Self::Signed => signed,
            Self::Role(r) => signed && req.who.role.as_deref() == Some(*r),
            Self::Check(f) => std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(req, args))).unwrap_or(false),
        }
    }
}

/// Why a call failed: a refusal, with aio's code when it isn't the function's.
pub(crate) type Failed = crate::Rejected;

type Answer = Pin<Box<dyn Future<Output = crate::Result<Value>> + Send>>;
type Run = Arc<dyn Fn(Req, Value, Chunks) -> Answer + Send + Sync>;

/// Where a streamed server fn sends its parts: to its caller, in order,
/// before the answer. A link that is busy makes `send` wait (nothing is
/// dropped); it is `false` when the caller asked for no parts or is gone.
#[derive(Clone, Default)]
pub struct Chunks(Option<(tokio::sync::mpsc::Sender<Value>, Value)>);

impl Chunks {
    /// Parts of call `id`, as frames on `tx`.
    pub(crate) fn to(tx: tokio::sync::mpsc::Sender<Value>, id: Value) -> Self {
        Self(Some((tx, id)))
    }

    /// Send `part` to the caller.
    pub async fn send(&self, part: &impl Serialize) -> bool {
        let (Some((tx, id)), Ok(c)) = (&self.0, serde_json::to_value(part)) else { return false };
        tx.send(json!({ "t": "sfnc", "id": id, "c": c })).await.is_ok()
    }
}

/// The server functions an app serves, by name.
#[derive(Clone, Default)]
pub(crate) struct Fns(HashMap<&'static str, (Access, Run)>);

impl Fns {
    /// The names served.
    pub(crate) fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.0.keys().copied()
    }

    /// `other`'s functions where these have none of the name (the app wins
    /// over a plugin, [`crate::plugin`]).
    pub(crate) fn under(&mut self, other: Fns) {
        for (name, f) in other.0 {
            self.0.entry(name).or_insert(f);
        }
    }

    /// Panics on a name served twice (a programming error, found on the first run).
    pub(crate) fn add<A, R, F, Fut>(&mut self, f: ServerFn<A, R>, access: Access, run: F)
    where
        A: DeserializeOwned + Send + 'static,
        R: Serialize + 'static,
        F: Fn(Req, A) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = crate::Result<R>> + Send + 'static,
    {
        self.add_streamed(f, access, move |by, args, _| run(by, args));
    }

    /// [`Self::add`] for a function that sends parts as it goes.
    pub(crate) fn add_streamed<A, R, F, Fut>(&mut self, f: ServerFn<A, R>, access: Access, run: F)
    where
        A: DeserializeOwned + Send + 'static,
        R: Serialize + 'static,
        F: Fn(Req, A, Chunks) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = crate::Result<R>> + Send + 'static,
    {
        let name = f.name;
        let run = Arc::new(run);
        let run: Run = Arc::new(move |by, args: Value, chunks| {
            // `()` is `null`; an aio page sends `[]` for no arguments.
            let args = serde_json::from_value::<A>(args.clone()).or_else(|e| {
                if args == json!([]) { serde_json::from_value(Value::Null).map_err(|_| e) } else { Err(e) }
            });
            let run = run.clone();
            Box::pin(async move {
                let args = args.map_err(|e| crate::Rejected::new(format!("server fn \"{name}\": bad arguments: {e}")))?;
                let answer = run(by, args, chunks).await?;
                serde_json::to_value(answer).map_err(|e| {
                    crate::Rejected::new(format!(
                        "server fn \"{name}\" ran, but its answer can't be sent as JSON ({e}): return plain data"
                    ))
                })
            })
        });
        assert!(self.0.insert(name, (access, run)).is_none(), "server fn {name:?} is served twice");
    }

    /// Run `name` for `by`, if its access allows; its parts go to `chunks`.
    pub(crate) async fn invoke(&self, name: &str, args: Value, by: Req, chunks: Chunks) -> Result<Value, Failed> {
        let Some((access, run)) = self.0.get(name) else {
            return Err(Failed { why: format!("server fn \"{name}\" is not served by this app"), code: None });
        };
        if !access.allows(&by, &args) {
            let who = by.who.user().map_or("an anonymous client".into(), |u| {
                format!("user={u} role={}", by.who.role.as_deref().unwrap_or("?"))
            });
            eprintln!("[aio] auth: server fn \"{name}\" denied for {who}");
            return Err(Failed { why: format!("server fn \"{name}\": access denied"), code: Some(crate::Code::AccessDenied) });
        }
        let ran = std::panic::AssertUnwindSafe(run(by, args, chunks)).catch_unwind().await;
        match ran {
            Ok(r) => r,
            Err(p) => {
                let why = p
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| p.downcast_ref::<String>().cloned())
                    .unwrap_or_default();
                eprintln!("[aio] server fn \"{name}\" panicked: {why}");
                Err(Failed { why: format!("server fn \"{name}\" failed on the server"), code: None })
            }
        }
    }
}

/// The server functions one link may call, and how to learn who calls:
/// with accounts, the link's session, looked up at every call.
#[derive(Clone)]
pub(crate) struct Served {
    pub(crate) fns: Arc<Fns>,
    pub(crate) account: Option<(Arc<crate::auth::Auth>, String)>,
    /// A user the app knows by token: who calls, fixed for the link.
    pub(crate) known: Option<crate::Caller>,
    /// Where the link comes from: [`Req::ip`] and [`Req::headers`].
    pub(crate) from: (Option<std::net::IpAddr>, axum::http::HeaderMap),
}

impl Served {
    /// Start a call on `tasks`, its parts to `chunks`; it ends with
    /// `reply(outcome)` to send back. Meanwhile, `tell(held)` says a
    /// [`crate::deciding::Deciding`] began or ended.
    pub(crate) fn start(
        &self,
        tasks: &mut tokio::task::JoinSet<Value>,
        name: String,
        args: Value,
        chunks: Chunks,
        tell: impl Fn(bool) + Send + Sync + 'static,
        reply: impl FnOnce(Result<Value, Failed>) -> Value + Send + 'static,
    ) {
        let Self { fns, account, known, from: (ip, headers) } = self.clone();
        tasks.spawn(async move {
            let _in_flight = crate::deciding::InFlight::new(Arc::new(tell));
            let who = match account {
                None => known.unwrap_or_default(),
                Some((a, t)) => match tokio::task::spawn_blocking(move || a.session(&t)).await {
                    Ok(Ok(Some(s))) => crate::Caller { by: crate::By::User(s.user), role: Some(s.role) },
                    _ => crate::Caller::default(),
                },
            };
            let by = Req { who, ip, headers };
            reply(fns.invoke(&name, args, by, chunks).await)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const ADD: ServerFn<(i64, i64), i64> = ServerFn::new("math.add");
    const PING: ServerFn<(), &str> = ServerFn::new("math.ping");
    const MAP: ServerFn<(), BTreeMap<(u8, u8), u8>> = ServerFn::new("math.map");
    const BOOM: ServerFn<(), ()> = ServerFn::new("math.boom");
    const NO: ServerFn<(), ()> = ServerFn::new("math.no");

    fn fns(access: Access) -> Fns {
        let mut f = Fns::default();
        f.add(ADD, access, |_, (a, b)| async move { Ok(a + b) });
        f.add(PING, Access::Open, |_, ()| async { Ok("pong") });
        f.add(MAP, Access::Open, |_, ()| async { Ok(BTreeMap::from([((1, 2), 3)])) });
        f.add(BOOM, Access::Open, |_, ()| async { panic!("kaboom") });
        f.add(NO, Access::Open, |_, ()| async { Err(crate::Rejected::new("card declined")) });
        f
    }

    fn run(f: &Fns, name: &str, args: Value, by: Req) -> Result<Value, Failed> {
        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(f.invoke(
            name,
            args,
            by,
            Chunks::default(),
        ))
    }

    fn user(role: &str) -> Req {
        Req { who: crate::Caller { by: crate::By::User("ann".into()), role: Some(role.into()) }, ..Req::default() }
    }

    #[test]
    fn calls_answer_values_errors_and_unknown_names() {
        let f = fns(Access::Open);
        assert_eq!(run(&f, "math.add", json!([2, 3]), Req::default()), Ok(json!(5)));
        // No arguments: `null` (an aio-rs client) or `[]` (an aio page).
        assert_eq!(run(&f, "math.ping", Value::Null, Req::default()), Ok(json!("pong")));
        assert_eq!(run(&f, "math.ping", json!([]), Req::default()), Ok(json!("pong")));
        assert_eq!(run(&f, "math.no", json!([]), Req::default()).unwrap_err().why, "card declined");
        let bad = run(&f, "math.add", json!(["2", 3]), Req::default()).unwrap_err();
        assert!(bad.why.contains("bad arguments"), "{}", bad.why);
        for unknown in ["math.sub", "math", "constructor", ""] {
            let e = run(&f, unknown, json!([]), Req::default()).unwrap_err();
            assert_eq!((e.why.contains("not served"), e.code), (true, None), "{unknown}");
        }
    }

    #[test]
    fn a_panic_or_an_answer_json_cant_carry_fails_only_its_call() {
        let f = fns(Access::Open);
        let e = run(&f, "math.boom", json!([]), Req::default()).unwrap_err();
        assert!(e.why.contains("failed on the server") && !e.why.contains("kaboom"), "{}", e.why);
        let e = run(&f, "math.map", json!([]), Req::default()).unwrap_err();
        assert!(e.why.contains("ran, but its answer can't be sent"), "{}", e.why);
        assert_eq!(run(&f, "math.add", json!([1, 1]), Req::default()), Ok(json!(2)));
    }

    #[test]
    fn access_fails_closed() {
        let denied = |a: Access, by: Req| run(&fns(a), "math.add", json!([1, 2]), by).err().map(|e| e.code);
        assert_eq!(denied(Access::Open, Req::default()), None);
        assert_eq!(denied(Access::Signed, Req::default()), Some(Some(crate::Code::AccessDenied)));
        assert_eq!(denied(Access::Signed, user("user")), None);
        assert_eq!(denied(Access::Role("admin"), user("user")), Some(Some(crate::Code::AccessDenied)));
        assert_eq!(denied(Access::Role("admin"), user("admin")), None);
        // A role without a user is no one's.
        let ghost = Req { who: crate::Caller { role: Some("admin".into()), ..Default::default() }, ..Req::default() };
        assert_eq!(denied(Access::Role("admin"), ghost), Some(Some(crate::Code::AccessDenied)));
        // Row-level: the rule sees the arguments; a rule that panics is a no.
        let small = Access::Check(Arc::new(|_, args| args[0].as_i64() < Some(10)));
        assert_eq!(denied(small.clone(), Req::default()), None);
        assert_eq!(
            run(&fns(small), "math.add", json!([10, 2]), Req::default()).unwrap_err().code,
            Some(crate::Code::AccessDenied)
        );
        assert_eq!(
            denied(Access::Check(Arc::new(|_, _| panic!("no rule"))), user("admin")),
            Some(Some(crate::Code::AccessDenied))
        );
    }

    #[tokio::test]
    async fn a_streamed_fn_sends_its_parts_in_order_to_a_caller_that_asked() {
        const TOKENS: ServerFn<(u8,), String> = ServerFn::new("llm.say");
        let mut f = Fns::default();
        f.add_streamed(TOKENS, Access::Open, |_, (n,), chunks: Chunks| async move {
            let mut said = vec![];
            for i in 0..n {
                said.push(chunks.send(&format!("t{i}")).await);
            }
            Ok(format!("{n} parts, sent {said:?}"))
        });
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let r = f.invoke("llm.say", json!([2]), Req::default(), Chunks::to(tx, json!("c1"))).await;
        assert_eq!(r, Ok(json!("2 parts, sent [true, true]")));
        for i in 0..2 {
            assert_eq!(rx.recv().await, Some(json!({ "t": "sfnc", "id": "c1", "c": format!("t{i}") })));
        }
        // No parts asked for: the answer only, and `send` says so.
        let r = f.invoke("llm.say", json!([1]), Req::default(), Chunks::default()).await;
        assert_eq!(r, Ok(json!("1 parts, sent [false]")));
        // A caller gone: false, and the function still answers.
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        drop(rx);
        assert_eq!(
            f.invoke("llm.say", json!([1]), Req::default(), Chunks::to(tx, json!(1))).await,
            Ok(json!("1 parts, sent [false]"))
        );
    }

    #[test]
    #[should_panic(expected = "served twice")]
    fn a_name_is_served_once() {
        let mut f = fns(Access::Open);
        f.add(ADD, Access::Open, |_, (a, b)| async move { Ok(a - b) });
    }
}

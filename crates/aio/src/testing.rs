// Test harnesses. Cells (aio's `testCell`, `bootCells`): seed a state, call,
// assert with the state printed on failure, see the effects a call asked for
// before they run, move the schedule clock on, and boot as the app does (its
// list of cells, its saved data), refused where the app's boot refuses. UI
// (aio's `testUI`, with the kit): the real app in the page its window shows,
// drawn by the engine its window runs, driven like a person (`ui`). Both
// run in a sandbox (`paths::sandbox`): the home and the OS data dirs are a
// temp world, and the user's real data is refused.
mod browser;
#[cfg(feature = "kit")]
mod electron;
#[cfg(feature = "server")]
mod sim;
#[cfg(feature = "kit")]
mod ui;
#[cfg(feature = "kit")]
mod video;

pub use browser::TestBrowser;
#[cfg(feature = "server")]
pub use sim::Sim;
#[cfg(feature = "kit")]
pub use electron::{Answered, Asked};
#[cfg(feature = "kit")]
pub use ui::{Info, Key, Modifiers, Mount, MountedComponent, Surface, SurfaceElement, TestUi};

use crate::{By, Cell, CellData, Cells, Door, Error, Handle, Store};
use serde::Serialize;
use serde_json::Value;
use std::time::{Duration, Instant};

/// How long `settle` (cells) and `until` (UI) wait before they fail.
const PATIENCE: Duration = Duration::from_secs(5);

thread_local!(static FAILING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });

/// Run `f`, which must fail: its panic message, for the test to check
/// (`assert!(fails(|| ui.click("Gone")).contains("no element"))`). A failure
/// meant so leaves nothing behind (no `TestUi` trace), where one under
/// `#[should_panic]` keeps its trace as any failure does. `f` passing fails.
#[track_caller]
pub fn fails(f: impl FnOnce()) -> String {
    let was = FAILING.replace(true);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    FAILING.set(was);
    let Err(e) = r else { panic!("expected a failure, but it passed") };
    e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default()
}

/// Is the failure under way one a test asked for (`fails`)?
#[cfg(feature = "kit")]
fn failing_on_purpose() -> bool {
    FAILING.get()
}

/// A state, as a failure prints it: JSON, cut after `max` chars.
pub fn dump(state: &impl Serialize, max: usize) -> String {
    let text = serde_json::to_string(state).unwrap_or_else(|e| format!("(unserializable: {e})"));
    match text.char_indices().nth(max) {
        Some((cut, _)) => format!("{}… ({} chars, cut)", &text[..cut], text.chars().count()),
        None => text,
    }
}

/// Assert `f` holds for the state; a failure says `what` and prints the state.
#[track_caller]
pub fn expect<T: Cell>(door: &impl Door<T>, what: &str, f: impl FnOnce(&T) -> bool) {
    door.read(|s| assert!(f(s), "{what}\n  state: {}", dump(s, 2000)));
}

/// Merge `patch` into `base`: objects key by key, all the way down; anything
/// else (a list, a number) replaces.
fn merge(base: &mut Value, patch: Value) {
    match (base, patch) {
        (Value::Object(b), Value::Object(p)) => {
            p.into_iter().for_each(|(k, v)| merge(b.entry(k).or_insert(Value::Null), v))
        }
        (b, p) => *b = p,
    }
}

/// The first key of `want` that `got` lost, with the keys `got` has there.
/// An empty value is no loss: a field may skip it when saved.
fn lost(want: &Value, got: &Value, at: &str) -> Option<(String, Vec<String>)> {
    let (Value::Object(w), Value::Object(g)) = (want, got) else { return None };
    w.iter().find_map(|(k, v)| {
        let path = if at.is_empty() { k.clone() } else { format!("{at}.{k}") };
        match g.get(k) {
            Some(gv) => lost(v, gv, &path),
            None if empty(v) => None,
            None => Some((path, g.keys().cloned().collect())),
        }
    })
}

fn empty(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        Value::String(s) => s.is_empty(),
        _ => false,
    }
}

/// The cell's default state with `seed` merged in (see `merge`), so a test
/// starts at the state under test. A key the state doesn't have panics,
/// naming the ones it does: a seed that pins nothing must not pass.
#[track_caller]
pub fn seeded<T: CellData>(seed: Value) -> T {
    let mut want = serde_json::to_value(T::default()).expect("the state serializes");
    merge(&mut want, seed);
    let st: T =
        serde_json::from_value(want.clone()).unwrap_or_else(|e| panic!("the seed doesn't fit {}: {e}", name::<T>()));
    let got = serde_json::to_value(&st).expect("the state serializes");
    if let Some((key, has)) = lost(&want, &got, "") {
        panic!("seed `{key}` isn't part of {}'s state; it has: {}", name::<T>(), has.join(", "));
    }
    st
}

/// The code an authenticator app shows for base32 `secret` at `at` (ms since
/// the epoch), for a test to log in with (aio's `totpCode`).
pub fn totp_code(secret: &str, at: i64) -> String {
    let key = crate::auth::totp::unbase32(secret).expect("a base32 TOTP secret");
    crate::auth::totp::code(&key, at.div_euclid(crate::auth::totp::STEP_MS))
}

/// A cell's name for people: its type's.
fn name<T>() -> &'static str {
    std::any::type_name::<T>().rsplit("::").next().unwrap_or("cell")
}

/// An effect a call asked for, not started yet (aio's `t.getEffects()`):
/// its cell, its key (an async method's is the method, with `#[ttl]` its
/// arguments too), how a second one with that key behaves, its timeout
/// (an async method's await: the call ceiling, unless `#[long]`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldEffect {
    pub cell: &'static str,
    pub key: String,
    pub mode: crate::Mode,
    pub timeout: Option<Duration>,
}

/// What `TestCells` needs of each cell, whatever its type.
trait Booted {
    fn held(&self) -> Vec<String>;
    fn held_effects(&self) -> Vec<HeldEffect>;
    fn start_held(&self) -> usize;
    fn running(&self) -> Vec<String>;
    fn fire_due(&self, now: i64);
    fn failed(&self) -> Vec<String>;
    fn last(&self) -> (u64, Vec<String>);
}

impl<T: Cell> Booted for Handle<T> {
    fn held(&self) -> Vec<String> {
        Handle::held(self).into_iter().map(|k| format!("{}.{k}", name::<T>())).collect()
    }

    fn held_effects(&self) -> Vec<HeldEffect> {
        let cell = name::<T>();
        Handle::held_effects(self)
            .into_iter()
            .map(|(key, mode, timeout)| HeldEffect { cell, key, mode, timeout })
            .collect()
    }

    fn start_held(&self) -> usize {
        Handle::start_held(self)
    }

    fn running(&self) -> Vec<String> {
        Handle::running(self).into_iter().map(|k| format!("{}.{k}", name::<T>())).collect()
    }

    fn fire_due(&self, now: i64) {
        self.store(|s| s.fire_due_but(now, |d| self.skips(d)))
            .unwrap_or_else(|e| panic!("{}: schedules: {e}", name::<T>()));
        self.fire_foreign();
    }

    fn failed(&self) -> Vec<String> {
        self.take_failed().into_iter().map(|f| format!("{}.{f}", name::<T>())).collect()
    }

    fn last(&self) -> (u64, Vec<String>) {
        let (at, keys) = self.last_held();
        (at, keys.into_iter().map(|k| format!("{}.{k}", name::<T>())).collect())
    }
}

/// Cells booted in-process for a logic test (no UI): each a real `Handle`
/// on a fresh store, so methods, invariants, listeners and effects run as in
/// the app (cross-cell wiring is the app's own `listen`/effect code, run on
/// these handles). Effects wait until `settle`, so a test sees what a call
/// asked for first; schedules fire only when `advance` moves their clock.
/// Time is virtual (aio's `sleep` under `advance`): the tokio clock of the
/// effects (`sleep`, `timeout`, `interval`, `Effect::timeout`) runs on
/// whenever nothing else can, so a 10 s backoff settles at once, timers firing
/// in their order, and never trails the schedule clock. An effect that waits
/// on real I/O under a timeout belongs in a `TestServer` test.
/// A schedule or effect whose call fails (refused, or a panic) fails the
/// test when it ends, unless the test took it with `failures`: nothing a
/// test didn't look at fails silently.
pub struct TestCells {
    rt: tokio::runtime::Runtime,
    cells: Vec<Box<dyn Booted>>,
    /// Its cells by id, for the schedules one sets on another.
    dir: crate::handle::Dir,
    /// The cells' clock: the wall clock at the start, then moved only by `advance`.
    clock: std::sync::Arc<std::sync::atomic::AtomicI64>,
    /// The tokio instant and the schedule clock at the start: tokio's virtual
    /// time is kept at least `clock - epoch.1` past `epoch.0`.
    epoch: (tokio::time::Instant, i64),
    /// Its own world: no test reaches the user's data (`paths::sandbox`).
    _sandbox: crate::paths::Sandbox,
}

impl Default for TestCells {
    fn default() -> Self {
        Self::new()
    }
}

impl TestCells {
    pub fn new() -> Self {
        let rt =
            tokio::runtime::Builder::new_current_thread().enable_all().start_paused(true).build().expect("runtime");
        let start = rt.block_on(async { tokio::time::Instant::now() });
        let clock = std::sync::Arc::new(std::sync::atomic::AtomicI64::new(crate::store::now_ms()));
        let at = clock.load(std::sync::atomic::Ordering::Relaxed);
        Self {
            rt,
            cells: vec![],
            dir: Default::default(),
            clock,
            epoch: (start, at),
            _sandbox: crate::paths::sandbox().expect("a test sandbox"),
        }
    }

    /// Boot cell `T` at its default state; its calls are `By::Test`.
    pub fn cell<T: Cell>(&mut self) -> Handle<T> {
        self.boot(Store::memory())
    }

    /// Boot cell `T` at its default state with `seed` merged in (`seeded`).
    #[track_caller]
    pub fn seeded<T: Cell>(&mut self, seed: Value) -> Handle<T> {
        self.boot(Store::with(seeded(seed)))
    }

    /// Boot cell `T` from data saved by version `v` of it, as the app's boot
    /// reads its file: data that doesn't fit is refused (fields this build
    /// doesn't declare, a newer version, a failed `migrate`), the rest is
    /// migrated, its `#[transient]` fields reset and its `restore` run.
    pub fn saved<T: Cell>(&mut self, v: u32, data: Value) -> Result<Handle<T>, Error> {
        Store::from_saved(&data.to_string(), v).map(|s| self.boot(s))
    }

    /// Start the app's list of cells (booted here) as the app does: a
    /// duplicate, an unknown dependency or a cycle refuses; otherwise each
    /// cell, in dependency order, fires its missed schedules and starts its
    /// scheduler, on this clock.
    pub fn start(&mut self, cells: Cells) -> Result<Cells, Error> {
        let _in = self.rt.enter();
        cells.start()
    }

    /// Take the failures of the cells' schedules and effects so far
    /// (`Cell.method: why (effect `key`)`), so the test's end doesn't fail on them.
    pub fn failures(&mut self) -> Vec<String> {
        let mut f: Vec<String> = self.cells.iter().flat_map(|c| c.failed()).collect();
        f.extend(crate::perf::watched(false));
        f
    }

    /// A call of `T`'s `method` slower than `ms` fails the test (aio's
    /// `perfBudget`, for this test alone): its own time, as `[perf]` counts
    /// it, of every call on this thread from now on. `failures` takes them.
    #[track_caller]
    pub fn budget<T: Cell>(&mut self, method: &str, ms: u64) {
        assert!(
            T::METHODS.contains(&method),
            "budget: {} has no method `{method}` (its methods: {})",
            name::<T>(),
            T::METHODS.join(", ")
        );
        crate::perf::watch(format!("{}.{method}", T::ID), Duration::from_millis(ms));
    }

    /// The effects the last commit asked for (aio's `expect.effects`): the
    /// newest one's, of any cell here (`Cell.key`, in order); `[]` when it
    /// asked none. [`Self::effects`] is every one still waiting.
    pub fn last_effects(&self) -> Vec<String> {
        self.cells.iter().map(|c| c.last()).max_by_key(|(at, _)| *at).map(|(_, keys)| keys).unwrap_or_default()
    }

    /// Cell `h` back at its default state with `seed` merged in (aio's
    /// `t.init(seed)`; `json!({})`: `t.destroy()`'s fresh state) mid-test:
    /// its `#[on_stop]` runs, its waiting effects are dropped, the state is
    /// replaced (one event, invariants checked), its `#[on_start]` runs.
    #[track_caller]
    pub fn reset<T: Cell>(&mut self, h: &Handle<T>, seed: Value) {
        let st = serde_json::to_value(seeded::<T>(seed)).expect("the state serializes");
        _ = h.live(false);
        h.drop_held();
        h.store(|s| s.load(&By::Test, st)).unwrap_or_else(|e| panic!("reset {}: {e}", name::<T>()));
        _ = h.live(true);
    }

    fn boot<T: Cell>(&mut self, mut store: Store<T>) -> Handle<T> {
        store.set_clock(self.clock.clone());
        let h = Handle::new(store).by(By::Test);
        h.hold();
        _ = h.live(true);
        h.join(&self.dir);
        self.cells.push(Box::new(h.clone()));
        h
    }

    /// The effects calls asked for that haven't started (`Cell.key`), in order.
    pub fn effects(&self) -> Vec<String> {
        self.cells.iter().flat_map(|c| c.held()).collect()
    }

    /// [`Self::effects`] as descriptions: each one's key, mode and timeout.
    pub fn held_effects(&self) -> Vec<HeldEffect> {
        self.cells.iter().flat_map(|c| c.held_effects()).collect()
    }

    /// Start the waiting effects and wait until they, and every effect they
    /// lead to, are done; panics after 5 s naming those still running.
    pub fn settle(&mut self) {
        let end = Instant::now() + PATIENCE;
        loop {
            let started: usize = {
                let _in = self.rt.enter();
                self.cells.iter().map(|c| c.start_held()).sum()
            };
            let running: Vec<String> = self.cells.iter().flat_map(|c| c.running()).collect();
            if started == 0 && running.is_empty() {
                return;
            }
            assert!(Instant::now() < end, "still running after {PATIENCE:?}: {}", running.join(", "));
            // Idle, the paused clock jumps to the next timer: an effect's, or this 1 ms.
            self.rt.block_on(async { tokio::time::sleep(Duration::from_millis(1)).await });
        }
    }

    /// Seeded random calls (aio's `t.fuzz`): `n` picks from `calls` (a method
    /// and how to make its arguments), each on the state the last one left,
    /// then the effects settled. A panic, or a state `holds` says no to,
    /// fails the test naming the seed and the calls up to it; a refusal is
    /// an answer (the invariants refuse what would break them). The same
    /// seed is the same run. Returns the calls made.
    #[track_caller]
    pub fn fuzz<T: Cell>(
        &mut self,
        h: &Handle<T>,
        seed: u64,
        n: usize,
        calls: &[RandomCall],
        holds: impl Fn(&T) -> bool,
    ) -> Vec<String> {
        assert!(!calls.is_empty(), "fuzz: no calls to make");
        self.run_fuzz(h, seed, n, holds, |rng| {
            let (method, args) = &calls[rng.below(calls.len() as u64) as usize];
            (method.to_string(), args(rng))
        })
    }

    /// [`Self::fuzz`] over the cell's own methods (aio's `fuzz({ skip })`):
    /// every one `#[methods]` lists but those in `skip` (boot-only ones,
    /// ones that need a clock or a device), each with arguments made from
    /// its parameter types: numbers, text, `bool`, `Option`, `Vec`, tuples,
    /// `Uuid`. A type it doesn't know gets `null`, so the call refuses (an
    /// answer); give such methods their arguments with `fuzz`.
    #[track_caller]
    pub fn fuzz_methods<T: Cell>(
        &mut self,
        h: &Handle<T>,
        seed: u64,
        n: usize,
        skip: &[&str],
        holds: impl Fn(&T) -> bool,
    ) -> Vec<String> {
        let unknown: Vec<&str> = skip.iter().copied().filter(|m| !T::METHODS.contains(m)).collect();
        assert!(
            unknown.is_empty(),
            "fuzz_methods: {} has no method {} (its methods: {})",
            name::<T>(),
            unknown.join(", "),
            T::METHODS.join(", ")
        );
        assert_eq!(T::SIGNATURES.len(), T::METHODS.len(), "fuzz_methods: {} has no #[methods] signatures", name::<T>());
        let picks: Vec<(&str, Vec<&str>)> = T::METHODS
            .iter()
            .zip(T::SIGNATURES)
            .filter(|(m, _)| !skip.contains(m))
            .map(|(m, sig)| (*m, params(sig)))
            .collect();
        assert!(!picks.is_empty(), "fuzz_methods: every method was skipped ({}): nothing would be called", skip.join(", "));
        self.run_fuzz(h, seed, n, holds, |rng| {
            let (method, types) = &picks[rng.below(picks.len() as u64) as usize];
            let args = match types.len() {
                0 => Value::Null,
                _ => Value::Array(types.iter().map(|t| arbitrary(t, rng)).collect()),
            };
            (method.to_string(), args)
        })
    }

    #[track_caller]
    fn run_fuzz<T: Cell>(
        &mut self,
        h: &Handle<T>,
        seed: u64,
        n: usize,
        holds: impl Fn(&T) -> bool,
        mut pick: impl FnMut(&mut Rng) -> (String, Value),
    ) -> Vec<String> {
        let (mut rng, mut made) = (Rng(seed), vec![]);
        for _ in 0..n {
            let (method, args) = pick(&mut rng);
            made.push(format!("{method}({})", dump(&args, 80)));
            let out = Door::call(h, &method, args);
            let fail = |why: String| -> ! {
                panic!("fuzz seed {seed}: {why}\n  calls: {}\n  state: {}", made.join(", "), dump(&*h.state(), 2000))
            };
            if let Err(e) = &out
                && e.why.starts_with(&format!("{}.{method} panicked", T::ID))
            {
                fail(e.why.clone());
            }
            if !holds(&h.state()) {
                fail(format!("the state no longer holds after {method}"));
            }
        }
        self.settle();
        assert!(
            holds(&h.state()),
            "fuzz seed {seed}: the state no longer holds after its effects: {}",
            made.join(", ")
        );
        // Refusals are answers; a panic in an effect is a bug like any other.
        let panics: Vec<String> = self.failures().into_iter().filter(|f| f.contains(" panicked: ")).collect();
        assert!(panics.is_empty(), "fuzz seed {seed}: {}\n  calls: {}", panics.join("; "), made.join(", "));
        made
    }

    /// Move the schedule clock `ms` on, fire every schedule then due (a
    /// repeat missed more than once fires once, as after a sleep), and settle.
    /// Effects' timers due by then fire too (tokio's clock moved as far).
    pub fn advance(&mut self, ms: i64) {
        let now = self.clock.fetch_add(ms, std::sync::atomic::Ordering::Relaxed) + ms;
        let at = self.epoch.0 + Duration::from_millis(now.saturating_sub(self.epoch.1).max(0) as u64);
        self.rt
            .block_on(async { tokio::time::advance(at.saturating_duration_since(tokio::time::Instant::now())).await });
        self.cells.iter().for_each(|c| c.fire_due(now));
        self.settle();
    }
}

/// Boot as a release build does (`on`) or as a debug one does: a cell whose log
/// no longer replays is quarantined (it runs at its last snapshot and
/// refuses calls), or its boot refused. Its own test binary: the setting is
/// the process's.
pub fn quarantine(on: bool) {
    crate::store::QUARANTINE.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// A cell as a network client `who` reaches it (aio's `t.as(user, …)`): every
/// call is asked of its `#[client]` rule first, on the state it then runs
/// on, as the app's server does; the method's `cx.by()` is `who.by`. The
/// typed `<Cell>Calls` methods work on it like on a `Handle`.
#[cfg(feature = "server")]
pub struct AsClient<T: Cell> {
    h: Handle<T>,
    who: crate::Caller,
}

#[cfg(feature = "server")]
impl<T: Cell> Door<T> for AsClient<T> {
    fn call(&self, method: &str, args: Value) -> crate::Result<Value> {
        self.h.call_by(&self.who, method, args, None).map(Option::unwrap_or_default)
    }

    fn ask(&self, method: &str, args: Value) -> crate::Result<crate::Answer> {
        self.h.ask_by(Some(&self.who), method, args, None, None).map(|(_, a)| a)
    }

    fn read<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.h.read(f)
    }

    fn own(&self, key: &str, res: Option<Box<dyn std::any::Any + Send>>) {
        self.h.own(key, res)
    }
}

#[cfg(feature = "server")]
impl TestCells {
    /// `h` called from the network by `who` (`Caller { by: By::User("ana"),
    /// role: Some("admin") }`); `Caller::default()` is someone with the key only.
    pub fn as_client<T: Cell>(&self, h: &Handle<T>, who: crate::Caller) -> AsClient<T> {
        AsClient { h: h.by(who.by.clone()), who }
    }
}

/// A real app server for a test (aio's `testServer`): the cell served as the
/// app serves it (sync, the app's routes and files) on a free loopback port
/// with a fresh key, in a sandbox. A whole app is its cell and the cells
/// `Http::cell` serves beside it, all on its one `/ws`, key and port
/// ([`Self::cell`], [`Self::client_of`]). Dropped, it stops.
#[cfg(feature = "server")]
pub struct TestServer<T: Cell> {
    /// `127.0.0.1:<port>`.
    pub addr: String,
    /// The key clients show.
    pub token: String,
    pub handle: Handle<T>,
    beside: Vec<std::sync::Arc<dyn crate::link::Beside>>,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
    _sandbox: crate::paths::Sandbox,
}

/// An HTTP answer: its status, its headers (names lowercase) and body.
#[cfg(feature = "server")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetched {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

#[cfg(feature = "server")]
impl Fetched {
    /// The first header called `name` (any case).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

#[cfg(feature = "server")]
impl<T: Cell> TestServer<T> {
    /// Serve `h` with `http` (`Http::default()` for none). Needs a tokio runtime.
    pub async fn start(h: Handle<T>, http: crate::http::Http) -> std::io::Result<Self> {
        Self::start_with(h, crate::sync::Options { http, ..Default::default() }).await
    }

    /// Serve `h` as `opts` say (accounts, limits, …), with a fresh key.
    pub async fn start_with(h: Handle<T>, opts: crate::sync::Options) -> std::io::Result<Self> {
        let sandbox = crate::paths::sandbox()?;
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let (addr, token) = (l.local_addr()?.to_string(), crate::uuid_v4().simple().to_string());
        let opts = crate::sync::Options { token: token.clone(), ..opts };
        let beside = opts.http.beside.clone();
        let task = tokio::spawn(crate::sync::serve_with(h.clone(), l, opts));
        Ok(Self { addr, token, handle: h, beside, task, _sandbox: sandbox })
    }

    /// The handle of served cell `U`: the app's own or one `Http::cell`
    /// serves beside it; panics naming the served cells.
    #[track_caller]
    pub fn cell<U: Cell>(&self) -> Handle<U> {
        let own: &(dyn std::any::Any + Send + Sync) = &self.handle;
        let found = match U::ID == T::ID {
            true => own.downcast_ref::<Handle<U>>(),
            false => self.beside.iter().find(|c| c.id() == U::ID).and_then(|c| c.any().downcast_ref::<Handle<U>>()),
        };
        found.cloned().unwrap_or_else(|| {
            let served: Vec<&str> = std::iter::once(T::ID).chain(self.beside.iter().map(|c| c.id())).collect();
            panic!("TestServer: {} isn't served here (it serves {})", name::<U>(), served.join(", "))
        })
    }

    /// A real synced client of served cell `U`, as `user`: its link shares
    /// the one socket with this test's other clients of the server.
    pub async fn client_of<U: Cell>(&self, user: &str) -> std::io::Result<crate::sync::Remote<U>> {
        crate::sync::Remote::connect(&self.addr, &self.token, user).await
    }

    /// A browser page of this server (aio's `testPageFetch`): its requests
    /// carry the cookies the server set them, as a browser carries them.
    pub fn page(&self) -> TestPage<'_, T> {
        TestPage { server: self, cookies: Default::default() }
    }

    /// A UI's copy of the served cell over the real wire, as `user`, each
    /// message held `latency` each way (aio's `testUI` wire emulation and
    /// `latency`; zero: as fast as the socket). Calls cross the wire
    /// encoding, the server decides who may make them (`#[client]`, roles,
    /// rules), and its refusals are the test's (`TestUi::failures`). Put it
    /// in the app's `HANDLE` with `Mount::before`.
    pub async fn ui_source(&self, user: &str, latency: Duration) -> std::io::Result<crate::Source<T>> {
        let addr = if latency.is_zero() { self.addr.clone() } else { slow_relay(&self.addr, latency).await? };
        let r = crate::sync::Remote::connect(&addr, &self.token, user).await?;
        let said = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
        let to = said.clone();
        r.on_refused(move |m, e| to.lock().unwrap_or_else(|e| e.into_inner()).push(format!("{m}: {e}")));
        WIRED.with_borrow_mut(|w| w.push(said));
        Ok(crate::Source::Remote(r))
    }

    /// `ws://127.0.0.1:<port>`, for a sync client.
    pub fn url(&self) -> String {
        format!("ws://{}", self.addr)
    }

    /// The served state now.
    pub fn state(&self) -> std::sync::Arc<T> {
        self.handle.state()
    }

    /// A real synced client of this server, as `user` (aio multi-client).
    pub async fn client(&self, user: &str) -> std::io::Result<crate::sync::Remote<T>> {
        crate::sync::Remote::connect(&self.addr, &self.token, user).await
    }

    /// Wait until every client (of any one served cell) is online, has no
    /// call waiting and shows the server's state; after 5 s, panics naming
    /// the ones that lag.
    pub async fn converged<U: Cell>(&self, clients: &[&crate::sync::Remote<U>]) {
        let end = Instant::now() + PATIENCE;
        let served = self.cell::<U>();
        loop {
            let want = served.state();
            let lag: Vec<String> = clients
                .iter()
                .enumerate()
                .filter(|(_, c)| !c.online() || c.pending() > 0 || *c.state() != *want)
                .map(|(i, c)| {
                    let link = if c.online() { "online" } else { "offline" };
                    format!("client {i} ({link}, {} pending): {}", c.pending(), dump(&*c.state(), 300))
                })
                .collect();
            if lag.is_empty() {
                return;
            }
            assert!(
                Instant::now() < end,
                "not converged after {PATIENCE:?} on {}: {}",
                dump(&*want, 300),
                lag.join("; ")
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// `method path` as the app's own page sends it: with the key
    /// (`Authorization: Bearer`) and, but for a GET or HEAD, `body` as JSON.
    pub async fn fetch(&self, method: &str, path: &str, body: &str) -> std::io::Result<Fetched> {
        let key = format!("Bearer {}", self.token);
        let mut headers = vec![("Authorization", key.as_str())];
        if !matches!(method, "GET" | "HEAD") {
            headers.push(("Content-Type", "application/json"));
        }
        self.fetch_with(method, path, &headers, body).await
    }

    /// `method path` with exactly these headers (none: a stranger).
    pub async fn fetch_with(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> std::io::Result<Fetched> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut s = tokio::net::TcpStream::connect(&self.addr).await?;
        let extra: String = headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
        let len = body.len();
        let req = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{extra}Content-Length: {len}\r\nConnection: close\r\n\r\n{body}"
        );
        s.write_all(req.as_bytes()).await?;
        let mut out = vec![];
        s.read_to_end(&mut out).await?;
        parse(&out).ok_or_else(|| std::io::Error::other("not an HTTP answer"))
    }
}

/// Every update one synced client received, in order (aio's `patches`),
/// from [`patches`] on: each `patch` delta and each whole `state`.
#[cfg(feature = "server")]
#[derive(Clone, Default)]
pub struct Patches(std::sync::Arc<std::sync::Mutex<Vec<Value>>>);

#[cfg(feature = "server")]
impl Patches {
    /// What came so far.
    pub fn all(&self) -> Vec<Value> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The first update, come or to come, that `pred` takes (aio's
    /// `waitForPatch`); after 5 s, panics naming the ones that came.
    pub async fn until(&self, pred: impl Fn(&Value) -> bool) -> Value {
        let end = Instant::now() + PATIENCE;
        loop {
            if let Some(p) = self.all().into_iter().find(|p| pred(p)) {
                return p;
            }
            if Instant::now() >= end {
                let came: Vec<String> = self.all().iter().map(|p| dump(p, 200)).collect();
                panic!("no such update after {PATIENCE:?}; came: [{}]", came.join(", "));
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

/// Record what `client` receives from now on (aio's `patches`, `onPatch`):
/// a test can see that a field was left out of a delta, or that the whole
/// state was sent again.
#[cfg(feature = "server")]
pub fn patches<T: Cell>(client: &crate::sync::Remote<T>) -> Patches {
    let p = Patches::default();
    let to = p.0.clone();
    client.on_patch(move |m| to.lock().unwrap_or_else(|e| e.into_inner()).push(m.clone()));
    p
}

/// The same call from every client at once (aio's `dispatchAll`): the race
/// nobody can reason about from outside. Each predicted at once and sent;
/// the answers, in client order. Wait with [`TestServer::converged`].
#[cfg(feature = "server")]
pub fn call_all<T: Cell>(clients: &[&crate::sync::Remote<T>], method: &str, args: Value) -> Vec<crate::Result<Value>> {
    clients.iter().map(|c| Door::call(*c, method, args.clone())).collect()
}

/// Several apps in one test (aio's `testApps`): a service and the apps
/// that are its clients, each with its own dirs (data, logs, cache), lock,
/// store file, port and key, all in the test world. Dropped, they stop,
/// the last booted first.
#[cfg(feature = "server")]
pub struct TestApps {
    apps: Vec<(String, Box<dyn std::any::Any + Send>, crate::paths::AppDirs)>,
    world: crate::paths::Sandbox,
}

#[cfg(feature = "server")]
impl Default for TestApps {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "server")]
impl TestApps {
    pub fn new() -> Self {
        Self { apps: vec![], world: crate::paths::sandbox().expect("a test sandbox") }
    }

    /// Boot app `name`: cell `T` opened from its own store and served
    /// (needs a tokio runtime). A name booted already is refused.
    pub async fn boot<T: Cell>(&mut self, name: &str) -> std::io::Result<&TestServer<T>> {
        if self.apps.iter().any(|(n, ..)| n == name) {
            return Err(std::io::Error::other(format!("TestApps: \"{name}\" is booted already")));
        }
        let dirs = crate::paths::AppDirs::at(&self.world.root().join(name), name)?;
        let h = Handle::<T>::open(&dirs.store).map_err(|e| std::io::Error::other(e.to_string()))?;
        let server = TestServer::start(h, crate::http::Http::default()).await?;
        self.apps.push((name.into(), Box::new(server), dirs));
        Ok(self.get(name))
    }

    /// App `name`'s server; panics naming the booted apps, or when it
    /// serves another cell.
    #[track_caller]
    pub fn get<T: Cell>(&self, app: &str) -> &TestServer<T> {
        let (_, server, _) = self.app(app);
        server.downcast_ref().unwrap_or_else(|| panic!("TestApps: \"{app}\" serves another cell than {}", name::<T>()))
    }

    /// App `name`'s dirs: its `logs`, `data`, `store`.
    #[track_caller]
    pub fn dirs(&self, name: &str) -> &crate::paths::AppDirs {
        &self.app(name).2
    }

    /// A client of app `name`, as `user`, as another app's client is.
    pub async fn connect<T: Cell>(&self, name: &str, user: &str) -> std::io::Result<crate::sync::Remote<T>> {
        self.get::<T>(name).client(user).await
    }

    #[track_caller]
    #[allow(clippy::type_complexity)]
    fn app(&self, name: &str) -> &(String, Box<dyn std::any::Any + Send>, crate::paths::AppDirs) {
        self.apps.iter().find(|(n, ..)| n == name).unwrap_or_else(|| {
            let booted: Vec<&str> = self.apps.iter().map(|(n, ..)| n.as_str()).collect();
            panic!("TestApps: no app \"{name}\"; booted: {}", booted.join(", "))
        })
    }
}

#[cfg(feature = "server")]
impl Drop for TestApps {
    fn drop(&mut self) {
        while self.apps.pop().is_some() {}
    }
}

/// A browser page of a [`TestServer`] ([`TestServer::page`]). The cookies
/// are the server's alone: a page reaches nobody else. A new page is a new
/// browser; the same page after a reload keeps them.
#[cfg(feature = "server")]
pub struct TestPage<'a, T: Cell> {
    server: &'a TestServer<T>,
    /// The cookies the server has set and not cleared, by name (as sent).
    pub cookies: std::collections::BTreeMap<String, String>,
}

#[cfg(feature = "server")]
impl<T: Cell> TestPage<'_, T> {
    /// [`TestServer::fetch`] from this page.
    pub async fn fetch(&mut self, method: &str, path: &str, body: &str) -> std::io::Result<Fetched> {
        let key = format!("Bearer {}", self.server.token);
        let mut headers = vec![("Authorization", key.as_str())];
        if !matches!(method, "GET" | "HEAD") {
            headers.push(("Content-Type", "application/json"));
        }
        self.fetch_with(method, path, &headers, body).await
    }

    /// [`TestServer::fetch_with`] from this page: its cookies sent (unless
    /// `headers` say a `Cookie` of their own), the ones answered kept.
    pub async fn fetch_with(
        &mut self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> std::io::Result<Fetched> {
        let jar: String = self.cookies.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; ");
        let mut sent = headers.to_vec();
        if !jar.is_empty() && !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("cookie")) {
            sent.push(("Cookie", &jar));
        }
        let f = self.server.fetch_with(method, path, &sent, body).await?;
        f.headers.iter().filter(|(k, _)| k == "set-cookie").for_each(|(_, line)| keep(&mut self.cookies, line));
        Ok(f)
    }
}

/// A `Set-Cookie` into `jar` as a browser takes it (pure): cleared by no
/// value or no time left to live (`Max-Age` ≤ 0).
#[cfg(feature = "server")]
fn keep(jar: &mut std::collections::BTreeMap<String, String>, line: &str) {
    let mut parts = line.split(';');
    let Some((name, value)) = parts.next().and_then(|p| p.split_once('=')) else { return };
    let name = name.trim();
    let gone = parts
        .filter_map(|a| a.split_once('='))
        .any(|(k, v)| k.trim().eq_ignore_ascii_case("max-age") && v.trim().parse::<i64>().is_ok_and(|s| s <= 0));
    if value.is_empty() || gone {
        jar.remove(name);
    } else {
        jar.insert(name.into(), value.into());
    }
}

/// One HTTP/1.1 answer as read off the socket (pure); a chunked body joined.
#[cfg(feature = "server")]
fn parse(raw: &[u8]) -> Option<Fetched> {
    let line = |b: &[u8]| b.windows(2).position(|w| w == b"\r\n");
    let at = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let mut lines = std::str::from_utf8(&raw[..at]).ok()?.split("\r\n");
    let status = lines.next()?.split(' ').nth(1)?.parse().ok()?;
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let chunked = headers.iter().any(|(k, v)| k == "transfer-encoding" && v.eq_ignore_ascii_case("chunked"));
    let mut rest = &raw[at + 4..];
    if !chunked {
        return Some(Fetched { status, headers, body: String::from_utf8_lossy(rest).into_owned() });
    }
    let mut body = vec![];
    loop {
        let i = line(rest)?;
        let n = usize::from_str_radix(std::str::from_utf8(&rest[..i]).ok()?.split(';').next()?.trim(), 16).ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(rest.get(i + 2..i + 2 + n)?);
        rest = rest.get(i + 4 + n..)?;
    }
    Some(Fetched { status, headers, body: String::from_utf8_lossy(&body).into_owned() })
}

/// A local relay to `to` that holds every chunk `latency` before passing
/// it on, in order, both ways; its address.
#[cfg(feature = "server")]
async fn slow_relay(to: &str, latency: Duration) -> std::io::Result<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    async fn late(mut r: tokio::net::tcp::OwnedReadHalf, mut w: tokio::net::tcp::OwnedWriteHalf, d: Duration) {
        let mut buf = vec![0; 16 << 10];
        while let Ok(n @ 1..) = r.read(&mut buf).await {
            tokio::time::sleep(d).await;
            if w.write_all(&buf[..n]).await.is_err() {
                break;
            }
        }
    }
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let (addr, to) = (l.local_addr()?.to_string(), to.to_string());
    tokio::spawn(async move {
        while let Ok((a, _)) = l.accept().await {
            let Ok(b) = tokio::net::TcpStream::connect(&to).await else { continue };
            let ((ar, aw), (br, bw)) = (a.into_split(), b.into_split());
            tokio::spawn(late(ar, bw, latency));
            tokio::spawn(late(br, aw, latency));
        }
    });
    Ok(addr)
}

#[cfg(feature = "server")]
impl<T: Cell> Drop for TestServer<T> {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// A whole command-line run captured (aio's `testIO`): what stdin gives
/// (`input`; used up, it is closed, so a prompt refuses and never hangs),
/// whether it is a terminal (`tty`, true so prompts run) and whether colour
/// is on (`color`, false so assertions read plain text). The prompts, the
/// styles and the progress drawing write to its stdout and stderr.
/// ```
/// let mut io = aio::testing::TestIo::new("ana\ny\n");
/// let name = io.prompt.ask("name", None).unwrap();
/// assert!(io.prompt.confirm("save?", false).unwrap());
/// aio::cli::Progress::to(io.err(), io.prompt.tty, 2, "saving").done();
/// assert_eq!((name.as_str(), io.stdout().as_str()), ("ana", "name: save? [y/N]: "));
/// assert!(io.stderr().ends_with("2/2 100%\n"));
/// ```
pub struct TestIo {
    pub prompt: crate::cli::Prompt<std::io::Cursor<Vec<u8>>, Captured>,
    pub style: crate::cli::Style,
    err: Captured,
}

/// A stream a [`TestIo`] keeps: everything written to it, shared by its clones.
#[derive(Clone, Default)]
pub struct Captured(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(b);
        Ok(b.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Captured {
    /// What was written, as text.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap_or_else(|e| e.into_inner())).into_owned()
    }
}

impl TestIo {
    /// A terminal without colour, `input` its stdin.
    pub fn new(input: &str) -> Self {
        let prompt = crate::cli::Prompt { input: std::io::Cursor::new(input.into()), out: Captured::default(), tty: true };
        Self { prompt, style: crate::cli::Style { color: false }, err: Captured::default() }
    }

    /// Stdin a terminal or not (a pipe: the prompts refuse).
    pub fn tty(mut self, on: bool) -> Self {
        self.prompt.tty = on;
        self
    }

    /// Colour on or off.
    pub fn color(mut self, on: bool) -> Self {
        self.style.color = on;
        self
    }

    /// Stderr, for [`crate::cli::Progress::to`] and [`crate::cli::Spinner::to`].
    pub fn err(&self) -> Box<dyn std::io::Write + Send> {
        Box::new(self.err.clone())
    }

    /// Everything written to stdout so far.
    pub fn stdout(&self) -> String {
        self.prompt.out.text()
    }

    /// Everything written to stderr so far.
    pub fn stderr(&self) -> String {
        self.err.text()
    }

    /// Run `main` as the binary would with `argv` (aio's `testIO()`): its
    /// exit code, 0 when it returns. `aio::cli::args` reads `argv`, and
    /// `args`'s `--help` or refusal and `fail` end the run with their code
    /// instead of the process, what they print kept in `stdout`/`stderr`.
    pub fn run(&mut self, argv: &[&str], main: impl FnOnce(&mut Self)) -> i32 {
        use crate::cli::{Exit, HELD, Held};
        let argv = argv.iter().map(|a| a.to_string()).collect();
        let held = Held { argv, out: Box::new(self.prompt.out.clone()), err: Box::new(self.err.clone()) };
        HELD.set(Some(held));
        let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| main(self)));
        HELD.set(None);
        match ran {
            Ok(()) => 0,
            Err(p) => match p.downcast::<Exit>() {
                Ok(e) => e.0,
                Err(p) => std::panic::resume_unwind(p),
            },
        }
    }
}

/// The bundle `ax build` made (`.deb`, AppImage, `.zip`, …) started and
/// stopped as `ax smoke` does it (aio's `smoke()`, inside a test): headless,
/// in a throwaway sandbox, its window (or boot, or `--version`) in time, a
/// clean stop, nothing left running, no panic in its log. Its report; a
/// failure panics with what `ax` said. `ax` is `AIO_AX`, else the one on `PATH`.
#[track_caller]
pub fn smoke(bundle: &std::path::Path) -> String {
    let ax = std::env::var_os("AIO_AX").unwrap_or_else(|| "ax".into());
    smoke_with(std::process::Command::new(ax), bundle)
}

#[track_caller]
fn smoke_with(mut ax: std::process::Command, bundle: &std::path::Path) -> String {
    let out = ax
        .arg("smoke")
        .arg(bundle)
        .output()
        .unwrap_or_else(|e| panic!("smoke {}: ax doesn't start ({e}): install it, or name it in AIO_AX", bundle.display()));
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "smoke {} failed ({}):\n{said}", bundle.display(), out.status);
    said
}

/// The parameter types of a `#[methods]` signature (`add(text: String, at:
/// Option<u32>) -> Uuid`: `String`, `Option<u32>`), split at the top-level
/// commas (pure).
fn params(sig: &str) -> Vec<&str> {
    let Some(open) = sig.find('(') else { return vec![] };
    let mut depth = 0;
    let mut close = sig.len();
    for (i, c) in sig[open..].char_indices() {
        match c {
            '(' | '<' | '[' => depth += 1,
            ')' | '>' | ']' => depth -= 1,
            _ => {}
        }
        if depth == 0 {
            close = open + i;
            break;
        }
    }
    split_top(&sig[open + 1..close]).into_iter().filter_map(|p| p.split_once(": ").map(|(_, t)| t)).collect()
}

/// `s` split at the commas outside brackets, each trimmed, empties dropped (pure).
fn split_top(s: &str) -> Vec<&str> {
    let (mut out, mut depth, mut from) = (vec![], 0i32, 0);
    for (i, c) in s.char_indices() {
        match c {
            '(' | '<' | '[' => depth += 1,
            ')' | '>' | ']' => depth -= 1,
            ',' if depth == 0 => {
                out.push(s[from..i].trim());
                from = i + 1;
            }
            _ => {}
        }
    }
    out.push(s[from..].trim());
    out.retain(|p| !p.is_empty());
    out
}

/// A random value of the Rust type written `ty` (as a signature has it),
/// as JSON; `null` for a type it doesn't know (pure in `rng`).
fn arbitrary(ty: &str, rng: &mut Rng) -> Value {
    let ty = ty.trim().trim_start_matches("&mut").trim_start_matches('&').trim();
    let mut pick = |vs: &[Value]| vs[rng.below(vs.len() as u64) as usize].clone();
    if let Some(inner) = ty.strip_prefix('(').and_then(|t| t.strip_suffix(')')) {
        return Value::Array(split_top(inner).into_iter().map(|t| arbitrary(t, rng)).collect());
    }
    if let Some(inner) = ty.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
        let n = rng.below(4);
        return Value::Array((0..n).map(|_| arbitrary(inner, rng)).collect());
    }
    let (path, inner) = match ty.split_once('<') {
        Some((p, rest)) => (p, rest.strip_suffix('>')),
        None => (ty, None),
    };
    use serde_json::json;
    match (path.rsplit("::").next().unwrap_or(path), inner) {
        ("Option", Some(t)) => {
            if rng.below(3) == 0 {
                Value::Null
            } else {
                arbitrary(t, rng)
            }
        }
        ("Box" | "Arc" | "Rc", Some(t)) => arbitrary(t, rng),
        ("Vec" | "VecDeque" | "BTreeSet" | "HashSet", Some(t)) => {
            let n = rng.below(4);
            Value::Array((0..n).map(|_| arbitrary(t, rng)).collect())
        }
        ("bool", None) => json!(rng.below(2) == 1),
        ("u8" | "u16" | "u32" | "u64" | "u128" | "usize", None) => pick(&[json!(0), json!(1), json!(2), json!(7), json!(100), json!(255)]),
        ("i8" | "i16" | "i32" | "i64" | "i128" | "isize", None) => pick(&[json!(0), json!(1), json!(-1), json!(7), json!(-100), json!(100)]),
        ("f32" | "f64", None) => pick(&[json!(0.0), json!(1.5), json!(-2.25), json!(100.0)]),
        ("String" | "str", None) => pick(&[json!(""), json!("a"), json!("Ana"), json!("two words"), json!("é ✓")]),
        ("char", None) => pick(&[json!("a"), json!("é"), json!(" ")]),
        ("Uuid", None) => {
            let (a, b) = (rng.number(), rng.number());
            json!(uuid::Uuid::from_u64_pair(a, b).to_string())
        }
        _ => Value::Null,
    }
}

/// One call `fuzz` may make: the method, and how to make its arguments.
pub type RandomCall<'a> = (&'a str, &'a dyn Fn(&mut Rng) -> Value);

/// A seeded random source for `fuzz` (SplitMix64): the same seed, the same
/// numbers, on every machine.
#[derive(Clone, Debug)]
pub struct Rng(pub u64);

impl Rng {
    pub fn number(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number in `0..n` (`n` > 0).
    pub fn below(&mut self, n: u64) -> u64 {
        self.number() % n
    }
}

impl Drop for TestCells {
    fn drop(&mut self) {
        let mut failed = self.failures();
        failed.extend(crate::perf::watched(true));
        if !failed.is_empty() && !std::thread::panicking() {
            panic!("failed and never looked at (take them with `failures`): {}", failed.join("; "));
        }
    }
}

// A view's refusals (`act!`, `call!`) seen on this thread while a
// `TestUi` lives on it: one nobody took fails the test.
thread_local!(static UNSEEN: std::cell::RefCell<Option<Vec<String>>> = const { std::cell::RefCell::new(None) });

pub(crate) fn unseen(what: String) {
    UNSEEN.with_borrow_mut(|u| u.as_mut().map(|u| u.push(what)));
}

#[cfg(any(feature = "kit", feature = "server"))]
type Said = std::sync::Arc<std::sync::Mutex<Vec<String>>>;

// What the servers of the UI sources made on this thread (`ui_source`) refused.
#[cfg(any(feature = "kit", feature = "server"))]
thread_local!(static WIRED: std::cell::RefCell<Vec<Said>> = const { std::cell::RefCell::new(Vec::new()) });

/// Take the wire's refusals so far.
#[cfg(feature = "kit")]
fn wire_refusals() -> Vec<String> {
    WIRED.with_borrow(|w| {
        w.iter().flat_map(|s| std::mem::take(&mut *s.lock().unwrap_or_else(|e| e.into_inner()))).collect()
    })
}

/// Failure traces kept (the newest; aio kept 20 too).
#[cfg(feature = "kit")]
const TRACES: usize = 20;

/// Cell `T`'s local source at its default state with `seed` merged in
/// (`seeded`), for a `client!` app's `HANDLE`: seeded before the first
/// render with `Mount::before(|| *HANDLE.write() = seeded_source(json!({…})))`.
#[track_caller]
pub fn seeded_source<T: Cell>(seed: Value) -> crate::Source<T> {
    crate::Source::Local(Handle::new(Store::with(seeded(seed))))
}

/// Key `name` (as the DOM names it: `Enter`, `ArrowLeft`, `a`, ` `) with
/// modifier `bits` (Alt 1, Ctrl 2, Meta 4, Shift 8) as the DevTools
/// protocol's `Input.dispatchKeyEvent` takes it: its `key`, `code`, key
/// code, the text it types (none when Alt, Ctrl or Meta is held) and the
/// bits (pure). `None`: no key aio knows by that name.
pub fn cdp_key(name: &str, bits: u32) -> Option<Value> {
    let named = [
        ("Enter", 13, "\r"),
        ("Tab", 9, ""),
        ("Escape", 27, ""),
        ("Backspace", 8, ""),
        ("Delete", 46, ""),
        ("ArrowLeft", 37, ""),
        ("ArrowUp", 38, ""),
        ("ArrowRight", 39, ""),
        ("ArrowDown", 40, ""),
        ("Home", 36, ""),
        ("End", 35, ""),
        ("PageUp", 33, ""),
        ("PageDown", 34, ""),
        (" ", 32, " "),
    ];
    let (code, vk, text) = match named.iter().find(|(n, ..)| *n == name) {
        Some((n, vk, text)) => (if *n == " " { "Space".into() } else { n.to_string() }, *vk, text.to_string()),
        None => {
            let mut cs = name.chars();
            let (Some(c), None) = (cs.next(), cs.next()) else { return None };
            let up = c.to_ascii_uppercase();
            let code = match c {
                'a'..='z' | 'A'..='Z' => format!("Key{up}"),
                '0'..='9' => format!("Digit{c}"),
                _ => String::new(),
            };
            (code, if c.is_ascii_alphanumeric() { up as u32 } else { 0 }, c.to_string())
        }
    };
    // A modified key types nothing (Ctrl+a selects, it doesn't type "a").
    let text = if bits & 7 == 0 { text } else { String::new() };
    Some(serde_json::json!({ "key": name, "code": code, "windowsVirtualKeyCode": vk, "text": text, "modifiers": bits }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[cfg(feature = "server")]
    #[test]
    fn an_answer_is_read_plain_or_chunked() {
        let f = parse(b"HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nX-A: 1\r\n\r\nno such page").unwrap();
        assert_eq!((f.status, f.header("content-type"), f.body.as_str()), (404, Some("text/plain"), "no such page"));
        let c = parse(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nhé\r\n4;x=1\r\nllo!\r\n0\r\n\r\n".as_bytes(),
        );
        assert_eq!(c.unwrap().body, "héllo!", "chunk sizes count bytes");
        assert_eq!(parse(b"garbage"), None);
    }

    #[cfg(feature = "server")]
    #[test]
    fn a_page_keeps_cookies_as_a_browser_does() {
        let mut jar = Default::default();
        keep(&mut jar, "sid=a%20b; Path=/; HttpOnly");
        keep(&mut jar, "theme=dark");
        assert_eq!(jar.get("sid").map(String::as_str), Some("a%20b"), "as sent");
        keep(&mut jar, "sid=c");
        assert_eq!(jar.get("sid").map(String::as_str), Some("c"), "a new value replaces it");
        keep(&mut jar, "sid=c; Max-Age=0");
        keep(&mut jar, "theme=; Path=/");
        keep(&mut jar, "junk");
        assert!(jar.is_empty(), "no time left or no value clears it: {jar:?}");
    }

    #[test]
    fn a_signature_gives_its_parameter_types() {
        assert_eq!(params("add(text: String, at: Option<u32>) -> Uuid"), ["String", "Option<u32>"]);
        assert_eq!(params("async load(m: HashMap<String, u32>, p: (u8, bool)) -> ()"), ["HashMap<String, u32>", "(u8, bool)"]);
        assert!(params("tick() -> ()").is_empty());
    }

    #[test]
    fn arguments_are_made_from_the_types() {
        let mut rng = Rng(3);
        let mut of = |t: &str| (0..40).map(|_| arbitrary(t, &mut rng)).collect::<Vec<_>>();
        assert!(of("u32").iter().all(|v| v.as_u64().is_some()));
        assert!(of("i64").iter().any(|v| v.as_i64().is_some_and(|n| n < 0)), "signed ones go below 0");
        assert!(of("&str").iter().all(Value::is_string) && of("bool").iter().all(Value::is_boolean));
        let opts = of("Option<String>");
        assert!(opts.iter().any(Value::is_null) && opts.iter().any(Value::is_string));
        assert!(of("Vec<u8>").iter().all(|v| v.as_array().is_some_and(|a| a.iter().all(|n| n.as_u64().is_some_and(|n| n < 256)))));
        assert!(of("(u8, bool)").iter().all(|v| v[0].is_u64() && v[1].is_boolean()));
        assert!(of("aio::Uuid").iter().all(|v| v.as_str().is_some_and(|s| s.parse::<uuid::Uuid>().is_ok())));
        assert!(of("Order").iter().all(Value::is_null), "a type it doesn't know: null");
        assert_eq!(arbitrary("u32", &mut Rng(9)), arbitrary("u32", &mut Rng(9)), "the same seed, the same value");
    }

    #[cfg(unix)]
    #[test]
    fn smoke_passes_on_ax_s_ok_and_fails_with_what_it_said() {
        let sh = |code: &str| {
            let mut c = std::process::Command::new("sh");
            c.args(["-c", &format!("echo \"$0 $1\"; exit {code}")]);
            c
        };
        let b = std::path::Path::new("notes-0.1.0-x86_64.AppImage");
        assert_eq!(smoke_with(sh("0"), b), "smoke notes-0.1.0-x86_64.AppImage\n");
        let e = fails(|| _ = smoke_with(sh("1"), b));
        assert!(e.contains("smoke notes-0.1.0-x86_64.AppImage failed") && e.ends_with("smoke notes-0.1.0-x86_64.AppImage\n"), "{e}");
    }

    #[test]
    fn a_seed_merges_objects_and_replaces_the_rest() {
        let mut v = json!({"cfg": {"a": 1, "b": {"c": 2, "d": 3}}, "list": [1, 2], "n": 0});
        merge(&mut v, json!({"cfg": {"b": {"c": 9}}, "list": [7], "n": 5}));
        assert_eq!(v, json!({"cfg": {"a": 1, "b": {"c": 9, "d": 3}}, "list": [7], "n": 5}));
    }

    #[test]
    fn a_key_the_state_drops_is_named_with_the_ones_it_has() {
        let got = json!({"cfg": {"a": 1, "b": 2}, "tags": []});
        let want = json!({"cfg": {"a": 1, "b": 2, "typo": 3}, "tags": []});
        assert_eq!(lost(&want, &got, ""), Some(("cfg.typo".into(), vec!["a".into(), "b".into()])));
        assert_eq!(lost(&json!({"skipped": []}), &json!({}), ""), None, "an empty value a field skips is no loss");
    }

    #[test]
    fn a_long_state_is_cut_on_a_char_boundary() {
        assert_eq!(dump(&json!({"a": 1}), 100), r#"{"a":1}"#);
        assert_eq!(dump(&"ééééé", 3), "\"éé… (7 chars, cut)");
    }

    #[test]
    fn a_key_is_sent_as_the_keyboard_sends_it() {
        let k = |name, bits| cdp_key(name, bits).unwrap();
        assert_eq!(k("Enter", 0), json!({ "key": "Enter", "code": "Enter", "windowsVirtualKeyCode": 13, "text": "\r", "modifiers": 0 }));
        assert_eq!(k("a", 8)["text"], "a", "Shift still types");
        assert_eq!((k("a", 2)["text"].clone(), k("a", 2)["code"].clone()), (json!(""), json!("KeyA")), "Ctrl+a types nothing");
        assert_eq!((k(" ", 0)["code"].clone(), k("7", 0)["code"].clone()), (json!("Space"), json!("Digit7")));
        assert_eq!(k("é", 0)["text"], "é");
        assert_eq!(cdp_key("Entr", 0), None);
    }

    #[test]
    fn a_run_is_captured_to_its_exit() {
        use crate::cli::{Flag, Spec, args, fail};
        let spec = || Spec::new("hi").about("Says hi.").flag(Flag::switch("loud"));
        let main = |io: &mut TestIo| {
            let a = args(&spec());
            if !a.on("loud") {
                fail("too quiet", 3, false);
            }
            _ = std::io::Write::write_all(&mut io.prompt.out, b"HI\n");
        };
        let mut io = TestIo::new("");
        assert_eq!(io.run(&["--loud"], main), 0);
        assert_eq!(io.stdout(), "HI\n");
        let mut io = TestIo::new("");
        assert_eq!(io.run(&[], main), 3);
        assert!(io.stderr().contains("too quiet"), "{}", io.stderr());
        let mut io = TestIo::new("");
        assert_eq!(io.run(&["--help"], main), crate::cli::OK);
        assert!(io.stdout().contains("Says hi."), "{}", io.stdout());
        let mut io = TestIo::new("");
        assert_eq!(io.run(&["--nope"], main), crate::cli::USAGE);
        assert!(!io.stderr().is_empty());
        let other = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| TestIo::new("").run(&[], |_| panic!("own"))));
        assert_eq!(other.unwrap_err().downcast_ref::<&str>(), Some(&"own"), "another panic goes on");
        assert!(crate::cli::HELD.with_borrow(Option::is_none), "the hold ends with the run");
    }
}

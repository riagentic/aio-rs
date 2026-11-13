//! aio-rs core. A cell is plain data; a method changes a copy that replaces
//! the state only when it returns `Ok` and the invariants hold, so every call
//! is atomic. Each committed call is an event in the log, with what it read
//! from the outside world (ids, time, randomness), so replay is exact.
//!
//! In a browser (`wasm32`) only the cells' core is built, with [`web`]: the
//! page's own cells and its synced copies, kept in the browser's storage.
// What only the native store and server use is left unused in a page.
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

// The cell macros name `::aio`: built-in cells (`update::Updates`) use them too.
extern crate self as aio;

/// Items only a native build has: the store, files, processes, the server.
macro_rules! native {
    ($($i:item)*) => { $(#[cfg(not(target_arch = "wasm32"))] $i)* };
}

native! {
    pub mod auth;
    pub mod backup;
    pub mod blobs;
    pub mod boot;
    pub mod budgets;
    pub mod capabilities;
    pub mod cassette;
    mod cells;
    pub mod cli;
    pub mod config;
    #[cfg(not(target_arch = "wasm32"))]
    pub mod connect;
    pub mod cost;
    #[cfg(feature = "tls")]
    pub mod cpace;
    pub mod db;
    pub mod deciding;
    #[cfg(feature = "tls")]
    pub mod discovery;
    pub mod door;
    #[cfg(feature = "server")]
    mod doorstep;
    pub mod feedback;
    #[cfg(feature = "server")]
    pub mod fns;
    #[cfg(feature = "server")]
    pub mod gate;
    mod handle;
    pub mod health;
    #[cfg(feature = "server")]
    pub mod http;
    pub mod ipc;
    pub mod lifecycle;
    mod link;
    pub mod log;
    #[cfg(feature = "tls")]
    pub mod pairing;
    #[cfg(feature = "kit")]
    pub mod page;
    pub mod paths;
    #[cfg(feature = "server")]
    pub mod peer;
    pub mod perf;
    pub mod plugin;
    #[cfg(feature = "server")]
    mod poll;
    pub mod process;
    pub mod report;
    pub mod security;
    mod source;
    mod store;
    #[cfg(feature = "server")]
    pub mod sync;
    #[cfg(feature = "tls")]
    pub mod tls;
    #[cfg(feature = "testing")]
    pub mod testing;
    pub mod update;
    pub mod video;
    pub mod vitals;
    pub use cells::{CellHealth, Cells};
    pub use handle::{Drained, Handle, Moment, RETRY_AFTER_MS, TIMELINE_STEPS, TimeAsk, Timeline};
    pub use source::Source;
    pub use store::{Due, Error, Event, Position, REDACTED, Recovered, TX, Steps, Store, Travel, PERSIST_DEBOUNCE, apply_event, on_recovered, shape_of};
}
/// In a page: the login doors of the server that served it.
#[cfg(target_arch = "wasm32")]
pub mod auth {
    pub mod client;
}
mod answer;
pub mod big;
pub use big::Big;
pub mod colors;
pub mod columns;
pub mod compat;
#[cfg(any(not(target_arch = "wasm32"), feature = "web"))]
pub mod held;
mod hidden;
#[cfg(any(feature = "server", feature = "kit", all(feature = "web", target_arch = "wasm32")))]
mod mux;
pub use hidden::Hidden;
pub mod cron;
mod cx;
mod derived;
pub mod effect;
pub mod filter;
pub mod fmt;
#[cfg(doctest)]
mod guide;
pub mod head;
#[cfg(feature = "kit")]
pub mod kit;
#[cfg(target_arch = "wasm32")]
#[path = "web/log.rs"]
pub mod log;
mod log_macros;
pub mod machine;
pub mod merge;
pub mod patch;
pub mod route;
pub mod sha256;
// Page text made smaller as the app builds (`shrunk!`): tooling, not API.
#[doc(hidden)]
pub mod shrink;
mod status;
pub mod tell;
pub mod ui;
pub mod web;
pub use status::{Conn, Status};
#[cfg(target_arch = "wasm32")]
pub use web::{Handle, Source, sync};

/// The app's monogram: its letter in its hue, as pixels or SVG.
pub use aio_config::icon;
#[doc(hidden)]
pub use aio_config_macro::config as __config_lit;
/// The app's one hue as a `u16`, from its `aio.toml` as it builds
/// (`Config::hue`): what `kit::theme_css` takes, so the page is the colour
/// of its icon and splash.
pub use aio_config_macro::hue;
pub use aio_macros::{cell, data, methods};
pub use answer::Answer;
#[doc(hidden)]
pub use answer::{Reply, Step, call_key, finish, ttl_key};
pub use cx::{Backoff, By, Cx, Inputs, On, Plan};
pub use derived::{Cache, Derived, Reads, Ver, Watching};
pub use effect::{Effect, Mode};
/// Persistent collections for cell state: a call copies the state, and with
/// `imbl::OrdMap` / `Vector` that copy is O(1), sharing what didn't change.
/// Ordered ones only (`OrdMap`, `OrdSet`, `Vector`): replay must be exact.
#[cfg(feature = "imbl")]
pub use imbl;
pub use uuid::Uuid;

/// aio-rs's version (the `aio-rs` crate's, as Cargo.lock has it): what an app's About names.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A build made to work on the app: a debug build, or `ax dev`'s and
/// `ax test`'s (aio's `dev` feature, without debug assertions). It says
/// more and refuses sooner than the build people get.
pub(crate) const DEV_BUILD: bool = cfg!(any(debug_assertions, feature = "dev"));

/// What aio is told built this side: the hello's `ver` and meta.json's `aio`.
/// aio only shows them (as `aio <ver>`, at most 64 chars), never compares
/// them, so they name the product: aio shows `aio aio-rs 0.1.0`.
pub const BUILT_BY: &str = concat!("aio-rs ", env!("CARGO_PKG_VERSION"));
const _: () = assert!(BUILT_BY.len() <= 64, "aio drops a longer `ver`");

/// The boot script of a sandboxed island (`Island::sandboxed`): an iframe with
/// `sandbox="allow-scripts"` (an origin of its own: no page DOM, no wire,
/// no storage, no cookies) under the page's policy, which it inherits. Its
/// one inline script (allowed by its hash) waits for the page's `mount`
/// with the island's script, runs it as a `blob:` script of its own, and
/// talks to the page by `postMessage` only: `mounted`, `event`, `error` out;
/// `update` and `unmount` in.
#[doc(hidden)]
pub const ISLAND_SANDBOX: &str = "let h = null, mount = null;
const up = (m) => parent.postMessage(m, '*');
const why = (e) => String(e && e.stack || e);
const emit = (v) => up({ t: 'event', v });
const go = (p) => { const r = mount(document.body, p, emit); h = typeof r === 'function' ? { unmount: r } : r || {}; };
addEventListener('message', (e) => {
  if (e.source !== parent || !e.data) return;
  const m = e.data;
  if (m.t === 'mount' && !mount) {
    const code = 'try { window.__aioMount = ' + m.src + '; } catch (e) { window.__aioError = String(e && e.stack || e); }';
    const u = URL.createObjectURL(new Blob([code], { type: 'text/javascript' }));
    const s = document.createElement('script');
    s.src = u;
    s.onerror = () => up({ t: 'error', why: 'the script failed to load: the page policy refused it' });
    s.onload = () => {
      URL.revokeObjectURL(u);
      mount = window.__aioMount;
      if (typeof mount !== 'function') return up({ t: 'error', why: 'the script failed to load: ' + (window.__aioError || 'its script returned no mount(el, props, emit) function') });
      try { go(m.props); up({ t: 'mounted' }); } catch (e) { up({ t: 'error', why: 'its script loaded but mount() threw: ' + why(e) }); }
    };
    document.head.appendChild(s);
  } else if (m.t === 'update' && h) {
    try { if (h.update) h.update(m.props); else { if (h.unmount) h.unmount(); go(m.props); } } catch (e) { up({ t: 'error', why: 'update() threw: ' + why(e) }); }
  } else if (m.t === 'unmount' && h && h.unmount) {
    try { h.unmount(); } catch (e) {}
  }
});";

/// Bytes from the OS's random source (in a page, `crypto.getRandomValues`).
/// Not for methods: there `cx.rand()` and `cx.id()` replay.
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    #[cfg(not(target_arch = "wasm32"))]
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut b).expect("the OS random source");
    #[cfg(target_arch = "wasm32")]
    web_sys::window()
        .and_then(|w| w.crypto().ok())
        .and_then(|c| c.get_random_values_with_u8_array(&mut b).ok())
        .expect("the page's random source");
    b
}

/// A random UUID (v4).
pub fn uuid_v4() -> Uuid {
    uuid::Builder::from_random_bytes(random_bytes()).into_uuid()
}

/// A UUIDv7 now: ordered by time, and within the process strictly
/// increasing, also within one millisecond.
pub fn uuid_v7() -> Uuid {
    static LAST: std::sync::Mutex<u128> = std::sync::Mutex::new(0);
    #[cfg(not(target_arch = "wasm32"))]
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    #[cfg(target_arch = "wasm32")]
    let ms = js_sys::Date::now() as u64;
    let fresh = uuid::Builder::from_unix_timestamp_millis(ms, &random_bytes()).into_uuid().as_u128();
    let mut last = LAST.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    *last = if fresh > *last { fresh } else { *last + 1 };
    Uuid::from_u128(*last)
}

/// `v` sorted by `cmp`, stably, through the one sort every caller shares (a
/// sort of `u32` positions): each element type's own sort is ~3 KB of a web
/// build's code.
pub(crate) fn sort_by<T>(v: &mut Vec<T>, cmp: impl Fn(&T, &T) -> std::cmp::Ordering) {
    if v.len() < 2 {
        return;
    }
    let mut order: Vec<u32> = (0..v.len() as u32).collect();
    let by: &mut dyn FnMut(&u32, &u32) -> std::cmp::Ordering = &mut |a, b| cmp(&v[*a as usize], &v[*b as usize]);
    order.sort_by(by);
    let mut old: Vec<Option<T>> = std::mem::take(v).into_iter().map(Some).collect();
    v.extend(order.iter().filter_map(|&i| old[i as usize].take()));
}

/// What this build can't do: aio's `feature` does it, and the build left it out.
#[cfg(all(not(target_arch = "wasm32"), not(feature = "tls")))]
pub(crate) fn left_out(feature: &str, what: &str) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        format!("{what} needs aio-rs's `{feature}` feature, which this build left out"),
    )
}

#[doc(hidden)]
pub use serde;
#[doc(hidden)]
pub use serde_json;
#[doc(hidden)]
pub use tokio as __tokio;

use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

/// Time for effects (aio's `sleep`, and `race` with a timeout): tokio's, so
/// `#[tokio::test(start_paused = true)]` runs it on a virtual clock that
/// jumps ahead whenever everything waits.
#[cfg(not(target_arch = "wasm32"))]
pub mod time {
    pub use tokio::time::{Instant, sleep, sleep_until, timeout};
}

pub mod prelude {
    #[cfg(not(target_arch = "wasm32"))]
    pub use crate::Store;
    pub use crate::{
        Answer, By, Caller, Cell, CellData, Code, Cx, Door, Effect, Handle, Hidden, Mode, Rejected, Result, Uuid, act,
        call, cell, data, ensure, methods, reject,
    };
}

/// A typed refusal: the call changed nothing. `why` names the cause and
/// the fix, for people; `code` is aio's kind of refusal, for code (match on
/// it, never on the words).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejected {
    pub why: String,
    /// `None` for a method's own refusal.
    pub code: Option<Code>,
}

/// How a `#[cell(transaction)]` cell's async methods commit (aio's
/// `transaction: { serialize, conflict }`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transaction {
    /// One at a time: a call waits for the one before it to end.
    pub serialize: bool,
    /// Commit over a conflict, said, instead of refusing with `TX_CONFLICT`.
    pub warn: bool,
}

/// The kinds of refusal aio makes, on either side of the wire (aio
/// `errorCode`). Stable: a code keeps its meaning and its wire name
/// (`ACCESS_DENIED`) across versions; new ones may be added, and one this
/// build doesn't know (a newer server's) is kept as [`Code::Other`]. Its
/// prefix says the layer, as aio's: `REDUCE_` a method, `INIT_` its start
/// hook, `DESTROY_` its stop hook, `EFFECT_` an async call's effect,
/// `HOOK_` a reaction, `QUEUE_`/`DISPATCH_` the calls' way in, `PERSIST_`
/// the store.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Code {
    /// The caller may not: sign in, or ask for the role.
    AccessDenied,
    /// Allowed, but there is no such method.
    ActionRefused,
    /// The link to the app dropped or never came up: the call may have run.
    ConnectionLost,
    /// A transaction's write met another call's (`#[cell(transaction)]`):
    /// nothing it wrote committed; call it again.
    TxConflict,
    /// The method panicked: a bug, its call changed nothing.
    ReduceError,
    /// An `#[on_start]` hook refused or panicked.
    InitError,
    /// The offline queue is full (500 calls wait for the server).
    QueueOverflow,
    /// The app is closing, the cell disabled or paused by time travel.
    DispatchClosed,
    /// The call's await was aborted, superseded or cut off before it answered.
    DispatchAborted,
    /// Refused in a call storm (`[perf.storm]`).
    DispatchLoop,
    /// The store could not write the call.
    PersistError,
    /// An async call's effect panicked before it answered.
    EffectError,
    /// An async call's effect ran past its timeout (the call ceiling, or
    /// `Effect::timeout`) and was cut off.
    EffectTimeout,
    /// A reaction to another cell's call failed (aio's hooks:
    /// `onAction`, `onEffect`); said on the bus, never a caller's answer.
    HookError,
    /// An `#[on_stop]` hook refused or panicked.
    DestroyError,
    /// A code this build doesn't know, verbatim (a newer server's).
    Other(String),
}

const CODES: [(Code, &str); 15] = [
    (Code::AccessDenied, "ACCESS_DENIED"),
    (Code::ActionRefused, "ACTION_REFUSED"),
    (Code::ConnectionLost, "CONNECTION_LOST"),
    (Code::TxConflict, "TX_CONFLICT"),
    (Code::ReduceError, "REDUCE_ERROR"),
    (Code::InitError, "INIT_ERROR"),
    (Code::QueueOverflow, "QUEUE_OVERFLOW"),
    (Code::DispatchClosed, "DISPATCH_CLOSED"),
    (Code::DispatchAborted, "DISPATCH_ABORTED"),
    (Code::DispatchLoop, "DISPATCH_LOOP"),
    (Code::PersistError, "PERSIST_ERROR"),
    (Code::EffectError, "EFFECT_ERROR"),
    (Code::EffectTimeout, "EFFECT_TIMEOUT"),
    (Code::HookError, "HOOK_ERROR"),
    (Code::DestroyError, "DESTROY_ERROR"),
];

impl Code {
    /// Its wire name (`ACCESS_DENIED`).
    pub fn as_str(&self) -> &str {
        match self {
            Self::Other(s) => s,
            c => CODES.iter().find(|(k, _)| k == c).map_or("", |(_, n)| n),
        }
    }

    /// The code a wire message carries in `code` (`None`: none); one this
    /// build doesn't know is kept as [`Code::Other`] (aio's `errorCode`).
    pub fn of(v: &Value) -> Option<Self> {
        let s = v.as_str().filter(|s| !s.is_empty())?;
        Some(CODES.iter().find(|(_, n)| *n == s).map_or_else(|| Self::Other(s.into()), |(c, _)| c.clone()))
    }
}

impl Serialize for Code {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for Code {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = <String as serde::Deserialize>::deserialize(d)?;
        Self::of(&Value::String(s)).ok_or_else(|| serde::de::Error::custom("an empty code"))
    }
}

/// One line for a log, a CI gate or an agent (aio's `errorSummary`): the
/// code, when there is one, and the first sentence of the words, at most
/// 160 characters. Branch on [`Rejected::code`], never on this.
pub fn error_summary(e: &Rejected) -> String {
    let first = e.why.lines().next().unwrap_or_default();
    let first = first.find(". ").map_or(first, |i| &first[..=i]);
    let line = match &e.code {
        Some(c) => format!("{}: {first}", c.as_str()),
        None => first.to_string(),
    };
    match line.char_indices().nth(159) {
        Some((i, _)) => format!("{}…", &line[..i]),
        None => line,
    }
}

impl Rejected {
    /// A refusal of the app's own, with no code.
    pub fn new(why: impl Into<String>) -> Self {
        Self { why: why.into(), code: None }
    }

    /// One of aio's kinds.
    pub fn coded(code: Code, why: impl Into<String>) -> Self {
        Self { why: why.into(), code: Some(code) }
    }

    pub fn unknown(method: &str) -> Self {
        Self::coded(Code::ActionRefused, format!("unknown method `{method}`"))
    }

    /// A network call its `#[client]` rule refuses.
    pub fn denied(method: &str, why: &str) -> Self {
        Self::coded(Code::AccessDenied, format!("access denied: `{method}` {why}"))
    }

    /// The link dropped or never came up.
    pub fn lost(why: impl Into<String>) -> Self {
        Self::coded(Code::ConnectionLost, why)
    }

    /// Its code (aio's `errorCode`): `None` for the method's own refusal.
    pub fn code(&self) -> Option<Code> {
        self.code.clone()
    }
}

impl From<String> for Rejected {
    fn from(why: String) -> Self {
        Self::new(why)
    }
}

impl From<&str> for Rejected {
    fn from(why: &str) -> Self {
        Self::new(why)
    }
}

impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.why)
    }
}

impl std::error::Error for Rejected {}

pub type Result<T = (), E = Rejected> = std::result::Result<T, E>;

/// Refuse unless `cond` holds.
#[macro_export]
macro_rules! ensure {
    ($cond:expr, $($msg:tt)+) => {
        if !$cond {
            return Err($crate::Rejected::new(format!($($msg)+)));
        }
    };
}

/// Refuse now.
#[macro_export]
macro_rules! reject {
    ($($msg:tt)+) => {
        return Err($crate::Rejected::new(format!($($msg)+)))
    };
}

/// What `#[cell]` generates: identity, flags and how old data upgrades.
pub trait CellData: Clone + Default + PartialEq + Serialize + DeserializeOwned + Send + Sync + 'static {
    const ID: &'static str;
    const VERSION: u32;
    const PERSIST: bool;
    const SHARED: bool;
    /// `#[cell(diagnostics = false)]` (aio's `diagnostics: false`): its calls
    /// stay out of the live timeline, reports and the debug log. Its state
    /// and events are kept as ever.
    const DIAGNOSTICS: bool = true;
    /// `#[cell(persist = false)]` (aio's `persist: "none"`): the cell is
    /// never saved, also when opened on a file: it starts from its defaults
    /// on every launch. Without it, a cell opened on a store is saved there.
    const SAVED: bool = true;
    /// `#[cell(coalesce)]` (aio's debounced persistence for the whole cell):
    /// every plain method is `#[coalesce]` (see `Cell::COALESCED`).
    const COALESCE: bool = false;
    /// `#[cell(redact)]` (aio's `redactActions: ["vault:*"]`): every method
    /// is `#[redact]` (see `Cell::REDACTED`).
    const REDACT: bool = false;
    /// Turn state saved by version `from` into this one. An `Err` refuses to
    /// boot (after the backup), never resets the data.
    fn migrate(old: Value, from: u32) -> std::result::Result<Self, String> {
        let _ = from;
        serde_json::from_value(old).map_err(|e| e.to_string())
    }
    /// `#[cell(visible = f)]` is set: [`Self::project`] runs the app's code.
    const SCREENED: bool = false;
    /// What `viewer` (who, with their role now) may see of the state, as
    /// sent to a synced client: without `#[server]` fields, filtered by
    /// `#[cell(visible = f)]`.
    fn project(&self, viewer: &Caller) -> Value {
        let _ = viewer;
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
    /// `#[transient]` fields and `#[cell(persist(exclude = ["feeds.*.busy"]))]`
    /// paths (aio's `persist: { exclude }`): never saved, back to their
    /// defaults on every boot ([`filter`]). A nested one's field needs a default.
    const TRANSIENT: &'static [&'static str] = &[];
    /// `#[cell(persist(include = ["name", "profile.email"]))]` (aio's
    /// `persist: { include }`): only these paths are saved; empty: all of it.
    const PERSIST_INCLUDE: &'static [&'static str] = &[];
    /// `#[server]` fields: saved, never sent to a client, and left out of
    /// problem reports and the debug log (a report may be delivered). A
    /// shared cell's credential (a password, an API key…) must be one:
    ///
    /// ```compile_fail
    /// #[aio::cell(shared)]
    /// pub struct Keys { pub api_key: String }
    /// ```
    /// ```
    /// #[aio::cell(shared)]
    /// pub struct Keys { #[server] pub api_key: aio::Hidden<String>, pub api_key_id: u32 }
    /// ```
    ///
    /// On a shared cell it is an [`Hidden`]: a copy never has it, and
    /// reading it there fails loudly instead of reading a default.
    ///
    /// A name that may be a secret (`seed`, `token`, `master_key`…) is
    /// warned about (aio's warning tier); `#[public]` says a field named
    /// like a secret is meant to be sent (aio's `publicFields`):
    ///
    /// ```compile_fail
    /// #![deny(deprecated)]
    /// #[aio::cell(shared)]
    /// pub struct Game { pub seed: u64 }
    /// ```
    /// ```
    /// #![deny(deprecated)]
    /// #[aio::cell(shared)]
    /// pub struct Game { #[public] pub seed: u64, #[public] pub api_key: String }
    /// ```
    ///
    /// ```compile_fail
    /// #[aio::cell(shared)]
    /// pub struct Keys { #[server] pub api_key: String }
    /// ```
    const SERVER: &'static [&'static str] = &[];
    /// On a synced copy, mark the `#[server]` fields absent (`#[cell]` writes it).
    #[doc(hidden)]
    fn conceal(&mut self) {}
    /// `#[table("name")]` fields (`#[table("name", key = "col")]` for a map
    /// keyed by that column): the rows of the `db:` table `name`, in an aio
    /// file ([`crate::Store::open_aio`]) instead of the `aio_kv` slice, in
    /// aio-rs's own beside its history (a table of aio-rs's own refuses the boot), as
    /// `(field, table, key, columns)`. With `columns = "id INTEGER PRIMARY
    /// KEY, name TEXT NOT NULL"` the boot makes the table, or brings a
    /// stored one to that shape (aio's declared schema); `columns = CONTACTS`
    /// (a `&[aio::columns::Col]`) declares it typed.
    #[allow(clippy::type_complexity)]
    const TABLES: &'static [(&'static str, &'static str, Option<&'static str>, Option<columns::Columns>)] = &[];
    /// `#[aio_cell]` fields (`#[aio_cell] accounts: Accounts`, `Accounts` a
    /// `#[cell(persist, id = "accounts", version = 3)]`): in an aio file, each
    /// is that aio cell's whole slice, read and written by its type's id,
    /// version, `migrate`, `#[transient]` and `#[table]` fields, so one cell
    /// owns many of aio's (docs/migration/data.md). Its other fields are aio-rs's only.
    #[cfg(not(target_arch = "wasm32"))]
    const AIO_CELLS: &'static [compat::data::Slice] = &[];
    /// The `#[aio_cell]` field `field` as JSON (`None`: not one): a commit
    /// serializes only the slices that changed (aio's per-cell serialization cache).
    #[doc(hidden)]
    #[cfg(not(target_arch = "wasm32"))]
    fn slice_json(&self, field: &str) -> Option<serde_json::Result<Value>> {
        let _ = field;
        None
    }
    /// `#[cell(retire = ["drafts", "draftsSeen"])]`: cells no build reads
    /// any more, deleted from the file at this cell's boot (their history,
    /// and in an aio file their slices and stamps), zeroed, never left behind.
    const RETIRED: &'static [&'static str] = &[];
    /// `#[cell(transaction)]` (aio's `transaction`): its async methods read
    /// the state as it was when they were called and commit all they wrote
    /// at their end, at once, or nothing (docs/state/methods.md).
    const TRANSACTION: Option<Transaction> = None;
    /// What is saved of the state, by `#[cell(save = f)]` (`fn(&Self) -> Self`);
    /// `None`: the state as is. Keep it idempotent: a crash and a clean stop
    /// then come back the same.
    fn save(&self) -> Option<Self> {
        None
    }
    /// Repair the state as it comes back from storage: on every boot and
    /// every rewind (`#[cell(restore = f)]`, `fn(Self) -> Self`), e.g. clear
    /// busy flags. A boot that changes the state logs it as a `$restore` event.
    fn restore(self) -> Self {
        self
    }
    /// The state to boot on when the store came back from a damaged file
    /// (`#[cell(recovered = f)]`, `fn(Self, &aio::Recovered) -> Self`;
    /// aio's `onCheckpointRestore` returning a state): `self` is the good
    /// copy's state, `r` says from when. The boot logs what it returns as a
    /// `$restore` event.
    #[cfg(not(target_arch = "wasm32"))]
    fn recovered(self, r: &Recovered) -> Self {
        let _ = r;
        self
    }
    /// `#[cell(machine = field)]`: move the machine for `method` before it
    /// runs, or refuse it (see [`mod@machine`]).
    fn advance(&mut self, method: &str) -> Result {
        let _ = method;
        Ok(())
    }
    /// What is wrong with its machine, on a cell with `methods`.
    fn machine_errors(methods: &[&str]) -> Vec<String> {
        let _ = methods;
        Vec::new()
    }
    /// Its machine's states with no way out.
    fn machine_ends() -> Vec<String> {
        Vec::new()
    }
    /// Its top-level fields as `#[cell]` lays them out (aio's cells: one
    /// signal each): a view reading one through `client!`'s `part`
    /// re-renders only when that one changes.
    const FIELDS: &'static [Field] = &[];
    /// Which fields differ from `other`'s: bit `i` for `FIELDS[i]` (the
    /// 64th and later share bit 63); every bit when it lays out none.
    fn changed_fields(&self, other: &Self) -> u64 {
        let _ = other;
        !0
    }
    /// Set the fields of bits `mask` ([`CellData::changed_fields`]' bits,
    /// below the 64th) to `from`'s.
    #[doc(hidden)]
    fn take_fields(&mut self, from: &Self, mask: u64) {
        let _ = (from, mask);
    }
}

/// A cell's top-level field: its name, and where it lies in the state
/// (offset and size in bytes), so a reference into the state names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: &'static str,
    pub at: usize,
    pub size: usize,
}

impl Field {
    /// The field of `fields` that the byte `offset` of the state lies in
    /// (a zero-sized one only at its very place).
    pub fn holding(fields: &[Field], offset: usize) -> Option<usize> {
        let sized = fields.iter().position(|f| f.size > 0 && (f.at..f.at + f.size).contains(&offset));
        sized.or_else(|| fields.iter().position(|f| f.size == 0 && f.at == offset))
    }
}

/// Who calls from the network, as a `#[client(...)]` rule sees them: the
/// user (a session's, or the one a key holder names: a shared key proves no
/// one in particular) and, signed in with an account, their role now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Caller {
    pub by: By,
    pub role: Option<String>,
}

impl Caller {
    /// The user's name, when a user calls (`By::User`).
    pub fn user(&self) -> Option<&str> {
        match &self.by {
            By::User(u) => Some(u),
            _ => None,
        }
    }
}

/// A `#[cell(visible = f)]` filter: `f(&state, &By)` by who views, or
/// `f(&state, &Caller)` by who views and their role now (aio's `forUser`).
#[doc(hidden)]
pub trait Visible<S, M> {
    fn show(&self, s: &S, who: &Caller) -> S;
}

impl<S, F: Fn(&S, &By) -> S> Visible<S, By> for F {
    fn show(&self, s: &S, who: &Caller) -> S {
        self(s, &who.by)
    }
}

impl<S, F: Fn(&S, &Caller) -> S> Visible<S, Caller> for F {
    fn show(&self, s: &S, who: &Caller) -> S {
        self(s, who)
    }
}

#[doc(hidden)]
pub fn visible<S, M>(f: impl Visible<S, M>, s: &S, who: &Caller) -> S {
    f.show(s, who)
}

type Shown = std::sync::Arc<dyn Fn(&str, Value, &Caller) -> Value + Send + Sync>;

static VISIBLE: std::sync::RwLock<Option<Shown>> = std::sync::RwLock::new(None);

/// The app's `visible` for every cell without its own `#[cell(visible = f)]`
/// (aio's `cellDefaults.visible`): `f(cell, view, viewer)` gets what a
/// client would see of the cell (its `#[server]` fields already out) and
/// returns what it may see. A field it leaves out reaches the client as its
/// default, so the client's copy still reads; one that panics shows the
/// cell's default state (fail closed). A second call replaces the first.
pub fn visible_by_default(f: impl Fn(&str, Value, &Caller) -> Value + Send + Sync + 'static) {
    *VISIBLE.write().unwrap_or_else(|e| e.into_inner()) = Some(std::sync::Arc::new(f));
}

/// Is there a [`visible_by_default`] for cells without their own `visible`?
pub(crate) fn screened<T: CellData>() -> bool {
    T::SCREENED || VISIBLE.read().unwrap_or_else(|e| e.into_inner()).is_some()
}

/// What `viewer` may see of `st`: its own `visible`, else the app's
/// [`visible_by_default`].
pub(crate) fn shown<T: CellData>(st: &T, viewer: &Caller) -> Value {
    let view = st.project(viewer);
    let Some(f) = VISIBLE.read().unwrap_or_else(|e| e.into_inner()).clone().filter(|_| !T::SCREENED) else {
        return view;
    };
    let keys: Vec<String> = view.as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default();
    let shown = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(T::ID, view, viewer)));
    let mut shown = shown.unwrap_or_else(|_| {
        eprintln!("[aio] the visible_by_default hook panicked on {}: its default state is shown", T::ID);
        Value::Null
    });
    if !shown.is_object() {
        shown = Value::Object(Default::default());
    }
    let blank = serde_json::to_value(T::default()).unwrap_or(Value::Null);
    if let Some(m) = shown.as_object_mut() {
        for k in keys {
            if !m.contains_key(&k) {
                m.insert(k.clone(), blank[&k].clone());
            }
        }
    }
    shown
}

/// A schedule a method declares (`#[every("5m")]`, `#[cron("0 9 * * 1")]`,
/// aio's `schedules:`): armed whenever the cell goes live, under the key
/// `@<method>`; one no longer declared is removed. `skip_if_running`: a
/// tick is skipped while the method's effect from the last one still runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Declared {
    pub method: &'static str,
    pub every: Option<i64>,
    pub cron: Option<&'static str>,
    pub skip_if_running: bool,
}

/// What `#[methods]` generates: calls by name, for the log and replay.
///
/// `call` and `read` are aio::Door's, never a method's name:
///
/// ```compile_fail
/// use aio::prelude::*;
/// #[cell]
/// pub struct Counter { pub n: i64 }
/// #[methods]
/// impl Counter {
///     pub fn call(&mut self) { self.n += 1 }
/// }
/// ```
/// ```
/// use aio::prelude::*;
/// #[cell]
/// pub struct Counter { pub n: i64 }
/// #[methods]
/// impl Counter {
///     pub fn bump(&mut self) { self.n += 1 }
/// }
/// ```
///
/// and `#[derived(reads = […])]` lists the state's fields by name:
///
/// ```compile_fail
/// use aio::prelude::*;
/// #[cell]
/// pub struct Counter { pub n: i64 }
/// #[methods]
/// impl Counter {
///     #[derived(reads = ["n"])]
///     pub fn double(&self) -> i64 { self.n * 2 }
/// }
/// ```
/// ```
/// use aio::prelude::*;
/// #[cell]
/// pub struct Counter { pub n: i64 }
/// #[methods]
/// impl Counter {
///     #[derived(reads = [n])]
///     pub fn double(&self) -> i64 { self.n * 2 }
/// }
/// ```
pub trait Cell: CellData {
    const METHODS: &'static [&'static str];
    /// Each method as its Rust signature (`add(text: String) -> Uuid`), in
    /// `METHODS` order: what `ax peer` shows a caller.
    const SIGNATURES: &'static [&'static str] = &[];
    const CLIENT: &'static [&'static str];
    /// `#[client(predict = false)]` methods: a synced copy sends them and
    /// shows only what the server made of them.
    const UNPREDICTED: &'static [&'static str] = &[];
    /// `#[redact]` methods (aio's `redactActions`): their arguments (a
    /// passphrase, a seed) are never kept. Their event keeps the state they
    /// left instead, so replay still works, and shows `[redacted]` wherever
    /// events are shown; the cell's state never reaches a problem report.
    const REDACTED: &'static [&'static str] = &[];
    /// `#[coalesce]` methods: a cursor, a scroll, a selection, called many
    /// times a second (every plain method of a `#[cell(coalesce)]` cell).
    /// Each call shows its state at once, without a write; a run of them
    /// goes into the log `[data] coalesce_ms` (default [`PERSIST_DEBOUNCE`])
    /// after its first call (aio's debounced persistence), or when a call that can't wait (one with an
    /// effect, a schedule, an id) or a time-travel step comes, or the app
    /// closes: as one event, its last call, when that alone ends where the
    /// run did (else call by call). A call that changes nothing logs
    /// nothing. A crash loses at most the window not yet logged, as in aio.
    const COALESCED: &'static [&'static str] = &[];
    /// Its `pub async fn` methods, each with its `#[mode]` (`Queue` unless
    /// set): a call of one ends later, in a step ([`crate::Answer`]).
    const ASYNC: &'static [(&'static str, crate::Mode)] = &[];
    /// Its `#[skip_if_running]` async methods: a schedule's tick of one,
    /// whoever set it (`cx.every`, `cx.after`…), is skipped while one runs
    /// (aio's `skipIfRunning`).
    const SKIP_IF_RUNNING: &'static [&'static str] = &[];
    /// Its `#[skip_travel]` methods (aio's `skipActions`): time travel's
    /// steps and its timeline pass over their events (a tick, a hover);
    /// they stay in the log.
    const UNTRAVELED: &'static [&'static str] = &[];
    /// Its `#[ttl]` async methods: their effects are keyed by their arguments too.
    const TTL: &'static [&'static str] = &[];
    /// Each async method's reads of its own state, as `#[methods]` finds
    /// them in its body: the fields it reads, or `None` when it may read
    /// any (it passes `self` on, calls a method of `self`). A field it only
    /// assigns (`self.f = …`) is a blind write: in a `#[cell(transaction)]`
    /// it never conflicts (aio's read tracking).
    #[doc(hidden)]
    const READS: &'static [(&'static str, Option<&'static [&'static str]>)] = &[];
    /// The fields `method` writes blind ([`Cell::READS`]), as
    /// [`CellData::changed_fields`]' bits (the 64th and later never).
    #[doc(hidden)]
    fn blind(method: &str) -> u64 {
        let Some((_, Some(read))) = Self::READS.iter().find(|(m, _)| *m == method) else { return 0 };
        Self::FIELDS
            .iter()
            .take(63)
            .enumerate()
            .filter(|(_, f)| !read.contains(&f.name))
            .fold(0, |m, (i, _)| m | 1 << i)
    }
    /// The async method `method` is, and its mode (`None`: a plain method).
    fn async_mode(method: &str) -> Option<crate::Mode> {
        Self::ASYNC.iter().find(|(m, _)| *m == method).map(|(_, mode)| *mode)
    }
    /// Is `method` (or a step of it, `method.N`) `#[redact]`?
    fn redacts(method: &str) -> bool {
        Self::REDACT
            || Self::REDACTED.iter().any(|r| method.strip_prefix(r).is_some_and(|s| s.is_empty() || s.starts_with('.')))
    }
    /// The `#[on_start]` method: called as the cell goes live (it starts,
    /// or is enabled again), after the cells it depends on.
    const ON_START: Option<&'static str> = None;
    /// The `#[on_stop]` method: called as it stops being live (shutdown,
    /// disabled, tripped), before its calls are refused.
    const ON_STOP: Option<&'static str> = None;
    /// The `#[on_aio]` method (feature `compat`): run at boot on what aio changed in the file
    /// (the first open, or aio ran since), before the invariants check it;
    /// the adoption is one event with it, and refused, the boot is too.
    const ON_AIO: Option<&'static str> = None;
    /// Its methods' `#[every]` and `#[cron]` schedules.
    const SCHEDULES: &'static [Declared] = &[];
    fn apply(&mut self, cx: &Cx, method: &str, args: Value) -> Result<Value>;
    /// May `who` call `method` with `args` from the network, on this state?
    /// Only `#[client]` methods, each as its `role` / `allow` rule says. Code
    /// in the app (handles, effects, schedules, the local door) never asks.
    fn allow(&self, who: &Caller, method: &str, args: &Value) -> Result {
        let _ = (who, args);
        Err(Rejected::denied(method, "is not a #[client] method"))
    }
    /// The `#[invariant]`s, checked after every method.
    fn check(&self) -> Result {
        Ok(())
    }
}

/// How code reaches a cell: a `Handle`, a UI client, a test. The generated
/// `<Name>Calls` trait gives every door typed methods.
pub trait Door<T: Cell> {
    fn call(&self, method: &str, args: Value) -> Result<Value>;
    /// Call `method` and follow it to its end: an async method's last
    /// value, or the refusal that ended it after an await (aio's `await
    /// cell.method()`); a plain method's answer at once. `Err` here: refused
    /// before anything committed.
    fn ask(&self, method: &str, args: Value) -> Result<Answer> {
        let v = self.call(method, args)?;
        Ok(Answer::ready(match T::async_mode(method) {
            None => Ok(v),
            Some(_) => Err(Rejected::new(format!("{method}: this door can't follow an async method to its end"))),
        }))
    }
    /// Read the current state. `f` must not call the same door.
    fn read<R>(&self, f: impl FnOnce(&T) -> R) -> R;
    /// Keep a resource (a watcher, a socket) under `key`, dropping the one it
    /// replaces; `None` drops it.
    fn own(&self, key: &str, res: Option<Box<dyn std::any::Any + Send>>);
}

/// Call a method through a door, logging a refusal: `call!(d, toggle(id))`
/// gives `Some(result)`, or `None` when refused (needs `<Cell>Calls` in scope).
/// Under `TestUi` the refusal is on its ledger too: unlooked-at, it fails the test.
#[macro_export]
macro_rules! call {
    ($door:expr, $method:ident($($arg:expr),* $(,)?)) => {
        $door.$method($($arg),*).inspect_err(|e| $crate::refused(stringify!($method), e)).ok()
    };
}

/// This app's version, as `ax build` numbered it from `[app] version`
/// (`1.2.<commit count>`, `-dirty.<hash>` for uncommitted edits), else
/// its package's: what it gives `aio_shell::start` and shows.
#[macro_export]
macro_rules! app_version {
    () => {
        match option_env!("AIORT_APP_VERSION") {
            Some(v) => v,
            None => env!("CARGO_PKG_VERSION"),
        }
    };
}

#[doc(hidden)]
pub fn refused(method: &str, e: &Rejected) {
    eprintln!("[aio] {method} refused: {e}");
    #[cfg(all(feature = "testing", not(target_arch = "wasm32")))]
    testing::unseen(format!("{method}: {e}"));
}

/// The process's background engine: schedules, sync serving and synced
/// copies run here, whatever runtime (if any) the UI uses.
#[cfg(not(target_arch = "wasm32"))]
pub fn rt() -> &'static tokio::runtime::Runtime {
    static RT: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("aio-rt")
            .enable_all()
            .build()
            .expect("aio background runtime")
    })
}

#[cfg(not(target_arch = "wasm32"))]
type Closer =
    Box<dyn Fn(std::time::Duration) -> std::pin::Pin<Box<dyn std::future::Future<Output = Drained> + Send>> + Send>;

#[cfg(not(target_arch = "wasm32"))]
fn closers() -> &'static std::sync::Mutex<Vec<Closer>> {
    static C: std::sync::OnceLock<std::sync::Mutex<Vec<Closer>>> = std::sync::OnceLock::new();
    C.get_or_init(Default::default)
}

/// A call about to run, as [`before_call`] sees it.
#[derive(Debug)]
pub struct Call<'a> {
    pub cell: &'static str,
    pub method: &'a str,
    pub args: &'a Value,
    pub by: &'a By,
    /// The network caller's role (see [`Cx::role`]).
    pub role: Option<&'a str>,
    /// The cell's state the call runs on: read it with [`Call::state`].
    pub state: &'a dyn std::any::Any,
    /// Where a network call came from (`None`: the app's own, a tool's).
    pub request: Option<&'a Request>,
}

impl Call<'_> {
    /// The cell's state, as its type `T` (`None`: another cell's call).
    pub fn state<T: Cell>(&self) -> Option<&T> {
        self.state.downcast_ref()
    }
}

/// Where a network call came from (aio's request in `beforeReduce`): the
/// client's address (the TCP peer, or a trusted proxy's last hop) and the
/// headers its link opened with, its credentials (`authorization`,
/// `cookie`) left out. Never recorded: replay doesn't ask the hooks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Request {
    pub ip: Option<std::net::IpAddr>,
    /// Each header as `(name, value)`, the name lower case.
    pub headers: Vec<(String, String)>,
}

impl Request {
    /// The first value of header `name` (any case).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

type Before = std::sync::Arc<dyn Fn(&Call) -> Result + Send + Sync>;

static BEFORE: std::sync::RwLock<Option<Before>> = std::sync::RwLock::new(None);

/// The app's one gate before every call of every cell (aio's `beforeReduce`):
/// an `Err` refuses the call, said to its caller, and nothing runs or is
/// saved; one that panics refuses too (fail closed). Replay never asks it:
/// what it let in is in the log. It runs while the cell is locked: decide
/// from the call, the state it runs on ([`Call::state`]) and where it came
/// from ([`Call::request`]), never call a cell from it.
pub fn before_call(f: impl Fn(&Call) -> Result + Send + Sync + 'static) {
    *BEFORE.write().unwrap_or_else(|e| e.into_inner()) = Some(std::sync::Arc::new(f));
}

type Rewrite = std::sync::Arc<dyn Fn(&Call) -> Result<Option<Value>> + Send + Sync>;

static REWRITE: std::sync::RwLock<Option<Rewrite>> = std::sync::RwLock::new(None);

/// [`before_call`] that may also change the call (aio's `beforeReduce`
/// returning another action): `Ok(Some(args))` runs it with those
/// arguments instead (a default filled in, a value normalized), `Ok(None)`
/// as it came, `Err` refuses it. The log holds what ran, so replay never
/// asks again. It runs after `before_call`, under the same rules.
pub fn rewrite_call(f: impl Fn(&Call) -> Result<Option<Value>> + Send + Sync + 'static) {
    *REWRITE.write().unwrap_or_else(|e| e.into_inner()) = Some(std::sync::Arc::new(f));
}

/// The app's gates on `call`: refused, or the arguments it runs with
/// (`None`: as it came).
pub(crate) fn ask_before(call: &Call) -> Result<Option<Value>> {
    let panicked =
        |hook: &str| Rejected::new(format!("{}.{}: refused: the {hook} hook panicked", call.cell, call.method));
    if let Some(f) = BEFORE.read().unwrap_or_else(|e| e.into_inner()).clone() {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(call)))
            .unwrap_or_else(|_| Err(panicked("before_call")))?;
    }
    let Some(f) = REWRITE.read().unwrap_or_else(|e| e.into_inner()).clone() else { return Ok(None) };
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(call))).unwrap_or_else(|_| Err(panicked("rewrite_call")))
}

pub(crate) type Started = std::sync::Arc<dyn Fn(&str, &str) + Send + Sync>;

static ON_EFFECT: std::sync::RwLock<Vec<Started>> = std::sync::RwLock::new(Vec::new());

/// Hear every effect as it starts (aio's `onEffect`), as its cell and key: one
/// a `Mode::First` drops never starts, so is never heard. Hooks compose (a
/// plugin's and the app's): each one added hears, in the order added.
/// Guarded: one that panics is logged, never takes the cell or the other
/// hooks down; never call a cell from it.
pub fn on_effect(f: impl Fn(&str, &str) + Send + Sync + 'static) {
    ON_EFFECT.write().unwrap_or_else(|e| e.into_inner()).push(std::sync::Arc::new(f));
}

pub(crate) fn effect_started(cell: &str, key: &str) {
    let hooks = ON_EFFECT.read().unwrap_or_else(|e| e.into_inner()).clone();
    for f in hooks {
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(cell, key))).is_err() {
            eprintln!("[aio] an on_effect hook panicked on {cell} `{key}`; the others still hear");
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) type Acted = std::sync::Arc<dyn Fn(&str, &Event, &Value) + Send + Sync>;

#[cfg(not(target_arch = "wasm32"))]
static ON_ACTION: std::sync::RwLock<Vec<Acted>> = std::sync::RwLock::new(Vec::new());

#[cfg(not(target_arch = "wasm32"))]
/// Hear every committed call (aio's `onAction`), observe-only: its cell, its
/// event (method, arguments, who made it, when) and the cell's state after
/// it, as JSON. Hooks compose (the app's and each plugin's), each guarded:
/// one that panics is logged, never takes the cell or the other hooks down.
/// Never call a cell from it; react with [`Handle::listens_to`].
pub fn on_action(f: impl Fn(&str, &Event, &Value) + Send + Sync + 'static) {
    ON_ACTION.write().unwrap_or_else(|e| e.into_inner()).push(std::sync::Arc::new(f));
}

#[cfg(not(target_arch = "wasm32"))]
/// The `on_action` hooks, none when nobody listens (the state isn't copied then).
pub(crate) fn action_hooks() -> Vec<Acted> {
    ON_ACTION.read().unwrap_or_else(|e| e.into_inner()).clone()
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn acted(hooks: &[Acted], cell: &str, ev: &Event, state: &Value) {
    for f in hooks {
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(cell, ev, state))).is_err() {
            eprintln!("[aio] an on_action hook panicked on {cell}.{}; the others still hear", ev.method);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
type Hook = Box<dyn FnOnce() + Send>;

/// The app's stop hooks: `on_stopping`'s, then `on_stop`'s.
#[cfg(not(target_arch = "wasm32"))]
static HOOKS: std::sync::Mutex<(Vec<Hook>, Vec<Hook>)> = std::sync::Mutex::new((Vec::new(), Vec::new()));

/// Run `f` first when the app stops (aio's `onStopping`), while methods still
/// commit: stop the app's own producers (timers, queues, pollers) here, so
/// nothing they write is refused by the drain. Inside the shutdown budget.
#[cfg(not(target_arch = "wasm32"))]
pub fn on_stopping(f: impl FnOnce() + Send + 'static) {
    HOOKS.lock().unwrap_or_else(|e| e.into_inner()).0.push(Box::new(f));
}

/// Run `f` last, once every store is drained and closed (aio's `onStop`):
/// let go of what the app holds. A method called from here is refused.
#[cfg(not(target_arch = "wasm32"))]
pub fn on_stop(f: impl FnOnce() + Send + 'static) {
    HOOKS.lock().unwrap_or_else(|e| e.into_inner()).1.push(Box::new(f));
}

/// The least the stores and `on_stop` get, whatever `on_stopping` used.
#[cfg(not(target_arch = "wasm32"))]
const FLOOR: std::time::Duration = std::time::Duration::from_millis(500);

/// Run `hooks` in order, each a panic logged, within `limit`; hooks still
/// running then are left behind (never holding the process: it exits after
/// `main`), named as `what`.
#[cfg(not(target_arch = "wasm32"))]
fn run_hooks(what: &str, hooks: Vec<Hook>, limit: std::time::Duration) -> Vec<String> {
    if hooks.is_empty() {
        return Vec::new();
    }
    let (done, wait) = std::sync::mpsc::channel();
    let name = what.to_string();
    let spawned = std::thread::Builder::new().name(format!("aio-{what}")).spawn(move || {
        for f in hooks {
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err() {
                eprintln!("[aio] an {name} hook panicked; the shutdown goes on");
            }
        }
        _ = done.send(());
    });
    match spawned.map(|_| wait.recv_timeout(limit)) {
        Ok(Ok(())) => Vec::new(),
        _ => {
            eprintln!("[aio] {what} hooks still ran after {} ms: left behind", limit.as_millis());
            vec![what.to_string()]
        }
    }
}

/// Stop the app, within `budget` in all: its `on_stopping` hooks, then
/// every store this process opened (`Source::open`), drained and closed,
/// then its `on_stop` hooks, then every synced copy's last save
/// (`Remote::connect_saved`). Stores, `on_stop` and the saves each get at
/// least `FLOOR` (500 ms). What was cut at a deadline, named: effects,
/// hooks that were left behind, and saved copies not written. Call it
/// from outside the runtime (the app's main thread), once its window is
/// gone; a second call has nothing left to do.
#[cfg(not(target_arch = "wasm32"))]
pub fn shutdown(budget: std::time::Duration) -> Vec<String> {
    // `ax status` says stopping from here on.
    health::stopping();
    let end = std::time::Instant::now() + budget;
    let left = || end.saturating_duration_since(std::time::Instant::now()).max(FLOOR);
    let (stopping, stop) = std::mem::take(&mut *HOOKS.lock().unwrap_or_else(|e| e.into_inner()));
    let mut cut = run_hooks("on_stopping", stopping, end.saturating_duration_since(std::time::Instant::now()));
    let fs = std::mem::take(&mut *closers().lock().unwrap_or_else(|e| e.into_inner()));
    let stores = left();
    let drained = rt().block_on(async { futures_util::future::join_all(fs.iter().map(|f| f(stores))).await });
    cut.extend(drained.into_iter().flat_map(|d| d.aborted));
    cut.extend(run_hooks("on_stop", stop, left()));
    // Every synced copy's last save (a call made offline just now) on disk.
    #[cfg(feature = "server")]
    cut.extend(sync::flush_saves(left()).into_iter().map(|f| format!("the saved copy {}", f.display())));
    // Its doors: nothing is served past here.
    ipc::unlink_all();
    #[cfg(feature = "compat")]
    compat::local::remove_record();
    // Last: a program the app started and never ended would outlive it.
    process::reap();
    cut
}

#[cfg(not(target_arch = "wasm32"))]
type Checkpoint = Box<dyn Fn() -> Result + Send>;

#[cfg(not(target_arch = "wasm32"))]
fn checkpoints() -> &'static std::sync::Mutex<Vec<Checkpoint>> {
    static C: std::sync::OnceLock<std::sync::Mutex<Vec<Checkpoint>>> = std::sync::OnceLock::new();
    C.get_or_init(Default::default)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn on_shutdown<T: Cell>(h: Handle<T>) {
    let c = h.clone();
    checkpoints().lock().unwrap_or_else(|e| e.into_inner()).push(Box::new(move || c.checkpoint()));
    closers().lock().unwrap_or_else(|e| e.into_inner()).push(Box::new(move |budget| {
        let h = h.clone();
        Box::pin(async move { h.shutdown(budget).await })
    }));
}

/// Every cell the app opened, its state snapshotted now
/// ([`Handle::checkpoint`]): `ax dev` does it before a hot-patch changes
/// the code, so a later start replays only calls the new code made. The
/// cells that couldn't, named with why.
#[doc(hidden)]
#[cfg(not(target_arch = "wasm32"))]
pub fn checkpoint_all() -> Vec<String> {
    checkpoints()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter_map(|c| c().err())
        .map(|e| e.to_string())
        .collect()
}

#[doc(hidden)]
pub fn json(v: &impl Serialize) -> Result<Value> {
    serde_json::to_value(v).map_err(|e| Rejected::new(format!("unserializable: {e}")))
}

#[doc(hidden)]
pub fn args<A: DeserializeOwned>(v: Value) -> Result<A> {
    // No arguments: `null`, or `[]` as aio clients send it.
    let v = if v.as_array().is_some_and(Vec::is_empty) { Value::Null } else { v };
    serde_json::from_value(v).map_err(|e| Rejected::new(format!("bad arguments: {e}")))
}

#[doc(hidden)]
pub fn ret<R: Serialize>(r: Result<R>) -> Result<Value> {
    json(&r?)
}

/// An async method's effect hands its awaited result to the next step; a
/// refused step changes nothing and ends the method there, said to its
/// caller ([`Answer`]).
#[doc(hidden)]
pub fn step<T: Cell>(h: &Handle<T>, method: &str, args: Result<Value>, reply: Option<Reply>) {
    if let Err(e) = args.and_then(|a| h.resume(method, a, reply.clone())) {
        eprintln!("[aio] {}: {method} refused: {e}", T::ID);
        if let Some(r) = reply {
            r.send(Err(e));
        }
    }
}

/// An awaited `io.await?` failed: the method ends there, as if its step
/// refused (nothing more commits), said to its caller and counted as the
/// cell's failure.
#[doc(hidden)]
pub fn fail<T: Cell>(h: &Handle<T>, method: &str, e: Rejected, reply: Option<Reply>) {
    eprintln!("[aio] {}: {method} refused: {e}", T::ID);
    h.failed(&format!("{method}: {e}"));
    if let Some(r) = reply {
        r.send(Err(e));
    }
}

/// An `allow` rule's answer: only `true` lets the call in, and a rule that
/// panics is a refusal, never permission.
#[doc(hidden)]
pub fn allowed(method: &str, rule: &str, f: impl FnOnce() -> bool) -> Result {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(true) => Ok(()),
        Ok(false) => Err(Rejected::denied(method, &format!("is refused by {rule}"))),
        Err(_) => Err(Rejected::denied(method, &format!("is refused: {rule} panicked"))),
    }
}

/// A synced copy's state from the view the server sent: its `#[server]` fields absent.
#[cfg(any(feature = "server", target_arch = "wasm32"))]
pub(crate) fn copy_of<T: CellData>(view: Value) -> serde_json::Result<T> {
    let mut t: T = serde_json::from_value(view)?;
    t.conceal();
    Ok(t)
}

/// The state a copy that holds `paths` of its cell (empty: all of it) makes
/// of the server's `view`: the parts it doesn't hold are the cell's defaults.
#[cfg(any(feature = "server", target_arch = "wasm32"))]
pub(crate) fn held<T: CellData>(paths: &[String], view: &Value) -> serde_json::Result<T> {
    match paths.is_empty() {
        true => copy_of(view.clone()),
        false => copy_of(filter::fill(view, &serde_json::to_value(T::default())?, paths)),
    }
}

#[doc(hidden)]
pub fn typed<R: DeserializeOwned>(r: Result<Value>) -> Result<R> {
    serde_json::from_value(r?).map_err(|e| Rejected::new(format!("bad result: {e}")))
}

/// An enum of plain names, like a TS string union: `keys! { pub enum Prio { Low, Normal, High } }`.
/// Gives Copy/Eq/Hash/Debug, serde as the lowercase name, `ALL`, and `key()`
/// (the same lowercase name, for CSS classes and handles). Extra derives such
/// as `Default` (with `#[default]` on a variant) go on top as usual.
#[macro_export]
macro_rules! keys {
    ($(#[$m:meta])* $vis:vis enum $name:ident { $($(#[$vm:meta])* $v:ident),* $(,)? }) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, $crate::serde::Serialize, $crate::serde::Deserialize)]
        #[serde(crate = "aio::serde", rename_all = "lowercase")]
        $vis enum $name { $($(#[$vm])* $v),* }

        impl $name {
            pub const ALL: [$name; [$(stringify!($v)),*].len()] = [$($name::$v),*];

            pub fn key(self) -> &'static str {
                static KEYS: ::std::sync::OnceLock<Vec<&'static str>> = ::std::sync::OnceLock::new();
                let keys = KEYS.get_or_init(|| {
                    [$(stringify!($v)),*].iter().map(|k| &*Box::leak(k.to_lowercase().into_boxed_str())).collect()
                });
                keys[Self::ALL.iter().position(|x| *x == self).unwrap_or(0)]
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::Rejected;

    #[test]
    fn a_refusal_says_its_kind_by_a_typed_code_never_by_its_words() {
        use super::{CODES, Code, error_summary};
        assert_eq!(Rejected::denied("give", "is not a #[client] method").code(), Some(Code::AccessDenied));
        assert_eq!(Rejected::unknown("nope").code(), Some(Code::ActionRefused));
        // The method's own words are never mistaken for aio's.
        for own in ["access denied: `x` by me", "unknown method `y`", "sign in first"] {
            assert_eq!(Rejected::new(own).code(), None, "{own}");
        }
        // The wire names are the contract.
        assert_eq!(serde_json::to_value(Code::AccessDenied).unwrap(), "ACCESS_DENIED");
        assert_eq!(Code::of(&serde_json::json!("CONNECTION_LOST")), Some(Code::ConnectionLost));
        assert_eq!(Code::of(&serde_json::json!("FROM_A_NEWER_AIO")), Some(Code::Other("FROM_A_NEWER_AIO".into())));
        assert_eq!(Code::of(&serde_json::json!(null)), None);
        for (c, n) in CODES {
            assert_eq!((c.as_str(), Code::of(&serde_json::json!(n))), (n, Some(c.clone())));
            assert_eq!(serde_json::to_value(&c).unwrap(), serde_json::json!(n));
        }
        let long =
            Rejected::coded(Code::ReduceError, format!("notes.add panicked: {}. Then more.\nsecond", "x".repeat(200)));
        let s = error_summary(&long);
        assert!(s.starts_with("REDUCE_ERROR: notes.add panicked: xxx") && s.chars().count() == 160, "{s}");
        assert_eq!(error_summary(&Rejected::new("over budget. Try later.\nmore")), "over budget.");
        assert_eq!(Code::ActionRefused.as_str(), "ACTION_REFUSED");
    }
}

#[cfg(test)]
mod sort_tests {
    #[test]
    fn sort_by_is_stable_and_keeps_every_element() {
        let mut v = vec![(2, 'a'), (1, 'b'), (2, 'c'), (0, 'd'), (1, 'e')];
        super::sort_by(&mut v, |a, b| a.0.cmp(&b.0));
        assert_eq!(v, [(0, 'd'), (1, 'b'), (1, 'e'), (2, 'a'), (2, 'c')]);
        let mut one = vec![String::from("x")];
        super::sort_by(&mut one, Ord::cmp);
        assert_eq!(one, ["x"]);
    }
}

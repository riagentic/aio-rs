//! Problem reports: when something goes wrong (a panic, a crashed or
//! frozen window, a part that keeps failing), a report is written to
//! `data/reports/<id>.json` before anything else happens with it: atomically,
//! owner-only (0600), credentials masked. The same problem is reported once
//! per run, at most [`AUTO_CAP`] automatic ones per run, and the newest
//! [`KEEP`] are kept (`[feedback] keep`). `ax report <app>` lists and shows
//! them. Nothing leaves the machine, unless the app names where to ([`sink`],
//! `[feedback] url`): then each is sent on, after it is on disk. A report carries the app's cells as saved, the newest calls
//! (never their arguments), what the diagnostic bus heard, and the build.
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Automatic reports per run; past it, problems are only logged.
pub const AUTO_CAP: usize = 10;
pub use aio_config::report::KEEP;
/// Log lines a report carries (the newest).
pub const LOG_LINES: usize = 200;
const TITLE_CHARS: usize = 300;
const BODY_CHARS: usize = 20_000;

/// What every report says about the app, and how its reports are kept.
#[derive(Clone, Debug, PartialEq)]
pub struct About {
    pub id: String,
    pub version: String,
    /// `data/reports`.
    pub dir: PathBuf,
    /// `logs/app.log`.
    pub log: PathBuf,
    /// The release channel (`[updates] channel`), if any.
    pub channel: String,
    /// Reports kept (`[feedback] keep`).
    pub keep: usize,
    /// Problems are reported by themselves (`[feedback] auto`); `false`:
    /// only the user's reports are written.
    pub auto: bool,
}

impl Default for About {
    fn default() -> Self {
        let (id, version, dir, log, channel) = Default::default();
        Self { id, version, dir, log, channel, keep: KEEP, auto: true }
    }
}

type Sink = (std::time::Duration, std::sync::Arc<dyn Fn(&Value) -> Result<(), String> + Send + Sync>);
static SINK: Mutex<Option<Sink>> = Mutex::new(None);

/// Send each report on with `f` (aio's `feedback.sink`; `aio_shell` sets one
/// for `[feedback] url`), after it is on disk, never instead. An `Err`, or
/// no answer `within`, leaves it only on disk. An automatic report is sent
/// from a thread of its own; [`send`] sends one now.
pub fn sink(within: std::time::Duration, f: impl Fn(&Value) -> Result<(), String> + Send + Sync + 'static) {
    *SINK.lock().unwrap_or_else(|e| e.into_inner()) = Some((within, std::sync::Arc::new(f)));
}

/// Send the report at `path` on: `Ok(false)` with no [`sink`] to send to.
pub fn send(path: &Path) -> Result<bool, String> {
    let Some((within, f)) = SINK.lock().unwrap_or_else(|e| e.into_inner()).clone() else { return Ok(false) };
    let r: Value =
        serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    // Its own thread: a sink that never answers holds nothing else up.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("aio-report-sink".into())
        .spawn(move || _ = tx.send(f(&r)))
        .map_err(|e| format!("no thread to send it: {e}"))?;
    match rx.recv_timeout(within) {
        Ok(sent) => sent.map(|_| true),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(format!("no answer within {}", crate::fmt::dur(within))),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err("the sink panicked".into()),
    }
}

struct Reporter {
    about: About,
    seen: HashSet<String>,
    auto: usize,
}

static REPORTER: Mutex<Option<Reporter>> = Mutex::new(None);

fn reporter() -> std::sync::MutexGuard<'static, Option<Reporter>> {
    REPORTER.lock().unwrap_or_else(|e| e.into_inner())
}

/// Start reporting for this app (`aio::boot` does, once).
pub fn init(about: About) {
    *reporter() = Some(Reporter { about, seen: HashSet::new(), auto: 0 });
}

/// The app reports are about, once [`init`] ran.
pub fn about() -> Option<About> {
    reporter().as_ref().map(|r| r.about.clone())
}

type Hook = std::sync::Arc<dyn Fn(&str, &str) + Send + Sync>;

static HOOK: Mutex<Option<Hook>> = Mutex::new(None);

/// The app's error hook: it sees every problem, also one not written as a
/// report (a repeat, past the cap), as its `kind` (the code: `panic`,
/// `window:crashed`, `degraded:<part>` …) and message. Guarded: a hook that
/// panics is logged, never re-entered, and never takes the caller down.
/// It also hears warnings that are not reports (`budget:*`, `refused`); see
/// [`Code`]. It runs where the problem happened, maybe while a cell is
/// mid-call: hand the work off (a channel, a task), never call a cell from it.
pub fn on_problem(hook: impl Fn(&str, &str) + Send + Sync + 'static) {
    *HOOK.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::sync::Arc::new(hook));
}

/// Tell the [`on_problem`] hook only: a warning (a blown budget, a refused
/// schedule), not worth a report of its own.
pub(crate) fn tell(kind: &str, message: &str) {
    notify(kind, message);
}

/// What a problem's `kind` is, typed: `on_problem(|kind, _| match Code::of(kind) { … })`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Code {
    /// `panic`: a method's panic refuses its call; anywhere else, a bug.
    Panic,
    /// `tripped`: a cell disabled after too many failures.
    Tripped,
    /// `refused`: the cell's own schedule or effect was refused.
    Refused,
    /// `effect:timeout`: an effect ran past its `timeout` and was cancelled.
    EffectTimeout,
    /// `budget:method`, `budget:effect` (see `perf`).
    BudgetMethod,
    BudgetEffect,
    /// `storm`: a method called in a runaway loop (`[perf.storm]`).
    Storm,
    /// `degraded:<part>`: a best-effort part keeps failing.
    Degraded,
    /// `memory:high`, `memory:growing`.
    Memory,
    /// `window:crashed`, `window:frozen`.
    Window,
    /// `update:failed`.
    UpdateFailed,
    /// `user`: filed by the user.
    User,
    /// Any other kind (an app's own).
    Other,
}

impl Code {
    pub fn of(kind: &str) -> Self {
        match kind.split_once(':').map_or(kind, |(k, _)| k) {
            "panic" => Self::Panic,
            "tripped" => Self::Tripped,
            "refused" => Self::Refused,
            "storm" => Self::Storm,
            "effect" if kind == "effect:timeout" => Self::EffectTimeout,
            "budget" if kind == "budget:method" => Self::BudgetMethod,
            "budget" if kind == "budget:effect" => Self::BudgetEffect,
            "degraded" => Self::Degraded,
            "memory" => Self::Memory,
            "window" => Self::Window,
            "update" if kind == "update:failed" => Self::UpdateFailed,
            "user" => Self::User,
            _ => Self::Other,
        }
    }

    /// The one thing to do about it.
    pub fn tip(self) -> Option<&'static str> {
        Some(match self {
            Self::Panic => "return Err (aio::reject!) for an expected failure; a panic is a bug",
            Self::Tripped => "fix the failing calls (in the log), then enable the cell again (Cells::enable)",
            Self::Refused => "a schedule or effect called a method that refused: check its arguments and state",
            Self::EffectTimeout => "raise the effect's .timeout(…), or make the work it waits on finish sooner",
            Self::BudgetMethod => "move slow work into an Effect, or raise the method's budget ([perf.methods])",
            Self::BudgetEffect => "await async I/O, or use tokio::task::spawn_blocking ([perf.effects])",
            Self::Memory => "look for state or caches that only grow",
            Self::Storm => "find what calls the method in reply to its own change (a feedback loop)",
            _ => return None,
        })
    }
}

/// A framework message that teaches, as one line (pure): what happened and
/// the one fix: `<what> → fix: <fix>`.
pub fn teach(what: &str, fix: &str) -> String {
    format!("{what} → fix: {fix}")
}

fn notify(kind: &str, message: &str) {
    thread_local!(static IN_HOOK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
    // A hook's own panic is a problem too: logged, not handed back to it.
    if IN_HOOK.replace(true) {
        return;
    }
    let hook = HOOK.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let heard = bus().publish(kind, message, now_ms());
    let guarded = |f: &dyn Fn()| std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).err();
    let mut panics: Vec<_> =
        hook.and_then(|h| guarded(&|| h(kind, message))).map(|p| ("on_problem hook", p)).into_iter().collect();
    if let Some((d, subs)) = heard {
        panics.extend(subs.iter().filter_map(|s| guarded(&|| s(&d))).map(|p| ("subscriber", p)));
    }
    IN_HOOK.set(false);
    for (who, p) in panics {
        crate::error!(kind = kind, "the {who} panicked: {}", panic_text(p.as_ref()));
    }
}

/// One thing the bus heard: a problem or a warning, as [`on_problem`] sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// When, ms since the epoch.
    pub at: i64,
    pub kind: String,
    pub message: String,
    /// Of this kind, how many the bus held back since the last one it let
    /// through ([`QUIET_MS`]): nothing is lost, only not said again.
    pub held: u64,
}

/// The same kind within this many ms is counted, not said again.
pub const QUIET_MS: i64 = 5_000;
/// Diagnostics [`recent`] keeps.
pub const RECENT: usize = 200;

type Sub = std::sync::Arc<dyn Fn(&Diagnostic) + Send + Sync>;

/// The diagnostic bus: every problem and warning, a kind at most once per
/// [`QUIET_MS`], to every subscriber and the [`recent`] list.
struct Bus {
    recent: std::collections::VecDeque<Diagnostic>,
    /// Per kind: when it was last let through, and how many held since.
    last: std::collections::BTreeMap<String, (i64, u64)>,
    subs: Vec<(u64, Sub)>,
    next: u64,
}

impl Bus {
    const fn new() -> Self {
        Self {
            recent: std::collections::VecDeque::new(),
            last: std::collections::BTreeMap::new(),
            subs: vec![],
            next: 0,
        }
    }

    /// Let `kind` through at `at`? Then as a diagnostic, with who hears it.
    fn publish(&mut self, kind: &str, message: &str, at: i64) -> Option<(Diagnostic, Vec<Sub>)> {
        let held = match self.last.get_mut(kind) {
            Some((t, n)) if at - *t < QUIET_MS => {
                *n += 1;
                return None;
            }
            Some((t, n)) => {
                *t = at;
                std::mem::take(n)
            }
            None => {
                // Kinds are a small vocabulary; an app's own may not be.
                if self.last.len() >= 1024 {
                    self.last.retain(|_, (t, _)| at - *t < QUIET_MS);
                }
                self.last.insert(kind.into(), (at, 0));
                0
            }
        };
        let d = Diagnostic { at, kind: kind.into(), message: cut(message, 2000), held };
        if self.recent.len() == RECENT {
            self.recent.pop_front();
        }
        self.recent.push_back(d.clone());
        Some((d, self.subs.iter().map(|(_, s)| s.clone()).collect()))
    }
}

fn bus() -> std::sync::MutexGuard<'static, Bus> {
    static B: Mutex<Bus> = Mutex::new(Bus::new());
    B.lock().unwrap_or_else(|e| e.into_inner())
}

/// Hear the bus while this lives (drop it to stop). Like [`on_problem`]:
/// guarded, and never call a cell from it.
#[must_use = "dropping it unsubscribes"]
pub struct Subscription(u64);

impl Drop for Subscription {
    fn drop(&mut self) {
        bus().subs.retain(|(id, _)| *id != self.0);
    }
}

/// Subscribe to the bus: as many as like, each sees a kind at most once per
/// [`QUIET_MS`] (with how many were held back).
pub fn subscribe(f: impl Fn(&Diagnostic) + Send + Sync + 'static) -> Subscription {
    let mut b = bus();
    b.next += 1;
    let id = b.next;
    b.subs.push((id, std::sync::Arc::new(f)));
    Subscription(id)
}

/// The newest [`RECENT`] diagnostics, oldest first.
pub fn recent() -> Vec<Diagnostic> {
    bus().recent.iter().cloned().collect()
}

/// What a linked client may put on this bus per [`QUIET_MS`].
pub const RELAYED: u32 = 20;

/// A diagnostic a linked client (`who`) heard on its own bus (`{kind,
/// message, held}`), as this bus's `client:<kind>` and its message, at `now`.
/// A client's claim off the wire: none without a kind, sizes cut, credentials
/// masked, at most [`RELAYED`] per `window` (when it opened, how many in it),
/// and nothing that already came from a client (two linked in one process
/// would echo forever). Pure.
pub(crate) fn relayed(who: &str, d: &Value, now: i64, window: &mut (i64, u32)) -> Option<(String, String)> {
    let kind = d["kind"].as_str().filter(|k| !k.is_empty() && !k.starts_with("client:"))?;
    if now - window.0 >= QUIET_MS {
        *window = (now, 0);
    }
    if window.1 >= RELAYED {
        return None;
    }
    window.1 += 1;
    let held = d["held"].as_u64().filter(|n| *n > 0).map_or(String::new(), |n| format!(" (+{n} held)"));
    let message = redact(&cut(d["message"].as_str().unwrap_or_default(), 500));
    Some((format!("client:{}", cut(kind, 64)), format!("{who}: {message}{held}")))
}

/// Put what a client heard ([`relayed`]) on this bus.
pub(crate) fn relay(who: &str, d: &Value, window: &mut (i64, u32)) {
    if let Some((kind, message)) = relayed(who, d, now_ms(), window) {
        notify(&kind, &message);
    }
}

/// Something went wrong: written as a report unless the same `kind` and
/// message was reported this run, or [`AUTO_CAP`] were. Returns the file.
/// The [`on_problem`] hook sees it either way.
pub fn problem(kind: &str, message: &str) -> Option<PathBuf> {
    notify(kind, message);
    let about = {
        let mut g = reporter();
        let r = g.as_mut().filter(|r| r.about.auto)?;
        let key = format!("{kind}:{}", cut(message, 200));
        if r.seen.contains(&key) {
            return None;
        }
        if r.auto == AUTO_CAP {
            r.auto += 1;
            eprintln!("[aio] {AUTO_CAP} problem reports this run: further problems are logged, not reported");
        }
        if r.auto > AUTO_CAP {
            return None;
        }
        r.seen.insert(key);
        r.auto += 1;
        r.about.clone()
    };
    let title = format!("{kind}: {}", first_line(message));
    match write(&about, kind, &title, Some(message), now_ms(), &Context::now()) {
        Ok(p) => {
            eprintln!("[aio] problem report written: {}", p.display());
            let sent = p.clone();
            std::thread::spawn(move || {
                if let Err(e) = send(&sent) {
                    eprintln!("[aio] problem report {} could not be delivered: {e}", sent.display());
                }
            });
            Some(p)
        }
        Err(e) => {
            eprintln!("[aio] problem report not written: {e}");
            None
        }
    }
}

/// A report the user files (a "Report a problem" button): not capped.
pub fn file(title: &str, body: &str) -> std::io::Result<PathBuf> {
    file_from(title, body, None)
}

/// [`fn@file`], with how to reach the user (an address, at most 200 chars).
pub fn file_from(title: &str, body: &str, contact: Option<&str>) -> std::io::Result<PathBuf> {
    let about = about().ok_or_else(|| std::io::Error::other("reports are not set up (aio::boot)"))?;
    let mut x = Context::now();
    x.contact = contact.map(|c| cut(c.trim(), CONTACT_CHARS)).filter(|c| !c.is_empty());
    write(&about, "user", title, Some(body), now_ms(), &x)
}

fn write(about: &About, kind: &str, title: &str, body: Option<&str>, at: i64, x: &Context) -> std::io::Result<PathBuf> {
    crate::paths::private_dir(&about.dir)?;
    let id = report_id(at);
    let logs = tail(&about.log, LOG_LINES);
    let r = build(about, &id, at, kind, title, body, &logs, x);
    let p = about.dir.join(format!("{id}.json"));
    crate::paths::write_atomic(&p, format!("{}\n", serde_json::to_string_pretty(&r)?).as_bytes(), true)?;
    prune(&about.dir, about.keep);
    Ok(p)
}

/// The state a report carries, at most this many bytes of it: past it a
/// cell is left out whole (half a state misleads; none does not), named.
pub const STATE_BYTES: usize = 256 << 10;
/// The newest calls a report lists.
pub const TIMELINE: usize = 100;
const CONTACT_CHARS: usize = 200;

/// What a report says besides the problem: the app's cells as they would be
/// saved, the calls that led here, what the bus heard, who to answer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Context {
    /// Cell id → its state as saved (`save` applied, `#[transient]` left out).
    pub state: serde_json::Map<String, Value>,
    /// Cells left out of `state`, and why.
    pub withheld: Vec<String>,
    /// The newest calls: `{at, call, refused?}`, never their arguments.
    pub timeline: Vec<Value>,
    pub diagnostics: Vec<Diagnostic>,
    pub contact: Option<String>,
}

type Snap = Box<dyn Fn() -> Option<Value> + Send + Sync>;
/// Each cell: its id, whether its state is withheld (`#[redact]`), its state.
static CELLS: Mutex<Vec<(&'static str, bool, Snap)>> = Mutex::new(Vec::new());
static CALLS: Mutex<std::collections::VecDeque<Value>> = Mutex::new(std::collections::VecDeque::new());

/// A cell of the app's own (`Source::open` does): its saved state goes in
/// every report from now on, unless `withheld` (it has `#[redact]` methods).
pub(crate) fn watch(id: &'static str, withheld: bool, snap: impl Fn() -> Option<Value> + Send + Sync + 'static) {
    let mut c = CELLS.lock().unwrap_or_else(|e| e.into_inner());
    c.retain(|(i, ..)| *i != id);
    c.push((id, withheld, Box::new(snap)));
}

/// A call ran, in `took`: the newest [`TIMELINE`] are kept in memory
/// ([`timeline`]) and go in reports (no arguments: they may be anything the
/// user typed).
pub(crate) fn called(cell: &str, method: &str, at: i64, took: std::time::Duration, refused: Option<&str>) {
    let ms = (took.as_secs_f64() * 1e6).round() / 1e3;
    let mut e = json!({ "at": crate::log::stamp(at), "call": format!("{cell}.{method}"), "ms": ms });
    if let Some(r) = refused {
        e["refused"] = cut(&redact(r), 200).into();
    }
    let mut c = CALLS.lock().unwrap_or_else(|e| e.into_inner());
    if c.len() == TIMELINE {
        c.pop_front();
    }
    c.push_back(e);
}

/// The newest calls, oldest first, live (aio's dispatch timeline): when,
/// which, how long it took (`ms`), and a refusal's reason.
pub fn timeline() -> Vec<Value> {
    CALLS.lock().unwrap_or_else(|e| e.into_inner()).iter().cloned().collect()
}

/// The app's cells by the size of their saved state, biggest first: what a
/// memory problem names.
pub(crate) fn sizes() -> Vec<(&'static str, u64)> {
    let cells = CELLS.lock().unwrap_or_else(|e| e.into_inner());
    let mut all: Vec<_> =
        cells.iter().filter_map(|(id, _, snap)| Some((*id, snap()?.to_string().len() as u64))).collect();
    all.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    all
}

impl Context {
    /// This process's, now.
    fn now() -> Self {
        let mut x = Context { diagnostics: recent(), ..Default::default() };
        x.timeline = timeline();
        let mut room = STATE_BYTES;
        for (id, withheld, snap) in CELLS.lock().unwrap_or_else(|e| e.into_inner()).iter() {
            if *withheld {
                x.withheld.push(format!("{id}: it has #[redact] methods"));
                continue;
            }
            let v = std::panic::catch_unwind(std::panic::AssertUnwindSafe(snap)).ok().flatten();
            match v.map(|v| (v.to_string().len(), v)) {
                Some((n, v)) if n <= room => {
                    room -= n;
                    x.state.insert(id.to_string(), v);
                }
                Some((n, _)) => x.withheld.push(format!("{id}: {n} bytes, over the {STATE_BYTES} the report holds")),
                None => x.withheld.push(format!("{id}: its state could not be read")),
            }
        }
        x
    }
}

/// Every string in `v` masked ([`redact`]).
fn masked(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(redact(s)),
        Value::Array(a) => Value::Array(a.iter().map(masked).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), masked(v))).collect()),
        v => v.clone(),
    }
}

/// The report, all text masked and capped (pure).
#[allow(clippy::too_many_arguments)]
pub fn build(
    about: &About,
    id: &str,
    at: i64,
    kind: &str,
    title: &str,
    body: Option<&str>,
    logs: &[String],
    x: &Context,
) -> Value {
    let mut truncated = vec![];
    let mut capped = |what: &str, s: &str, n: usize| {
        if s.chars().count() > n {
            truncated.push(what.to_string());
        }
        redact(&cut(s, n))
    };
    let title = capped("title", title, TITLE_CHARS);
    let body = body.map(|b| capped("body", b, BODY_CHARS));
    let logs: Vec<String> = logs.iter().map(|l| redact(l)).collect();
    let diagnostics: Vec<Value> = x
        .diagnostics
        .iter()
        .map(
            |d| json!({ "at": crate::log::stamp(d.at), "kind": d.kind, "message": redact(&d.message), "held": d.held }),
        )
        .collect();
    json!({
        "id": id,
        "createdAt": crate::log::stamp(at),
        "kind": kind,
        "tip": Code::of(kind).tip(),
        "title": title,
        "body": body,
        "contact": x.contact.as_deref().map(redact),
        "app": {
            "id": about.id,
            "version": about.version,
            "channel": (!about.channel.is_empty()).then_some(&about.channel),
            "build": if cfg!(debug_assertions) { "debug" } else { "release" },
            "aio": env!("CARGO_PKG_VERSION"),
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "pid": std::process::id(),
        },
        "state": masked(&Value::Object(x.state.clone())),
        "withheld": x.withheld,
        "timeline": masked(&Value::Array(x.timeline.clone())),
        "diagnostics": diagnostics,
        "logs": logs,
        "truncated": truncated,
    })
}

/// `2026-10-28T09-48-01-123Z-00001-3f9a0c`: sorts by time, then by order
/// within the run; the random part keeps two runs apart.
fn report_id(at: i64) -> String {
    static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let rand = crate::uuid_v4().simple().to_string();
    format!("{}-{seq:05}-{}", crate::log::stamp(at).replace([':', '.'], "-"), &rand[..6])
}

/// Keep the newest `keep` reports (their names sort by time).
fn prune(dir: &Path, keep: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut names: Vec<_> =
        rd.flatten().map(|e| e.file_name()).filter(|n| n.to_string_lossy().ends_with(".json")).collect();
    names.sort();
    let extra = names.len().saturating_sub(keep);
    for n in &names[..extra] {
        _ = std::fs::remove_file(dir.join(n));
    }
}

/// The last `n` lines of `file` (none if unreadable).
fn tail(file: &Path, n: usize) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(file) else { return vec![] };
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].iter().map(|l| l.to_string()).collect()
}

/// The reports in `dir`, newest first: (id, the report).
pub fn list(dir: &Path) -> Vec<(String, Value)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut all: Vec<_> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let id = name.strip_suffix(".json")?.to_string();
            let v = serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok()?;
            Some((id, v))
        })
        .collect();
    all.sort_by(|a, b| b.0.cmp(&a.0));
    all
}

/// Credentials masked with `…`: `token=` values (URLs, env), `Bearer`
/// tokens and pairing codes. Applied to every log line and report.
pub fn redact(s: &str) -> String {
    const KEYS: [&str; 4] = ["token=", "TOKEN=", "Bearer ", "pair code: "];
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some((i, key)) = KEYS.iter().filter_map(|k| rest.find(k).map(|i| (i, *k))).min_by_key(|(i, _)| *i) {
        let start = i + key.len();
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let end = after.find(|c: char| c.is_whitespace() || "&#'\"`,;)]}".contains(c)).unwrap_or(after.len());
        if end > 0 {
            out.push('…');
        }
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

fn cut(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("")
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// Report every panic (also a method's, which the store then refuses):
/// after the default hook printed it, so the log has it first.
pub fn on_panic() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        // In a dev build, the code that panicked and the fix, under it.
        if cfg!(debug_assertions)
            && let Some(l) = info.location()
        {
            for line in source(l.file()).map(|s| excerpt(&s, l.line(), l.column())).unwrap_or_default() {
                eprintln!("  {}", crate::log::safe(&line));
            }
            eprintln!("  → fix: {}", Code::Panic.tip().unwrap_or_default());
        }
        let what = panic_text(info.payload());
        let at = info.location().map(|l| format!(" at {}:{}", l.file(), l.line())).unwrap_or_default();
        let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
        let during = DURING.with_borrow(|d| d.as_ref().map(|d| format!(" in {d}"))).unwrap_or_default();
        problem("panic", &format!("{what}{at}{during} (thread {thread})"));
    }));
}

thread_local!(static DURING: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) });

/// Run `f` as `what` (`notes.add`, `effect notes.fetch …`): a panic in it
/// says which call or effect it was, the correlation a stack trace lacks.
pub(crate) fn during<R>(what: impl FnOnce() -> String, f: impl FnOnce() -> R) -> R {
    struct Back(Option<String>);
    impl Drop for Back {
        fn drop(&mut self) {
            DURING.set(self.0.take());
        }
    }
    let _back = Back(DURING.replace(Some(what())));
    f()
}

/// The app's own source file a panic points at: a relative path (a
/// dependency's or the standard library's is absolute), read from where the
/// app runs, and only a plausible size.
fn source(file: &str) -> Option<String> {
    let p = Path::new(file);
    if p.is_absolute() || std::fs::metadata(p).ok()?.len() > 1 << 20 {
        return None;
    }
    std::fs::read_to_string(p).ok()
}

/// The lines around `line` of `text` (1-based), numbered, with a caret under
/// `col` (pure). Nothing when `line` is not in it.
pub fn excerpt(text: &str, line: u32, col: u32) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let at = line as usize;
    if at == 0 || at > lines.len() {
        return vec![];
    }
    let (from, to) = (at.saturating_sub(1).max(1), (at + 1).min(lines.len()));
    let w = to.to_string().len();
    let mut out = vec![];
    for n in from..=to {
        let text = lines[n - 1].replace('\t', "  ");
        out.push(format!("{n:>w$} │ {text}"));
        if n == at && col > 0 {
            out.push(format!("{:w$} │ {}^", "", " ".repeat(col as usize - 1)));
        }
    }
    out
}

/// What a panic said (`panic!("…")`'s text), or nothing.
/// What a panic said (its `&str` or `String` payload; else nothing).
pub fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_never_reach_a_log_or_report() {
        assert_eq!(redact("GET /ws?token=abc123&x=1 ok"), "GET /ws?token=…&x=1 ok");
        assert_eq!(redact("AIORT_TOKEN=s3cr3t rest"), "AIORT_TOKEN=… rest");
        assert_eq!(redact("authorization: Bearer eyJ.x.y"), "authorization: Bearer …");
        assert_eq!(redact("pair code: 12345678"), "pair code: …");
        assert_eq!(redact("token= nothing"), "token= nothing");
        assert_eq!(redact("plain line"), "plain line");
    }

    #[test]
    fn the_bus_says_a_kind_once_per_window_and_counts_the_rest() {
        let mut b = Bus::new();
        let kinds = |b: &mut Bus, k: &str, at| b.publish(k, "m", at).map(|(d, _)| d.held);
        assert_eq!(kinds(&mut b, "budget:method", 0), Some(0));
        assert_eq!(kinds(&mut b, "budget:method", 1000), None, "within the window: held");
        assert_eq!(kinds(&mut b, "budget:method", 4999), None);
        assert_eq!(kinds(&mut b, "refused", 1000), Some(0), "per kind: one cannot hide another");
        assert_eq!(kinds(&mut b, "budget:method", QUIET_MS), Some(2), "the next one says how many were held");
        assert_eq!(kinds(&mut b, "budget:method", 3 * QUIET_MS), Some(0), "a quiet kind carries no count");
        for i in 0..RECENT + 10 {
            b.publish(&format!("mine:{i}"), &"x".repeat(5000), 100 * QUIET_MS);
        }
        assert_eq!(b.recent.len(), RECENT, "capped");
        assert_eq!((b.recent[0].kind.as_str(), b.recent[0].message.len()), ("mine:10", 2000));
        // Stale kinds go when there are many: the map is bounded too.
        for i in 0..2000 {
            b.publish(&format!("flood:{i}"), "m", 200 * QUIET_MS + i * 10);
        }
        assert!(b.last.len() <= 1024 + 1, "{}", b.last.len());
    }

        #[test]
    fn a_clients_diagnostic_is_relayed_bounded_and_never_echoed() {
        let mut w = (0, 0);
        let d = json!({ "kind": "window:frozen", "message": "token=abc stuck", "held": 3 });
        assert_eq!(
            relayed("ana from 10.0.0.2", &d, 1, &mut w),
            Some(("client:window:frozen".into(), "ana from 10.0.0.2: token=… stuck (+3 held)".into()))
        );
        assert_eq!(relayed("a", &json!({ "kind": "client:x", "message": "m" }), 1, &mut w), None, "no echo");
        assert_eq!(relayed("a", &json!({ "message": "m" }), 1, &mut w), None, "a kind is needed");
        let (k, m) = relayed("a", &json!({ "kind": "k".repeat(99), "message": "m".repeat(999) }), 1, &mut w).unwrap();
        assert_eq!((k.len(), m.len()), ("client:".len() + 64, "a: ".len() + 500));
        while relayed("a", &json!({ "kind": "x" }), 2, &mut w).is_some() {}
        assert_eq!(w.1, RELAYED, "a flood is cut per window");
        assert!(relayed("a", &json!({ "kind": "x" }), 2 + QUIET_MS, &mut w).is_some(), "the next window opens");
    }

    #[test]
    fn a_kind_is_a_typed_code_with_its_fix() {
        let codes =
            ["panic", "tripped", "budget:method", "budget:effect", "effect:timeout", "degraded:sync"].map(Code::of);
        assert_eq!(
            codes,
            [Code::Panic, Code::Tripped, Code::BudgetMethod, Code::BudgetEffect, Code::EffectTimeout, Code::Degraded]
        );
        assert_eq!((Code::of("budget:other"), Code::of("mine")), (Code::Other, Code::Other));
        assert!(Code::Panic.tip().unwrap().contains("aio::reject!"));
        assert_eq!(Code::User.tip(), None);
        assert_eq!(teach("no window", "install the runtime"), "no window → fix: install the runtime");
        let about = About::default();
        assert_eq!(
            build(&about, "i", 0, "effect:timeout", "t", None, &[], &Context::default())["tip"],
            Code::EffectTimeout.tip().unwrap()
        );
        assert_eq!(build(&about, "i", 0, "user", "t", None, &[], &Context::default())["tip"], Value::Null);
    }

    #[test]
    fn a_panic_shows_the_code_around_it() {
        let src = "fn a() {\n\tlet x = 1;\n    panic!(\"no\");\n}\n";
        assert_eq!(excerpt(src, 3, 5), ["2 │   let x = 1;", "3 │     panic!(\"no\");", "  │     ^", "4 │ }"]);
        assert_eq!(excerpt(src, 1, 0), ["1 │ fn a() {", "2 │   let x = 1;"], "first line, no column");
        assert!(excerpt(src, 9, 1).is_empty() && excerpt(src, 0, 1).is_empty(), "the file changed since");
        let ten = "x\n".repeat(10);
        assert_eq!(excerpt(&ten, 10, 1), [" 9 │ x", "10 │ x", "   │ ^"], "a wide gutter lines up");
        assert_eq!(source("/abs/elsewhere.rs"), None, "not the app's own file");
    }

    #[test]
    fn a_report_carries_state_calls_diagnostics_facts_and_contact_masked() {
        let about = About { id: "notes".into(), version: "1.2.0".into(), channel: "beta".into(), ..Default::default() };
        let mut x = Context {
            contact: Some("ana@example.org".into()),
            withheld: vec!["big: too big".into()],
            ..Default::default()
        };
        x.state.insert("notes".into(), json!({ "url": "https://h/?token=abc", "n": 3 }));
        x.timeline.push(json!({ "at": "t", "call": "notes.add", "refused": "Bearer xyz" }));
        x.diagnostics.push(Diagnostic { at: 0, kind: "storm".into(), message: "token=q".into(), held: 2 });
        let r = build(&about, "i", 0, "user", "t", None, &[], &x);
        assert_eq!(r["state"]["notes"], json!({ "url": "https://h/?token=…", "n": 3 }), "state is masked too");
        assert_eq!(
            (r["withheld"][0].as_str(), r["timeline"][0]["refused"].as_str()),
            (Some("big: too big"), Some("Bearer …"))
        );
        assert_eq!(
            r["diagnostics"][0],
            json!({ "at": "1970-01-01T00:00:00.000Z", "kind": "storm", "message": "token=…", "held": 2 })
        );
        assert_eq!((r["contact"].as_str(), r["app"]["channel"].as_str()), (Some("ana@example.org"), Some("beta")));
        assert_eq!(r["app"]["build"], if cfg!(debug_assertions) { "debug" } else { "release" });
        let bare = build(&About::default(), "i", 0, "user", "t", None, &[], &Context::default());
        assert_eq!((bare["contact"].clone(), bare["app"]["channel"].clone()), (Value::Null, Value::Null));
    }

    #[test]
    fn cells_are_sized_by_their_saved_state_biggest_first() {
        watch("t_small", false, || Some(json!({ "n": 1 })));
        watch("t_big", false, || Some(json!({ "items": "x".repeat(100) })));
        watch("t_gone", false, || None);
        watch("t_secret", true, || Some(json!({ "k": "vv" })));
        let mine: Vec<_> = sizes().into_iter().filter(|(c, _)| c.starts_with("t_")).collect();
        assert_eq!(
            mine,
            [("t_big", 112), ("t_secret", 10), ("t_small", 7)],
            "an unreadable one is left out; a withheld one is sized"
        );
    }

    #[test]
    fn a_report_is_capped_masked_and_names_its_cuts() {
        let about = About { id: "notes".into(), version: "1.2.0".into(), ..Default::default() };
        let long = "x".repeat(400);
        let r =
            build(&about, "id1", 0, "panic", &long, Some("see token=abc"), &["a token=zz".into()], &Context::default());
        assert_eq!(r["title"].as_str().unwrap().len(), 300);
        assert_eq!(r["body"], "see token=…");
        assert_eq!(r["logs"][0], "a token=…");
        assert_eq!(r["truncated"], json!(["title"]));
        assert_eq!(
            (r["app"]["id"].as_str(), r["createdAt"].as_str()),
            (Some("notes"), Some("1970-01-01T00:00:00.000Z"))
        );
    }

    #[test]
    fn problems_are_written_privately_once_each_capped_and_pruned() {
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("app.log");
        std::fs::write(&log, "one\ntwo\n").unwrap();
        init(About {
            id: "notes".into(),
            version: "1".into(),
            dir: d.path().join("reports"),
            log,
            ..Default::default()
        });
        let p = problem("panic", "boom\nmore").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let v: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        assert_eq!((v["title"].as_str(), v["logs"].as_array().unwrap().len()), (Some("panic: boom"), 2));
        // The same problem again: not reported twice.
        assert_eq!(problem("panic", "boom\nmore"), None);
        for i in 0..20 {
            problem("panic", &format!("other {i}"));
        }
        let dir = d.path().join("reports");
        assert_eq!(list(&dir).len(), AUTO_CAP, "capped per run");
        // A user's report is never capped; old ones go past KEEP.
        for _ in 0..KEEP {
            file("Slow", "it's slow").unwrap();
        }
        let all = list(&dir);
        assert_eq!(all.len(), KEEP);
        assert!(all.iter().all(|(_, v)| v["kind"] == "user"), "the oldest went first");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), KEEP, "no temp files left");
    }
}

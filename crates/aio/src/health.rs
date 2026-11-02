//! Health. A part that fails [`AFTER`] times in a row is degraded:
//! said once in the log, reported (`aio::report`), and said again when it
//! recovers. Memory is sampled every [`EVERY`]: a process holding half the
//! machine, or one that keeps growing, degrades too. The app writes all of it
//! to `data/health.json` (0600) every [`EVERY`]; `ax health <app>` reads it,
//! and fails for a degraded, stuck or stopped app. A served app also answers
//! `GET /__aio/health` (this document, fresh) and `GET /__aio/metrics`
//! (Prometheus text, [`prometheus`]), to who shows the key.
//! An app watches its own best-effort work with [`part`], and names what can
//! grow with [`gauge`] and [`budget`]: a memory problem says the biggest.
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// Failures in a row before a part is degraded.
pub const AFTER: u32 = 5;
/// How often memory is sampled and `health.json` written.
pub const EVERY: Duration = Duration::from_secs(10);
/// A `health.json` older than this is from an app that is stuck or gone.
pub const STALE_MS: i64 = 30_000;

#[derive(Clone, Debug, PartialEq)]
pub struct Episode {
    pub failures: u32,
    /// When the first failure of this run of failures was, ms since the epoch.
    pub since: i64,
    pub last_error: String,
}

static PARTS: Mutex<BTreeMap<String, Episode>> = Mutex::new(BTreeMap::new());

fn parts() -> std::sync::MutexGuard<'static, BTreeMap<String, Episode>> {
    PARTS.lock().unwrap_or_else(|e| e.into_inner())
}

/// `name` failed again. The [`AFTER`]th failure in a row degrades it:
/// logged and reported.
pub fn fail(name: &str, error: &str) {
    if bump(name, error, false) {
        crate::report::problem(&format!("degraded:{name}"), &crate::report::redact(error));
    }
}

/// `name` is down now (a window that crashed or hangs): degraded at once,
/// logged; the caller reports what happened.
pub fn degrade(name: &str, error: &str) {
    bump(name, error, true);
}

/// Count a failure; true when this one degrades `name`.
fn bump(name: &str, error: &str, at_once: bool) -> bool {
    let now = now_ms();
    let error: String = crate::report::redact(error).chars().take(200).collect();
    let after = after(name);
    let (escalate, n) = {
        let mut p = parts();
        let e = p.entry(name.into()).or_insert(Episode { failures: 0, since: now, last_error: String::new() });
        let was = e.failures;
        e.failures = if at_once { e.failures.max(after) } else { e.failures + 1 };
        e.last_error = error.clone();
        (was < after && e.failures >= after, e.failures)
    };
    if escalate {
        let why = if at_once { String::new() } else { format!(" ({n} failures in a row)") };
        eprintln!("[aio] {name} is degraded{why}: {error}");
    }
    escalate
}

/// `name` worked: a degraded part says it recovered.
pub fn ok(name: &str) {
    let Some(e) = parts().remove(name) else { return };
    if e.failures >= after(name) {
        let secs = (now_ms() - e.since) / 1000;
        let line = format!("{name} recovered ({secs} s after it began failing)");
        eprintln!("[aio] {line}");
        crate::report::say(&format!("degraded-recovered:{name}"), &line, crate::report::Severity::Info);
    }
}

/// The degraded parts, by name.
pub fn degraded() -> Vec<(String, Episode)> {
    let p = parts();
    p.iter().filter(|(n, e)| e.failures >= after(n)).map(|(n, e)| (n.clone(), e.clone())).collect()
}

/// Thresholds set by [`Part::after`]; any other part's is [`AFTER`].
static AFTERS: Mutex<BTreeMap<String, u32>> = Mutex::new(BTreeMap::new());

fn after(name: &str) -> u32 {
    AFTERS.lock().unwrap_or_else(|e| e.into_inner()).get(name).copied().unwrap_or(AFTER)
}

/// A best-effort part, by name: `let cache = health::part("cache");` then
/// `cache.guard(read())` keeps going without the value, but not silently.
/// The same name is the same part everywhere.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Part(String);

/// The part called `name` (its first 64 characters: names are a small,
/// fixed vocabulary, never data).
pub fn part(name: &str) -> Part {
    Part(name.chars().take(64).collect())
}

impl Part {
    /// Degraded after `n` failures in a row (at least 1) instead of
    /// [`AFTER`]. One threshold per name: the first one set stays, and
    /// another is warned about.
    pub fn after(self, n: u32) -> Self {
        let n = n.max(1);
        let had = *AFTERS.lock().unwrap_or_else(|e| e.into_inner()).entry(self.0.clone()).or_insert(n);
        if had != n {
            crate::warn!(part = self.0, "after({n}) asked, but this part degrades after {had}: one threshold per name");
        }
        self
    }

    pub fn fail(&self, error: impl std::fmt::Display) {
        fail(&self.0, &error.to_string());
    }

    pub fn ok(&self) {
        ok(&self.0);
    }

    /// The value of `r`, counted: `Ok` is a success, `Err` a failure (and
    /// `None`, so the caller degrades gracefully).
    pub fn guard<T, E: std::fmt::Display>(&self, r: Result<T, E>) -> Option<T> {
        match r {
            Ok(v) => {
                self.ok();
                Some(v)
            }
            Err(e) => {
                self.fail(e);
                None
            }
        }
    }

    /// Failures in a row right now.
    pub fn failures(&self) -> u32 {
        parts().get(&self.0).map_or(0, |e| e.failures)
    }

    pub fn is_degraded(&self) -> bool {
        self.failures() >= after(&self.0)
    }
}

/// What a gauge counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Unit {
    Bytes,
    Count,
}

/// A named series: a `level` (a size now, that rises and falls) or a
/// `counter` (work done, that only grows: it needs a [`budget`]).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Reading {
    /// Stable and dotted, its owner first: `sync.queue`.
    pub name: String,
    pub unit: Unit,
    pub counter: bool,
    /// `None`: its reader failed (a part mid-teardown).
    pub value: Option<u64>,
    /// A budget's ceiling.
    pub bound: Option<u64>,
}

type Read = std::sync::Arc<dyn Fn() -> u64 + Send + Sync>;

/// The watched series; at most [`GAUGES`] (one per resource would be a
/// leak with a name).
static LEDGER: Mutex<Vec<(Reading, Read)>> = Mutex::new(Vec::new());
pub const GAUGES: usize = 64;

fn register(r: Reading, read: Read) {
    let mut l = LEDGER.lock().unwrap_or_else(|e| e.into_inner());
    // The first registration wins: a second owner of a name is not wired to it.
    if l.iter().any(|(g, _)| g.name == r.name) {
        return;
    }
    if l.len() == GAUGES {
        static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
            crate::warn!(cat: "memory", gauge = r.name, "over {GAUGES} gauges: ignored; register bounded series only");
        }
        return;
    }
    l.push((r, read));
}

/// Watch a size by name (`sync.queue`), read cheaply each time health
/// samples: `health.json` lists it, and a memory problem names the biggest.
pub fn gauge(name: &str, unit: Unit, read: impl Fn() -> u64 + Send + Sync + 'static) {
    let r = Reading { name: name.into(), unit, counter: false, value: None, bound: None };
    register(r, std::sync::Arc::new(read));
}

/// Every series, read now (a reader that panics reads `None`).
pub fn gauges() -> Vec<Reading> {
    let all: Vec<_> = LEDGER.lock().unwrap_or_else(|e| e.into_inner()).clone();
    all.into_iter()
        .map(|(r, read)| {
            let value = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| read())).ok();
            Reading { value, ..r }
        })
        .collect()
}

/// A named counter with a ceiling: work past it is refused by name, so a
/// loop stops at a stated bound instead of eating the machine.
#[derive(Clone, Debug)]
pub struct Budget {
    name: String,
    max: u64,
    used: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

/// The budget called `name` (one per name: a second call shares the first's
/// counter and ceiling), watched as a counter gauge.
pub fn budget(name: &str, max: std::num::NonZeroU64) -> Budget {
    static ALL: Mutex<BTreeMap<String, Budget>> = Mutex::new(BTreeMap::new());
    let mut all = ALL.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(b) = all.get(name) {
        return b.clone();
    }
    let b = Budget { name: name.into(), max: max.get(), used: Default::default() };
    let used = b.used.clone();
    let r = Reading { name: name.into(), unit: Unit::Count, counter: true, value: None, bound: Some(b.max) };
    register(r, std::sync::Arc::new(move || used.load(std::sync::atomic::Ordering::Relaxed)));
    all.insert(name.into(), b.clone());
    b
}

impl Budget {
    /// Count `n` more; past the ceiling, refused: `MEMORY_UNBOUNDED`, by name.
    pub fn spend(&self, n: u64) -> crate::Result {
        let used = self.used.fetch_add(n, std::sync::atomic::Ordering::Relaxed).saturating_add(n);
        if used > self.max {
            crate::reject!(
                "{}",
                crate::report::teach(
                    &format!("MEMORY_UNBOUNDED: {} passed its ceiling: {used} > {} (+{n})", self.name, self.max),
                    &format!("find the loop that keeps feeding {}: a counter only grows", self.name)
                )
            );
        }
        Ok(())
    }

    pub fn value(&self) -> u64 {
        self.used.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn max(&self) -> u64 {
        self.max
    }

    /// Back to zero (a per-session series); the ceiling stays.
    pub fn reset(&self) {
        self.used.store(0, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The biggest byte gauges, for a memory problem (pure): `sync.queue
/// 120.0 MB, cache 3.0 MB`.
pub fn biggest(gauges: &[Reading], n: usize) -> String {
    let mut sizes: Vec<_> = gauges
        .iter()
        .filter(|g| g.unit == Unit::Bytes && !g.counter)
        .filter_map(|g| Some((g.value?, &g.name)))
        .collect();
    sizes.sort_by_key(|(v, _)| std::cmp::Reverse(*v));
    sizes.iter().take(n).map(|(v, name)| format!("{name} {}", crate::fmt::bytes(*v))).collect::<Vec<_>>().join(", ")
}

/// What each linked client says is degraded on its side (aio's `cdiag`), by
/// link, while the link lives.
static CLIENTS: Mutex<BTreeMap<u64, BTreeMap<String, Episode>>> = Mutex::new(BTreeMap::new());

/// Names a client may report at most; longer names and errors are cut.
const CLIENT_NAMES: usize = 16;

/// The client on link `link` (`who`: its user and address, for the log) says
/// these parts of it are degraded: `[{name, failures, since, lastError}]`,
/// the whole list each time (an empty one: all recovered). A client's claim
/// off the wire: malformed rows are dropped, numbers no client can truly
/// have are not taken as fact, sizes are capped, and each new name is said
/// in the log once, naming who said it.
pub fn client_says(link: u64, who: &str, down: &Value) {
    let now = now_ms();
    let rows = down.as_array().into_iter().flatten().filter_map(|d| {
        let name: String = d["name"].as_str().filter(|n| !n.is_empty())?.chars().take(64).collect();
        let failures = d["failures"].as_u64().map_or(0, |f| u32::try_from(f).unwrap_or(u32::MAX));
        let since = d["since"].as_i64().filter(|t| (1..=now).contains(t)).unwrap_or(now);
        let last_error =
            crate::report::redact(&d["lastError"].as_str().unwrap_or_default().chars().take(200).collect::<String>());
        Some((name, Episode { failures, since, last_error }))
    });
    let rows: BTreeMap<String, Episode> = rows.take(CLIENT_NAMES).collect();
    let mut all = CLIENTS.lock().unwrap_or_else(|e| e.into_inner());
    let old = all.remove(&link).unwrap_or_default();
    for (name, e) in rows.iter().filter(|(n, _)| !old.contains_key(*n)) {
        eprintln!("[aio] a client ({who}) says its {name} is degraded ({} failures): {}", e.failures, e.last_error);
    }
    if !rows.is_empty() {
        all.insert(link, rows);
    }
}

/// Link `link` closed: what its client said is no longer live.
pub fn client_gone(link: u64) {
    CLIENTS.lock().unwrap_or_else(|e| e.into_inner()).remove(&link);
}

/// What the client on link `link` says is degraded, by name.
pub(crate) fn client_parts(link: u64) -> Vec<(String, Episode)> {
    let all = CLIENTS.lock().unwrap_or_else(|e| e.into_inner());
    all.get(&link).map(|m| m.iter().map(|(n, e)| (n.clone(), e.clone())).collect()).unwrap_or_default()
}

/// What linked clients say is degraded, one row per name: how many say it,
/// the most failures and the newest error.
pub fn client_degraded() -> Vec<(String, usize, Episode)> {
    let mut by: BTreeMap<String, (usize, Episode)> = BTreeMap::new();
    for (name, e) in CLIENTS.lock().unwrap_or_else(|e| e.into_inner()).values().flatten() {
        let row = by.entry(name.clone()).or_insert((0, Episode { failures: 0, since: e.since, ..e.clone() }));
        row.0 += 1;
        row.1.failures = row.1.failures.max(e.failures);
        row.1.since = row.1.since.min(e.since);
        if !e.last_error.is_empty() {
            row.1.last_error = e.last_error.clone();
        }
    }
    by.into_iter().map(|(n, (c, e))| (n, c, e)).collect()
}

/// What the memory samples say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Memory {
    Ok,
    /// This process holds `[memory] high` percent of the machine or more.
    High,
    /// It kept growing, well past where it started.
    Growing,
    /// It holds `[memory] critical` percent: the machine is about to swap
    /// or kill it.
    Critical,
}

impl Memory {
    pub fn word(self) -> &'static str {
        match self {
            Memory::Ok => "ok",
            Memory::High => "high",
            Memory::Growing => "growing",
            Memory::Critical => "critical",
        }
    }
}

/// Memory over time: pure, so its rules are tested.
#[derive(Debug, Default)]
pub struct Sampler {
    limits: crate::config::Memory,
    base: u64,
    window: VecDeque<u64>,
    high: bool,
    /// Growth past this is said again (a leak is said once per step).
    next_growth: u64,
}

/// Samples in a growth window (100 s at [`EVERY`]).
const WINDOW: usize = 10;

impl Sampler {
    /// A sampler with these thresholds (`[memory]`).
    pub fn with(limits: crate::config::Memory) -> Self {
        Self { limits, ..Self::default() }
    }

    /// One sample: the process's resident memory and the machine's.
    pub fn sample(&mut self, rss: u64, total: u64) -> Memory {
        let l = &self.limits;
        let step = l.growth_mb << 20;
        if self.base == 0 {
            self.base = rss;
            self.next_growth = rss + step.max(rss / 4);
        }
        self.window.push_back(rss);
        if self.window.len() > WINDOW {
            self.window.pop_front();
        }
        // `high` percent of the machine is high; it is clear again below `clear`.
        let pct = if total == 0 { 0.0 } else { rss as f64 * 100.0 / total as f64 };
        self.high = if self.high { pct >= f64::from(l.clear) } else { pct >= f64::from(l.high) };
        let climbing =
            self.window.len() == WINDOW && self.window.iter().zip(self.window.iter().skip(1)).all(|(a, b)| b >= a);
        let grown = climbing && rss > self.next_growth;
        if grown {
            self.next_growth = rss + (total / 10).max(step);
        }
        if pct >= f64::from(l.critical) {
            return Memory::Critical;
        }
        match (self.high, grown) {
            (_, true) => Memory::Growing,
            (true, _) => Memory::High,
            _ => Memory::Ok,
        }
    }
}

/// This process's resident memory and the machine's, in bytes.
#[cfg(windows)]
pub fn memory() -> Option<(u64, u64)> {
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    // SAFETY: plain out-structs, sized as the calls require.
    unsafe {
        let mut pmc: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        pmc.cb = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        let mut ms: MEMORYSTATUSEX = std::mem::zeroed();
        ms.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
        if GetProcessMemoryInfo(GetCurrentProcess(), &mut pmc, pmc.cb) == 0 || GlobalMemoryStatusEx(&mut ms) == 0 {
            return None;
        }
        Some((pmc.WorkingSetSize as u64, ms.ullTotalPhys))
    }
}

/// This process's resident memory and the machine's, in bytes.
#[cfg(target_os = "macos")]
pub fn memory() -> Option<(u64, u64)> {
    // SAFETY: out-params sized as the calls require, for this process.
    unsafe {
        let mut ti: libc::proc_taskinfo = std::mem::zeroed();
        let size = size_of::<libc::proc_taskinfo>() as i32;
        if libc::proc_pidinfo(libc::getpid(), libc::PROC_PIDTASKINFO, 0, (&raw mut ti).cast(), size) != size {
            return None;
        }
        let (mut total, mut len) = (0u64, size_of::<u64>());
        if libc::sysctlbyname(c"hw.memsize".as_ptr(), (&raw mut total).cast(), &mut len, std::ptr::null_mut(), 0) != 0 {
            return None;
        }
        Some((ti.pti_resident_size, total))
    }
}

/// This process's resident memory and the machine's, in bytes.
#[cfg(not(any(windows, target_os = "macos")))]
pub fn memory() -> Option<(u64, u64)> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kb: u64 = meminfo.lines().find(|l| l.starts_with("MemTotal:"))?.split_whitespace().nth(1)?.parse().ok()?;
    Some((pages * page_size(), kb * 1024))
}

#[cfg(not(any(windows, target_os = "macos")))]
fn page_size() -> u64 {
    #[cfg(unix)]
    // SAFETY: sysconf only reads a constant.
    let n = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    #[cfg(not(unix))]
    let n = 4096;
    if n > 0 { n as u64 } else { 4096 }
}

/// What `health.json` says (pure).
pub fn doc(
    about: &crate::report::About,
    (started, now): (i64, i64),
    (rss, total): (u64, u64),
    memory: Memory,
    degraded: &[(String, Episode)],
    clients: &[(String, usize, Episode)],
    gauges: &[Reading],
) -> Value {
    let row = |name: &str, e: &Episode| json!({ "name": name, "failures": e.failures, "since": crate::log::stamp(e.since), "lastError": e.last_error });
    let degraded: Vec<Value> = degraded.iter().map(|(name, e)| row(name, e)).collect();
    let clients: Vec<Value> = clients
        .iter()
        .map(|(name, n, e)| {
            let mut r = row(name, e);
            r["clients"] = (*n).into();
            r
        })
        .collect();
    let level = memory.word();
    let healthy = degraded.is_empty() && clients.is_empty() && memory == Memory::Ok;
    json!({
        "status": if healthy { "healthy" } else { "degraded" },
        "appId": about.id,
        "version": about.version,
        "aio": env!("CARGO_PKG_VERSION"),
        "pid": std::process::id(),
        "startedAt": crate::log::stamp(started),
        "updatedAt": now,
        "uptimeS": (now - started) / 1000,
        "memory": { "level": level, "rssBytes": rss, "machinePct": (rss * 100).checked_div(total).unwrap_or(0) },
        "degraded": degraded,
        "clientDegraded": clients,
        "gauges": gauges,
    })
}

/// `ax health`'s verdict on a `health.json` read at `now`: healthy or why
/// not. `alive`: is its pid still running?
pub fn judge(doc: &Value, now: i64, alive: bool) -> Result<String, String> {
    let id = doc["appId"].as_str().unwrap_or("?");
    let updated = doc["updatedAt"].as_i64().unwrap_or(0);
    if !alive {
        return Err(format!("{id} is not running (last seen {})", crate::log::stamp(updated)));
    }
    match doc["status"].as_str() {
        Some("stopping") => return Err(format!("{id} is stopping (pid {})", doc["pid"])),
        Some("maintenance") => {
            let op = doc["op"].as_str().unwrap_or("?");
            return Err(format!("{id} is held by `ax {op}` (pid {})", doc["pid"]));
        }
        _ => {}
    }
    if now - updated > STALE_MS {
        return Err(format!("{id} is stuck: no health update for {} s", (now - updated) / 1000));
    }
    let mem = &doc["memory"];
    let summary = format!(
        "pid {}, up {} s, {} MB ({}% of RAM)",
        doc["pid"],
        doc["uptimeS"],
        mem["rssBytes"].as_u64().unwrap_or(0) >> 20,
        mem["machinePct"]
    );
    if doc["status"] == "healthy" {
        return Ok(format!("{id} is healthy: {summary}"));
    }
    let mut why: Vec<String> = doc["degraded"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|d| {
            format!(
                "{} ({} failures since {}): {}",
                d["name"].as_str().unwrap_or("?"),
                d["failures"],
                d["since"].as_str().unwrap_or("?"),
                d["lastError"].as_str().unwrap_or("")
            )
        })
        .collect();
    why.extend(doc["clientDegraded"].as_array().into_iter().flatten().map(|d| {
        format!(
            "{} on {} client(s) ({} failures): {}",
            d["name"].as_str().unwrap_or("?"),
            d["clients"],
            d["failures"],
            d["lastError"].as_str().unwrap_or("")
        )
    }));
    if mem["level"] != "ok" {
        why.push(format!("memory is {}", mem["level"].as_str().unwrap_or("?")));
    }
    why.extend(doc["budgets"]["breaches"].as_array().into_iter().flatten().map(|b| {
        let (name, said) = (b["budget"].as_str().unwrap_or("?"), b["said"].as_str().unwrap_or("?"));
        format!("{} over [budgets] {name}: {said}", b["detail"].as_str().unwrap_or("?"))
    }));
    Err(format!("{id} is degraded: {summary}\n  {}", why.join("\n  ")))
}

/// This process's CPU time so far, user and system, in ms (`health.json`'s
/// `cpuMs`: `ax ui` charts its rise).
pub fn cpu_ms() -> Option<u64> {
    #[cfg(unix)]
    {
        // SAFETY: a plain C struct of integers: all zeroes is valid; the call fills it.
        let mut u: libc::rusage = unsafe { std::mem::zeroed() };
        // SAFETY: `u` is the out-struct the call takes, for this process.
        if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) } != 0 {
            return None;
        }
        let ms = |t: libc::timeval| Some(u64::try_from(t.tv_sec).ok()? * 1000 + u64::try_from(t.tv_usec).ok()? / 1000);
        Some(ms(u.ru_utime)? + ms(u.ru_stime)?)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::FILETIME;
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
        let z = || FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut made, mut gone, mut kernel, mut user) = (z(), z(), z(), z());
        // SAFETY: out-structs for this process's own pseudo-handle.
        let ok = unsafe { GetProcessTimes(GetCurrentProcess(), &mut made, &mut gone, &mut kernel, &mut user) };
        // In 100 ns steps.
        let n = |f: FILETIME| ((u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime)) / 10_000;
        (ok != 0).then(|| n(kernel) + n(user))
    }
    #[cfg(not(any(unix, windows)))]
    None
}

/// When this process began watching its health (its first ask).
fn started() -> i64 {
    static AT: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *AT.get_or_init(now_ms)
}

/// The health document now, for `/__aio/health`; `id` names an app that
/// has no reporter (one served without `boot`).
pub fn now(id: &str) -> Value {
    let about = crate::report::about().unwrap_or_else(|| crate::report::About { id: id.into(), ..Default::default() });
    let mem = memory().unwrap_or((0, 0));
    crate::budgets::measure_cells();
    let mut d = doc(&about, (started(), now_ms()), mem, memory_level(), &degraded(), &client_degraded(), &gauges());
    live(&mut d);
    with_budgets(d, crate::budgets::report())
}

/// What `ax ui` reads of the running app on every OS: its CPU time
/// (`cpuMs`) and the config it was started with (`config`).
fn live(d: &mut Value) {
    d["cpuMs"] = cpu_ms().into();
    d["config"] = CONFIG.get().cloned().unwrap_or(Value::Null);
}

/// The live config `health.json` says ([`start`]'s).
static CONFIG: std::sync::OnceLock<Value> = std::sync::OnceLock::new();

/// What of `cfg` `ax ui`'s runtime config card shows (pure): its title,
/// how callers sign in, its mode.
pub fn config_of(cfg: &crate::config::Config) -> Value {
    let title = if cfg.app.name.is_empty() { &cfg.app.id } else { &cfg.app.name };
    json!({
        "title": title,
        "auth": match (cfg.accounts.on, cfg.accounts.sessions) {
            (true, _) => "accounts",
            (false, true) => "sessions",
            _ => "none",
        },
        "mode": serde_json::to_value(cfg.app.mode).unwrap_or(Value::Null),
    })
}

/// `d` with the `[budgets]` verdict (`null`: none declared); a breach
/// degrades it (pure).
pub fn with_budgets(mut d: Value, verdict: Value) -> Value {
    if verdict["ok"] == false {
        d["status"] = "degraded".into();
    }
    d["budgets"] = verdict;
    d
}

/// Sample memory and write `data/health.json` every [`EVERY`], from now
/// until the process ends (`stop` removes the file); `config`, what it
/// says of the app's config ([`config_of`]).
pub fn start(data: &Path, limits: crate::config::Memory, config: Value) {
    let Some(about) = crate::report::about() else { return };
    _ = CONFIG.set(config);
    let file = data.join("health.json");
    *FILE.lock().unwrap_or_else(|e| e.into_inner()) = Some(file.clone());
    let started = started();
    let spawned = std::thread::Builder::new().name("aio-health".into()).spawn(move || {
        let mut sampler = Sampler::with(limits);
        let mut said = Memory::Ok;
        loop {
            let (rss, total) = memory().unwrap_or((0, 0));
            let m = sampler.sample(rss, total);
            if m != said {
                memory_changed(m, rss, total);
            }
            if m != said && m != Memory::Ok {
                let mut what = format!(
                    "{} MB resident, {}% of the machine",
                    rss >> 20,
                    (rss * 100).checked_div(total).unwrap_or(0)
                );
                let big = biggest(&gauges(), 3);
                if !big.is_empty() {
                    what += &format!("; biggest: {big}");
                }
                let cells = crate::report::sizes();
                if !cells.is_empty() {
                    let say: Vec<String> =
                        cells.iter().take(3).map(|(c, n)| format!("{c} {}", crate::fmt::bytes(*n))).collect();
                    what += &format!("; cells: {}", say.join(", "));
                }
                eprintln!("[aio] memory is {}: {what}", m.word());
                crate::report::problem(&format!("memory:{}", m.word()), &what);
            }
            said = m;
            *LEVEL.lock().unwrap_or_else(|e| e.into_inner()) = m;
            crate::budgets::measure_cells();
            let mut d = doc(&about, (started, now_ms()), (rss, total), m, &degraded(), &client_degraded(), &gauges());
            live(&mut d);
            let (snap, loopback) = crate::vitals::snapshot();
            d["vitals"] = crate::vitals::doc(&snap, loopback, &crate::vitals::get());
            // The file only (it is this user's, never served): what runs and where.
            d["exe"] = std::env::current_exe().ok().map(|p| p.display().to_string()).into();
            d["cwd"] = std::env::current_dir().ok().map(|p| p.display().to_string()).into();
            let d = with_budgets(d, crate::budgets::report());
            // Under the lock: never written after `stopping` or `stop`.
            let f = FILE.lock().unwrap_or_else(|e| e.into_inner());
            if f.is_none() || STOPPING.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }
            if let Err(e) = crate::paths::write_atomic(&file, d.to_string().as_bytes(), true) {
                eprintln!("[aio] health.json not written: {e}");
            }
            drop(f);
            std::thread::sleep(limits.every);
        }
    });
    if let Err(e) = spawned {
        eprintln!("[aio] no health monitor: {e}");
    }
}

/// A change of [`Memory`] level, as [`on_memory`] hears it (aio's
/// `onMemoryPressure` report).
#[derive(Clone, Debug, PartialEq)]
pub struct MemoryReport {
    /// What it is now (`Ok`: back under the thresholds).
    pub level: Memory,
    /// This process's resident memory and the machine's, in bytes.
    pub rss: u64,
    pub total: u64,
    /// The largest gauge ([`gauge`]): its name and value.
    pub biggest: Option<(String, u64)>,
    /// The largest cells by state size, bytes.
    pub cells: Vec<(String, u64)>,
}

type MemoryHook = std::sync::Arc<dyn Fn(&MemoryReport) + Send + Sync>;
static MEMORY_HOOK: Mutex<Option<MemoryHook>> = Mutex::new(None);

/// Hear each change of the memory level (`[memory]`, sampled every
/// `[memory] every`), back to `Ok` included: drop a cache, shed load. The
/// log line and the `memory:<level>` problem come as before. Guarded: a
/// hook that panics is logged. Never call a cell from it (hand it off).
pub fn on_memory(f: impl Fn(&MemoryReport) + Send + Sync + 'static) {
    *MEMORY_HOOK.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::sync::Arc::new(f));
}

/// The level changed to `m`: the [`on_memory`] hook hears it, guarded.
pub(crate) fn memory_changed(level: Memory, rss: u64, total: u64) {
    let report = MemoryReport {
        level,
        rss,
        total,
        biggest: gauges().into_iter().filter_map(|g| Some((g.name, g.value?))).max_by_key(|g| g.1),
        cells: crate::report::sizes().into_iter().take(3).map(|(c, n)| (c.to_string(), n)).collect(),
    };
    let hook = MEMORY_HOOK.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(Err(p)) = hook.map(|h| std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| h(&report)))) {
        crate::error!("the on_memory hook panicked: {}", crate::report::panic_text(p.as_ref()));
    }
}

static FILE: Mutex<Option<PathBuf>> = Mutex::new(None);
/// What the memory samples said last.
static LEVEL: Mutex<Memory> = Mutex::new(Memory::Ok);

/// What memory is like now, as the last sample said.
pub fn memory_level() -> Memory {
    *LEVEL.lock().unwrap_or_else(|e| e.into_inner())
}

/// What a served app says on `GET /__aio/metrics`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Metrics {
    pub uptime_s: u64,
    /// This process's resident memory and the machine's, in bytes.
    pub memory: Option<(u64, u64)>,
    /// Open sync links.
    pub clients: usize,
    /// Calls this run (every method call).
    pub calls: u64,
    /// The served cell: (id, enabled, failures).
    pub cell: (String, bool, usize),
    /// Healthy (`aio_up` 1) or degraded.
    pub up: bool,
    /// The degraded parts and their failures in a row.
    pub degraded: Vec<(String, u32)>,
    pub gauges: Vec<Reading>,
}

/// `m` in Prometheus' text format (pure).
pub fn prometheus(m: &Metrics) -> String {
    use std::fmt::Write as _;
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "");
    let mut out = String::new();
    let mut one = |name: &str, kind: &str, help: &str, rows: &[(String, u64)]| {
        _ = writeln!(out, "# HELP {name} {help}\n# TYPE {name} {kind}");
        rows.iter().for_each(|(labels, v)| _ = writeln!(out, "{name}{labels} {v}"));
    };
    let bare = |v: u64| vec![(String::new(), v)];
    one("aio_up", "gauge", "1 when the app is healthy, 0 when degraded", &bare(m.up.into()));
    one("aio_uptime_seconds", "gauge", "Seconds since the server started", &bare(m.uptime_s));
    if let Some((rss, total)) = m.memory {
        one("aio_memory_rss_bytes", "gauge", "Resident set size", &bare(rss));
        one("aio_memory_machine_bytes", "gauge", "The machine's memory", &bare(total));
    }
    one("aio_clients_connected", "gauge", "Open sync links", &bare(m.clients as u64));
    one("aio_calls_total", "counter", "Method calls this run", &bare(m.calls));
    let cell = format!("{{cell=\"{}\"}}", esc(&m.cell.0));
    one("aio_cell_enabled", "gauge", "Cell enabled (1) or tripped (0)", &[(cell.clone(), m.cell.1.into())]);
    one("aio_cell_failures", "gauge", "Failures since the cell was last enabled", &[(cell, m.cell.2 as u64)]);
    one("aio_degraded_parts", "gauge", "Best-effort parts degraded now", &bare(m.degraded.len() as u64));
    if !m.degraded.is_empty() {
        let rows: Vec<_> =
            m.degraded.iter().map(|(n, f)| (format!("{{part=\"{}\"}}", esc(n)), u64::from(*f))).collect();
        one("aio_degraded_failures", "gauge", "Failures in a row of a degraded part", &rows);
    }
    let read: Vec<_> = m.gauges.iter().filter_map(|g| Some((g, g.value?))).collect();
    if !read.is_empty() {
        let unit = |g: &Reading| if g.unit == Unit::Bytes { "bytes" } else { "count" };
        let rows: Vec<_> = read
            .iter()
            .map(|(g, v)| (format!("{{name=\"{}\",unit=\"{}\",counter=\"{}\"}}", esc(&g.name), unit(g), g.counter), *v))
            .collect();
        one("aio_gauge", "gauge", "A named series of the memory ledger", &rows);
        let bounds: Vec<_> =
            read.iter().filter_map(|(g, _)| Some((format!("{{name=\"{}\"}}", esc(&g.name)), g.bound?))).collect();
        if !bounds.is_empty() {
            one("aio_gauge_limit", "gauge", "The ceiling of a budget", &bounds);
        }
    }
    out
}

static STOPPING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The app has begun to stop: `health.json` says `stopping` (`ax status`)
/// until [`stop`] removes it.
pub fn stopping() {
    let f = FILE.lock().unwrap_or_else(|e| e.into_inner());
    STOPPING.store(true, std::sync::atomic::Ordering::Relaxed);
    let Some(file) = f.as_ref() else { return };
    let Some(mut d) = std::fs::read(file).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else { return };
    d["status"] = "stopping".into();
    d["updatedAt"] = now_ms().into();
    _ = crate::paths::write_atomic(file, d.to_string().as_bytes(), true);
}

/// What `ax restore` (holding the app's lock) leaves in `data/health.json`
/// while it works: the app is in maintenance.
pub fn maintenance(id: &str, op: &str) -> Value {
    json!({ "status": "maintenance", "appId": id, "op": op, "pid": std::process::id(), "updatedAt": now_ms() })
}

/// A clean stop: no `health.json` (`ax health` says "not running").
pub fn stop() {
    if let Some(f) = FILE.lock().unwrap_or_else(|e| e.into_inner()).take() {
        _ = std::fs::remove_file(f);
    }
}

/// Watch that `rt` keeps making progress (freeze detection, aio's loop
/// probe): each second, how late its tick is, with the effects waiting,
/// graded by `[vitals]`. A step up to warning or frozen is said in the log
/// with its likely cause, and the way back; a frozen tick is a failure of
/// "runtime", so a frozen app degrades.
pub fn watch(rt: tokio::runtime::Handle) {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicI64, Ordering};
    let last = Arc::new(AtomicI64::new(now_ms()));
    let spawned = std::thread::Builder::new().name("aio-watch".into()).spawn(move || {
        let mut track = crate::vitals::Track::default();
        loop {
            let l = last.clone();
            rt.spawn(async move { l.store(now_ms(), Ordering::Relaxed) });
            std::thread::sleep(Duration::from_secs(1));
            // Its tick was asked for a second ago: how much later it ran.
            let late = (now_ms() - last.load(Ordering::Relaxed) - 1_000).max(0) as u64;
            let t = crate::vitals::get();
            let lp = crate::vitals::probe(late, track.since, &t);
            crate::vitals::looped(&lp);
            if let Some(step) = track.step(lp.layer.status, now_ms()) {
                crate::vitals::stepped("loop", step, &crate::vitals::says(step, &lp, &t));
            }
            if t.event_loop.grade(late) == crate::vitals::Status::Frozen {
                fail("runtime", &format!("no progress for {} ms", late + 1_000));
            } else {
                ok("runtime");
            }
        }
    });
    if let Err(e) = spawned {
        eprintln!("[aio] no freeze watch: {e}");
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1 << 30;

    /// `ax ui`'s runtime config card reads the live app, not its file.
    #[test]
    fn health_says_the_config_the_app_runs_with() {
        let cfg = crate::config::Config::parse("[app]\nid = \"notes\"\nname = \"Notes\"\nmode = \"server\"\n[server]\nlisten = \"127.0.0.1:7420\"\n[accounts]\non = true\n").unwrap();
        assert_eq!(config_of(&cfg), json!({ "title": "Notes", "auth": "accounts", "mode": "server" }));
        let bare = crate::config::Config::parse("[app]\nid = \"notes\"").unwrap();
        assert_eq!(config_of(&bare), json!({ "title": "notes", "auth": "none", "mode": "standalone" }));
        let served = now("notes");
        assert!(served["cpuMs"].is_u64() && served.get("exe").is_none(), "what runs where is the file's alone");
    }

    #[test]
    fn the_memory_hook_hears_each_change_with_a_report_and_survives_its_own_panic() {
        let heard = std::sync::Arc::new(Mutex::new(vec![]));
        let h = heard.clone();
        on_memory(move |r| {
            h.lock().unwrap().push((r.level, r.rss, r.biggest.clone()));
            assert_ne!(r.level, Memory::Critical, "a hook bug");
        });
        memory_changed(Memory::High, 6 * GB, 10 * GB);
        memory_changed(Memory::Critical, 9 * GB, 10 * GB);
        memory_changed(Memory::Ok, GB, 10 * GB);
        let got = heard.lock().unwrap().clone();
        assert_eq!(got.iter().map(|g| (g.0, g.1)).collect::<Vec<_>>(), [(Memory::High, 6 * GB), (Memory::Critical, 9 * GB), (Memory::Ok, GB)]);
        assert_eq!(got[2].2, got[0].2, "the biggest gauge, if any, by name and value");
        *MEMORY_HOOK.lock().unwrap() = None;
    }

    #[test]
    fn a_part_degrades_after_five_failures_in_a_row_and_recovers() {
        let name = "test:sync";
        for _ in 0..AFTER - 1 {
            fail(name, "refused");
        }
        assert!(degraded().iter().all(|(n, _)| n != name), "4 in a row is not yet degraded");
        fail(name, "refused: token=abc");
        let (_, e) = degraded().into_iter().find(|(n, _)| n == name).unwrap();
        assert_eq!((e.failures, e.last_error.as_str()), (AFTER, "refused: token=…"));
        ok(name);
        assert!(degraded().iter().all(|(n, _)| n != name));
        degrade(name, "crashed");
        assert_eq!(degraded().into_iter().find(|(n, _)| n == name).unwrap().1.failures, AFTER, "at once");
        ok(name);
    }

    #[test]
    fn a_part_guards_best_effort_work_with_its_own_threshold() {
        let open = part("test:open").after(2);
        assert_eq!(open.guard(Ok::<_, String>(42)), Some(42), "a success passes the value through");
        assert_eq!(open.guard(Err::<u8, _>("no file")), None, "a failure is None: the caller goes on");
        assert_eq!((open.failures(), open.is_degraded()), (1, false));
        assert_eq!(open.guard(Ok::<_, String>(1)), Some(1));
        assert_eq!(open.failures(), 0, "failures in a row: a success in between resets them");
        open.fail("no file");
        open.fail("no file");
        assert!(open.is_degraded(), "after(2): two in a row");
        let (_, e) = degraded().into_iter().find(|(n, _)| n == "test:open").unwrap();
        assert_eq!((e.failures, e.last_error.as_str()), (2, "no file"));
        // The same name is the same part, with the threshold it was made with.
        let other = part("test:open").after(9);
        assert!(other.is_degraded() && other == open);
        other.ok();
        assert!(!open.is_degraded() && degraded().iter().all(|(n, _)| n != "test:open"));
        assert_eq!(part(&"x".repeat(100)), part(&"x".repeat(64)), "names are capped");
        assert_eq!(part("test:fresh").failures(), 0);
    }

    #[test]
    fn gauges_name_what_grew_and_budgets_stop_a_runaway_by_name() {
        let find = |n: &str| gauges().into_iter().find(|g| g.name == n);
        gauge("test.queue", Unit::Bytes, || 120 << 20);
        gauge("test.queue", Unit::Count, || 1);
        let g = find("test.queue").unwrap();
        assert_eq!(
            (g.unit, g.counter, g.value),
            (Unit::Bytes, false, Some(120 << 20)),
            "the first owner keeps the name"
        );
        gauge("test.broken", Unit::Bytes, || panic!("mid-teardown"));
        assert_eq!(find("test.broken").unwrap().value, None, "a failing reader reads unknown, never a crash");
        let max = std::num::NonZeroU64::new(3).unwrap();
        let b = budget("test.replay", max);
        b.spend(2).unwrap();
        assert_eq!(find("test.replay").unwrap().bound, Some(3));
        let e = budget("test.replay", std::num::NonZeroU64::new(99).unwrap()).spend(2).unwrap_err();
        assert!(e.why.starts_with("MEMORY_UNBOUNDED: test.replay passed its ceiling: 4 > 3 (+2)"), "one counter: {e}");
        b.reset();
        assert_eq!((b.value(), b.max()), (0, 3), "reset rewinds the counter, not the ceiling");
        let all = [
            Reading { name: "a".into(), unit: Unit::Bytes, counter: false, value: Some(3 << 20), bound: None },
            Reading { name: "n".into(), unit: Unit::Count, counter: false, value: Some(1 << 40), bound: None },
            Reading { name: "b".into(), unit: Unit::Bytes, counter: false, value: Some(120 << 20), bound: None },
            Reading { name: "x".into(), unit: Unit::Bytes, counter: false, value: None, bound: None },
        ];
        assert_eq!(biggest(&all, 3), "b 120.0 MB, a 3.0 MB", "sizes only, biggest first");
        let about = crate::report::About::default();
        let d = doc(&about, (0, 0), (GB, 16 * GB), Memory::Ok, &[], &[], &all[..1]);
        assert_eq!(d["gauges"][0]["name"], "a");
        // Past the cap a new name is ignored, said once in the log.
        for i in 0..GAUGES {
            gauge(&format!("cap.{i}"), Unit::Count, || 0);
        }
        assert_eq!(gauges().len(), GAUGES);
        assert!(find("test.queue").is_some(), "never evicts what it watches");
    }

    #[test]
    fn the_cpu_time_so_far_is_read_and_rises_with_work() {
        let before = cpu_ms().expect("this OS says the process's CPU time");
        let t = std::time::Instant::now();
        let mut n = 0u64;
        while cpu_ms().unwrap() < before + 30 {
            assert!(t.elapsed() < Duration::from_secs(10), "work never showed as CPU time");
            for _ in 0..100_000 {
                n = std::hint::black_box(n.wrapping_add(1));
            }
        }
    }

    #[test]
    fn metrics_read_as_prometheus_text() {
        let q = Reading { name: "sync.\"q\"".into(), unit: Unit::Bytes, counter: false, value: Some(7), bound: None };
        let b = Reading { name: "replay".into(), unit: Unit::Count, counter: true, value: Some(2), bound: Some(9) };
        let gone = Reading { value: None, ..q.clone() };
        let m = Metrics {
            uptime_s: 61,
            memory: Some((300, 1000)),
            clients: 2,
            calls: 40,
            cell: ("notes".into(), false, 3),
            up: false,
            degraded: vec![("sync".into(), 5)],
            gauges: vec![q, b, gone],
        };
        let t = prometheus(&m);
        for l in [
            "# HELP aio_uptime_seconds Seconds since the server started\n# TYPE aio_uptime_seconds gauge\naio_uptime_seconds 61\n",
            "aio_memory_rss_bytes 300\n",
            "aio_clients_connected 2\n",
            "# TYPE aio_calls_total counter\naio_calls_total 40\n",
            "aio_cell_enabled{cell=\"notes\"} 0\n",
            "aio_cell_failures{cell=\"notes\"} 3\n",
            "# TYPE aio_up gauge\naio_up 0\n",
            "aio_degraded_parts 1\n",
            "aio_degraded_failures{part=\"sync\"} 5\n",
            "aio_gauge{name=\"sync.\\\"q\\\"\",unit=\"bytes\",counter=\"false\"} 7\n",
            "aio_gauge_limit{name=\"replay\"} 9\n",
        ] {
            assert!(t.contains(l), "{l:?} not in:\n{t}");
        }
        assert_eq!(t.matches("aio_gauge{").count(), 2, "an unreadable gauge is left out");
        assert!(!prometheus(&Metrics::default()).contains("aio_memory"), "no reading, no line");
        assert!(prometheus(&Metrics { up: true, ..Metrics::default() }).contains("aio_up 1\n"));
        // A label is a label: quotes and line breaks can't make a new series.
        let odd = prometheus(&Metrics { degraded: vec![("a\"} 9\nx{\"".into(), 5)], ..Metrics::default() });
        assert!(odd.contains("aio_degraded_failures{part=\"a\\\"} 9x{\\\"\"} 5\n"), "{odd}");
    }

    #[test]
    fn memory_is_high_at_half_the_machine_and_growth_is_said_once_per_step() {
        let mut s = Sampler::default();
        assert_eq!(s.sample(GB, 16 * GB), Memory::Ok);
        assert_eq!(s.sample(8 * GB, 16 * GB), Memory::High);
        assert_eq!(s.sample(7 * GB, 16 * GB), Memory::High, "clear only below 40%");
        assert_eq!(s.sample(6 * GB, 16 * GB), Memory::Ok);
        assert_eq!(s.sample(14 * GB, 16 * GB), Memory::Critical, "80% and over: critical, whatever else");
        // Thresholds are config: a box that may use more says so later.
        let lax = crate::config::Memory { high: 70, clear: 60, critical: 95, growth_mb: 1024, ..Default::default() };
        let mut s = Sampler::with(lax);
        assert_eq!((s.sample(9 * GB, 16 * GB), s.sample(12 * GB, 16 * GB)), (Memory::Ok, Memory::High));
        assert_eq!(s.sample(10 * GB, 16 * GB), Memory::High, "clear only below 60%");
        assert_eq!(s.sample(9 * GB, 16 * GB), Memory::Ok);
        let mut s = Sampler::with(lax);
        let seen: Vec<_> = (0..12).map(|i| s.sample(GB + i * (64 << 20), 64 * GB)).collect();
        assert!(seen.iter().all(|m| *m == Memory::Ok), "768 MB of growth is under a 1 GB step");
        // Steady growth over a full window, past 256 MB: growing, once.
        let mut s = Sampler::default();
        let seen: Vec<_> = (0..12).map(|i| s.sample(GB + i * (64 << 20), 64 * GB)).collect();
        assert_eq!(seen.iter().filter(|m| **m == Memory::Growing).count(), 1, "{seen:?}");
        // Up and down is not a leak.
        let mut s = Sampler::default();
        assert!((0..30).all(|i| s.sample(GB + (i % 2) * GB, 64 * GB) == Memory::Ok));
    }

    #[test]
    fn health_says_why_an_app_is_not_healthy() {
        let about = crate::report::About { id: "notes".into(), version: "1".into(), ..Default::default() };
        let d = doc(&about, (0, 20_000), (300 << 20, 16 * GB), Memory::Ok, &[], &[], &[]);
        let ok = judge(&d, 25_000, true).unwrap();
        assert!(ok.starts_with("notes is healthy: pid ") && ok.contains("300 MB"), "{ok}");
        assert!(judge(&d, 25_000, false).unwrap_err().contains("not running"));
        assert!(judge(&d, 20_000 + STALE_MS + 1, true).unwrap_err().contains("stuck"));
        let high = doc(&about, (0, 20_000), (9 * GB, 16 * GB), Memory::High, &[], &[], &[]);
        assert!(judge(&high, 20_000, true).unwrap_err().contains("memory is high"));
        let e = Episode { failures: AFTER, since: 0, last_error: "refused".into() };
        let sick = doc(&about, (0, 20_000), (GB, 16 * GB), Memory::Ok, &[("sync".into(), e)], &[], &[]);
        let why = judge(&sick, 20_000, true).unwrap_err();
        assert!(why.contains("sync (5 failures since 1970-01-01T00:00:00.000Z): refused"), "{why}");
        // Stopping, or held for a restore: said, however old the file.
        let mut stopping = d.clone();
        stopping["status"] = "stopping".into();
        assert!(judge(&stopping, 20_000 + STALE_MS * 9, true).unwrap_err().contains("notes is stopping (pid "));
        let held = maintenance("notes", "restore");
        assert!(judge(&held, now_ms() + STALE_MS * 9, true).unwrap_err().contains("held by `ax restore`"));
    }

    #[test]
    fn what_clients_say_is_degraded_shows_in_health_while_they_are_linked() {
        let row = |name: &str, f: i64, since: i64, err: &str| json!({ "name": name, "failures": f, "since": since, "lastError": err });
        let future = now_ms() + 86_400_000;
        let mine = |n: &str| client_degraded().into_iter().find(|(name, ..)| name == n);
        let mine_of = |link: u64| -> Vec<(String, String)> {
            let all = CLIENTS.lock().unwrap();
            all[&link].iter().map(|(n, e)| (n.clone(), e.last_error.clone())).collect()
        };
        client_says(9001, "ann", &json!([row("t:storage", 7, 1000, "quota: token=abc"), { "name": "" }, 5]));
        assert_eq!(mine_of(9001), vec![("t:storage".to_string(), "quota: token=…".to_string())], "redacted");
        client_says(9002, "bob", &json!([row("t:storage", 999, future, &"x".repeat(500)), row("t:render", -3, 0, "")]));
        let (_, n, e) = mine("t:storage").unwrap();
        // Two say it; bob's clock is not taken as fact, and every value is capped.
        assert_eq!((n, e.failures, e.since), (2, 999, 1000));
        assert_eq!(e.last_error.chars().count(), 200);
        assert_eq!(mine("t:render").unwrap().2.failures, 0, "a negative count is no count");
        // Capped names; the whole list each time: an empty one is all recovered.
        client_says(9003, "eve", &json!((0..40).map(|i| row(&format!("t:x{i}"), 1, 1, "")).collect::<Vec<_>>()));
        assert_eq!(client_degraded().iter().filter(|(n, ..)| n.starts_with("t:x")).count(), CLIENT_NAMES);
        client_says(9003, "eve", &json!([]));
        client_says(9001, "ann", &json!([row("t:render", 1, 1, "")]));
        assert_eq!(mine("t:storage").unwrap().1, 1);
        client_gone(9002);
        assert!(mine("t:storage").is_none() && mine("t:x0").is_none());
        let about = crate::report::About { id: "notes".into(), ..Default::default() };
        let d = doc(&about, (0, 20_000), (GB, 16 * GB), Memory::Ok, &[], &[mine("t:render").unwrap()], &[]);
        let why = judge(&d, 20_000, true).unwrap_err();
        assert!(why.contains("t:render on 1 client(s) (1 failures)"), "{why}");
        assert_eq!(d["clientDegraded"][0]["clients"], 1);
        client_gone(9001);
        assert!(mine("t:render").is_none());
    }
}

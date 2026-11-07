//! Vitals (aio's `vitals`): how far behind each layer is, graded against the
//! `[vitals]` thresholds of `aio.toml` into a [`Status`], and when one is
//! in trouble a [`Hint`] naming the likely cause. Layers: `loop` (how late
//! the runtime's tick is: a call or an effect step holding it), `queue`
//! (effects waiting their turn), `transport` (a client unheard), `render`
//! (a client's paint). The server probes `loop` and `queue` itself
//! (`health::watch`, every second): a step up to warning or frozen is said
//! in the log with its hint, frozen fails the `runtime` part, and the way
//! back is said as recovered. The grading and the hints are pure.
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};

pub use aio_config::vitals::{Status, Tier, Vitals};

/// A client's paint, as it reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct Render {
    pub status: Status,
    pub measured: u64,
    /// The call it made last before it froze (`cell.method`).
    pub last_call: Option<String>,
    /// Its window is showing: a hidden one is not frozen, only throttled.
    pub visible: bool,
    /// Freezes in the last 30 s.
    pub freezes: u32,
}

/// A layer graded over time.
#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub status: Status,
    pub measured: u64,
    /// When its current stretch of trouble began (ms since the epoch).
    pub since: Option<i64>,
}

/// The server's runtime.
#[derive(Clone, Debug, PartialEq)]
pub struct Loop {
    pub layer: Layer,
    /// Effects waiting their turn.
    pub queue: u64,
    /// Effects that got their turn in the last second (aio's `drainRate`).
    pub drained: u64,
    /// The newest call: `cell.method`, and how long it took (ms).
    pub last: Option<(String, u64)>,
}

/// Every layer at one moment.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub render: Render,
    pub transport: Layer,
    pub event_loop: Loop,
}

impl Default for Snapshot {
    fn default() -> Self {
        let healthy = Layer { status: Status::Healthy, measured: 0, since: None };
        Self {
            render: Render { status: Status::Healthy, measured: 0, last_call: None, visible: true, freezes: 0 },
            transport: healthy.clone(),
            event_loop: Loop { layer: healthy, queue: 0, drained: 0, last: None },
        }
    }
}

/// How sure a hint is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// Two probes agree, one of them measured it.
    Likely,
    /// Measured by one probe.
    Possible,
    /// Inferred.
    Speculative,
}

/// The likely cause of trouble, what shows it, and what to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hint {
    pub cause: String,
    pub evidence: Vec<String>,
    pub fix: String,
    pub severity: Severity,
}

/// How sure: from how many probes saw it, whether one measured it, and
/// whether they agree (pure).
pub fn severity(probes: u32, measured: bool, agree: bool) -> Severity {
    match (probes, measured, agree) {
        (2.., true, true) => Severity::Likely,
        (1.., true, _) => Severity::Possible,
        _ => Severity::Speculative,
    }
}

/// The likely cause of what `s` shows, first rule that fits (pure; aio's
/// hint engine): a hidden window is no freeze; freezes again and again;
/// a slow call froze the paint; the effect queue backed up first; the
/// link stalled with both ends healthy (on `loopback`: the client's own
/// main thread); the client froze on its own code. None: nothing to say.
pub fn hint(s: &Snapshot, t: &Vitals, loopback: bool) -> Option<Hint> {
    let (r, tr, lp) = (&s.render, &s.transport, &s.event_loop);
    if r.status == Status::Frozen && !r.visible {
        return None;
    }
    if r.status == Status::Recovered && r.freezes > 1 {
        return Some(Hint {
            cause: format!("Repeated freeze-recover cycle ({} times)", r.freezes),
            evidence: vec![format!("{} freezes in the last 30 s", r.freezes)],
            fix: "Find what runs on each recovery (a reload, a full resync, a heavy handler) and make it cheaper."
                .into(),
            severity: Severity::Likely,
        });
    }
    if let Some((call, ms)) = &lp.last
        && r.status == Status::Frozen
        && *ms > t.event_loop.degraded
        && r.last_call.as_ref() == Some(call)
    {
        return Some(Hint {
            cause: format!("The call {call} took {ms} ms"),
            evidence: vec![
                format!("{call} took {ms} ms (budget: {} ms)", t.event_loop.degraded),
                format!("render frozen for {} ms", r.measured),
            ],
            fix: "Make the method cheaper, or move the slow part into an effect.".into(),
            severity: severity(2, true, true),
        });
    }
    if let Some(at) = lp.layer.since
        && lp.queue > t.queue.frozen
        && tr.since.is_none_or(|tr| at < tr)
    {
        let top = lp.last.as_ref().map_or("none".into(), |(c, _)| c.clone());
        return Some(Hint {
            cause: format!("The effect queue backed up to {} waiting", lp.queue),
            evidence: vec![
                format!("queue: {} (threshold: {})", lp.queue, t.queue.frozen),
                format!("newest call: {top}"),
            ],
            fix: "Debounce rapid calls (cx.after with one key), or use Mode::Newest for effects only the last matters."
                .into(),
            severity: severity(2, true, true),
        });
    }
    if tr.status == Status::Frozen && r.status == Status::Healthy && lp.layer.status == Status::Healthy {
        let evidence = format!("transport frozen: {} ms (threshold: {} ms)", tr.measured, t.transport.frozen);
        return Some(match loopback {
            true => Hint {
                cause: format!(
                    "The client did not ping in {} ms: its main thread was busy (loopback, not the network)",
                    tr.measured
                ),
                evidence: vec![evidence, "client on loopback".into()],
                fix: "Profile that client: a long render or script blocks its heartbeat.".into(),
                severity: severity(1, true, false),
            },
            false => Hint {
                cause: format!("The network link stalled: no answer in {} ms", tr.measured),
                evidence: vec![evidence],
                fix: "Check the network; the client reconnects on its own.".into(),
                severity: severity(1, true, false),
            },
        });
    }
    if r.status == Status::Frozen
        && tr.status == Status::Healthy
        && lp.layer.status == Status::Healthy
        && r.last_call.is_none()
    {
        return Some(Hint {
            cause: "The client's main thread was blocked by code that is not aio's".into(),
            evidence: vec![format!("render frozen: {} ms", r.measured), "no ax call before the freeze".into()],
            fix: "Check third-party scripts, large DOM work and synchronous I/O in the page.".into(),
            severity: Severity::Possible,
        });
    }
    None
}

/// One layer's grade as it moves: says when it steps up to warning or
/// frozen, and when it comes back from there.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub worst: Option<Status>,
    pub since: Option<i64>,
}

impl Track {
    /// The layer is `now` at `at`: the step worth saying, if any (pure).
    pub fn step(&mut self, now: Status, at: i64) -> Option<Status> {
        if now == Status::Healthy {
            self.since = None;
            return self.worst.take().filter(|w| *w >= Status::Warning).map(|_| Status::Recovered);
        }
        self.since.get_or_insert(at);
        let up = self.worst.is_none_or(|w| now > w);
        if up {
            self.worst = Some(now);
        }
        (up && now >= Status::Warning).then_some(now)
    }
}

/// Every layer as last measured, for the hints and `/__aio/vitals`: the
/// loop (each second), the worst link's transport (each pong) and its
/// address on loopback, the window's paint (each step its probe says).
struct Now {
    snap: Snapshot,
    loopback: bool,
    /// The link the transport layer is, by its listed id.
    link: Option<u64>,
    /// When the paint froze, the last 30 s.
    freezes: std::collections::VecDeque<i64>,
}

static NOW: Mutex<Option<Now>> = Mutex::new(None);

fn now<R>(f: impl FnOnce(&mut Now) -> R) -> R {
    let mut g = NOW.lock().unwrap_or_else(|e| e.into_inner());
    let n = g.get_or_insert_with(|| Now { snap: Snapshot::default(), loopback: false, link: None, freezes: Default::default() });
    f(n)
}

/// Every layer now, and whether the transport's link is on loopback.
pub fn snapshot() -> (Snapshot, bool) {
    now(|n| (n.snap.clone(), n.loopback))
}

/// The loop as the watch measured it.
pub(crate) fn looped(lp: &Loop) {
    now(|n| n.snap.event_loop = lp.clone());
}

/// Link `id` (on `loopback` or not) graded `layer` at its last pong: the
/// transport layer is the worst link's.
pub(crate) fn linked(id: u64, loopback: bool, layer: Layer) {
    now(|n| {
        if n.link == Some(id) || n.link.is_none() || layer.status >= n.snap.transport.status {
            (n.snap.transport, n.loopback, n.link) = (layer, loopback, Some(id));
        }
    });
}

/// Link `id` is gone: the transport layer is healthy again if it was that link's.
pub(crate) fn unlinked(id: u64) {
    now(|n| {
        if n.link == Some(id) {
            (n.snap.transport, n.link) = (Snapshot::default().transport, None);
        }
    });
}

/// The window's render probe stepped to `status` at `ms` (its `[vitals]
/// render` line, heard in the log): the paint layer, with the call made
/// last and the freezes of the last 30 s.
pub fn rendered(status: Status, ms: u64, at: i64) -> Snapshot {
    let last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let last_call = (!last.0.is_empty()).then(|| format!("{}.{}", last.0, last.1));
    drop(last);
    now(|n| {
        n.freezes.retain(|t| at - t < 30_000);
        if status == Status::Frozen {
            n.freezes.push_back(at);
        }
        let freezes = n.freezes.len() as u32;
        n.snap.render = Render { status, measured: ms, last_call, visible: true, freezes };
        n.snap.clone()
    })
}

/// The render probe's step in a log line (pure): `[page] [vitals] render
/// frozen: an update took 3200 ms…` is `(Frozen, 3200)`.
pub fn render_step(line: &str) -> Option<(Status, u64)> {
    let rest = line.split_once("[page] [vitals] render ")?.1;
    let (word, rest) = rest.split_once(':')?;
    let status = match word {
        "warning" => Status::Warning,
        "frozen" => Status::Frozen,
        "recovered" => Status::Recovered,
        _ => return None,
    };
    let ms = rest.split(" ms").next()?.rsplit(' ').next()?.parse().ok()?;
    Some((status, ms))
}

/// A layer stepped (`line` says how): said in the log with the likely
/// cause from every layer at once (`hints`), and on the bus as
/// `vitals:<layer>` (a warning; the way back is info).
pub(crate) fn stepped(layer: &str, step: Status, line: &str) {
    let t = get();
    let (snap, loopback) = snapshot();
    let likely = hint(&snap, &t, loopback).filter(|_| t.hints && step != Status::Recovered).map(|h| h.cause);
    let line = match likely {
        Some(h) if !line.contains("· likely:") => format!("{line} · likely: {h}"),
        _ => line.to_string(),
    };
    let severity = match step {
        Status::Recovered => crate::log::Level::Info,
        _ => crate::log::Level::Warn,
    };
    match severity {
        crate::log::Level::Info => crate::info!(cat: "vitals", "{line}"),
        _ => crate::warn!(cat: "vitals", "{line}"),
    }
    crate::report::say(&format!("vitals:{layer}"), &line, severity);
}

/// The render probe's step, heard: the paint layer, on the bus as
/// `vitals:render`; the page said its line, so the log gets one only when
/// the other layers name a likely cause.
pub(crate) fn render_heard(status: Status, ms: u64) {
    let snap = rendered(status, ms, crate::store::now_ms());
    let t = get();
    let word = format!("{status:?}").to_lowercase();
    let mut line = format!("render {word}: {ms} ms to paint");
    let loopback = snapshot().1;
    if let Some(h) = hint(&snap, &t, loopback).filter(|_| t.hints && status != Status::Recovered) {
        line += &format!(" · likely: {}", h.cause);
        crate::warn!(cat: "vitals", "{line}");
    }
    let severity = if status == Status::Recovered { crate::log::Level::Info } else { crate::log::Level::Warn };
    crate::report::say("vitals:render", &line, severity);
}

/// `GET /__aio/vitals` and `ax vitals` (aio's vitals endpoint; pure): each
/// layer's grade and measure, the likely cause now, and the thresholds.
pub fn doc(snap: &Snapshot, loopback: bool, t: &Vitals) -> serde_json::Value {
    use serde_json::json;
    let word = |s: Status| format!("{s:?}").to_lowercase();
    let layer = |l: &Layer| json!({ "status": word(l.status), "measured": l.measured, "since": l.since });
    let tier = |t: &Tier| json!({ "degraded": t.degraded, "warning": t.warning, "frozen": t.frozen });
    let r = &snap.render;
    let h = hint(snap, t, loopback);
    let mut transport = layer(&snap.transport);
    transport["loopback"] = loopback.into();
    json!({
        "loop": {
            "status": word(snap.event_loop.layer.status),
            "lateMs": snap.event_loop.layer.measured,
            "since": snap.event_loop.layer.since,
            "queue": snap.event_loop.queue,
            "drainRate": snap.event_loop.drained,
            "lastCall": snap.event_loop.last.as_ref().map(|(c, ms)| json!({ "call": c, "ms": ms })),
        },
        "transport": transport,
        "render": { "status": word(r.status), "measured": r.measured, "lastCall": r.last_call, "freezes": r.freezes },
        "hint": h.map(|h| json!({
            "cause": h.cause,
            "evidence": h.evidence,
            "fix": h.fix,
            "severity": format!("{:?}", h.severity).to_lowercase(),
        })),
        "thresholds": {
            "render": tier(&t.render),
            "transport": tier(&t.transport),
            "loop": tier(&t.event_loop),
            "queue": tier(&t.queue),
        },
    })
}

/// A client link as the server sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub id: u64,
    pub cell: String,
    /// ms since it was last heard (a ping's answer comes every 2 s).
    pub gap: u64,
    /// The bytes of the last state or patch it was sent, and of all.
    pub last_bytes: u64,
    pub total_bytes: u64,
}

/// What a server adds to [`doc`] (pure): each link's liveness (`frozen`
/// past `[vitals] transport` frozen, with `frozenFor`) and payloads
/// (aio's `clients` and `payloadStats`), each served cell's state size
/// (`cellSizes`), and the cells tripped (`breakers`).
pub fn served(links: &[Link], cells: &[(&str, usize, bool)], t: &Vitals) -> serde_json::Value {
    use serde_json::json;
    let clients: Vec<_> = links
        .iter()
        .map(|l| {
            let frozen = l.gap > t.transport.frozen;
            let mut row = json!({
                "id": l.id,
                "cell": l.cell,
                "status": if frozen { "frozen" } else { "healthy" },
                "gap": l.gap,
                "lastPayloadBytes": l.last_bytes,
                "totalBytes": l.total_bytes,
            });
            if frozen {
                row["frozenFor"] = l.gap.into();
            }
            row
        })
        .collect();
    let sizes: serde_json::Map<_, _> = cells.iter().map(|(id, n, _)| (id.to_string(), (*n).into())).collect();
    let breakers: Vec<_> = cells.iter().filter(|(_, _, on)| !on).map(|(id, ..)| *id).collect();
    json!({ "clients": clients, "cellSizes": sizes, "breakers": breakers })
}

static SET: RwLock<Option<Vitals>> = RwLock::new(None);
static QUEUED: AtomicU64 = AtomicU64::new(0);
/// Effects that got their turn this run, and the count at the last [`probe`].
static DRAINED: AtomicU64 = AtomicU64::new(0);
static PROBED: AtomicU64 = AtomicU64::new(0);
/// The newest call: its cell, method and own time (ms), kept in place.
static LAST: Mutex<(&str, String, u64)> = Mutex::new(("", String::new(), 0));

/// The app's `[vitals]` (at boot).
pub fn set(v: Vitals) {
    *SET.write().unwrap_or_else(|e| e.into_inner()) = Some(v);
}

/// The `[vitals]` in force.
pub fn get() -> Vitals {
    SET.read().unwrap_or_else(|e| e.into_inner()).unwrap_or_default()
}

/// An effect waiting for its turn (the one before it, same key), until dropped.
pub(crate) struct Waiting;

impl Waiting {
    pub(crate) fn new() -> Self {
        QUEUED.fetch_add(1, Ordering::Relaxed);
        Waiting
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        QUEUED.fetch_sub(1, Ordering::Relaxed);
        DRAINED.fetch_add(1, Ordering::Relaxed);
    }
}

/// A call ran: its cell, method and own time.
pub(crate) fn ran(cell: &'static str, method: &str, took: std::time::Duration) {
    let mut l = LAST.lock().unwrap_or_else(|e| e.into_inner());
    l.0 = cell;
    l.1.clear();
    l.1.push_str(method);
    l.2 = took.as_millis() as u64;
}

/// The server's loop and queue as they are, `late` ms behind, and the
/// effects that got their turn since the probe before (a second ago).
pub fn probe(late: u64, since: Option<i64>, t: &Vitals) -> Loop {
    let queue = QUEUED.load(Ordering::Relaxed);
    let total = DRAINED.load(Ordering::Relaxed);
    let drained = total.saturating_sub(PROBED.swap(total, Ordering::Relaxed));
    let status = t.event_loop.grade(late).max(t.queue.grade(queue));
    let last = {
        let l = LAST.lock().unwrap_or_else(|e| e.into_inner());
        (!l.0.is_empty()).then(|| (format!("{}.{}", l.0, l.1), l.2))
    };
    Loop { layer: Layer { status, measured: late, since }, queue, drained, last }
}

/// The log line for the loop stepping to `step` (pure): what, how much,
/// and with `hints`, the likely cause.
pub fn says(step: Status, lp: &Loop, t: &Vitals) -> String {
    let what = match step {
        Status::Recovered => return "loop recovered".into(),
        s => format!("{s:?}").to_lowercase(),
    };
    let mut line = format!("loop {what}: {} ms late, {} effects waiting", lp.layer.measured, lp.queue);
    let (snap, loopback) = snapshot();
    let snap = Snapshot { event_loop: lp.clone(), ..snap };
    let slow = lp.last.as_ref().filter(|(_, ms)| *ms > t.event_loop.degraded);
    let h = hint(&snap, t, loopback).map(|h| h.cause).or_else(|| slow.map(|(c, ms)| format!("the call {c} took {ms} ms")));
    if let Some(h) = h.filter(|_| t.hints) {
        line += &format!(" · likely: {h}");
    }
    line
}

/// The log line for client `who`'s link stepping to `step` at a round
/// trip of `rtt` ms (pure).
pub fn transport_says(step: Status, who: &str, rtt: u64) -> String {
    match step {
        Status::Recovered => format!("transport recovered: {who}, round trip {rtt} ms"),
        s => format!("transport {}: {who}, round trip {rtt} ms", format!("{s:?}").to_lowercase()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_links_round_trip_is_graded_and_said_on_each_step() {
        let (t, mut track) = (t().transport, Track::default());
        let mut at = 0;
        let mut step = |rtt: u64| {
            at += 1;
            track.step(t.grade(rtt), at).map(|s| transport_says(s, "ana from 10.0.0.2", rtt))
        };
        assert_eq!(step(40), None);
        assert_eq!(step(150), None, "degraded is not said");
        assert_eq!(step(600).as_deref(), Some("transport warning: ana from 10.0.0.2, round trip 600 ms"));
        assert_eq!(step(700), None, "said once");
        assert_eq!(step(2500).as_deref(), Some("transport frozen: ana from 10.0.0.2, round trip 2500 ms"));
        assert_eq!(step(30).as_deref(), Some("transport recovered: ana from 10.0.0.2, round trip 30 ms"));
    }

    fn t() -> Vitals {
        Vitals::default()
    }

    fn frozen_render(last: Option<&str>) -> Render {
        Render { status: Status::Frozen, measured: 3200, last_call: last.map(Into::into), visible: true, freezes: 0 }
    }

    #[test]
    fn each_layer_grades_by_its_thresholds() {
        let q = t().queue;
        assert_eq!(
            (q.grade(49), q.grade(50), q.grade(200), q.grade(1000)),
            (Status::Healthy, Status::Degraded, Status::Warning, Status::Frozen)
        );
        assert_eq!(t().event_loop.grade(2500), Status::Frozen);
        assert_eq!(t().check(), Ok(()));
        let bad = Vitals { render: Tier { degraded: 300, warning: 200, frozen: 2000 }, ..t() };
        assert_eq!(bad.check().unwrap_err(), "aio.toml: [vitals.render] needs degraded < warning < frozen");
        let v: Vitals = toml::from_str("hints = false\n[loop]\ndegraded = 10\nwarning = 20\nfrozen = 30").unwrap();
        assert_eq!((v.hints, v.event_loop.frozen, v.queue), (false, 30, t().queue), "the rest stay aio's");
        let cfg = |more: &str| crate::config::Config::parse(&format!("[app]\nid = \"t\"\n{more}"));
        assert_eq!(cfg("[vitals.queue]\ndegraded = 1\nwarning = 2\nfrozen = 3").unwrap().vitals.queue.frozen, 3);
        assert!(cfg("[vitals.queue]\ndegraded = 3\nwarning = 2\nfrozen = 1").unwrap_err().contains("[vitals.queue]"));
        // The server's own probe grades how late its tick is.
        assert_eq!(probe(2500, Some(7), &t()).layer, Layer { status: Status::Frozen, measured: 2500, since: Some(7) });
    }

    #[test]
    fn a_slow_call_that_froze_the_paint_is_named() {
        let s = Snapshot {
            render: frozen_render(Some("orders.recalc")),
            event_loop: Loop {
                layer: Layer { status: Status::Warning, measured: 600, since: Some(50) },
                queue: 0,
                drained: 0,
                last: Some(("orders.recalc".into(), 3100)),
            },
            ..Snapshot::default()
        };
        let h = hint(&s, &t(), false).unwrap();
        assert_eq!((h.severity, h.cause.as_str()), (Severity::Likely, "The call orders.recalc took 3100 ms"));
    }

    #[test]
    fn a_queue_that_backed_up_first_is_named() {
        let s = Snapshot {
            transport: Layer { status: Status::Degraded, measured: 150, since: Some(200) },
            event_loop: Loop {
                layer: Layer { status: Status::Frozen, measured: 0, since: Some(50) },
                queue: 1500,
                drained: 0,
                last: Some(("market.tick".into(), 1)),
            },
            ..Snapshot::default()
        };
        let h = hint(&s, &t(), false).unwrap();
        assert!(h.cause.contains("queue backed up to 1500") && h.evidence[1].contains("market.tick"), "{h:?}");
        // The link went bad first: the queue is not the cause.
        let late = Snapshot { transport: Layer { since: Some(10), ..s.transport.clone() }, ..s };
        assert_eq!(hint(&late, &t(), false), None);
    }

    #[test]
    fn a_stalled_link_with_both_ends_healthy_is_the_network_or_on_loopback_the_client() {
        let s = Snapshot {
            transport: Layer { status: Status::Frozen, measured: 5000, since: Some(100) },
            ..Snapshot::default()
        };
        let net = hint(&s, &t(), false).unwrap();
        assert!(net.cause.contains("network link stalled") && net.severity == Severity::Possible, "{net:?}");
        assert!(hint(&s, &t(), true).unwrap().cause.contains("its main thread was busy"));
    }

    #[test]
    fn a_client_frozen_on_its_own_code_a_spiral_and_a_hidden_window() {
        let own = Snapshot { render: frozen_render(None), ..Snapshot::default() };
        let h = hint(&own, &t(), false).unwrap();
        assert!(h.cause.contains("not aio's") && h.severity == Severity::Possible);
        let spiral = Render { status: Status::Recovered, measured: 0, last_call: None, visible: true, freezes: 3 };
        let h = hint(&Snapshot { render: spiral, ..Snapshot::default() }, &t(), false).unwrap();
        assert!(h.cause.starts_with("Repeated freeze-recover cycle (3 times)"));
        let hidden = Render { visible: false, ..frozen_render(None) };
        assert_eq!(hint(&Snapshot { render: hidden, ..Snapshot::default() }, &t(), false), None);
        assert_eq!(hint(&Snapshot::default(), &t(), false), None, "all healthy");
    }

    #[test]
    fn the_render_probes_lines_are_read_back() {
        let l = "WARN  [page] [vitals] render frozen: an update took 3200 ms to paint · likely: a large DOM update";
        assert_eq!(render_step(l), Some((Status::Frozen, 3200)));
        assert_eq!(
            render_step("INFO  [page] [vitals] render recovered: an update painted in 12 ms"),
            Some((Status::Recovered, 12))
        );
        assert_eq!(render_step("WARN  [page] [vitals] render warning: an update took 600 ms to paint"), Some((Status::Warning, 600)));
        for no in ["WARN  [app] [vitals] render frozen: an update took 1 ms", "[page] [vitals] render slow: 3 ms", "x"] {
            assert_eq!(render_step(no), None, "{no}");
        }
    }

    #[test]
    fn the_vitals_doc_says_each_layer_the_likely_cause_and_the_thresholds() {
        let s = Snapshot {
            transport: Layer { status: Status::Frozen, measured: 5000, since: Some(100) },
            ..Snapshot::default()
        };
        let d = doc(&s, true, &t());
        assert_eq!((d["loop"]["status"].as_str(), d["transport"]["measured"].as_u64()), (Some("healthy"), Some(5000)));
        assert_eq!(d["transport"]["loopback"], true);
        assert!(d["hint"]["cause"].as_str().unwrap().contains("its main thread was busy"), "{d}");
        assert_eq!(d["thresholds"]["queue"]["frozen"], t().queue.frozen);
        assert_eq!(doc(&Snapshot::default(), false, &t())["hint"], serde_json::Value::Null);
    }

    #[test]
    fn every_layer_feeds_one_snapshot_so_the_hints_see_them_all() {
        // The newest call was slow, then the paint froze: the call is named.
        ran("orders", "recalc", std::time::Duration::from_millis(3100));
        let lp = probe(0, None, &t());
        looped(&lp);
        let s = rendered(Status::Frozen, 3200, 1_000);
        assert_eq!(s.render.last_call.as_deref(), Some("orders.recalc"));
        assert!(hint(&s, &t(), false).unwrap().cause.contains("The call orders.recalc took 3100 ms"));
        // A paint that freezes and recovers again and again is a spiral.
        rendered(Status::Recovered, 10, 2_000);
        rendered(Status::Frozen, 3000, 3_000);
        let s = rendered(Status::Recovered, 10, 4_000);
        assert_eq!(s.render.freezes, 2, "the freezes of the last 30 s");
        assert!(hint(&s, &t(), false).unwrap().cause.starts_with("Repeated freeze-recover cycle (2 times)"));
        assert_eq!(rendered(Status::Recovered, 10, 60_000).render.freezes, 0, "older ones drop out");
        // The worst link is the transport; when it goes, the layer is healthy again.
        linked(7, true, Layer { status: Status::Frozen, measured: 4000, since: Some(1) });
        linked(8, false, Layer { status: Status::Healthy, measured: 10, since: None });
        assert_eq!(snapshot(), (Snapshot { transport: Layer { status: Status::Frozen, measured: 4000, since: Some(1) }, ..snapshot().0 }, true));
        unlinked(7);
        assert_eq!(snapshot().0.transport.status, Status::Healthy);
    }

    #[test]
    fn a_step_is_said_on_the_bus_as_vitals_layer() {
        let heard = std::sync::Arc::new(Mutex::new(vec![]));
        let h = heard.clone();
        let _sub = crate::report::subscribe(move |d| {
            if d.kind == "vitals:test-layer" {
                h.lock().unwrap().push((d.severity, d.message.clone(), d.code()));
            }
        });
        stepped("test-layer", Status::Frozen, "test-layer frozen: 9 ms");
        let got = heard.lock().unwrap().clone();
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].0, got[0].2), (crate::log::Level::Warn, crate::report::Code::Vitals));
        assert!(got[0].1.starts_with("test-layer frozen: 9 ms"), "{got:?}");
    }

    #[test]
    fn how_sure_a_hint_is() {
        assert_eq!(severity(2, true, true), Severity::Likely);
        assert_eq!(severity(1, true, false), Severity::Possible);
        assert_eq!(severity(0, false, false), Severity::Speculative);
    }

    #[test]
    fn a_layer_says_each_step_up_to_trouble_and_the_way_back() {
        let mut tr = Track::default();
        assert_eq!(tr.step(Status::Degraded, 10), None, "degraded is not said");
        assert_eq!(tr.since, Some(10));
        assert_eq!(tr.step(Status::Warning, 20), Some(Status::Warning));
        assert_eq!(tr.step(Status::Warning, 30), None, "said once");
        assert_eq!(tr.step(Status::Frozen, 40), Some(Status::Frozen));
        assert_eq!(tr.step(Status::Degraded, 50), None, "still in trouble");
        assert_eq!((tr.step(Status::Healthy, 60), tr.since), (Some(Status::Recovered), None));
        assert_eq!(tr.step(Status::Degraded, 70), None);
        assert_eq!(tr.step(Status::Healthy, 80), None, "from degraded, nothing to say");
    }

    #[test]
    fn a_server_says_each_link_s_liveness_and_payloads_and_each_cell_s_size() {
        let link = |id, gap| Link { id, cell: "notes".into(), gap, last_bytes: 40, total_bytes: 900 };
        let v = served(&[link(1, 800), link(2, 2500)], &[("notes", 128, true), ("wallet", 4096, false)], &t());
        assert_eq!(
            v["clients"][0],
            serde_json::json!({ "id": 1, "cell": "notes", "status": "healthy", "gap": 800, "lastPayloadBytes": 40, "totalBytes": 900 })
        );
        assert_eq!((v["clients"][1]["status"].as_str(), v["clients"][1]["frozenFor"].as_u64()), (Some("frozen"), Some(2500)));
        assert_eq!(v["cellSizes"], serde_json::json!({ "notes": 128, "wallet": 4096 }));
        assert_eq!(v["breakers"], serde_json::json!(["wallet"]));
    }

    #[test]
    fn effects_that_got_their_turn_are_the_drain_rate() {
        let before = DRAINED.load(Ordering::Relaxed);
        drop([Waiting::new(), Waiting::new(), Waiting::new()]);
        assert!(DRAINED.load(Ordering::Relaxed) >= before + 3);

    }

    #[test]
    fn the_loop_line_says_how_late_and_the_likely_cause() {
        let lp = Loop {
            layer: Layer { status: Status::Frozen, measured: 2400, since: Some(1) },
            queue: 3,
            drained: 0,
            last: Some(("orders.recalc".into(), 2300)),
        };
        assert_eq!(
            says(Status::Frozen, &lp, &t()),
            "loop frozen: 2400 ms late, 3 effects waiting · likely: the call orders.recalc took 2300 ms"
        );
        let quiet = Vitals { hints: false, ..t() };
        assert_eq!(says(Status::Frozen, &lp, &quiet), "loop frozen: 2400 ms late, 3 effects waiting");
        assert_eq!(says(Status::Recovered, &lp, &t()), "loop recovered");
    }
}

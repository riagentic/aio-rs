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
            event_loop: Loop { layer: healthy, queue: 0, last: None },
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

static SET: RwLock<Option<Vitals>> = RwLock::new(None);
static QUEUED: AtomicU64 = AtomicU64::new(0);
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

/// The server's loop and queue as they are, `late` ms behind.
pub fn probe(late: u64, since: Option<i64>, t: &Vitals) -> Loop {
    let queue = QUEUED.load(Ordering::Relaxed);
    let status = t.event_loop.grade(late).max(t.queue.grade(queue));
    let last = {
        let l = LAST.lock().unwrap_or_else(|e| e.into_inner());
        (!l.0.is_empty()).then(|| (format!("{}.{}", l.0, l.1), l.2))
    };
    Loop { layer: Layer { status, measured: late, since }, queue, last }
}

/// The log line for the loop stepping to `step` (pure): what, how much,
/// and with `hints`, the likely cause.
pub fn says(step: Status, lp: &Loop, t: &Vitals) -> String {
    let what = match step {
        Status::Recovered => return "loop recovered".into(),
        s => format!("{s:?}").to_lowercase(),
    };
    let mut line = format!("loop {what}: {} ms late, {} effects waiting", lp.layer.measured, lp.queue);
    let snap = Snapshot { event_loop: lp.clone(), ..Snapshot::default() };
    let slow = lp.last.as_ref().filter(|(_, ms)| *ms > t.event_loop.degraded);
    let h = hint(&snap, t, false).map(|h| h.cause).or_else(|| slow.map(|(c, ms)| format!("the call {c} took {ms} ms")));
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
    fn the_loop_line_says_how_late_and_the_likely_cause() {
        let lp = Loop {
            layer: Layer { status: Status::Frozen, measured: 2400, since: Some(1) },
            queue: 3,
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

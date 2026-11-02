//! Perf budgets: a method slower than its budget, or an effect step (one
//! poll: the time it holds a worker thread) slower than its, is a violation.
//! Every one goes to `logs/perf.log`, with where a method's time went
//! (`clone`, `apply`, `check`, `commit`), and to the `on_problem` hook
//! (`budget:method`, `budget:effect`); the app log says it as a `WARN [perf]`
//! line with the fix, at most once per label per [`QUIET`]. Observe-only: a
//! slow call still commits.
//!
//! A method's budget counts its own work (the state's copy, the method, the
//! invariants), not the commit: storage speed is the disk's and
//! `[data] durability`'s, not the method's. An effect's step does not count
//! the calls it makes; each of those is a method, budgeted itself.
use std::cell::Cell as Slot;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

pub use aio_config::perf::{Budgets, Storm};

/// How long the log stays quiet about the same label after saying it.
pub const QUIET: Duration = Duration::from_secs(10);
/// `perf.log`'s size before it rotates.
const MAX: u64 = 1 << 20;

/// One method's calls, in one-second buckets.
#[derive(Debug)]
struct Rate {
    start: i64,
    count: u32,
    hot: u32,
    storming: bool,
}

/// What a tracked call changed: a storm began or ended (pure detector).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Weather {
    Began { label: String, rate: u32, secs: u32 },
    Ended { label: String, rate: u32 },
}

/// Call rates per method label, driven by the calls (no timer): a storm
/// whose source went quiet ends at the next call of any method.
#[derive(Debug, Default)]
pub struct Storms {
    rates: HashMap<String, Rate>,
    storming: usize,
}

impl Storms {
    /// A call of `label` at `now` (ms): refuse it (a storm, breaker on)?
    /// And what began or ended.
    pub fn track(&mut self, cfg: &Storm, label: &str, now: i64) -> (bool, Vec<Weather>) {
        let mut said = vec![];
        if self.storming > 0 {
            let hot: Vec<String> = self.rates.iter().filter(|(_, r)| r.storming).map(|(l, _)| l.clone()).collect();
            for l in hot.iter().filter(|l| *l != label) {
                self.roll(cfg, l, now, &mut said);
            }
        }
        self.roll(cfg, label, now, &mut said);
        let r = self.rates.entry(label.into()).or_insert(Rate { start: now, count: 0, hot: 0, storming: false });
        r.count += 1;
        (cfg.breaker && r.storming, said)
    }

    /// Close `label`'s bucket if its second is over.
    fn roll(&mut self, cfg: &Storm, label: &str, now: i64, said: &mut Vec<Weather>) {
        let Some(r) = self.rates.get_mut(label) else { return };
        let elapsed = now - r.start;
        if elapsed < 1000 {
            return;
        }
        // A gap of more than a bucket: the seconds skipped had no calls.
        let rate = if elapsed < 2000 { r.count } else { 0 };
        if rate >= cfg.rate {
            r.hot += 1;
            if !r.storming && r.hot >= cfg.secs {
                r.storming = true;
                self.storming += 1;
                said.push(Weather::Began { label: label.into(), rate, secs: r.hot });
            }
        } else {
            if r.storming {
                self.storming -= 1;
                said.push(Weather::Ended { label: label.into(), rate });
            }
            (r.hot, r.storming) = (0, false);
            // Quiet labels go: the map holds only what is being called.
            if rate == 0 {
                self.rates.remove(label);
                return;
            }
        }
        (r.count, r.start) = (0, now);
    }
}

/// A call of `cell.method` starts: refused when it is in a storm and the
/// breaker is on. A storm is said (log, `on_problem` as `storm`) as it
/// begins, and its end in the log.
pub(crate) fn storm(cell: &str, method: &str) -> crate::Result {
    static STORMS: Mutex<Option<Storms>> = Mutex::new(None);
    let cfg = budgets(|b| b.storm);
    let label = format!("{cell}.{method}");
    let (refuse, said) = STORMS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_default().track(
        &cfg,
        &label,
        crate::store::now_ms(),
    );
    for w in said {
        match w {
            Weather::Began { label, rate, secs } => {
                let what = format!("{label} called {rate} times a second, {secs} s in a row");
                let fix = "a feedback loop: find what calls it in reply to its own change ([perf.storm] breaker = true refuses it meanwhile)";
                crate::warn!(cat: "perf", "{}", crate::report::teach(&format!("call storm: {what}"), fix));
                crate::report::tell("storm", &what);
            }
            Weather::Ended { label, rate } => crate::info!(cat: "perf", "call storm over: {label} ({rate} a second)"),
        }
    }
    if refuse {
        crate::reject!("{label}: refused during a call storm ([perf.storm] breaker)");
    }
    Ok(())
}

/// Where a method's time went.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Phases {
    /// Copying the state the method changes.
    pub clone: Duration,
    /// The method itself.
    pub apply: Duration,
    /// The invariants.
    pub check: Duration,
    /// Writing the event (and a snapshot, now and then).
    pub commit: Duration,
}

impl Phases {
    /// What the budget counts.
    pub fn own(&self) -> Duration {
        self.clone + self.apply + self.check
    }
}

static BUDGETS: RwLock<Option<Budgets>> = RwLock::new(None);
static SINK: Mutex<Option<crate::log::Rotating>> = Mutex::new(None);
static SAID: Mutex<Option<Quiet>> = Mutex::new(None);

/// The budgets from now on (`aio::boot` sets `[perf]`).
pub fn set(b: Budgets) {
    *BUDGETS.write().unwrap_or_else(|e| e.into_inner()) = Some(b);
}

fn budgets<R>(f: impl FnOnce(&Budgets) -> R) -> R {
    f(BUDGETS.read().unwrap_or_else(|e| e.into_inner()).as_ref().unwrap_or(&Budgets::default()))
}

/// Write violations to `logs/perf.log` (`aio::boot` does, once).
pub fn start(logs: &Path) -> std::io::Result<()> {
    *SINK.lock().unwrap_or_else(|e| e.into_inner()) = Some(crate::log::Rotating::named(logs, "perf", MAX)?);
    Ok(())
}

/// `took` in ms as text that is over `limit` whenever `took` is (pure): whole
/// ms, or as many decimals as it takes; never `5ms > 5ms`.
pub fn over(took: Duration, limit: Duration) -> String {
    let (v, l) = (took.as_secs_f64() * 1e3, limit.as_secs_f64() * 1e3);
    (0..=3).map(|d| format!("{v:.d$}")).find(|s| s.parse::<f64>().is_ok_and(|x| x > l)).unwrap_or_else(|| v.to_string())
}

/// The violation (pure): `method notes.add 43ms > 16ms (clone=0.1ms …)`.
pub fn line(kind: &str, label: &str, took: Duration, budget: Duration, phases: Option<&Phases>) -> String {
    let ms = |d: Duration| format!("{:.1}ms", d.as_secs_f64() * 1e3);
    let mut l = format!("{kind} {label} {}ms > {}ms", over(took, budget), budget.as_millis());
    if let Some(p) = phases {
        l += &format!(" (clone={} apply={} check={} commit={})", ms(p.clone), ms(p.apply), ms(p.check), ms(p.commit));
    }
    l
}

/// The one thing to do, with the budget that would fit just this label
/// (pure): the next 10 ms step at or over twice what it took.
pub fn fix(kind: &str, label: &str, took: Duration) -> String {
    let fits = (took.as_millis() as u64 * 2).div_ceil(10).max(1) * 10;
    let what = if kind == "method" {
        "move slow work into an Effect"
    } else {
        "a step blocks a worker thread: await async I/O, or use tokio::task::spawn_blocking"
    };
    format!("{what}; slow on purpose? aio.toml [perf.{kind}s] \"{label}\" = {fits}")
}

/// Labels said lately, and how many violations each kept quiet since.
#[derive(Default)]
struct Quiet(HashMap<String, (Instant, u64)>);

impl Quiet {
    /// Say `label` now? Then with how many were kept quiet since the last time.
    fn admit(&mut self, label: &str, now: Instant) -> Option<u64> {
        match self.0.get_mut(label) {
            Some((at, kept)) if now.duration_since(*at) < QUIET => {
                *kept += 1;
                None
            }
            Some((at, kept)) => {
                *at = now;
                Some(std::mem::take(kept))
            }
            None => {
                self.0.insert(label.into(), (now, 0));
                Some(0)
            }
        }
    }
}

fn violated(kind: &str, label: &str, took: Duration, budget: Duration, phases: Option<&Phases>) {
    let l = line(kind, label, took, budget, phases);
    if let Some(log) = SINK.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        _ = log.line(crate::store::now_ms(), &format!("PERF  [perf] {l}"));
    }
    let said = SAID.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_default().admit(label, Instant::now());
    if let Some(kept) = said {
        let more = if kept > 0 { format!(" ({kept} more in the last {}s)", QUIET.as_secs()) } else { String::new() };
        crate::warn!(cat: "perf", "{}", crate::report::teach(&format!("{l}{more}"), &fix(kind, label, took)));
    }
    crate::report::tell(&format!("budget:{kind}"), &l);
}

/// The await ceiling of async methods (`[perf] call_ceiling`).
pub(crate) fn ceiling() -> Duration {
    budgets(|b| b.ceiling())
}

/// The budgets set for methods `T` does not have.
pub(crate) fn unknown<T: crate::Cell>() -> Vec<String> {
    budgets(|b| b.unknown(T::ID, T::METHODS))
}

/// A method committed: over its budget?
pub(crate) fn method(cell: &'static str, method: &str, p: Phases) {
    let took = p.own();
    crate::cost::ran(cell, took);
    crate::vitals::ran(cell, method, took);
    // No label to build on the fast path: no per-method budget, within the default.
    let budget = budgets(|b| if b.methods.is_empty() { b.method("") } else { b.method(&format!("{cell}.{method}")) });
    if took > budget {
        violated("method", &format!("{cell}.{method}"), took, budget, Some(&p));
    }
    WATCHED.with_borrow_mut(|w| {
        let Some((budgets, over)) = w else { return };
        let label = format!("{cell}.{method}");
        if let Some(b) = budgets.get(&label).filter(|b| took > **b) {
            over.push(line("method", &label, took, *b, Some(&p)));
        }
    });
}

/// A test's own budgets on this thread (`TestCells::budget`), by
/// `cell.method`, and the calls over them so far.
type Watched = (HashMap<String, Duration>, Vec<String>);
thread_local!(static WATCHED: std::cell::RefCell<Option<Watched>> = const { std::cell::RefCell::new(None) });

#[cfg(feature = "testing")]
/// Hold `label`'s calls on this thread to `budget` (a failure, not a log line).
pub(crate) fn watch(label: String, budget: Duration) {
    WATCHED.with_borrow_mut(|w| _ = w.get_or_insert_default().0.insert(label, budget));
}

#[cfg(feature = "testing")]
/// The calls over a watched budget since the last take; `stop`: watch no more.
pub(crate) fn watched(stop: bool) -> Vec<String> {
    WATCHED.with_borrow_mut(|w| {
        let over = w.as_mut().map(|(_, o)| std::mem::take(o)).unwrap_or_default();
        if stop {
            *w = None;
        }
        over
    })
}

thread_local!(static IN_CALLS: Slot<Duration> = const { Slot::new(Duration::ZERO) });

/// A call took `d` on this thread (not billed to the effect step around it).
pub(crate) fn called(d: Duration) {
    IN_CALLS.set(IN_CALLS.get() + d);
}

/// Run one step of effect `key` (`poll`), timed: over its budget?
pub(crate) fn step<R>(cell: &str, key: &str, poll: impl FnOnce() -> R) -> R {
    let before = IN_CALLS.replace(Duration::ZERO);
    let t = Instant::now();
    let r = crate::report::during(|| format!("effect {cell}.{key}"), poll);
    let took = t.elapsed().saturating_sub(IN_CALLS.replace(before));
    let label = || format!("{cell}.{}", key.split(':').next().unwrap_or(key));
    let budget = budgets(|b| if b.effects.is_empty() { b.effect("") } else { b.effect(&label()) });
    if took > budget {
        violated("effect", &label(), took, budget, None);
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn the_text_is_over_the_budget_whenever_the_time_is() {
        assert_eq!(over(Duration::from_micros(42_600), 16 * MS), "43");
        assert_eq!(over(Duration::from_micros(5_400), 5 * MS), "5.4");
        assert_eq!(over(Duration::from_micros(5_040), 5 * MS), "5.04");
        assert_eq!(over(Duration::from_micros(5_004), 5 * MS), "5.004");
        assert_eq!(over(Duration::from_nanos(5_000_400), 5 * MS), "5.0004");
        assert_eq!(over(Duration::from_micros(5_600), 5 * MS), "6");
        for i in 1..2000 {
            let t = 5 * MS + Duration::from_micros(i);
            assert!(over(t, 5 * MS).parse::<f64>().unwrap() > 5.0, "{t:?}");
        }
    }

    #[test]
    fn a_violation_says_where_the_time_went_and_the_fix() {
        let p = Phases { clone: MS / 10, apply: 40 * MS, check: Duration::ZERO, commit: 2 * MS };
        assert_eq!(p.own(), MS / 10 + 40 * MS);
        assert_eq!(
            line("method", "notes.add", p.own(), 16 * MS, Some(&p)),
            "method notes.add 40ms > 16ms (clone=0.1ms apply=40.0ms check=0.0ms commit=2.0ms)"
        );
        assert_eq!(
            line("effect", "notes.fetch", Duration::from_micros(5_400), 5 * MS, None),
            "effect notes.fetch 5.4ms > 5ms"
        );
        // The budget it names fits a one-off: twice the time, in 10 ms steps.
        assert!(fix("method", "notes.add", 14 * MS).ends_with("[perf.methods] \"notes.add\" = 30"));
        assert!(fix("effect", "notes.fetch", MS / 2).ends_with("[perf.effects] \"notes.fetch\" = 10"));
        assert!(fix("effect", "x.y", MS).contains("spawn_blocking"));
    }

    #[test]
    fn a_per_method_budget_raises_only_that_method() {
        let b: Budgets =
            toml::from_str("method = 5\n[methods]\n\"builds.compile\" = 5000\n[effects]\n\"hw.poll\" = 250").unwrap();
        assert_eq!(
            (b.method("builds.compile"), b.method("builds.list"), b.method("other.compile")),
            (5000 * MS, 5 * MS, 5 * MS)
        );
        assert_eq!((b.effect("hw.poll"), b.effect("hw.read")), (250 * MS, 5 * MS));
        let d = Budgets::default();
        assert_eq!(d.method("a.b"), if cfg!(debug_assertions) { 16 * MS } else { 100 * MS });
        assert!(toml::from_str::<Budgets>("reduce = 5").is_err(), "a typo is an error");
        // A key naming no method of its cell is found; other cells' are not ours.
        let b: Budgets =
            toml::from_str("[methods]\n\"builds.compile\" = 1\n\"builds.compil\" = 1\n\"hw.x\" = 1").unwrap();
        assert_eq!(b.unknown("builds", &["compile", "list"]), ["builds.compil"]);
    }

    #[test]
    fn the_log_says_a_label_once_per_quiet_spell() {
        let (mut q, t0) = (Quiet::default(), Instant::now());
        assert_eq!(q.admit("a.x", t0), Some(0));
        assert_eq!(q.admit("a.x", t0 + MS), None);
        assert_eq!(q.admit("a.x", t0 + 2 * MS), None);
        assert_eq!(q.admit("b.y", t0 + 2 * MS), Some(0), "labels are apart");
        assert_eq!(q.admit("a.x", t0 + QUIET), Some(2), "said again, with what it kept quiet");
        assert_eq!(q.admit("a.x", t0 + QUIET + MS), None);
    }

    /// `n` calls of `label` spread over the second starting at `s` seconds.
    fn second(st: &mut Storms, cfg: &Storm, label: &str, s: i64, n: i64) -> (Vec<bool>, Vec<Weather>) {
        let (mut refused, mut said) = (vec![], vec![]);
        for i in 0..n {
            let (r, w) = st.track(cfg, label, s * 1000 + i * 1000 / n);
            refused.push(r);
            said.extend(w);
        }
        (refused, said)
    }

    #[test]
    fn a_sustained_rate_is_a_storm_said_once_as_it_begins_and_ends() {
        let cfg = Storm { rate: 10, secs: 3, breaker: false };
        let mut st = Storms::default();
        let said: Vec<_> = (0..6).flat_map(|s| second(&mut st, &cfg, "w.changed", s, 20).1).collect();
        assert_eq!(said, [Weather::Began { label: "w.changed".into(), rate: 20, secs: 3 }], "once, not per second");
        // Ending by dropping under the rate says it ended, at that rate.
        let (_, said) = second(&mut st, &cfg, "w.changed", 6, 2);
        let (_, end) = second(&mut st, &cfg, "w.changed", 7, 1);
        assert_eq!((said, end), (vec![], vec![Weather::Ended { label: "w.changed".into(), rate: 2 }]));
        // Bursts shorter than `secs`, and quiet labels, never storm.
        let mut st = Storms::default();
        let mut said = vec![];
        for s in 0..20 {
            said.extend(second(&mut st, &cfg, "burst.x", s, if s % 3 == 2 { 1 } else { 50 }).1);
            said.extend(second(&mut st, &cfg, "quiet.y", s, 3).1);
        }
        assert_eq!(said, []);
    }

    #[test]
    fn the_breaker_refuses_mid_storm_and_a_silent_source_ends_at_any_call() {
        let cfg = Storm { rate: 10, secs: 2, breaker: true };
        let mut st = Storms::default();
        let refused: Vec<bool> = (0..4).flat_map(|s| second(&mut st, &cfg, "loop.a", s, 20).0).collect();
        assert!(!refused[..40].contains(&true) && refused[60..].iter().all(|r| *r), "refused once it is a storm");
        // Other methods are tracked apart: never refused.
        assert_eq!(second(&mut st, &cfg, "calm.b", 4, 5).0, [false; 5]);
        // loop.a went silent: the next call of anything ends its storm.
        let (_, said) = second(&mut st, &cfg, "calm.b", 9, 1);
        assert_eq!(said, [Weather::Ended { label: "loop.a".into(), rate: 0 }]);
        assert_eq!(second(&mut st, &cfg, "loop.a", 10, 1).0, [false], "recovered");
        assert!(st.rates.len() <= 2, "quiet labels are dropped");
    }

    #[test]
    fn an_effect_step_is_not_billed_for_the_calls_it_makes() {
        // The calls are billed elsewhere: the step itself was quick.
        let r = step("perf-test", "k", || {
            called(Duration::from_secs(3600));
            7
        });
        assert_eq!((r, IN_CALLS.get()), (7, Duration::ZERO));
    }
}

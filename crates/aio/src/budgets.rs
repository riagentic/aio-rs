//! `[budgets]` (aio's `budgets`): the sizes and rates an app commits to, in
//! the units a person writes them in: `state = "1MB"` (one cell's saved
//! state), `broadcast_rate = "20/s"` (state pushes to one client),
//! `payload = "500KB"` (one push). `[perf]` holds the times; this the sizes.
//! A breach is kept with its worst reading, said once in the log and to the
//! `on_problem` hook (`budget:<name>`), and turns `/__aio/health` degraded,
//! so a test or a CI step can fail on it. An app that declared none has no
//! verdict (`null`), never a green tick for a promise nobody made.
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Mutex;

pub use aio_config::budgets::{Budgets, parse_rate, parse_size};

/// A budget that was exceeded, with the worst reading seen.
#[derive(Clone, Debug, PartialEq)]
pub struct Breach {
    pub limit: f64,
    pub worst: f64,
    /// What: `cell notes`.
    pub detail: String,
}

/// The declared budgets, and their breaches by name (`state`, …).
#[derive(Default)]
struct Ledger {
    declared: Budgets,
    breaches: BTreeMap<&'static str, Breach>,
}

static LEDGER: Mutex<Option<Ledger>> = Mutex::new(None);

fn ledger() -> std::sync::MutexGuard<'static, Option<Ledger>> {
    LEDGER.lock().unwrap_or_else(|e| e.into_inner())
}

/// This app's budgets (`aio::boot` does it, from `aio.toml`); breaches start over.
pub fn declare(b: Budgets) {
    *ledger() = Some(Ledger { declared: b, breaches: BTreeMap::new() });
}

fn declared() -> Budgets {
    ledger().as_ref().map(|l| l.declared).unwrap_or_default()
}

/// `measured` against the budget `name`: kept when over it and the worst
/// so far; the first breach of each budget is said.
fn record(name: &'static str, measured: f64, detail: impl FnOnce() -> String) {
    let said = {
        let mut g = ledger();
        let Some(l) = g.as_mut() else { return };
        let limit = match name {
            "state" => l.declared.state.map(|n| n as f64),
            "payload" => l.declared.payload.map(|n| n as f64),
            _ => l.declared.broadcast_rate,
        };
        let Some(limit) = limit.filter(|limit| measured > *limit) else { return };
        let first = !l.breaches.contains_key(name);
        if l.breaches.get(name).is_some_and(|b| b.worst >= measured) {
            return;
        }
        let b = Breach { limit, worst: measured, detail: detail() };
        let said = first.then(|| format!("{} is over its [budgets] {name}: {}", b.detail, shown(name, &b)));
        l.breaches.insert(name, b);
        said
    };
    if let Some(msg) = said {
        eprintln!("[aio] {msg}");
        crate::report::tell(&format!("budget:{name}"), &msg);
    }
}

/// `2.0 MB > 1.0 MB`, `31/s > 20/s`.
fn shown(name: &str, b: &Breach) -> String {
    match name {
        "broadcast_rate" => format!("{}/s > {}/s", b.worst, b.limit),
        _ => format!("{} > {}", crate::fmt::bytes(b.worst as u64), crate::fmt::bytes(b.limit as u64)),
    }
}

/// Every cell's saved size against `state` (from the health sampler and
/// `/__aio/health`). Free when no `state` budget is declared.
pub(crate) fn measure_cells() {
    if declared().state.is_none() {
        return;
    }
    for (cell, n) in crate::report::sizes() {
        record("state", n as f64, || format!("cell {cell}"));
    }
}

/// aio's own limits where `[budgets]` declares none (aio's pressure monitor):
/// one push, pushes a second, and bytes a second to one client. Over one,
/// it is said in the log (at most once each [`PRESSURE_EVERY_MS`] per
/// limit and client), never a breach: nobody promised it.
const PRESSURE_PAYLOAD: u64 = 500 << 10;
const PRESSURE_RATE: u32 = 30;
const PRESSURE_BANDWIDTH: u64 = 1 << 20;
const PRESSURE_EVERY_MS: i64 = 2000;

/// State pushes to one client, counted per second.
#[derive(Default)]
pub(crate) struct Pushes {
    second: i64,
    n: u32,
    bytes: u64,
    /// When each pressure line was last said.
    said: [Option<i64>; 3],
}

impl Pushes {
    /// One push of `msg` to this client at `now` (ms): its size and the
    /// pushes this second, against `payload` and `broadcast_rate`, else
    /// aio's own limits.
    pub(crate) fn push(&mut self, msg: &Value, now: i64) {
        for line in self.pressure(json_len(msg), now, declared()) {
            crate::warn!(cat: "vitals", "{line}");
        }
    }

    /// [`Self::push`]'s readings, and the pressure lines to say (pure, but
    /// for declared budgets' breaches, which are recorded).
    fn pressure(&mut self, size: u64, now: i64, b: Budgets) -> Vec<String> {
        if now / 1000 != self.second {
            (self.second, self.n, self.bytes) = (now / 1000, 0, 0);
        }
        self.n += 1;
        self.bytes += size;
        if b.payload.is_some() {
            record("payload", size as f64, || "a push to a client".into());
        }
        if b.broadcast_rate.is_some() {
            record("broadcast_rate", self.n as f64, || "pushes to a client".into());
        }
        let over = [
            (b.payload.is_none() && size >= PRESSURE_PAYLOAD).then(|| {
                format!(
                    "pressure: a push of {} to one client (aio-rs's limit {}) · likely: a big cell; find it with `ax \
                     cost`, keep bulk rows in a db table and page them, or if it is meant, declare [budgets] payload \
                     = \"{}\"",
                    crate::fmt::bytes(size),
                    crate::fmt::bytes(PRESSURE_PAYLOAD),
                    crate::fmt::bytes(size.next_power_of_two()).replace(' ', "")
                )
            }),
            (b.broadcast_rate.is_none() && self.n == PRESSURE_RATE + 1).then(|| {
                format!(
                    "pressure: over {PRESSURE_RATE} pushes a second to one client · likely: calls in a burst; \
                     debounce them (cx.after with one key) or batch them in one call"
                )
            }),
            (self.bytes >= PRESSURE_BANDWIDTH && self.bytes - size < PRESSURE_BANDWIDTH).then(|| {
                format!(
                    "pressure: {} a second to one client (aio-rs's limit {}) · likely: much sent per change; find the \
                     cell with `ax cost`, show each client less, or page bulk rows",
                    crate::fmt::bytes(self.bytes),
                    crate::fmt::bytes(PRESSURE_BANDWIDTH)
                )
            }),
        ];
        let mut lines = vec![];
        for (line, said) in over.into_iter().zip(&mut self.said) {
            if let Some(l) = line.filter(|_| said.is_none_or(|at| now - at >= PRESSURE_EVERY_MS)) {
                *said = Some(now);
                lines.push(l);
            }
        }
        lines
    }
}

/// `v`'s size as JSON text, without writing it out.
fn json_len(v: &Value) -> u64 {
    struct Count(u64);
    impl std::io::Write for Count {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0 += b.len() as u64;
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut c = Count(0);
    _ = serde_json::to_writer(&mut c, v);
    c.0
}

/// The verdict for `/__aio/health`: `null` when nothing is declared.
pub fn report() -> Value {
    let g = ledger();
    let Some(l) = g.as_ref().filter(|l| l.declared != Budgets::default()) else { return Value::Null };
    let breaches: Vec<Value> = l
        .breaches
        .iter()
        .map(|(name, b)| json!({ "budget": name, "limit": b.limit, "worst": b.worst, "detail": b.detail, "said": shown(name, b) }))
        .collect();
    json!({ "ok": breaches.is_empty(), "breaches": breaches })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_rates_parse_as_a_person_writes_them() {
        assert_eq!(parse_size("1MB", "state"), Ok(1 << 20));
        assert_eq!(parse_size("512KB", "state"), Ok(512 << 10));
        assert_eq!(parse_size("1.5mb", "state"), Ok((1.5 * (1 << 20) as f64) as u64));
        assert_eq!(parse_size(" 2048 ", "state"), Ok(2048));
        for bad in ["", "MB", "1 XB", "-1MB", "one", "1MB2"] {
            let e = parse_size(bad, "state").unwrap_err();
            assert!(e.starts_with("[budgets] state:") && e.contains("is not a size"), "{bad}: {e}");
        }
        assert!(parse_size("0", "payload").unwrap_err().contains("zero"));
        assert_eq!(parse_rate("20/s", "r"), Ok(20.0));
        assert_eq!(parse_rate("20 / sec", "r"), Ok(20.0));
        assert_eq!(parse_rate("20 per second", "r"), Ok(20.0));
        assert_eq!(parse_rate("60", "r"), Ok(60.0));
        assert!(parse_rate("fast", "r").unwrap_err().contains("is not a rate"));
        assert!(parse_rate("0/s", "r").unwrap_err().contains("zero"));
        let b: Budgets = toml::from_str("state = \"1MB\"\nbroadcast_rate = 30\npayload = 4096").unwrap();
        assert_eq!(b, Budgets { state: Some(1 << 20), broadcast_rate: Some(30.0), payload: Some(4096) });
        assert_eq!(
            serde_json::to_value(b).unwrap(),
            json!({ "state": "1MB", "broadcast_rate": "30/s", "payload": "4KB" })
        );
        assert!(toml::from_str::<Budgets>("state = \"1 XB\"").unwrap_err().to_string().contains("is not a size"));
        assert!(toml::from_str::<Budgets>("cell_state = \"1MB\"").is_err(), "a misspelt key is an error");
        assert_eq!(toml::from_str::<Budgets>("").unwrap(), Budgets::default());
    }

    #[test]
    fn a_breach_keeps_its_worst_reading_is_said_once_and_none_declared_is_no_verdict() {
        declare(Budgets::default());
        record("state", 1e9, || "cell x".into());
        assert_eq!(report(), Value::Null, "declared nothing: no verdict, never a green tick");
        declare(Budgets { state: Some(1024), payload: Some(100), broadcast_rate: Some(2.0) });
        assert_eq!(report(), json!({ "ok": true, "breaches": [] }));
        record("state", 1000.0, || "cell small".into());
        assert_eq!(report()["ok"], true, "under the limit");
        record("state", 2048.0, || "cell big".into());
        record("state", 1500.0, || "cell bigger?".into());
        let r = report();
        assert_eq!(
            r["breaches"],
            json!([{ "budget": "state", "limit": 1024.0, "worst": 2048.0, "detail": "cell big", "said": "2.0 KB > 1.0 KB" }])
        );
        let mut p = Pushes::default();
        p.push(&json!("x".repeat(200)), 5_000);
        p.push(&json!(1), 5_100);
        assert_eq!(report()["breaches"].as_array().map(Vec::len), Some(2), "the payload; 2 pushes is 2/s");
        p.push(&json!(1), 5_900);
        p.push(&json!(1), 6_000);
        let r = report();
        assert_eq!((r["ok"].as_bool(), r["breaches"][0]["budget"].as_str()), (Some(false), Some("broadcast_rate")));
        assert_eq!(r["breaches"][0]["worst"], 3.0, "the next second starts over");
        declare(Budgets::default());
    }

    #[test]
    fn with_nothing_declared_aios_own_limits_are_said_throttled_and_never_a_breach() {
        // aio tests/pressure-monitor.test.ts: 500 KB a push, 30 pushes a second, 1 MB/s.
        let none = Budgets::default();
        let mut p = Pushes::default();
        assert!(p.pressure(PRESSURE_PAYLOAD - 1, 0, none).is_empty(), "under the limit");
        let l = p.pressure(600 << 10, 1_100, none);
        assert_eq!(l.len(), 1, "{l:?}");
        assert!(l[0].starts_with("pressure: a push of 600.0 KB to one client (aio-rs's limit 500.0 KB)"), "{l:?}");
        assert!(l[0].ends_with("declare [budgets] payload = \"1.0MB\""), "{l:?}");
        assert_eq!(parse_size("1.0MB", "payload"), Ok(1 << 20), "the hint parses as written");
        assert!(p.pressure(600 << 10, 2_900, none).is_empty(), "said once each 2 s");
        assert_eq!(p.pressure(600 << 10, 3_200, none).len(), 1, "2 s on: said again");
        // Pushes a second: the 31st is said, once.
        let mut p = Pushes::default();
        let said: Vec<_> = (0..40).flat_map(|i| p.pressure(10, 5_000 + i, none)).collect();
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].starts_with("pressure: over 30 pushes a second to one client"), "{said:?}");
        assert!(p.pressure(10, 6_000, none).is_empty(), "a new second");
        // Bytes a second: said when they cross it.
        let mut p = Pushes::default();
        let said: Vec<_> = (0..5).flat_map(|i| p.pressure(300 << 10, 9_000 + i, none)).collect();
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].starts_with("pressure: 1.2 MB a second to one client (aio-rs's limit 1.0 MB)"), "{said:?}");
        let v = serde_json::json!({ "a": [1, "x"], "é": null });
        assert_eq!(json_len(&v), v.to_string().len() as u64);
    }
}

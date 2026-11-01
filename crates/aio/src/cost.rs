//! What the app moves on its links, per cell (aio's `am cost`): each state
//! and patch sent (bytes, and a patch's bytes by the top key it changes),
//! the full resends, and how long its methods take (mean, p95). It reports,
//! it doesn't advise. Rates are per second actually watched (from the
//! first thing seen), never per a window asked for.
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Mutex;
use std::time::Duration;

/// The method times kept per cell for the p95.
const TIMES: usize = 512;

/// One cell's meter.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meter {
    /// When it saw its first send or call, ms since the epoch.
    pub since: i64,
    pub pushes: u64,
    pub bytes: u64,
    /// Whole states sent (a link's first, a resync), not patches.
    pub full: u64,
    /// By top key: (patches that changed it, their ops' bytes).
    pub keys: BTreeMap<String, (u64, u64)>,
    /// The newest method times, µs.
    pub times: VecDeque<u64>,
}

impl Meter {
    fn start(&mut self, now: i64) {
        if self.since == 0 {
            self.since = now;
        }
    }

    /// Message `msg` (a `state` or `patch`) went out as `bytes` (pure).
    pub fn sent(&mut self, msg: &Value, bytes: usize, now: i64) {
        self.start(now);
        self.pushes += 1;
        self.bytes += bytes as u64;
        if msg["t"] == "state" {
            self.full += 1;
        }
        for op in msg["ops"].as_array().into_iter().flatten() {
            let key = match op["path"].get(0) {
                Some(Value::String(k)) => k.clone(),
                Some(v) => v.to_string(),
                None => "(the whole state)".into(),
            };
            let k = self.keys.entry(key).or_default();
            k.0 += 1;
            k.1 += op.to_string().len() as u64;
        }
    }

    /// A method took `took` (pure).
    pub fn ran(&mut self, took: Duration, now: i64) {
        self.start(now);
        if self.times.len() == TIMES {
            self.times.pop_front();
        }
        self.times.push_back(took.as_micros() as u64);
    }

    /// The meter at `now`, as `ax cost` shows it (pure).
    pub fn report(&self, now: i64) -> Value {
        let secs = ((now - self.since).max(1) as f64) / 1000.0;
        let mut sorted: Vec<u64> = self.times.iter().copied().collect();
        sorted.sort_unstable();
        let ms = |us: u64| us as f64 / 1000.0;
        let p95 = sorted.get((sorted.len() * 95).div_ceil(100).saturating_sub(1)).map_or(0.0, |v| ms(*v));
        let mean = if sorted.is_empty() { 0.0 } else { ms(sorted.iter().sum::<u64>()) / sorted.len() as f64 };
        let mut keys: Vec<(&String, &(u64, u64))> = self.keys.iter().collect();
        keys.sort_by(|a, b| b.1.1.cmp(&a.1.1).then(a.0.cmp(b.0)));
        json!({
            "windowS": secs,
            "pushes": self.pushes,
            "bytes": self.bytes,
            "pushesPerS": self.pushes as f64 / secs,
            "bytesPerS": self.bytes as f64 / secs,
            "meanBytes": self.bytes.checked_div(self.pushes).unwrap_or(0),
            "fullResends": self.full,
            "calls": sorted.len(),
            "p95Ms": p95,
            "meanMs": mean,
            "keys": keys.iter().map(|(k, (n, b))| json!({ "key": k, "patches": n, "bytes": b, "bytesPerS": *b as f64 / secs })).collect::<Vec<_>>(),
        })
    }
}

fn meters() -> std::sync::MutexGuard<'static, HashMap<&'static str, Meter>> {
    static M: std::sync::OnceLock<Mutex<HashMap<&'static str, Meter>>> = std::sync::OnceLock::new();
    M.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// Cell `cell` sent `msg` to a link, `bytes` on the wire.
pub fn sent(cell: &'static str, msg: &Value, bytes: usize) {
    meters().entry(cell).or_default().sent(msg, bytes, crate::store::now_ms());
}

/// A method of cell `cell` took `took`.
pub(crate) fn ran(cell: &'static str, took: Duration) {
    meters().entry(cell).or_default().ran(took, crate::store::now_ms());
}

/// Cell `cell`'s meter now (an empty one: nothing seen yet).
pub fn report(cell: &str) -> Value {
    let now = crate::store::now_ms();
    meters().get(cell).cloned().unwrap_or(Meter { since: now, ..Default::default() }).report(now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_are_per_second_watched_and_keys_are_billed_their_ops() {
        let mut m = Meter::default();
        m.sent(&json!({ "t": "state", "state": {} }), 1000, 10_000);
        let patch = json!({ "t": "patch", "ops": [
            { "op": "set", "path": ["notes", "a"], "value": "x".repeat(50) },
            { "op": "set", "path": ["count"], "value": 2 },
        ]});
        m.sent(&patch, 200, 11_000);
        m.sent(&json!({ "t": "patch", "ops": [{ "op": "set", "path": [], "value": 1 }] }), 40, 11_500);
        let r = m.report(12_000);
        assert_eq!(r["windowS"], 2.0, "from the first thing seen, not a window asked for");
        assert_eq!(
            (r["pushes"].as_u64(), r["bytes"].as_u64(), r["fullResends"].as_u64()),
            (Some(3), Some(1240), Some(1))
        );
        assert_eq!(r["bytesPerS"], 620.0);
        assert_eq!(r["meanBytes"], 413);
        let keys: Vec<&str> = r["keys"].as_array().unwrap().iter().map(|k| k["key"].as_str().unwrap()).collect();
        assert_eq!(keys, ["notes", "count", "(the whole state)"], "largest first");
        assert_eq!(r["keys"][0]["bytes"].as_u64(), Some(patch["ops"][0].to_string().len() as u64));
    }

    #[test]
    fn the_p95_is_of_the_newest_calls() {
        let mut m = Meter::default();
        for ms in 1..=100 {
            m.ran(Duration::from_millis(ms), 1);
        }
        let r = m.report(1001);
        assert_eq!(
            (r["p95Ms"].as_f64(), r["meanMs"].as_f64(), r["calls"].as_u64()),
            (Some(95.0), Some(50.5), Some(100))
        );
        for _ in 0..TIMES {
            m.ran(Duration::from_millis(1), 2);
        }
        assert_eq!(m.report(3)["p95Ms"], 1.0, "only the newest are kept");
        assert_eq!(Meter::default().report(5)["p95Ms"], 0.0);
    }
}

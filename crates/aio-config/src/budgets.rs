//! `[budgets]` (aio's `budgets`): the sizes and rates an app commits to, in
//! the units a person writes them in: `state = "1MB"` (one cell's saved
//! state), `broadcast_rate = "20/s"` (state pushes to one client),
//! `payload = "500KB"` (one push). `[perf]` holds the times; this the sizes.
//! A breach is kept with its worst reading, said once in the log and to the
//! `on_problem` hook (`budget:<name>`), and turns `/__aio/health` degraded,
//! so a test or a CI step can fail on it. An app that declared none has no
//! verdict (`null`), never a green tick for a promise nobody made.
use aio_macros::Lit;
use serde::{Deserialize, Serialize};

/// What `[budgets]` declares, each in its base unit (bytes, per second).
#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize, Lit)]
#[serde(deny_unknown_fields, default)]
pub struct Budgets {
    /// Largest one cell's saved state may get (bytes of its JSON).
    #[serde(with = "size", skip_serializing_if = "Option::is_none")]
    pub state: Option<u64>,
    /// State pushes per second to one client.
    #[serde(with = "rate", skip_serializing_if = "Option::is_none")]
    pub broadcast_rate: Option<f64>,
    /// Largest one push to one client (bytes).
    #[serde(with = "size", skip_serializing_if = "Option::is_none")]
    pub payload: Option<u64>,
}

// A rate is parsed finite and above zero: never NaN.
impl Eq for Budgets {}

/// `"1MB"`, `"512kb"`, `"1.5 MB"`, `"2048"` → bytes (`KB` = 1024). A
/// typo is an error, never a default nobody declared.
pub fn parse_size(s: &str, key: &str) -> Result<u64, String> {
    let bad = || format!("[budgets] {key}: {s:?} is not a size: write it as \"1MB\", \"512KB\" or a number of bytes");
    let t = s.trim().to_ascii_lowercase();
    let (n, unit) = t.split_at(t.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(t.len()));
    let unit = match unit.trim() {
        "" | "b" => 1u64,
        "k" | "kb" => 1 << 10,
        "m" | "mb" => 1 << 20,
        "g" | "gb" => 1 << 30,
        _ => return Err(bad()),
    };
    let n: f64 = n.parse().map_err(|_| bad())?;
    match (n * unit as f64).floor() {
        b if b >= 1.0 => Ok(b as u64),
        _ => Err(format!("[budgets] {key}: {s:?} is 0 bytes: a budget of zero would refuse even an empty cell")),
    }
}

/// `"20/s"`, `"20 / sec"`, `"20 per second"`, `"20"` → per second.
pub fn parse_rate(s: &str, key: &str) -> Result<f64, String> {
    let t = s.trim().to_ascii_lowercase();
    let n = ["per second", "/second", "/sec", "/s"]
        .iter()
        .find_map(|u| t.replace(' ', "").strip_suffix(&u.replace(' ', "")).map(String::from))
        .unwrap_or_else(|| t.replace(' ', ""));
    match n.parse::<f64>() {
        Ok(r) if r > 0.0 && r.is_finite() => Ok(r),
        Ok(_) => Err(format!("[budgets] {key}: {s:?} is 0/s: a budget of zero would refuse the first push")),
        Err(_) => Err(format!("[budgets] {key}: {s:?} is not a rate: write it as \"20/s\" or a number per second")),
    }
}

/// A budget as written: text, or a plain number in the base unit.
#[derive(Deserialize)]
#[serde(untagged)]
enum Human {
    Num(f64),
    Text(String),
}

mod size {
    use super::Human;
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
        match Human::deserialize(d)? {
            Human::Num(n) if n >= 1.0 && n.is_finite() => Ok(Some(n as u64)),
            Human::Num(n) => super::parse_size(&n.to_string(), "size").map(Some),
            Human::Text(s) => super::parse_size(&s, "size").map(Some),
        }
        .map_err(serde::de::Error::custom)
    }
    pub fn serialize<S: Serializer>(v: &Option<u64>, s: S) -> Result<S::Ok, S::Error> {
        let n = v.unwrap_or(0);
        let unit = [(1 << 30, "GB"), (1 << 20, "MB"), (1 << 10, "KB")].into_iter().find(|(u, _)| n.is_multiple_of(*u));
        match unit {
            Some((u, name)) if n > 0 => s.serialize_str(&format!("{}{name}", n / u)),
            _ => s.serialize_u64(n),
        }
    }
}

mod rate {
    use super::Human;
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
        match Human::deserialize(d)? {
            Human::Num(n) => super::parse_rate(&n.to_string(), "broadcast_rate").map(Some),
            Human::Text(s) => super::parse_rate(&s, "broadcast_rate").map(Some),
        }
        .map_err(serde::de::Error::custom)
    }
    pub fn serialize<S: Serializer>(v: &Option<f64>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("{}/s", v.unwrap_or(0.0)))
    }
}

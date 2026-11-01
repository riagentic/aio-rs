use aio_macros::Lit;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

/// `[perf]` in `aio.toml`, in milliseconds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(deny_unknown_fields)]
pub struct Budgets {
    /// A method's own time. Unset: one frame (16) in a debug build, 100 in
    /// a release build ("feels instant").
    pub method: Option<u64>,
    /// One effect step. Unset: 5.
    pub effect: Option<u64>,
    /// How long each await of an async method may take, unless the method
    /// is `#[long]` (aio's `effectTimeoutMs`). Unset: 30000.
    pub call_ceiling: Option<u64>,
    /// Per method, `"cell.method" = ms`: one meant to be slow, while every
    /// other stays strict.
    #[serde(default)]
    pub methods: BTreeMap<String, u64>,
    /// Per effect, `"cell.name" = ms` (the name its key starts with).
    #[serde(default)]
    pub effects: BTreeMap<String, u64>,
    /// `[perf.storm]`: a runaway loop of calls.
    #[serde(default)]
    pub storm: Storm,
}

/// A method called `rate` times a second or more, `secs` seconds in a row, is
/// a storm (a feedback loop: a watcher that sees its own writes): said once
/// when it starts and once when it ends. With `breaker`, its calls are
/// refused while it lasts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(deny_unknown_fields, default)]
pub struct Storm {
    pub rate: u32,
    pub secs: u32,
    pub breaker: bool,
}

impl Default for Storm {
    fn default() -> Self {
        Self { rate: 200, secs: 5, breaker: false }
    }
}

impl Budgets {
    /// The await ceiling of async methods ([`Self::call_ceiling`], at least 1 ms).
    pub fn ceiling(&self) -> Duration {
        Duration::from_millis(self.call_ceiling.unwrap_or(30_000).max(1))
    }

    /// The budget of method `label` (`cell.method`).
    pub fn method(&self, label: &str) -> Duration {
        let default = if cfg!(debug_assertions) { 16 } else { 100 };
        Duration::from_millis(self.methods.get(label).copied().or(self.method).unwrap_or(default))
    }

    /// The budget of one step of effect `label` (`cell.name`).
    pub fn effect(&self, label: &str) -> Duration {
        Duration::from_millis(self.effects.get(label).copied().or(self.effect).unwrap_or(5))
    }

    /// The `[perf.methods]` keys for `cell` that name none of its `methods`:
    /// a typo would budget nothing.
    pub fn unknown(&self, cell: &str, methods: &[&str]) -> Vec<String> {
        let mine = self.methods.keys().filter(|k| k.split_once('.').is_some_and(|(c, _)| c == cell));
        mine.filter(|k| !methods.contains(&&k[cell.len() + 1..])).cloned().collect()
    }
}

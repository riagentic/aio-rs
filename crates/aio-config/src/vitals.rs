use aio_macros::Lit;
use serde::{Deserialize, Serialize};
use std::time::Duration;
/// How a layer is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Healthy,
    Degraded,
    Warning,
    Frozen,
    /// Back to healthy after a warning or a freeze (a step, never a grade).
    Recovered,
}

/// A time layer's thresholds: from each of these on, it is degraded,
/// warning, frozen. Spans in `aio.toml` (`"200ms"`, `"2s"`), or a number
/// of ms as before.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(deny_unknown_fields)]
pub struct Tier {
    #[serde(with = "crate::config::ms_span")]
    pub degraded: Duration,
    #[serde(with = "crate::config::ms_span")]
    pub warning: Duration,
    #[serde(with = "crate::config::ms_span")]
    pub frozen: Duration,
}

impl Tier {
    /// What `ms` late is (pure).
    pub fn grade(&self, ms: u64) -> Status {
        let at = |d: Duration| u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
        grade([at(self.degraded), at(self.warning), at(self.frozen)], ms)
    }
}

/// The effect queue's thresholds: from this many effects waiting on, it
/// is degraded, warning, frozen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(deny_unknown_fields)]
pub struct Counts {
    pub degraded: u64,
    pub warning: u64,
    pub frozen: u64,
}

impl Counts {
    /// What `n` waiting is (pure).
    pub fn grade(&self, n: u64) -> Status {
        grade([self.degraded, self.warning, self.frozen], n)
    }
}

/// `measured` against `[degraded, warning, frozen]` (pure).
fn grade([degraded, warning, frozen]: [u64; 3], measured: u64) -> Status {
    match measured {
        m if m >= frozen => Status::Frozen,
        m if m >= warning => Status::Warning,
        m if m >= degraded => Status::Degraded,
        _ => Status::Healthy,
    }
}

/// `[vitals]` in `aio.toml` (aio's `vitals: { thresholds, hints }`); its
/// defaults are aio's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(deny_unknown_fields, default)]
pub struct Vitals {
    /// Say a hint (the likely cause) with each warning and freeze.
    pub hints: bool,
    pub render: Tier,
    pub transport: Tier,
    #[serde(rename = "loop")]
    pub event_loop: Tier,
    pub queue: Counts,
}

impl Default for Vitals {
    fn default() -> Self {
        let ms = Duration::from_millis;
        let t = |degraded, warning, frozen| Tier { degraded: ms(degraded), warning: ms(warning), frozen: ms(frozen) };
        Self {
            hints: true,
            render: t(50, 200, 2000),
            transport: t(100, 500, 2000),
            event_loop: t(100, 500, 2000),
            queue: Counts { degraded: 50, warning: 200, frozen: 1000 },
        }
    }
}

impl Vitals {
    /// Each layer needs degraded < warning < frozen.
    pub fn check(&self) -> Result<(), String> {
        let rising = |[d, w, f]: [u128; 3]| d < w && w < f;
        let time = |t: Tier| [t.degraded, t.warning, t.frozen].map(|d| d.as_millis());
        let q = self.queue;
        let layers = [
            ("render", time(self.render)),
            ("transport", time(self.transport)),
            ("loop", time(self.event_loop)),
            ("queue", [q.degraded, q.warning, q.frozen].map(u128::from)),
        ];
        match layers.iter().find(|(_, t)| !rising(*t)) {
            Some((name, _)) => Err(format!("aio.toml: [vitals.{name}] needs degraded < warning < frozen")),
            None => Ok(()),
        }
    }
}

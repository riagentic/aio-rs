use aio_macros::Lit;
use serde::{Deserialize, Serialize};
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

/// One layer's thresholds: from each of these on (ms, or effects for
/// `queue`), it is degraded, warning, frozen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(deny_unknown_fields)]
pub struct Tier {
    pub degraded: u64,
    pub warning: u64,
    pub frozen: u64,
}

impl Tier {
    /// What `measured` is (pure).
    pub fn grade(&self, measured: u64) -> Status {
        match measured {
            m if m >= self.frozen => Status::Frozen,
            m if m >= self.warning => Status::Warning,
            m if m >= self.degraded => Status::Degraded,
            _ => Status::Healthy,
        }
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
    pub queue: Tier,
}

impl Default for Vitals {
    fn default() -> Self {
        let t = |degraded, warning, frozen| Tier { degraded, warning, frozen };
        Self {
            hints: true,
            render: t(50, 200, 2000),
            transport: t(100, 500, 2000),
            event_loop: t(100, 500, 2000),
            queue: t(50, 200, 1000),
        }
    }
}

impl Vitals {
    /// Each layer needs degraded < warning < frozen.
    pub fn check(&self) -> Result<(), String> {
        let layers =
            [("render", self.render), ("transport", self.transport), ("loop", self.event_loop), ("queue", self.queue)];
        match layers.iter().find(|(_, t)| !(t.degraded < t.warning && t.warning < t.frozen)) {
            Some((name, _)) => Err(format!("aio.toml: [vitals.{name}] needs degraded < warning < frozen")),
            None => Ok(()),
        }
    }
}

//! The call context: the only way a method sees the outside world. Live, it
//! records each id and random number it hands out; on replay it hands back
//! the recorded ones, so a replayed event computes the same state.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{any::Any, cell::RefCell};
use uuid::Uuid;

/// Who caused an event.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "name", rename_all = "lowercase")]
pub enum By {
    #[default]
    System,
    User(String),
    Agent(String),
    Schedule(String),
    Effect(String),
    Test,
}

/// What a call read from the outside world, saved with its event.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inputs {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ids: Vec<Uuid>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rands: Vec<u64>,
    /// The caller's role, when the method read it ([`Cx::role`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

impl Inputs {
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty() && self.rands.is_empty() && self.role.is_none()
    }
}

/// A synced call's id: a UUIDv7 made by the client. Its time bounds it: one
/// from a clock far ahead, or older than the kept call ids, is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Op(Uuid);

impl Default for Op {
    fn default() -> Self {
        Self::new()
    }
}

impl Op {
    /// Calls made offline may wait this long before the server refuses them.
    pub const MAX_AGE_MS: i64 = 30 * 86_400_000;
    /// A client clock at most this far ahead is accepted.
    pub const MAX_AHEAD_MS: i64 = 5 * 60_000;

    pub fn new() -> Self {
        Self(crate::uuid_v7())
    }

    pub fn ms(&self) -> i64 {
        self.0.get_timestamp().map_or(0, |t| {
            let (s, n) = t.to_unix();
            (s as i64) * 1000 + i64::from(n / 1_000_000)
        })
    }

    pub fn parse(text: &str) -> Option<Self> {
        Uuid::parse_str(text).ok().filter(|u| u.get_version_num() == 7).map(Self)
    }

    /// Parse and check against the server's clock `now`.
    pub fn check(text: &str, now: i64) -> Result<Self, String> {
        let op = Self::parse(text).ok_or("a call id must be a UUIDv7")?;
        if op.ms() > now + Self::MAX_AHEAD_MS {
            return Err(format!(
                "this device's clock is {} s ahead of the server's: fix the clock",
                (op.ms() - now + 500) / 1000
            ));
        }
        if op.ms() < now - Self::MAX_AGE_MS {
            return Err("this call waited offline too long (over 30 days)".into());
        }
        Ok(op)
    }
}

impl std::fmt::Display for Op {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

struct Derive {
    seed: [u8; 32],
    ms: u64,
}

impl Derive {
    fn new(cell: &str, by: &By, op: &Op) -> Self {
        let by = serde_json::to_string(by).unwrap_or_default();
        let seed = crate::sha256::Sha256::digest(format!("aio-ids\0{cell}\0{by}\0{op}").as_bytes());
        Self { seed, ms: u64::try_from(op.ms()).unwrap_or(0) }
    }

    /// The `n`th id: a UUIDv7 at the call's time, its other bits hashed.
    fn id(&self, n: usize) -> Uuid {
        let mut ctx = crate::sha256::Sha256::new();
        ctx.update(self.seed);
        ctx.update((n as u64).to_le_bytes());
        let h = ctx.finalize();
        let bits: [u8; 10] = h[..10].try_into().expect("10 bytes");
        uuid::Builder::from_unix_timestamp_millis(self.ms, &bits).into_uuid()
    }
}

/// A schedule change asked for by a method; saved with its event.
#[derive(Clone, Debug, PartialEq)]
pub enum Plan {
    Set { key: String, due: i64, every: Option<i64>, cron: Option<String>, method: String, args: Value },
    Cancel(String),
}

/// One thing a call asks for after it commits (an effect, an abort, a
/// tell); in a page an effect needn't be `Send` (`effect::Fut`).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) type Asked = Box<dyn Any + Send>;
#[cfg(target_arch = "wasm32")]
pub(crate) type Asked = Box<dyn Any>;

/// What a live call asked for besides its state change.
#[derive(Default)]
pub(crate) struct Asks {
    pub inputs: Inputs,
    pub effects: Vec<Asked>,
    pub plans: Vec<Plan>,
    /// The caller's id for this call, kept so a re-sent call applies once.
    pub op: Option<String>,
}

pub struct Cx {
    at: i64,
    by: By,
    /// The network caller's role now (accounts on), as the gate proved it.
    role: Option<String>,
    replay: Option<Inputs>,
    /// A synced call's ids come from its call id (`derive_ids`), not chance.
    derive: Option<Derive>,
    used: RefCell<Inputs>,
    effects: RefCell<Vec<Asked>>,
    plans: RefCell<Vec<Plan>>,
}

impl Cx {
    pub(crate) fn live(at: i64, by: &By) -> Self {
        Self {
            at,
            by: by.clone(),
            role: None,
            replay: None,
            derive: None,
            used: RefCell::default(),
            effects: RefCell::default(),
            plans: RefCell::default(),
        }
    }

    /// A synced call `op` by `by`: its ids are derived, so the client that
    /// predicts it and the server that runs it make the same ones, and no
    /// client can choose them (and so overwrite another's row).
    pub(crate) fn synced(at: i64, cell: &str, by: &By, op: &Op) -> Self {
        Self { derive: Some(Derive::new(cell, by, op)), ..Self::live(at, by) }
    }

    /// On replay, effects and schedule changes are dropped: their results
    /// and rows were saved when the event first ran.
    pub(crate) fn replay(at: i64, inputs: Inputs, by: &By) -> Self {
        Self { replay: Some(inputs), ..Self::live(at, by) }
    }

    /// The call made by a network caller with this role.
    pub(crate) fn with_role(self, role: Option<&str>) -> Self {
        Self { role: role.map(Into::into), ..self }
    }

    /// The role, unrecorded: for [`crate::before_call`], never the method.
    pub(crate) fn caller_role(&self) -> Option<&str> {
        self.role.as_deref()
    }

    pub(crate) fn into_asks(self) -> Asks {
        Asks {
            inputs: self.used.into_inner(),
            effects: self.effects.into_inner(),
            plans: self.plans.into_inner(),
            op: None,
        }
    }

    /// Start an effect after this call commits (never if it is refused).
    pub fn run<T: crate::Cell>(&self, effect: crate::Effect<T>) {
        self.effects.borrow_mut().push(Box::new(effect));
    }

    /// Tell the people using the app `t` after this call commits (aio
    /// `notify()`, `appIcon()`): shown by this cell's window and every synced
    /// client's ([`crate::tell`]). A blank notification is refused here.
    pub fn tell(&self, t: crate::tell::Tell) -> crate::Result {
        t.check()?;
        self.effects.borrow_mut().push(Box::new(t));
        Ok(())
    }

    /// Stop the running and queued effects with this key after this call
    /// commits (a cancel button, or a call that makes their work moot):
    /// their tasks are dropped at their next await, so they call back
    /// nothing; one not started yet (held by `TestCells`) never starts.
    pub fn abort(&self, key: &str) {
        self.effects.borrow_mut().push(Box::new(crate::effect::Abort(key.into())));
    }

    /// Call `method(args)` once, `ms` from now. The same key replaces
    /// (debounce, autosave, retry). In every schedule, a `method` the cell
    /// doesn't have refuses the call that sets it.
    pub fn after(&self, key: &str, ms: i64, method: &str, args: impl Serialize) -> crate::Result {
        self.plan(key, self.at + ms.max(0), None, method, args)
    }

    /// Call `method(args)` every `ms`, first `ms` from now. Runs missed while
    /// the app was off fire once.
    pub fn every(&self, key: &str, ms: i64, method: &str, args: impl Serialize) -> crate::Result {
        crate::ensure!(ms > 0, "schedule `{key}`: interval must be > 0");
        self.plan(key, self.at + ms, Some(ms), method, args)
    }

    /// Call `method(args)` at `at` (ms since the epoch).
    pub fn at(&self, key: &str, at: i64, method: &str, args: impl Serialize) -> crate::Result {
        self.plan(key, at, None, method, args)
    }

    /// Call `method(args)` on a cron schedule (`"0 9 * * 1"`: Mondays at 9),
    /// in this machine's local time (`aio::cron` has the clock rules).
    pub fn cron(&self, key: &str, expr: &str, method: &str, args: impl Serialize) -> crate::Result {
        let c = crate::cron::Cron::parse(expr).map_err(crate::Rejected::new)?;
        let due = c.next(self.at).ok_or_else(|| crate::Rejected::new(format!("cron `{expr}` never runs")))?;
        let args = crate::json(&args)?;
        let (key, method, cron) = (key.into(), method.into(), Some(expr.into()));
        self.plans.borrow_mut().push(Plan::Set { key, due, every: None, cron, method, args });
        Ok(())
    }

    /// aio's `schedule.backoff` / `poll`: `method(args)` once, after
    /// [`Backoff::ms`] for this `attempt`: a retry, or a poll, that waits
    /// longer each time. The same key replaces.
    pub fn backoff(&self, key: &str, attempt: u32, b: Backoff, method: &str, args: impl Serialize) -> crate::Result {
        self.after(key, b.ms(attempt), method, args)
    }

    pub fn cancel(&self, key: &str) {
        self.plans.borrow_mut().push(Plan::Cancel(key.into()));
    }

    fn plan(&self, key: &str, due: i64, every: Option<i64>, method: &str, args: impl Serialize) -> crate::Result {
        let args = crate::json(&args)?;
        self.plans.borrow_mut().push(Plan::Set {
            key: key.into(),
            due,
            every,
            cron: None,
            method: method.into(),
            args,
        });
        Ok(())
    }

    /// Who made this call: on a synced server, the signed-in user (with
    /// accounts on, proven by their session, never what a client says).
    /// Saved with the event, so replay sees the same caller.
    pub fn by(&self) -> &By {
        &self.by
    }

    /// The caller's role (aio's `auth.role`): with accounts on, the signed-in
    /// user's role at this call, read from the account, never what a client
    /// says; `None` for the app's own calls, a key holder and the door.
    /// Saved with the event once read, so replay sees the same role.
    pub fn role(&self) -> Option<&str> {
        let role = self.replay.as_ref().map_or(self.role.as_deref(), |r| r.role.as_deref());
        self.used.borrow_mut().role = role.map(Into::into);
        role
    }

    /// The call's time, in ms since the Unix epoch: one value per call.
    pub fn now(&self) -> i64 {
        self.at
    }

    /// A new UUIDv7.
    pub fn id(&self) -> Uuid {
        let mut u = self.used.borrow_mut();
        let n = u.ids.len();
        let id = self.replay.as_ref().and_then(|r| r.ids.get(n).copied()).unwrap_or_else(|| match &self.derive {
            Some(d) => d.id(n),
            None => crate::uuid_v7(),
        });
        u.ids.push(id);
        id
    }

    /// A random number.
    pub fn rand(&self) -> u64 {
        let mut u = self.used.borrow_mut();
        let n = self
            .replay
            .as_ref()
            .and_then(|r| r.rands.get(u.rands.len()).copied())
            .unwrap_or_else(|| crate::uuid_v4().as_u64_pair().0);
        u.rands.push(n);
        n
    }
}

/// How long a retry waits: `base · factorᵃᵗᵗᵉᵐᵖᵗ` ms, at most `max` (aio:
/// `backoff` doubles, `poll` keeps its interval: `factor` 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Backoff {
    pub base: i64,
    pub factor: f64,
    pub max: i64,
}

impl Backoff {
    /// Doubling from `base` ms, at most an hour.
    pub fn from(base: i64) -> Self {
        Self { base, factor: 2.0, max: 3_600_000 }
    }

    /// The wait before try `attempt` (0: the first), in ms (at least 1).
    pub fn ms(self, attempt: u32) -> i64 {
        let ms = self.base.max(1) as f64 * self.factor.max(1.0).powi(attempt.min(1024) as i32);
        (ms.min(self.max.max(1) as f64).round() as i64).max(1)
    }
}

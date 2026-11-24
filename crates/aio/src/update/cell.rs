//! What an app sees of its updates, and the verbs a UI needs (aio's
//! `updates` cell): is a release available or blocked, how far its install
//! is, check now, install now, not this one. The I/O behind the verbs is a
//! [`Runtime`] (`aio_shell` installs the signed-manifest one; an app may
//! bring its own transport), so the cell stays plain data: every answer comes
//! back as a method call, and the log replays it.
use super::UpdateError;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};

/// Where an install stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    #[default]
    Idle,
    Checking,
    Available,
    Blocked,
    Downloading,
    /// Backing the data up and handing over to the swap helper.
    Installing,
    /// Done: the app stops, and the new version starts with the same args.
    Restarting,
    Error,
}

/// A release this app could move to.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    pub version: String,
    /// Why it is offered, in the words the user sees.
    pub reason: String,
    pub notes: String,
    /// Bytes to download (0 once staged).
    pub size: u64,
    pub released_at: String,
    /// The key that signed it (`ed25519:<base64>`).
    pub key: String,
    /// Downloaded and checked already: it installs the next time the app
    /// starts, or now with `apply`.
    pub staged: bool,
    /// It moves the data here to a newer version of a cell (a backup is
    /// taken first): a UI may say so before the person installs it.
    pub migrates: bool,
}

/// A newer release that can't read the data here, and why.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blocked {
    pub version: String,
    pub blockers: Vec<String>,
}

/// What a look for a release found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Found {
    /// Nothing to offer, and why, in the words the user sees.
    Current {
        reason: String,
    },
    Offer(Offer),
    Blocked(Blocked),
}

/// An install's steps, as the runtime reports them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Percent of the download.
    Downloaded(u8),
    /// Percent of the Electron runtime the new version needs, downloaded
    /// after it (once per machine).
    Runtime(u8),
    Installing,
}

/// Which download `Updates::progress` measures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProgressOf {
    /// The release itself.
    App,
    /// The Electron runtime it needs: `progress` counts from 0 again.
    Runtime,
}

/// How an install treats the data here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Data {
    /// The data gate holds: a release that can't read it is not installed.
    #[default]
    Gated,
    /// Installed over the gate's blockers: the data stays, backed up first,
    /// and the new version tries to read it (aio's `acceptDataLoss`).
    AcceptLoss,
    /// Start fresh: the data is moved aside whole, never deleted, and the
    /// new version starts on none (aio's `retireData`).
    Retire,
}

/// The I/O behind the cell. Both calls block (they run off the cell).
pub trait Runtime: Send + Sync + 'static {
    /// Look for a release once.
    fn check(&self) -> std::result::Result<Found, UpdateError>;
    /// Download (if needed), check and install `version` now, its `data`
    /// that way; the app stops after, and the new version starts with the
    /// same arguments.
    fn apply(&self, version: &str, data: Data, step: &dyn Fn(Step)) -> std::result::Result<(), UpdateError>;
}

/// The app's say on an install now (aio's `canApply`), awaited: `Err(why)`
/// refuses it (mid-recording, a sync not done).
type Veto = Arc<
    dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = std::result::Result<(), String>> + Send>>
        + Send
        + Sync,
>;

static VETO: RwLock<Option<Veto>> = RwLock::new(None);

/// Await `veto` before every install from now on (aio's `canApply`): the
/// cell's `apply`, `apply_anyway` and `start_fresh`, a server's unattended
/// install, and a staged release's install at the next start (set it in
/// `main`, before `aio_shell::start`). It runs off the cell, so it may do
/// work first (a backup, a sync to finish); `Err(why)` is "not now".
pub fn can_apply<F, Fut>(veto: F)
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = std::result::Result<(), String>> + Send + 'static,
{
    *VETO.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(move || Box::pin(veto())));
}

/// What the app's veto says to an install now; `None`: it set none.
pub async fn veto() -> Option<std::result::Result<(), String>> {
    let v = VETO.read().unwrap_or_else(|e| e.into_inner()).clone();
    match v {
        Some(f) => Some(f().await),
        None => None,
    }
}

/// What the runtime says at start.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ready {
    pub channel: String,
    /// The running version.
    pub current: String,
    /// A release that failed its first start here: never offered again.
    pub rolled_back: Option<String>,
    /// The data's backup the update to this version took (a fresh start's:
    /// the store it set aside).
    pub backup: Option<String>,
    /// Where a fresh start to this version set the data aside (aio's
    /// `retireData` archive, `<home>/archive/<id>-<version>-<time>`).
    pub retired: Option<String>,
}

/// A stand-in runtime for tests (aio's stub `installUpdatesRuntime`): every
/// check finds what it holds; an install steps its download to 100 % and installs,
/// touching nothing. `install(Arc::new(Stub(found)))`, then drive the
/// app's own update UI.
pub struct Stub(pub Found);

impl Runtime for Stub {
    fn check(&self) -> std::result::Result<Found, UpdateError> {
        Ok(self.0.clone())
    }
    fn apply(&self, _version: &str, _data: Data, step: &dyn Fn(Step)) -> std::result::Result<(), UpdateError> {
        [50, 100].into_iter().for_each(|p| step(Step::Downloaded(p)));
        step(Step::Installing);
        Ok(())
    }
}

pub(super) static RUNTIME: RwLock<Option<Arc<dyn Runtime>>> = RwLock::new(None);

/// The runtime the cell uses from now on (one per process).
pub fn install(rt: Arc<dyn Runtime>) {
    *RUNTIME.write().unwrap_or_else(|e| e.into_inner()) = Some(rt);
}

/// An update runtime is installed (the app's own, before `aio_shell` starts).
pub fn installed() -> bool {
    RUNTIME.read().unwrap_or_else(|e| e.into_inner()).is_some()
}

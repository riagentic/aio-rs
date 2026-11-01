//! What an app sees of its updates, and the verbs a UI needs (aio's
//! `updates` cell): is a release available or blocked, how far its install
//! is, check now, install now, not this one. The I/O behind the verbs is a
//! [`Runtime`] (`aio_shell` installs the signed-manifest one; an app may
//! bring its own transport), so the cell stays plain data: every answer comes
//! back as a method call, and the log replays it.
use crate::prelude::*;
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
    pub notes: String,
    /// Bytes to download (0 once staged).
    pub size: u64,
    pub released_at: String,
    /// The key that signed it (`ed25519:<base64>`).
    pub key: String,
    /// Downloaded and checked already: it installs the next time the app
    /// starts, or now with `apply`.
    pub staged: bool,
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
    Installing,
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
    fn check(&self) -> std::result::Result<Found, String>;
    /// Download (if needed), check and install `version` now, its `data`
    /// that way; the app stops after, and the new version starts with the
    /// same arguments.
    fn apply(&self, version: &str, data: Data, step: &dyn Fn(Step)) -> std::result::Result<(), String>;
}

/// The app's say on an install now (aio's `canApply`): `Err(why)` refuses it
/// (mid-recording, a sync not done), asked before every `apply`.
pub type Veto = fn() -> std::result::Result<(), String>;

static VETO: RwLock<Option<Veto>> = RwLock::new(None);

/// Ask `veto` before every install from now on.
pub fn can_apply(veto: Veto) {
    *VETO.write().unwrap_or_else(|e| e.into_inner()) = Some(veto);
}

fn vetoed() -> Option<String> {
    let v = *VETO.read().unwrap_or_else(|e| e.into_inner());
    v.and_then(|f| f().err())
}

/// What the runtime says at start.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ready {
    pub channel: String,
    /// The running version.
    pub current: String,
    /// A release that failed its first start here: never offered again.
    pub rolled_back: Option<String>,
    /// The data's backup the update to this version took.
    pub backup: Option<String>,
}

/// A stand-in runtime for tests (aio's stub `installUpdatesRuntime`): every
/// check finds what it holds; an install steps its download to 100 % and installs,
/// touching nothing. `install(Arc::new(Stub(found)))`, then drive the
/// app's own update UI.
pub struct Stub(pub Found);

impl Runtime for Stub {
    fn check(&self) -> std::result::Result<Found, String> {
        Ok(self.0.clone())
    }
    fn apply(&self, _version: &str, _data: Data, step: &dyn Fn(Step)) -> std::result::Result<(), String> {
        [50, 100].into_iter().for_each(|p| step(Step::Downloaded(p)));
        step(Step::Installing);
        Ok(())
    }
}

static RUNTIME: RwLock<Option<Arc<dyn Runtime>>> = RwLock::new(None);

/// The runtime the cell uses from now on (one per process).
pub fn install(rt: Arc<dyn Runtime>) {
    *RUNTIME.write().unwrap_or_else(|e| e.into_inner()) = Some(rt);
}

fn runtime() -> std::result::Result<Arc<dyn Runtime>, String> {
    RUNTIME.read().unwrap_or_else(|e| e.into_inner()).clone().ok_or_else(|| NOT_CONFIGURED.to_string())
}

const NOT_CONFIGURED: &str = "updates are not configured for this app: set [updates] url and keys in aio.toml";

/// Run a blocking runtime call off the cell.
async fn blocking<R: Send + 'static>(
    f: impl FnOnce(Arc<dyn Runtime>) -> std::result::Result<R, String> + Send + 'static,
) -> std::result::Result<R, String> {
    let rt = runtime()?;
    tokio::task::spawn_blocking(move || f(rt)).await.map_err(|e| format!("the update runtime failed: {e}"))?
}

/// The app's updates. Only `dismissed` is saved: the rest is learned again
/// each run.
#[cell(persist, id = "updates")]
pub struct Updates {
    /// A runtime is set up (`ready`).
    #[transient]
    pub on: bool,
    #[transient]
    pub channel: String,
    /// The running version.
    #[transient]
    pub current: String,
    #[transient]
    pub phase: Phase,
    #[transient]
    pub available: Option<Offer>,
    #[transient]
    pub blocked: Option<Blocked>,
    /// Percent of the download.
    #[transient]
    pub progress: u8,
    /// When the last check answered (ms).
    #[transient]
    pub checked_at: Option<i64>,
    #[transient]
    pub error: Option<String>,
    /// The data's backup the update to this version took.
    #[transient]
    pub backup: Option<String>,
    /// The install that failed: (version, why), said again while offered.
    #[transient]
    pub failed: Option<(String, String)>,
    /// The release the user said no to: never offered, a newer one is.
    pub dismissed: Option<String>,
    /// The channel the user chose (`set_channel`): followed from now on,
    /// unless a run says otherwise (`AIORT_UPDATE_CHANNEL`).
    pub pinned: Option<String>,
}

impl Updates {
    fn busy(&self) -> bool {
        matches!(self.phase, Phase::Downloading | Phase::Installing | Phase::Restarting)
    }

    /// Install the release offered (or, past the gate, the one blocked).
    fn install(&mut self, cx: &Cx, data: Data) -> Result {
        ensure!(self.on, "{NOT_CONFIGURED}");
        ensure!(!self.busy(), "an update is already being installed ({:?})", self.phase);
        let version = match (&self.available, &self.blocked, data) {
            (Some(o), _, _) => o.version.clone(),
            (None, Some(b), Data::AcceptLoss | Data::Retire) => b.version.clone(),
            (None, Some(b), Data::Gated) => reject!(
                "{} is blocked: {}; it is not installed, because the data here may not survive it \
                 (apply_anyway keeps the data and tries, start_fresh sets it aside)",
                b.version,
                b.blockers.join("; ")
            ),
            (None, None, _) => reject!("no update is available to apply: check first"),
        };
        if let Some(why) = vetoed() {
            reject!("the app says not now: {why}");
        }
        self.phase = Phase::Downloading;
        self.progress = 0;
        (self.error, self.failed) = (None, None);
        cx.run(
            Effect::new("apply", move |h: Handle<Updates>| async move {
                let report = h.clone();
                let r = blocking(move |rt| {
                    rt.apply(&version, data, &|s| {
                        _ = match s {
                            Step::Downloaded(p) => report.progress(p),
                            Step::Installing => report.installing(),
                        };
                    })
                })
                .await;
                _ = h.applied(r);
            })
            .mode(Mode::First),
        );
        Ok(())
    }
}

#[methods]
impl Updates {
    /// The runtime is set up.
    pub fn ready(&mut self, r: Ready) {
        self.on = true;
        (self.channel, self.current, self.backup) = (r.channel, r.current, r.backup);
        if let Some(v) = r.rolled_back {
            if self.available.as_ref().is_some_and(|o| o.version == v) {
                self.available = None;
                self.phase = Phase::Idle;
            }
            self.dismissed = Some(v);
        }
    }

    /// Look for a release now. A check already running is joined, never
    /// run twice.
    pub fn check(&mut self, cx: &Cx) -> Result {
        ensure!(self.on, "{NOT_CONFIGURED}");
        ensure!(!self.busy(), "an update is being installed ({:?}): no check until it is done", self.phase);
        self.phase = Phase::Checking;
        self.error = None;
        cx.run(
            Effect::new("check", |h: Handle<Updates>| async move {
                let r = blocking(|rt| rt.check()).await;
                _ = h.checked(r);
            })
            .mode(Mode::First),
        );
        Ok(())
    }

    /// A check's answer (the runtime's, or the app's own poll).
    pub fn checked(&mut self, cx: &Cx, r: std::result::Result<Found, String>) {
        self.checked_at = Some(cx.now());
        // An install under way keeps its offer.
        if self.busy() {
            return;
        }
        let r = match r {
            Ok(Found::Offer(o)) if self.dismissed.as_ref() == Some(&o.version) => {
                Ok(Found::Current { reason: format!("{} was dismissed", o.version) })
            }
            Ok(Found::Blocked(b)) if self.dismissed.as_ref() == Some(&b.version) => {
                Ok(Found::Current { reason: format!("{} was dismissed", b.version) })
            }
            r => r,
        };
        (self.available, self.blocked) = (None, None);
        match r {
            Err(e) => {
                self.phase = Phase::Error;
                self.error = Some(e);
            }
            Ok(Found::Current { .. }) => self.phase = Phase::Idle,
            Ok(Found::Blocked(b)) => {
                self.phase = Phase::Blocked;
                self.blocked = Some(b);
            }
            Ok(Found::Offer(o)) => {
                self.phase = Phase::Available;
                // A refused install stays said while its release is offered.
                self.error = self.failed.as_ref().filter(|(v, _)| *v == o.version).map(|(_, e)| e.clone());
                self.available = Some(o);
            }
        }
    }

    /// Install the offered release now: the app stops, and the new version
    /// starts with the same arguments.
    pub fn apply(&mut self, cx: &Cx) -> Result {
        self.install(cx, Data::Gated)
    }

    /// Install the release the data gate blocks anyway: the data is backed
    /// up, and the new version tries to read it (the backup is the way back).
    pub fn apply_anyway(&mut self, cx: &Cx) -> Result {
        self.install(cx, Data::AcceptLoss)
    }

    /// Install the release (blocked or not) on fresh data: what is here now
    /// is moved aside whole, never deleted, and comes back if it fails.
    pub fn start_fresh(&mut self, cx: &Cx) -> Result {
        self.install(cx, Data::Retire)
    }

    /// The download's progress, in percent.
    pub fn progress(&mut self, percent: u8) {
        if self.phase == Phase::Downloading {
            self.progress = percent.min(100);
        }
    }

    pub fn installing(&mut self) {
        if self.busy() {
            (self.phase, self.progress) = (Phase::Installing, 100);
        }
    }

    /// An install's end: the app restarts, or why it can't.
    pub fn applied(&mut self, r: std::result::Result<(), String>) {
        match r {
            Ok(()) => (self.phase, self.progress) = (Phase::Restarting, 100),
            Err(e) => {
                self.phase = Phase::Error;
                let v = self.available.as_ref().map(|o| &o.version).or(self.blocked.as_ref().map(|b| &b.version));
                self.failed = v.map(|v| (v.clone(), e.clone()));
                self.error = Some(e);
            }
        }
    }

    /// Not this release (available or blocked): a newer one is still offered.
    pub fn dismiss(&mut self) {
        let Some(v) = self.available.as_ref().map(|o| &o.version).or(self.blocked.as_ref().map(|b| &b.version)) else {
            return;
        };
        self.dismissed = Some(v.clone());
        (self.available, self.blocked, self.error) = (None, None, None);
        self.phase = Phase::Idle;
    }

    /// Offer the dismissed release again (at the next check).
    pub fn undismiss(&mut self) {
        (self.dismissed, self.error) = (None, None);
    }

    /// Follow `channel` from now on (the next check asks it). What the old
    /// channel offered or was told no goes: crossing channels may even move
    /// the version back, and a no on one says nothing about another.
    pub fn set_channel(&mut self, channel: String) -> Result {
        if let Some(why) = crate::update::channel_refused(&channel) {
            reject!("{why}");
        }
        ensure!(!self.busy(), "an update is being installed ({:?}): the channel stays until it is done", self.phase);
        (self.available, self.blocked, self.error, self.failed, self.dismissed) = (None, None, None, None, None);
        self.phase = Phase::Idle;
        self.pinned = Some(channel.clone());
        self.channel = channel;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;

    /// One runtime per process: tests that install one take turns.
    static TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// A runtime that answers `found`, and installs after `steps`.
    struct Stub {
        found: Mutex<Found>,
        checks: Mutex<u32>,
        applies: Mutex<Vec<(String, Data)>>,
        fail: Option<String>,
        slow: Duration,
    }

    impl Runtime for Stub {
        fn check(&self) -> std::result::Result<Found, String> {
            *self.checks.lock().unwrap() += 1;
            std::thread::sleep(self.slow);
            Ok(self.found.lock().unwrap().clone())
        }
        fn apply(&self, version: &str, data: Data, step: &dyn Fn(Step)) -> std::result::Result<(), String> {
            self.applies.lock().unwrap().push((version.into(), data));
            std::thread::sleep(self.slow);
            for p in [30, 70, 100] {
                step(Step::Downloaded(p));
            }
            step(Step::Installing);
            self.fail.clone().map_or(Ok(()), Err)
        }
    }

    fn offer(v: &str) -> Found {
        Found::Offer(Offer { version: v.into(), size: 10, ..Offer::default() })
    }

    fn stub(found: Found, fail: Option<&str>, slow: u64) -> Arc<Stub> {
        let s = Arc::new(Stub {
            found: Mutex::new(found),
            checks: Mutex::new(0),
            applies: Mutex::new(vec![]),
            fail: fail.map(String::from),
            slow: Duration::from_millis(slow),
        });
        install(s.clone());
        s
    }

    fn on(channel: &str, rolled_back: Option<&str>) -> Ready {
        Ready {
            channel: channel.into(),
            current: "1.0.0".into(),
            rolled_back: rolled_back.map(String::from),
            backup: None,
        }
    }

    fn ready() -> Handle<Updates> {
        let h = Handle::<Updates>::memory();
        h.ready(on("stable", None)).unwrap();
        h
    }

    async fn check(h: &Handle<Updates>) {
        h.check().unwrap();
        h.done("check").await;
    }

    #[test]
    fn off_until_a_runtime_is_ready() {
        let h = Handle::<Updates>::memory();
        assert!(!h.state().on);
        assert!(h.check().unwrap_err().why.contains("not configured"));
        assert!(h.apply().unwrap_err().why.contains("not configured"));
        h.ready(Ready { backup: Some("/data/backups/pre-1.0.0.db".into()), ..on("beta", None) }).unwrap();
        let s = h.state();
        assert_eq!((s.on, s.channel.as_str(), s.current.as_str(), s.phase), (true, "beta", "1.0.0", Phase::Idle));
        assert_eq!(s.backup.as_deref(), Some("/data/backups/pre-1.0.0.db"), "the update's backup is seen");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_channel_switch_is_kept_and_forgets_what_the_old_one_offered() {
        let _turn = TURN.lock().await;
        stub(offer("1.1.0"), None, 100);
        let h = ready();
        check(&h).await;
        h.dismiss().unwrap();
        h.set_channel("beta".into()).unwrap();
        let s = h.state();
        assert_eq!((s.channel.as_str(), s.pinned.as_deref(), s.phase), ("beta", Some("beta"), Phase::Idle));
        assert_eq!((s.available.as_ref(), s.dismissed.as_ref()), (None, None));
        assert!(h.set_channel("../x".into()).unwrap_err().why.contains("not a channel name"));
        check(&h).await;
        h.apply().unwrap();
        assert!(h.set_channel("stable".into()).unwrap_err().why.contains("being installed"));
        h.done("apply").await;
        assert_eq!(h.state().pinned.as_deref(), Some("beta"), "a refused switch changes nothing");
    }

    #[test]
    fn the_channel_is_the_runs_then_the_users_then_the_builds_then_aio_toml() {
        use crate::update::channel;
        assert_eq!(channel(Some("dev"), Some("beta"), Some("rc"), "stable"), "dev");
        assert_eq!(channel(None, Some("beta"), Some("rc"), "stable"), "beta");
        assert_eq!(channel(None, None, Some("rc"), "stable"), "rc", "a beta build never follows stable");
        assert_eq!(channel(None, None, None, "stable"), "stable");
        assert_eq!(channel(Some(""), Some("../x"), None, "stable"), "stable", "a bad name is no choice");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_check_offers_and_apply_installs_with_progress() {
        let _turn = TURN.lock().await;
        let rt = stub(offer("1.1.0"), None, 0);
        let h = ready();
        assert!(h.apply().unwrap_err().why.contains("check first"));
        check(&h).await;
        let s = h.state();
        assert_eq!((s.phase, s.available.as_ref().map(|o| o.version.as_str())), (Phase::Available, Some("1.1.0")));
        assert!(s.checked_at.is_some());
        h.apply().unwrap();
        h.done("apply").await;
        let s = h.state();
        assert_eq!((s.phase, s.progress), (Phase::Restarting, 100));
        assert_eq!(*rt.applies.lock().unwrap(), [("1.1.0".into(), Data::Gated)]);
        // The steps are events: replay has every one.
        let methods: Vec<String> = h.store(|s| s.events().unwrap()).into_iter().map(|e| e.method).collect();
        assert!(methods.ends_with(&["progress", "progress", "progress", "installing", "applied"].map(String::from)));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn checks_never_overlap_and_installs_never_run_twice() {
        let _turn = TURN.lock().await;
        let rt = stub(offer("1.1.0"), None, 150);
        let h = ready();
        h.check().unwrap();
        assert_eq!(h.state().phase, Phase::Checking, "a running check is seen");
        h.check().unwrap();
        h.done("check").await;
        assert_eq!(*rt.checks.lock().unwrap(), 1, "the second check joined the first");
        h.apply().unwrap();
        assert!(h.apply().unwrap_err().why.contains("already being installed"));
        assert!(h.check().unwrap_err().why.contains("being installed"));
        // A poll that lands mid-install keeps the offer.
        h.checked(Ok(Found::Current { reason: "x".into() })).unwrap();
        assert!(h.state().available.is_some());
        h.done("apply").await;
        assert_eq!(*rt.applies.lock().unwrap(), [("1.1.0".into(), Data::Gated)]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_failed_install_stays_said_while_its_release_is_offered() {
        let _turn = TURN.lock().await;
        stub(offer("1.1.0"), Some("no space left"), 0);
        let h = ready();
        check(&h).await;
        h.apply().unwrap();
        h.done("apply").await;
        let s = h.state();
        assert_eq!((s.phase, s.error.as_deref()), (Phase::Error, Some("no space left")));
        check(&h).await;
        let s = h.state();
        assert_eq!((s.phase, s.error.as_deref()), (Phase::Available, Some("no space left")));
    }

    #[test]
    fn dismissed_releases_are_never_offered_and_a_newer_one_is() {
        let h = ready();
        h.checked(Ok(offer("1.1.0"))).unwrap();
        h.dismiss().unwrap();
        let s = h.state();
        assert_eq!((s.phase, s.dismissed.as_deref(), s.available.is_none()), (Phase::Idle, Some("1.1.0"), true));
        h.checked(Ok(offer("1.1.0"))).unwrap();
        assert_eq!(h.state().phase, Phase::Idle);
        h.checked(Ok(offer("1.2.0"))).unwrap();
        assert_eq!(h.state().available.as_ref().map(|o| o.version.as_str()), Some("1.2.0"));
        // Undismissed: back within reach.
        h.undismiss().unwrap();
        h.checked(Ok(offer("1.1.0"))).unwrap();
        assert_eq!(h.state().phase, Phase::Available);
        // A blocked release is dismissed too.
        h.checked(Ok(Found::Blocked(Blocked { version: "2.0.0".into(), blockers: vec!["v3 > v2".into()] }))).unwrap();
        h.dismiss().unwrap();
        h.checked(Ok(Found::Blocked(Blocked { version: "2.0.0".into(), blockers: vec![] }))).unwrap();
        assert_eq!((h.state().phase, h.state().blocked.is_none()), (Phase::Idle, true));
    }

    #[test]
    fn a_blocked_release_is_never_applied_and_says_why() {
        let h = ready();
        h.checked(Ok(Found::Blocked(Blocked { version: "2.0.0".into(), blockers: vec!["notes: v3 > v2".into()] })))
            .unwrap();
        assert_eq!(h.state().phase, Phase::Blocked);
        let e = h.apply().unwrap_err().why;
        assert!(e.contains("2.0.0 is blocked: notes: v3 > v2"), "{e}");
        h.checked(Err("offline".into())).unwrap();
        assert_eq!((h.state().phase, h.state().error.as_deref()), (Phase::Error, Some("offline")));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn past_the_gate_only_when_asked_and_never_when_the_app_says_not_now() {
        // aio tests: acceptDataLoss / retireData install a blocked release; canApply vetoes.
        let _turn = TURN.lock().await;
        let blocked = Found::Blocked(Blocked { version: "2.0.0".into(), blockers: vec!["notes: v3 > v2".into()] });
        let rt = stub(blocked, None, 0);
        let h = ready();
        check(&h).await;
        assert!(h.apply().unwrap_err().why.contains("apply_anyway keeps the data"));
        fn busy() -> std::result::Result<(), String> {
            Err("a recording is running".into())
        }
        fn fine() -> std::result::Result<(), String> {
            Ok(())
        }
        can_apply(busy);
        assert_eq!(h.apply_anyway().unwrap_err().why, "the app says not now: a recording is running");
        assert_eq!(h.state().phase, Phase::Blocked, "a veto changes nothing");
        can_apply(fine);
        h.apply_anyway().unwrap();
        h.done("apply").await;
        h.start_fresh().unwrap_err();
        assert_eq!(h.state().phase, Phase::Restarting);
        assert_eq!(*rt.applies.lock().unwrap(), [("2.0.0".into(), Data::AcceptLoss)]);
        let h = ready();
        check(&h).await;
        h.start_fresh().unwrap();
        h.done("apply").await;
        assert_eq!(rt.applies.lock().unwrap()[1], ("2.0.0".into(), Data::Retire));
    }

    #[test]
    fn a_rolled_back_release_is_dismissed_at_start() {
        let h = Handle::<Updates>::memory();
        h.checked(Ok(offer("1.1.0"))).unwrap();
        h.ready(on("stable", Some("1.1.0"))).unwrap();
        let s = h.state();
        assert_eq!((s.dismissed.as_deref(), s.available.is_none(), s.phase), (Some("1.1.0"), true, Phase::Idle));
        h.checked(Ok(offer("1.1.0"))).unwrap();
        assert_eq!(h.state().phase, Phase::Idle, "a manual check doesn't offer it");
    }

    #[test]
    fn only_the_dismissal_is_saved() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("updates.db");
        {
            let h = Handle::<Updates>::open(&f).unwrap();
            h.ready(on("stable", None)).unwrap();
            h.checked(Ok(offer("1.1.0"))).unwrap();
            h.dismiss().unwrap();
        }
        let s = Handle::<Updates>::open(&f).unwrap().state();
        assert_eq!((s.dismissed.as_deref(), s.on, s.checked_at), (Some("1.1.0"), false, None));
    }
}

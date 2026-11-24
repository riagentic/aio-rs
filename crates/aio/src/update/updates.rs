//! The `updates` cell ([`Updates`], aio's): what a UI shows of its
//! updates and the verbs it calls, on the installed [`Runtime`]. Only a
//! build with aio-rs's `updates` feature (aio-rs-shell's `updates` turns
//! it on) has it: the cell's code is ~0.5 MB of a debug build.
use super::*;
use crate::prelude::*;
use std::sync::{Arc, RwLock};

fn runtime() -> std::result::Result<Arc<dyn Runtime>, String> {
    super::cell::RUNTIME.read().unwrap_or_else(|e| e.into_inner()).clone().ok_or_else(|| NOT_CONFIGURED.to_string())
}

const NOT_CONFIGURED: &str = "updates are not configured for this app: set [updates] url and keys in aio.toml";

/// Run a blocking runtime call off the cell: its error, as the cell keeps it
/// (its words).
async fn blocking<R: Send + 'static>(
    f: impl FnOnce(Arc<dyn Runtime>) -> std::result::Result<R, UpdateError> + Send + 'static,
) -> std::result::Result<R, String> {
    let rt = runtime()?;
    let r = tokio::task::spawn_blocking(move || f(rt)).await.map_err(|e| format!("the update runtime failed: {e}"))?;
    r.map_err(|e| e.to_string())
}

/// The app's updates. Only `dismissed` is saved: the rest is learned again
/// each run. Served to the app's clients beside its own cells ([`share`]):
/// they see it all but the paths on this machine (`backup`, `retired`, and
/// any in `error` and `failed`), and
/// call `check`, `dismiss` and `undismiss`; installing (`apply`,
/// `apply_anyway`, `start_fresh`) and `set_channel` are an admin's
/// (`role = "admin"`), as restarting the app is.
#[cell(persist, shared, id = "updates", visible = seen)]
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
    /// Which download `progress` measures; `None` when none runs.
    #[transient]
    pub progress_of: Option<ProgressOf>,
    /// When the last check answered (ms).
    #[transient]
    pub checked_at: Option<i64>,
    #[transient]
    pub error: Option<String>,
    /// The data's backup the update to this version took.
    #[transient]
    pub backup: Option<String>,
    /// Where this version's fresh start set the data aside ([`Ready::retired`]).
    #[transient]
    pub retired: Option<String>,
    /// The install that failed: (version, why), said again while offered.
    #[transient]
    pub failed: Option<(String, String)>,
    /// The release the user said no to: never offered, a newer one is.
    pub dismissed: Option<String>,
    /// The channel the user chose (`set_channel`): followed from now on,
    /// unless a run says otherwise (`AIORT_UPDATE_CHANNEL`).
    pub pinned: Option<String>,
}

/// What a client is shown: never a path on the server's disk, nor one
/// named in a failure's words.
fn seen(u: &Updates, _: &By) -> Updates {
    Updates {
        backup: None,
        retired: None,
        error: u.error.as_deref().map(unpathed),
        failed: u.failed.as_ref().map(|(v, why)| (v.clone(), unpathed(why))),
        ..u.clone()
    }
}

/// `why` with each word that names a path (or a `file:` URL) said as `…`,
/// its closing punctuation kept (pure).
fn unpathed(why: &str) -> String {
    let word = |w: &str| {
        let bare = w.trim_end_matches([':', ',', '.', ';', ')']);
        match bare.contains(['/', '\\']) || bare.starts_with('~') {
            true => format!("…{}", &w[bare.len()..]),
            false => w.to_string(),
        }
    };
    why.split(' ').map(word).collect::<Vec<_>>().join(" ")
}

static SHARED: RwLock<Option<Handle<Updates>>> = RwLock::new(None);

/// Serve `h` to the app's clients beside its own cells, from the next server
/// the app starts on (`aio_shell` shares the cell it opens).
pub fn share(h: Handle<Updates>) {
    *SHARED.write().unwrap_or_else(|e| e.into_inner()) = Some(h);
}

/// `http` serving the shared `updates` cell too, unless it serves one.
#[cfg(feature = "server")]
pub(crate) fn beside(http: crate::http::Http) -> crate::http::Http {
    let h = SHARED.read().unwrap_or_else(|e| e.into_inner()).clone();
    match h {
        Some(h) if !http.serves_cell(<Updates as crate::CellData>::ID) => http.cell(h),
        _ => http,
    }
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
        self.phase = Phase::Downloading;
        self.progress = 0;
        (self.error, self.failed) = (None, None);
        cx.run(
            Effect::new("apply", move |h: Handle<Updates>| async move {
                if let Some(Err(why)) = veto().await {
                    _ = h.vetoed(why);
                    return;
                }
                let report = h.clone();
                let r = blocking(move |rt| {
                    rt.apply(&version, data, &|s| {
                        _ = match s {
                            Step::Downloaded(p) => report.progress(p),
                            Step::Runtime(p) => report.runtime_progress(p),
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
        (self.channel, self.current, self.backup, self.retired) = (r.channel, r.current, r.backup, r.retired);
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
    #[client(predict = false)]
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
    #[client(role = "admin", predict = false)]
    pub fn apply(&mut self, cx: &Cx) -> Result {
        self.install(cx, Data::Gated)
    }

    /// Install the release the data gate blocks anyway: the data is backed
    /// up, and the new version tries to read it (the backup is the way back).
    #[client(role = "admin", predict = false)]
    pub fn apply_anyway(&mut self, cx: &Cx) -> Result {
        self.install(cx, Data::AcceptLoss)
    }

    /// Install the release (blocked or not) on fresh data: what is here now
    /// is moved aside whole, never deleted, and comes back if it fails.
    #[client(role = "admin", predict = false)]
    pub fn start_fresh(&mut self, cx: &Cx) -> Result {
        self.install(cx, Data::Retire)
    }

    /// The app said not now ([`can_apply`]): the release stays offered.
    pub fn vetoed(&mut self, why: String) {
        if self.busy() {
            self.phase = match (&self.available, &self.blocked) {
                (Some(_), _) => Phase::Available,
                (None, Some(_)) => Phase::Blocked,
                (None, None) => Phase::Idle,
            };
            (self.progress, self.progress_of) = (0, None);
            self.error = Some(format!("the app says not now: {why}"));
        }
    }

    /// The download's progress, in percent.
    pub fn progress(&mut self, percent: u8) {
        if self.phase == Phase::Downloading {
            (self.progress, self.progress_of) = (percent.min(100), Some(ProgressOf::App));
        }
    }

    /// The runtime's download progress, in percent.
    pub fn runtime_progress(&mut self, percent: u8) {
        if self.phase == Phase::Downloading {
            (self.progress, self.progress_of) = (percent.min(100), Some(ProgressOf::Runtime));
        }
    }

    pub fn installing(&mut self) {
        if self.busy() {
            (self.phase, self.progress, self.progress_of) = (Phase::Installing, 100, None);
        }
    }

    /// An install's end: the app restarts, or why it can't.
    pub fn applied(&mut self, r: std::result::Result<(), String>) {
        self.progress_of = None;
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
    #[client]
    pub fn dismiss(&mut self) {
        let Some(v) = self.available.as_ref().map(|o| &o.version).or(self.blocked.as_ref().map(|b| &b.version)) else {
            return;
        };
        self.dismissed = Some(v.clone());
        (self.available, self.blocked, self.error) = (None, None, None);
        self.phase = Phase::Idle;
    }

    /// Offer the dismissed release again (at the next check).
    #[client]
    pub fn undismiss(&mut self) {
        (self.dismissed, self.error) = (None, None);
    }

    /// Follow `channel` from now on (the next check asks it). What the old
    /// channel offered or was told no goes: crossing channels may even move
    /// the version back, and a no on one says nothing about another.
    #[client(role = "admin", predict = false)]
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
    use super::super::cell::{can_apply, install};
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
        fn check(&self) -> std::result::Result<Found, UpdateError> {
            *self.checks.lock().unwrap() += 1;
            std::thread::sleep(self.slow);
            Ok(self.found.lock().unwrap().clone())
        }
        fn apply(&self, version: &str, data: Data, step: &dyn Fn(Step)) -> std::result::Result<(), UpdateError> {
            self.applies.lock().unwrap().push((version.into(), data));
            std::thread::sleep(self.slow);
            for p in [30, 70, 100] {
                step(Step::Downloaded(p));
            }
            step(Step::Runtime(40));
            step(Step::Installing);
            self.fail.clone().map_or(Ok(()), |e| Err(UpdateError::Other(e)))
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
            retired: None,
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
    fn a_client_never_sees_a_server_path_in_a_failure() {
        let why = "the update needs about 30 MB free in /srv/app/install/versions: free some space";
        let u = Updates {
            error: Some(why.into()),
            failed: Some(("1.1.0".into(), r"can't read C:\Users\me\up.json (file:///srv/up.json), ~/x".into())),
            backup: Some("/data/b.db".into()),
            ..Updates::default()
        };
        let s = seen(&u, &By::User("eve".into()));
        assert_eq!(s.error.as_deref(), Some("the update needs about 30 MB free in …: free some space"));
        assert_eq!(s.failed, Some(("1.1.0".into(), "can't read … …), …".into())));
        assert_eq!((s.backup, unpathed("offline")), (None, "offline".into()));
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
        // A fresh start's first start: where the data went, and its store there as the backup.
        let aside = "/home/archive/notes-1.0.0-x";
        let fresh = Ready { retired: Some(aside.into()), backup: Some(format!("{aside}/state.db")), ..on("beta", None) };
        h.ready(fresh).unwrap();
        assert_eq!(h.state().retired.as_deref(), Some(aside));
        assert_eq!(h.state().backup, Some(format!("{aside}/state.db")));
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
        let steps = ["progress", "progress", "progress", "runtime_progress", "installing", "applied"];
        assert!(methods.ends_with(&steps.map(String::from)), "{methods:?}");
        assert_eq!(h.state().progress_of, None, "no download runs now");
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
    fn progress_says_which_download_it_measures() {
        let mut u = Updates::default();
        u.progress(50);
        assert_eq!(u.progress_of, None, "no install runs: nothing to measure");
        u.phase = Phase::Downloading;
        u.progress(100);
        assert_eq!((u.progress, u.progress_of), (100, Some(ProgressOf::App)));
        u.runtime_progress(10);
        assert_eq!((u.progress, u.progress_of), (10, Some(ProgressOf::Runtime)));
        u.installing();
        assert_eq!((u.progress, u.progress_of), (100, None));
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
        // Awaited, off the cell: it may do work (here, wait) before it answers.
        can_apply(|| async {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            Err("a recording is running".into())
        });
        h.apply_anyway().unwrap();
        h.done("apply").await;
        let s = h.state();
        assert_eq!(s.error.as_deref(), Some("the app says not now: a recording is running"));
        assert_eq!(s.phase, Phase::Blocked, "a veto changes nothing: still offered");
        assert!(rt.applies.lock().unwrap().is_empty(), "nothing installed");
        can_apply(|| async { Ok(()) });
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

    #[cfg(feature = "server")]
    #[test]
    fn a_shared_cell_is_served_beside_the_apps_once() {
        assert!(!beside(crate::http::Http::default()).serves_cell("updates"), "none shared: none served");
        let h = Handle::<Updates>::memory();
        share(h.clone());
        assert!(beside(crate::http::Http::default()).serves_cell("updates"));
        // An app serving its own is left as it is (twice would panic).
        assert!(beside(crate::http::Http::default().cell(h)).serves_cell("updates"));
        *SHARED.write().unwrap() = None;
    }
}

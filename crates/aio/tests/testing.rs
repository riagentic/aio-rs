//! The cell test harness (aio's `testCell`, `bootCells`): seeds, assertions
//! that print the state, effects seen before they run, the schedule clock,
//! booting as the app does, and the failures nobody looked at.
use aio::prelude::*;
use aio::testing::{TestCells, expect, seeded, totp_code};
use aio::{Cells, auth::totp};
use serde_json::json;
use std::time::Duration;

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Cfg {
    url: String,
    retries: u32,
}

#[cell]
pub struct Feed {
    cfg: Cfg,
    items: Vec<String>,
    loading: bool,
    toast: Option<String>,
}

#[methods]
impl Feed {
    pub fn refresh(&mut self, cx: &Cx) -> Result {
        ensure!(!self.cfg.url.is_empty(), "no url");
        self.loading = true;
        let url = self.cfg.url.clone();
        cx.run(Effect::new("fetch", move |h: Handle<Feed>| async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            _ = h.fetched(vec![format!("{url}/1")]);
        }));
        Ok(())
    }

    pub fn fetched(&mut self, items: Vec<String>) {
        self.items = items;
        self.loading = false;
    }

    pub fn notify(&mut self, cx: &Cx, text: String) -> Result {
        self.toast = Some(text);
        cx.after("dismiss", 3000, "dismiss", ())
    }

    pub fn dismiss(&mut self) {
        self.toast = None;
    }

    /// Asks for an effect whose call is refused without a url.
    pub fn retry(&mut self, cx: &Cx) {
        cx.run(Effect::new("retry", |h: Handle<Feed>| async move {
            _ = h.refresh();
        }));
    }

    /// Asks for a schedule whose call is refused without a url.
    pub fn later(&mut self, cx: &Cx) -> Result {
        cx.after("again", 10, "refresh", ())
    }

    /// Stops the fetch, started or not.
    pub fn cancel(&mut self, cx: &Cx) {
        cx.abort("fetch");
        self.loading = false;
    }

    /// Asks for an effect that never ends.
    pub fn hang(&mut self, cx: &Cx) {
        cx.run(Effect::new("hang", |_: Handle<Feed>| std::future::pending::<()>()));
    }
}

/// Counts what `Feed` loaded: wired to it by a listener, as an app would.
#[cell]
pub struct Stats {
    loads: u32,
    at: i64,
}

#[methods]
impl Stats {
    pub fn loaded(&mut self) {
        self.loads += 1;
    }

    pub fn stamp(&mut self, cx: &Cx) {
        self.at = cx.now();
    }
}

/// v1 of `Job` saved `{"name": …}`.
fn job_up(old: serde_json::Value, from: u32) -> std::result::Result<Job, String> {
    assert_eq!(from, 1);
    Ok(Job { title: old["name"].as_str().ok_or("no name")?.into(), ..Job::default() })
}

/// A job that was stuck when the app stopped is not stuck after a boot.
fn job_repair(s: Job) -> Job {
    Job { stuck: false, ..s }
}

#[cell(persist, version = 2, migrate = job_up, restore = job_repair)]
pub struct Job {
    title: String,
    stuck: bool,
    #[transient]
    busy: bool,
}

#[methods]
impl Job {}

#[test]
fn a_seed_merges_into_the_default_state() {
    let s: Feed = seeded(json!({"cfg": {"url": "https://x"}}));
    assert_eq!(s.cfg, Cfg { url: "https://x".into(), retries: 0 }, "the rest of cfg stays at its default");
    assert!(s.items.is_empty() && !s.loading);
}

#[test]
#[should_panic(expected = "seed `cfg.ur1` isn't part of Feed's state; it has: retries, url")]
fn a_seed_key_the_state_lacks_is_refused() {
    let _: Feed = seeded(json!({"cfg": {"ur1": "https://x"}}));
}

#[test]
#[should_panic(expected = "loaded\n  state: {\"cfg\":{\"url\":\"\",\"retries\":0},\"items\":[],\"loading\":false")]
fn a_failed_expectation_prints_the_state() {
    let mut t = TestCells::new();
    let feed = t.cell::<Feed>();
    expect(&feed, "loaded", |s| !s.items.is_empty());
}

#[test]
fn an_abort_drops_a_held_effect_unstarted() {
    let mut t = TestCells::new();
    let feed = t.seeded::<Feed>(json!({"cfg": {"url": "https://x"}}));
    feed.refresh().unwrap();
    assert_eq!(t.effects(), ["Feed.fetch"]);
    feed.cancel().unwrap();
    assert!(t.effects().is_empty(), "aborted before it started");
    t.settle();
    expect(&feed, "nothing fetched", |s| s.items.is_empty() && !s.loading);
}

#[test]
fn effects_wait_for_settle_and_their_results_land() {
    let mut t = TestCells::new();
    let feed = t.seeded::<Feed>(json!({"cfg": {"url": "https://x"}}));
    feed.refresh().unwrap();
    assert_eq!(t.effects(), ["Feed.fetch"], "asked for, not started");
    expect(&feed, "loading until the fetch is back", |s| s.loading);
    t.settle();
    assert!(t.effects().is_empty());
    expect(&feed, "fetched", |s| s.items == ["https://x/1"] && !s.loading);
    let empty = t.cell::<Feed>();
    assert_eq!(empty.refresh(), Err(Rejected::new("no url")));
    assert!(t.effects().is_empty(), "a refused call asks for nothing");
}

#[test]
#[should_panic(expected = "still running after 5s: Feed.hang")]
fn settle_names_the_effects_still_running() {
    let mut t = TestCells::new();
    t.cell::<Feed>().hang().unwrap();
    t.settle();
}

#[test]
fn advance_fires_due_schedules_only() {
    let mut t = TestCells::new();
    let feed = t.cell::<Feed>();
    feed.notify("saved".into()).unwrap();
    // Real time passing changes nothing: only `advance` moves the clock.
    std::thread::sleep(std::time::Duration::from_millis(20));
    t.advance(2999);
    expect(&feed, "not due yet", |s| s.toast.is_some());
    t.advance(1);
    expect(&feed, "dismissed on time", |s| s.toast.is_none());
    assert_eq!(feed.store(|s| s.events().unwrap().last().unwrap().by.clone()), By::Schedule("dismiss".into()));
}

#[test]
fn cells_booted_together_follow_each_other() {
    let mut t = TestCells::new();
    let feed = t.seeded::<Feed>(json!({"cfg": {"url": "https://x"}}));
    let stats = t.cell::<Stats>();
    let s = stats.clone();
    feed.listen(move |f| {
        if !f.loading && !f.items.is_empty() {
            _ = s.loaded();
        }
    });
    feed.refresh().unwrap();
    t.settle();
    expect(&stats, "the effect's result reached the other cell", |s| s.loads == 1);
}

#[test]
fn calls_see_the_test_clock() {
    let mut t = TestCells::new();
    let stats = t.cell::<Stats>();
    stats.stamp().unwrap();
    let t0 = stats.state().at;
    std::thread::sleep(Duration::from_millis(20));
    stats.stamp().unwrap();
    assert_eq!(stats.state().at, t0, "real time passing moves nothing");
    t.advance(5000);
    stats.stamp().unwrap();
    assert_eq!(stats.state().at, t0 + 5000);
}

#[test]
fn the_apps_list_boots_in_dependency_order_on_the_test_clock() {
    let mut t = TestCells::new();
    let (feed, stats) = (t.cell::<Feed>(), t.cell::<Stats>());
    let cells = t.start(Cells::default().with(stats, &[Feed::ID]).with(feed.clone(), &[])).unwrap();
    assert_eq!(cells.ids(), [Feed::ID, Stats::ID], "listed out of order on purpose");
    // The started schedulers fire on this clock, not the wall's.
    feed.notify("saved".into()).unwrap();
    std::thread::sleep(Duration::from_millis(20));
    t.advance(2999);
    expect(&feed, "not due yet", |s| s.toast.is_some());
    t.advance(1);
    expect(&feed, "dismissed on time", |s| s.toast.is_none());
    cells.disable(Feed::ID).unwrap();
    assert_eq!(
        feed.notify("x".into()),
        Err(Rejected::coded(aio::Code::DispatchClosed, format!("{} is disabled", Feed::ID)))
    );
}

#[test]
fn a_list_the_app_would_refuse_is_refused() {
    let mut t = TestCells::new();
    let mut refused = |c: fn(&mut TestCells) -> Cells| {
        let list = c(&mut t);
        t.start(list).err().unwrap().0
    };
    let dup = refused(|t| Cells::default().with(t.cell::<Feed>(), &[]).with(t.cell::<Feed>(), &[]));
    assert!(dup.contains(&format!("duplicate cell name `{}`", Feed::ID)), "{dup}");
    let unknown = refused(|t| Cells::default().with(t.cell::<Feed>(), &["missing"]));
    assert!(unknown.contains("depends on unknown cell `missing`"), "{unknown}");
    let cycle = refused(|t| Cells::default().with(t.cell::<Feed>(), &[Stats::ID]).with(t.cell::<Stats>(), &[Feed::ID]));
    assert_eq!(cycle, format!("dependency cycle: {} → {} → {}", Feed::ID, Stats::ID, Feed::ID));
}

#[test]
fn saved_data_boots_as_the_app_boots_it() {
    let mut t = TestCells::new();
    let job = t.saved::<Job>(2, json!({"title": "x", "stuck": true, "busy": true})).unwrap();
    assert_eq!(*job.state(), Job { title: "x".into(), ..Job::default() }, "repaired, the transient field reset");
    let old = t.saved::<Job>(1, json!({"name": "old"})).unwrap();
    assert_eq!(old.state().title, "old", "migrated");

    let refused = |t: &mut TestCells, v, data| t.saved::<Job>(v, data).err().unwrap().0;
    assert!(refused(&mut t, 1, json!({})).contains("migrate from v1 failed: no name"));
    assert_eq!(t.saved::<Job>(3, json!({"title": "y", "later": 1})).unwrap().state().title, "y", "a rollback boots");
    assert!(refused(&mut t, 3, json!({"title": 5})).contains("data from a newer build (v3 > v2) doesn't fit"));
    let undeclared = refused(&mut t, 2, json!({"title": "x", "owner": "ann"}));
    assert!(undeclared.contains("fields this build doesn't declare (owner)"), "{undeclared}");
}

#[test]
#[should_panic(
    expected = "failed and never looked at (take them with `failures`): Feed.refresh: no url (effect `retry`)"
)]
fn a_failure_nobody_looked_at_fails_the_test() {
    let mut t = TestCells::new();
    t.cell::<Feed>().retry().unwrap();
    t.settle();
}

#[test]
fn failures_taken_are_looked_at_and_a_refused_test_call_is_an_answer() {
    let mut t = TestCells::new();
    let feed = t.cell::<Feed>();
    assert!(feed.refresh().is_err(), "the test sees this one");
    feed.retry().unwrap();
    feed.later().unwrap();
    t.advance(10);
    // `advance` fires the schedule, then settles the waiting effect.
    assert_eq!(t.failures(), ["Feed.refresh: no url (schedule `again`)", "Feed.refresh: no url (effect `retry`)"]);
    assert!(t.failures().is_empty(), "taken");
}

/// Effects that wait on tokio timers (aio's `sleep` in a method).
#[cell]
pub struct Nap {
    woke: Vec<String>,
}

thread_local! {
    /// When each `mark` effect ran, on the effects' clock.
    static MARKS: std::cell::RefCell<Vec<tokio::time::Instant>> = const { std::cell::RefCell::new(vec![]) };
}

#[methods]
impl Nap {
    pub fn nap(&mut self, cx: &Cx, ms: u64, tag: String) {
        cx.run(Effect::new(&format!("nap-{tag}"), move |h: Handle<Nap>| async move {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            _ = h.woke(tag);
        }));
    }

    pub fn woke(&mut self, tag: String) {
        self.woke.push(tag);
    }

    /// An effect cut off by its own timeout.
    pub fn stall(&mut self, cx: &Cx) {
        cx.run(Effect::new("stall", |h: Handle<Nap>| async move {
            let out = tokio::time::timeout(Duration::from_secs(30), std::future::pending::<()>()).await;
            _ = h.woke(format!("{out:?}"));
        }));
    }

    /// Real I/O under a timeout: a thread answers after 1 s, half the
    /// timeout (virtual time's 2 s pass long before, on a slow runner too).
    pub fn ask_device(&mut self, cx: &Cx) {
        cx.run(Effect::new("device", |h: Handle<Nap>| async move {
            let (tx, rx) = tokio::sync::oneshot::channel();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(1));
                _ = tx.send(());
            });
            let out = tokio::time::timeout(Duration::from_secs(2), rx).await;
            _ = h.woke(if out.is_ok() { "answered" } else { "timed out" }.into());
        }));
    }

    /// Work left running on its own: a thread calls back after 50 ms.
    pub fn later_on_a_thread(&mut self, cx: &Cx) {
        cx.run(Effect::new("thread", |h: Handle<Nap>| async move {
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(50));
                _ = h.woke("from the thread".into());
            });
        }));
    }

    pub fn mark(&mut self, cx: &Cx) {
        cx.run(Effect::new("mark", |_: Handle<Nap>| async {
            MARKS.with(|m| m.borrow_mut().push(tokio::time::Instant::now()));
        }));
    }
}

#[test]
fn timers_in_effects_run_on_virtual_time_in_their_order() {
    let mut t = TestCells::new();
    let nap = t.cell::<Nap>();
    let real = std::time::Instant::now();
    nap.nap(10_000, "long".into()).unwrap();
    nap.nap(30, "short".into()).unwrap();
    nap.stall().unwrap();
    t.settle();
    expect(&nap, "each woke on its own time, the timeout too", |s| s.woke == ["short", "long", "Err(Elapsed(()))"]);
    // Under the 10 s nap: never waited for. (The paused clock steps 1 ms at
    // a time; a slow machine takes seconds for these 30 s, a macOS VM 2 s.)
    assert!(real.elapsed() < Duration::from_secs(10), "took real time: {:?}", real.elapsed());
}

/// aio's `t.settle(ms)` on real timers: virtual time jumps to a timeout
/// while real I/O is pending; the wall clock gives the I/O its time.
#[test]
fn real_io_under_a_timeout_needs_real_time() {
    let mut t = TestCells::new();
    let nap = t.cell::<Nap>();
    nap.ask_device().unwrap();
    t.settle();
    expect(&nap, "virtual time ran out first", |s| s.woke == ["timed out"]);

    let mut t = TestCells::real_time();
    let nap = t.cell::<Nap>();
    nap.ask_device().unwrap();
    t.settle();
    expect(&nap, "the device answered in time", |s| s.woke == ["answered"]);
    // Schedules still move only with `advance`.
    t.advance(60_000);
}

#[test]
fn settle_for_waits_for_work_an_effect_left_running() {
    let mut t = TestCells::real_time();
    let nap = t.cell::<Nap>();
    nap.later_on_a_thread().unwrap();
    t.settle();
    expect(&nap, "settle sees no running effect", |s| s.woke.is_empty());
    t.settle_for(500);
    expect(&nap, "the thread called back", |s| s.woke == ["from the thread"]);
}

#[test]
fn advance_moves_the_effects_clock_too() {
    let mut t = TestCells::new();
    let nap = t.cell::<Nap>();
    nap.mark().unwrap();
    t.settle();
    t.advance(60_000);
    nap.mark().unwrap();
    t.settle();
    let marks = MARKS.with(|m| m.borrow().clone());
    assert!(marks[1] - marks[0] >= Duration::from_secs(60), "{:?}", marks[1] - marks[0]);
    // A step back moves the schedule clock only: tokio's never runs backwards.
    t.advance(-1000);
}

#[test]
fn a_totp_code_is_the_one_a_login_accepts() {
    // RFC 6238 appendix B: secret "12345678901234567890", t = 59 s.
    assert_eq!(totp_code("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ", 59_000), "287082");
    let secret = totp::secret();
    let now = 1_700_000_000_000;
    let key = totp::unbase32(&secret).unwrap();
    assert_eq!(totp::accept(&key, &totp_code(&secret, now), now, 0), Some(now / totp::STEP_MS));
}

#[cell]
pub struct Board {
    posts: Vec<String>,
}

#[methods]
impl Board {
    /// The author is who calls: never an argument a caller could forge.
    #[client]
    pub fn post(&mut self, cx: &Cx, text: &str) {
        let by = match cx.by() {
            By::User(u) => u.clone(),
            _ => "anyone".into(),
        };
        self.posts.push(format!("{by}: {text}"));
    }

    #[client(role = "admin")]
    pub fn clear(&mut self) {
        self.posts.clear();
    }

    /// A method may depend on the caller's role (aio's `auth.role`).
    #[client]
    pub fn badge(&mut self, cx: &Cx) {
        self.posts.push(cx.role().unwrap_or("guest").to_string());
    }

    pub fn seed(&mut self) {
        self.posts.push("server".into());
    }
}

#[test]
fn a_call_runs_as_a_client_under_its_rules() {
    let mut t = TestCells::new();
    let b = t.cell::<Board>();
    let ana = t.as_client(&b, Caller { by: By::User("ana".into()), role: Some("member".into()) });
    let root = t.as_client(&b, Caller { by: By::User("root".into()), role: Some("admin".into()) });
    ana.post("hi").unwrap();
    expect(&b, "the method saw ana", |s| s.posts == ["ana: hi"]);
    assert!(ana.clear().unwrap_err().why.contains("needs the role admin"));
    assert!(ana.seed().unwrap_err().why.contains("is not a #[client] method"), "the server's own");
    root.clear().unwrap();
    // The key alone proves no one in particular.
    t.as_client(&b, Caller::default()).post("x").unwrap();
    expect(&ana, "anonymous", |s| s.posts == ["anyone: x"]);
}

#[test]
fn a_method_sees_the_callers_role_and_replay_sees_the_same() {
    let mut t = TestCells::new();
    let b = t.cell::<Board>();
    let root = t.as_client(&b, Caller { by: By::User("root".into()), role: Some("admin".into()) });
    root.badge().unwrap();
    t.as_client(&b, Caller::default()).badge().unwrap();
    b.badge().unwrap();
    expect(&b, "each call saw its role", |s| s.posts == ["admin", "guest", "guest"]);
    // Saved with the event once read: the log replays to the same state.
    let events = b.store(|s| s.events().unwrap());
    assert_eq!(events[0].inputs.role.as_deref(), Some("admin"));
    assert!(events[1].inputs.is_empty(), "no role: nothing saved");
    let replayed = events.iter().try_fold(Board::default(), |st, ev| aio::apply_event(&st, ev)).unwrap();
    assert_eq!(replayed, *b.state());
}

#[cell(shared)]
pub struct Shop {
    sold: u32,
}

#[methods]
impl Shop {
    #[client]
    pub fn sell(&mut self) {
        self.sold += 1;
    }
}

#[tokio::test]
async fn a_test_server_serves_the_app_and_fetches_its_routes() {
    use aio::http::{Http, Reply, Route};
    let h = Handle::<Shop>::memory();
    let sell = h.clone();
    let http = Http::default().route(Route::post("/buy", move |_| {
        let h = sell.clone();
        async move { Reply::text(if h.sell().is_ok() { "sold" } else { "no" }) }
    }));
    let s = aio::testing::TestServer::start(h, http).await.unwrap();
    assert!(s.url().starts_with("ws://127.0.0.1:") && !s.token.is_empty());
    let f = s.fetch("POST", "/buy", "").await.unwrap();
    assert_eq!((f.status, f.body.as_str()), (200, "sold"));
    assert!(f.header("content-type").unwrap().starts_with("text/plain"));
    assert_eq!(s.state().sold, 1);
    let json = [("Content-Type", "application/json")];
    assert_eq!(s.fetch_with("POST", "/buy", &json, "").await.unwrap().status, 401, "a stranger has no key");
    assert_eq!(s.state().sold, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_page_signs_up_and_its_cookie_signs_it_in_until_it_signs_out() {
    let auth =
        std::sync::Arc::new(aio::auth::Auth::memory(aio::auth::Policy { signup: true, ..Default::default() }).unwrap());
    let opts = aio::sync::Options { auth: Some(auth), ..Default::default() };
    let s = aio::testing::TestServer::start_with(Handle::<Shop>::memory(), opts).await.unwrap();
    let json = [("Content-Type", "application/json")];
    let mut page = s.page();
    let made = page.fetch_with("POST", "/__aio/auth/signup", &json, r#"{"id":"bob","password":"password123"}"#);
    assert_eq!(made.await.unwrap().status, 201);
    assert_eq!(page.cookies.len(), 1, "the session cookie is kept");
    let me = page.fetch_with("GET", "/__aio/auth/me", &[], "").await.unwrap();
    assert!(me.body.contains(r#""id":"bob""#), "the cookie was sent back: {}", me.body);
    let other = s.page().fetch_with("GET", "/__aio/auth/me", &[], "").await.unwrap();
    assert!(other.body.contains(r#""user":null"#), "another browser is nobody: {}", other.body);
    assert_eq!(page.fetch_with("POST", "/__aio/auth/logout", &json, "").await.unwrap().status, 200);
    assert!(page.cookies.is_empty(), "signing out clears it");
    let me = page.fetch_with("GET", "/__aio/auth/me", &[], "").await.unwrap();
    assert!(me.body.contains(r#""user":null"#), "{}", me.body);
}

#[tokio::test(flavor = "multi_thread")]
async fn clients_of_one_server_converge_on_its_state() {
    let s = aio::testing::TestServer::start(Handle::<Shop>::memory(), aio::http::Http::default()).await.unwrap();
    let (a, b) = (s.client("ana").await.unwrap(), s.client("bo").await.unwrap());
    // Each sells on its own copy at once; both land, and every copy agrees.
    for _ in 0..5 {
        a.sell().unwrap();
        b.sell().unwrap();
    }
    s.converged(&[&a, &b]).await;
    assert_eq!((s.state().sold, a.state().sold, b.state().sold), (10, 10, 10));
    s.handle.sell().unwrap();
    s.converged(&[&a, &b]).await;
    assert_eq!(b.state().sold, 11, "a change on the server reaches every client");
}

/// A cell no other test serves: its link count is this test's alone.
#[cell(shared)]
pub struct Stall {
    sold: u32,
}

#[methods]
impl Stall {
    pub fn sell(&mut self) {
        self.sold += 1;
    }
}

#[tokio::test]
async fn a_served_app_says_its_metrics_and_health() {
    let h = Handle::<Stall>::memory();
    h.sell().unwrap();
    let s = aio::testing::TestServer::start(h, aio::http::Http::default()).await.unwrap();
    let m = s.fetch("GET", "/__aio/metrics", "").await.unwrap();
    assert_eq!(m.status, 200);
    assert!(m.header("content-type").unwrap().starts_with("text/plain; version=0.0.4"));
    for l in ["# TYPE aio_uptime_seconds gauge", "aio_cell_enabled{cell=\"stall\"} 1", "aio_clients_connected 0"] {
        assert!(m.body.contains(l), "{l} not in {}", m.body);
    }
    // What clients were sent: server totals, counted as states and patches go out.
    let sent = |body: &str, name: &str| -> u64 {
        let line = body.lines().find(|l| l.starts_with(&format!("{name} "))).unwrap_or_else(|| panic!("no {name}"));
        line[name.len() + 1..].parse().unwrap()
    };
    let (bytes, msgs) = (sent(&m.body, "aio_broadcast_bytes_total"), sent(&m.body, "aio_broadcast_messages_total"));
    let c = s.client("ana").await.unwrap();
    s.handle.sell().unwrap();
    s.converged(&[&c]).await;
    let m = s.fetch("GET", "/__aio/metrics", "").await.unwrap().body;
    assert!(sent(&m, "aio_broadcast_messages_total") >= msgs + 2, "its state and the patch: {m}");
    assert!(sent(&m, "aio_broadcast_bytes_total") > bytes);
    // Vitals: the link's liveness and payloads, the cell's size.
    let v: serde_json::Value = serde_json::from_str(&s.fetch("GET", "/__aio/vitals", "").await.unwrap().body).unwrap();
    let row = v["clients"].as_array().unwrap().iter().find(|c| c["cell"] == "stall").expect("the link is listed");
    assert_eq!(row["status"], "healthy");
    assert!(
        row["gap"].as_u64().unwrap() < 5_000
            && row["totalBytes"].as_u64().unwrap() >= row["lastPayloadBytes"].as_u64().unwrap()
    );
    assert!(v["cellSizes"]["stall"].as_u64().unwrap() > 0 && v["breakers"] == serde_json::json!([]), "{v}");
    assert!(v["loop"]["drainRate"].is_u64());
    let h = s.fetch("GET", "/__aio/health", "").await.unwrap();
    let doc: serde_json::Value = serde_json::from_str(&h.body).unwrap();
    assert_eq!((doc["cell"]["id"].as_str(), doc["cell"]["enabled"].as_bool()), (Some("stall"), Some(true)));
    assert!(doc["status"].is_string() && doc["memory"]["level"].is_string());
    // This machine's monitor needs no key; over the network it does (tests/http.rs).
    for path in ["/__aio/metrics", "/__aio/health"] {
        assert_eq!(s.fetch_with("GET", path, &[], "").await.unwrap().status, 200, "{path}");
    }
    assert_eq!(s.fetch("GET", "/__aio/nope", "").await.unwrap().status, 404);
}

#[cell]
pub struct Account {
    balance: u64,
    moves: u32,
}

#[methods]
impl Account {
    #[invariant]
    fn not_too_rich(&self) -> Result {
        ensure!(self.balance <= 1000, "over the limit");
        Ok(())
    }

    pub fn deposit(&mut self, n: u64) {
        self.balance += n;
        self.moves += 1;
    }

    pub fn withdraw(&mut self, n: u64) -> Result {
        ensure!(n <= self.balance, "not enough");
        self.balance -= n;
        self.moves += 1;
        Ok(())
    }

    /// A bug a fuzzer finds: the 13th move panics.
    pub fn audit(&mut self) {
        assert!(self.moves != 13, "unlucky audit");
    }
}

fn amount(r: &mut aio::testing::Rng) -> serde_json::Value {
    json!([r.below(300)])
}

#[test]
fn a_seeded_fuzz_calls_at_random_and_replays_exactly() {
    let money: [aio::testing::RandomCall; 2] = [("deposit", &amount), ("withdraw", &amount)];
    let mut t = TestCells::new();
    let a = t.cell::<Account>();
    let run = t.fuzz(&a, 42, 200, &money, |s| s.balance <= 1000);
    assert_eq!(run.len(), 200);
    assert!(run.iter().any(|c| c.starts_with("deposit(")) && run.iter().any(|c| c.starts_with("withdraw(")));
    let mut again = TestCells::new();
    let b = again.cell::<Account>();
    assert_eq!(again.fuzz(&b, 42, 200, &money, |_| true), run, "the same seed is the same run");
    assert_eq!(*a.state(), *b.state());
    let mut other = TestCells::new();
    let c = other.cell::<Account>();
    assert_ne!(other.fuzz(&c, 43, 200, &money, |_| true), run);
}

#[test]
#[should_panic(expected = "fuzz seed 7: account.audit panicked: unlucky audit")]
fn a_fuzz_that_finds_a_panic_names_its_seed_and_calls() {
    let calls: [aio::testing::RandomCall; 2] = [("deposit", &|_| json!([1])), ("audit", &|_| serde_json::Value::Null)];
    let mut t = TestCells::new();
    let a = t.cell::<Account>();
    t.fuzz(&a, 7, 500, &calls, |_| true);
}

/// A service app's cell.
#[cell(shared, persist)]
pub struct Ledger {
    balance: u64,
}

#[methods]
impl Ledger {
    #[client]
    pub fn credit(&mut self, n: u64) {
        self.balance += n;
    }
}

/// The service, as the desk app's client of it reaches it.
static LEDGER: std::sync::OnceLock<aio::sync::Remote<Ledger>> = std::sync::OnceLock::new();

/// A client app's own cell: it pays through its client of the service.
#[cell(shared, persist)]
pub struct Desk {
    bought: u32,
}

#[methods]
impl Desk {
    pub fn buy(&mut self, cx: &Cx, n: u64) {
        self.bought += 1;
        cx.run(Effect::new("pay", move |_: Handle<Desk>| async move { _ = LEDGER.get().map(|l| l.credit(n)) }));
    }
}

/// aio's `testApps`: a service and a client app that is an app of its own,
/// each with its own dirs, lock, store, port and key, in one test.
#[tokio::test(flavor = "multi_thread")]
async fn several_apps_in_one_test_keep_their_own_dirs_ports_and_lives() {
    use aio::{http::Http, paths::AppDirs, testing::TestServer};
    let world = aio::paths::sandbox().unwrap();
    let dirs = |id: &str| AppDirs::at(&world.root().join(id), id).unwrap();
    let (sd, dd) = (dirs("service"), dirs("desk"));
    assert!(AppDirs::at(&world.root().join("desk"), "desk").is_err(), "each app holds its own lock");
    assert_ne!(sd.store, dd.store);
    let service = TestServer::start(Handle::<Ledger>::open(&sd.store).unwrap(), Http::default()).await.unwrap();
    let desk = TestServer::start(Handle::<Desk>::open(&dd.store).unwrap(), Http::default()).await.unwrap();
    assert!(service.addr != desk.addr && service.token != desk.token, "own port, own key");

    // The desk is a client of the service: its call lands there, not here.
    assert!(LEDGER.set(service.client("desk").await.unwrap()).is_ok());
    let ledger = LEDGER.get().unwrap();
    desk.handle.buy(5).unwrap();
    desk.handle.done("pay").await;
    service.converged(&[ledger]).await;
    assert_eq!((service.state().balance, desk.state().bought), (5, 1));
    // A client of the desk sees the desk's state.
    let shopper = desk.client("ana").await.unwrap();
    desk.converged(&[&shopper]).await;
    assert_eq!(shopper.state().bought, 1);

    // Closing the desk leaves the service running; each kept its own file.
    drop((shopper, desk));
    let late = service.client("bo").await.unwrap();
    service.converged(&[&late]).await;
    assert_eq!(late.state().balance, 5);
    assert!(sd.store.is_file() && dd.store.is_file());
}

/// aio's `testApps`, as one harness: named apps, each its own dirs and
/// server; a client of one; the wrong name or cell said.
#[tokio::test(flavor = "multi_thread")]
async fn test_apps_boot_named_apps_apart_and_name_the_booted_ones() {
    use aio::testing::{TestApps, fails};
    let mut world = TestApps::new();
    world.boot::<Ledger>("bank").await.unwrap();
    world.boot::<Desk>("till").await.unwrap();
    assert!(world.boot::<Desk>("till").await.is_err_and(|e| e.to_string().contains("booted already")));
    assert_ne!(world.dirs("bank").logs, world.dirs("till").logs, "each app its own logs");
    assert_ne!(world.get::<Ledger>("bank").addr, world.get::<Desk>("till").addr);
    let client = world.connect::<Ledger>("bank", "till").await.unwrap();
    world.get::<Ledger>("bank").handle.credit(3).unwrap();
    world.get::<Ledger>("bank").converged(&[&client]).await;
    assert_eq!(client.state().balance, 3);
    assert!(world.dirs("bank").store.is_file());
    assert!(fails(|| _ = world.get::<Ledger>("shop")).contains("no app \"shop\"; booted: bank, till"));
    assert!(fails(|| _ = world.get::<Desk>("bank")).contains("serves another cell than Desk"));
}

/// aio's `patches`, `waitForPatch`, `dispatchAll`: the stream a client got,
/// and one call from every client at once.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_s_updates_are_recorded_and_every_client_calls_at_once() {
    use aio::testing::{TestServer, call_all, patches};
    let server = TestServer::start(Handle::<Ledger>::memory(), aio::http::Http::default()).await.unwrap();
    let (a, b) = (server.client("ana").await.unwrap(), server.client("bo").await.unwrap());
    let seen = patches(&a);
    let answers = call_all(&[&a, &b], "credit", json!([2]));
    assert!(answers.iter().all(Result::is_ok), "{answers:?}");
    server.converged(&[&a, &b]).await;
    assert_eq!(server.state().balance, 4, "both calls landed once");
    let p = seen.until(|p| p["t"] == "patch" && p["seq"].as_u64() == Some(2)).await;
    assert!(p["ops"].is_array(), "a delta, not the whole state: {p}");
    assert!(seen.all().iter().all(|p| p["t"] == "patch"), "no resync: {:?}", seen.all());
}

#[cell]
pub struct Slow {
    n: u32,
}

#[methods]
impl Slow {
    pub fn crawl(&mut self, ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
        self.n += 1;
    }
}

/// aio's `perfBudget`, for one test: a call over it fails the test.
#[test]
fn a_call_over_the_test_s_budget_is_a_failure() {
    let mut t = TestCells::new();
    let slow = t.cell::<Slow>();
    t.budget::<Slow>("crawl", 15);
    slow.crawl(0).unwrap();
    assert!(t.failures().is_empty(), "within it");
    slow.crawl(40).unwrap();
    let f = t.failures();
    assert!(f.len() == 1 && f[0].contains("crawl") && f[0].contains("> 15ms"), "{f:?}");
    assert!(aio::testing::fails(|| t.budget::<Slow>("craw1", 5)).contains("its methods: crawl"));
}

/// aio's `expect.effects`: what the last call asked for, apart from the
/// ones still waiting from before.
#[test]
fn the_last_call_s_effects_are_its_own() {
    let mut t = TestCells::new();
    let feed = t.seeded::<Feed>(json!({"cfg": {"url": "https://x"}}));
    feed.refresh().unwrap();
    assert_eq!(t.last_effects(), ["Feed.fetch"]);
    feed.retry().unwrap();
    assert_eq!(
        (t.last_effects(), t.effects()),
        (vec!["Feed.retry".into()], vec!["Feed.fetch".into(), "Feed.retry".into()])
    );
    feed.dismiss().unwrap();
    assert!(t.last_effects().is_empty(), "it asked for none");
    t.settle();
}

#[cell]
pub struct Timer {
    pub on: bool,
}

#[methods]
impl Timer {
    pub fn begin(&mut self, cx: &Cx) -> Result {
        self.on = true;
        cx.every("tick", 1000, "tick", ())?;
        cx.tell(aio::tell::Tell::notify("Timer", "started"))?;
        cx.run(Effect::new("sound", |_: Handle<Timer>| async {}));
        Ok(())
    }

    pub fn halt(&mut self, cx: &Cx) {
        self.on = false;
        cx.cancel("tick");
        cx.abort("sound");
    }

    pub fn tick(&mut self) {}
}

/// aio's `t.getEffects()`: what the last call asked for, as values.
#[test]
fn the_last_call_s_effects_schedules_and_tells_are_values() {
    use aio::Plan;
    use aio::testing::{AskedFor, HeldEffect};
    let mut t = TestCells::new();
    let timer = t.cell::<Timer>();
    timer.begin().unwrap();
    let asked = t.last_asked();
    assert_eq!(
        asked.iter().map(|a| (a.is_schedule(), a.is_tell(), a.is_effect())).collect::<Vec<_>>(),
        [(true, false, false), (false, true, false), (false, false, true)]
    );
    let AskedFor::Schedule(Plan::Set { key, every, method, .. }) = &asked[0] else { panic!("{asked:?}") };
    assert_eq!((key.as_str(), *every, method.as_str()), ("tick", Some(1000), "tick"));
    assert_eq!(asked[1], AskedFor::Tell(aio::tell::Tell::notify("Timer", "started")));
    let AskedFor::Effect(HeldEffect { cell, key, .. }) = &asked[2] else { panic!("{asked:?}") };
    assert_eq!((*cell, key.as_str()), ("Timer", "sound"));
    timer.halt().unwrap();
    assert_eq!(t.last_asked(), [AskedFor::Schedule(Plan::Cancel("tick".into())), AskedFor::Abort("sound".into())]);
    assert!(t.last_asked().iter().all(|a| !a.is_effect()) && t.last_asked()[1].is_abort());
    t.settle();
}

/// aio's `t.init(seed)` / `t.destroy()` mid-test.
#[test]
fn a_cell_is_reset_mid_test() {
    let mut t = TestCells::new();
    let feed = t.seeded::<Feed>(json!({"cfg": {"url": "https://x"}}));
    feed.refresh().unwrap();
    t.reset(&feed, json!({"cfg": {"url": "https://y"}}));
    assert!(t.effects().is_empty(), "its waiting effects dropped");
    expect(&feed, "reset", |s| s.cfg.url == "https://y" && !s.loading && s.items.is_empty());
    t.reset(&feed, json!({}));
    expect(&feed, "fresh", |s| s.cfg.url.is_empty());
}

/// aio's `fuzz({ skip })`: the methods and their arguments found from
/// the cell itself.
#[test]
fn a_fuzz_finds_the_methods_and_makes_their_arguments() {
    let mut t = TestCells::new();
    let a = t.cell::<Account>();
    let run = t.fuzz_methods(&a, 5, 300, &["audit"], |s| s.balance <= 1000);
    assert!(run.iter().any(|c| c.starts_with("deposit([")) && run.iter().any(|c| c.starts_with("withdraw([")));
    assert!(!run.iter().any(|c| c.starts_with("audit")), "skipped");
    assert!(a.state().moves > 0, "calls landed");
    let mut again = TestCells::new();
    let b = again.cell::<Account>();
    assert_eq!(again.fuzz_methods(&b, 5, 300, &["audit"], |_| true), run, "the same seed is the same run");
    let e = aio::testing::fails(|| _ = again.fuzz_methods(&b, 5, 1, &["audt"], |_| true));
    assert!(e.contains("Account has no method audt"), "{e}");
}

/// aio's `t.randomActions(n)`: a fresh seed each run, said, and replayed
/// by `fuzz_methods`.
#[test]
fn random_calls_pick_a_fresh_seed_that_replays() {
    let mut t = TestCells::new();
    let a = t.cell::<Account>();
    // Under 13 moves: the audit's bug stays out of this one.
    let (seed, run) = t.random_calls(&a, 12);
    let (other, _) = TestCells::new().random_calls(&TestCells::new().cell::<Account>(), 1);
    assert_ne!(seed, other, "a fresh seed each run");
    let mut again = TestCells::new();
    let b = again.cell::<Account>();
    assert_eq!(again.fuzz_methods(&b, seed, 12, &[], |_| true), run, "its seed replays it");
}

#[test]
fn random_calls_find_the_panic_naming_the_seed() {
    let caught = (0..500).find_map(|_| {
        let mut t = TestCells::new();
        let a = t.cell::<Account>();
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| _ = t.random_calls(&a, 40))).err()
    });
    let e = caught.expect("a run found the panic");
    let e = e.downcast_ref::<String>().expect("its message");
    assert!(e.starts_with("fuzz seed ") && e.contains("account.audit panicked: unlucky audit"), "{e}");
}

#[test]
#[should_panic(expected = "account.audit panicked: unlucky audit")]
fn a_fuzz_over_the_methods_finds_the_panic() {
    for seed in 0..500 {
        let mut t = TestCells::new();
        let a = t.cell::<Account>();
        t.fuzz_methods(&a, seed, 40, &[], |_| true);
    }
}

/// aio's `schedule.after(id, ms, other.method.action())`: a schedule one cell
/// sets fires a method of another.
#[cell]
pub struct Drafts {
    pub text: String,
}

#[methods]
impl Drafts {
    pub fn edit(&mut self, cx: &Cx, text: String) -> Result {
        self.text = text.clone();
        cx.on::<Archive>().after("autosave", 3_000, "save", (text,))
    }

    pub fn stop(&mut self, cx: &Cx) {
        cx.cancel("autosave");
    }

    pub fn typo(&mut self, cx: &Cx) -> Result {
        cx.on::<Archive>().after("x", 1, "sav", ())
    }

    pub fn hourly(&mut self, cx: &Cx) -> Result {
        cx.on::<Archive>().every("tick", 1_000, "tick", ())
    }
}

#[cell]
pub struct Archive {
    pub saved: Vec<String>,
    pub ticks: u32,
}

#[methods]
impl Archive {
    pub fn save(&mut self, cx: &Cx, text: String) -> Result {
        ensure!(!text.is_empty(), "nothing to save");
        assert_eq!(cx.by(), &By::Schedule("autosave".into()));
        self.saved.push(text);
        Ok(())
    }

    pub fn tick(&mut self) {
        self.ticks += 1;
    }
}

#[test]
fn a_schedule_fires_another_cells_method() {
    let mut t = TestCells::new();
    let (d, a) = (t.cell::<Drafts>(), t.cell::<Archive>());
    d.edit("one".into()).unwrap();
    d.edit("two".into()).unwrap(); // the same key replaces: a debounce
    t.advance(2_999);
    assert!(a.state().saved.is_empty());
    t.advance(1);
    assert_eq!(a.state().saved, ["two"]);
    t.advance(10_000);
    assert_eq!(a.state().saved, ["two"], "once");
    // Cancelled with the cell's own schedules.
    d.edit("three".into()).unwrap();
    d.stop().unwrap();
    t.advance(5_000);
    assert_eq!(a.state().saved, ["two"]);
    // A repeat, re-armed each time.
    d.hourly().unwrap();
    t.advance(1_000);
    t.advance(1_000);
    assert_eq!(a.state().ticks, 2);
    // A typo refuses the call that sets it; a refusal where it fires is the schedule's failure.
    let e = d.typo().unwrap_err();
    assert!(e.why.contains("archive has no method `sav`"), "{e}");
    d.edit(String::new()).unwrap();
    t.advance(3_000);
    let f = t.failures();
    assert_eq!(f, ["Archive.save: nothing to save (schedule `autosave`)"], "the cell that refused it");
}

#[test]
fn a_schedule_on_a_cell_the_app_doesnt_hold_is_a_failure() {
    let mut t = TestCells::new();
    let d = t.cell::<Drafts>();
    d.edit("x".into()).unwrap();
    t.advance(3_000);
    let f = t.failures();
    assert!(f.iter().any(|f| f.contains("archive is not one of the app's cells")), "{f:?}");
}

/// aio's client log in a release: a page's console posted to its server
/// lands in the log as `[page]` lines; `[server] page_log = false` takes none.
#[tokio::test(flavor = "multi_thread")]
async fn a_pages_console_reaches_the_servers_log_unless_turned_off() {
    let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let keep = lines.clone();
    aio::log::set_sink(Some(std::sync::Arc::new(move |_, l: &str| keep.lock().unwrap().push(l.into()))));
    let body = r#"[{"level":"warn","msg":"page-probe low disk"}]"#;
    let json = [("Content-Type", "application/json")];
    let on = aio::sync::Options { page_log: true, ..Default::default() };
    let s = aio::testing::TestServer::start_with(Handle::<Shop>::memory(), on).await.unwrap();
    assert_eq!(s.page().fetch_with("POST", "/__aio/log", &json, body).await.unwrap().status, 204);
    let off = aio::testing::TestServer::start_with(Handle::<Shop>::memory(), Default::default()).await.unwrap();
    let refused = off.page().fetch_with("POST", "/__aio/log", &json, body).await.unwrap().status;
    aio::log::set_sink(None);
    let said: Vec<String> = lines.lock().unwrap().iter().filter(|l| l.contains("page-probe")).cloned().collect();
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(said[0].contains("WARN") && said[0].contains("[page] page-probe low disk"), "{said:?}");
    assert_ne!(refused, 204, "off: nothing taken");
}

/// `/__aio/log` is a door like health: a stranger on the network (here, a
/// request a proxy relayed) needs the key, as a bearer or the page's key
/// cookie; this machine's own tools need none.
#[tokio::test(flavor = "multi_thread")]
async fn a_strangers_page_log_needs_the_key() {
    let on = aio::sync::Options { page_log: true, ..Default::default() };
    let s = aio::testing::TestServer::start_with(Handle::<Shop>::memory(), on).await.unwrap();
    let body = r#"[{"msg":"x"}]"#;
    let relayed = ("X-Forwarded-For", "192.0.2.7");
    let post = |h: Vec<(&'static str, String)>| {
        let s = &s;
        async move {
            let json = std::iter::once(("Content-Type", "application/json"));
            let h: Vec<(&str, &str)> = json.chain(h.iter().map(|(k, v)| (*k, v.as_str()))).collect();
            s.fetch_with("POST", "/__aio/log", &h, body).await.unwrap().status
        }
    };
    let far = |more: Option<(&'static str, String)>| {
        std::iter::once((relayed.0, relayed.1.to_string())).chain(more).collect::<Vec<_>>()
    };
    assert_eq!(post(far(None)).await, 401, "no key from afar");
    assert_eq!(post(far(Some(("Authorization", "Bearer nope".into())))).await, 401, "a wrong key");
    assert_eq!(post(far(Some(("Authorization", format!("Bearer {}", s.token))))).await, 204);
    assert_eq!(post(far(Some(("Cookie", format!("aio_key_{}={}", Shop::ID, s.token))))).await, 204, "a page's cookie");
    assert_eq!(post(vec![]).await, 204, "this machine");
}

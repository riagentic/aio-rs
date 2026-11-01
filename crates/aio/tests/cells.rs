use aio::prelude::*;
use aio::{CellHealth, Cells};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

#[cell]
pub struct Counter {
    pub n: i64,
    pub ticks: u32,
    pub status: String,
}

#[methods]
impl Counter {
    pub fn add(&mut self, n: i64) -> Result {
        ensure!(n > 0, "add a positive number");
        self.n += n;
        Ok(())
    }

    pub fn set_status(&mut self, s: String) {
        self.status = s;
    }

    pub fn explode(&mut self) {
        panic!("boom");
    }

    /// An effect whose result is refused: the cell's own failure.
    pub fn fail_later(&mut self, cx: &Cx) {
        cx.run(Effect::new("fail", |h: Handle<Counter>| async move { _ = h.add(-1) }));
    }

    pub fn wait(&mut self, cx: &Cx) {
        cx.run(Effect::new("wait", |h: Handle<Counter>| async move {
            tokio::time::sleep(Duration::from_secs(30)).await;
            _ = h.add(1);
        }));
    }

    pub fn start_ticking(&mut self, cx: &Cx) -> Result {
        cx.every("tick", 10, "tick", ())
    }

    pub fn tick(&mut self) {
        self.ticks += 1;
    }
}

#[cell]
pub struct Base {
    pub hits: u32,
}

#[methods]
impl Base {
    pub fn hit(&mut self) {
        self.hits += 1;
    }
}

/// The breaker test's own cell: the app's health is one per process.
#[cell]
pub struct Flaky {
    pub n: i64,
}

#[methods]
impl Flaky {
    pub fn add(&mut self, n: i64) -> Result {
        ensure!(n > 0, "add a positive number");
        self.n += n;
        Ok(())
    }

    pub fn fail_later(&mut self, cx: &Cx) {
        cx.run(Effect::new("fail", |h: Handle<Flaky>| async move { _ = h.add(-1) }));
    }
}

/// The breaker on a saved cell: its data must outlive the trip and a restart.
#[cell(persist)]
pub struct Kept {
    pub items: Vec<String>,
}

#[methods]
impl Kept {
    pub fn add(&mut self, x: String) -> Result {
        ensure!(!x.is_empty(), "nothing to add");
        self.items.push(x);
        Ok(())
    }

    pub fn flaky(&mut self, cx: &Cx) {
        cx.run(Effect::new("flaky", |h: Handle<Kept>| async move { _ = h.add(String::new()) }));
    }
}

#[cell]
pub struct Upper {
    pub sent: u32,
}

/// The `Base` an `Upper` effect reaches (one test uses it).
static BASE: OnceLock<Handle<Base>> = OnceLock::new();

#[methods]
impl Upper {
    /// Reaches `Base` after `ms`: during a shutdown, Base must still be up.
    pub fn relay(&mut self, cx: &Cx, ms: u64) {
        self.sent += 1;
        cx.run(Effect::new("relay", move |_: Handle<Upper>| async move {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            _ = BASE.get().map(|b| b.hit());
        }));
    }
}

/// A cell whose method blocks its caller (aio's `worker: true`'s case).
#[cell]
pub struct Slow {
    pub done: u32,
}

#[methods]
impl Slow {
    pub fn block(&mut self, ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
        self.done += 1;
    }

    /// Block until the test opens `gate`: as long as it takes, not a guess.
    pub fn hold(&mut self, gate: usize) {
        let mut open = GATES.0.lock().unwrap();
        while !open[gate] {
            open = GATES.1.wait(open).unwrap();
        }
        self.done += 1;
    }

    pub fn stall_soon(&mut self, cx: &Cx) -> Result {
        cx.after("stall", 0, "hold", (1,))
    }
}

/// Gates `Slow::hold` waits on, one per test.
static GATES: (std::sync::Mutex<[bool; 2]>, std::sync::Condvar) =
    (std::sync::Mutex::new([false; 2]), std::sync::Condvar::new());

fn open(gate: usize) {
    GATES.0.lock().unwrap()[gate] = true;
    GATES.1.notify_all();
}

/// A source that never knows who listens (aio's `listensTo`'s case).
#[cell]
pub struct Session {
    pub user: Option<String>,
}

#[methods]
impl Session {
    pub fn login(&mut self, user: String) {
        self.user = Some(user);
    }

    pub fn logout(&mut self) {
        self.user = None;
    }
}

#[cell]
pub struct Prefs {
    pub theme: String,
    pub greeted: Vec<String>,
    pub cleared_by: Option<By>,
}

#[methods]
impl Prefs {
    pub fn greet(&mut self, user: String) -> Result {
        ensure!(user != "mallory", "not greeting {user}");
        self.greeted.push(user);
        Ok(())
    }

    pub fn clear(&mut self, cx: &Cx) {
        self.theme.clear();
        self.cleared_by = Some(cx.by().clone());
    }

    pub fn set_theme(&mut self, t: String) {
        self.theme = t;
    }
}

async fn until(what: &str, f: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

fn row(cells: &Cells, id: &str) -> CellHealth {
    cells.health().into_iter().find(|r| r.id == id).unwrap()
}

#[tokio::test]
async fn cells_start_in_dependency_order_and_a_bad_list_refuses_to_boot() {
    let cells = Cells::default()
        .with(Handle::<Upper>::memory(), &[Base::ID, Counter::ID])
        .with(Handle::<Counter>::memory(), &[Base::ID])
        .with(Handle::<Base>::memory(), &[])
        .start()
        .unwrap();
    assert_eq!(cells.ids(), [Base::ID, Counter::ID, Upper::ID], "listed out of order on purpose");

    let err = |c: Cells| c.start().err().unwrap().0;
    let dup = Cells::default().with(Handle::<Base>::memory(), &[]).with(Handle::<Base>::memory(), &[]);
    assert!(err(dup).contains("duplicate cell name `base`"));
    let unknown = Cells::default().with(Handle::<Base>::memory(), &["missing"]);
    assert!(err(unknown).contains("depends on unknown cell `missing`"));
    let cycle =
        Cells::default().with(Handle::<Base>::memory(), &[Upper::ID]).with(Handle::<Upper>::memory(), &[Base::ID]);
    assert_eq!(err(cycle), "dependency cycle: base → upper → base");
}

#[tokio::test]
async fn a_disabled_cell_refuses_calls_pauses_schedules_and_keeps_its_state() {
    let h = Handle::<Counter>::memory();
    let cells = Cells::default().with(h.clone(), &[]).start().unwrap();
    h.add(5).unwrap();
    h.start_ticking().unwrap();
    until("ticks", || h.state().ticks > 0).await;
    h.own("watcher", Some(Box::new(())));
    h.wait().unwrap();

    cells.disable(Counter::ID).unwrap();
    assert!(!cells.enabled(Counter::ID) && !h.enabled());
    assert_eq!(h.add(1).unwrap_err().why, "counter is disabled", "every door refuses");
    assert!(!h.owns("watcher"), "what it owned is dropped");
    h.done("wait").await;
    let ticks = h.state().ticks;
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(h.state().ticks, ticks, "its schedules are paused");
    assert_eq!(h.state().n, 5, "the state stays");
    assert_eq!(cells.status(Counter::ID).as_deref(), Some("disabled"));

    cells.enable(Counter::ID).unwrap();
    h.add(1).unwrap();
    assert_eq!(h.state().n, 6, "routed again, from the state it had");
    until("ticks again", || h.state().ticks > ticks).await;
    assert!(cells.disable("nope").unwrap_err().why.contains("unknown cell `nope`"));
}

#[tokio::test]
async fn health_has_a_row_per_cell_with_status_failures_and_the_last_call() {
    let (c, b) = (Handle::<Counter>::memory(), Handle::<Base>::memory());
    let cells = Cells::default().with(c.clone(), &[]).with(b.clone(), &[]).start().unwrap();
    let rows = cells.health();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.enabled && r.failures == 0 && r.last_call.is_none()));
    assert_eq!(row(&cells, Base::ID).status, None, "no status field");

    c.set_status("saving".into()).unwrap();
    assert_eq!(cells.status(Counter::ID).as_deref(), Some("saving"), "the cell's own status field");
    assert_eq!(row(&cells, Counter::ID).last_call.map(|l| l.0).as_deref(), Some("set_status"));
}

#[tokio::test]
async fn panics_and_refused_effect_results_are_failures_a_persons_refusal_is_not() {
    let h = Handle::<Counter>::memory();
    let cells = Cells::default().with(h.clone(), &[]).start().unwrap();
    assert!(h.add(-1).is_err());
    assert_eq!(row(&cells, Counter::ID).failures, 0, "a refusal of a caller is an answer");

    assert!(h.explode().is_err());
    h.fail_later().unwrap();
    h.done("fail").await;
    assert_eq!(row(&cells, Counter::ID).failures, 2);
    assert!(cells.enabled(Counter::ID), "no breaker: it stays enabled");

    cells.disable(Counter::ID).unwrap();
    assert_eq!(row(&cells, Counter::ID).failures, 2, "a disable keeps the count");
    cells.enable(Counter::ID).unwrap();
    assert_eq!(row(&cells, Counter::ID).failures, 0, "an enable starts it from zero");
}

#[tokio::test]
async fn the_breaker_disables_a_failing_cell_and_keeps_its_data() {
    let h = Handle::<Flaky>::memory();
    let cells = Cells::default().with(h.clone(), &[]).trip_after(3, None).start().unwrap();
    h.add(7).unwrap();
    for _ in 0..2 {
        h.fail_later().unwrap();
        h.done("fail").await;
    }
    assert!(cells.enabled(Flaky::ID), "below the threshold");
    h.fail_later().unwrap();
    h.done("fail").await;
    assert!(!cells.enabled(Flaky::ID), "tripped at 3");
    assert_eq!(row(&cells, Flaky::ID).failures, 3);
    assert_eq!(h.state().n, 7, "a trip keeps the data");
    assert!(aio::health::degraded().iter().any(|(n, _)| n == "cell flaky"), "the app's health says so");

    cells.enable(Flaky::ID).unwrap();
    assert!(!aio::health::degraded().iter().any(|(n, _)| n == "cell flaky"), "recovered");
}

#[tokio::test]
async fn a_trip_keeps_saved_data_through_enable_and_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("kept.db");
    let h = Handle::<Kept>::open(&db).unwrap();
    let cells = Cells::default().with(h.clone(), &[]).trip_after(3, None).start().unwrap();
    h.add("a".into()).unwrap();
    for _ in 0..3 {
        h.flaky().unwrap();
        h.done("flaky").await;
    }
    assert!(!cells.enabled(Kept::ID), "tripped at 3");
    assert_eq!(h.state().items, ["a"], "right away");
    assert!(h.add("b".into()).is_err(), "a tripped cell refuses calls");
    cells.enable(Kept::ID).unwrap();
    assert_eq!(h.state().items, ["a"], "after the enable");
    assert!(cells.shutdown(Duration::from_secs(1)).await.is_empty());
    drop((cells, h));
    assert_eq!(Handle::<Kept>::open(&db).unwrap().state().items, ["a"], "after a restart");
}

#[tokio::test]
async fn shutdown_goes_in_reverse_order_so_dependencies_outlive_their_dependents() {
    let base = BASE.get_or_init(Handle::<Base>::memory).clone();
    let upper = Handle::<Upper>::memory();
    let cells = Cells::default().with(upper.clone(), &[Base::ID]).with(base.clone(), &[]).start().unwrap();
    upper.relay(40).unwrap();
    let aborted = cells.shutdown(Duration::from_secs(5)).await;
    assert!(aborted.is_empty());
    assert_eq!(base.state().hits, 1, "Upper's effect reached Base while Upper drained");
    assert!(base.closing() && upper.closing());

    let c = Handle::<Counter>::memory();
    let cells = Cells::default().with(c.clone(), &[]).start().unwrap();
    c.wait().unwrap();
    assert_eq!(cells.shutdown(Duration::from_millis(20)).await, ["counter: wait"], "named by cell");
}

/// aio runs such a cell in its own worker; in aio-rs each cell is its own actor
/// (one lock per cell, network calls on the blocking pool), so no worker is needed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_blocking_cell_never_delays_another_cells_calls_or_schedules() {
    let (slow, c) = (Handle::<Slow>::memory(), Handle::<Counter>::memory());
    let _cells = Cells::default().with(slow.clone(), &[]).with(c.clone(), &[]).start().unwrap();
    c.start_ticking().unwrap();
    let s = slow.clone();
    let blocked = tokio::task::spawn_blocking(move || s.hold(0));
    tokio::time::sleep(Duration::from_millis(20)).await;
    let add = tokio::task::spawn_blocking({
        let c = c.clone();
        move || c.add(1)
    });
    // A hang guard: the slow call never ends until the gate opens.
    tokio::time::timeout(Duration::from_secs(10), add)
        .await
        .expect("another cell's call runs at once")
        .unwrap()
        .unwrap();
    let ticks = c.state().ticks;
    until("its schedule ticks meanwhile", || c.state().ticks > ticks + 3).await;
    assert_eq!(slow.state().done, 0, "its call still runs, and its state stays readable");
    // Its own calls keep their order: the next one waits for the first.
    let s = slow.clone();
    let next = tokio::task::spawn_blocking(move || s.block(0));
    open(0);
    next.await.unwrap().unwrap();
    blocked.await.unwrap().unwrap();
    assert_eq!(slow.state().done, 2);
}

/// Its schedules too: a slow scheduled method holds no runtime thread, even
/// on a one-thread runtime.
#[tokio::test(flavor = "current_thread")]
async fn a_blocking_schedule_never_stalls_the_runtime() {
    let (slow, c) = (Handle::<Slow>::memory(), Handle::<Counter>::memory());
    let _cells = Cells::default().with(slow.clone(), &[]).with(c.clone(), &[]).start().unwrap();
    c.start_ticking().unwrap();
    slow.stall_soon().unwrap();
    let ticks = c.state().ticks;
    until("ticks while the other cell's schedule blocks", || c.state().ticks > ticks + 3).await;
    assert_eq!(slow.state().done, 0, "its scheduled call still runs");
    open(1);
    until("the slow call", || slow.state().done == 1).await;
}

/// aio tests/method-workflows.test.ts (`listensTo`).
#[test]
fn a_cell_reacts_to_another_cells_calls_with_their_arguments_and_caller() {
    let (session, prefs) = (Handle::<Session>::memory(), Handle::<Prefs>::memory());
    prefs.listens_to(&session, "logout", |p, _| p.clear());
    prefs.listens_to(&session, "login", |p, e| p.greet(aio::args::<(String,)>(e.args.clone())?.0));
    prefs.set_theme("dark".into()).unwrap();

    session.by(By::User("ana".into())).login("ana".into()).unwrap();
    assert_eq!(prefs.state().greeted, ["ana"], "it got the call's arguments");
    assert_eq!(prefs.state().theme, "dark", "only the method it listens to");
    session.by(By::User("ana".into())).logout().unwrap();
    assert_eq!((prefs.state().theme.as_str(), prefs.state().cleared_by.clone()), ("", Some(By::User("ana".into()))));
    assert_eq!(prefs.store(|s| s.events().unwrap()).last().unwrap().by, By::User("ana".into()), "same caller");

    // A refused reaction never undoes the call it reacts to.
    session.login("mallory".into()).unwrap();
    assert_eq!(session.state().user.as_deref(), Some("mallory"));
    assert_eq!(prefs.state().greeted, ["ana"]);
}

/// A reaction to a method the source doesn't have is a wiring mistake,
/// said as the app wires it (else it never fires).
#[test]
#[should_panic(expected = "has no method `logot`")]
fn listening_to_a_method_that_isnt_there_panics_at_wiring() {
    let (session, prefs) = (Handle::<Session>::memory(), Handle::<Prefs>::memory());
    prefs.listens_to(&session, "logot", |p, _| p.clear());
}

/// aio's `aio.run({ onStart, onRestore, fatalOnStart })` and `getFullState()`.
#[tokio::test]
async fn app_hooks_run_around_the_start_and_see_every_cell() {
    let said = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    let (a, b) = (said.clone(), said.clone());
    let cells = Cells::default()
        .with(Handle::<Base>::memory(), &[])
        .with(Handle::<Counter>::memory(), &[Base::ID])
        .on_restore(move |c| {
            // Not live yet: a repair across cells, through their methods.
            c.get::<Base>().unwrap().hit()?;
            a.lock().unwrap().push("restore".to_string());
            Ok(())
        })
        .on_start(move |c| {
            c.get::<Counter>().unwrap().add(2)?;
            b.lock().unwrap().push(format!("start {}", serde_json::Value::Object(c.view())));
            Ok(())
        })
        .start()
        .unwrap();
    assert_eq!(
        *said.lock().unwrap(),
        ["restore".to_string(), r#"start {"base":{"hits":1},"counter":{"n":2,"status":"","ticks":0}}"#.to_string()]
    );
    assert!(cells.get::<Upper>().is_none(), "not in the list");

    let refusing = || Cells::default().with(Handle::<Base>::memory(), &[]).on_start(|_| aio::reject!("no seed data"));
    assert!(refusing().start().is_ok(), "said, and the start goes on");
    let e = refusing().fatal_on_start().start().err().unwrap();
    assert!(e.0.contains("on_start refused at start: no seed data"), "{}", e.0);
}

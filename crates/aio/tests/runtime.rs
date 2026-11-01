use aio::prelude::*;
use std::time::Duration;

#[cell(persist)]
pub struct Feed {
    pub query: String,
    pub results: Vec<String>,
    pub ticks: u32,
    pub saves: u32,
    pub fetches: u32,
}

/// Fake I/O: answers after `ms`.
async fn search(q: String, ms: u64) -> Vec<String> {
    tokio::time::sleep(Duration::from_millis(ms)).await;
    vec![format!("{q}!")]
}

#[methods]
impl Feed {
    pub fn typed(&mut self, cx: &Cx, q: String, ms: u64, mode: u8) -> Result {
        self.query = q.clone();
        let mode = [Mode::Queue, Mode::Newest, Mode::First][mode as usize];
        cx.run(
            Effect::new("search", move |h: Handle<Feed>| async move {
                let r = search(q.clone(), ms).await;
                _ = h.found(q, r);
            })
            .mode(mode),
        );
        Ok(())
    }

    /// A refresh per path: a second scan of a path already scanning shares it.
    pub fn scan(&mut self, cx: &Cx, path: String, ms: u64) -> Result {
        let p = path.clone();
        let e = Effect::new("scan", move |h: Handle<Feed>| async move {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            _ = h.scanned(p);
        });
        cx.run(e.per(&path)?.mode(Mode::First));
        Ok(())
    }

    pub fn scanned(&mut self, path: String) {
        self.results.push(path);
    }

    /// Saves in order: no I/O, so only the queue keeps them in line.
    pub fn queued(&mut self, cx: &Cx, n: u32) {
        cx.run(Effect::new("q", move |h: Handle<Feed>| async move { _ = h.scanned(n.to_string()) }));
    }

    pub fn stop_search(&mut self, cx: &Cx) {
        cx.abort("search");
    }

    pub fn found(&mut self, q: String, r: Vec<String>) {
        self.fetches += 1;
        if q == self.query {
            self.results = r;
        }
    }

    pub fn slow(&mut self, cx: &Cx) {
        cx.run(
            Effect::new("slow", |h: Handle<Feed>| async move {
                tokio::time::sleep(Duration::from_secs(5)).await;
                _ = h.found("late".into(), vec![]);
            })
            .timeout(Duration::from_millis(30)),
        );
    }

    /// Asks for an effect, then refuses: the effect must never start.
    pub fn bad(&mut self, cx: &Cx) -> Result {
        cx.run(Effect::new("bad", |h: Handle<Feed>| async move { _ = h.found("bad".into(), vec![]) }));
        reject!("no")
    }

    pub fn edit(&mut self, cx: &Cx) -> Result {
        // Debounced autosave: each edit moves it.
        cx.after("autosave", 40, "save", ())
    }

    /// An effect that would outlive any shutdown budget.
    pub fn long(&mut self, cx: &Cx) {
        cx.run(Effect::new("long", |h: Handle<Feed>| async move {
            tokio::time::sleep(Duration::from_secs(30)).await;
            _ = h.found("never".into(), vec![]);
        }));
    }

    pub fn remind(&mut self, cx: &Cx, ms: i64) -> Result {
        cx.after("remind", ms, "save", ())
    }

    /// Changes the state, then panics: the change must not survive.
    pub fn explode(&mut self) {
        self.ticks += 100;
        panic!("boom");
    }

    pub fn save(&mut self) {
        self.saves += 1;
    }

    pub fn start_ticking(&mut self, cx: &Cx, ms: i64) -> Result {
        cx.every("tick", ms, "tick", ())
    }

    pub fn stop_ticking(&mut self, cx: &Cx) {
        cx.cancel("tick");
    }

    pub fn tick(&mut self) {
        self.ticks += 1;
    }

    pub fn missed(&mut self, cx: &Cx) -> Result {
        cx.at("old", cx.now() - 60_000, "tick", ())?;
        cx.every("old-every", 10, "tick", ())
    }

    pub fn refusing(&mut self, cx: &Cx) -> Result {
        cx.after("r", 0, "refuse", ())
    }

    pub fn refuse(&mut self) -> Result {
        aio::reject!("not now")
    }

    pub fn misspelt(&mut self, cx: &Cx) -> Result {
        cx.after("m", 10, "tik", ())
    }

    pub fn weekly(&mut self, cx: &Cx, expr: String) -> Result {
        cx.cron("weekly", &expr, "tick", ())
    }
}

async fn until(what: &str, f: impl Fn() -> bool) {
    for _ in 0..200 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out: {what}");
}

#[tokio::test(flavor = "multi_thread")]
async fn effects_run_after_commit_and_report_back() {
    let h = Handle::<Feed>::memory();
    h.typed("a".into(), 10, 0).unwrap();
    until("found", || h.state().results == ["a!"]).await;
    let ev = h.store(|s| s.events().unwrap());
    assert_eq!(ev[1].method, "found");
    assert_eq!(ev[1].by, By::Effect("search".into()));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_call_starts_no_effect() {
    let h = Handle::<Feed>::memory();
    assert!(h.bad().is_err());
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(h.state().fetches, 0);
    assert!(!h.busy("bad"));
}

#[tokio::test(flavor = "multi_thread")]
async fn newest_cancels_first_skips_queue_keeps_all() {
    let h = Handle::<Feed>::memory();
    h.typed("a".into(), 80, 1).unwrap();
    h.typed("ab".into(), 10, 1).unwrap();
    until("ab", || h.state().results == ["ab!"]).await;
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert_eq!(h.state().fetches, 1, "the first search was cancelled");

    let h = Handle::<Feed>::memory();
    h.typed("a".into(), 50, 2).unwrap();
    h.typed("b".into(), 10, 2).unwrap();
    until("one", || h.state().fetches == 1).await;
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(h.state().fetches, 1, "the second was skipped");

    let h = Handle::<Feed>::memory();
    h.typed("a".into(), 30, 0).unwrap();
    h.typed("b".into(), 5, 0).unwrap();
    until("both", || h.state().fetches == 2).await;
    let order: Vec<_> = h
        .store(|s| s.events().unwrap())
        .into_iter()
        .filter(|e| e.method == "found")
        .map(|e| e.args[0].clone())
        .collect();
    assert_eq!(order, ["a", "b"], "queued in order");
}

#[tokio::test(flavor = "multi_thread")]
async fn first_shares_the_running_result_and_is_keyed_by_args() {
    let h = Handle::<Feed>::memory();
    h.scan("/a".into(), 60).unwrap();
    h.scan("/a".into(), 0).unwrap();
    h.scan("/b".into(), 0).unwrap();
    let a = aio::effect::key("scan", &"/a").unwrap();
    assert_eq!(a, r#"scan:"/a""#);
    assert!(h.busy(&a));
    // The second caller of /a waits for the first and reads its result.
    h.done(&a).await;
    assert!(h.state().results.contains(&"/a".to_string()));
    h.done(&aio::effect::key("scan", &"/b").unwrap()).await;
    let mut r = h.state().results.to_vec();
    r.sort();
    assert_eq!(r, ["/a", "/b"], "/a ran once, /b was not answered by /a");
    // Over: the next scan runs again.
    h.scan("/a".into(), 0).unwrap();
    h.done(&a).await;
    assert_eq!(h.state().results.len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_effects_start_in_call_order() {
    let h = Handle::<Feed>::memory();
    let n = 50;
    for i in 0..n {
        h.queued(i).unwrap();
    }
    h.done("q").await;
    let want: Vec<_> = (0..n).map(|i| i.to_string()).collect();
    assert_eq!(h.state().results, want);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_method_aborts_running_and_queued_effects_by_key() {
    let h = Handle::<Feed>::memory();
    h.typed("a".into(), 5000, 0).unwrap();
    h.typed("b".into(), 0, 0).unwrap();
    assert!(h.busy("search"));
    h.stop_search().unwrap();
    tokio::time::timeout(Duration::from_secs(1), h.done("search")).await.expect("aborted");
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(h.state().fetches, 0, "neither the running nor the queued one called back");
    // Nothing running: aborting is a no-op, and new effects run.
    h.stop_search().unwrap();
    h.typed("c".into(), 0, 0).unwrap();
    h.done("search").await;
    assert_eq!(h.state().results, ["c!"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn timeouts_cancel() {
    let h = Handle::<Feed>::memory();
    h.slow().unwrap();
    until("cancelled", || !h.busy("slow")).await;
    assert_eq!(h.state().fetches, 0);
}

#[test]
fn effects_need_a_runtime_and_never_replay() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("s.db");
    // No runtime: the call commits, the effect is dropped (and said so).
    Handle::<Feed>::open(&p).unwrap().typed("x".into(), 0, 0).unwrap();
    // Replay doesn't start effects either.
    let h = Handle::<Feed>::open(&p).unwrap();
    assert_eq!(h.state().query, "x");
    assert_eq!(h.state().fetches, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn debounced_one_shot_fires_once() {
    let h = Handle::<Feed>::memory();
    let _run = h.start();
    for _ in 0..5 {
        h.edit().unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    until("saved", || h.state().saves == 1).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(h.state().saves, 1);
    assert!(h.store(|s| s.schedules().unwrap()).is_empty());
    let ev = h.store(|s| s.events().unwrap());
    assert_eq!(ev.last().unwrap().by, By::Schedule("autosave".into()));
}

#[tokio::test(flavor = "multi_thread")]
async fn repeating_until_cancelled() {
    let h = Handle::<Feed>::memory();
    let _run = h.start();
    h.start_ticking(15).unwrap();
    until("3 ticks", || h.state().ticks >= 3).await;
    h.stop_ticking().unwrap();
    let n = h.state().ticks;
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(h.state().ticks, n);
}

#[test]
fn schedules_survive_restart_and_missed_runs_fire_once() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("s.db");
    Handle::<Feed>::open(&p).unwrap().missed().unwrap();
    // "Later": the app was off for a while.
    std::thread::sleep(Duration::from_millis(60));
    let mut s = Store::<Feed>::open(&p).unwrap();
    assert_eq!(s.schedules().unwrap().len(), 2);
    let now = aio_now();
    assert_eq!(s.fire_due(now).unwrap(), 2);
    assert_eq!(s.state().ticks, 2, "the one-shot and one catch-up tick, not six");
    let left = s.schedules().unwrap();
    assert_eq!(left.len(), 1);
    assert!(left[0].due > now);
}

#[test]
fn a_refused_schedule_is_removed_not_looped() {
    let mut s = Store::<Feed>::memory();
    s.call(&By::Test, "refusing", serde_json::Value::Null).unwrap();
    assert_eq!(s.fire_due(aio_now()).unwrap(), 1);
    assert!(s.schedules().unwrap().is_empty());
}

#[test]
fn a_schedule_naming_no_method_is_refused_when_set() {
    let mut s = Store::<Feed>::memory();
    let e = s.call(&By::Test, "misspelt", serde_json::Value::Null).unwrap_err();
    assert!(e.to_string().contains("schedule `m`: feed has no method `tik`"), "{e}");
    assert!(s.schedules().unwrap().is_empty() && s.events().unwrap().is_empty(), "the call changed nothing");
}

#[test]
fn a_cron_schedule_fires_and_moves_to_its_next_slot() {
    let mut s = Store::<Feed>::memory();
    s.call(&By::Test, "weekly", serde_json::json!(["0 9 * * 1"])).unwrap();
    let d = s.schedules().unwrap().remove(0);
    let cron = aio::cron::Cron::parse("0 9 * * 1").unwrap();
    assert_eq!(d.cron.as_deref(), Some("0 9 * * 1"));
    // The saved slot is a slot: the next one after the minute before it.
    assert_eq!(cron.next(d.due - 60_000), Some(d.due));
    assert!(d.due > aio_now() && d.due - aio_now() <= 7 * 86_400_000, "within a week");
    // Its slot arrives (or the app was asleep through it): one run, then
    // the next Monday 9:00, never the same slot again.
    assert_eq!(s.fire_due(d.due + 3_600_000).unwrap(), 1);
    assert_eq!(s.state().ticks, 1);
    let next = s.schedules().unwrap().remove(0);
    assert_eq!(Some(next.due), cron.next(d.due + 3_600_000));
    assert!(next.due - d.due >= 6 * 86_400_000, "a week on (give or take a DST hour)");
    // A bad expression is a refusal, not a schedule.
    assert!(s.call(&By::Test, "weekly", serde_json::json!(["0 25 * * *"])).is_err());
}

#[test]
fn a_store_from_before_cron_schedules_is_upgraded_in_place() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("s.db");
    {
        // Schema 1, as the previous aio wrote it, with a saved schedule.
        let db = rusqlite::Connection::open(&p).unwrap();
        db.execute_batch(
            "CREATE TABLE log (seq INTEGER PRIMARY KEY AUTOINCREMENT, cell TEXT NOT NULL, method TEXT NOT NULL,
               args TEXT NOT NULL, inputs TEXT NOT NULL, by TEXT NOT NULL, at INTEGER NOT NULL, v INTEGER NOT NULL);
             CREATE TABLE snap (cell TEXT NOT NULL, seq INTEGER NOT NULL, v INTEGER NOT NULL, state TEXT NOT NULL,
               PRIMARY KEY (cell, seq));
             CREATE TABLE floor (cell TEXT PRIMARY KEY, seq INTEGER NOT NULL);
             CREATE TABLE sched (cell TEXT NOT NULL, key TEXT NOT NULL, due INTEGER NOT NULL, every INTEGER,
               method TEXT NOT NULL, args TEXT NOT NULL, PRIMARY KEY (cell, key));
             CREATE TABLE ops (cell TEXT NOT NULL, op TEXT NOT NULL, seq INTEGER NOT NULL, PRIMARY KEY (cell, op));
             INSERT INTO sched VALUES ('feed', 'tick', 1, 1000, 'tick', 'null');
             PRAGMA user_version = 1;",
        )
        .unwrap();
    }
    let s = Store::<Feed>::open(&p).unwrap();
    let left = s.schedules().unwrap();
    assert_eq!((left.len(), left[0].every, left[0].cron.as_deref()), (1, Some(1000), None));
    drop(s);
    let v: i64 = rusqlite::Connection::open(&p).unwrap().query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(v, 2);
}

fn aio_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64
}

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_drains_effects_within_its_budget_and_refuses_new_calls() {
    let h = Handle::<Feed>::memory();
    let _sched = h.start();
    h.start_ticking(10).unwrap();
    h.typed("a".into(), 100, 0).unwrap();
    h.long().unwrap();
    let d = h.shutdown(Duration::from_millis(500)).await;
    // The quick effect finished and its commit landed; the long one was cut.
    assert_eq!(d.aborted, ["long"]);
    assert_eq!(h.state().results, ["a!"]);
    // New calls are refused with when to retry.
    let e = h.save().unwrap_err();
    assert!(e.why.contains("closing; retry after 2000 ms"), "{e:?}");
    // Schedules stopped.
    let ticks = h.state().ticks;
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(h.state().ticks, ticks);
}

#[test]
fn schedules_follow_the_wall_clock_through_steps_sleep_and_long_delays() {
    let h = Handle::<Feed>::memory();
    h.start_ticking(1000).unwrap();
    let n = h.store(|s| s.schedules().unwrap())[0].due - 1000;
    let fire = |t: i64| h.store(|s| s.fire_due(t).unwrap());
    assert_eq!(fire(n + 1000), 1);
    // The clock steps back: the slot that ran doesn't run again.
    assert_eq!(fire(n), 0);
    assert_eq!(fire(n + 1500), 0);
    // A long sleep: the missed slots fire once, then the rhythm restarts from now.
    assert_eq!(fire(n + 600_000), 1);
    assert_eq!(h.state().ticks, 2);
    assert_eq!(h.store(|s| s.schedules().unwrap())[0].due, n + 601_000);
    // 30 days ahead (past the 24.8-day limit of JS timers) is just a time.
    h.stop_ticking().unwrap();
    let day = 86_400_000;
    h.remind(30 * day).unwrap();
    let due = h.store(|s| s.schedules().unwrap())[0].due;
    assert_eq!(fire(due - day), 0);
    assert_eq!(fire(due), 1);
    assert_eq!(h.state().saves, 1);
}

#[test]
fn a_panicking_method_is_refused_keeps_the_state_and_three_stop_the_cell() {
    let h = Handle::<Feed>::memory();
    h.save().unwrap();
    for n in 1..=3 {
        let e = h.explode().unwrap_err();
        assert!(e.why.contains("feed.explode panicked: boom"), "{e:?}");
        assert_eq!((h.state().ticks, h.state().saves), (0, 1), "panic {n}: nothing half-done is kept");
    }
    // The third within a minute stops the cell: even good calls are refused.
    assert!(h.save().unwrap_err().why.contains("stopped after 3 panics"));
    assert_eq!(h.store(|s| s.events().unwrap().len()), 1);
}

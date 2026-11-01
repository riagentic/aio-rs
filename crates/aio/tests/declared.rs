//! Schedules a method declares (aio's `aio.run({ schedules })`): `#[every]`,
//! `#[cron]`, `skip_if_running`, and backoff for retries and polls.
use aio::Backoff;
use aio::prelude::*;
use aio::testing::TestCells;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::time::Duration;

#[cell(persist)]
pub struct Feed {
    pub ticks: u32,
    pub mornings: u32,
    pub retry_in: Vec<i64>,
}

#[cell]
pub struct Poller {
    pub begun: u32,
    pub done: u32,
}

static OPEN: AtomicBool = AtomicBool::new(false);

/// I/O that waits until the test lets it through.
async fn gate() -> u32 {
    while !OPEN.load(SeqCst) {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    1
}

#[methods]
impl Feed {
    #[every("1m")]
    pub fn tick(&mut self) {
        self.ticks += 1;
    }

    #[cron("0 9 * * *")]
    pub fn morning(&mut self) {
        self.mornings += 1;
    }

    /// Leaves an `@` schedule that no method declares.
    pub fn plant(&mut self, cx: &Cx) -> Result {
        cx.every("@gone", 1000, "tick", ())
    }

    pub fn fail(&mut self, cx: &Cx, attempt: u32) -> Result {
        let b = Backoff::from(1000);
        self.retry_in.push(b.ms(attempt));
        cx.backoff("retry", attempt, b, "tick", ())
    }
}

#[methods]
impl Poller {
    #[every(40, skip_if_running)]
    pub async fn poll(&mut self) -> Result {
        self.begun += 1;
        let n: u32 = gate().await;
        self.done += n;
        Ok(())
    }
}

#[test]
fn declared_schedules_are_armed_when_the_cell_goes_live() {
    let mut t = TestCells::new();
    let h = t.cell::<Feed>();
    let mut s = h.store(|s| s.schedules().unwrap());
    s.sort_by(|a, b| a.key.cmp(&b.key));
    let keys: Vec<_> = s.iter().map(|d| (d.key.as_str(), d.every, d.cron.as_deref())).collect();
    assert_eq!(keys, [("@morning", None, Some("0 9 * * *")), ("@tick", Some(60_000), None)]);
    t.advance(60_000);
    assert_eq!(h.state().ticks, 1);
    t.advance(180_000);
    assert_eq!(h.state().ticks, 2, "missed ones fire once");
    t.advance(86_400_000);
    assert_eq!(h.state().mornings, 1);
}

#[test]
fn a_retry_waits_longer_each_time() {
    let b = Backoff::from(1000);
    assert_eq!([0, 1, 3, 30].map(|a| b.ms(a)), [1000, 2000, 8000, 3_600_000], "doubling, at most an hour");
    let poll = Backoff { base: 5000, factor: 1.0, max: 60_000 };
    assert_eq!([0, 7].map(|a| poll.ms(a)), [5000, 5000], "a poll keeps its interval");
    let grow = Backoff { base: 5000, factor: 1.5, max: 10_000 };
    assert_eq!([1, 9].map(|a| grow.ms(a)), [7500, 10_000]);
    let mut t = TestCells::new();
    let h = t.cell::<Feed>();
    h.fail(2).unwrap();
    let retry = h.store(|s| s.schedules().unwrap()).into_iter().find(|d| d.key == "retry").unwrap();
    assert_eq!((h.state().retry_in.clone(), retry.every), (vec![4000], None));
    t.advance(3999);
    assert_eq!(h.state().ticks, 0);
    t.advance(1);
    assert_eq!(h.state().ticks, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_kept_schedule_keeps_its_time_and_a_stale_one_goes() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.db");
    let h = Handle::<Feed>::open(&p).unwrap();
    drop(h.start());
    let due =
        |h: &Handle<Feed>, k: &str| h.store(|s| s.schedules().unwrap()).into_iter().find(|d| d.key == k).map(|d| d.due);
    let tick = due(&h, "@tick").unwrap();
    h.plant().unwrap();
    assert!(due(&h, "@gone").is_some());
    h.shutdown(Duration::from_secs(1)).await;
    drop(h);
    let h = Handle::<Feed>::open(&p).unwrap();
    drop(h.start());
    assert_eq!((due(&h, "@tick"), due(&h, "@gone")), (Some(tick), None));
    h.shutdown(Duration::from_secs(1)).await;
}

#[test]
fn a_tick_is_skipped_while_the_last_one_still_runs() {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let h = Handle::<Poller>::memory();
    rt.block_on(async {
        drop(h.start());
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!((h.state().begun, h.state().done), (1, 0), "7 ticks came; one ran");
        OPEN.store(true, SeqCst);
        h.until(Duration::from_secs(5), |s| s.done >= 2).await.unwrap();
        assert!(h.state().begun >= 2);
        h.shutdown(Duration::from_secs(1)).await;
    });
}

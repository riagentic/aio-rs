//! aio's async helpers in an effect: `until` a condition on the state, and
//! `aio::time` (sleep, timeout as a race), on tokio's virtual clock in tests.
use aio::prelude::*;
use std::time::Duration;

#[cell]
pub struct Checkout {
    pub confirmed: bool,
    pub status: String,
}

#[methods]
impl Checkout {
    /// aio: `s.status = "paying"; await until(() => s.confirmed, { timeoutMs }); s.status = "paid"`.
    pub fn pay(&mut self, cx: &Cx, wait_ms: u64) {
        self.status = "paying".into();
        cx.run(Effect::new("pay", move |h: Handle<Checkout>| async move {
            let r = h.until(Duration::from_millis(wait_ms), |c| c.confirmed).await;
            _ = h.settled(r.err().map(|e| e.why));
        }));
    }

    pub fn confirm(&mut self) {
        self.confirmed = true;
    }

    pub fn settled(&mut self, why: Option<String>) {
        self.status = why.unwrap_or_else(|| "paid".into());
    }
}

#[tokio::test(start_paused = true)]
async fn an_effect_waits_until_the_state_says_so_or_gives_up() {
    let h = Handle::<Checkout>::memory();
    h.pay(30_000).unwrap();
    aio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(h.state().status, "paying", "ten virtual seconds in, still waiting");
    h.confirm().unwrap();
    h.done("pay").await;
    assert_eq!(h.state().status, "paid");

    let h = Handle::<Checkout>::memory();
    let t0 = aio::time::Instant::now();
    h.pay(5_000).unwrap();
    h.done("pay").await;
    assert_eq!(t0.elapsed(), Duration::from_secs(5));
    assert_eq!(h.state().status, "checkout: the condition did not hold within 5s");
    h.confirm().unwrap();
    assert_eq!(h.until(Duration::ZERO, |c| c.confirmed).await, Ok(()), "already true: no wait");
}

#[tokio::test(start_paused = true)]
async fn a_race_with_a_timeout_takes_the_first_to_finish() {
    let fast = aio::time::timeout(Duration::from_secs(2), aio::time::sleep(Duration::from_secs(1))).await;
    let slow = aio::time::timeout(Duration::from_secs(1), aio::time::sleep(Duration::from_secs(2))).await;
    assert!(fast.is_ok() && slow.is_err());
}

//! The call ceiling (aio's default 30 s): an await of an async method that
//! never settles is cut off, a `#[long]` one may run as long as it needs;
//! `retry` is aio's `call({ timeoutMs, retries })`.
use aio::prelude::*;
use std::sync::atomic::{AtomicU32, Ordering::SeqCst};
use std::time::Duration;

#[cell]
pub struct Job {
    pub status: String,
}

async fn never() -> u32 {
    std::future::pending().await
}

#[methods]
impl Job {
    pub async fn hang(&mut self) -> Result {
        self.status = "waiting".into();
        let n: u32 = never().await;
        self.status = format!("got {n}");
        Ok(())
    }

    #[long]
    pub async fn colorize(&mut self) -> Result {
        self.status = "colorizing".into();
        let n: u32 = async {
            tokio::time::sleep(Duration::from_secs(3600)).await;
            7
        }
        .await;
        self.status = format!("done {n}");
        Ok(())
    }
}

#[tokio::test(start_paused = true)]
async fn an_await_past_the_ceiling_is_cut_off_and_a_long_one_is_not() {
    let h = Handle::<Job>::memory();
    h.hang().unwrap();
    let t0 = tokio::time::Instant::now();
    h.done("hang").await;
    assert_eq!(t0.elapsed(), aio::effect::CALL_CEILING, "cut off at the ceiling");
    assert_eq!(h.state().status, "waiting", "its later steps never ran");

    h.colorize().unwrap();
    tokio::time::sleep(aio::effect::CALL_CEILING * 2).await;
    assert!(h.busy("colorize"), "a #[long] method is past the ceiling and still running");
    h.done("colorize").await;
    assert_eq!(h.state().status, "done 7");
}

#[tokio::test(start_paused = true)]
async fn retry_tries_again_while_it_fails_each_try_timed() {
    let tries = AtomicU32::new(0);
    let flaky = || async {
        let n = tries.fetch_add(1, SeqCst);
        if n < 2 { Err(format!("down {n}")) } else { Ok(n) }
    };
    assert_eq!(aio::effect::retry(2, None, flaky).await, Ok(2), "the third try");
    tries.store(0, SeqCst);
    assert_eq!(aio::effect::retry(1, None, flaky).await, Err(Rejected::new("down 1")), "the last failure");
    assert_eq!(tries.load(SeqCst), 2);

    let slow = || async {
        tokio::time::sleep(Duration::from_secs(10)).await;
        Ok::<_, String>(1)
    };
    let e = aio::effect::retry(1, Some(Duration::from_secs(5)), slow).await;
    assert_eq!(e, Err(Rejected::new("timed out after 5s")));
    assert_eq!(aio::effect::retry(0, Some(Duration::from_secs(11)), slow).await, Ok(1));
}

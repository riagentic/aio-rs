// Its own test binary: the log's sink is process-wide.
use aio::prelude::*;
use aio::sync::{Remote, serve};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A server cell that tells the people using the app (aio's `notify()`).
#[cell(shared)]
pub struct Jobs {
    pub done: u32,
}

#[methods]
impl Jobs {
    #[client]
    pub fn finish(&mut self, cx: &Cx) -> Result {
        self.done += 1;
        cx.tell(aio::tell::Tell::notify("Export", "the report is ready"))
    }
}

/// aio's `connectCli`: a client with no window can show no card, so it
/// says the sentence in its log, never drops it silently.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_with_no_window_says_what_it_was_told() {
    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    let keep = lines.clone();
    aio::log::set_sink(Some(Arc::new(move |_, l: &str| keep.lock().unwrap().push(l.into()))));
    let h = Handle::<Jobs>::memory();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(serve(h.clone(), l, "secret".into()));
    let cli = Remote::<Jobs>::connect(&addr, "secret", "ana").await.unwrap();
    cli.finish().unwrap();
    let said = || lines.lock().unwrap().iter().any(|l| l.contains("[cli] notify: Export — the report is ready"));
    let end = std::time::Instant::now() + Duration::from_secs(10);
    while !said() && std::time::Instant::now() < end {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(said(), "{:?}", lines.lock().unwrap());
}

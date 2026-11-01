// Its own test binary: the reporter, its sink and the feedback cell are global.
use aio::feedback::{FeedbackCalls, MAX, Status};
use aio::prelude::*;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cell(persist)]
pub struct Notes {
    pub text: String,
}

#[methods]
impl Notes {
    pub fn set(&mut self, t: String) {
        self.text = t;
    }
}

/// What the sink does next: take it, refuse it, or never answer.
#[derive(Clone, Copy, PartialEq)]
enum Sink {
    Take,
    Refuse,
    Hang,
}

async fn until(what: &str, f: impl Fn() -> bool) {
    for _ in 0..500 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out: {what}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_user_report_is_saved_then_delivered_budgeted_and_kept_to_keep() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("reports");
    aio::report::init(aio::report::About { id: "notes".into(), dir: dir.clone(), keep: 3, ..Default::default() });
    let notes = aio::Source::<Notes>::open(&d.path().join("state.db")).unwrap();
    notes.set("the draft".into()).unwrap();
    let sent = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let next = Arc::new(Mutex::new(Sink::Take));
    let (s, n) = (sent.clone(), next.clone());
    // A hung sink is let go at the end.
    let (release, hung) = std::sync::mpsc::channel::<()>();
    let hung = Mutex::new(hung);
    aio::report::sink(Duration::from_millis(300), move |r| {
        s.lock().unwrap().push(r.clone());
        let what = *n.lock().unwrap();
        match what {
            Sink::Take => Ok(()),
            Sink::Refuse => Err("collector down".into()),
            Sink::Hang => {
                _ = hung.lock().unwrap().recv();
                Ok(())
            }
        }
    });
    let titles = || sent.lock().unwrap().iter().map(|r| r["title"].as_str().unwrap_or_default().to_string()).collect();
    let h = aio::feedback::open().unwrap();
    assert!(aio::feedback::cell().is_some());
    assert_eq!((h.state().enabled, h.state().pending), (true, 0));
    assert!(h.report("  ".into(), None, None).unwrap_err().why.contains("one-line description"));

    let send = async |title: &str| {
        h.report(title.into(), Some("details".into()), Some("ana@example.org".into())).unwrap();
        until("saved", || h.state().status != Status::Capturing).await;
    };
    send("Slow start").await;
    let st = h.state();
    assert_eq!(st.status, Status::Saved);
    let last = st.last.clone().unwrap();
    assert!(last.delivered && std::path::Path::new(&last.path).exists(), "{last:?}");
    assert_eq!((st.pending, titles()), (1, vec!["Slow start".to_string()]));
    let on_disk: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&last.path).unwrap()).unwrap();
    assert_eq!(sent.lock().unwrap()[0], on_disk, "the sink got the report as written");
    assert_eq!(
        (on_disk["id"].as_str(), on_disk["createdAt"].as_str()),
        (Some(last.id.as_str()), Some(&*last.created_at))
    );
    assert_eq!(on_disk["state"]["notes"]["text"], "the draft", "the facts are collected");
    // A collector that is down, or never answers within the deadline: saved
    // all the same, said not delivered.
    for what in [Sink::Refuse, Sink::Hang] {
        *next.lock().unwrap() = what;
        send("Lost a note").await;
        assert_eq!(h.state().status, Status::Saved);
        assert!(!h.state().last.as_ref().unwrap().delivered);
    }
    *next.lock().unwrap() = Sink::Take;
    // Kept to `keep`, newest first.
    for i in 0..2 {
        send(&format!("again {i}")).await;
    }
    assert_eq!((h.state().pending, aio::report::list(&dir).len()), (3, 3));
    // Five a minute: a person never meets it, a script does.
    let e = h.report("sixth".into(), None, None).unwrap_err();
    assert!(e.why.contains(&format!("too many reports: this app takes {MAX} a minute")), "{e}");
    h.dismiss().unwrap();
    assert_eq!((h.state().status, h.state().last.clone()), (Status::Idle, None));
    let (newest, _) = aio::report::list(&dir).remove(0);
    std::fs::remove_file(dir.join(format!("{newest}.json"))).unwrap();
    h.refresh().unwrap();
    until("counted", || h.state().pending == 2).await;

    // The app's own reports go to the sink too, from a thread of their own.
    aio::report::problem("window:crashed", "gone");
    until("delivered", || titles().iter().any(|t| t == "window:crashed: gone")).await;

    // `[feedback] auto = false`: only the user's reports are written.
    aio::report::init(aio::report::About { id: "notes".into(), dir: dir.clone(), auto: false, ..Default::default() });
    assert_eq!(aio::report::problem("window:crashed", "gone again"), None);
    assert!(aio::report::file("by hand", "").is_ok());
    drop(release);
}

//! Its own test binary: the bus is global.
use std::sync::{Arc, Mutex};

#[aio::cell]
pub struct Jobs {
    pub done: u32,
}

#[aio::methods]
impl Jobs {
    pub fn finish(&mut self, cx: &aio::Cx) -> aio::Result {
        self.done += 1;
        cx.tell(aio::tell::Tell::notify("Export ready", "your file"))
    }
}

/// aio's warning: a notification in a signed-in user's call reaches every user.
#[test]
fn a_notification_in_a_users_call_is_said_to_reach_every_user_once() {
    let heard = Arc::new(Mutex::new(vec![]));
    let h = heard.clone();
    let _sub = aio::report::subscribe(move |d| {
        if d.kind == "tell:every-user" {
            h.lock().unwrap().push(d.message.clone());
        }
    });
    let jobs = aio::Handle::<Jobs>::memory();
    jobs.finish().unwrap();
    assert!(heard.lock().unwrap().is_empty(), "the app's own call is no one's news");
    let ana = jobs.by(aio::By::User("ana".into()));
    ana.finish().unwrap();
    ana.finish().unwrap();
    let said = heard.lock().unwrap().clone();
    assert_eq!(said.len(), 1, "once: {said:?}");
    assert!(said[0].contains("shown to every connected user, not only ana"), "{said:?}");
}

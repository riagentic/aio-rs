//! `aio::visible_by_default` (aio's `cellDefaults.visible`): one filter for
//! every cell without its own `visible`. A process has one, so this file
//! holds one test.
use aio::prelude::*;
use serde_json::{Value, json};

#[cell(shared)]
pub struct Notes {
    pub title: String,
    pub draft: String,
    pub tags: Vec<String>,
}

#[methods]
impl Notes {
    pub fn write(&mut self, title: String, draft: String) {
        self.title = title;
        self.draft = draft;
        self.tags = vec!["work".into()];
    }
}

#[cell(shared, visible = own)]
pub struct Inbox {
    pub draft: String,
}

fn own(s: &Inbox, _: &By) -> Inbox {
    s.clone()
}

#[methods]
impl Inbox {
    pub fn write(&mut self, draft: String) {
        self.draft = draft;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn every_cell_without_its_own_visible_is_filtered_by_the_apps_default() {
    aio::visible_by_default(|cell, mut view, who| {
        assert_ne!(cell, "inbox", "a cell with its own visible keeps it");
        if who.user() != Some("ana") {
            view.as_object_mut().unwrap().remove("draft");
            view["tags"] = json!([]);
        }
        view
    });
    let notes = Handle::<Notes>::memory();
    notes.write("Plan".into(), "secret".into()).unwrap();
    let inbox = Handle::<Inbox>::memory();
    inbox.write("mine".into()).unwrap();
    let http = aio::http::Http::default().cell(inbox);
    let s = aio::testing::TestServer::start(notes, http).await.unwrap();
    let (ana, bo) = (s.client("ana").await.unwrap(), s.client("bo").await.unwrap());
    assert_eq!((ana.state().title.as_str(), ana.state().draft.as_str()), ("Plan", "secret"));
    // Left out: the copy still reads, the field at its default.
    assert_eq!((bo.state().title.as_str(), bo.state().draft.as_str()), ("Plan", ""));
    assert!(bo.state().tags.is_empty());
    let bos_inbox = s.client_of::<Inbox>("bo").await.unwrap();
    assert_eq!(bos_inbox.state().draft, "mine", "its own visible, not the app's");
    // A failing filter shows nothing: the defaults.
    aio::visible_by_default(|_, _: Value, _| panic!("broken"));
    let cy = s.client("cy").await.unwrap();
    assert_eq!((cy.state().title.as_str(), cy.state().draft.as_str()), ("", ""));
}

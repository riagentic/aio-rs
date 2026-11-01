// Its own test binary: the reporter, its cells and the panic hook are global.
use aio::prelude::*;

#[cell(persist)]
pub struct Notes {
    pub items: Vec<String>,
    #[transient]
    pub draft: String,
    /// Kept and saved, never sent: never in a report either.
    #[server]
    pub sealed: String,
}

#[methods]
impl Notes {
    pub fn add(&mut self, text: String) -> Result {
        ensure!(!text.is_empty(), "empty note");
        self.items.push(text);
        Ok(())
    }

    pub fn type_draft(&mut self, text: String) {
        self.sealed = format!("sealed-{text}");
        self.draft = text;
    }

    pub fn crash(&mut self) {
        panic!("crashed mid-call");
    }
}

#[test]
fn a_report_shows_the_cells_and_the_calls_that_led_to_it() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("reports");
    aio::report::init(aio::report::About { id: "notes".into(), dir: dir.clone(), ..Default::default() });
    aio::report::on_panic();
    let s = aio::Source::<Notes>::open(&d.path().join("notes.db")).unwrap();
    s.add("milk".into()).unwrap();
    s.add(String::new()).unwrap_err();
    s.type_draft("secret draft".into()).unwrap();
    // A panic mid-call: the report is written without waiting on the store.
    assert!(s.crash().unwrap_err().why.contains("crashed mid-call"));
    let p = aio::report::file_from("Lost a note", "it vanished", Some("  ana@example.org ")).unwrap();
    let all = aio::report::list(&dir);
    let crash = &all.iter().find(|(_, v)| v["kind"] == "panic").unwrap().1;
    assert_eq!(crash["state"]["notes"], serde_json::json!({ "items": ["milk"] }), "saved shape: no transient draft");
    let user: serde_json::Value = serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap();
    let calls: Vec<_> = user["timeline"].as_array().unwrap().iter().map(|e| e["call"].as_str().unwrap()).collect();
    assert_eq!(calls, ["notes.add", "notes.add", "notes.type_draft", "notes.crash"]);
    assert_eq!(user["timeline"][1]["refused"], "empty note");
    assert!(!user.to_string().contains("secret draft"), "never a call's arguments, a transient or a #[server] field");
    assert_eq!(s.state().sealed, "sealed-secret draft");
    assert_eq!(user["contact"], "ana@example.org");
    assert!(user["diagnostics"].as_array().unwrap().iter().any(|d| d["kind"] == "panic"), "{}", user["diagnostics"]);
}

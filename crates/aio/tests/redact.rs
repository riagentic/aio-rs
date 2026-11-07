// Its own test binary: the reporter and its cells are global.
use aio::prelude::*;

#[cell(persist)]
pub struct Vault {
    pub locked: bool,
    pub tries: u32,
    pub notes: Vec<String>,
}

#[methods]
impl Vault {
    #[redact]
    pub fn unlock(&mut self, password: String) -> Result {
        self.tries += 1;
        ensure!(password == "hunter2", "wrong password");
        self.locked = false;
        Ok(())
    }

    /// `password` lives across the await: its step's arguments carry it.
    #[redact]
    pub async fn rekey(&mut self, password: String) -> Result {
        let n: usize = measure(password.clone()).await;
        ensure!(!password.is_empty(), "empty password");
        self.tries = n as u32;
        Ok(())
    }

    pub fn note(&mut self, text: String) {
        self.notes.push(text);
    }
}

/// aio's `redactActions: ["keys:*"]`: every method of the cell.
#[cell(persist, redact)]
pub struct Keys {
    pub count: u32,
}

#[methods]
impl Keys {
    pub fn add(&mut self, seed: String) {
        self.count += u32::from(!seed.is_empty());
    }
}

#[test]
fn a_redacted_cell_keeps_no_arguments_of_any_method() {
    assert!(Keys::redacts("add") && Keys::redacts("anything"));
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("keys.db");
    {
        let mut s = aio::Store::<Keys>::open(&path).unwrap();
        s.call(&By::System, "add", serde_json::json!(["correct horse battery"])).unwrap();
        assert_eq!(s.events().unwrap()[0].args, serde_json::json!({ aio::REDACTED: { "count": 1 } }));
    }
    assert!(!every_file_holds(d.path(), "correct horse"), "not in the database, its journal or its WAL");
    assert_eq!(aio::Store::<Keys>::open(&path).unwrap().state().count, 1, "it still replays");
}

async fn measure(p: String) -> usize {
    p.len()
}

fn every_file_holds(dir: &std::path::Path, needle: &str) -> bool {
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .any(|e| std::fs::read(e.path()).is_ok_and(|b| b.windows(needle.len()).any(|w| w == needle.as_bytes())))
}

#[test]
fn a_redacted_calls_arguments_never_reach_the_log_and_it_still_replays() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("vault.db");
    {
        let mut s = aio::Store::<Vault>::open(&path).unwrap();
        s.call(&By::System, "note", serde_json::json!(["shopping"])).unwrap();
        s.call(&By::System, "unlock", serde_json::json!(["hunter2"])).unwrap();
        s.call(&By::System, "note", serde_json::json!(["after"])).unwrap();
        let ev = s.events().unwrap();
        assert_eq!(
            ev.iter().map(|e| e.method.as_str()).collect::<Vec<_>>(),
            ["note", "unlock", "note"],
            "the call is kept"
        );
        assert_eq!(
            ev[1].args,
            serde_json::json!({ aio::REDACTED: { "locked": false, "tries": 1, "notes": ["shopping"] } })
        );
        assert_eq!(ev[0].args, serde_json::json!(["shopping"]), "other calls keep theirs");
        // Time travel goes through it.
        assert!(!s.state_at(2).unwrap().locked);
        assert_eq!(s.state_at(1).unwrap().tries, 0);
    }
    assert!(!every_file_holds(d.path(), "hunter2"), "not in the database, its journal or its WAL");
    // A boot replays the events after the newest snapshot: through it too.
    let s = aio::Store::<Vault>::open(&path).unwrap();
    assert_eq!((s.state().locked, s.state().tries, s.state().notes.len()), (false, 1, 2));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_redacted_async_methods_steps_are_redacted_too() {
    assert!(Vault::redacts("rekey") && Vault::redacts("rekey.1") && Vault::redacts("unlock"));
    assert!(!Vault::redacts("note") && !Vault::redacts("rekeyed") && !Vault::redacts("unlocks.1"));
    let d = tempfile::tempdir().unwrap();
    let h = Handle::<Vault>::open(d.path().join("vault.db")).unwrap();
    h.rekey("correct horse".into()).unwrap();
    for _ in 0..300 {
        if h.state().tries == 13 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let ev = h.store(|s| s.events().unwrap());
    assert_eq!(ev.iter().map(|e| e.method.as_str()).collect::<Vec<_>>(), ["rekey", "rekey.1"]);
    assert!(ev.iter().all(|e| e.args.get(aio::REDACTED).is_some()), "{ev:?}");
    drop(h);
    assert!(!every_file_holds(d.path(), "correct horse"));
}

#[test]
fn a_cell_with_redacted_methods_is_withheld_from_reports() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("reports");
    aio::report::init(aio::report::About { id: "vault".into(), dir: dir.clone(), ..Default::default() });
    let s = aio::Source::<Vault>::open(&d.path().join("vault.db")).unwrap();
    s.note("visible?".into()).unwrap();
    let r: serde_json::Value =
        serde_json::from_slice(&std::fs::read(aio::report::file("t", "b").unwrap()).unwrap()).unwrap();
    assert_eq!(r["state"].get("vault"), None, "{}", r["state"]);
    assert_eq!(r["withheld"], serde_json::json!(["vault: it has #[redact] methods"]));
    assert!(!r.to_string().contains("visible?"));
}

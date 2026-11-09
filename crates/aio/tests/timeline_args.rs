// Its own test binary: the timeline and the reporter are the process's.
use aio::prelude::*;

#[cell(persist)]
pub struct Vault {
    pub notes: Vec<String>,
}

#[methods]
impl Vault {
    pub fn note(&mut self, text: String) {
        self.notes.push(text);
    }

    #[redact]
    pub fn unlock(&mut self, password: String) -> Result {
        ensure!(password == "hunter2", "wrong password");
        Ok(())
    }
}

/// aio's timeline payloads, opted into (`[feedback] args`): each call's
/// arguments, a `#[redact]` method's never, keys masked, cut at 1 KB.
#[test]
fn with_feedback_args_the_timeline_carries_each_calls_arguments_but_redacted_ones() {
    aio::report::init(aio::report::About { id: "vault".into(), args: true, ..Default::default() });
    let d = tempfile::tempdir().unwrap();
    let v = aio::Source::<Vault>::open(&d.path().join("vault.db")).unwrap();
    v.note("milk".into()).unwrap();
    v.unlock("hunter2".into()).unwrap();
    v.note("see token=abc123".into()).unwrap();
    v.note("x".repeat(5000)).unwrap();
    let t = aio::report::timeline();
    assert_eq!(t[0]["args"], r#"["milk"]"#);
    assert_eq!(t[1]["args"], r#""[redacted]""#, "a #[redact] method's never");
    assert!(!t.iter().any(|e| e.to_string().contains("hunter2")));
    assert_eq!(t[2]["args"], r#"["see token=…"]"#, "a key masked");
    assert_eq!(t[3]["args"].as_str().unwrap().chars().count(), 1024, "cut");
}

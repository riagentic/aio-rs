//! aio's `onError` in aio-rs: a failing call, effect or hook puts its
//! cell's state on the diagnostic bus (aio's `stateSnapshot`), screened as
//! a problem report's is, with aio's error code; and an async call's
//! caller hears aio's `EFFECT_TIMEOUT` / `EFFECT_ERROR`. Its own test
//! binary: the bus and the panic hook are global.
use aio::prelude::*;
use aio::report::Diagnostic;
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cell(persist)]
pub struct Vault {
    note: String,
    #[server]
    secret: String,
}

#[methods]
impl Vault {
    pub fn set(&mut self, note: String) {
        (self.note, self.secret) = (note, "hidden".into());
    }

    pub fn boom(&mut self) {
        panic!("vault bug");
    }

    pub async fn hang(&mut self) -> Result {
        let n: u32 = std::future::pending().await;
        self.note = format!("{n}");
        Ok(())
    }

    pub async fn crash(&mut self) -> Result {
        let n: u32 = async { panic!("io bug") }.await;
        self.note = format!("{n}");
        Ok(())
    }

    #[on_stop]
    pub fn close(&mut self) -> Result {
        reject!("still writing")
    }
}

/// A cell with a `#[redact]` method: its state never goes out.
#[cell(persist)]
pub struct Keys {
    pin: String,
}

#[methods]
impl Keys {
    #[redact]
    pub fn boom(&mut self, pin: String) {
        self.pin = pin;
        panic!("keys bug");
    }
}

fn heard() -> (Arc<Mutex<Vec<Diagnostic>>>, aio::report::Subscription) {
    let all = Arc::new(Mutex::new(vec![]));
    let a = all.clone();
    (all, aio::report::subscribe(move |d| a.lock().unwrap().push(d.clone())))
}

fn last(all: &Mutex<Vec<Diagnostic>>, kind: &str) -> Diagnostic {
    let all = all.lock().unwrap();
    all.iter().rev().find(|d| d.kind == kind).cloned().unwrap_or_else(|| panic!("no {kind} in {all:?}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failing_call_effect_or_hook_carries_its_screened_state_and_aios_code() {
    aio::report::on_panic();
    let d = tempfile::tempdir().unwrap();
    let (all, _sub) = heard();
    let aio::Source::Local(h) = aio::Source::<Vault>::open(&d.path().join("vault.db")).unwrap() else { unreachable!() };
    h.set("see token=abc123".into()).unwrap();

    // A method's panic: REDUCE_ERROR, the state as a client sees it, masked.
    assert!(h.boom().is_err());
    let p = last(&all, "panic");
    assert_eq!((p.cell.as_deref(), p.error_code()), (Some("vault"), Some(aio::Code::ReduceError)));
    assert_eq!(p.state, Some(json!({ "note": "see token=…" })), "no #[server] field, the token masked");

    // An async call past its ceiling: its caller hears EFFECT_TIMEOUT, the bus too.
    aio::perf::set(aio::perf::Budgets { call_ceiling: Some(50), ..Default::default() });
    let e = h.hang().unwrap().await.unwrap_err();
    aio::perf::set(aio::perf::Budgets::default());
    assert_eq!(e.code(), Some(aio::Code::EffectTimeout), "{e}");
    assert!(e.why.contains("vault: effect `hang` timed out after"), "{e}");
    let t = last(&all, "effect:timeout");
    assert_eq!((t.cell.as_deref(), t.error_code()), (Some("vault"), Some(aio::Code::EffectTimeout)));
    assert_eq!(t.state, Some(json!({ "note": "see token=…" })));

    // An async call whose effect panics: EFFECT_ERROR, not "aborted".
    let e = h.crash().unwrap().await.unwrap_err();
    assert_eq!(e.code(), Some(aio::Code::EffectError), "{e}");
    assert!(e.why.contains("crash: its effect panicked before it answered"), "{e}");

    // A reaction that fails: HOOK_ERROR (aio's onAction), with the reacting cell's state.
    let aio::Source::Local(k) = aio::Source::<Keys>::open(&d.path().join("keys.db")).unwrap() else { unreachable!() };
    k.listens_to(&h, "set", |_, _| Err::<(), _>(aio::Rejected::new("not this one")));
    h.set("again".into()).unwrap();
    let r = last(&all, "reaction");
    assert_eq!((r.cell.as_deref(), r.error_code(), r.state), (Some("keys"), Some(aio::Code::HookError), None));

    // `#[on_stop]` refused: DESTROY_ERROR on the bus.
    h.shutdown(Duration::from_secs(1)).await;
    let s = last(&all, "hook:stop");
    assert_eq!((s.cell.as_deref(), s.error_code()), (Some("vault"), Some(aio::Code::DestroyError)));
    assert_eq!(s.state, Some(json!({ "note": "again" })));

    // A cell with #[redact] methods: its state is withheld, as from a report.
    tokio::time::sleep(Duration::from_millis(QUIET)).await;
    assert!(k.boom("1234".into()).is_err());
    let p = last(&all, "panic");
    assert_eq!((p.cell.as_deref(), p.state), (Some("keys"), None));
}

/// The bus says a kind at most once per 5 s: wait it out between panics.
const QUIET: u64 = aio::report::QUIET_MS as u64 + 100;

#[test]
fn aios_effect_and_hook_codes_keep_their_wire_names() {
    for (c, n) in [
        (aio::Code::EffectError, "EFFECT_ERROR"),
        (aio::Code::EffectTimeout, "EFFECT_TIMEOUT"),
        (aio::Code::HookError, "HOOK_ERROR"),
        (aio::Code::DestroyError, "DESTROY_ERROR"),
    ] {
        assert_eq!(c.as_str(), n);
        assert_eq!(aio::Code::of(&json!(n)), Some(c));
    }
}

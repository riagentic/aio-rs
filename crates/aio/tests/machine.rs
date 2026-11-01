use aio::prelude::*;

aio::machine! {
    pub enum Phase {
        Draft { submit => Review },
        Review { approve => Approved, reject => Draft },
        Approved { ship => Shipped },
        Shipped {},
    }
}

#[cell(persist, machine = phase)]
pub struct Order {
    pub phase: Phase,
    pub title: String,
    pub log: Vec<String>,
}

#[methods]
impl Order {
    pub fn submit(&mut self) {
        self.log.push(format!("submitted, now {:?}", self.phase));
    }
    pub fn approve(&mut self) {}
    pub fn reject(&mut self, why: String) -> Result {
        ensure!(!why.is_empty(), "say why");
        Ok(())
    }
    pub fn ship(&mut self) {}
    /// Never named by the machine: runs in any state.
    pub fn rename(&mut self, title: String) {
        self.title = title;
    }
}

aio::machine! {
    pub enum Loose {
        A { go => B },
        B {},
        C { go => A },
    }
}

#[cell(machine = at)]
pub struct Bad {
    pub at: Loose,
}

#[methods]
impl Bad {
    pub fn go(&mut self) {}
}

#[test]
fn a_machine_guards_every_call_and_replays() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.db");
    let h = Handle::<Order>::open(&p).unwrap();
    assert_eq!(h.state().phase, Phase::Draft);
    let e = h.ship().unwrap_err();
    assert_eq!(e.why, "`ship` can't run while Draft: only from Approved");
    h.submit().unwrap();
    assert_eq!((h.state().phase, h.state().log[0].as_str()), (Phase::Review, "submitted, now Review"));
    assert!(h.reject(String::new()).is_err());
    assert_eq!(h.state().phase, Phase::Review, "a refused call moves nothing");
    h.reject("typo".into()).unwrap();
    h.rename("x".into()).unwrap();
    h.submit().unwrap();
    h.approve().unwrap();
    h.ship().unwrap();
    assert!(h.submit().is_err() && h.state().phase == Phase::Shipped);
    let want = h.state();
    std::mem::forget(h);
    let h = Handle::<Order>::open(&p).unwrap();
    assert_eq!(*h.state(), *want, "replayed after a crash");
    assert_eq!(h.store(|s| s.state_at(2).unwrap().phase), Phase::Draft, "time travel");
}

#[test]
fn a_wrong_machine_opens_no_store() {
    let d = tempfile::tempdir().unwrap();
    let e = Handle::<Bad>::open(d.path().join("state.db")).err().unwrap();
    assert_eq!(e.0, "bad: its machine is wrong: C is unreachable from A");
    let Err(panicked) = std::panic::catch_unwind(Handle::<Bad>::memory) else { panic!("a wrong machine ran") };
    assert!(panicked.downcast_ref::<String>().unwrap().contains("C is unreachable from A"));
}

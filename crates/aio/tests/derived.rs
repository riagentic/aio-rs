//! `#[derived]` values (aio selectors): computed from the state, cached until
//! it (or another cell they read) changes, parameterized.
use aio::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

static LIMITS: AtomicUsize = AtomicUsize::new(0);
static FINDS: AtomicUsize = AtomicUsize::new(0);
static SUMMARIES: AtomicUsize = AtomicUsize::new(0);

#[cell]
pub struct Edition {
    tier: u32,
    names: Vec<String>,
}

#[methods]
impl Edition {
    pub fn set_tier(&mut self, t: u32) -> Result {
        ensure!(t > 0, "no tier 0");
        self.tier = t;
        Ok(())
    }

    pub fn name(&mut self, n: String) {
        self.names.push(n);
    }

    #[derived]
    pub fn limit(&self) -> u32 {
        LIMITS.fetch_add(1, SeqCst);
        10 * self.tier
    }

    #[derived]
    fn find(&self, prefix: &str) -> Option<String> {
        FINDS.fetch_add(1, SeqCst);
        self.names.iter().find(|n| n.starts_with(prefix)).cloned()
    }

    /// Reads another cell (aio's `deps` form): typed, so an unknown dep can't compile.
    #[derived]
    pub fn summary(&self, purse: &Arc<Purse>, sep: char) -> String {
        SUMMARIES.fetch_add(1, SeqCst);
        format!("tier {}{sep}balance {}", self.tier, purse.balance)
    }
}

#[cell]
pub struct Purse {
    balance: i64,
}

#[methods]
impl Purse {
    pub fn pay(&mut self, n: i64) {
        self.balance += n;
    }
}

fn edition() -> Handle<Edition> {
    let h = Handle::<Edition>::memory();
    h.set_tier(2).unwrap();
    h
}

#[test]
fn a_derived_value_is_computed_once_per_state() {
    let h = edition();
    assert_eq!(h.state().limit(), 20);
    assert_eq!((h.state().limit(), LIMITS.load(SeqCst)), (20, 1), "cached");
    h.set_tier(5).unwrap();
    assert_eq!((h.state().limit(), LIMITS.load(SeqCst)), (50, 2), "recomputed");
    h.name("x".into()).unwrap();
    assert_eq!((h.state().limit(), LIMITS.load(SeqCst)), (50, 3), "any commit is a new state");
    assert!(h.set_tier(0).is_err());
    assert_eq!((h.state().limit(), LIMITS.load(SeqCst)), (50, 3), "a refused call changes nothing");
    assert_eq!(Edition::limit(&h.state()), 50, "on &Edition: the plain fn");
    assert_eq!(LIMITS.load(SeqCst), 4);
}

#[test]
fn a_watch_is_told_only_when_its_value_changes() {
    let h = edition();
    let seen = Arc::new(std::sync::Mutex::new(vec![]));
    let s = seen.clone();
    // Not `limit`: the other tests count its runs.
    h.watch(|e| e.tier * 10, move |l| s.lock().unwrap().push(*l));
    h.name("x".into()).unwrap();
    h.set_tier(3).unwrap();
    assert!(h.set_tier(0).is_err());
    h.set_tier(3).unwrap();
    assert_eq!(*seen.lock().unwrap(), [20, 30], "now, then only the change");
}

/// What the reactions did.
fn log() -> (Arc<std::sync::Mutex<Vec<String>>>, impl Fn(String) + Send + Sync + Clone + 'static) {
    let seen = Arc::new(std::sync::Mutex::new(vec![]));
    let s = seen.clone();
    (seen, move |l| s.lock().unwrap().push(l))
}

/// Dropped: says so (a camera the reaction opened).
struct Opened(String, Arc<dyn Fn(String) + Send + Sync>);

impl Drop for Opened {
    fn drop(&mut self) {
        (self.1)(format!("close {}", self.0))
    }
}

#[test]
fn on_change_waits_for_a_change_gives_the_old_value_and_stops_when_dropped() {
    // aio tests/use-resource.test.ts `onChange`.
    let h = edition();
    let (seen, say) = log();
    let w = h.on_change(false, |e| e.tier, move |t, old| say(format!("{old:?} -> {t}")));
    assert!(seen.lock().unwrap().is_empty(), "nothing yet: it has not changed");
    h.set_tier(3).unwrap();
    h.set_tier(3).unwrap();
    h.name("x".into()).unwrap();
    h.set_tier(4).unwrap();
    assert_eq!(*seen.lock().unwrap(), ["Some(2) -> 3", "Some(3) -> 4"], "the same value again is no change");
    drop(w);
    h.set_tier(5).unwrap();
    assert_eq!(seen.lock().unwrap().len(), 2, "a dropped reaction is stopped");
}

#[test]
fn what_a_reaction_made_is_dropped_before_the_next_and_with_it() {
    let h = edition();
    let (seen, say) = log();
    let say: Arc<dyn Fn(String) + Send + Sync> = Arc::new(say);
    let w = h.on_change(
        true,
        |e| e.tier,
        move |t, _| {
            say(format!("open {t}"));
            Opened(t.to_string(), say.clone())
        },
    );
    h.set_tier(3).unwrap();
    assert_eq!(*seen.lock().unwrap(), ["open 2", "close 2", "open 3"], "immediate, and close before open");
    drop(w);
    assert_eq!(seen.lock().unwrap().last().unwrap(), "close 3");
}

#[test]
fn a_reaction_that_writes_the_cell_does_not_loop() {
    let h = edition();
    let (seen, say) = log();
    let me = h.clone();
    let _w = h.on_change(
        false,
        |e| e.tier,
        move |t, _| {
            say(format!("tier {t}"));
            me.name("again".into()).unwrap();
        },
    );
    h.set_tier(3).unwrap();
    h.name("other".into()).unwrap();
    assert_eq!(*seen.lock().unwrap(), ["tier 3"], "only the selector wakes it, and its own write doesn't");
    assert_eq!(h.state().names.len(), 2);
}

#[test]
fn parameterized_values_are_cached_per_arguments() {
    let h = edition();
    h.name("ana".into()).unwrap();
    h.name("bo".into()).unwrap();
    let st = h.state();
    assert_eq!(st.find("b"), Some("bo".into()));
    assert_eq!((st.find("b"), FINDS.load(SeqCst)), (Some("bo".into()), 1));
    assert_eq!((st.find("a"), FINDS.load(SeqCst)), (Some("ana".into()), 2));
    assert_eq!(st.find("z"), None);
}

#[test]
fn a_value_over_another_cell_recomputes_when_either_changes() {
    let (e, w) = (edition(), Handle::<Purse>::memory());
    w.pay(100).unwrap();
    assert_eq!(e.state().summary(&w.state(), ','), "tier 2,balance 100");
    assert_eq!(e.state().summary(&w.state(), ','), "tier 2,balance 100");
    assert_eq!(SUMMARIES.load(SeqCst), 1);
    w.pay(-30).unwrap();
    assert_eq!(e.state().summary(&w.state(), ','), "tier 2,balance 70");
    e.set_tier(3).unwrap();
    assert_eq!(e.state().summary(&w.state(), ';'), "tier 3;balance 70");
    assert_eq!(SUMMARIES.load(SeqCst), 3);
}

static SORTS: AtomicUsize = AtomicUsize::new(0);

/// A list and its cursor: the sorted rows follow the rows, not the cursor.
#[cell]
pub struct Picker {
    rows: Vec<String>,
    at: u32,
}

#[methods]
impl Picker {
    pub fn add(&mut self, r: String) {
        self.rows.push(r);
    }

    pub fn down(&mut self) {
        self.at += 1;
    }

    /// aio's `memoLast(() => [rows], …)`.
    #[derived(reads = [rows])]
    pub fn sorted(&self) -> Arc<Vec<String>> {
        SORTS.fetch_add(1, SeqCst);
        let mut v = self.rows.clone();
        v.sort();
        Arc::new(v)
    }
}

#[test]
fn a_derived_value_that_reads_some_fields_holds_while_they_do() {
    let h = Handle::<Picker>::memory();
    h.add("b".into()).unwrap();
    h.add("a".into()).unwrap();
    let first = h.state().sorted();
    let n = SORTS.load(SeqCst);
    for _ in 0..10 {
        h.down().unwrap();
        assert!(Arc::ptr_eq(&h.state().sorted(), &first), "a cursor step: the same rows, the same Arc");
    }
    assert_eq!(SORTS.load(SeqCst), n, "not sorted again");
    h.add("c".into()).unwrap();
    assert_eq!(*h.state().sorted(), ["a", "b", "c"]);
    assert_eq!(SORTS.load(SeqCst), n + 1);
}

#[test]
fn a_cell_lays_out_its_fields_and_says_which_changed() {
    use aio::CellData;
    let names: Vec<&str> = Picker::FIELDS.iter().map(|f| f.name).collect();
    assert_eq!(names, ["rows", "at"]);
    let p = Picker::default();
    let base = &p as *const Picker as usize;
    let at = Picker::FIELDS[1];
    assert_eq!(&p.at as *const u32 as usize - base, at.at);
    assert_eq!(aio::Field::holding(Picker::FIELDS, at.at + 1), Some(1));
    let moved = Picker { at: 3, ..Picker::default() };
    assert_eq!(moved.changed_fields(&p), 0b10);
    assert_eq!(p.changed_fields(&p.clone()), 0);
}

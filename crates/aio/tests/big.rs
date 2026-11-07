//! `aio::Big`: a held key over a 1000-row list compares no row (aio's Immer
//! drafts: what a call didn't touch is the same object). The rows count
//! their comparisons: none after a cursor step, in the store (the held run,
//! its flush) or the views (which field changed).
use aio::act;
use aio::kit::{GlobalKeys, use_global_key};
use aio::testing::TestUi;
use aio::ui::prelude::*;
use aio::{Big, CellData as _};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

static COMPARED: AtomicUsize = AtomicUsize::new(0);

#[aio::data]
#[derive(Clone, Debug, Default)]
pub struct Row {
    pub name: String,
}

impl PartialEq for Row {
    fn eq(&self, other: &Self) -> bool {
        COMPARED.fetch_add(1, Ordering::Relaxed);
        self.name == other.name
    }
}

#[aio::cell(persist, id = "big")]
pub struct List {
    pub rows: Big<Vec<Row>>,
    pub cursor: usize,
}

#[aio::methods]
impl List {
    pub fn fill(&mut self, n: usize) {
        *self.rows = (0..n).map(|i| Row { name: format!("Account {i}") }).collect();
    }

    pub fn rename(&mut self, i: usize, name: String) {
        self.rows[i].name = name;
    }

    #[coalesce]
    pub fn down(&mut self) {
        self.cursor = (self.cursor + 1).min(self.rows.len().saturating_sub(1));
    }
}

aio::client!(List, list);

#[component]
fn Lines() -> View {
    use_global_key("ArrowDown", |_| _ = act!(down()));
    let (rows, cursor) = (part(|l| &l.rows), part(|l| &l.cursor));
    rsx! {
        ul { "data-t": "list",
            for i in cursor.get()..(cursor.get() + 3).min(rows.with(|r| r.len())) {
                li { key: "{i}", {rows.with(|r| r[i].name.clone())} }
            }
        }
    }
}

fn app() -> View {
    use_live();
    rsx! { GlobalKeys { Lines {} } }
}

#[test]
fn a_step_that_leaves_a_big_field_compares_none_of_it() {
    let dir = tempfile::tempdir().unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let h = aio::Handle::<List>::open(dir.path().join("big.db")).unwrap();
    h.fill(1000).unwrap();
    let _started = {
        let _in = rt.enter();
        h.start()
    };
    assert!(SOURCE.set(aio::Source::Local(h.clone())).is_ok());
    let mut ui = TestUi::new(app);
    let before = h.state();
    COMPARED.store(0, Ordering::Relaxed);
    for _ in 0..30 {
        ui.press_key(Key::ArrowDown, Modifiers::empty());
    }
    // The held run goes into the log (its flush compares the run's states).
    let end = Instant::now() + Duration::from_secs(10);
    while h.store(|s| s.holds()) {
        assert!(Instant::now() < end, "the held run was never saved");
        std::thread::sleep(Duration::from_millis(10));
    }
    let after = h.state();
    assert_eq!(after.cursor, 30);
    assert!(ui.text("list").starts_with("Account 30"), "{}", ui.text("list"));
    assert!(Big::ptr_eq(&before.rows, &after.rows), "the rows are shared, never copied");
    assert_eq!(COMPARED.load(Ordering::Relaxed), 0, "no row compared");
    assert_eq!(after.changed_fields(&before), 0b10, "the cursor alone changed");

    // A write copies the rows once (the state before still shares the old ones).
    h.rename(5, "Renamed".into()).unwrap();
    let renamed = h.state();
    assert!(!Big::ptr_eq(&after.rows, &renamed.rows) && after.rows[5].name == "Account 5");
    assert_eq!(renamed.changed_fields(&after) & 1, 1, "the rows changed");
    // Saved and sent as the value it holds.
    let json = aio::serde_json::to_value(&*renamed).unwrap();
    assert_eq!(json["rows"][5]["name"], "Renamed");
    assert_eq!(aio::serde_json::from_value::<List>(json).unwrap(), *renamed);
}

/// A value slow to compare (1 ms).
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Slow(u32);

impl PartialEq for Slow {
    fn eq(&self, other: &Self) -> bool {
        std::thread::sleep(Duration::from_millis(1));
        self.0 == other.0
    }
}

#[aio::cell(id = "slow")]
pub struct Ledger {
    pub rows: Slow,
    pub cursor: usize,
}

#[aio::methods]
impl Ledger {}

#[test]
fn a_dev_build_names_a_field_slow_to_compare_once() {
    let (a, b) = (Ledger::default(), Ledger { cursor: 1, ..Default::default() });
    let (mask, said) = aio::log::capture(|| (a.changed_fields(&b), a.changed_fields(&b)));
    assert_eq!(mask, (0b10, 0b10));
    let hints: Vec<_> = said.iter().filter(|l| l.contains("make it `aio::Big<…>`")).collect();
    assert_eq!(hints.len(), 1, "once: {said:?}");
    assert!(hints[0].contains("field `rows` of cell `slow`"), "{}", hints[0]);
    assert!(!said.iter().any(|l| l.contains("`cursor`")), "a fast field is not named");
}

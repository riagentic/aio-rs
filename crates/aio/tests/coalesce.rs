//! `#[coalesce]`: a cursor's steps are shown at once and logged as one event.
use aio::prelude::*;
use serde_json::json;

#[cell(persist)]
pub struct List {
    rows: Vec<String>,
    at: u32,
    /// Two panels' cursors, and the panel last moved in.
    cursors: [u32; 2],
    panel: u32,
}

#[methods]
impl List {
    #[coalesce]
    pub fn focus(&mut self, at: u32) {
        self.at = at.min(self.rows.len().saturating_sub(1) as u32);
    }

    #[coalesce]
    pub fn pick(&mut self, panel: u32, at: u32) {
        self.panel = panel.min(1);
        self.cursors[self.panel as usize] = at;
    }

    pub fn add(&mut self, row: String) {
        self.rows.push(row);
    }
}

fn filled(s: &mut Store<List>) {
    for i in 0..50 {
        s.call(&By::Test, "add", serde_json::json!([format!("row {i}")])).unwrap();
    }
}

#[test]
fn a_held_down_key_is_one_event_not_one_per_step() {
    let mut s = Store::<List>::memory();
    filled(&mut s);
    let before = s.events().unwrap().len();
    for i in 1..=40u32 {
        s.call(&By::Test, "focus", serde_json::json!([i])).unwrap();
        assert_eq!(s.state().at, i, "each step is shown at once");
    }
    assert_eq!(s.events().unwrap().len(), before, "no step was committed yet");
    s.call(&By::Test, "add", serde_json::json!(["late"])).unwrap();
    let ev = s.events().unwrap();
    assert_eq!(ev.len(), before + 2, "the run is one event, then the call after it");
    assert_eq!((ev[before].method.as_str(), &ev[before].args), ("focus", &serde_json::json!([40])));
    assert_eq!(s.state_at(ev[before].seq).unwrap().at, 40, "its replay lands where the run did");
}

#[test]
fn a_run_its_last_call_alone_cant_stand_for_is_logged_call_by_call() {
    let mut s = Store::<List>::memory();
    let n = s.events().unwrap().len();
    (1..=10u32).for_each(|i| _ = s.call(&By::Test, "pick", serde_json::json!([0, i])).unwrap());
    s.call(&By::Test, "pick", serde_json::json!([1, 3])).unwrap();
    s.flush().unwrap();
    let ev = s.events().unwrap();
    assert_eq!(ev.len(), n + 11, "a step in the other panel: the last call alone would lose the first's");
    assert_eq!(s.state_at(ev.last().unwrap().seq).unwrap().cursors, [10, 3]);
    (1..=10u32).for_each(|i| _ = s.call(&By::Test, "pick", serde_json::json!([0, 20 + i])).unwrap());
    s.flush().unwrap();
    assert_eq!(s.events().unwrap().len(), n + 12, "one panel's steps: one event");
    assert_eq!(s.state().cursors, [30, 3]);
}

#[test]
fn a_step_that_moves_nothing_holds_nothing() {
    let mut s = Store::<List>::memory();
    filled(&mut s);
    s.call(&By::Test, "focus", serde_json::json!([49])).unwrap();
    s.flush().unwrap();
    let n = s.events().unwrap().len();
    for _ in 0..30 {
        s.call(&By::Test, "focus", serde_json::json!([99])).unwrap();
    }
    assert!(!s.holds(), "Down on the last row changes nothing");
    s.call(&By::Test, "focus", serde_json::json!([3])).unwrap();
    s.call(&By::Test, "focus", serde_json::json!([49])).unwrap();
    s.flush().unwrap();
    assert_eq!(s.events().unwrap().len(), n, "a run back to where it began logs nothing");
}

#[test]
fn the_close_saves_a_held_run_and_undo_takes_it_back_whole() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    {
        let mut s = Store::<List>::open(&path).unwrap();
        filled(&mut s);
        (1..=20u32).for_each(|i| _ = s.call(&By::Test, "focus", serde_json::json!([i])).unwrap());
    }
    let mut s = Store::<List>::open(&path).unwrap();
    assert_eq!(s.state().at, 20, "the close flushed the run");
    assert_eq!(s.events().unwrap().len(), 51);
    (21..=30u32).for_each(|i| _ = s.call(&By::Test, "focus", serde_json::json!([i])).unwrap());
    s.travel(&By::Test, aio::Travel::Undo).unwrap();
    assert_eq!(s.state().at, 20, "one undo: the whole run, not one step");
}

#[test]
fn a_run_goes_into_the_log_by_itself() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let h = Handle::<List>::memory();
        h.store(filled);
        let _sched = h.start();
        let mut events = h.events();
        for i in 1..=40u32 {
            h.call("focus", serde_json::json!([i])).unwrap();
        }
        assert!(events.try_recv().is_err(), "nothing sent while the key is held");
        tokio::time::sleep(aio::PERSIST_DEBOUNCE * 3).await;
        let (_, ev) = events.try_recv().expect("the run, once idle");
        assert_eq!(ev.args, serde_json::json!([40]));
        assert!(events.try_recv().is_err(), "as one event");
    });
}

static SERIALIZED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A value that counts how often it is serialized.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
pub struct Counted(u32);

impl serde::Serialize for Counted {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        SERIALIZED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.0.serialize(s)
    }
}

#[cell(persist, id = "tally")]
pub struct Tally {
    n: u32,
}

/// A port's cell: aio's `tally` as a slice, and a field of its own.
#[cell(persist, id = "pane")]
pub struct Pane {
    #[aio_cell]
    tally: Tally,
    mark: Counted,
}

#[methods]
impl Pane {
    pub fn bump(&mut self) {
        self.tally.n += 1;
        self.mark.0 += 1;
    }
}

#[test]
fn a_commit_serializes_the_state_once_not_again_for_the_one_before() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::<Pane>::open_aio(dir.path().join("state.db"), "app").unwrap();
    s.call(&By::Test, "bump", serde_json::Value::Null).unwrap();
    let before = SERIALIZED.load(std::sync::atomic::Ordering::SeqCst);
    for _ in 0..20 {
        s.call(&By::Test, "bump", serde_json::Value::Null).unwrap();
    }
    let per = (SERIALIZED.load(std::sync::atomic::Ordering::SeqCst) - before) as f64 / 20.0;
    assert!(per <= 1.0, "{per} serializations per commit");
    drop(s);
    let s = Store::<Pane>::open_aio(dir.path().join("state.db"), "app").unwrap();
    assert_eq!((s.state().tally.n, s.state().mark.0), (21, 21), "and every slice saved");
}

static ROWS_SERIALIZED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A row that counts how often it is serialized.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
pub struct Row(u32);

impl serde::Serialize for Row {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        ROWS_SERIALIZED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.0.serialize(s)
    }
}

/// aio's `rows` cell: big, and serialized only when it changed.
#[cell(persist, id = "rows")]
pub struct Rows {
    all: Vec<Row>,
}

/// A port's cell: a big slice beside a small one.
#[cell(persist, id = "port")]
pub struct Port {
    #[aio_cell]
    rows: Rows,
    #[aio_cell]
    tally: Tally,
}

#[methods]
impl Port {
    pub fn fill(&mut self) {
        self.rows.all = (0..100).map(Row).collect();
    }

    pub fn bump(&mut self) {
        self.tally.n += 1;
    }
}

#[test]
fn a_commit_serializes_only_the_slices_that_changed_as_aio_did() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::<Port>::open_aio(dir.path().join("state.db"), "app").unwrap();
    s.call(&By::Test, "fill", serde_json::Value::Null).unwrap();
    s.call(&By::Test, "bump", serde_json::Value::Null).unwrap();
    let before = ROWS_SERIALIZED.load(std::sync::atomic::Ordering::SeqCst);
    for _ in 0..20 {
        s.call(&By::Test, "bump", serde_json::Value::Null).unwrap();
    }
    let n = ROWS_SERIALIZED.load(std::sync::atomic::Ordering::SeqCst) - before;
    assert_eq!(n, 0, "{n} rows serialized for 20 commits of the small slice");
    drop(s);
    let s = Store::<Port>::open_aio(dir.path().join("state.db"), "app").unwrap();
    assert_eq!((s.state().rows.all.len(), s.state().tally.n), (100, 21), "both slices saved");
}

/// aio's `rows` cell as plain numbers (its own counter stays the other test's).
#[cell(persist, id = "rows")]
pub struct PlainRows {
    all: Vec<u32>,
}

/// The big slice shared between states (`aio::Big`).
#[cell(persist, id = "port")]
pub struct BigPort {
    #[aio_cell]
    rows: aio::Big<PlainRows>,
    #[aio_cell]
    tally: Tally,
}

#[methods]
impl BigPort {
    pub fn fill(&mut self) {
        self.rows.all = (0..100).collect();
    }

    pub fn bump(&mut self) {
        self.tally.n += 1;
    }
}

#[test]
fn a_big_aio_cell_field_is_its_slice_shared_until_written() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::<BigPort>::open_aio(dir.path().join("state.db"), "app").unwrap();
    s.call(&By::Test, "fill", serde_json::Value::Null).unwrap();
    let rows = s.shared();
    s.call(&By::Test, "bump", serde_json::Value::Null).unwrap();
    assert!(aio::Big::ptr_eq(&rows.rows, &s.state().rows), "shared, not copied");
    drop(s);
    let s = Store::<BigPort>::open_aio(dir.path().join("state.db"), "app").unwrap();
    assert_eq!((s.state().rows.all.len(), s.state().tally.n), (100, 1), "both slices saved");
}

#[test]
fn a_held_key_is_written_once_a_debounce_window_as_aio_did() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let h = Handle::<List>::memory();
        h.store(filled);
        let _sched = h.start();
        let mut events = h.events();
        let t = std::time::Instant::now();
        let mut i = 0u32;
        // A key held for 6 windows, a step every 10 ms.
        while t.elapsed() < aio::PERSIST_DEBOUNCE * 6 {
            i += 1;
            h.call("focus", serde_json::json!([i % 50])).unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        tokio::time::sleep(aio::PERSIST_DEBOUNCE * 2).await;
        let n = std::iter::from_fn(|| events.try_recv().ok()).count() as u32;
        assert!((3..=8).contains(&n), "{n} writes for {i} steps: one per window, not one per step");
        assert_eq!(h.state().at, i % 50);
    });
}

/// A whole cell coalesced (`#[cell(coalesce)]`, aio's debounced writes):
/// every plain call shows at once and waits for the window; one that
/// needs an id can't wait, and goes after what was held.
#[cell(persist, coalesce)]
pub struct Notes {
    lines: Vec<String>,
    at: u32,
    ids: Vec<aio::Uuid>,
}

#[methods]
impl Notes {
    pub fn add(&mut self, line: String) {
        self.lines.push(line);
    }

    pub fn go(&mut self, at: u32) {
        self.at = at;
    }

    pub fn stamp(&mut self, cx: &Cx) {
        self.ids.push(cx.id());
    }
}

#[test]
fn a_coalesced_cell_holds_every_plain_call_and_logs_them_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let mut s = Store::<Notes>::open(&path).unwrap();
    let n = s.events().unwrap().len();
    for (m, a) in [("add", json!(["a"])), ("go", json!([1])), ("add", json!(["b"])), ("go", json!([2]))] {
        s.call(&By::Test, m, a).unwrap();
    }
    assert_eq!((s.state().lines.len(), s.state().at), (2, 2), "shown at once");
    assert_eq!(s.events().unwrap().len(), n, "nothing written yet");
    s.call(&By::Test, "stamp", serde_json::Value::Null).unwrap();
    let ev = s.events().unwrap();
    let methods: Vec<&str> = ev[n..].iter().map(|e| e.method.as_str()).collect();
    assert_eq!(
        methods,
        ["add", "go", "add", "go", "stamp"],
        "mixed calls: each, in order, then the one that couldn't wait"
    );
    drop(s);
    let s = Store::<Notes>::open(&path).unwrap();
    assert_eq!((s.state().lines.clone(), s.state().at, s.state().ids.len()), (vec!["a".into(), "b".into()], 2, 1));
}

#[test]
fn coalesce_ms_sets_the_window_and_zero_logs_each_call() {
    let mut s = Store::<List>::memory();
    filled(&mut s);
    s.set_coalesce(std::time::Duration::ZERO).unwrap();
    let before = s.events().unwrap().len();
    s.call(&By::Test, "focus", serde_json::json!([3])).unwrap();
    assert!(!s.holds(), "a zero window holds nothing");
    assert_eq!(s.events().unwrap().len(), before + 1);
    // A held run when the window shrinks to zero goes into the log at once.
    s.set_coalesce(std::time::Duration::from_secs(60)).unwrap();
    s.call(&By::Test, "focus", serde_json::json!([4])).unwrap();
    assert!(s.holds());
    s.set_coalesce(std::time::Duration::ZERO).unwrap();
    assert!(!s.holds());
    assert_eq!(s.events().unwrap().len(), before + 2);
}

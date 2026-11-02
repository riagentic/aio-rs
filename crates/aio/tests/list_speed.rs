//! A held ↓ over a 1000-row list, two ways: the blunt port (every view
//! reads the whole state, every step a commit) and the way aio-rs offers
//! (aio's): views read their fields (`part`), rows take props, the cursor is
//! coalesced. Renders, commits and wire bytes per key, and the step's p95.
//! `cargo test --test list_speed -- --nocapture` prints them.
use aio::act;
use aio::kit::{GlobalKeys, use_global_key};
use aio::testing::TestUi;
use aio::ui::prelude::*;
use std::time::{Duration, Instant};

const ROWS: usize = 1000;
/// Rows drawn around the cursor (aio windowed its long lists).
const SHOWN: usize = 30;

#[aio::data]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row {
    pub name: String,
    pub amount: u64,
}

fn rows() -> Vec<Row> {
    (0..ROWS).map(|i| Row { name: format!("Account {i}"), amount: i as u64 * 7 }).collect()
}

/// What one held key cost, per key.
#[derive(Debug)]
#[allow(dead_code)] // said by its Debug
struct Cost {
    renders: f64,
    /// Writes to the file (a commit, or a held run's one transaction).
    writes: f64,
    /// Rows added to the log.
    logged: f64,
    bytes: f64,
    p50: Duration,
    p95: Duration,
    /// The whole hold.
    took: Duration,
}

/// Hold ↓ for `keys` presses: what each cost.
fn hold(ui: &mut TestUi, keys: usize, events: &mut tokio::sync::broadcast::Receiver<(u64, aio::Event)>) -> Cost {
    let renders = |ui: &TestUi| {
        fn sum(c: &[aio::testing::MountedComponent]) -> u32 {
            c.iter().map(|c| c.renders + c.updates + sum(&c.children)).sum()
        }
        sum(&ui.components())
    };
    while events.try_recv().is_ok() {}
    let (r0, b0, t0) = (renders(ui), ui.wire_bytes(), Instant::now());
    let (mut writes, mut logged) = (0, 0);
    let mut took: Vec<Duration> = (0..keys)
        .map(|_| {
            let t = Instant::now();
            ui.press_key(Key::ArrowDown, Modifiers::empty());
            let took = t.elapsed();
            // What was written since the last key: one write, its events.
            let n = std::iter::from_fn(|| events.try_recv().ok()).count();
            (writes, logged) = (writes + (n > 0) as usize, logged + n);
            took
        })
        .collect();
    let held = t0.elapsed();
    took.sort();
    let per = |n: usize| n as f64 / keys as f64;
    Cost {
        renders: per((renders(ui) - r0) as usize),
        writes: per(writes),
        logged: per(logged),
        bytes: per(ui.wire_bytes() - b0),
        p50: took[keys / 2],
        p95: took[keys * 95 / 100],
        took: held,
    }
}

/// The events `e` heard since it was last drained (those it fell behind on too).
fn drained(e: &mut tokio::sync::broadcast::Receiver<(u64, aio::Event)>) -> usize {
    use tokio::sync::broadcast::error::TryRecvError;
    let mut n = 0;
    loop {
        match e.try_recv() {
            Ok(_) => n += 1,
            Err(TryRecvError::Lagged(k)) => n += k as usize,
            Err(_) => return n,
        }
    }
}

/// `n` calls of `f`: the p50 and p95 of one (the store's share of a key).
fn timed(n: usize, f: impl Fn()) -> (Duration, Duration) {
    let mut t: Vec<Duration> = (0..n)
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed()
        })
        .collect();
    t.sort();
    (t[n / 2], t[n * 95 / 100])
}

/// The blunt port: one whole-state reader everywhere, a commit per step.
mod blunt {
    use super::*;

    #[aio::cell(persist, id = "blunt")]
    pub struct List {
        pub rows: Vec<Row>,
        pub cursor: usize,
    }

    #[aio::methods]
    impl List {
        pub fn fill(&mut self, rows: Vec<Row>) {
            self.rows = rows;
        }

        pub fn down(&mut self) {
            self.cursor = (self.cursor + 1).min(self.rows.len().saturating_sub(1));
        }
    }

    aio::client!(List, list);

    #[component]
    fn Line(i: usize) -> View {
        rsx! {
            li { "aria-selected": list().cursor == i,
                {
                    let l = list();
                    format!("{} {}", l.rows[i].name, l.rows[i].amount)
                }
            }
        }
    }

    #[component]
    fn Lines() -> View {
        use_global_key("ArrowDown", |_| _ = act!(down()));
        let shown = || {
            let l = list();
            let from = l.cursor.saturating_sub(SHOWN / 2);
            from..(from + SHOWN).min(l.rows.len())
        };
        rsx! {
            ul { "data-t": "list",
                for i in shown() {
                    Line { key: "{i}", i }
                }
            }
        }
    }

    pub fn app() -> View {
        use_live();
        rsx! { GlobalKeys { Lines {} } }
    }
}

/// aio's way in aio-rs: `part` readers, rows by props, a coalesced cursor.
mod fast {
    use super::*;

    #[aio::cell(persist, id = "fast")]
    pub struct List {
        pub rows: Vec<Row>,
        pub cursor: usize,
    }

    #[aio::methods]
    impl List {
        pub fn fill(&mut self, rows: Vec<Row>) {
            self.rows = rows;
        }

        #[coalesce]
        pub fn down(&mut self) {
            self.cursor = (self.cursor + 1).min(self.rows.len().saturating_sub(1));
        }
    }

    aio::client!(List, list);

    #[component]
    fn Line(name: String, amount: u64, selected: Prop<bool>) -> View {
        rsx! { li { "aria-selected": selected.get(), "{name} {amount}" } }
    }

    #[component]
    fn Lines() -> View {
        use_global_key("ArrowDown", |_| _ = act!(down()));
        let (rows, cursor) = (part(|l| &l.rows), part(|l| &l.cursor));
        // The rows shown: the window moves only every few keys' worth of rows.
        let shown = memo(move || {
            let from = cursor.get().saturating_sub(SHOWN / 2);
            from..(from + SHOWN).min(rows.with(Vec::len))
        });
        rsx! {
            ul { "data-t": "list",
                for i in shown.get() {
                    Line {
                        key: "{i}",
                        name: rows.with(|r| r[i].name.clone()),
                        amount: rows.with(|r| r[i].amount),
                        selected: cursor.get() == i,
                    }
                }
            }
        }
    }

    pub fn app() -> View {
        use_live();
        rsx! { GlobalKeys { Lines {} } }
    }
}

#[test]
fn a_held_key_over_a_long_list_costs_a_few_renders_and_no_write_per_step() {
    const KEYS: usize = 60;
    let dir = tempfile::tempdir().unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let b = aio::Handle::<blunt::List>::open(dir.path().join("blunt.db")).unwrap();
    let f = aio::Handle::<fast::List>::open(dir.path().join("fast.db")).unwrap();
    use blunt::ListCalls as _;
    use fast::ListCalls as _;
    b.fill(rows()).unwrap();
    f.fill(rows()).unwrap();
    let (_bs, _fs) = {
        let _in = rt.enter();
        (b.start(), f.start())
    };
    let (mut be, mut fe) = (b.events(), f.events());
    assert!(
        blunt::SOURCE.set(aio::Source::Local(b.clone())).is_ok()
            && fast::SOURCE.set(aio::Source::Local(f.clone())).is_ok()
    );

    let mut ui = TestUi::new(blunt::app);
    let before = hold(&mut ui, KEYS, &mut be);
    drop(ui);
    let mut ui = TestUi::new(fast::app);
    let after = hold(&mut ui, KEYS, &mut fe);
    assert_eq!(ui.count("[aria-selected=true]"), 1);
    eprintln!("list speed (1000 rows, {KEYS} × ↓): blunt {before:?} → aio-rs {after:?}");
    drop(ui);
    // The store's share of a key: written per call, or once per window. Counted,
    // not timed: a loaded machine slows either side at random. The held keys'
    // run above is saved first (its last window may still be open).
    let saved = || {
        let last = f.store(|s| s.events()).unwrap().pop().unwrap();
        f.store(|s| s.state_at(last.seq)).unwrap().cursor
    };
    let end = Instant::now() + Duration::from_secs(10);
    while saved() != KEYS {
        assert!(Instant::now() < end, "the held keys were never saved");
        std::thread::sleep(Duration::from_millis(10));
    }
    drained(&mut be);
    drained(&mut fe);
    let bs = timed(300, || b.down().unwrap());
    let blunt_logged = drained(&mut be);
    let t = Instant::now();
    let fs = timed(300, || f.down().unwrap());
    let (fast_logged, fast_took) = (drained(&mut fe), t.elapsed());
    eprintln!("list speed, the store's share of a key (p50, p95): blunt {bs:?} → aio-rs {fs:?}");
    assert_eq!(blunt_logged, 300, "the blunt port writes every call");
    let windows = (fast_took.as_secs_f64() / aio::PERSIST_DEBOUNCE.as_secs_f64()).ceil() + 1.0;
    assert!(fast_logged as f64 <= windows, "a held call writes nothing: {fast_logged} rows in {fast_took:?}");

    assert!(after.renders <= 4.0, "the list and the two rows the cursor left and reached: {after:?}");
    assert!(before.renders > 4.0 * after.renders, "{before:?} vs {after:?}");
    let windows = (after.took.as_secs_f64() / aio::PERSIST_DEBOUNCE.as_secs_f64()).ceil() + 1.0;
    assert!(after.writes * (KEYS as f64) <= windows, "a write per debounce window at most: {after:?}");
    assert!(before.writes >= 1.0, "the blunt port writes every step: {before:?}");
    assert!(after.bytes <= before.bytes, "{before:?} vs {after:?}");
    // Once released, the run is saved.
    std::thread::sleep(aio::PERSIST_DEBOUNCE * 3);
    assert_eq!(f.state().cursor, KEYS + 300);
    let last = f.store(|s| s.events()).unwrap().pop().unwrap();
    assert_eq!(
        (last.method.as_str(), f.store(|s| s.state_at(last.seq)).unwrap().cursor),
        ("down", KEYS + 300),
        "saved"
    );
}

// Its own test binary: the timeline is the process's.
use aio::prelude::*;

#[cell(persist)]
pub struct Notes {
    pub items: Vec<String>,
}

#[methods]
impl Notes {
    pub fn add(&mut self, text: String) -> Result {
        ensure!(!text.is_empty(), "empty note");
        self.items.push(text);
        Ok(())
    }

    pub fn slow(&mut self) {
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
}

/// aio's `diagnostics: false`: a ticker nobody needs in the timeline.
#[cell(persist, diagnostics = false)]
pub struct Clock {
    pub ticks: u64,
}

#[methods]
impl Clock {
    pub fn tick(&mut self) {
        self.ticks += 1;
    }
}

#[test]
fn the_live_timeline_times_each_call_and_leaves_out_quiet_cells() {
    const { assert!(<Notes as CellData>::DIAGNOSTICS && !<Clock as CellData>::DIAGNOSTICS) };
    let d = tempfile::tempdir().unwrap();
    let (notes, clock) = (
        aio::Source::<Notes>::open(&d.path().join("notes.db")).unwrap(),
        aio::Source::<Clock>::open(&d.path().join("clock.db")).unwrap(),
    );
    notes.add("milk".into()).unwrap();
    clock.tick().unwrap();
    notes.add(String::new()).unwrap_err();
    clock.tick().unwrap();
    notes.slow().unwrap();
    let t = aio::report::timeline();
    let calls: Vec<_> = t.iter().map(|e| e["call"].as_str().unwrap()).collect();
    assert_eq!(calls, ["notes.add", "notes.add", "notes.slow"], "the clock's calls stay out");
    assert_eq!(t[1]["refused"], "empty note");
    assert!(t[2]["ms"].as_f64().unwrap() >= 30.0, "{}", t[2]);
    assert!(t.iter().all(|e| e["at"].is_string() && e["ms"].is_number()));
    assert_eq!(clock.state().ticks, 2, "its state and events are kept as ever");
}

//! `ax record` (aio: `am record`): turn what a running app's cell just did
//! into a Rust test. The test boots the cell on the state before the range,
//! makes the calls it took (its inputs only: what they caused, the test's
//! cell makes again), and asserts the state they left.
use crate::travel::{Row, skip};
use aio::serde_json::Value;

/// `TodoList` for the cell id `todo_list` (the `#[cell]` default id is the
/// type's name in snake case).
pub fn type_of(cell: &str) -> String {
    cell.split('_').map(|w| w[..1].to_uppercase() + &w[1..]).collect()
}

/// Can a test make the state `rows` left exactly (pure)? Not when one drew
/// ids or random numbers, or a schedule, an effect or the boot made one.
pub fn exact(rows: &[Row]) -> bool {
    rows.iter().all(|r| !r.drew && (r.method.starts_with('$') || skip(r, None).is_none()))
}

/// A test function's name from `file`'s stem (`tests/add_note.rs`).
pub fn name_of(file: &str) -> String {
    let stem = std::path::Path::new(file).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let n: String =
        stem.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect();
    if n.starts_with(|c: char| c.is_ascii_alphabetic()) { n } else { format!("recorded_{n}") }
}

/// The test (pure): cell `ty` (its path, `notes::Notes`) booted on `before`, then each of `rows` that
/// is an input called again, then `after` asserted when the test can make
/// it `exact`ly (no call drew ids or random numbers, which come out new in
/// a test, and no schedule or boot made a change between them); else said.
pub fn test(app: &str, ty: &str, name: &str, before: &Value, rows: &[Row], after: &Value, exact: bool) -> String {
    let json = |v: &Value| aio::serde_json::to_string(v).unwrap_or_default();
    let range = match (rows.first(), rows.last()) {
        (Some(a), Some(b)) => format!("{}..{}", a.seq, b.seq),
        _ => "-".into(),
    };
    let mut out = format!(
        "//! Recorded by `ax record` from the events {range} of {app}: the calls\n\
         //! they took, on the state before them, and the state they left.\n\
         use aio::prelude::*;\n\
         use aio::serde_json::json;\n\
         use aio::testing::TestCells;\n\
         use {ty};\n\n\
         #[test]\n\
         fn {name}() {{\n    \
         let mut t = TestCells::new();\n    \
         let h = t.seeded::<{name_ty}>(json!({}));\n",
        json(before),
        name_ty = ty.rsplit("::").next().unwrap_or(ty)
    );
    for r in rows {
        match skip(r, None) {
            Some(why) => out.push_str(&format!("    // {} {}: not called again: {why}\n", r.seq, r.method)),
            None => {
                out.push_str(&format!("    h.call({:?}, json!({})).unwrap(); // {}\n", r.method, json(&r.args), r.seq))
            }
        }
    }
    out.push_str("    t.settle();\n");
    let check = format!("aio::serde_json::to_value(&*h.state()).unwrap(), json!({})", json(after));
    match exact {
        true => out.push_str(&format!("    assert_eq!({check});\n")),
        false => out.push_str(&format!(
            "    // Its calls drew ids or random numbers, or a schedule ran between them,\n    \
             // which a test makes otherwise; the state they left was:\n    // {}\n",
            json(after)
        )),
    }
    out.push_str("}\n");
    out
}

/// The recording `the_test_it_writes_is_the_one_that_compiles_and_passes`
/// makes, as a test of ours: it compiles, and it passes.
#[cfg(test)]
#[path = "record_fixture.rs"]
#[rustfmt::skip]
mod recorded;

#[cfg(test)]
mod tests {
    use super::*;
    use aio::prelude::*;
    use aio::serde_json::json;

    #[cell(persist)]
    pub struct Steps {
        pub n: u32,
        pub log: Vec<String>,
    }

    #[methods]
    impl Steps {
        pub fn add(&mut self, by: u32, why: String) -> Result {
            self.n += by;
            self.log.push(why);
            Ok(())
        }
    }

    fn row(seq: u64, method: &str, args: Value, by: Value) -> Row {
        Row { seq, cell: "steps".into(), method: method.into(), args, by, v: 1, drew: false }
    }

    #[test]
    fn the_test_it_writes_is_the_one_that_compiles_and_passes() {
        let rows = [
            row(4, "add", json!([2, "first"]), json!({ "User": "ana" })),
            row(5, "$rewind", json!({ "to": 4 }), json!({ "User": "ops" })),
            row(6, "add", json!([3, "second"]), json!({ "Agent": "bot" })),
        ];
        let before = json!({ "n": 1, "log": ["seed"] });
        let after = json!({ "n": 6, "log": ["seed", "first", "second"] });
        let text = test("notes", "super::tests::Steps", "adds_two_notes", &before, &rows, &after, true);
        assert_eq!(text, include_str!("record_fixture.rs"));
        let tick = [row(7, "add", json!([1, "tick"]), json!({ "Schedule": "tick" }))];
        let inexact = test("notes", "Steps", "x", &before, &tick, &after, false);
        assert!(inexact.contains("// 7 add: not called again: caused by a schedule"), "{inexact}");
        assert!(inexact.contains("the state they left was:") && !inexact.contains("assert_eq!"), "{inexact}");
        assert!(exact(&rows) && !exact(&tick));
        assert!(!exact(&[Row { drew: true, ..rows[0].clone() }]), "a call that drew an id");
    }

    #[test]
    fn names_come_from_the_cell_and_the_file() {
        assert_eq!(type_of("todo_list"), "TodoList");
        assert_eq!(type_of("notes"), "Notes");
        assert_eq!(name_of("tests/add-note.rs"), "add_note");
        assert_eq!(name_of("tests/2fa.rs"), "recorded_2fa");
    }
}

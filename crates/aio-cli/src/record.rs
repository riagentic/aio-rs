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

/// `[cell] <file> [range]` (pure): a `.rs` file first records every cell
/// the range touched (aio's `am record`), else the cell named first.
pub fn args<'a, 'b>(rest: &'b [&'a str]) -> Result<(Option<&'a str>, &'a str, &'b [&'a str]), String> {
    match rest {
        [file, spec @ ..] if file.ends_with(".rs") => Ok((None, file, spec)),
        [cell, file, spec @ ..] => Ok((Some(cell), file, spec)),
        _ => Err("ax record <app> [cell] <file.rs> [range]".into()),
    }
}

/// A test function's name from `file`'s stem (`tests/add_note.rs`).
pub fn name_of(file: &str) -> String {
    let stem = std::path::Path::new(file).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let n: String =
        stem.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect();
    if n.starts_with(|c: char| c.is_ascii_alphabetic()) { n } else { format!("recorded_{n}") }
}

/// One cell of a recording: its id, its type's path (`notes::Notes`), and
/// its states before and after the range.
pub struct Recorded<'a> {
    pub id: &'a str,
    pub ty: String,
    pub before: &'a Value,
    pub after: &'a Value,
}

/// The test (pure): each of `cells` booted on its state before, then each of
/// `rows` (of any of them, in the log's order) that is an input called again,
/// then each state after asserted when the test can make it `exact`ly (no
/// call drew ids or random numbers, which come out new in a test, and no
/// schedule or boot made a change between them); else said. One cell's
/// handle is `h`, several are named by their ids.
pub fn test(app: &str, name: &str, cells: &[Recorded], rows: &[Row], exact: bool) -> String {
    let json = |v: &Value| aio::serde_json::to_string(v).unwrap_or_default();
    let range = match (rows.first(), rows.last()) {
        (Some(a), Some(b)) => format!("{}..{}", a.seq, b.seq),
        _ => "-".into(),
    };
    let var = |id: &str| if cells.len() == 1 { "h".to_string() } else { id.to_string() };
    let uses: String = cells.iter().map(|c| format!("use {};\n", c.ty)).collect();
    let mut out = format!(
        "//! Recorded by `ax record` from the events {range} of {app}: the calls\n\
         //! they took, on the state before them, and the state they left.\n\
         use aio::prelude::*;\n\
         use aio::serde_json::json;\n\
         use aio::testing::TestCells;\n\
         {uses}\n\
         #[test]\n\
         fn {name}() {{\n    \
         let mut t = TestCells::new();\n"
    );
    for c in cells {
        let short = c.ty.rsplit("::").next().unwrap_or(&c.ty);
        out.push_str(&format!("    let {} = t.seeded::<{short}>(json!({}));\n", var(c.id), json(c.before)));
    }
    for r in rows {
        match skip(r, None) {
            Some(why) => out.push_str(&format!("    // {} {}: not called again: {why}\n", r.seq, r.method)),
            None => {
                // As its caller: a method may keep who called (`cx.by()`).
                out.push_str(&format!(
                    "    {}.by(by(json!({}))).call({:?}, json!({})).unwrap(); // {}\n",
                    var(&r.cell),
                    json(&r.by),
                    r.method,
                    json(&r.args),
                    r.seq
                ))
            }
        }
    }
    out.push_str("    t.settle();\n");
    match exact {
        true => cells.iter().for_each(|c| {
            out.push_str(&format!(
                "    assert_eq!(aio::serde_json::to_value(&*{}.state()).unwrap(), json!({}));\n",
                var(c.id),
                json(c.after)
            ))
        }),
        false => {
            out.push_str(
                "    // Its calls drew ids or random numbers, or a schedule ran between them,\n    \
                 // which a test makes otherwise; the state they left was:\n",
            );
            cells.iter().for_each(|c| out.push_str(&format!("    // {}: {}\n", c.id, json(c.after))));
        }
    }
    out.push_str("}\n\n/// Who made a call, as the log has it.\nfn by(v: aio::serde_json::Value) -> aio::By {\n    aio::serde_json::from_value(v).unwrap_or_default()\n}\n");
    out
}

/// The recording `the_test_it_writes_is_the_one_that_compiles_and_passes`
/// makes, as a test of ours: it compiles, and it passes.
#[cfg(test)]
#[path = "record_fixture.rs"]
#[rustfmt::skip]
mod recorded;

/// The whole-app recording `a_whole_app_recording_boots_every_cell_it_touched`
/// makes: it compiles, and it passes.
#[cfg(test)]
#[path = "record_fixture_app.rs"]
#[rustfmt::skip]
mod recorded_app;

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

    #[cell(persist)]
    pub struct Tally {
        pub n: u32,
    }

    #[methods]
    impl Tally {
        pub fn bump(&mut self) -> Result {
            self.n += 1;
            Ok(())
        }
    }

    fn row(seq: u64, method: &str, args: Value, by: Value) -> Row {
        Row { seq, cell: "steps".into(), method: method.into(), args, by, v: 1, drew: false }
    }

    fn one<'a>(ty: &str, before: &'a Value, after: &'a Value) -> [Recorded<'a>; 1] {
        [Recorded { id: "steps", ty: ty.into(), before, after }]
    }

    #[test]
    fn the_test_it_writes_is_the_one_that_compiles_and_passes() {
        let rows = [
            row(4, "add", json!([2, "first"]), json!({ "kind": "user", "name": "ana" })),
            row(5, "$rewind", json!({ "to": 4 }), json!({ "kind": "user", "name": "ops" })),
            row(6, "add", json!([3, "second"]), json!({ "kind": "agent", "name": "bot" })),
        ];
        let before = json!({ "n": 1, "log": ["seed"] });
        let after = json!({ "n": 6, "log": ["seed", "first", "second"] });
        let text = test("notes", "adds_two_notes", &one("super::tests::Steps", &before, &after), &rows, true);
        assert_eq!(text, include_str!("record_fixture.rs"));
        let tick = [row(7, "add", json!([1, "tick"]), json!({ "Schedule": "tick" }))];
        let inexact = test("notes", "x", &one("Steps", &before, &after), &tick, false);
        assert!(inexact.contains("// 7 add: not called again: caused by a schedule"), "{inexact}");
        assert!(inexact.contains("the state they left was:") && !inexact.contains("assert_eq!"), "{inexact}");
        assert!(exact(&rows) && !exact(&tick));
        assert!(!exact(&[Row { drew: true, ..rows[0].clone() }]), "a call that drew an id");
    }

    #[test]
    fn a_whole_app_recording_boots_every_cell_it_touched() {
        let user = json!({ "kind": "user", "name": "ana" });
        let rows = [
            row(8, "add", json!([2, "first"]), user.clone()),
            Row { cell: "tally".into(), ..row(9, "bump", json!([]), user.clone()) },
            row(10, "add", json!([1, "second"]), user),
        ];
        let (steps_at, tally_at) = (json!({ "n": 0, "log": [] }), json!({ "n": 4 }));
        let (steps_left, tally_left) = (json!({ "n": 3, "log": ["first", "second"] }), json!({ "n": 5 }));
        let cells = [
            Recorded { id: "steps", ty: "super::tests::Steps".into(), before: &steps_at, after: &steps_left },
            Recorded { id: "tally", ty: "super::tests::Tally".into(), before: &tally_at, after: &tally_left },
        ];
        assert_eq!(test("notes", "a_session", &cells, &rows, true), include_str!("record_fixture_app.rs"));
        let inexact = test("notes", "x", &cells, &rows, false);
        assert!(inexact.contains("// tally: {\"n\":5}") && !inexact.contains("assert_eq!"), "{inexact}");
    }

    #[test]
    fn names_come_from_the_cell_and_the_file() {
        assert_eq!(type_of("todo_list"), "TodoList");
        assert_eq!(type_of("notes"), "Notes");
        assert_eq!(name_of("tests/add-note.rs"), "add_note");
        assert_eq!(name_of("tests/2fa.rs"), "recorded_2fa");
        assert_eq!(args(&["tests/s.rs", "4..9"]), Ok((None, "tests/s.rs", &["4..9"][..])));
        assert_eq!(args(&["tasks", "bug.rs"]), Ok((Some("tasks"), "bug.rs", &[][..])));
        assert!(args(&["tasks"]).is_err());
    }
}

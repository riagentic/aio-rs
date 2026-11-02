//! Gate 3 in the Rust page: a test of the page fails when the renderer
//! breaks. Each fault is planted in air (feature `fault`), and the same
//! test that passes on a sound renderer must fail on the broken one.
use air::__fault::{Fault, plant};
use air::testing::TestPage;
use air::*;

fn app() -> View {
    let n = signal(|| 0u32);
    let rows = signal(|| vec![1u32, 2]);
    rsx! {
        button { "data-t": "add", onclick: move |_| { n += 1; rows.update(|r| r.push(r.len() as u32 + 1)) }, "add" }
        p { "data-t": "n", "{n}" }
        ul { "data-t": "rows", for r in rows.get() { li { key: r, "row {r}" } } }
    }
}

/// What a person sees after two clicks.
fn the_test() {
    let mut p = TestPage::mount(app);
    p.click("add");
    p.click("add");
    assert_eq!(p.text("n"), "2", "the count follows the clicks");
    assert_eq!(p.texts("rows"), ["row 1", "row 2", "row 3", "row 4"], "every row is shown");
}

fn fails(f: Fault) -> String {
    plant(f);
    let r = std::panic::catch_unwind(the_test);
    plant(Fault::None);
    let e = r.expect_err("the planted fault must fail the test");
    e.downcast_ref::<String>().cloned().unwrap_or_default()
}

#[test]
fn gate_3_the_test_passes_on_a_sound_renderer() {
    the_test();
}

#[test]
fn gate_3_an_unbound_hole_fails_the_test() {
    assert!(fails(Fault::UnboundHole).contains("the count follows the clicks"));
}

#[test]
fn gate_3_a_lost_row_fails_the_test() {
    assert!(fails(Fault::LostRow).contains("every row is shown"));
}

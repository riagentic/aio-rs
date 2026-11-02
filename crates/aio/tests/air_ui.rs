//! `TestUi` on air (feature `air`): the app's views drawn by air in the
//! page the window shows (the pinned Electron, headless), driven like a
//! person. Gate 3 of docs/design/air.md: a renderer fault planted in air
//! (`air::__fault`) fails the same test that passes on the sound one.
#![cfg(all(feature = "air", not(feature = "kit")))]
use aio::testing::{TestUi, fails};
use aio::ui::prelude::*;

fn counter() -> View {
    let n = signal(|| 0u32);
    let rows = signal(|| vec![1u32, 2]);
    let name = signal(String::new);
    rsx! {
        main {
            button { "data-t": "add", onclick: move |_| { n += 1; rows.update(|r| r.push(r.len() as u32 + 1)) }, "Add" }
            p { "data-t": "n", "{n}" }
            ul { "data-t": "rows", for r in rows.get() { li { key: r, "row {r}" } } }
            input { "data-t": "name", value: name, oninput: move |e: FormEvent| name.set(e.value()) }
            p { "data-t": "hello", if name.with(String::is_empty) { "Who are you?" } else { "Hello, {name}!" } }
        }
    }
}

/// What a person sees after two clicks and some typing.
fn the_test() {
    let mut ui = TestUi::new(counter);
    ui.click("add");
    ui.click("add");
    assert_eq!(ui.text("n"), "2", "the count follows the clicks");
    assert_eq!(ui.count("[data-t=rows] li"), 4, "every row is shown");
    ui.type_text("name", "Ada");
    assert_eq!(ui.text("hello"), "Hello, Ada!");
    assert_eq!(ui.value("name"), "Ada", "typing is never undone by an older echo");
}

#[test]
fn a_view_on_air_is_driven_in_the_windows_page() {
    the_test();
}

#[test]
fn gate_3_an_unbound_hole_fails_the_ui_test() {
    air::__fault::plant(air::__fault::Fault::UnboundHole);
    let e = fails(the_test);
    air::__fault::plant(air::__fault::Fault::None);
    assert!(e.contains("the count follows the clicks"), "{e}");
}

#[test]
fn gate_3_a_lost_row_fails_the_ui_test() {
    air::__fault::plant(air::__fault::Fault::LostRow);
    let e = fails(the_test);
    air::__fault::plant(air::__fault::Fault::None);
    assert!(e.contains("every row is shown"), "{e}");
}

#[test]
fn keys_bubble_and_a_page_script_answers_its_view() {
    fn app() -> View {
        let got = signal(|| 0i64);
        let keys = signal(Vec::<String>::new);
        task(move || async move {
            let mut e = eval("aio.send(41 + 1); const back = await aio.recv(); aio.send(back * 2);");
            if let Ok(v) = e.recv::<i64>().await {
                _ = e.send(v);
            }
            if let Ok(v) = e.recv::<i64>().await {
                got.set(v);
            }
        });
        rsx! {
            div { "data-t": "box", tabindex: "0", onkeydown: move |e: KeyboardEvent| keys.update(|k| k.push(format!("outer {}", e.key()))),
                input { "data-t": "field", onkeydown: move |e: KeyboardEvent| keys.update(|k| k.push(format!("field {} {}", e.key(), e.modifiers().contains(Modifiers::SHIFT)))) }
            }
            p { "data-t": "got", "{got}" }
            p { "data-t": "keys", {keys.get().join(", ")} }
        }
    }
    let mut ui = TestUi::new(app);
    ui.until_text("got", "84");
    ui.press_with("field", "ArrowDown", Modifiers::SHIFT);
    assert_eq!(ui.text("keys"), "field ArrowDown true, outer ArrowDown", "innermost first, then up");
}

static PICKED: Global<u32> = Global::new(|| 0);

#[test]
fn globals_live_in_the_apps_root_and_a_test_reads_and_runs_there() {
    fn app() -> View {
        rsx! { button { "data-t": "pick", onclick: move |_| PICKED.set(7), "{PICKED}" } }
    }
    let mut ui = TestUi::new(app);
    assert!(!in_views(), "a test's own code");
    assert!(ui.run(in_views));
    ui.click("pick");
    assert_eq!(ui.read(&PICKED), 7);
    ui.run(|| PICKED.set(9));
    assert_eq!(ui.text("pick"), "9");
}

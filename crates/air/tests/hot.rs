//! Hot reload (`air::hot`, `Root::hot`): views built again in place keep
//! their hooks' state, by place and by call order; markup sent for a
//! template replaces its built one; nothing is left behind.
use air::hot::Markup;
use air::testing::TestPage;
use air::*;
use std::cell::Cell;

#[component]
fn Counter(label: String) -> View {
    let n = signal(|| 0u32);
    rsx! { button { "data-t": "{label}", onclick: move |_| n += 1, "{label} {n}" } }
}

fn app() -> View {
    let total = signal(|| 10u32);
    let rows = signal(|| vec![1u32, 2]);
    rsx! {
        p { "data-t": "total", onclick: move |_| total += 1, "{total}" }
        Counter { label: "a" }
        Counter { label: "b" }
        ul {
            for r in rows.get() {
                Counter { key: r, label: format!("row{r}") }
            }
        }
        button { "data-t": "flip", onclick: move |_| rows.update(|r| r.reverse()), "flip" }
    }
}

#[test]
fn views_built_again_keep_their_state_by_place() {
    let mut page = TestPage::mount(app);
    page.click("total");
    page.click("a");
    page.click("b");
    page.click("b");
    page.click("row2");
    page.click("flip");
    let before = page.html();
    let live = air::live();
    page.root().hot();
    page.flush();
    assert_eq!(page.html(), before, "the same page, drawn anew");
    assert_eq!(
        (page.text("total"), page.text("a"), page.text("b"), page.text("row1"), page.text("row2")),
        ("11".into(), "a 1".into(), "b 2".into(), "row1 0".into(), "row2 1".into())
    );
    assert_eq!(air::live(), live, "the views before are gone, their state kept once");
    // Still live: a click after the reload is the kept signal's.
    page.click("b");
    assert_eq!(page.text("b"), "b 3");
}

thread_local!(static CHANGED: Cell<bool> = const { Cell::new(false) });

fn changing() -> View {
    // Its "code" changes between the two builds: another hook first.
    if CHANGED.get() {
        let _first = signal(|| "new");
    }
    let n = signal(|| 0u32);
    rsx! { p { "data-t": "n", onclick: move |_| n += 1, "{n}" } }
}

#[test]
fn a_view_whose_hooks_changed_starts_afresh_from_the_first_that_differs() {
    let mut page = TestPage::mount(changing);
    page.click("n");
    assert_eq!(page.text("n"), "1");
    CHANGED.set(true);
    page.root().hot();
    page.flush();
    assert_eq!(page.text("n"), "0", "a hook of another type at its place: fresh, no misuse");
    page.click("n");
    // Built again with the same hooks: kept from here on.
    page.root().hot();
    page.flush();
    assert_eq!(page.text("n"), "1");
}

thread_local!(static AT: Cell<u32> = const { Cell::new(0) });

fn greeting() -> View {
    let n = signal(|| 0u32);
    // The `rsx!` call's place: the next line, column 5.
    AT.set(line!() + 1);
    rsx! { p { "data-t": "hi", onclick: move |_| n += 1, "Hello" } span { "{n}" } }
}

/// The template `p { "data-t": "hi", <text> }` + `span { hole }` as `rsx!` encodes it.
fn markup(text: &str) -> Vec<u8> {
    let s = |b: &mut Vec<u8>, v: &str| {
        b.extend_from_slice(&(v.len() as u32).to_le_bytes());
        b.extend_from_slice(v.as_bytes());
    };
    let mut b = vec![1];
    s(&mut b, "p");
    b.push(1);
    s(&mut b, "data-t");
    s(&mut b, "hi");
    b.extend_from_slice(&1u16.to_le_bytes());
    b.push(1);
    b.push(2);
    s(&mut b, text);
    b.push(1);
    s(&mut b, "span");
    b.push(0);
    b.extend_from_slice(&1u16.to_le_bytes());
    b.push(0);
    b.push(3);
    b
}

#[test]
fn markup_sent_for_a_template_replaces_it_and_the_state_stays() {
    let mut page = TestPage::mount(greeting);
    page.click("hi");
    assert_eq!(page.text("hi"), "Hello");
    let edit = Markup { file: file!().into(), line: AT.get(), col: 5, idx: 0, nodes: markup("Hi there"), count: 4 };
    air::hot::set_markup(&[file!().into()], vec![edit]);
    page.root().hot();
    page.flush();
    assert_eq!(page.text("hi"), "Hi there", "the new markup, nothing compiled: {}", page.html());
    assert!(page.html().contains("<span>1</span>"), "the hole still follows its signal: {}", page.html());
    page.click("hi");
    assert!(page.html().contains("<span>2</span>"), "{}", page.html());
    // The file's markup sent again without that template: its built one is back.
    air::hot::set_markup(&[file!().into()], vec![]);
    page.root().hot();
    page.flush();
    assert_eq!(page.text("hi"), "Hello");
    assert!(page.html().contains("<span>2</span>"), "{}", page.html());
}

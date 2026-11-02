//! The spike's views in the Rust page: what the window would show, and
//! what each step sends.
use air::testing::TestPage;
use air::{FormEvent, Key, KeyboardEvent, Raw, Signal, View, component, rsx, selector};

#[derive(Clone, Debug, PartialEq)]
struct Row {
    id: u32,
    name: String,
}

fn rows(n: u32) -> Vec<Row> {
    (0..n).map(|id| Row { id, name: format!("Account {id}") }).collect()
}

#[test]
fn a_counter_counts_and_resets() {
    fn app() -> View {
        let count = Signal::new(0i64);
        rsx! {
            main {
                h1 { "data-t": "count", "{count}" }
                button { "data-t": "add", onclick: move |_| count += 1, "Add" }
                button { "data-t": "sub", onclick: move |_| count -= 1, "Subtract" }
                button { "data-t": "reset", disabled: count.get() == 0, onclick: move |_| count.set(0), "Reset" }
            }
        }
    }
    let mut p = TestPage::mount(app);
    assert_eq!(p.text("count"), "0");
    assert_eq!(p.attr("reset", "disabled").as_deref(), Some(""), "nothing to reset");
    p.click("add");
    p.click("add");
    p.click("sub");
    assert_eq!(p.text("count"), "1");
    assert_eq!(p.attr("reset", "disabled"), None);
    let before = p.bytes;
    p.click("add");
    // One SET_TEXT: op 1 + id 4 + len 4 + "2".
    assert_eq!(p.bytes - before, 10, "a click sends one text edit");
    p.click("reset");
    assert_eq!(p.text("count"), "0");
}

/// `aria-*` and `data-*` bools are text (`"true"`, `"false"`), as ARIA reads
/// them; `disabled` and the other HTML flags are presence.
#[test]
fn an_aria_bool_is_text_and_a_flag_is_presence() {
    fn app() -> View {
        let on = Signal::new(false);
        rsx! {
            li { "data-t": "row", "aria-selected": on.get(), "aria-busy": true, "data-open": false, hidden: on.get(), onclick: move |_| on.set(true), "Row" }
        }
    }
    let mut p = TestPage::mount(app);
    let attrs = |p: &TestPage| ["aria-selected", "aria-busy", "data-open", "hidden"].map(|a| p.attr("row", a));
    let s = |v: &str| Some(v.to_string());
    assert_eq!(attrs(&p), [s("false"), s("true"), s("false"), None]);
    p.click("row");
    assert_eq!(attrs(&p), [s("true"), s("true"), s("false"), s("")]);
}

/// 1000 rows, a filter, a cursor: the list keyed by id.
fn list() -> View {
    let all = Signal::new(rows(1000));
    let filter = Signal::new(String::new());
    let cursor = Signal::new(0u32);
    let sel = selector(move || cursor.get());
    let shown = air::memo(move || {
        let f = filter.get();
        all.with(|r| r.iter().filter(|r| r.name.contains(f.as_str())).cloned().collect::<Vec<_>>())
    });
    rsx! {
        input { "data-t": "filter", value: filter.get(), oninput: move |e: FormEvent| filter.set(e.value()) }
        div { "data-t": "keys", tabindex: "0",
            onkeydown: move |e: KeyboardEvent| match e.key() {
                Key::ArrowDown => cursor += 1,
                Key::ArrowUp => cursor.update(|c| *c = c.saturating_sub(1)),
                Key::Character(c) if c == "r" => all.update(|r| r.reverse()),
                Key::Character(c) if c == "x" => all.update(|r| _ = r.remove(0)),
                Key::Character(c) if c == "i" => all.update(|r| r.insert(0, Row { id: 5000, name: "New".into() })),
                _ => {}
            },
            ul { "data-t": "list",
                for row in shown.get() {
                    li { key: row.id, class: if sel.is(row.id) { "row on" } else { "row" }, "{row.name}" }
                }
            }
        }
    }
}

#[test]
fn a_keyed_list_moves_inserts_and_removes_rows() {
    let mut p = TestPage::mount(list);
    let texts = p.texts("list");
    assert_eq!(texts.len(), 1000);
    assert_eq!(texts[0], "Account 0");
    let named = p.named();
    p.key("keys", "r");
    assert_eq!(p.texts("list")[0], "Account 999", "reversed");
    assert_eq!(p.named(), named, "rows moved, none made");
    p.key("keys", "x");
    assert_eq!(p.texts("list").len(), 999);
    p.key("keys", "i");
    assert_eq!(p.texts("list")[0], "New");
    p.input("filter", "Account 99");
    let t = p.texts("list");
    assert_eq!(t.len(), 10, "99 and 990..=998: 999 was removed ({t:?})");
    p.input("filter", "");
    assert_eq!(p.texts("list").len(), 1000);
}

#[test]
fn a_held_arrow_rewrites_two_rows() {
    let mut p = TestPage::mount(list);
    let html = p.html();
    assert!(html.contains(r#"<li class="row on">Account 0</li>"#), "{}", &html[..300]);
    let (b0, f0) = (p.bytes, p.frames);
    for _ in 0..100 {
        p.key("keys", "ArrowDown");
    }
    assert_eq!(p.frames - f0, 100, "a frame a key");
    let per = (p.bytes - b0) / 100;
    // Two SET_ATTR "class": 1 + 4 + (4 + 5) + (4 + 3..6).
    assert!(per <= 50, "{per} bytes a key");
    assert!(p.html().contains(r#"<li class="row on">Account 100</li>"#));
}

#[test]
fn a_form_shows_what_was_typed_and_submits() {
    fn form() -> View {
        let name = Signal::new(String::new());
        let agreed = Signal::new(false);
        let sent = Signal::new(Vec::<String>::new());
        rsx! {
            form { "data-t": "form", onsubmit: move |_| sent.update(|s| s.push(name.get_untracked())),
                input { "data-t": "name", value: name, oninput: move |e| name.set(e.value()) }
                input { "data-t": "agree", r#type: "checkbox", checked: agreed.get(), onchange: move |e| agreed.set(e.checked()) }
                p { "data-t": "hello", if name.with(|n| n.is_empty()) { "Who are you?" } else { "Hello, {name}!" } }
                button { "data-t": "send", disabled: !agreed.get(), "Send" }
                ul { "data-t": "sent", for (i, s) in sent.get().into_iter().enumerate() { li { key: i, "{s}" } } }
            }
        }
    }
    let mut p = TestPage::mount(form);
    assert_eq!(p.text("hello"), "Who are you?");
    p.input("name", "Ada");
    assert_eq!(p.text("hello"), "Hello, Ada!");
    assert_eq!(p.attr("name", "value").as_deref(), Some("Ada"));
    assert!(p.attr("send", "disabled").is_some());
    p.check("agree", true);
    assert_eq!(p.attr("send", "disabled"), None);
    p.fire("form", Raw { name: "submit".into(), ..Raw::default() });
    assert_eq!(p.texts("sent"), ["Ada"]);
}

#[component]
fn Card(title: String, count: Signal<u32>, children: View) -> View {
    rsx! {
        section { class: "card",
            h2 { "data-t": "title", "{title} ({count})" }
            {children}
        }
    }
}

#[test]
fn a_component_takes_props_and_children_and_frees_all_it_made() {
    fn app() -> View {
        let n = Signal::new(1u32);
        let open = Signal::new(true);
        rsx! {
            button { "data-t": "more", onclick: move |_| n += 1, "More" }
            button { "data-t": "toggle", onclick: move |_| open.update(|o| *o = !*o), "Toggle" }
            if open.get() {
                Card { title: "Cart", count: n,
                    p { "data-t": "body", "{n} items" }
                    for i in 0..n.get() { span { key: i, "·" } }
                }
            } else {
                p { "data-t": "closed", "closed" }
            }
        }
    }
    let mut p = TestPage::mount(app);
    assert_eq!(p.text("title"), "Cart (1)");
    p.click("more");
    assert_eq!(p.text("title"), "Cart (2)");
    assert_eq!(p.text("body"), "2 items");
    let (live, named) = (air::live(), p.named());
    for _ in 0..5 {
        p.click("toggle");
        assert_eq!(p.text("closed"), "closed");
        p.click("toggle");
    }
    assert_eq!(p.text("title"), "Cart (2)");
    assert_eq!(air::live(), live, "no signal, effect, subscription, handler or owner left behind");
    assert_eq!(p.named(), named, "no node left named in the page");
}

//! The spike's views in the Rust page: what the window would show, and
//! what each step sends.
use air::testing::TestPage;
use air::{FormEvent, Key, KeyboardEvent, Raw, Signal, View, component, rsx, selector, signal};

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
/// them; `hidden` and the other HTML flags are presence; `None` is none.
#[test]
fn an_aria_bool_is_text_and_a_flag_is_presence() {
    fn app() -> View {
        let on = Signal::new(false);
        rsx! {
            li {
                "data-t": "row",
                "aria-selected": on.get(),
                "aria-busy": true,
                "data-open": false,
                "aria-controls": on.get().then_some("menu"),
                hidden: on.get(),
                onclick: move |_| on.set(true),
                "Row"
            }
        }
    }
    let mut p = TestPage::mount(app);
    let attrs =
        |p: &TestPage| ["aria-selected", "aria-busy", "data-open", "aria-controls", "hidden"].map(|a| p.attr("row", a));
    let s = |v: &str| Some(v.to_string());
    assert_eq!(attrs(&p), [s("false"), s("true"), s("false"), None, None], "an Option's None is no attribute");
    p.click("row");
    assert_eq!(attrs(&p), [s("true"), s("true"), s("false"), s("menu"), s("")]);
}

/// A hole may take what it reads by value (`match who`, `note.map(..)`):
/// its names are cloned in per run.
#[test]
fn a_hole_takes_its_names_by_value() {
    fn app() -> View {
        let n = Signal::new(0i64);
        let who = String::from("ada");
        let note: Option<String> = Some("n".into());
        rsx! {
            button { "data-t": "add", onclick: move |_| n += 1, "+" }
            p { "data-t": "a", {match who { w if n.get() > 0 => format!("{w}!"), w => w }} }
            p { "data-t": "b", {note.map(|s| format!("{s}{}", n.get())).unwrap_or_default()} }
        }
    }
    let mut p = TestPage::mount(app);
    assert_eq!((p.text("a"), p.text("b")), ("ada".to_string(), "n0".to_string()));
    p.click("add");
    assert_eq!((p.text("a"), p.text("b")), ("ada!".to_string(), "n1".to_string()));
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

#[test]
fn a_hole_may_return_a_borrow_of_what_it_reads() {
    // `t(&x)` borrows the hole's own clone of `x`: a child or a row's
    // child (each was E0716 "temporary value dropped while borrowed").
    fn t(s: &str) -> &str {
        s
    }
    fn app() -> View {
        let name = String::from("Ada");
        let names = vec![String::from("Bo")];
        rsx! {
            p { "data-t": "one", {t(&name)} }
            ul { "data-t": "rows",
                for x in names.clone() {
                    li { key: "{x}", {t(&x)} }
                }
            }
        }
    }
    let mut p = TestPage::mount(app);
    p.flush();
    assert_eq!(p.text("one"), "Ada");
    assert_eq!(p.texts("rows"), ["Bo"]);
}

#[test]
fn an_attribute_takes_any_string_a_hole_makes() {
    // An attribute took only a `&'static str`: a borrow of the hole's
    // clone (`t(&label)`), a `Cow` and an `Option<&String>` were refused.
    fn t(s: &str) -> &str {
        s
    }
    fn app() -> View {
        let label = String::from("Bo");
        let n = Signal::new(1u32);
        rsx! {
            p { "data-t": "p", title: t(&label), "data-cow": std::borrow::Cow::<str>::Borrowed(t(&label)),
                "data-opt": Some(&label), "data-n": n.get(), "data-on": n.get() > 1,
            }
            button { "data-t": "more", onclick: move |_| n += 1, "More" }
        }
    }
    let mut p = TestPage::mount(app);
    p.flush();
    assert_eq!(p.attr("p", "title").as_deref(), Some("Bo"));
    assert_eq!(p.attr("p", "data-cow").as_deref(), Some("Bo"));
    assert_eq!(p.attr("p", "data-opt").as_deref(), Some("Bo"));
    assert_eq!(p.attr("p", "data-n").as_deref(), Some("1"));
    assert_eq!(p.attr("p", "data-on").as_deref(), Some("false"));
    p.click("more");
    assert_eq!(p.attr("p", "data-n").as_deref(), Some("2"));
    assert_eq!(p.attr("p", "data-on").as_deref(), Some("true"));
}

#[test]
fn a_hole_may_borrow_a_temporary_it_makes() {
    // `t(&name.get())` borrows the `String` `get` returns: E0716
    // "temporary value dropped while borrowed", as a child and an attribute.
    fn t(s: &str) -> &str {
        s
    }
    fn app() -> View {
        let name = Signal::new(String::from("Ada"));
        rsx! {
            p { "data-t": "p", "aria-label": t(&name.get()), {t(&name.get())} }
            button { "data-t": "rename", onclick: move |_| name.set("Cy".into()), "Rename" }
        }
    }
    let mut p = TestPage::mount(app);
    p.flush();
    assert_eq!((p.text("p"), p.attr("p", "aria-label")), ("Ada".into(), Some("Ada".into())));
    p.click("rename");
    assert_eq!((p.text("p"), p.attr("p", "aria-label")), ("Cy".into(), Some("Cy".into())));
}

#[test]
fn if_let_shows_what_its_pattern_binds() {
    // `if let` in rsx! was refused (`{opt.map(|x| rsx! { .. })}` instead).
    fn app() -> View {
        let note = Signal::new(None::<String>);
        let n = Signal::new(0u32);
        rsx! {
            if let Some(text) = note.get() {
                p { "data-t": "note", "{text} ({n})" }
            } else if let 0 = n.get() {
                p { "data-t": "none", "none" }
            } else {
                p { "data-t": "none", "none yet" }
            }
            button { "data-t": "write", onclick: move |_| note.set(Some(format!("note {}", n.get()))), "Write" }
            button { "data-t": "more", onclick: move |_| n += 1, "More" }
            button { "data-t": "clear", onclick: move |_| note.set(None), "Clear" }
        }
    }
    let mut p = TestPage::mount(app);
    assert_eq!(p.text("none"), "none");
    p.click("more");
    assert_eq!(p.text("none"), "none yet");
    p.click("write");
    assert_eq!(p.text("note"), "note 1 (1)");
    assert!(!p.present("none"));
    p.click("more");
    assert_eq!(p.text("note"), "note 1 (2)", "the branch's own holes follow");
    p.click("write");
    assert_eq!(p.text("note"), "note 2 (2)", "a new binding: the branch made again");
    let live = air::live();
    p.click("clear");
    p.click("write");
    assert_eq!(air::live(), live, "the old branch ended");
    p.click("clear");
    assert_eq!(p.text("none"), "none yet");
}

#[test]
fn a_match_keeps_its_arm_until_another_matches() {
    // `match` in rsx! was refused (a `{match ..}` hole instead, which made
    // its view again on every change).
    #[derive(Clone, Copy, PartialEq)]
    enum Page {
        Home,
        Item(u32),
        Settings,
    }
    #[component]
    fn Counter(name: String) -> View {
        let n = signal(|| 0u32);
        rsx! { button { "data-t": "{name}", onclick: move |_| n += 1, "{name} {n}" } }
    }
    fn app() -> View {
        let page = Signal::new(Page::Home);
        let size = Signal::new(1u32);
        rsx! {
            match page.get() {
                Page::Home => { Counter { name: "home" } }
                Page::Item(id) if id > 9 => p { "data-t": "big", "item {id}" },
                Page::Item(id) => p { "data-t": "item", "item {id}" },
                Page::Settings => match size.get() {
                    1..=3 => { Counter { name: "small" } }
                    _ => { p { "data-t": "large", "large" } }
                }
            }
            button { "data-t": "item", onclick: move |_| page.set(Page::Item(3)), "3" }
            button { "data-t": "item12", onclick: move |_| page.set(Page::Item(12)), "12" }
            button { "data-t": "settings", onclick: move |_| page.set(Page::Settings), "S" }
            button { "data-t": "grow", onclick: move |_| size += 1, "+" }
        }
    }
    let mut p = TestPage::mount(app);
    p.click("home");
    assert_eq!(p.text("home"), "home 1");
    p.click("item");
    assert_eq!(p.text("item"), "item 3");
    p.click("item12");
    assert_eq!(p.text("big"), "item 12", "a guard picks the arm");
    p.click("settings");
    p.click("small");
    p.click("grow");
    assert_eq!(p.text("small"), "small 1", "the same arm: its view and state kept");
    p.click("grow");
    p.click("grow");
    assert_eq!(p.text("large"), "large");
    let live = air::live();
    p.click("item");
    p.click("settings");
    assert_eq!(air::live(), live, "the arms left ended");
}

/// A tab, matched by its whole path in a view.
#[derive(Clone, Copy, PartialEq)]
pub enum Tab {
    A,
    B,
}

#[test]
fn an_arm_matched_by_a_path_keeps_its_state() {
    // `crate::Tab::A` binds nothing: its arm is kept while it matches.
    #[component]
    fn Counter() -> View {
        let n = signal(|| 0u32);
        rsx! { button { "data-t": "count", onclick: move |_| n += 1, "{n}" } }
    }
    fn app() -> View {
        let (tab, size) = (Signal::new(Tab::A), Signal::new(1u32));
        rsx! {
            match (tab.get(), size.get() > 0) {
                (crate::Tab::A, true) => { Counter {} }
                (self::Tab::B, _) | (crate::Tab::A, false) => p { "data-t": "b", "b" },
            }
            button { "data-t": "grow", onclick: move |_| size += 1, "+" }
        }
    }
    let mut p = TestPage::mount(app);
    p.click("count");
    p.click("grow");
    assert_eq!(p.text("count"), "1", "the same arm: its state kept");
}

/// What a row of a list holds, counted when dropped.
struct Held(std::rc::Rc<std::cell::Cell<u32>>);

impl Drop for Held {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

#[test]
fn rows_that_leave_a_list_stop_at_once_and_are_dropped_once_the_frame_is_out() {
    thread_local!(static DROPPED: std::rc::Rc<std::cell::Cell<u32>> = Default::default());
    thread_local!(static FROM: std::cell::Cell<Option<Signal<u32>>> = const { std::cell::Cell::new(None) });
    let dropped = DROPPED.with(|d| d.clone());
    fn app() -> View {
        let from = Signal::new(0u32);
        FROM.set(Some(from));
        let tick = Signal::new(0u32);
        rsx! {
            div { "data-t": "keys", tabindex: "0",
                onkeydown: move |e: KeyboardEvent| match e.key() {
                    Key::End => from.set(100),
                    _ => tick += 1,
                },
                ul { "data-t": "list",
                    for i in from.get()..from.get() + 3 {
                        li { key: i,
                            {
                                let held = DROPPED.with(|d| Held(d.clone()));
                                air::text_view(move || {
                                    let _held = &held;
                                    format!("{i}:{}", tick.get())
                                })
                            }
                        }
                    }
                }
            }
        }
    }
    let mut p = TestPage::mount(app);
    assert_eq!(p.texts("list"), ["0:0", "1:0", "2:0"]);
    let end = p.aim("keys", Raw { name: "keydown".into(), key: "End".into(), ..Raw::default() });
    assert!(p.root().event(&end.encode()));
    assert_eq!(dropped.get(), 0, "kept until the frame is out");
    p.flush();
    assert_eq!(p.texts("list"), ["100:0", "101:0", "102:0"]);
    assert_eq!(dropped.get(), 3, "the three rows that left, once the frame is out");
    p.key("keys", "a");
    assert_eq!(p.texts("list"), ["100:1", "101:1", "102:1"], "the rows that left run no more");
    // Rows that leave with no event (a task, a change from outside): dropped
    // at the flush, not kept until some event comes.
    p.within(|| FROM.get().unwrap().set(200));
    assert_eq!(p.texts("list"), ["200:1", "201:1", "202:1"]);
    assert_eq!(dropped.get(), 6, "no event needed");
}

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

/// `components()` on air: each component by name and key, its body runs
/// and its holes' updates. A view reading one field of a cell (`part`) is
/// updated for that field only (aio's `trackCell`); no body runs again.
mod tree {
    use super::*;
    use aio::testing::MountedComponent;

    #[aio::cell]
    pub struct Board {
        pub rows: Vec<String>,
        pub cursor: u32,
    }

    #[aio::methods]
    impl Board {
        pub fn down(&mut self) {
            self.cursor += 1;
        }

        pub fn add(&mut self, row: String) {
            self.rows.push(row);
        }

        pub fn same(&mut self) {}
    }

    aio::client!(Board, board);

    #[component]
    fn Cursor() -> View {
        let at = part(|b| &b.cursor);
        rsx! { span { "data-t": "cursor", "{at}" } }
    }

    #[component]
    fn Row(name: String) -> View {
        rsx! { li { "{name}" } }
    }

    #[component]
    fn Rows() -> View {
        let rows = part(|b| &b.rows);
        rsx! { ul { "data-t": "rows", for r in rows.get() { Row { key: r.clone(), name: r } } } }
    }

    fn app() -> View {
        rsx! { Cursor {} Rows {} }
    }

    fn get<'a>(c: &'a [MountedComponent], name: &str) -> &'a MountedComponent {
        c.iter().find(|c| c.name == name).unwrap_or_else(|| panic!("no {name}: {c:?}"))
    }

    #[test]
    fn components_on_air_count_body_runs_and_the_updates_a_change_cost() {
        let mut ui = TestUi::new(app);
        let tree = ui.components();
        assert_eq!(tree.iter().map(|c| c.name).collect::<Vec<_>>(), ["Cursor", "Rows"]);
        for _ in 0..5 {
            ui.run(|| act!(down()));
        }
        assert_eq!(ui.text("cursor"), "5");
        let tree = ui.components();
        let (c, r) = (get(&tree, "Cursor"), get(&tree, "Rows"));
        assert_eq!((c.renders, c.updates), (1, 5), "its text, once a step");
        assert_eq!((r.renders, r.updates), (1, 0), "the rows' reader stayed");
        ui.run(|| act!(add("one".into())));
        ui.run(|| act!(add("two".into())));
        let tree = ui.components();
        let r = get(&tree, "Rows");
        assert_eq!(r.updates, 2, "its list, once a commit");
        let keys: Vec<_> = r.children.iter().map(|c| (c.name, c.key.as_deref(), c.renders)).collect();
        assert_eq!(keys, [("Row", Some("one"), 1), ("Row", Some("two"), 1)], "row one kept, not made again");
        assert_eq!(get(&tree, "Cursor").updates, 5, "the cursor's reader stayed");
        ui.run(|| act!(same()));
        assert_eq!(ui.components(), tree, "a call that changes nothing updates nothing");
    }
}

#[test]
fn surface_and_shot_on_air_read_the_page_a_person_sees() {
    fn app() -> View {
        rsx! {
            main { style: "background: rgb(0, 128, 0); width: 100vw; height: 100vh",
                h1 { "Hello" }
                button { "data-t": "go", "Go" }
            }
        }
    }
    let ui = aio::testing::Mount::default().size(200, 100).no_scrollbars().app(app);
    let s = ui.surface();
    assert!(s.contains("heading \"Hello\"") && s.contains("button \"Go\" [go]"), "{s}");
    let png = ui.shot();
    assert_eq!(&png[1..4], b"PNG");
    let dim = |at: usize| u32::from_be_bytes(png[at..at + 4].try_into().unwrap());
    assert_eq!((dim(16), dim(20)), (200, 100), "the window's size");
}

/// aio's `find(component, key)` and the surface as a tree of components:
/// rows that draw the same names are told apart by their component and key.
mod scopes {
    use super::*;

    #[component]
    fn Todo(id: u32, title: String, done: Signal<Vec<u32>>) -> View {
        rsx! {
            li {
                span { "{title}" }
                button { onclick: move |_| done.update(|d| d.push(id)), "Done" }
                input { r#type: "checkbox", checked: done.with(|d| d.contains(&id)) }
            }
        }
    }

    #[component]
    fn Empty() -> View {
        rsx! {}
    }

    fn app() -> View {
        let done = signal(Vec::<u32>::new);
        let todos = [(1u32, "milk"), (2, "bread")];
        rsx! {
            h1 { "Todos" }
            ul { for (id, title) in todos { Todo { key: id, id, title: title.to_string(), done } } }
            p { "data-t": "done", {format!("{:?}", done.get())} }
            Empty {}
        }
    }

    #[test]
    fn a_component_is_found_by_its_key_and_a_test_acts_inside_it() {
        let mut ui = TestUi::new(app);
        let bread = ui.find("Todo", Some("2"));
        assert_eq!(bread, "comp=Todo[2]");
        assert_eq!(ui.text(&bread), "breadDone");
        ui.within(&bread, |ui| ui.click("DoneButton"));
        assert_eq!(ui.text("done"), "[2]", "the bread row's button, not the first one");
        assert!(ui.checked("comp=Todo[2]/css=input") && !ui.checked("comp=Todo[1]/css=input"));
        assert!(ui.present("comp=Todo[1]"));
        assert!(ui.absent("comp=Empty"), "a component that drew nothing doesn't show");
        drop(ui); // one window's views per thread
        let e = fails(|| _ = TestUi::new(app).find("Todo", None));
        assert!(e.contains("2 components match comp=Todo"), "{e}");
        let e = fails(|| _ = TestUi::new(app).find("Todo", Some("9")));
        assert!(e.contains("the components are: Todo[1], Todo[2], Empty"), "{e}");
    }

    #[test]
    fn the_surface_tree_says_what_each_component_drew() {
        let ui = TestUi::new(app);
        let s = ui.surface_tree();
        assert_eq!((s.component.as_str(), s.path.as_str()), ("app", "app"));
        let own: Vec<_> = s.elements.iter().map(|e| (e.role.as_str(), e.name.as_str())).collect();
        assert_eq!(own, [("heading", "TodosHeading"), ("generic", "done")], "the app's own, not its rows'");
        let rows: Vec<_> = s.children.iter().map(|c| (c.path.as_str(), c.key.as_deref(), c.text.as_str())).collect();
        assert_eq!(
            rows,
            [("app/Todo[1]", Some("1"), "milkDone"), ("app/Todo[2]", Some("2"), "breadDone"), ("app/Empty", None, "")]
        );
        let row = &s.children[1].elements;
        assert_eq!(row.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["DoneButton2", "Checkbox2"]);
        assert_eq!((row[1].checked, row[0].disabled), (Some(false), false));
    }
}

#[test]
fn advance_moves_the_views_clock_and_fires_their_timers_in_order() {
    fn app() -> View {
        let log = signal(Vec::<String>::new);
        task(move || async move {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            log.update(|l| l.push("10 s".into()));
        });
        task(move || async move {
            for i in 1..=3 {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                log.update(|l| l.push(format!("tick {i}")));
            }
        });
        rsx! { p { "data-t": "log", {log.get().join(", ")} } }
    }
    let mut ui = TestUi::new(app);
    let t = std::time::Instant::now();
    ui.advance(std::time::Duration::from_secs(5));
    assert_eq!(ui.text("log"), "tick 1");
    ui.advance(std::time::Duration::from_secs(5));
    assert_eq!(ui.text("log"), "tick 1, tick 2, tick 3, 10 s", "in their order");
    assert!(t.elapsed() < std::time::Duration::from_secs(5), "virtual: {:?}", t.elapsed());
}

#[test]
fn a11y_says_what_fails_a_keyboard_or_a_screen_reader() {
    fn app() -> View {
        rsx! {
            div { "data-t": "card", onclick: move |_| {}, "Open" }
            div { "data-t": "row", role: "option", onclick: move |_| {}, "Item" }
            button { r#type: "button", onclick: move |_| {}, "Fine" }
            button { "data-t": "bare", "Submit?" }
            input { "data-t": "who" }
            label { "Name" input { r#type: "text" } }
            img { "data-t": "pic", src: "data:," }
            a { "data-t": "fake", onclick: move |_| {}, "Go" }
            span { "data-t": "skip", tabindex: "3", "x" }
            button { r#type: "button", "data-t": "ghost", "aria-hidden": "true", "Hidden" }
        }
    }
    let ui = TestUi::new(app);
    let got = ui.a11y();
    let has = |t: &str, why: &str| got.iter().any(|f| f.starts_with(&format!("<{t}\">: ")) && f.contains(why));
    for (t, why) in [
        ("div data-t=\"card", "no keyboard one"),
        ("button data-t=\"bare", "no type"),
        ("input data-t=\"who", "no label"),
        ("img data-t=\"pic", "missing \"alt\""),
        ("a data-t=\"fake", "no href"),
        ("span data-t=\"skip", "tabindex 3"),
        ("button data-t=\"ghost", "aria-hidden but focusable"),
    ] {
        assert!(has(t, why), "{t}: {why} in {got:#?}");
    }
    assert_eq!(got.len(), 7, "an option's keyboard is its list's; a labelled field, a typed button pass: {got:#?}");
}

#[test]
fn a_saved_stylesheet_redraws_its_hole_on_air() {
    // `use_css!` in a hole (an effect: no hooks there), as the template has it.
    static SHEETS: std::sync::OnceLock<&'static [aio::ui::Sheet]> = std::sync::OnceLock::new();
    let dir = std::env::temp_dir().join(format!("aio-air-css-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("style.css");
    std::fs::write(&file, "h1{color:red}").unwrap();
    let path: &'static str = Box::leak(file.to_string_lossy().into_owned().into_boxed_str());
    SHEETS.set(Box::leak(Box::new([(path, "")]))).unwrap();
    fn app() -> View {
        let sheets = *SHEETS.get().unwrap();
        rsx! {
            p { "data-t": "css", {{ aio::ui::follow_sheets(sheets); aio::ui::sheet_texts(sheets).concat() }} }
        }
    }
    let mut ui = TestUi::new(app);
    assert_eq!(ui.text("css"), "h1{color:red}");
    std::fs::write(&file, "h1{color:blue}").unwrap();
    ui.until_text("css", "h1{color:blue}");
    std::fs::remove_dir_all(&dir).unwrap();
    // The template's own line, in its hole.
    fn template() -> View {
        let theme = aio::kit::theme_css(aio::hue!());
        rsx! { style { "data-t": "page", {aio::use_css!(aio::ui::scheme(), &theme; "assets/kit.css")} } }
    }
    assert!(TestUi::new(template).text("page").contains("box-sizing"));
}

#[test]
fn svg_made_by_a_component_a_row_or_a_branch_is_svgs() {
    const DOT: &str = "M0,0h1";
    #[component]
    fn Dot(r: u32) -> View {
        rsx! { circle { r: "{r}" } }
    }
    fn app() -> View {
        let n = signal(|| 2u32);
        rsx! {
            svg { "data-t": "pic", view_box: "0 0 9 9",
                Dot { r: 1 }
                for i in 0..n.get() { line { key: i, x1: "{i}" } }
                if n.get() > 1 { title { "two" } }
                // A constant alone as a value is read where it is.
                path { d: DOT }
            }
            p { "data-t": "html", title { "page" } }
        }
    }
    let mut ui = TestUi::new(app);
    let ns = ui.eval("[...document.querySelectorAll('[data-t=pic] *, [data-t=html] *')].map(e => e.localName + ':' + e.namespaceURI.split('/').pop()).join(' ')");
    assert_eq!(ns, "circle:svg line:svg line:svg title:svg path:svg title:xhtml");
}

#[test]
fn style_attributes_set_css_properties() {
    fn app() -> View {
        let n = signal(|| 2u32);
        let wide = signal(|| true);
        rsx! {
            div { "data-t": "box", style: "color: red", display: "flex", gap: "{n}px", min_width: wide.get().then_some("10px"),
                button { "data-t": "narrow", onclick: move |_| wide.set(false), "narrow" }
            }
            img { "data-t": "img", width: "3" }
            svg { "data-t": "svg", width: "4" }
            span { "data-t": "quoted", "display": "x" }
        }
    }
    let mut ui = TestUi::new(app);
    let read = "[...document.querySelectorAll('[data-t=box], [data-t=img], [data-t=svg], [data-t=quoted]')].map(e => (e.getAttribute('style') || '') + '|' + (e.getAttribute('width') || '') + '|' + (e.getAttribute('display') || '')).join(' / ')";
    assert_eq!(ui.eval(read), "color: red; display: flex; gap: 2px; min-width: 10px;|| / |3| / |4| / ||x");
    ui.click("narrow");
    assert_eq!(
        ui.eval("document.querySelector('[data-t=box]').getAttribute('style')"),
        "color: red; display: flex; gap: 2px;"
    );
}

use super::*;

/// A project in a temp dir: `(path, contents)` pairs.
fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    for (p, text) in files {
        let p = d.path().join(p);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    d
}

/// `(area, file:line)` of every finding.
fn found(r: &Report) -> Vec<(&'static str, String)> {
    r.issues.iter().map(|i| (i.area, format!("{}:{}", i.file.as_deref().unwrap_or(""), i.line.unwrap_or(0)))).collect()
}

#[test]
fn a_credential_named_cell_field_every_client_gets_is_an_error() {
    let d = project(&[(
        "src/cell.rs",
        r#"
#[aio::cell(persist)]
pub struct Vault {
    pub label: String,
    pub api_key: String,
    #[server]
    pub password: String,
    pub public_key: String,
    /// aio-ok(security): a demo, the words are on the box
    pub mnemonic: String,
}

#[cell(visible = only_mine)]
pub struct Filtered {
    pub private_key: String,
}
"#,
    )]);
    let r = lint(d.path());
    let sec: Vec<_> = r.issues.iter().filter(|i| i.area == "security").collect();
    assert_eq!(sec.len(), 1, "{:?}", r.issues);
    assert_eq!((sec[0].severity, sec[0].line), (Severity::Error, Some(5)));
    assert!(sec[0].message.contains("`Vault` field `api_key`"), "{}", sec[0].message);
    assert!(sec[0].fix.as_deref().unwrap().contains("#[server]"));
}

#[test]
fn methods_that_reach_io_the_clock_randomness_or_threads_are_errors() {
    let d = project(&[(
        "src/cell.rs",
        r#"
#[methods]
impl Notes {
    pub fn add(&mut self, cx: &Cx, t: String) -> Result {
        let at = std::time::SystemTime::now();
        let x = std::fs::read_to_string("a");
        let n = rand::random::<u64>();
        std::thread::spawn(|| {});
        let ok = cx.now();
        cx.run(Effect::new("save", move |h| async move { std::fs::write("a", "b").unwrap(); }));
        let later = || Instant::now();
        // aio-ok(cells): read once at boot, never replayed
        let v = std::env::var("X");
        Ok(())
    }
}

impl Plain {
    fn io(&self) { std::fs::read("x"); }
}
"#,
    )]);
    let r = lint(d.path());
    let cells: Vec<_> =
        r.issues.iter().filter(|i| i.area == "cells").map(|i| (i.line.unwrap(), i.message.clone())).collect();
    assert_eq!(cells.iter().map(|c| c.0).collect::<Vec<_>>(), [5, 6, 7, 8], "{cells:?}");
    assert!(cells[0].1.contains("`Notes::add` uses the clock (`std::time::SystemTime::now`)"), "{}", cells[0].1);
    assert!(r.issues.iter().filter(|i| i.area == "cells").all(|i| i.severity == Severity::Error));
    assert!(r.issues[0].fix.as_deref().unwrap().contains("cx.now()"));
}

#[test]
fn impure_paths() {
    let p = |s: &str| rust::impure(&s.split("::").map(String::from).collect::<Vec<_>>());
    assert_eq!(p("Utc::now"), Some("the clock"));
    assert_eq!(p("chrono::Local::now"), Some("the clock"));
    assert_eq!(p("Uuid::new_v4"), Some("randomness"));
    assert_eq!(p("aio::uuid_v7"), Some("randomness"));
    assert_eq!(p("tokio::spawn"), Some("a task"));
    assert_eq!(p("fs::write"), Some("file I/O"));
    assert_eq!(p("File::open"), Some("file I/O"));
    assert_eq!(p("Command::new"), Some("network or process I/O"));
    assert_eq!(p("ureq::get"), Some("network or process I/O"));
    assert_eq!(p("std::io::stdin"), Some("terminal I/O"));
    for pure in ["Vec::new", "fs", "now", "self::now", "chrono::NaiveDate::from_ymd_opt", "String::from"] {
        assert_eq!(p(pure), None, "{pure}");
    }
}

#[test]
fn hooks_under_a_branch_a_loop_or_a_closure_are_errors() {
    let d = project(&[(
        "src/ui.rs",
        r#"
fn App() -> Element {
    let a = use_signal(|| 0);
    if let Some(c) = use_context::<C>() {}
    if a() > 1 {
        let b = use_signal(|| 1);
    }
    for i in 0..3 { use_memo(move || i); }
    let f = move |_| { let s = use_signal(|| 2); };
    let ok = a() > 0 && use_hook(|| true);
    match a() { 0 => {}, _ => { use_effect(|| {}); } }
    rsx! {}
}
fn use_thing() -> Signal<i32> {
    if true { use_signal(|| 0) } else { use_signal(|| 1) }
}
fn helper() { if x { use_signal(|| 0); } }
fn Early(id: Option<u32>) -> Element {
    let a = use_signal(|| 0);
    let f = move || -> Option<u32> { let v = id?; return Some(v); };
    let Some(id) = id else { return rsx! {} };
    let b = use_signal(|| 1);
    rsx! {}
}
fn Tried() -> Element {
    let n = parse()?;
    let fresh = Signal::new(0);
    let kept = use_hook(|| Signal::new(1));
    use_memo(move || n);
    rsx! {}
}
"#,
    )]);
    let r = lint(d.path());
    let lines: Vec<_> = r.issues.iter().filter(|i| i.area == "hooks").map(|i| i.line.unwrap()).collect();
    assert_eq!(lines, [6, 8, 9, 10, 11, 15, 15, 22, 27, 28, 29], "{:?}", r.issues);
    assert!(r.issues.iter().any(|i| i.message.starts_with("`Signal::new` in the body of `Tried` makes a new signal")));
    assert!(r.issues.iter().any(|i| i.message.starts_with("`use_signal` in `App` runs only sometimes")));
}

#[test]
fn rsx_without_accessible_names_is_an_error() {
    let d = project(&[(
        "src/ui.rs",
        r#"
fn App() -> Element {
    rsx! {
        img { src: "a.png" }
        img { src: "a.png", alt: "" }
        button { onclick: move |_| {} }
        button { "Save" }
        button { aria_label: "Close", "✕" }
        button { {icon()} }
        button { span { class: "icon" } }
        input { r#type: "text" }
        input { }
        input { r#type: "hidden" }
        input { r#type: "{kind}" }
        input { placeholder: "Name" }
        input { "aria-label": "Name" }
        label { "Name" input { } }
        textarea { }
        select { option { "a" } }
        if x { img { src: "b" } } else { img { src: "c", alt: "c" } }
        for i in 0..2 { Card { img { src: "d" } } }
        div { {rsx! { button { } }} }
        img { src: "e", aria_hidden: "true" }
        input { r#type: "image", src: "go.png" }
        input { r#type: "submit" }
        // aio-ok(a11y): a canvas island draws its own label
        button { class: "x" }
    }
}
"#,
    )]);
    let r = lint(d.path());
    let lines: Vec<_> = r.issues.iter().filter(|i| i.area == "a11y").map(|i| i.line.unwrap()).collect();
    assert_eq!(lines, [4, 6, 10, 11, 12, 18, 19, 20, 21, 22, 24], "{:?}", r.issues);
    let msg = |l: usize| r.issues.iter().find(|i| i.line == Some(l)).unwrap().message.clone();
    assert_eq!(msg(4), "<img> has no alt text: screen readers and agents can't name it");
    assert_eq!(msg(6), "<button> has no accessible name: screen readers and agents can't name it");
    assert_eq!(msg(11), "<input> has no label: screen readers and agents can't name it");
}

#[test]
fn a_click_without_keys_is_a_warning() {
    let d = project(&[(
        "src/ui.rs",
        r#"
fn App() -> Element {
    rsx! {
        div { onclick: move |_| {}, "Open" }
        li { role: "option", onclick: pick, "A" }
        span { tabindex: 0, onclick: pick, onkeydown: key, "B" }
        button { onclick: pick, "C" }
        a { href: "/d", onclick: pick, "D" }
    }
}
"#,
    )]);
    let r = lint(d.path());
    assert_eq!(found(&r), [("a11y", "src/ui.rs:4".into())], "{:?}", r.issues);
    assert_eq!(r.issues[0].severity, Severity::Warn, "a backdrop's click has Esc elsewhere: advice, not a failure");
    assert!(r.issues[0].message.starts_with("<div> takes clicks but no keys"));
}

#[test]
fn markup_that_does_what_its_author_did_not_mean_is_a_warning() {
    // aio tests/dev-a11y*.test.ts.
    let d = project(&[(
        "src/ui.rs",
        r#"
fn App() -> Element {
    rsx! {
        form { button { "Save" } button { r#type: "button", "Cancel" } }
        button { "Outside a form" }
        a { onclick: go, "Go" }
        a { href: "/x", onclick: go, "X" }
        div { tabindex: 2, role: "button", onkeydown: key, onclick: go, "A" }
        div { tabindex: "0", onkeydown: key, "B" }
        button { aria_hidden: "true", "C" }
        div { aria_hidden: "true", "D" }
        span { aria_disabled: "true", role: "tab", onclick: go, "E" }
        button { aria_disabled: "true", disabled: true, onclick: go, "F" }
        div { role: "button", onclick: go, "G" }
        ul { role: "listbox", aria_activedescendant: "o1", onclick: go, "H" }
        div { role: "{r}", onclick: go, "I" }
    }
}
"#,
    )]);
    let r = lint(d.path());
    let got: Vec<(usize, &str)> =
        r.issues.iter().filter(|i| i.area == "a11y").map(|i| (i.line.unwrap(), i.message.as_str())).collect();
    assert_eq!(
        got,
        [
            (4, "<button> in a form without a type submits it"),
            (6, "<a> with onclick and no href can't be focused or announced"),
            (8, "tabindex 2 reorders the whole page's tab sequence"),
            (10, "<button> is aria-hidden but focusable: the keyboard lands on what a reader hides"),
            (12, "<span> is aria-disabled but its onclick still runs"),
            (14, "<div> takes clicks but no keys: keyboard users and agents can't use it"),
        ],
        "{:?}",
        r.issues
    );
    assert!(r.issues.iter().all(|i| i.severity == Severity::Warn));
}

#[test]
fn a_list_item_without_a_key_is_an_error() {
    let d = project(&[(
        "src/ui.rs",
        r#"
fn App() -> Element {
    rsx! {
        ul {
            for t in todos() { li { "{t.title}" } }
            for t in todos() { li { key: "{t.id}", "{t.title}" } }
            for t in todos() { Row { t } }
            for t in todos() { Row { key: "{t.id}", t } }
        }
    }
}
"#,
    )]);
    let r = lint(d.path());
    let got: Vec<_> =
        r.issues.iter().filter(|i| i.area == "render").map(|i| (i.line.unwrap(), i.message.as_str())).collect();
    assert_eq!(
        got,
        [
            (
                5,
                "<li> in a for loop has no key: a removed or moved row hands its state to the next (air, aio::ui's next renderer, requires one)"
            ),
            (
                7,
                "<Row> in a for loop has no key: a removed or moved row hands its state to the next (air, aio::ui's next renderer, requires one)"
            ),
        ],
        "{:?}",
        r.issues
    );
    assert!(r.issues.iter().filter(|i| i.area == "render").all(|i| i.severity == Severity::Error));
}

#[test]
fn aio_ui_hooks_off_the_top_level_are_errors_too() {
    let d = project(&[(
        "src/ui.rs",
        r#"
fn App() -> View {
    let a = signal(|| 0);
    if a.get() > 1 {
        let b = signal(|| 1);
        effect(move || {});
    }
    let f = move |_| { let m = memo(|| 2); };
    let kept = once(|| Signal::new(1));
    let fresh = Signal::new(0);
    for i in 0..3 { watch(i, |_| {}); }
    rsx! {}
}
"#,
    )]);
    let r = lint(d.path());
    let lines: Vec<_> = r.issues.iter().filter(|i| i.area == "hooks").map(|i| i.line.unwrap()).collect();
    assert_eq!(lines, [5, 6, 8, 10, 11], "{:?}", r.issues);
    assert!(r.issues.iter().any(|i| i.message.starts_with("`signal` in `App` runs only sometimes")));
    assert!(r.issues.iter().any(|i| i.message.starts_with("`Signal::new` in the body of `App` makes a new signal")));
}

#[test]
fn an_if_let_in_rsx_is_a_warning() {
    let d = project(&[(
        "src/ui.rs",
        r#"
fn App() -> View {
    rsx! {
        if let Some(n) = pick.get() { p { "{n}" } }
        if pick.get().is_some() { p { "some" } } else if let Some(m) = other() { p { "{m}" } }
        {pick.get().map(|n| rsx! { p { "{n}" } })}
    }
}
"#,
    )]);
    let r = lint(d.path());
    let got: Vec<_> =
        r.issues.iter().filter(|i| i.message.contains("`if let`")).map(|i| (i.severity, i.line.unwrap())).collect();
    assert_eq!(got, [(Severity::Warn, 4), (Severity::Warn, 5)], "{:?}", r.issues);
}

#[test]
fn naming_the_renderer_behind_aio_ui_is_an_error() {
    let d = project(&[
        (
            "Cargo.toml",
            "[package]\nname = \"x\"\n\n[dependencies]\naio = { path = \"a\" }\ndioxus = \"0.7\"\n# dioxus = \"0.7\"\ndioxus-document = \"0.7\"\n",
        ),
        (
            "src/ui.rs",
            r#"
use dioxus::prelude::*;
use aio::ui::prelude::*;
fn App() -> View {
    let t = dioxus_document::eval("dioxus.send(1)");
    // dioxus in a comment is prose
    let dioxus = 1;
    rsx! {}
}
"#,
        ),
        ("tests/ui.rs", "use dioxus::prelude::Key;\n"),
    ]);
    let r = lint(d.path());
    let got: Vec<_> =
        r.issues.iter().filter(|i| i.area == "renderer").map(|i| (i.file.clone().unwrap(), i.line.unwrap())).collect();
    let at = |f: &str, l: usize| (f.to_string(), l);
    let (path, script) = (at("src/ui.rs", 5), at("src/ui.rs", 5));
    assert_eq!(
        got,
        [at("Cargo.toml", 6), at("Cargo.toml", 8), at("src/ui.rs", 2), path, script, at("tests/ui.rs", 1)],
        "{:?}",
        r.issues
    );
    assert!(r.issues.iter().all(|i| i.area != "renderer" || i.severity == Severity::Error));
    assert!(r.issues.iter().any(|i| i.message.contains("its page object is `aio`")), "a page script's dioxus.send");
}

#[test]
fn a_for_loop_over_a_block_is_a_warning() {
    let d = project(&[(
        "src/ui.rs",
        r#"
fn App() -> Element {
    rsx! {
        ul {
            for t in todos() { { let n = t.n; rsx! { li { key: "{t.id}", "{n}" } } } }
            {todos().into_iter().map(|t| rsx! { li { key: "{t.id}", "{t.n}" } })}
        }
    }
}
"#,
    )]);
    let r = lint(d.path());
    let got: Vec<_> = r.issues.iter().filter(|i| i.area == "render").map(|i| i.line.unwrap()).collect();
    assert_eq!(got, [5], "{:?}", r.issues);
    assert!(r.issues[0].message.contains("keys nothing"), "{:?}", r.issues);
}

#[test]
fn a_key_dioxus_ignores_is_a_warning() {
    let d = project(&[(
        "src/ui.rs",
        r#"
fn App() -> Element {
    rsx! {
        for g in groups() {
            if g.first { h2 { "{g.name}" } }
            Row { key: "{g.id}", g }
        }
        for g in groups() {
            Fragment { key: "{g.id}", if g.first { h2 { "{g.name}" } } Row { g } }
        }
        if open() { Dialog { key: "{open_id()}" } }
        for k in [open_id()] { Dialog { key: "{k}" } }
        ul { li { key: "{x}" } }
        {items().map(|t| rsx! { li { key: "{t.id}", "{t.name}" } })}
    }
}

fn Avatar(src: String) -> Element {
    let rows: Vec<_> = items().iter().map(|t| rsx! { li { key: "{t.id}" } }).collect();
    rsx! { img { key: "{src}", src: "{src}", alt: "" } }
}
"#,
    )]);
    let r = lint(d.path());
    let got: Vec<_> =
        r.issues.iter().filter(|i| i.area == "render").map(|i| (i.line.unwrap(), i.message.as_str())).collect();
    assert_eq!(
        got,
        [
            (6, "key on <Row>, not its block's first node: aio::ui ignores it (the first node keys the block)"),
            (11, "key on <Dialog> in no list: aio::ui never remounts it when the key changes"),
            (13, "key on <li> in no list: aio::ui never remounts it when the key changes"),
            (20, "key on <img> in no list: aio::ui never remounts it when the key changes"),
        ],
        "{:?}",
        r.issues
    );
}

#[test]
fn drag_data_set_in_a_views_dragstart_is_a_warning() {
    let d = project(&[(
        "src/ui.rs",
        r#"
fn App() -> Element {
    rsx! {
        div { draggable: true, ondragstart: move |e| { _ = e.data_transfer().set_data("text/plain", "a"); }, "A" }
        div { draggable: true, "data-drag": "b", ondragstart: move |_| log("b"), "B" }
    }
}
"#,
    )]);
    let r = lint(d.path());
    let got: Vec<_> =
        r.issues.iter().filter(|i| i.area == "render").map(|i| (i.line.unwrap(), i.fix.clone())).collect();
    assert_eq!(got.len(), 1, "{:?}", r.issues);
    assert_eq!(got[0].0, 4);
    assert!(got[0].1.as_deref().is_some_and(|f| f.contains("data-drag")), "{got:?}");
}

#[test]
fn a_signal_read_kept_through_a_block_that_writes_is_a_warning() {
    let d = project(&[(
        "src/ui.rs",
        r#"
fn on_key(k: Key) {
    if k == Key::Escape {
    } else if let Some(id) = TOAST.peek().as_ref().map(|t| t.id) {
        dismiss(id);
    }
    while let Some(n) = queue.write().pop() { log(n) }
    match MODE.read().kind { Kind::A => *MODE.write() = Mode::B, _ => {} }
    let id = TOAST.peek().as_ref().map(|t| t.id);
    if let Some(id) = id { dismiss(id) }
    if let Some(t) = TOAST.read().as_ref() { let _ = Some(t.id); }
    if let Some(t) = TOAST.cloned() { dismiss(t.id) }
}
"#,
    )]);
    let r = lint(d.path());
    let got: Vec<_> = r.issues.iter().filter(|i| i.area == "render").map(|i| (i.line.unwrap(), &i.message)).collect();
    assert_eq!(got.iter().map(|g| g.0).collect::<Vec<_>>(), [4, 7, 8], "{:?}", r.issues);
    assert!(got[0].1.contains("`TOAST.peek()` in this `if let`") && got[0].1.contains("`dismiss()`"), "{}", got[0].1);
    let fix = r.issues.iter().find(|i| i.area == "render").and_then(|i| i.fix.as_deref()).unwrap_or_default();
    assert!(fix.contains("take the value first"), "{fix}");
}

#[test]
fn credentials_in_source_and_assets() {
    let d = project(&[
        (
            "src/lib.rs",
            "pub const API_KEY: &str = \"0123456789abcdef\";\npub fn f() -> &'static str { \"AKIAIOSFODNN7EXAMPLE\" }\n#[cfg(test)]\nmod t { const PASSWORD: &str = \"hunter22\"; }\n",
        ),
        ("assets/config.json", "{\"token\": \"abcdefghij0123456789xyz\"}\n"),
        ("aio.toml", "[app]\nid = \"demo\"\nname = \"Demo\"\n"),
        ("tests/fixture.json", "{\"token\": \"abcdefghij0123456789xyz\"}\n"),
        ("tests/it.rs", "const PASSWORD: &str = \"hunter22\";\n#[test]\nfn t() { assert!(!PASSWORD.is_empty()) }\n"),
        ("assets/logo.png", "\u{0}\u{159}PNG"),
    ]);
    let r = lint(d.path());
    assert_eq!(
        found(&r),
        [
            ("security", "assets/config.json:1".into()),
            ("security", "src/lib.rs:1".into()),
            ("security", "src/lib.rs:2".into())
        ],
        "{:?}",
        r.issues
    );
    assert_eq!(r.issues[2].severity, Severity::Error, "a known key format is no guess");
    assert!(r.passed.contains(&"aio.toml: app demo".to_string()), "{:?}", r.passed);
    assert_eq!(r.tests, 1);
}

#[test]
fn a_bad_aio_toml_and_a_committed_env_file() {
    let d = project(&[("aio.toml", "[app]\nname = \"No id\"\n"), (".env", "TOKEN=x\n"), (".gitignore", "target\n")]);
    let r = lint(d.path());
    let areas: Vec<_> = r.issues.iter().map(|i| (i.area, i.severity)).collect();
    assert!(areas.contains(&("config", Severity::Error)), "{:?}", r.issues);
    assert!(areas.contains(&("security", Severity::Warn)), "{:?}", r.issues);
    std::fs::write(d.path().join(".gitignore"), "target\n.env\n").unwrap();
    assert!(!lint(d.path()).issues.iter().any(|i| i.area == "security"));
}

#[test]
fn duplicate_cell_ids_and_untested_cells() {
    let d = project(&[
        ("src/a.rs", "#[cell]\npub struct TodoList { pub n: u32 }\n"),
        (
            "src/b.rs",
            "#[aio::cell(id = \"todo_list\")]\npub struct Other { pub n: u32 }\n#[cell(persist)]\npub struct Lone { pub n: u32 }\n",
        ),
        ("tests/cells.rs", "#[test]\nfn t() { let h = Handle::<TodoList>::open(p); let o = Other::default(); }\n"),
    ]);
    let r = lint(d.path());
    let dup = r.issues.iter().find(|i| i.area == "cells").unwrap();
    assert_eq!((dup.severity, dup.file.as_deref(), dup.line), (Severity::Error, Some("src/b.rs"), Some(2)));
    assert!(dup.message.contains("`Other` has the id `todo_list` of `TodoList` (src/a.rs:2)"), "{}", dup.message);
    let hints: Vec<_> = r.issues.iter().filter(|i| i.severity == Severity::Hint).map(|i| i.message.as_str()).collect();
    assert_eq!(hints, ["no test names cell `Lone`"]);
    assert_eq!((r.cells, r.tests), (3, 1));
}

#[test]
fn a_file_that_does_not_parse_is_said_not_skipped() {
    let r = lint(project(&[("src/lib.rs", "fn broken( {")]).path());
    assert_eq!(r.issues[0].area, "scan");
    assert!(r.issues[0].message.starts_with("not checked"));
}

#[test]
fn build_output_dot_dirs_and_test_modules_are_not_app_code() {
    let d = project(&[
        ("target/x.rs", "const PASSWORD: &str = \"hunter22\";"),
        (".git/x.toml", "password = \"hunter22\""),
        (
            "src/lib.rs",
            "#[cfg(test)]\nmod tests { #[test] fn a() { assert!(1 < 2) } #[aio::test] fn b() { assert!(1 < 2) } fn App() -> Element { rsx! { img {} } } }\n",
        ),
        ("src/tests.rs", "fn x() -> Element { rsx! { img {} } }"),
    ]);
    let r = lint(d.path());
    assert!(r.issues.is_empty(), "{:?}", r.issues);
    assert_eq!((r.files, r.tests), (2, 2));
}

#[test]
fn the_marker_needs_a_reason_its_area_and_no_blank_line() {
    fn l(s: &str) -> Vec<&str> {
        s.lines().collect()
    }
    assert!(suppressed(&l("x // aio-ok: why"), 0, "a11y"));
    assert!(suppressed(&l("// aio-ok(a11y): why\nx"), 1, "a11y"));
    assert!(suppressed(&l("// aio-ok(cells, a11y): why\n#[attr]\n/// doc\nx"), 3, "a11y"));
    assert!(!suppressed(&l("// aio-ok(cells): why\nx"), 1, "a11y"), "another area's");
    assert!(!suppressed(&l("// aio-ok\nx"), 1, "a11y"), "no reason");
    assert!(!suppressed(&l("// aio-ok:   \nx"), 1, "a11y"), "an empty reason");
    assert!(!suppressed(&l("// aio-ok: why\n\nx"), 2, "a11y"), "a blank line ends the block");
    assert!(!suppressed(&l("// aio-ok: why\nlet y = 1;\nx"), 2, "a11y"), "code ends the block");
    assert!(!suppressed(&l("x // no-aio-ok: why"), 0, "a11y"), "a word, not a substring");
    assert!(suppressed(&l("# aio-ok(security): a public demo key\ntoken = \"…\""), 1, "security"));
}

#[test]
fn clippy_json_becomes_findings_once_each_in_the_project() {
    let root = Path::new("/ws");
    let msg = |level: &str, file: &str, code: &str| {
        serde_json::json!({"reason": "compiler-message", "message": {"level": level, "message": "m", "code": {"code": code},
            "spans": [{"file_name": file, "line_start": 3, "is_primary": true}]}})
        .to_string()
    };
    let out = [
        msg("warning", "app/src/lib.rs", "clippy::dbg_macro"),
        msg("warning", "app/src/lib.rs", "clippy::dbg_macro"),
        msg("error", "app/src/ui.rs", "unused_imports"),
        msg("note", "app/src/ui.rs", ""),
        msg("warning", "other/src/lib.rs", "clippy::todo"),
        r#"{"reason":"compiler-artifact"}"#.into(),
        r#"{"reason":"compiler-message","message":{"level":"warning","message":"2 warnings emitted","spans":[]}}"#
            .into(),
        "not json".into(),
    ]
    .join("\n");
    let got = clippy_issues(&out, root, &root.join("app"));
    assert_eq!(
        got.iter().map(|i| (i.severity, i.area, i.file.clone().unwrap(), i.fix.clone().unwrap())).collect::<Vec<_>>(),
        [
            (Severity::Warn, "clippy", "src/lib.rs".into(), "see clippy::dbg_macro".into()),
            (Severity::Error, "rustc", "src/ui.rs".into(), "see unused_imports".into()),
        ]
    );
}

#[test]
fn json_has_aios_shape_and_human_hides_hints_on_request() {
    let r = Report {
        issues: vec![
            Issue::new(Severity::Error, "a11y", "e".into(), "src/a.rs", 2).fix("f"),
            Issue::new(Severity::Hint, "testing", "h".into(), "src/a.rs", 0),
        ],
        passed: vec!["ok".into()],
        files: 3,
        cells: 1,
        tests: 0,
    };
    let v = json(&r);
    assert_eq!(v["stats"], serde_json::json!({"filesScanned": 3, "cellsFound": 1, "testsFound": 0}));
    assert_eq!(v["summary"], serde_json::json!({"errors": 1, "warnings": 0, "hints": 1}));
    assert_eq!(
        v["issues"][0],
        serde_json::json!({"severity": "error", "area": "a11y", "message": "e", "file": "src/a.rs", "line": 2, "fix": "f"})
    );
    assert_eq!(v["issues"][1].get("line"), None);
    let h = human(&r, Path::new("/p"), true);
    assert!(h.contains("✗ error src/a.rs:2  [a11y] e\n      fix: f") && h.contains("✓ ok"), "{h}");
    assert!(!h.contains("[testing]") && h.contains("1 error · 0 warnings · 1 hint (hidden"), "{h}");
    assert!(human(&r, Path::new("/p"), false).contains("[testing] h"));
}

#[test]
fn usage_errors_exit_2_and_a_missing_dir_exits_1() {
    let a = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    assert_eq!(main(&a(&["--safefix"])), ExitCode::from(2));
    assert_eq!(main(&a(&["a", "b"])), ExitCode::from(2));
    let d = project(&[("f.txt", "")]);
    assert_eq!(main(&a(&[d.path().join("f.txt").to_str().unwrap()])), ExitCode::from(2));
    assert_eq!(main(&a(&[d.path().join("none").to_str().unwrap()])), ExitCode::FAILURE);
    assert_eq!(main(&a(&["--help"])), ExitCode::SUCCESS);
}

#[test]
fn the_usage_lists_every_flag() {
    for f in super::FLAGS.iter().filter(|f| **f != "--help") {
        assert!(super::USAGE.lines().next().unwrap().contains(f), "{f} missing from the usage line");
    }
}

#[test]
fn exit_is_1_on_errors_only() {
    let a = |p: &Path| vec![p.to_string_lossy().into_owned(), "--no-clippy".into(), "--json".into()];
    let bad = project(&[("Cargo.toml", ""), ("src/ui.rs", "fn A() -> Element { rsx! { img {} } }")]);
    assert_eq!(main(&a(bad.path())), ExitCode::FAILURE);
    let hinted = project(&[("Cargo.toml", ""), ("src/a.rs", "#[cell]\npub struct Lone { pub n: u32 }")]);
    assert_eq!(main(&a(hinted.path())), ExitCode::SUCCESS);
}

#[test]
fn the_project_is_the_nearest_cargo_toml() {
    let d = project(&[("app/Cargo.toml", ""), ("app/src/ui/x.rs", ""), (".git/HEAD", "")]);
    assert_eq!(project_root(&d.path().join("app/src/ui")), d.path().join("app"));
    assert_eq!(project_root(&d.path().join("app")), d.path().join("app"));
    let none = project(&[("x/y.rs", ""), (".git/HEAD", "")]);
    assert_eq!(project_root(&none.path().join("x")), none.path().join("x"), "stops at the repository root");
}

#[test]
fn page_css_kept_in_a_hook_is_an_error_because_edits_never_show() {
    let d = project(&[(
        "src/lib.rs",
        r#"
#[component]
pub fn App() -> View {
    let css = once(|| aio::ui::page_css(&[CSS], false));
    let live = aio::use_css!(false; "assets/style.css");
    let fine = aio::ui::page_css(&[CSS], false);
    let n = once(|| 1);
    let old = use_hook(|| aio::ui::page_css(&[CSS], false));
    rsx! { style { {css} } }
}
"#,
    )]);
    let r = lint(d.path());
    let stale: Vec<_> = r.issues.iter().filter(|i| i.message.contains("made in `once`")).collect();
    assert_eq!(stale.len(), 2, "{:?}", r.issues);
    assert_eq!((stale[0].severity, stale[0].line, stale[1].line), (Severity::Error, Some(4), Some(8)));
    assert!(stale[0].fix.as_deref().unwrap().contains("aio::use_css!"));
}

const OLD_ROOT: &str = r#"[package]
name = "sky"

[dependencies]
aio = { package = "aio-rs", path = "../aio/crates/aio", default-features = false, features = ["kit"] }
dioxus = { version = "=0.7.10" }

[dev-dependencies]
aio = { package = "aio-rs", path = "../aio/crates/aio", default-features = false, features = ["testing"] }

[profile.release]
lto = true
"#;

const OLD_APP: &str = r#"[package]
name = "sky-app"

[dependencies]
app = { package = "sky", path = ".." }
aio = { package = "aio-rs", path = "../../aio/crates/aio", default-features = false }

# build.rs parses aio.toml: the binary holds it, without a TOML parser.
[build-dependencies]
aio = { package = "aio-rs", path = "../../aio/crates/aio", default-features = false }
"#;

const OLD_BUILD: &str =
    "// aio.toml, parsed as the app builds.\nfn main() {\n    aio::config::build(\"../aio.toml\");\n}\n";

#[test]
fn an_app_of_the_older_layout_is_told_and_moved_by_fix() {
    let main = "fn main() -> std::io::Result<()> {\n    let cfg = aio::config!()?;\n    Ok(())\n}\n";
    let d = project(&[
        ("Cargo.toml", OLD_ROOT),
        ("app/Cargo.toml", OLD_APP),
        ("app/build.rs", OLD_BUILD),
        ("app/src/main.rs", main),
    ]);
    let r = lint(d.path());
    let got: Vec<_> = found(&r).into_iter().filter(|(a, _)| *a == "migrate").map(|(_, at)| at).collect();
    assert_eq!(got, ["Cargo.toml:9", "app/Cargo.toml:10", "app/build.rs:1", "app/src/main.rs:2"], "{:?}", r.issues);
    assert!(r.issues.iter().filter(|i| i.area == "migrate").all(|i| i.fix.as_deref() == Some("ax lint --fix")));

    let files = |d: &Path| {
        let mut v = Vec::new();
        walk(d, &mut v);
        v
    };
    assert_eq!(layout::fix(d.path(), &files(d.path())).unwrap().len(), 4);
    let read = |p: &str| std::fs::read_to_string(d.path().join(p)).unwrap();
    assert_eq!(read("app/src/main.rs"), main.replace("aio::config!()?", "aio::config!()"));
    assert_eq!(
        read("Cargo.toml"),
        "[package]\nname = \"sky\"\n\n[dependencies]\naio = { package = \"aio-rs\", path = \"../aio/crates/aio\", \
         default-features = false, features = [\"kit\"] }\ndioxus = { version = \"=0.7.10\" }\n\n[profile.release]\nlto = true\n"
    );
    assert_eq!(
        read("app/Cargo.toml"),
        "[package]\nname = \"sky-app\"\n\n[dependencies]\napp = { package = \"sky\", path = \"..\" }\naio = { package = \
         \"aio-rs\", path = \"../../aio/crates/aio\", default-features = false }\n"
    );
    assert!(!d.path().join("app/build.rs").exists());
    assert!(lint(d.path()).issues.iter().all(|i| i.area != "migrate"), "nothing left to move");
    assert!(layout::fix(d.path(), &files(d.path())).unwrap().is_empty(), "a second --fix does nothing");
}

#[test]
fn a_build_rs_that_does_more_than_parse_aio_toml_is_left_alone() {
    assert!(layout::old_build_rs(OLD_BUILD));
    assert!(!layout::old_build_rs("fn main() {\n    aio::config::build(\"../aio.toml\");\n    cc::Build::new();\n}\n"));
    assert!(!layout::old_build_rs("fn main() { println!(\"cargo::rerun-if-changed=x\"); }"));
    let kept = "[dev-dependencies]\naio = { package = \"aio-rs\", path = \"a\", features = [\"testing\", \"kit\"] }\n";
    assert_eq!(
        layout::migrate_manifest(kept).as_deref(),
        Some("[dev-dependencies]\naio = { package = \"aio-rs\", path = \"a\", features = [\"kit\"] }\n")
    );
}

#[test]
fn a_typed_newline_is_told_it_no_longer_presses_enter() {
    let src = "fn t(ui: &mut TestUi) {\n    ui.set_value(\"a\", \"x\\n\");\n    ui.type_text(\"a\", \"x\");\n}\n";
    let d = project(&[("Cargo.toml", "[package]\nname = \"x\"\n"), ("tests/ui.rs", src)]);
    let r = lint(d.path());
    let got: Vec<_> = found(&r).into_iter().filter(|(a, _)| *a == "migrate").map(|(_, at)| at).collect();
    assert_eq!(got, ["tests/ui.rs:2"], "{:?}", r.issues);
    // aio's `am migrate --tests`: the test shapes alone.
    let shapes: Vec<&Issue> = r.issues.iter().filter(|i| test_shape(i)).collect();
    assert_eq!(shapes.len(), 1, "{:?}", r.issues);
}

#[test]
fn from_a_version_only_what_changed_after_it() {
    // aio's `am migrate --from`: an app made with a release has its layout.
    let src = "fn f() {\n    let _ = aio::retry;\n}\n";
    let d = project(&[("Cargo.toml", "[package]\nname = \"x\"\n"), ("src/lib.rs", src)]);
    let all = lint(d.path()).issues;
    assert!(all.iter().any(|i| i.area == "migrate"), "{all:?}");
    let before = migrate_from(lint(d.path()).issues, "0.0.9").unwrap();
    assert!(!before.is_empty() && before.iter().all(|i| i.area == "migrate"), "{before:?}");
    assert!(migrate_from(all, layout::SINCE).unwrap().is_empty(), "made with it: nothing to move");
    assert!(migrate_from(vec![], "soon").unwrap_err().contains("an aio-rs version"));
}

#[test]
fn renamed_paths_are_told_and_moved_by_fix() {
    let old = "use aio::testing::Call;\nfn f() {\n    let _d = aio::hold::deciding();\n    let _ = aio::retry;\n    let _ = (aio::retry_later, aio::retry);\n}\n";
    let d = project(&[("Cargo.toml", "[package]\nname = \"x\"\n"), ("src/lib.rs", old)]);
    let r = lint(d.path());
    let got: Vec<_> = found(&r).into_iter().filter(|(a, _)| *a == "migrate").map(|(_, at)| at).collect();
    assert_eq!(
        got,
        ["src/lib.rs:1", "src/lib.rs:3", "src/lib.rs:4", "src/lib.rs:5"],
        "a longer name is not a rename: {:?}",
        r.issues
    );
    let mut files = Vec::new();
    walk(d.path(), &mut files);
    assert_eq!(layout::fix(d.path(), &files).unwrap().len(), 1);
    assert_eq!(
        std::fs::read_to_string(d.path().join("src/lib.rs")).unwrap(),
        "use aio::testing::RandomCall;\nfn f() {\n    let _d = aio::deciding::start();\n    let _ = aio::effect::retry;\n    let _ = (aio::retry_later, aio::effect::retry);\n}\n"
    );
}

#[test]
fn tests_that_check_nothing_are_named() {
    let d = project(&[
        (
            "tests/notes.rs",
            r#"
#[test]
fn adds() {
    let n = Notes::default();
    n.add("a".into());
}

#[test]
fn compares_itself() {
    let n = 1 + 1;
    assert_eq!(n, n);
}

#[test]
#[should_panic(expected = "full")]
fn refuses() {
    Notes::full().add("a".into());
}

#[test]
fn checks() -> Result<(), String> {
    let mut t = TestCells::new();
    t.cell::<Notes>().add("a".into())?;
    assert_eq!(t.effects(), ["Notes.save"]);
    Ok(())
}
"#,
        ),
        (
            "src/lib.rs",
            r#"
pub fn f() -> u8 { 1 }

#[cfg(test)]
mod tests {
    #[test]
    fn calls_only() {
        super::f();
    }

    #[test]
    fn unwraps() {
        "1".parse::<u8>().unwrap();
    }
}
"#,
        ),
    ]);
    let r = lint(d.path());
    let t: Vec<_> = r
        .issues
        .iter()
        .filter(|i| i.area == "testing" && i.message.contains("("))
        .map(|i| (i.file.clone().unwrap(), i.line.unwrap(), i.message.clone()))
        .collect();
    assert_eq!(
        t.iter().map(|(f, l, _)| format!("{f}:{l}")).collect::<Vec<_>>(),
        ["src/lib.rs:7", "tests/notes.rs:3", "tests/notes.rs:11"],
        "{t:?}"
    );
    assert!(t[1].2.contains("test `adds` checks nothing") && t[2].2.contains("compares `n` with itself"), "{t:?}");

#[test]
fn ax_dev_lints_each_saved_file_alone() {
    let ui = "use aio::ui::prelude::*;\nfn Ui() -> Element {\n    rsx! {\n        div { onclick: move |_| {}, \"Open\" }\n    }\n}\n";
    let d = project(&[("src/ui.rs", ui), ("src/half.rs", "fn x( {"), ("tests/t.rs", ui), ("assets/a.css", "x")]);
    let files: Vec<PathBuf> =
        ["src/ui.rs", "src/half.rs", "tests/t.rs", "assets/a.css"].map(|f| d.path().join(f)).into();
    let said = changed(&files, d.path());
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(said[0].starts_with("warn src/ui.rs:4 [a11y] <div> takes clicks but no keys"), "{said:?}");
}

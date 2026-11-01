//! The UI driver acts as a person does and reads what they see (aio's `testUI`).
use aio::act;
use aio::prelude::*;
use aio::testing::{Key, Modifiers, Mount, TestUi, fails, seeded_source};
use aio::ui::prelude::*;
use serde_json::json;
use std::cell::RefCell;

#[cell]
pub struct Prefs {
    pub theme: String,
    pub agreed: bool,
    pub gpus: Vec<String>,
}

#[methods]
impl Prefs {
    pub fn set_theme(&mut self, v: String) {
        self.theme = v;
    }

    pub fn agree(&mut self, v: bool) {
        self.agreed = v;
    }
}

aio::client!(Prefs, prefs);

thread_local! {
    static LOG: RefCell<Vec<String>> = const { RefCell::new(vec![]) };
}

fn log(s: impl Into<String>) {
    LOG.with(|l| l.borrow_mut().push(s.into()));
}

/// What was logged since the last call.
fn logged() -> Vec<String> {
    LOG.with(|l| l.take())
}

#[component]
fn Greeting(name: String, times: u32) -> View {
    rsx! { p { "data-t": "hi", "Hello {name} ×{times}" } }
}

#[test]
fn a_component_mounts_alone_with_its_props() {
    let ui = TestUi::with(Greeting, GreetingProps { name: "Ada".into(), times: 2 });
    assert_eq!(ui.text("hi"), "Hello Ada ×2");
    assert_eq!(ui.html(), "<p data-t=\"hi\">Hello Ada ×2</p>");
}

fn gpus() -> View {
    use_live();
    let p = prefs();
    rsx! {
        if p.gpus.is_empty() {
            p { "data-t": "none", "no GPU" }
        } else {
            p { "data-t": "gpus", "{p.gpus.join(\", \")}" }
        }
        button { "data-t": "dark", onclick: move |_| _ = act!(set_theme("dark".into())), "Dark" }
    }
}

#[test]
fn a_seed_pins_the_state_before_the_first_render_and_methods_run_on_it() {
    let mut ui = Mount::default().before(|| *HANDLE.write() = seeded_source(json!({"gpus": ["rtx"]}))).app(gpus);
    assert!(ui.present("gpus") && ui.absent("none"), "the branch this machine never takes");
    ui.click("dark");
    ui.expect_cell(&Ui, "the theme is dark and the seed kept", |p| p.theme == "dark" && p.gpus == ["rtx"]);
    assert!(TestUi::new(gpus).present("none"), "no seed: the declared default");
}

#[test]
fn a_seed_mid_test_moves_the_state_and_the_ui_follows_it() {
    let mut ui = TestUi::new(gpus);
    assert!(ui.present("none"));
    ui.run(|| *HANDLE.write() = seeded_source(json!({"gpus": ["a", "b"]})));
    assert_eq!(ui.text("gpus"), "a, b");
    // Commits made off the UI on the new source re-render too.
    let h = ui.run(|| HANDLE.read_untracked().clone());
    std::thread::spawn(move || h.set_theme("x".into()).unwrap()).join().unwrap();
    ui.expect_cell(&Ui, "the seeded source took the call", |p| p.theme == "x" && p.gpus.len() == 2);
    ui.run(|| *HANDLE.write() = seeded_source(json!({})));
    assert!(ui.present("none"), "back to the default");
}

#[test]
fn a_seed_key_the_cell_lacks_is_refused() {
    let why = fails(|| {
        Mount::default().before(|| *HANDLE.write() = seeded_source::<Prefs>(json!({"gpu": ["x"]}))).app(gpus);
    });
    assert!(why.contains("seed `gpu` isn't part of Prefs's state; it has: agreed, gpus, theme"), "{why}");
}

#[test]
fn expect_cell_retries_then_prints_the_state() {
    let why = fails(|| {
        let mut ui = TestUi::new(gpus);
        ui.click("dark");
        ui.expect_cell(&Ui, "the theme is light", |p| p.theme == "light");
    });
    assert!(why.contains("the theme is light\n  state: {\"theme\":\"dark\""), "{why}");
}

fn sized() -> View {
    rsx! {
        style { "@media (max-width: 600px) {{ .wide {{ display: none }} }} @media (prefers-color-scheme: dark) {{ .light {{ display: none }} }}" }
        p { class: "wide", "data-t": "wide", "wide" }
        p { class: "light", "data-t": "light", "light" }
    }
}

#[test]
fn the_window_size_and_colour_scheme_are_the_tests() {
    let ui = TestUi::new(sized);
    assert!(ui.present("wide") && ui.present("light"));
    let ui = Mount::default().size(400, 800).dark().app(sized);
    assert!(ui.absent("wide") && ui.absent("light"));
}

fn controls() -> View {
    use_live();
    let p = prefs();
    let on = signal(|| false);
    rsx! {
        select { "data-t": "theme", onchange: move |e| _ = act!(set_theme(e.value())),
            option { value: "light", "Light" }
            option { value: "dark", "Dark" }
            option { value: "neon", disabled: true, "Neon" }
        }
        input { "data-t": "agree", r#type: "checkbox", checked: p.agreed, onchange: move |e| _ = act!(agree(e.checked())) }
        input { "data-t": "name", placeholder: "Name", required: true,
            onchange: move |e| log(format!("change {}", e.value())),
            onblur: move |_| log("blur"), onfocus: move |_| log("focus") }
        input { "data-t": "ro", readonly: true, value: "x" }
        button { "data-t": "mute", "aria-pressed": "{on}", onclick: move |_| on.toggle(), "Mute" }
        button { "data-t": "menu", "aria-expanded": "false", "aria-label": "Open menu", "☰" }
        div { "data-t": "switch", role: "switch", "aria-checked": "{on}", onclick: move |_| on.toggle(), "Sw" }
        label { r#for: "e", "Email" }
        input { id: "e", "data-t": "email" }
    }
}

#[test]
fn select_check_and_clear_act_as_a_person_does() {
    let mut ui = TestUi::new(controls);
    ui.select("theme", "dark");
    ui.expect_cell(&Ui, "dark", |p| p.theme == "dark");
    ui.check("agree");
    ui.check("agree");
    assert!(ui.checked("agree"), "checking twice is still checked");
    ui.expect_cell(&Ui, "agreed once", |p| p.agreed);
    ui.uncheck("agree");
    ui.expect_cell(&Ui, "unagreed", |p| !p.agreed);
    ui.type_text("name", "Ann");
    assert_eq!(ui.value("name"), "Ann");
    ui.clear("name");
    assert_eq!(ui.value("name"), "");
    ui.check("switch");
    assert_eq!(ui.attr("switch", "aria-checked").as_deref(), Some("true"), "an ARIA switch checks too");
}

#[test]
fn an_option_that_isnt_there_cant_be_picked() {
    let why = fails(|| {
        TestUi::new(controls).select("theme", "blue");
    });
    assert!(why.contains("data-t=\"theme\" has no option \"blue\"; it has: \"light\", \"dark\", \"neon\""), "{why}");
}

#[test]
fn a_disabled_option_cant_be_picked() {
    let why = fails(|| {
        TestUi::new(controls).select("theme", "neon");
    });
    assert!(why.contains("option \"neon\" of data-t=\"theme\" is disabled"), "{why}");
}

#[test]
fn check_never_clicks_a_plain_button() {
    let why = fails(|| {
        TestUi::new(controls).check("mute");
    });
    assert!(why.contains("data-t=\"mute\" has no checked state: click it instead"), "{why}");
}

#[test]
fn an_edited_field_fires_change_when_it_loses_focus() {
    let mut ui = TestUi::new(controls);
    logged();
    ui.type_text("name", "Bo");
    assert_eq!(logged(), ["focus"], "no change while typing");
    ui.blur("name");
    assert_eq!(logged(), ["change Bo", "blur"]);
    ui.focus("name");
    ui.click("mute");
    assert_eq!(logged(), ["focus", "blur"], "unedited: no change");
}

#[test]
fn element_state_reads_as_booleans() {
    let mut ui = TestUi::new(controls);
    assert!(ui.required("name") && !ui.required("ro"));
    assert!(ui.readonly("ro") && !ui.readonly("name"));
    assert!(!ui.pressed("mute") && !ui.expanded("menu"));
    ui.click("mute");
    assert!(ui.pressed("mute"));
    let menu = ui.info("menu");
    assert_eq!((menu.tag.as_str(), menu.role.as_str(), menu.name.as_str()), ("button", "button", "Open menu"));
    assert_eq!(ui.info("email").name, "Email", "named by its label");
    assert_eq!(ui.info("name").name, "Name", "named by its placeholder");
    assert_eq!(ui.info("switch").role, "switch");
}

#[test]
fn the_surface_lists_what_a_person_can_see_and_use() {
    let ui = TestUi::new(controls);
    let s = ui.surface();
    for line in [
        "combobox \"\" [theme] = \"light\"",
        "checkbox \"\" [agree] unchecked",
        "textbox \"Name\" [name] = \"\"",
        "button \"Mute\" [mute]",
        "button \"Open menu\" [menu]",
        "switch \"Sw\" [switch]",
        "textbox \"Email\" [email] = \"\"",
    ] {
        assert!(s.lines().any(|l| l.trim() == line), "{line:?} not in:\n{s}");
    }
}

fn unnamed() -> View {
    let mut n = signal(|| 0);
    rsx! {
        form {
            label { "Email", input { r#type: "email" } }
            input { placeholder: "Title" }
            input {}
            button { onclick: move |e| { e.prevent_default(); n += 1 }, "Save" }
            button { onclick: move |e| { e.prevent_default(); n += 10 }, "Save" }
            button { "aria-label": "", "Buy milk!" }
            button { "ปิด" }
            a { href: "/help", "Help" }
            button { "data-t": "SaveButton3", "the author's" }
            ul { li { "One thing" } }
            p { "data-t": "n", "{n}" }
        }
        div { style: "display:none", button { "Hidden" } }
    }
}

#[test]
fn elements_are_named_by_their_label_and_role_with_no_handle() {
    // aio tests: ui-surface auto names (label + role, numbered in order).
    let mut ui = TestUi::new(unnamed);
    assert_eq!(
        ui.names(),
        [
            "Form",
            "EmailInput",
            "TitleInput",
            "Input",
            "SaveButton",
            "SaveButton2",
            "BuyMilkButton",
            "ปิดButton",
            "HelpLink",
            "TheAuthorSButton",
            "OneThingItem"
        ],
        "a blank aria-label names nothing; a hidden element has no name"
    );
    ui.click("SaveButton2");
    assert_eq!(ui.text("n"), "10", "the second Save, by its name");
    ui.type_text("EmailInput", "a@b.c");
    assert_eq!(ui.value("EmailInput"), "a@b.c");
    assert!(ui.present("HelpLink") && ui.absent("HiddenButton"));
    assert_eq!(ui.text("SaveButton3"), "the author's", "a handle wins over a name");
}

fn rows() -> View {
    let gone = signal(Vec::<u32>::new);
    rsx! {
        ul { "data-t": "list",
            for id in (1..=3).filter(|i| !gone.read().contains(i)) {
                li { key: "{id}", "data-t": "row-{id}", "Task {id}",
                    button { onclick: move |_| gone.write().push(id), "Delete" }
                    span { "data-t": "state", "open" }
                }
            }
        }
    }
}

#[test]
fn a_path_finds_a_name_or_handle_inside_another_element() {
    // aio tests: ui.find("Row", key).delete; names restart in each scope.
    let mut ui = TestUi::new(rows);
    assert_eq!(ui.names()[..4], ["Task1Item", "DeleteButton", "Task2Item", "DeleteButton2"], "numbered on the page");
    ui.click("row-2/DeleteButton");
    assert!(ui.absent("row-2") && ui.present("row-3/DeleteButton") && ui.absent("row-3/DeleteButton2"));
    assert_eq!(ui.text("list/row-3/state"), "open", "a handle inside, and paths nest");
    assert!(ui.present("list/Task3Item"), "a row named by its own text");
}

#[test]
fn a_missing_name_lists_the_names_there() {
    let why = fails(|| {
        TestUi::new(unnamed).click("SavButton");
    });
    assert!(why.contains("the handles there are: SaveButton3, n; by name: Form, EmailInput"), "{why}");
}

fn gestures() -> View {
    rsx! {
        button { "data-t": "twice",
            onclick: move |e| log(format!("click shift={}", e.modifiers().shift())),
            ondoubleclick: move |_| log("dblclick"), "x" }
        div { "data-t": "feed", style: "height: 100px; overflow: auto",
            onscroll: move |e| log(format!("scroll {}", e.scroll_top())),
            div { style: "height: 1000px", "long list" }
        }
        div { "data-t": "flat", "short" }
        div { "data-t": "card", draggable: true, "data-drag": "card-1",
            ondragstart: move |_| log("start"),
            ondragend: move |_| log("end"), "Card" }
        div { "data-t": "bin",
            ondragover: move |_| log("over"),
            ondrop: move |e| log(format!("drop {}", e.data_transfer().get_as_text().unwrap_or_default())), "Bin" }
        div { "data-t": "pad", style: "width: 200px; height: 100px",
            onpointerdown: move |e| log(format!("down {}", e.element_coordinates().x)),
            onpointermove: move |e| if e.held_buttons().contains(MouseButton::Primary) { log(format!("move {}", e.client_coordinates().x)) },
            onpointerup: move |_| log("up"),
            onwheel: move |e| log(format!("wheel {}", e.delta().strip_units().y)), "pad" }
        input { "data-t": "keys",
            onkeydown: move |e| log(format!("down {} shift={}", e.key(), e.modifiers().shift())),
            onkeyup: move |e| log(format!("up {}", e.key())) }
    }
}

#[test]
fn clicks_carry_modifiers_and_a_double_click_is_two_clicks_then_dblclick() {
    let mut ui = TestUi::new(gestures);
    ui.click_with("twice", Modifiers::SHIFT);
    assert_eq!(logged(), ["click shift=true"]);
    ui.dblclick("twice");
    assert_eq!(logged(), ["click shift=false", "click shift=false", "dblclick"]);
}

#[test]
fn scroll_and_drag_and_drop_fire_faithful_sequences() {
    let mut ui = TestUi::new(gestures);
    ui.scroll("feed", 250.0, 0.0);
    assert_eq!(logged(), ["scroll 250"]);
    ui.drag_to("card", "bin");
    // What the card carries (`data-drag`) is set in the page's dragstart,
    // so the drop reads it from the browser's own data transfer.
    assert_eq!(logged(), ["start", "over", "drop card-1", "end"]);
}

#[test]
fn a_box_that_doesnt_scroll_cant_be_scrolled() {
    let why = fails(|| {
        TestUi::new(gestures).scroll("flat", 50.0, 0.0);
    });
    assert!(why.contains("data-t=\"flat\" can't scroll to top 50, left 0: it stopped at top 0"), "{why}");
}

#[test]
fn a_pointer_drag_and_the_wheel_reach_the_element_with_its_coordinates() {
    let mut ui = TestUi::new(gestures);
    logged();
    ui.drag_by("pad", 30.0, 0.0);
    let got = logged();
    assert_eq!(
        got.first().map(String::as_str),
        Some("down 100"),
        "the centre, in the element's own coordinates: {got:?}"
    );
    assert_eq!(got.last().map(String::as_str), Some("up"));
    assert_eq!(got.iter().filter(|l| l.starts_with("move ")).count(), 1, "one held move: {got:?}");
    ui.wheel("pad", 0.0, -120.0);
    assert_eq!(logged(), ["wheel -120"]);
}

#[test]
fn keys_are_named_held_and_released_with_modifiers() {
    let mut ui = TestUi::new(gestures);
    ui.press_with("keys", "a", Modifiers::SHIFT);
    ui.key_down("keys", "ArrowLeft", Modifiers::empty());
    ui.key_down("keys", "ArrowLeft", Modifiers::empty());
    ui.key_up("keys", "ArrowLeft", Modifiers::empty());
    let got: Vec<String> = logged().into_iter().filter(|l| l.starts_with("down") || l.starts_with("up ")).collect();
    assert_eq!(
        got,
        ["down a shift=true", "up a", "down ArrowLeft shift=false", "down ArrowLeft shift=false", "up ArrowLeft"]
    );
}

#[test]
fn a_chord_is_pressed_as_aio_trigger_takes_it() {
    let mut ui = TestUi::new(gestures);
    ui.press("keys", "shift+a");
    let got: Vec<String> = logged().into_iter().filter(|l| l.starts_with("down") || l.starts_with("up ")).collect();
    assert_eq!(got, ["down a shift=true", "up a"]);
    let cmd = if cfg!(target_os = "macos") { Modifiers::META } else { Modifiers::CONTROL };
    assert_eq!(TestUi::chord("mod+shift+K"), ("K".into(), cmd | Modifiers::SHIFT));
    assert_eq!(TestUi::chord("ctrl++"), ("+".into(), Modifiers::CONTROL));
    assert_eq!(TestUi::chord("+"), ("+".into(), Modifiers::empty()));
    assert_eq!(TestUi::chord("esc").0, "esc", "a lone key is the DOM's name, as before");
    assert_eq!(TestUi::chord("alt+esc"), ("Escape".into(), Modifiers::ALT));
    let why = fails(|| {
        TestUi::chord("hyper+a");
    });
    assert!(why.contains("\"hyper\" is not a modifier"), "{why}");
}

#[test]
fn a_key_name_that_names_no_key_is_refused() {
    let why = fails(|| {
        TestUi::new(gestures).press("keys", "Entr");
    });
    assert!(why.contains("no key is named \"Entr\""), "{why}");
}

#[component]
fn Form(fields: u32, button: bool, required: bool) -> View {
    rsx! {
        form { "data-t": "form",
            onsubmit: move |e| log(format!("submit {:?}", e.values().into_iter().map(|(k, v)| match v { FormValue::Text(t) => format!("{k}={t}"), _ => k }).collect::<Vec<_>>())),
            input { "data-t": "a", name: "a", required, onkeydown: move |e| if e.key() == Key::Escape { e.prevent_default() } }
            for i in 1..fields {
                input { "data-t": "f{i}", name: "f{i}" }
            }
            input { r#type: "checkbox", name: "c", checked: true }
            if button {
                button { "data-t": "go", onclick: move |_| log("click"), "Go" }
            }
            button { r#type: "button", "data-t": "plain", "Plain" }
        }
    }
}

fn form(fields: u32, button: bool, required: bool) -> TestUi {
    logged();
    TestUi::with(Form, FormProps { fields, button, required })
}

#[test]
fn enter_submits_a_form_by_the_html_rule() {
    let mut ui = form(1, false, false);
    ui.type_text("a", "x");
    ui.press("a", "Enter");
    assert_eq!(logged(), ["submit [\"a=x\", \"c=on\"]"], "one line input, no button: Enter submits");
    ui.press_with("a", "Enter", Modifiers::CONTROL);
    assert!(logged().is_empty(), "a modified Enter is a shortcut, not a submit");
    let mut ui = form(2, false, false);
    ui.press("a", "Enter");
    assert_eq!(
        logged(),
        ["submit [\"a=\", \"f1=\", \"c=on\"]"],
        "two line inputs and no submit button: submitted all the same (aio)"
    );
    let mut ui = form(2, true, false);
    ui.press("a", "Enter");
    assert_eq!(logged(), ["click", "submit [\"a=\", \"f1=\", \"c=on\"]"], "the first submit button is clicked");
    ui.click("plain");
    assert!(logged().is_empty(), "a type=button button submits nothing");
    ui.click("go");
    assert_eq!(logged(), ["click", "submit [\"a=\", \"f1=\", \"c=on\"]"]);
}

#[test]
fn a_required_blank_refuses_the_submit() {
    let why = fails(|| {
        form(1, true, true).click("go");
    });
    assert!(why.contains("the form can't be submitted: required field `a` is empty"), "{why}");
}

fn slow() -> View {
    rsx! { p { "data-t": "hello", "hello" } }
}

#[test]
fn until_passes_at_once_when_it_holds_whatever_the_patience() {
    TestUi::new(slow).until_within(std::time::Duration::ZERO, "nothing", || true);
}

#[test]
fn a_timeout_shows_the_screen_and_leaves_a_trace() {
    let mut ui = TestUi::new(slow);
    let e = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ui.until_within(std::time::Duration::from_millis(20), "a goodbye", || false)
    }))
    .expect_err("times out");
    let msg = e.downcast_ref::<String>().expect("a message");
    assert!(
        msg.starts_with("timed out after 20ms waiting for: a goodbye\n  screen:\n  generic \"hello\" [hello]"),
        "{msg}"
    );
    let file = msg.rsplit("trace: ").next().expect("a trace line").trim();
    let trace: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(file).expect("the trace")).unwrap();
    assert_eq!(trace["handles"], json!(["hello"]));
    assert_eq!(trace["html"], json!("<p data-t=\"hello\">hello</p>"));
    let dir = std::path::Path::new(file).parent().unwrap().to_path_buf();
    assert!(dir.ends_with("a_timeout_shows_the_screen_and_leaves_a_trace"), "the test's own folder: {}", dir.display());
    drop(ui);
    assert!(!dir.exists(), "a test that passed leaves no trace");
}

fn handles() -> View {
    let field = signal(|| None::<std::rc::Rc<MountedData>>);
    let size = signal(String::new);
    rsx! {
        input { "data-t": "field", onmounted: move |e| *field.write() = Some(e.data()) }
        div { "data-t": "box", style: "width: 120px; height: 30px",
            onmounted: move |e| async move {
                let r = e.data().get_client_rect().await.expect("a rect");
                size.set(format!("{}x{}", r.width(), r.height()));
            },
            "{size}"
        }
        button { "data-t": "go", onclick: move |_| async move {
            if let Some(f) = field.get() { f.set_focus(true).await.expect("focus") }
        }, "Go" }
    }
}

#[test]
fn a_mounted_handle_reads_its_box_and_moves_focus() {
    let mut ui = TestUi::new(handles);
    assert_eq!(ui.text("box"), "120x30", "its laid-out size");
    ui.click("go");
    assert_eq!(ui.focused().as_deref(), Some("field"));
}

fn palette() -> View {
    rsx! {
        p { style: "color: #000", "fine" }
        p { class: "faint note", style: "color: #999", "faint" }
        p { style: "color: #888; font-size: 30px", "big enough" }
        div { style: "background: #222",
            p { style: "color: #333", "dark on dark" }
            p { style: "color: #eee", "light on dark" }
            p { style: "color: rgba(255, 255, 255, 0.2)", "a ghost" }
        }
        div { "data-aio-contrast": "skip", style: "background: #000", p { style: "color: #111", "over a video" } }
        div { style: "background-image: linear-gradient(red, blue)", p { style: "color: #f00", "on a picture" } }
        p { style: "color: #999; display: none", "hidden" }
    }
}

#[test]
fn text_a_person_cant_read_is_found_and_marked_skips_are_honoured() {
    let found = TestUi::new(palette).contrast();
    assert_eq!(
        found,
        [
            "<p class=\"faint note\"> \"faint\" 2.85:1, needs 4.5:1",
            "<p> \"dark on dark\" 1.26:1, needs 4.5:1",
            "<p> \"a ghost\" 1.92:1, needs 4.5:1",
        ],
        "a ghost is its colour blended over its backdrop"
    );
    assert!(Mount::default().dark().app(palette).contrast().len() == 3, "the same page, whatever the OS says");
}

fn feedback() -> View {
    let n = signal(|| 0);
    effect(move || n.set(n.get() + 1));
    rsx! { "{n}" }
}

#[test]
fn a_view_that_writes_what_it_reads_is_caught() {
    let why = fails(|| {
        TestUi::new(feedback);
    });
    assert!(why.contains("still rendering after 100 passes: a view writes state it reads (a render loop)"), "{why}");
}

#[component]
fn Clicks() -> View {
    let mut n = signal(|| 0);
    rsx! { button { "data-t": "more", onclick: move |_| n += 1, "{n}" } }
}

#[component]
fn Still(label: String) -> View {
    rsx! { p { "{label}" } }
}

fn panel() -> View {
    rsx! {
        Still { label: "a" }
        for i in 0..2 {
            Clicks { key: "{i}" }
        }
    }
}

/// aio DevTools: the component tree and who re-rendered.
#[test]
fn the_component_tree_says_who_rendered_how_often() {
    let mut ui = TestUi::new(panel);
    let tree = |ui: &TestUi| {
        let app = &ui.components()[0];
        (app.name, app.children.iter().map(|c| (c.name, c.renders)).collect::<Vec<_>>())
    };
    assert_eq!(tree(&ui), ("root", vec![("Still", 1), ("Clicks", 1), ("Clicks", 1)]));
    ui.click("more");
    ui.click("more");
    assert_eq!(ui.text("more"), "2");
    assert_eq!(tree(&ui).1, [("Still", 1), ("Clicks", 3), ("Clicks", 1)], "only the clicked one re-rendered");
}

static LOCKS: Global<u32> = Global::new(|| 0);

fn purse_like() -> View {
    let tip = signal(|| false);
    rsx! {
        div { "data-t": "bar", style: "width: 300px; overflow-x: auto; white-space: nowrap; --split-a: 40%",
            span { style: "display: inline-block; width: 280px", "filler" }
            span { "data-t": "chip", style: "display: inline-block; width: 100px", onmouseenter: move |_| tip.set(true), "chip" }
        }
        if tip.get() {
            p { "data-t": "tip", "tip" }
        }
        button { onclick: move |_| *LOCKS.write() += 1, "Lock" }
        input { "data-t": "memo" }
        textarea { "data-t": "note" }
        button { "data-t": "pick", onclick: move |_| log("picked"), "Pick" }
        div { "data-t": "plain", "plain" }
    }
}

#[test]
fn a_hover_on_what_shows_scrolls_nothing() {
    let mut ui = TestUi::new(purse_like);
    ui.hover("chip");
    assert!(ui.present("tip"));
    assert_eq!(ui.eval("document.querySelector('[data-t=bar]').scrollLeft"), json!(0), "20 px of it showed");
}

#[test]
fn an_auto_name_is_no_handle() {
    let mut ui = TestUi::new(purse_like);
    assert!(ui.present("LockButton") && !ui.has_handle("LockButton"));
    assert!(ui.has_handle("memo") && ui.handles().contains(&"memo".to_string()));
    ui.click("LockButton");
    assert_eq!(ui.read(&LOCKS), 1, "a global, read with no closure");
}

#[test]
fn focus_clicks_nothing_and_text_is_typed_literally() {
    logged();
    let mut ui = TestUi::new(purse_like);
    ui.focus("pick");
    assert_eq!((ui.focused().as_deref(), logged()), (Some("pick"), vec![]), "focused, not clicked");
    ui.set_value("memo", "bell\u{7}");
    assert_eq!(ui.value("memo"), "bell\u{7}", "a control character stays");
    ui.set_value("note", "a\nb\tc");
    assert_eq!(ui.value("note"), "a\nb\tc");
    let mut ui = form(1, false, false);
    ui.type_text("a", "x\n");
    assert!(logged().is_empty(), "a typed \\n is no Enter: nothing submitted");
    assert_eq!(ui.value("a"), "x", "a line input drops it");
}

#[test]
fn what_cant_take_focus_is_refused() {
    let why = fails(|| {
        TestUi::new(purse_like).focus("plain");
    });
    assert!(why.contains("data-t=\"plain\" can't take focus: no control, link or tabindex"), "{why}");
}

#[test]
fn styles_boxes_body_level_nodes_and_shots_are_read_from_the_live_page() {
    let mut ui = Mount::default().size(400, 300).app(purse_like);
    assert_eq!(ui.style("bar", "--split-a"), "40%");
    assert_eq!(ui.rect("bar").width, 300.0);
    ui.eval("document.body.appendChild(Object.assign(document.createElement('div'), { role: 'tooltip', textContent: 'Locks it' })), null");
    let html = ui.html();
    assert!(html.ends_with("<div role=\"tooltip\">Locks it</div>") && !html.contains("<script"), "{html}");
    let png = ui.shot();
    assert_eq!(&png[..4], b"\x89PNG");
    let size = |at: usize| u32::from_be_bytes(png[at..at + 4].try_into().unwrap());
    assert_eq!((size(16), size(20)), (400, 300), "the window's size");
}

fn texts() -> View {
    let later = signal(|| false);
    rsx! {
        button { onclick: move |_| log("skip"), "Skip this version" }
        button { onclick: move |_| log("save"), span { "Save" } }
        p { "Skip all" }
        p { "Twice" }
        p { "Twice" }
        button { "data-t": "start", onclick: move |_| {
            spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(60)).await;
                later.set(true);
            });
        }, "Start" }
        if later.get() {
            p { "data-t": "done", "done" }
        }
        button { "data-t": "pick", onclick: move |_| log("picked"), "Pick" }
        div { "data-t": "fade", style: "transition: opacity 1000ms linear", "fade" }
        // Small: turning, it covers nothing to click.
        div { style: "width: 10px; height: 10px; animation: spin 1s linear infinite" }
        style { "@keyframes spin {{ to {{ transform: rotate(360deg) }} }}" }
    }
}

#[test]
fn an_element_is_found_by_its_visible_text() {
    logged();
    let mut ui = TestUi::new(texts);
    ui.click("text=Skip this version");
    ui.click("text=  Save ");
    assert_eq!(logged(), ["skip", "save"], "the buttons, by their text (spaces folded)");
    assert_eq!(ui.info("text=Save").tag, "button", "the button around the text, not its span");
    assert!(ui.present("text=Skip all") && !ui.present("text=Skip"), "the whole text, not a part");
}

#[test]
fn a_text_two_elements_show_is_refused() {
    let why = fails(|| {
        TestUi::new(texts).text("text=Twice");
    });
    assert!(why.contains("2 elements show the text \"Twice\" (<p>, <p>): name one of them instead"), "{why}");
}

#[test]
fn until_text_waits_for_an_element_still_to_come() {
    let mut ui = TestUi::new(texts);
    ui.click("start");
    ui.until_text("done", "done");
}

#[test]
fn focus_shows_the_ring_the_keyboard_would() {
    let mut ui = TestUi::new(texts);
    ui.click("start");
    ui.focus("pick");
    assert_eq!(ui.eval("document.activeElement.matches(':focus-visible')"), json!(true), "after a click too");
}

#[test]
fn a_shot_waits_for_transitions_but_not_for_an_endless_animation() {
    let mut ui = TestUi::new(texts);
    ui.eval("document.querySelector('[data-t=fade]').style.opacity = '0', null");
    let t = std::time::Instant::now();
    ui.shot();
    assert!(t.elapsed() >= std::time::Duration::from_millis(600), "the fade ran: {:?}", t.elapsed());
    assert!(t.elapsed() < std::time::Duration::from_secs(4), "the spinner held nothing: {:?}", t.elapsed());
    let ending = "document.getAnimations().filter((a) => a.playState === 'running' && a.effect.getComputedTiming().endTime !== Infinity).length";
    assert_eq!(ui.eval(ending), json!(0));
}

#[test]
fn a_click_whose_element_moves_between_press_and_release_fails() {
    let why = fails(|| {
        let mut ui = TestUi::new(texts);
        ui.eval("addEventListener('mousedown', () => { document.querySelector('[data-t=pick]').style.marginTop = '300px' }, { once: true }), null");
        ui.click("pick");
    });
    assert!(
        why.contains("the click on data-t=\"pick\" missed it: pressed on <button> in data-t=\"pick\", released on"),
        "{why}"
    );
}

#[test]
fn the_test_world_has_an_app_home() {
    let _ui = TestUi::new(texts);
    let d = aio::paths::app_dirs().expect("the test world's app dirs");
    let world = aio::paths::sandbox().unwrap();
    assert!(d.data.starts_with(world.root()) && d.data.is_dir(), "{}", d.data.display());
    assert_eq!(aio::paths::app_dirs().unwrap().data, d.data, "the same each time");
}

fn coach() -> View {
    let got = signal(String::new);
    let mut inputs = signal(|| 0);
    rsx! {
        ul { class: "parts",
            li { id: "part-0", "data-done": "true", "Purse" }
            li { "data-done": "false", "Backup" }
        }
        div { id: "backdrop", onclick: move |_| log("backdrop"), "backdrop" }
        p { "data-t": "note", "The validator list did not load. Try again later." }
        input {
            "data-t": "uri",
            oninput: move |e| {
                inputs += 1;
                got.set(e.value());
            },
        }
        p { "data-t": "got", "{inputs} {got}" }
        button { "data-t": "lit", style: "background: rgb(0, 0, 255)", class: "lit", "Lit" }
        style { ".lit:hover {{ background: rgb(255, 0, 0) !important }}" }
        div { "data-t": "wide", style: "width: 200px; height: 60px; overflow: auto",
            div { style: "width: 800px; height: 20px" }
        }
    }
}

#[test]
fn a_css_selector_names_an_element_as_a_handle_does() {
    let mut ui = TestUi::new(coach);
    assert_eq!(ui.attr("css=#part-0", "data-done").as_deref(), Some("true"));
    assert_eq!(ui.attr("css=.parts li:nth-child(2)", "data-done").as_deref(), Some("false"));
    assert_eq!(ui.text("css=.parts li:last-child"), "Backup");
    ui.click("css=#backdrop");
    assert_eq!(logged(), ["backdrop"]);
    let two = fails(|| _ = ui.attr("css=.parts li", "data-done"));
    assert!(two.contains("2 elements match css=.parts li: narrow the selector to one"), "{two}");
    let none = fails(|| _ = ui.text("css=#gone"));
    assert!(none.starts_with("no element matches css=#gone"), "{none}");
    let bare = fails(|| _ = ui.text("#part-0"));
    assert!(bare.starts_with("no element with data-t=\"#part-0\" (a CSS selector? css=#part-0)"), "{bare}");
}

#[test]
fn until_contains_waits_for_part_of_the_text() {
    let mut ui = TestUi::new(coach);
    ui.until_contains("note", "did not load.");
    let why = fails(|| ui.until_text("note", "The validator list did not load."));
    assert!(why.contains("to show \"The validator list did not load.\": it shows \"The validator list"), "{why}");
}

#[test]
fn a_paste_puts_the_whole_text_in_at_once() {
    let mut ui = TestUi::new(coach);
    ui.set_value("uri", "typed");
    ui.eval("addEventListener('paste', (e) => { window.pasted = e.clipboardData.getData('text/plain') }), null");
    ui.paste("uri", "data:image/png;base64,iVBOR");
    assert_eq!(ui.value("uri"), "data:image/png;base64,iVBOR", "the typed text replaced");
    assert_eq!(ui.eval("window.pasted"), json!("data:image/png;base64,iVBOR"));
    assert_eq!(ui.text("got"), "6 data:image/png;base64,iVBOR", "five typed, one pasted");
}

#[test]
fn a_shot_shows_the_hover_look_only_where_hover_put_the_pointer() {
    let mut ui = TestUi::new(coach);
    let lit = "document.querySelector('[data-t=lit]').matches(':hover')";
    ui.click("lit");
    assert_eq!(ui.eval(lit), json!(true), "the pointer rests on what it clicked");
    ui.shot();
    assert_eq!(ui.eval(lit), json!(false), "a shot moves it off first");
    ui.hover("lit");
    ui.shot();
    assert_eq!(ui.eval(lit), json!(true), "a hover is what the test asked to see");
}

#[test]
fn a_mount_without_scrollbars_gives_them_no_room() {
    let gutter =
        "(() => { const w = document.querySelector('[data-t=wide]'); return w.offsetHeight - w.clientHeight })()";
    assert!(TestUi::new(coach).eval(gutter).as_f64().unwrap() > 0.0, "the window's scrollbars");
    assert_eq!(Mount::default().no_scrollbars().app(coach).eval(gutter), json!(0), "headless Chromium's none");
}

#[test]
fn a_failure_a_test_asks_for_leaves_no_trace() {
    let dir = std::env::temp_dir().join("aio-traces");
    let why = fails(|| TestUi::new(coach).click("Gone"));
    assert!(why.starts_with("no element with data-t=\"Gone\""), "{why}");
    assert!(!why.contains("trace:"), "{why}");
    let mine = std::thread::current().name().unwrap().to_string();
    let left = std::fs::read_dir(&dir).into_iter().flatten().flatten().any(|b| b.path().join(&mine).exists());
    assert!(!left, "no folder of this test's traces");
    assert_eq!(fails(|| panic!("plain")), "plain");
}

fn side() -> View {
    let open = signal(|| false);
    rsx! {
        div { "data-t": "side", style: "height: 100px; overflow: auto",
            for i in 0..6 {
                button { key: "{i}", "data-t": "g{i}", style: "display: block; height: 40px", onclick: move |_| open.set(true), "Group {i}" }
            }
        }
        if open.get() {
            div { "data-t": "card", style: "height: 100px; overflow: auto",
                p { style: "height: 300px", "terms" }
                input { "data-t": "agree", r#type: "checkbox", aria_label: "agree", autofocus: true }
            }
        }
    }
}

/// A click scrolls as a person's does: not while part of the element shows;
/// `autofocus` scrolls its box as the browser's own does (and aio's React
/// `autoFocus`, a `focus()`): a shot of the box's top scrolls it back.
#[test]
fn a_click_on_what_partly_shows_scrolls_nothing_and_autofocus_scrolls_as_browsers_do() {
    let mut ui = TestUi::new(side);
    let top = |ui: &mut TestUi, t: &str| ui.eval(&format!("document.querySelector('[data-t={t}]').scrollTop"));
    ui.click("g2");
    assert_eq!(top(&mut ui, "side"), json!(0), "g2 shows 20 of its 40 px");
    assert_eq!(ui.focused().as_deref(), Some("agree"));
    assert!(top(&mut ui, "card").as_f64().unwrap() > 0.0, "the browser brings the focused field into view");
    ui.scroll("card", 0.0, 0.0);
    ui.click("g5");
    assert!(top(&mut ui, "side").as_f64().unwrap() > 0.0, "none of g5 showed: scrolled in, as a person must");
}

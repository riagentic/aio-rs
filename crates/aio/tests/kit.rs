//! The UI kit, driven like a person: what each control renders, says to
//! assistive tech, and does on a click or a key.
use aio::kit::*;
use aio::testing::TestUi;
use dioxus::prelude::*;
use keyboard_types::{Key, Modifiers};
use std::cell::RefCell;
use std::time::Duration;

thread_local! {
    static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn log(s: impl Into<String>) {
    LOG.with(|l| l.borrow_mut().push(s.into()));
}

/// What the handlers said since the last look.
fn said() -> Vec<String> {
    LOG.with(|l| std::mem::take(&mut *l.borrow_mut()))
}

fn styled(body: Element) -> Element {
    rsx! {
        style { {aio::ui::page_css(&[&theme_css(theme::hue("kit")), ""], false)} }
        {body}
    }
}

/// Move focus to `t` with Tab, as a keyboard user does (a click focuses
/// only some controls on some systems).
fn tab_to(ui: &mut TestUi, t: &str) {
    for _ in 0..80 {
        if ui.count(&format!("[data-t=\"{t}\"]:focus")) == 1 {
            return;
        }
        ui.press_key(Key::Tab, Modifiers::empty());
    }
    panic!("Tab never reached {t}");
}

fn press(ui: &mut TestUi, t: &str, key: Key) {
    tab_to(ui, t);
    ui.press_key(key, Modifiers::empty());
}

fn buttons_and_inputs() -> Element {
    let mut text = use_signal(String::new);
    let mut on = use_signal(|| false);
    styled(rsx! {
        Button { variant: Variant::Primary, "data-t": "save", onclick: move |_| log("save"), "Save" }
        Button { size: Size::Sm, class: "mine", "data-t": "plain", "Plain" }
        Input { "data-t": "name", value: text(), oninput: move |v| text.set(v), invalid: text().len() > 3 }
        p { "data-t": "echo", "{text}" }
        Checkbox { "data-t": "agree", label: "Email me", checked: on(), onchange: move |c| on.set(c) }
        p { "data-t": "on", "{on}" }
        Select { "data-t": "pick", options: vec!["a".into(), Opt { value: "b".into(), label: "Bee".into(), disabled: false }], value: "b" }
        Textarea { "data-t": "bio", value: "hi", rows: 5 }
    })
}

#[test]
fn a_button_has_its_classes_and_a_click_reaches_it() {
    let mut ui = TestUi::new(buttons_and_inputs);
    assert_eq!(ui.count("button[data-t=save][type=button].aio-btn.aio-btn--primary.aio-btn--md"), 1);
    assert_eq!(ui.count("[data-t=plain].aio-btn--secondary.aio-btn--sm.mine"), 1, "the caller's class is appended");
    ui.click("save");
    assert_eq!(said(), ["save"]);
}

#[test]
fn an_input_hands_over_its_value_and_says_when_it_is_invalid() {
    let mut ui = TestUi::new(buttons_and_inputs);
    ui.type_text("name", "abc");
    assert_eq!(ui.text("echo"), "abc");
    assert_eq!(ui.count("[data-t=name][aria-invalid]"), 0);
    ui.type_text("name", "d");
    assert_eq!(ui.count("[data-t=name].aio-input--invalid[aria-invalid=true]"), 1, "painted and said");
}

#[test]
fn a_checkbox_reports_its_state_and_is_named_by_its_label() {
    let mut ui = TestUi::new(buttons_and_inputs);
    assert_eq!(ui.count("label.aio-checkbox-row input[type=checkbox][aria-label='Email me']"), 1);
    ui.click("agree");
    assert_eq!(ui.text("on"), "true");
    ui.click("agree");
    assert_eq!(ui.text("on"), "false");
}

#[test]
fn a_select_lists_its_options_and_a_textarea_its_rows() {
    let ui = TestUi::new(buttons_and_inputs);
    assert_eq!(ui.count("select[data-t=pick].aio-select option"), 2);
    assert_eq!(ui.count("select[data-t=pick] option[value=b]:checked"), 1);
    assert_eq!(ui.count("textarea[data-t=bio].aio-textarea[rows='5']"), 1);
}

fn fields() -> Element {
    let mut email = use_signal(String::new);
    let error = email().is_empty().then(|| "Required".to_string());
    rsx! {
        Field { label: "Email", error, required: true, "data-t": "email-field",
            Input { "data-t": "email", value: email(), oninput: move |v| email.set(v) }
        }
        Field { label: "Name", hint: "As on your card",
            Input { "data-t": "name", id: "mine" }
        }
        Field { label: "Notify",
            Checkbox { "data-t": "notify", label: "Email me" }
        }
        Field { label: "Size",
            RadioGroup { "data-t": "size", options: vec!["s".into(), "m".into()], value: "s" }
        }
    }
}

#[test]
fn a_field_wires_its_label_error_and_required_to_its_control() {
    let mut ui = TestUi::new(fields);
    let email = "[data-t=email-field]";
    assert_eq!(
        ui.count(&format!("{email} label.aio-field__label[for$='-control']")),
        1,
        "the label targets the control"
    );
    assert_eq!(ui.count(&format!("{email} .aio-field__req")), 1);
    let input =
        format!("{email} input[id$='-control'][aria-invalid=true][aria-required=true][aria-describedby$='-error']");
    assert_eq!(ui.count(&input), 1, "the error is said, not only shown");
    assert_eq!(ui.text("email-field"), "Email *Required");
    ui.type_text("email", "a@b.c");
    assert_eq!(
        ui.count(&format!("{email} [aria-invalid]")),
        0,
        "the control follows the field, though its props didn't change"
    );
    assert_eq!(ui.count(&format!("{email} .aio-field__error")), 0);
}

#[test]
fn a_hint_describes_and_the_callers_own_say_wins() {
    let ui = TestUi::new(fields);
    assert_eq!(ui.count("[data-t=name][id=mine][aria-describedby$='-hint']"), 1);
    assert_eq!(ui.count("[data-t=name][aria-invalid]"), 0);
}

#[test]
fn no_for_onto_a_group_or_a_self_labelled_checkbox() {
    let ui = TestUi::new(fields);
    assert_eq!(ui.count("label.aio-field__label[for]"), 2, "only the two text inputs are targeted");
    assert_eq!(ui.count("[data-t=notify][aria-label='Email me']:not([id])"), 1);
    assert_eq!(ui.count("[data-t=size][role=group][aria-label=Size]"), 1, "the field names the group");
}

fn choices() -> Element {
    let mut size = use_signal(|| "s".to_string());
    let mut dark = use_signal(|| false);
    rsx! {
        RadioGroup { label: "Size", options: vec!["s".into(), "m".into(), Opt { value: "l".into(), label: "l".into(), disabled: true }],
            value: size(), onchange: move |v| size.set(v)
        }
        p { "data-t": "size", "{size}" }
        Switch { "data-t": "dark", label: "Dark mode", checked: dark(), onchange: move |v| dark.set(v) }
        p { "data-t": "dark-on", "{dark}" }
    }
}

#[test]
fn radios_share_one_name_and_each_is_named() {
    let ui = TestUi::new(choices);
    assert_eq!(ui.count("[role=group][aria-label=Size] input[type=radio][name^='aio-radio-']"), 3);
    assert_eq!(ui.count("input[type=radio][aria-label=m]"), 1);
    assert_eq!(ui.count("input[type=radio][value=s]:checked"), 1);
    assert_eq!(ui.count(".aio-radio-row[data-disabled] input[disabled]"), 1);
}

#[test]
fn a_switch_is_a_real_checkbox_and_reports_the_new_state() {
    let mut ui = TestUi::new(choices);
    assert_eq!(ui.count("input[type=checkbox][role=switch][aria-label='Dark mode'].aio-switch"), 1);
    ui.click("dark");
    assert_eq!(ui.text("dark-on"), "true");
}

fn tabs() -> Element {
    let mut on = use_signal(|| true);
    let mut tabs = vec![
        Tab::new("general settings", "General", rsx! { p { "data-t": "p1", "one" } }),
        Tab::new("b", "Beta", rsx! { p { "data-t": "p2", "two" } }).disabled(),
        Tab::new("c", "Gamma", rsx! { p { "data-t": "p3", "three" } }),
    ];
    if !on() {
        tabs.remove(0);
    }
    rsx! {
        Tabs { "data-t": "tabs", label: "Sections", tabs, onchange: log }
        Tabs { "data-t": "stale", value: "gone", tabs: vec![Tab::new("x", "X", rsx! {}), Tab::new("y", "Y", rsx! {})] }
        button { "data-t": "drop", onclick: move |_| on.set(false), "drop" }
    }
}

#[test]
fn exactly_one_tab_is_in_the_tab_order_and_the_panel_is_its() {
    let ui = TestUi::new(tabs);
    let t = "[data-t=tabs]";
    assert_eq!(ui.count(&format!("{t} [role=tablist][aria-label=Sections] [role=tab]")), 3);
    assert_eq!(ui.count(&format!("{t} [role=tab][tabindex='0']")), 1);
    assert_eq!(ui.count(&format!("{t} [role=tab][aria-selected=true][tabindex='0'][data-t='general settings']")), 1);
    assert_eq!(ui.count(&format!("{t} [role=tab][id$='-tab-general%20settings']")), 1, "a spaced id is one DOM id");
    assert_eq!(ui.count(&format!("{t} [role=tabpanel][aria-labelledby$='-tab-general%20settings'][tabindex='0']")), 1);
    assert_eq!(ui.count(&format!("{t} [role=tab][aria-controls$='-panel']")), 3, "every tab names the one panel");
    assert!(ui.present("p1") && ui.absent("p3"), "only the active panel renders");
}

#[test]
fn arrows_move_home_and_end_jump_and_disabled_tabs_are_skipped() {
    // One key from tab `from` (made active by a click), on a fresh page.
    let step = |from: &str, key: Key| {
        let mut ui = TestUi::new(tabs);
        ui.click(from);
        said();
        press(&mut ui, from, key);
        (said(), ui.present("p3"))
    };
    assert_eq!(step("general settings", Key::ArrowRight), (vec!["c".into()], true), "Beta is disabled");
    assert_eq!(step("general settings", Key::ArrowDown).0, ["c"]);
    assert_eq!(step("c", Key::ArrowRight).0, ["general settings"], "wraps");
    assert_eq!(step("general settings", Key::ArrowLeft).0, ["c"], "wraps back");
    assert_eq!(step("general settings", Key::End).0, ["c"]);
    assert_eq!(step("c", Key::Home).0, ["general settings"]);
    let mut ui = TestUi::new(tabs);
    ui.click("b");
    assert_eq!(said(), Vec::<String>::new(), "a disabled tab can't be chosen");
}

fn mirrored() -> Element {
    let mut rtl = use_signal(|| true);
    let three = |p: &str| ["a", "b", "c"].map(|t| Tab::new(format!("{p}{t}"), t, rsx! {})).to_vec();
    rsx! {
        Direction { rtl: rtl(),
            Tabs { "data-t": "read", tabs: three(""), onchange: log }
            Tabs { "data-t": "said", rtl: false, tabs: three("x"), onchange: log }
        }
        button { "data-t": "ltr", onclick: move |_| rtl.set(false), "ltr" }
    }
}

#[test]
fn tabs_mirror_in_a_right_to_left_part_of_the_page() {
    // aio tests: Tabs reads the closest [dir].
    let mut ui = TestUi::new(mirrored);
    assert_eq!(ui.count("div[dir=rtl] [data-t=read]"), 1);
    press(&mut ui, "a", Key::ArrowLeft);
    assert_eq!(said(), ["b"], "ArrowLeft is forward right to left");
    ui.click("ltr");
    assert_eq!(ui.count("div[dir=ltr] [data-t=read]"), 1);
    press(&mut ui, "b", Key::ArrowLeft);
    assert_eq!(said(), ["a"], "it follows a change of direction");
    let mut ui = TestUi::new(mirrored);
    press(&mut ui, "xa", Key::ArrowLeft);
    assert_eq!(said(), ["xc"], "a tab list's own rtl wins");
}

#[test]
fn a_tab_that_is_gone_falls_back_to_the_first_enabled_one() {
    let mut ui = TestUi::new(tabs);
    press(&mut ui, "general settings", Key::End);
    ui.click("drop");
    assert_eq!(ui.count("[data-t=tabs] [role=tab][tabindex='0'][data-t=c]"), 1);
    assert!(ui.present("p3"));
    assert_eq!(
        ui.count("[data-t=stale] [role=tab][tabindex='0'][data-t=x]"),
        1,
        "a stale controlled value keeps a tab stop"
    );
}

// Unstyled: the menu as markup, its placement not under test.
fn menu() -> Element {
    rsx! {
        Menu { "data-t": "actions", label: "Actions", trigger: rsx! { "⋯" }, onselect: log,
            items: vec![MenuItem::new("edit", "Edit"), MenuItem::new("lock", "Lock").disabled(), MenuItem::new("delete", "Delete").danger()]
        }
    }
}

#[test]
fn a_menu_is_closed_and_says_so() {
    let ui = TestUi::new(menu);
    assert_eq!(ui.count("[role=menu]"), 0);
    assert_eq!(ui.count("[data-t=menuTrigger][aria-haspopup=menu][aria-expanded=false]:not([aria-controls])"), 1);
}

#[test]
fn a_click_opens_it_and_escape_and_tab_close_it() {
    let mut ui = TestUi::new(menu);
    ui.click("menuTrigger");
    assert_eq!(ui.count("[role=menu][aria-label=Actions] [role=menuitem][tabindex='-1']"), 3);
    assert_eq!(
        ui.count("[data-t=menuTrigger][aria-expanded=true][aria-controls^='aio-menu-']"),
        1,
        "names the list while it exists"
    );
    assert_eq!(ui.count("[data-t=delete].aio-menu__item--danger"), 1);
    press(&mut ui, "menuTrigger", Key::Escape);
    assert_eq!(ui.count("[role=menu]"), 0);
    press(&mut ui, "menuTrigger", Key::ArrowDown);
    assert_eq!(ui.count("[role=menu]"), 1, "ArrowDown opens it");
    press(&mut ui, "menuTrigger", Key::Tab);
    assert_eq!(ui.count("[role=menu]"), 0, "Tab leaves");
    ui.click("menuTrigger");
    ui.click("menuTrigger");
    assert_eq!(ui.count("[role=menu]"), 0, "the trigger toggles it");
}

#[test]
fn the_arrows_carry_focus_through_the_items_and_escape_brings_it_back() {
    let mut ui = TestUi::new(menu);
    press(&mut ui, "menuTrigger", Key::ArrowDown);
    assert_eq!(ui.focused().as_deref(), Some("edit"), "opened on the first item");
    ui.press_key(Key::ArrowDown, Modifiers::empty());
    assert_eq!(ui.focused().as_deref(), Some("delete"), "the disabled one is skipped");
    ui.press_key(Key::Home, Modifiers::empty());
    assert_eq!(ui.focused().as_deref(), Some("edit"));
    ui.press_key(Key::Escape, Modifiers::empty());
    assert_eq!((ui.count("[role=menu]"), ui.focused().as_deref()), (0, Some("menuTrigger")), "closed, focus back");
    press(&mut ui, "menuTrigger", Key::ArrowUp);
    assert_eq!(ui.focused().as_deref(), Some("delete"), "ArrowUp opens on the last");
}

#[test]
fn a_click_outside_closes_it_and_leaves_focus_where_it_lands() {
    // The scrim covers the window (the kit's `position: fixed; inset: 0`, under the list).
    let mut ui = TestUi::new(|| styled(menu()));
    press(&mut ui, "menuTrigger", Key::ArrowDown);
    ui.click("menuScrim");
    assert_eq!(ui.count("[role=menu]"), 0);
    assert_ne!(ui.focused().as_deref(), Some("menuTrigger"), "focus isn't pulled back, unlike Escape");
    assert_eq!(said(), Vec::<String>::new(), "nothing chosen");
}

#[test]
fn the_tab_arrows_move_focus_with_the_choice() {
    let mut ui = TestUi::new(tabs);
    press(&mut ui, "general settings", Key::ArrowRight);
    assert_eq!(ui.focused().as_deref(), Some("c"));
}

#[test]
fn choosing_reports_the_id_and_a_disabled_item_does_nothing() {
    let mut ui = TestUi::new(menu);
    ui.click("menuTrigger");
    ui.click("lock");
    assert_eq!(said(), Vec::<String>::new());
    ui.click("edit");
    assert_eq!(said(), ["edit"]);
    assert_eq!(ui.count("[role=menu]"), 0, "choosing closes it");
}

fn statuses() -> Element {
    let mut shown = use_signal(|| true);
    styled(rsx! {
        div { "data-t": "half-row", Progress { "data-t": "half", value: 42.0, label: "Upload", show_value: true } }
        Progress { "data-t": "busy", label: "Working" }
        Progress { "data-t": "nan", value: f64::NAN, show_value: true }
        div { "data-t": "over-row", Progress { "data-t": "over", value: 150.0, show_value: true } }
        Alert { "data-t": "saved", tone: Tone::Success, title: "Saved", "All good" }
        if shown() {
            Alert { "data-t": "failed", tone: Tone::Error, ondismiss: move |_| shown.set(false), "Broke" }
        }
        Tooltip { "data-t": "tip", text: "Copies the link",
            Button { "data-t": "copy", "Copy" }
        }
        Tooltip { "data-t": "tip2", text: "Your address", bubble_id: "mail-tip",
            Field { label: "Mail", hint: "Never shared", Input { "data-t": "mail" } }
            button { "data-t": "plain", "aria-describedby": "mail-tip", "?" }
        }
        Breadcrumb { "data-t": "crumbs", items: vec![Crumb::new("Home").href("/"), Crumb::new("Docs").href("/docs"), Crumb::new("Kit")] }
        Skeleton { "data-t": "one", width: "4em" }
        Skeleton { "data-t": "three", lines: 3 }
        EmptyState { "data-t": "empty", icon: "📭", title: "No mail", description: "Nothing arrived yet.",
            action: rsx! { Button { "Refresh" } }
        }
    })
}

#[test]
fn progress_carries_a_value_or_none_and_its_label_never_contradicts_the_bar() {
    let ui = TestUi::new(statuses);
    assert_eq!(ui.count("progress[data-t=half][value='42'][max='100'][aria-label=Upload]"), 1);
    assert_eq!(ui.count("progress[data-t=busy]:not([value])"), 1, "indeterminate");
    assert_eq!(ui.count("progress[data-t=nan]:not([value])"), 1, "a non-finite value degrades to indeterminate");
    assert_eq!(ui.count(".aio-progress__value"), 2, "no number beside an unknown");
    let shown: Vec<String> = ["half", "over"].map(|t| ui.text(&format!("{t}-row"))).into();
    assert_eq!(shown, ["42%", "100%"], "the label clamps as the bar does");
}

#[test]
fn only_an_error_interrupts_and_dismiss_exists_only_when_it_does_something() {
    let mut ui = TestUi::new(statuses);
    assert_eq!(ui.count("[data-t=saved][role=note].aio-alert--success .aio-alert__title"), 1);
    assert_eq!(ui.count("[data-t=saved] .aio-alert__x"), 0);
    assert_eq!(ui.count("[data-t=failed][role=alert]"), 1);
    ui.click("alertDismiss");
    assert!(ui.absent("failed"));
}

#[test]
fn a_tooltip_shows_on_hover_and_escape_puts_it_away() {
    let mut ui = TestUi::new(statuses);
    let bubble = ui.attr("copy", "aria-describedby").expect("the control is described, not its wrapper");
    assert_eq!(ui.count(&format!("[data-t=tip] [role=tooltip][id^='aio-tip-'][id='{bubble}']")), 1);
    assert_eq!(ui.count("[data-t=tip] .aio-tip__trigger[aria-describedby]"), 0);
    assert_eq!(ui.count("[data-t=tip2] [role=tooltip]#mail-tip"), 1);
    assert!(
        ui.attr("mail", "aria-describedby").is_some_and(|d| d.ends_with("-hint mail-tip")),
        "the hint, then the tip"
    );
    assert_eq!(ui.count("[data-t=tip][data-hidden]"), 0);
    press(&mut ui, "copy", Key::Escape);
    assert_eq!(ui.count("[data-t=tip][data-hidden]"), 1);
}

#[test]
fn the_current_crumb_is_no_link_and_skeletons_are_not_announced() {
    let ui = TestUi::new(statuses);
    assert_eq!(ui.count("nav[data-t=crumbs][aria-label=Breadcrumb] a"), 2);
    assert_eq!(ui.count("[data-t=crumbs] li:last-child span[aria-current=page]"), 1);
    assert_eq!(ui.count("[data-t=crumbs] .aio-crumbs__sep[aria-hidden=true]"), 2);
    assert_eq!(
        ui.count("span[data-t=one].aio-skel[aria-hidden=true][style*='width']"),
        1,
        "one line is the component, attributes and all"
    );
    assert_eq!(ui.count("[data-t=three][aria-hidden=true] .aio-skel"), 3);
    assert_eq!(ui.text("empty"), "📭No mailNothing arrived yet.Refresh");
    assert_eq!(ui.count("[data-t=empty] .aio-empty__icon[aria-hidden=true]"), 1);
}

#[derive(Clone, PartialEq)]
struct Person {
    name: &'static str,
    age: u32,
}

fn data() -> Element {
    let mut page = use_signal(|| 1u32);
    let mut stale = use_signal(|| 9u32);
    let rows = vec![Person { name: "Ada", age: 36 }, Person { name: "Alan", age: 41 }];
    let cols = vec![
        Column::text("Name", |p: &Person| p.name.into()),
        Column::view("Age", |p: &Person, _| rsx! { input { "data-t": "age-{p.name}", value: "{p.age}" } })
            .align(Align::End),
    ];
    rsx! {
        Table { "data-t": "people", columns: cols.clone(), rows, key_of: |p: Person| p.name.to_string(), onrow: move |(p, i): (Person, usize)| log(format!("{} {i}", p.name)) }
        Table { "data-t": "nobody", columns: Vec::<Column<Person>>::new(), rows: vec![], empty: rsx! { "Nobody yet" } }
        Pagination { "data-t": "pager", page: page(), pages: 10, onpage: move |p| page.set(p) }
        Pagination { "data-t": "stale", page: stale(), pages: 3, onpage: move |p| stale.set(p) }
        p { "data-t": "stale-page", "{stale}" }
        Card { "data-t": "card", title: rsx! { "Title" }, footer: rsx! { "Foot" }, "Body" }
        Stack { "data-t": "stack", gap: "4px", "a" }
        Row { "data-t": "row", "b" }
        Spinner { "data-t": "spin" }
        Avatar { "data-t": "ada", name: "Ada Lovelace" }
        Avatar { "data-t": "pic", name: "Ada", src: "a.png", size: 48 }
    }
}

#[test]
fn a_table_renders_its_columns_and_a_row_opens_by_click_or_key() {
    let mut ui = TestUi::new(data);
    assert_eq!(ui.count("[data-t=people] th.aio-th"), 2);
    assert_eq!(ui.count("[data-t=people] tbody tr.aio-tr--click[tabindex='0']"), 2);
    assert_eq!(ui.count("[data-t=people] td[style*='text-align:end'] input"), 2);
    assert!(ui.text("people").contains("Ada") && ui.text("people").contains("Alan"));
    ui.click("age-Alan");
    assert_eq!(said(), ["Alan 1"]);
    press(&mut ui, "age-Ada", Key::Enter);
    assert_eq!(said(), Vec::<String>::new(), "Enter in a cell's input is the input's");
    assert_eq!(ui.count("[data-t=nobody] td.aio-table__empty[colspan='1']"), 1, "the empty row spans a column");
    assert_eq!(ui.text("nobody"), "Nobody yet");
}

#[test]
fn the_pager_windows_marks_the_current_page_and_clamps() {
    let mut ui = TestUi::new(data);
    let p = "[data-t=pager]";
    assert_eq!(ui.count(&format!("nav{p}[aria-label=Pagination] [aria-label^='Page ']")), 5);
    assert_eq!(ui.count(&format!("{p} [aria-current=page][aria-label='Page 1'].aio-page__btn--current")), 1);
    assert_eq!(ui.count(&format!("{p} [data-t=pagePrev][disabled]")), 1);
    ui.click("pageNext");
    ui.click("page3");
    assert_eq!(ui.count(&format!("{p} [aria-current=page][aria-label='Page 3']")), 1);
    assert_eq!(ui.count(&format!("{p} [aria-label^='Page ']")), 5);
    // Out of range (page 9 of 3): every enabled button leads back in.
    assert_eq!(ui.count("[data-t=stale] [data-t=pagePrev]:not([disabled])"), 1);
}

#[test]
fn cards_stacks_spinners_and_avatars() {
    let ui = TestUi::new(data);
    assert_eq!(ui.count("[data-t=card] .aio-card__title + .aio-card__body + .aio-card__footer"), 1);
    assert_eq!(ui.count("[data-t=stack][style*='flex-direction:column'][style*='gap:4px']"), 1);
    assert_eq!(ui.count("[data-t=row][style*='flex-wrap:wrap']"), 1);
    assert_eq!(ui.count("[data-t=spin][role=status][aria-label=Loading]"), 1);
    assert_eq!(ui.text("ada"), "AL");
    assert_eq!(ui.count("[data-t=ada][role=img][aria-label='Ada Lovelace'][style*='hsl(']"), 1);
    assert_eq!(ui.count("[data-t=pic] img.aio-avatar__img[alt=Ada][width='48']"), 1);
}

fn dialogs() -> Element {
    let mut a = use_signal(|| false);
    let mut b = use_signal(|| false);
    let mut stuck = use_signal(|| false);
    // Only room around each dialog: the headless layout misplaces the kit's fixed overlay.
    rsx! {
        style { ".aio-modal-backdrop{{padding:20px}}" }
        button { "data-t": "open-a", onclick: move |_| a.set(true), "a" }
        Modal { open: a(), title: "Settings", onclose: move |_| { log("close a"); a.set(false) },
            button { "data-t": "in-a", onclick: move |_| b.set(true), "open b" }
            button { "data-t": "stuck", onclick: move |_| stuck.set(true), "stuck" }
        }
        Modal { open: b(), title: "Sure?", "data-t": "b-backdrop", onclose: move |_| { log("close b"); b.set(false) },
            button { "data-t": "in-b", "b" }
        }
        Modal { open: stuck(), dismissable: false, onclose: move |_| log("close stuck"),
            button { "data-t": "in-stuck", "saving" }
        }
    }
}

#[test]
fn a_modal_renders_only_when_open_and_its_backdrop_closes_it() {
    let mut ui = TestUi::new(dialogs);
    assert_eq!(ui.count("[role=dialog]"), 0);
    ui.click("open-a");
    assert_eq!(
        ui.count(
            ".aio-modal-backdrop[role=presentation] .aio-modal[role=dialog][aria-modal=true][aria-labelledby$='-title']"
        ),
        1
    );
    ui.click("in-a");
    ui.click("in-b");
    assert_eq!(said(), Vec::<String>::new(), "a click inside a dialog is not a backdrop click");
    ui.click_at("b-backdrop", 0.01, 0.01);
    assert_eq!(said(), ["close b"], "the caller's handle names the backdrop");
    ui.click_at("modalBackdrop", 0.01, 0.01);
    assert_eq!(said(), ["close a"]);
}

#[test]
fn escape_closes_the_most_recently_opened_dialog_only() {
    let mut ui = TestUi::new(dialogs);
    ui.click("open-a");
    ui.click("in-a");
    // The dialog on top holds the keyboard (the page's focus trap).
    assert_eq!(ui.focused().as_deref(), Some("in-b"));
    ui.press_key(Key::Escape, Modifiers::empty());
    assert_eq!(said(), ["close b"], "the one on top goes");
    assert_eq!(ui.focused().as_deref(), Some("in-a"), "focus back where it was before it opened");
    ui.press_key(Key::Escape, Modifiers::empty());
    assert_eq!(said(), ["close a"]);
}

#[test]
fn a_dialog_that_cannot_be_dismissed_blocks_escape() {
    let mut ui = TestUi::new(dialogs);
    ui.click("open-a");
    ui.click("stuck");
    assert_eq!(ui.focused().as_deref(), Some("in-stuck"));
    ui.press_key(Key::Escape, Modifiers::empty());
    assert_eq!(said(), Vec::<String>::new(), "the one behind doesn't close either");
}

// Unstyled: the headless layout misplaces a styled dialog's footer.
fn confirming() -> Element {
    rsx! {
        ConfirmButton { "data-t": "delete", variant: Variant::Danger, confirm: "Delete it?", onconfirm: move |_| log("deleted"), "Delete" }
    }
}

#[test]
fn a_confirm_button_acts_only_once_confirmed() {
    let mut ui = TestUi::new(confirming);
    assert_eq!(ui.count("button[data-t=delete].aio-btn--danger"), 1);
    ui.click("delete");
    assert_eq!(ui.count("[role=dialog] .aio-confirm__msg"), 1);
    assert_eq!(ui.count("[data-t=confirmOk].aio-btn--danger"), 1, "a danger button confirms with danger");
    ui.click("confirmCancel");
    assert_eq!((said(), ui.count("[role=dialog]")), (vec![], 0));
    ui.click("delete");
    ui.click("confirmOk");
    assert_eq!((said(), ui.count("[role=dialog]")), (vec!["deleted".to_string()], 0));
}

fn toaster() -> Element {
    rsx! {
        ToastHost { "data-t": "toasts" }
        button { "data-t": "ok", onclick: move |_| _ = toast_for("Saved", Tone::Success, None), "ok" }
        button { "data-t": "bad", onclick: move |_| _ = toast_for("Failed", Tone::Error, None), "bad" }
        button { "data-t": "brief", onclick: move |_| _ = toast_for("Brief", Tone::Info, Some(std::time::Duration::from_millis(300))), "brief" }
    }
}

#[test]
fn a_toast_shows_in_the_host_until_dismissed_or_its_time_is_up() {
    let mut ui = TestUi::new(toaster);
    assert_eq!(ui.count("[data-t=toasts][role=status][aria-live=polite] .aio-toast"), 0);
    ui.click("ok");
    ui.click("bad");
    assert_eq!(ui.count(".aio-toast--success:not([role])"), 1);
    assert_eq!(ui.count(".aio-toast--error[role=alert]"), 1);
    ui.click("toastDismiss");
    assert_eq!(ui.text("toasts"), "Failed×");
    ui.click("brief");
    assert!(ui.text("toasts").contains("Brief"));
    ui.until("its timer dismisses it", || aio::kit::toasts() == ["Failed"]);
    assert_eq!(ui.text("toasts"), "Failed×");
}

fn docs() -> Element {
    rsx! {
        Markdown { "data-t": "md", source: "# Hi\n\nSee [site](https://e.com) and [home](/h). <script>alert(1)</script> [x](javascript:alert(1))\n\n- a\n- b" }
    }
}

#[test]
fn markdown_renders_elements_never_live_html() {
    let ui = TestUi::new(docs);
    assert_eq!(ui.count(".aio-md h1"), 1);
    assert_eq!(ui.count(".aio-md a.aio-md__a[target=_blank][rel='noopener noreferrer'][href='https://e.com']"), 1);
    assert_eq!(ui.count(".aio-md a[href='/h']:not([target])"), 1);
    assert_eq!(ui.count(".aio-md script"), 0, "raw HTML is text");
    assert_eq!(ui.count(".aio-md a"), 2, "the javascript: link is dropped");
    assert_eq!(ui.count(".aio-md ul.aio-md__list li"), 2);
}

fn viewport() -> Element {
    rsx! {
        ZoomPan { "data-t": "zp", onview: move |v: ZoomView| log(format!("{}", v.scale)),
            div { "data-t": "content", onclick: |_| log("content"), "map" }
        }
    }
}

#[test]
fn the_zoom_buttons_zoom_and_a_click_still_reaches_the_content() {
    let mut ui = TestUi::new(viewport);
    assert_eq!(ui.count("[data-t=zoomStage][style*='scale(1)']"), 1);
    ui.click("zoomIn");
    assert_eq!(said(), ["1.25"]);
    assert_eq!(ui.count("[data-t=zoomStage][style*='scale(1.25)']"), 1);
    ui.click("zoomOut");
    assert_eq!(said(), ["1"]);
    ui.click("content");
    assert_eq!(said(), ["content"], "a plain click is not a drag");
}

fn camera() -> Element {
    rsx! {
        ZoomPan { "data-t": "zp", style: "width: 400px; height: 300px",
            onview: move |v: ZoomView| log(format!("{:.1} {:.1} {:.3}", v.x, v.y, v.scale)),
            div { "data-t": "content", style: "height: 100px", onclick: |_| log("content"), "map" }
        }
    }
}

#[test]
fn the_wheel_zooms_at_the_cursor_and_a_drag_pans_without_clicking() {
    let mut ui = TestUi::new(camera);
    ui.wheel("zp", 0.0, -120.0);
    let k = (120.0f64 * 0.0015).exp();
    let want = zoom_at(ZoomView::default(), k, (200.0, 150.0), (0.1, 8.0));
    assert_eq!(said(), [format!("{:.1} {:.1} {:.3}", want.x, want.y, want.scale)], "the centre stays put");
    ui.click("zoomOut");
    said();
    let mut ui = TestUi::new(|| styled(camera()));
    ui.drag_by("content", 50.0, 20.0);
    assert_eq!(said(), ["50.0 20.0 1.000"], "panned, and the release is no click on the content");
    ui.drag_by("zp", 2.0, 0.0);
    assert!(said().is_empty(), "inside the slop a press is a click, not a pan");
}

#[test]
fn a_pan_follows_the_pointer_out_of_the_box_and_ends_there() {
    // aio's pointer capture: the box is 400x300 at the window's corner.
    let mut ui = TestUi::new(|| styled(camera()));
    ui.drag_along("content", &[(10.0, 0.0), (600.0, 100.0)]);
    assert_eq!(said(), ["10.0 0.0 1.000", "600.0 100.0 1.000"], "moves outside the box still pan");
    assert_eq!(ui.count(".aio-zoompan__capture"), 0, "released outside: the drag is over");
    ui.click("content");
    assert_eq!(said(), ["content"], "and a click reaches the content again");
}

#[test]
fn fit_shows_the_content_whole_and_centred() {
    let mut ui = TestUi::new(camera);
    ui.click("zoomFit");
    let want = fit_view((400.0, 100.0), (400.0, 300.0), 16.0, (0.1, 8.0));
    assert_eq!(said(), [format!("{:.1} {:.1} {:.3}", want.x, want.y, want.scale)], "measured from the laid-out boxes");
}

/// Cameras that say what they do, opening in 10 ms.
fn cameras() -> &'static aio::held::Held<&'static str, String> {
    static CAMS: std::sync::OnceLock<aio::held::Held<&'static str, String>> = std::sync::OnceLock::new();
    CAMS.get_or_init(|| {
        let said = |s: String| SAID.lock().unwrap().push(s);
        aio::held::Held::new(
            move |k| async move {
                said(format!("open {k}"));
                tokio::time::sleep(Duration::from_millis(10)).await;
                Ok::<_, String>(format!("cam {k}"))
            },
            move |v, _| async move { said(format!("close {v}")) },
        )
    })
}

/// What the cameras did (they run off the view's thread).
static SAID: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn camera_view() -> Element {
    let mut key = use_signal(|| Some("front"));
    let mut shown = use_signal(|| true);
    rsx! {
        button { "data-t": "back", onclick: move |_| key.set(Some("back")), "back" }
        button { "data-t": "off", onclick: move |_| key.set(None), "off" }
        button { "data-t": "hide", onclick: move |_| shown.set(false), "hide" }
        if shown() {
            Preview { cam: key() }
        }
    }
}

#[component]
fn Preview(cam: Option<&'static str>) -> Element {
    let cam = use_held(cameras(), cam);
    let text = match &cam {
        None => "none".to_string(),
        Some(aio::held::State::Open(v)) => v.to_string(),
        Some(_) => "…".to_string(),
    };
    rsx! { p { "data-t": "preview", "{text}" } }
}

#[test]
fn a_view_holds_one_camera_per_key_and_lets_go_on_a_change_and_at_unmount() {
    let mut ui = TestUi::new(camera_view);
    ui.wait(Duration::from_millis(30));
    assert_eq!(ui.text("preview"), "cam front");
    ui.click("back");
    ui.wait(Duration::from_millis(30));
    assert_eq!(ui.text("preview"), "cam back");
    ui.click("off");
    ui.wait(Duration::from_millis(5));
    assert_eq!(ui.text("preview"), "none");
    assert_eq!(cameras().len(), 0, "nothing held without a key");
    ui.click("back");
    ui.wait(Duration::from_millis(30));
    ui.click("hide");
    ui.wait(Duration::from_millis(5));
    assert_eq!(
        *SAID.lock().unwrap(),
        ["open front", "close cam front", "open back", "close cam back", "open back", "close cam back"],
        "one open per key across re-renders, the old closed before the new opens, closed at unmount"
    );
    assert_eq!(cameras().len(), 0);
}

fn screens() -> Element {
    let mut screen = use_signal(|| 2);
    let mut dialog = use_signal(|| true);
    use_back_button(move || {
        if screen() == 0 {
            return false;
        }
        screen -= 1;
        true
    });
    rsx! {
        p { "data-t": "screen", "{screen}" }
        if dialog() {
            Dialog { close: move |_| dialog.set(false) }
        }
    }
}

#[component]
fn Dialog(close: EventHandler) -> Element {
    use_back_button(move || {
        close.call(());
        true
    });
    rsx! { div { "data-t": "dialog" } }
}

#[test]
fn back_closes_the_dialog_first_then_goes_up_then_leaves() {
    use futures_util::FutureExt;
    let mut ui = TestUi::new(screens);
    let back = |ui: &mut TestUi| {
        let asked = aio::ui::back_pressed();
        ui.settle();
        asked.now_or_never().expect("answered by the views")
    };
    assert!(back(&mut ui));
    assert!(ui.absent("dialog"), "the dialog, mounted last, took it");
    assert_eq!(ui.text("screen"), "2");
    assert!(back(&mut ui) && back(&mut ui));
    assert_eq!(ui.text("screen"), "0");
    assert!(!back(&mut ui), "at home nobody takes it: the app leaves");
}

fn window_app() -> Element {
    let mut on = use_signal(|| true);
    rsx! {
        GlobalKeys { style: "height: 300px",
            button { "data-t": "stop", onclick: move |e| e.stop_propagation(), "stop" }
            button { "data-t": "unmount", onclick: move |_| on.set(false), "unmount" }
            div { "data-t": "box", style: "width: 50px; height: 50px" }
            if on() {
                Tracker {}
            }
        }
    }
}

#[component]
fn Tracker() -> Element {
    let mut n = use_signal(|| 0);
    use_global_key("ctrl+s", |_| log("saved"));
    use_window_event(move |e| {
        n += 1;
        log(match e {
            WindowEvent::MouseMove(_) => format!("move {n}"),
            WindowEvent::KeyDown(k) => format!("key {}", k.key()),
            WindowEvent::Resize { width, height } => format!("size {width}x{height}"),
            WindowEvent::Click(_) => "click".into(),
            _ => return,
        })
    });
    rsx! { p { "data-t": "n", "{n}" } }
}

#[test]
fn window_listeners_hear_what_reaches_the_root_with_the_latest_render_until_unmounted() {
    // aio tests/on-window-event.test.tsx.
    let mut ui = TestUi::new(|| styled(window_app()));
    said();
    ui.hover("box");
    ui.wait(Duration::from_millis(MOVE_MS as u64 + 50));
    ui.hover("n");
    ui.wait(Duration::from_millis(MOVE_MS as u64 + 50));
    // The browser may move the pointer of its own once a page is laid out
    // (its hover upkeep): each move counted, by the latest render's count.
    let moves = said();
    let n: Vec<u32> = moves.iter().filter_map(|m| m.strip_prefix("move ")?.parse().ok()).collect();
    assert!(
        n.len() == moves.len() && n.len() >= 2 && n.windows(2).all(|w| w[1] == w[0] + 1),
        "anywhere in the window; the latest render's state: {moves:?}"
    );
    ui.press_key(Key::Character("x".into()), Modifiers::empty());
    ui.press_key(Key::Character("s".into()), Modifiers::CONTROL);
    assert_eq!(said(), ["key x", "saved"], "a key a global binding took doesn't reach the window");
    ui.resize(640, 480);
    assert_eq!(said(), ["size 640x300"], "the window's new size, as the root is laid out");
    ui.click("stop");
    assert!(said().iter().all(|s| s != "click"), "an event stopped on its way up never reaches the window");
    ui.click("unmount");
    assert_eq!(said().last().map(String::as_str), Some("click"));
    ui.hover("box");
    ui.click("box");
    assert!(said().is_empty(), "unmounted: nothing listens any more");
}

#[test]
fn the_pointers_moves_reach_the_window_at_most_once_per_move_ms_the_last_too() {
    let mut ui = TestUi::new(|| styled(window_app()));
    ui.hover("box");
    ui.wait(Duration::from_millis(MOVE_MS as u64 * 2));
    said();
    let t = std::time::Instant::now();
    for i in 0..30 {
        ui.hover(if i % 2 == 0 { "n" } else { "box" });
    }
    ui.hover("n");
    ui.wait(Duration::from_millis(MOVE_MS as u64 * 2));
    let (moves, took) = (said().into_iter().filter(|m| m.starts_with("move")).count(), t.elapsed());
    let most = (took.as_millis() / MOVE_MS as u128) as usize + 2;
    assert!((1..=most).contains(&moves), "{moves} moves told for 31 in {took:?} (at most {most})");
    assert_eq!(ui.eval("document.elementFromPoint(1, 1) !== null"), true);
}

#[component]
fn Repeats() -> Element {
    use_global_key("ctrl+s", |_| log("save"));
    use_global_key("ArrowDown", |_| log("down"));
    use_global_key(Chord::parse("ctrl+z").unwrap().repeating(true), |_| log("undo"));
    rsx! { p { "keys" } }
}

fn repeats() -> Element {
    rsx! {
        GlobalKeys { style: "height: 100px", Repeats {} }
    }
}

#[test]
fn a_held_keys_repeats_fire_only_movement_and_bindings_that_say_so() {
    // aio key-handler REPEATABLE_KEYS: a held shortcut fires once.
    let mut ui = TestUi::new(|| styled(repeats()));
    said();
    let send = |key: &str, ctrl: bool, repeat: bool| {
        format!(
            "document.querySelector('[data-aio-keys]').dispatchEvent(new KeyboardEvent('keydown', \
             {{ key: '{key}', code: 'Key{}', ctrlKey: {ctrl}, repeat: {repeat}, bubbles: true }}))",
            key.to_uppercase()
        )
    };
    for (key, ctrl) in [("s", true), ("ArrowDown", false), ("z", true)] {
        ui.eval(&send(key, ctrl, false));
        for _ in 0..3 {
            ui.eval(&send(key, ctrl, true));
        }
    }
    ui.settle();
    let n = |w: &str, got: &[String]| got.iter().filter(|s| *s == w).count();
    let got = said();
    assert_eq!((n("save", &got), n("down", &got), n("undo", &got)), (1, 4, 4), "{got:?}");
}

fn forms() -> Element {
    let mut sent = use_signal(|| 0);
    let mut name = use_signal(String::new);
    rsx! {
        p { "data-t": "sent", "{sent}" }
        form { onsubmit: move |e| { e.prevent_default(); sent += 1 },
            input { "data-t": "to" }
            input { "data-t": "amount" }
        }
        form { onsubmit: move |e| { e.prevent_default(); if !name().is_empty() { sent += 10 } },
            input { "data-t": "name", value: "{name}", oninput: move |e| name.set(e.value()) }
            button { r#type: "submit", disabled: name().is_empty(), "Create" }
        }
    }
}

#[test]
fn enter_submits_a_form_with_no_submit_button_and_one_whose_button_the_view_is_enabling() {
    let mut ui = TestUi::new(forms);
    ui.type_text("to", "alice");
    ui.press("amount", "Enter");
    assert_eq!(ui.text("sent"), "1", "two fields, no button: aio requestSubmit");
    // Typed, and Enter at once: the view hasn't enabled the button yet.
    ui.eval(
        "const f = document.querySelector('[data-t=name]'); f.focus(); f.value = 'v'; \
         f.dispatchEvent(new Event('input', { bubbles: true })); \
         f.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true }))",
    );
    ui.settle();
    assert_eq!(ui.text("sent"), "11", "submitted once the view's answer enabled the button");
    ui.press("name", "Enter");
    assert_eq!(ui.text("sent"), "21", "an enabled button: the browser's own submit");
}

fn portals() -> Element {
    let mut open = use_signal(|| false);
    let mut menu = use_signal(|| true);
    let mut n = use_signal(|| 0);
    rsx! {
        PortalHost {
            div { "data-t": "clip", style: "overflow: hidden; height: 10px",
                button { "data-t": "open", onclick: move |_| open.toggle(), "open" }
                button { "data-t": "menu", onclick: move |_| menu.toggle(), "menu" }
                if open() {
                    Portal {
                        div { "data-t": "dialog",
                            button { "data-t": "inc", onclick: move |_| n += 1, "{n}" }
                            if menu() {
                                Portal { ul { "data-t": "dropdown", "menu" } }
                            }
                            Counter {}
                        }
                    }
                }
                Portal { disabled: true, i { "data-t": "inline", "here" } }
            }
        }
    }
}

#[component]
fn Counter() -> Element {
    let mut c = use_signal(|| 0);
    rsx! { button { "data-t": "own", onclick: move |_| c += 1, "own {c}" } }
}

#[test]
fn a_portal_draws_at_the_root_keeps_its_state_and_handlers_and_leaves_nothing() {
    let mut ui = TestUi::new(portals);
    assert_eq!(ui.count("[data-t=clip] [data-t=inline]"), 1, "disabled: in place");
    ui.click("open");
    assert_eq!(ui.count("[data-t=clip] [data-t=dialog]"), 0, "out of the clipping box");
    assert_eq!(ui.count(".aio-portals > [data-t=dialog] + [data-t=dropdown]"), 1, "the inner one after, not inside");
    ui.click("inc");
    ui.click("own");
    assert_eq!((ui.text("inc"), ui.text("own")), ("1".into(), "own 1".into()), "its handlers and state work there");
    ui.click("menu");
    assert!(ui.absent("dropdown"));
    ui.click("menu");
    ui.click("inc");
    assert_eq!((ui.text("inc"), ui.text("own")), ("2".into(), "own 1".into()), "a re-render keeps the inner state");
    ui.click("open");
    assert_eq!(ui.count(".aio-portals > *"), 0, "closed: nothing left behind");
}

fn fading() -> Element {
    let mut show = use_signal(|| true);
    rsx! {
        button { "data-t": "toggle", onclick: move |_| show.toggle(), "toggle" }
        Transition { show: show(), enter: Motion::FADE, exit: Motion::FADE.ms(100), "data-t": "fx",
            p { "data-t": "msg", "hello" }
        }
    }
}

#[test]
fn a_transition_plays_its_enter_and_keeps_the_content_until_its_exit_is_over() {
    let mut ui = TestUi::new(fading);
    assert!(ui.attr("fx", "style").unwrap().contains("aio-fade 300ms ease 0ms 1 normal"));
    ui.click("toggle");
    assert!(ui.present("msg"), "still there while it leaves");
    assert!(ui.attr("fx", "style").unwrap().contains("reverse"));
    ui.wait(Duration::from_millis(150));
    assert!(ui.absent("msg"), "gone once the exit is over");
    ui.click("toggle");
    ui.click("toggle");
    ui.click("toggle");
    ui.wait(Duration::from_millis(150));
    assert!(ui.present("msg"), "show, hide, show: a cancelled exit removes nothing");
    assert!(ui.attr("fx", "style").unwrap().contains("normal"));
}

fn list() -> Element {
    let mut items = use_signal(|| vec!["a", "b", "c"]);
    let rows: Vec<(String, Element)> = items()
        .into_iter()
        .map(|k| (k.to_string(), rsx! { p { "data-t": "{k}", style: "height: 20px; margin: 0", "{k}" } }))
        .collect();
    rsx! {
        button { "data-t": "drop-b", onclick: move |_| items.set(vec!["a", "c", "d"]), "drop b" }
        TransitionGroup { items: rows, enter: Motion::FADE, exit: Motion::FADE.ms(1000), flip: true, flip_ms: 100 }
    }
}

/// The rows' handles in drawn order.
fn order(ui: &TestUi) -> String {
    (1..=5)
        .filter_map(|i| {
            ["a", "b", "c", "d"]
                .into_iter()
                .find(|t| ui.count(&format!(".aio-tgroup__row:nth-child({i}) [data-t={t}]")) == 1)
        })
        .collect()
}

/// Is row `t`'s style saying `what`?
fn row_says(ui: &TestUi, t: &str, what: &str) -> bool {
    ui.count(&format!(".aio-tgroup__row[style*='{what}'] [data-t={t}]")) == 1
}

#[test]
fn a_group_keeps_a_leaving_row_in_its_place_until_it_is_gone_and_slides_moved_rows() {
    let mut ui = TestUi::new(list);
    ui.wait(Duration::from_millis(20));
    ui.click("drop-b");
    assert_eq!(order(&ui), "abcd", "b leaves where it was; d enters at the end");
    assert!(row_says(&ui, "b", "reverse") && row_says(&ui, "d", "normal"));
    // Each phase lasts a while or a frame in real time: watched for as it plays.
    let seen = |ui: &mut TestUi, what: &str, f: &dyn Fn(&TestUi) -> bool| {
        let end = std::time::Instant::now() + Duration::from_secs(2);
        while !f(ui) {
            assert!(std::time::Instant::now() < end, "never seen: {what}");
            ui.wait(Duration::from_millis(1));
        }
    };
    seen(&mut ui, "gone once its exit is over", &|ui| order(ui) == "acd");
    seen(&mut ui, "the rows below slide up from where they were", &|ui| {
        row_says(ui, "c", "translate(0px,20px)") && row_says(ui, "d", "translate(0px,20px)")
    });
    assert!(!row_says(&ui, "a", "translate"), "a stays");
    seen(&mut ui, "played back", &|ui| row_says(ui, "c", "transform:none;transition:transform 100ms"));
    // (Dioxus keeps a style property the new style leaves out: `transform: none` stays.)
    seen(&mut ui, "and landed", &|ui| !row_says(ui, "c", "translate") && !row_says(ui, "c", "transition:transform"));
}

#[component]
fn Deferred(trigger: Trigger, t: &'static str) -> Element {
    rsx! {
        Defer { trigger, "data-t": "{t}", placeholder: rsx! { span { "data-t": "{t}-wait", "…" } },
            p { "data-t": "{t}-shown", "chart" }
        }
    }
}

fn deferred() -> Element {
    rsx! {
        Deferred { trigger: Trigger::Now, t: "now" }
        Deferred { trigger: Trigger::After(Duration::from_millis(300)), t: "timer" }
        Deferred { trigger: Trigger::Idle, t: "idle" }
        Deferred { trigger: Trigger::Hover, t: "hover" }
        Deferred { trigger: Trigger::Interaction, t: "tap" }
        div { "data-t": "list", style: "height: 100px; overflow: auto",
            div { style: "height: 300px" }
            Deferred { trigger: Trigger::Viewport, t: "seen" }
        }
    }
}

#[test]
fn deferred_content_waits_for_its_trigger_then_stays() {
    // aio tests/defer.test.ts.
    let mut ui = TestUi::new(deferred);
    assert!(ui.present("now-shown"));
    for t in ["timer", "hover", "tap", "seen"] {
        assert!(ui.present(&format!("{t}-wait")) && ui.absent(&format!("{t}-shown")), "{t}: the placeholder first");
    }
    ui.wait(Duration::from_millis(30));
    assert!(ui.present("idle-shown"), "idle: after the first frame");
    ui.wait(Duration::from_millis(300));
    assert!(ui.present("timer-shown"));
    ui.hover("hover");
    assert!(ui.present("hover-shown"));
    ui.hover("tap");
    assert!(ui.absent("tap-shown"), "a hover is no interaction");
    ui.click("tap");
    assert!(ui.present("tap-shown"));
    assert!(ui.absent("seen-shown"), "scrolled out of its box: not in view");
    ui.scroll("list", 210.0, 0.0);
    assert!(ui.present("seen-shown"), "scrolled into view");
    ui.scroll("list", 0.0, 0.0);
    assert!(ui.present("seen-shown"), "one-shot: it stays");
}

#[derive(Clone, Default, PartialEq)]
struct Signup {
    name: String,
    age: Option<u32>,
    password: String,
    confirm: String,
    tags: Vec<String>,
}

fn signup_check(s: &Signup) -> Problems {
    let mut p = vec![];
    if s.name.trim().is_empty() {
        p.push(("name", "Required".to_string()));
    }
    if s.age.is_none_or(|a| a < 18) {
        p.push(("age", "18 or over".to_string()));
    }
    if s.password != s.confirm {
        p.push(("confirm", "Passwords differ".to_string()));
    }
    if s.tags.len() > 2 {
        p.push(("", "At most two tags".to_string()));
    }
    p
}

fn signup() -> Element {
    let f = use_form(Signup::default, signup_check);
    use_async_rule(
        f,
        "name",
        |s| s.name.clone(),
        Duration::from_millis(300),
        |name: String| async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            (name == "taken").then(|| "That name is taken".to_string())
        },
    );
    let v = f.values();
    let tags = v.tags.join(",");
    rsx! {
        Field { label: "Name", error: f.error("name"), "data-t": "name-field",
            Input { value: v.name, "data-t": "name", oninput: move |n| f.edit("name", |s| s.name = n), onchange: move |_| f.touch("name") }
        }
        Field { label: "Age", error: f.error("age"),
            Input { r#type: "number", "data-t": "age", oninput: move |n: String| f.edit("age", |s| s.age = n.parse().ok()) }
        }
        Field { label: "Password",
            Input { r#type: "password", "data-t": "pw", oninput: move |n| f.edit("password", |s| s.password = n) }
        }
        Field { label: "Confirm", error: f.error("confirm"),
            Input { r#type: "password", "data-t": "confirm", oninput: move |n| f.edit("confirm", |s| s.confirm = n) }
        }
        p { "data-t": "tags", "{tags}" }
        button { "data-t": "add-tag", onclick: move |_| f.edit("tags", |s| s.tags.push(format!("t{}", s.tags.len()))), "add" }
        button { "data-t": "move-tag", onclick: move |_| f.edit("tags", |s| move_item(&mut s.tags, 0, 1)), "move" }
        if let Some(e) = f.form_error() {
            p { "data-t": "form-error", "{e}" }
        }
        p { "data-t": "state", "{f.valid()} {f.dirty()} {f.checking(\"name\")}" }
        button { "data-t": "submit", onclick: move |_| if let Some(s) = f.submit() { log(format!("sent {} {:?}", s.name, s.age)) }, "submit" }
        button { "data-t": "reset", onclick: move |_| f.reset(), "reset" }
    }
}

/// Let `ms` pass in 10 ms steps, so timers started on the way run too.
fn pass(ui: &mut TestUi, ms: u64) {
    for _ in 0..ms / 10 {
        ui.wait(Duration::from_millis(10));
    }
}

#[test]
fn a_form_shows_errors_once_left_or_submitted_and_sends_only_valid_values() {
    // aio tests/form-*.test.ts.
    let mut ui = TestUi::new(signup);
    assert_eq!(ui.text("state"), "false false false");
    assert!(ui.absent("form-error") && !ui.text("name-field").contains("Required"), "nothing shown before a touch");
    ui.type_text("name", "ana");
    // A field left as it was found fires no `change` (the browser's rule):
    // left once with a value, then emptied and left again.
    ui.blur("name");
    ui.set_value("name", "");
    ui.blur("name");
    assert!(ui.text("name-field").contains("Required"), "left empty: its own error shows");
    ui.click("submit");
    assert!(said().is_empty(), "invalid: nothing sent");
    assert!(ui.html().contains("18 or over"), "a submit shows every error");
    ui.type_text("name", "ana");
    ui.type_text("age", "42");
    ui.type_text("pw", "secret");
    ui.type_text("confirm", "secreT");
    assert!(ui.html().contains("Passwords differ"), "a cross-field rule lands on the field it names");
    ui.set_value("confirm", "secret");
    pass(&mut ui, 500);
    assert_eq!(ui.text("state"), "true true false");
    ui.click("submit");
    assert_eq!(said(), ["sent ana Some(42)"], "the typed values, the number parsed");
    for _ in 0..3 {
        ui.click("add-tag");
    }
    ui.click("submit");
    assert_eq!(ui.text("form-error"), "At most two tags");
    ui.click("move-tag");
    assert_eq!(ui.text("tags"), "t1,t0,t2", "a row moved");
    ui.click("reset");
    assert_eq!((ui.text("tags"), ui.text("state")), (String::new(), "false false false".into()));
    assert!(ui.absent("form-error"));
}

#[test]
fn a_waiting_rule_answers_the_latest_value_after_its_debounce_and_yields_to_the_fields_own() {
    // aio tests/form-async.test.ts, form-async-superseded-by-sync-error.test.ts.
    let mut ui = TestUi::new(signup);
    ui.type_text("name", "taken");
    ui.blur("name");
    assert!(ui.text("state").ends_with("true"), "checking");
    pass(&mut ui, 500);
    assert!(ui.text("name-field").contains("That name is taken"));
    ui.type_text("name", "x");
    assert!(!ui.text("name-field").contains("taken"), "an edit makes the answer stale");
    pass(&mut ui, 500);
    assert!(!ui.text("name-field").contains("taken"), "takenx is free");
    ui.set_value("name", "taken");
    ui.set_value("name", "");
    pass(&mut ui, 500);
    assert!(
        ui.text("name-field").contains("Required") && !ui.text("name-field").contains("taken"),
        "the sync error wins; the late answer for 'taken' is dropped"
    );
}

fn shortcuts() -> Element {
    let mut on = use_signal(|| true);
    rsx! {
        GlobalKeys {
            Bindings {}
            if on() {
                Scoped {}
            }
            button { "data-t": "btn", "focus me" }
            button { "data-t": "drop", onclick: move |_| on.set(false), "drop" }
            Input { "data-t": "field" }
        }
    }
}

#[component]
fn Bindings() -> Element {
    use_global_key("ctrl+s", |_| log("save"));
    use_global_key("ctrl+shift+s", |_| log("save-as"));
    use_global_key("mod+k", |_| log("palette"));
    use_global_key("ctrl+/", |_| log("comment"));
    use_global_key("ctrl+shift+/", |_| log("help"));
    use_global_key(Chord::parse("ctrl+i").unwrap().in_fields(), |_| log("in-field"));
    rsx! {}
}

#[component]
fn Scoped() -> Element {
    use_global_key("escape", |_| log("escape"));
    rsx! {}
}

fn keys(ui: &mut TestUi, t: &str, presses: &[(&str, Modifiers)]) -> Vec<String> {
    tab_to(ui, t);
    for (k, m) in presses {
        ui.press_key(Key::Character((*k).into()), *m);
    }
    said()
}

#[test]
fn a_global_chord_fires_on_its_exact_keys() {
    let mut ui = TestUi::new(shortcuts);
    let (c, s, m) = (Modifiers::CONTROL, Modifiers::SHIFT, Modifiers::META);
    assert_eq!(
        keys(&mut ui, "btn", &[("s", Modifiers::empty()), ("s", c), ("s", c | Modifiers::ALT), ("s", m)]),
        ["save"]
    );
    assert_eq!(keys(&mut ui, "btn", &[("S", c | s)]), ["save-as"]);
    assert_eq!(keys(&mut ui, "btn", &[("k", c), ("k", m), ("k", c | s)]), ["palette", "palette"]);
}

#[test]
fn on_a_shift_held_press_the_binding_naming_shift_wins() {
    let mut ui = TestUi::new(shortcuts);
    let (c, s) = (Modifiers::CONTROL, Modifiers::SHIFT);
    // German Ctrl+Shift+7 types "/": both bindings match, only the one naming Shift runs.
    assert_eq!(keys(&mut ui, "btn", &[("/", c | s)]), ["help"]);
    assert_eq!(keys(&mut ui, "btn", &[("/", c)]), ["comment"]);
}

#[test]
fn a_field_keeps_its_keys_unless_the_binding_says_otherwise() {
    let mut ui = TestUi::new(shortcuts);
    let c = Modifiers::CONTROL;
    assert_eq!(keys(&mut ui, "field", &[("s", c), ("i", c)]), ["in-field"]);
    assert_eq!(keys(&mut ui, "btn", &[("s", c)]), ["save"], "the field's keystroke doesn't stick to the next one");
}

#[test]
fn a_binding_goes_with_its_component() {
    let mut ui = TestUi::new(shortcuts);
    tab_to(&mut ui, "btn");
    ui.press_key(Key::Escape, Modifiers::empty());
    assert_eq!(said(), ["escape"]);
    ui.click("drop");
    tab_to(&mut ui, "btn");
    ui.press_key(Key::Escape, Modifiers::empty());
    assert_eq!(said(), Vec::<String>::new());
}

#[test]
fn the_kit_stylesheet_is_the_theme_and_every_component_class() {
    for class in [
        ".aio-btn--primary",
        ".aio-tablist",
        ".aio-menu__scrim",
        ".aio-toast--error",
        ".aio-md__pre",
        ".aio-zoompan__stage",
    ] {
        assert!(CSS.contains(class), "{class}");
    }
    let css = theme_css(115);
    assert!(css.starts_with("@layer aio {\n:root{\n  --aio-hue:115;"), "tokens first, then the kit");
    assert!(css.ends_with(CSS));
}

thread_local! {
    static CONN: RefCell<Option<Signal<aio::Conn>>> = const { RefCell::new(None) };
}

fn connection() -> Element {
    let conn = use_signal(|| aio::Conn::Online);
    CONN.with(|c| *c.borrow_mut() = Some(conn));
    rsx! { ConnectionStatus { status: aio::Status { conn: conn(), ..aio::Status::LOCAL } } }
}

#[test]
fn the_connection_pill_says_reconnecting_then_connected() {
    let mut ui = TestUi::new(connection);
    let to = |ui: &mut TestUi, c: aio::Conn| {
        ui.run(|| CONN.with(|s| s.borrow_mut().as_mut().unwrap().set(c)));
        ui.settle();
    };
    // Up from the start: nothing to say, nothing shown.
    assert!(ui.absent("connStatus"));
    to(&mut ui, aio::Conn::Syncing);
    assert!(ui.absent("connStatus"));
    to(&mut ui, aio::Conn::Offline);
    assert_eq!(ui.text("connStatus"), "Reconnecting…");
    assert_eq!(ui.attr("connStatus", "role").as_deref(), Some("status"));
    assert!(ui.attr("connStatus", "class").unwrap().contains("aio-conn--warn"));
    to(&mut ui, aio::Conn::Blocked);
    assert!(ui.text("connStatus").contains("too many changes wait"));
    to(&mut ui, aio::Conn::Online);
    assert_eq!(ui.text("connStatus"), "Connected");
    assert!(ui.attr("connStatus", "class").unwrap().contains("aio-conn--success"));
    assert!(CSS.contains(".aio-conn--success") && CSS.contains("aio-conn-out"));
}

fn springy() -> Element {
    let x = use_spring(0.0, Spring::default());
    let mut ticks = use_signal(|| 0);
    let mut on = use_signal(|| true);
    let mut frames = use_signal(|| 0);
    use_interval(Duration::from_millis(5), on(), move || ticks += 1);
    use_frame(on(), move |_, _| frames += 1);
    rsx! {
        p { "data-t": "x", "{x.value():.2} {x.animating()}" }
        p { "data-t": "ticks", "{ticks}" }
        p { "data-t": "frames", "{frames}" }
        button { "data-t": "go", onclick: move |_| x.to(100.0), "go" }
        button { "data-t": "jump", onclick: move |_| x.set(7.0), "jump" }
        button { "data-t": "stop", onclick: move |_| on.set(false), "stop" }
    }
}

#[test]
fn a_spring_moves_to_its_target_and_rests_there() {
    let mut ui = TestUi::new(springy);
    assert_eq!(ui.text("x"), "0.00 false");
    ui.click("go");
    let mut seen = vec![];
    while ui.text("x") != "100.00 false" {
        assert!(seen.len() < 500, "never at rest: {}", ui.text("x"));
        seen.push(ui.text("x").split(' ').next().unwrap().parse::<f64>().unwrap());
        ui.wait(Duration::from_millis(10));
    }
    assert!(seen.iter().any(|&v| v > 1.0 && v < 99.0), "it moved through the way: {seen:?}");
    assert!(seen.windows(2).all(|w| w[1] >= w[0]), "toward the target only: {seen:?}");
    ui.click("jump");
    assert_eq!(ui.text("x"), "7.00 false", "set is immediate");
}

#[test]
fn loops_tick_while_active_and_stop_after() {
    let mut ui = TestUi::new(springy);
    let n = |ui: &TestUi, t: &str| ui.text(t).parse::<u32>().unwrap();
    for _ in 0..500 {
        if n(&ui, "ticks") >= 3 && n(&ui, "frames") >= 3 {
            break;
        }
        ui.wait(Duration::from_millis(10));
    }
    assert!(n(&ui, "ticks") >= 3 && n(&ui, "frames") >= 3, "they tick");
    ui.click("stop");
    let (ticks, frames) = (n(&ui, "ticks"), n(&ui, "frames"));
    ui.wait(Duration::from_millis(60));
    assert_eq!((n(&ui, "ticks"), n(&ui, "frames")), (ticks, frames), "inactive loops are stopped");
}

thread_local! {
    static TICKS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

#[component]
fn Ticker() -> Element {
    use_interval(Duration::from_millis(5), true, || TICKS.with(|t| t.set(t.get() + 1)));
    rsx! { "ticking" }
}

fn mounted_ticker() -> Element {
    let mut on = use_signal(|| true);
    rsx! {
        if on() { Ticker {} }
        button { "data-t": "unmount", onclick: move |_| on.set(false), "x" }
    }
}

#[test]
fn a_loop_stops_with_its_component() {
    let mut ui = TestUi::new(mounted_ticker);
    ui.wait(Duration::from_millis(40));
    assert!(TICKS.with(|t| t.get()) > 0);
    ui.click("unmount");
    let n = TICKS.with(|t| t.get());
    ui.wait(Duration::from_millis(40));
    assert_eq!(TICKS.with(|t| t.get()), n);
}

#[derive(Default)]
struct Look {
    color: &'static str,
    size: u32,
}

thread_local! {
    static RENDERS: RefCell<Vec<&'static str>> = const { RefCell::new(vec![]) };
}

#[component]
fn ColorOf() -> Element {
    RENDERS.with(|r| r.borrow_mut().push("color"));
    let c = use_context_select(|l: &Look| l.color);
    rsx! { p { "data-t": "color", "{c}" } }
}

#[component]
fn SizeOf() -> Element {
    RENDERS.with(|r| r.borrow_mut().push("size"));
    let s = use_context_select(|l: &Look| l.size);
    rsx! { p { "data-t": "size", "{s}" } }
}

fn looks() -> Element {
    let mut look = use_context_provider(|| Signal::new(Look { color: "red", size: 1 }));
    rsx! {
        ColorOf {}
        SizeOf {}
        button { "data-t": "recolor", onclick: move |_| look.write().color = "blue", "x" }
    }
}

#[test]
fn a_context_selection_re_renders_only_when_its_part_changes() {
    let mut ui = TestUi::new(looks);
    RENDERS.with(|r| r.borrow_mut().clear());
    ui.click("recolor");
    assert_eq!((ui.text("color"), ui.text("size")), ("blue".into(), "1".into()));
    assert_eq!(RENDERS.with(|r| r.take()), ["color"], "the size view didn't re-render");
    let ui = TestUi::new(|| rsx! { SizeOf {} });
    assert_eq!(ui.text("size"), "0", "no provider: the default");
}

#[test]
fn a_kit_control_on_a_page_nothing_styles_is_said() {
    // aio tests: _kitStyled.
    assert!(unstyled("", "aio-btn").is_some_and(|w| w.contains("Any rule naming .aio-btn counts")));
    assert_eq!(
        unstyled(&aio::ui::page_css(&[&theme_css(theme::hue("kit"))], false), "aio-btn"),
        None,
        "the kit's sheet"
    );
    assert_eq!(unstyled("@layer app{.aio-btn{color:red}}", "aio-btn"), None, "the app's own rule");
    assert_eq!(unstyled("", "aio-stack"), None, "a class the kit doesn't style can't lack it");
}

fn long_list() -> Element {
    let mut to = use_signal(|| None::<usize>);
    rsx! {
        VirtualList { "data-t": "list", len: 1000, row_height: 20.0, height: 100.0, overscan: 1, scroll_to: to(),
            row: |i| rsx! { span { "data-t": "row{i}", "#{i}" } }
        }
        button { "data-t": "jump", onclick: move |_| to.set(Some(500)), "jump" }
    }
}

#[test]
fn a_virtual_list_renders_only_the_rows_its_scroll_shows() {
    let mut ui = TestUi::new(long_list);
    assert_eq!(ui.count("[data-t=list] [data-t^=row]"), 7, "5 in view and overscan (2 at the top)");
    assert_eq!((ui.count("[data-t=row0]"), ui.count("[data-t=row7]")), (1, 0));
    ui.scroll("list", 2010.0, 0.0);
    assert_eq!(ui.count("[data-t=row99]"), 1, "the one above, as overscan");
    assert_eq!(ui.count("[data-t=row106]"), 1, "the one cut at the bottom, then overscan");
    assert_eq!((ui.count("[data-t=row0]"), ui.count("[data-t=row107]")), (0, 0));
    ui.click("jump");
    assert_eq!((ui.text("row500"), ui.count("[data-t=row499]")), ("#500".into(), 1), "scroll_to moves the scrollbar");
}

thread_local!(static ROWS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) });

fn counted_list() -> Element {
    rsx! {
        VirtualList { "data-t": "list", len: 1000, row_height: 20.0, height: 100.0, overscan: 3,
            row: |i| {
                ROWS.set(ROWS.get() + 1);
                rsx! { span { "data-t": "row{i}", "#{i}" } }
            }
        }
    }
}

#[test]
fn a_scroll_within_the_rows_drawn_renders_nothing() {
    let mut ui = TestUi::new(counted_list);
    ui.scroll("list", 5.0, 0.0);
    let n = ROWS.get();
    ui.scroll("list", 12.0, 0.0);
    ui.scroll("list", 17.0, 0.0);
    assert_eq!(ROWS.get(), n, "still the same window of rows");
    ui.scroll("list", 400.0, 0.0);
    assert!(ROWS.get() > n && ui.count("[data-t=row20]") == 1, "a new window");
}

mod travel {
    use super::*;
    use aio::prelude::*;

    #[cell]
    pub struct Count {
        pub n: u32,
    }

    #[methods]
    impl Count {
        pub fn inc(&mut self) {
            self.n += 1;
        }
    }

    aio::client!(Count, count);

    fn app() -> Element {
        use_live();
        let tl = use_time_travel();
        rsx! {
            GlobalKeys {
                p { "data-t": "n", "{count().n}" }
                p { "data-t": "hist", {tl().map_or("none".into(), |t| t.steps.len().to_string())} }
                button { "data-t": "inc", onclick: |_| _ = act!(inc()), "inc" }
                TimeTravelPanel { timeline: tl(), onask: |a| if let Err(e) = time_travel(a) { log(e.why) } }
            }
        }
    }

    #[test]
    fn the_panel_steps_back_holds_the_cell_and_goes_anywhere() {
        // aio tests/time-travel-real-page.test.ts, time-travel-paused-dispatch.
        let mut ui = TestUi::new(app);
        ui.click("inc");
        ui.click("inc");
        assert_eq!((ui.text("n"), ui.text("hist")), ("2".into(), "2".into()), "the hook reads the history");
        assert!(ui.absent("timeTravel"), "hidden until Ctrl+.");
        tab_to(&mut ui, "inc");
        ui.press_key(Key::Character(".".into()), Modifiers::CONTROL);
        assert!(ui.present("timeTravel"));
        assert!(ui.disabled("ttRedo"), "nothing to redo yet");
        let first = ui.run(|| time_travel(aio::TimeAsk::Pause).unwrap().steps[0].seq);
        ui.click("ttPause");
        assert!(!ui.text("ttWhere").contains("paused"), "resumed");
        ui.click("ttUndo");
        assert_eq!(ui.text("n"), "1", "undo");
        assert!(ui.text("ttWhere").contains("paused") && ui.text("ttPause") == "Resume");
        ui.click("inc");
        assert_eq!(ui.text("n"), "1", "a step holds the cell: calls are refused, never dropped as if they ran");
        assert_eq!(ui.failures().len(), 1);
        ui.click("ttRedo");
        assert_eq!(said(), Vec::<String>::new(), "the step was taken");
        assert_eq!(ui.text("n"), "2", "redo");
        ui.click(&format!("ttStep{first}"));
        assert_eq!(ui.text("n"), "1", "a step of the list");
        assert_eq!(ui.attr(&format!("ttStep{first}"), "aria-current").as_deref(), Some("step"));
        ui.click("ttPause");
        ui.click("inc");
        assert_eq!(ui.text("n"), "2", "resumed: calls run again");
        tab_to(&mut ui, "inc");
        ui.press_key(Key::Character(".".into()), Modifiers::CONTROL);
        assert!(ui.absent("timeTravel"));
    }
}

#[derive(aio::serde::Serialize, Clone, PartialEq)]
#[serde(crate = "aio::serde")]
struct Chart {
    series: Vec<i32>,
}

const CHART: Island = Island::js("return (el, p) => { el.textContent = p.series.join(','); };");

fn island() -> Element {
    rsx! {
        WebIsland::<Chart, ()> {
            island: CHART,
            props: Chart { series: vec![1, 2] },
            class: "wide",
            "data-t": "chart",
            loading: rsx! { span { "data-t": "chart-wait", "loading" } },
        }
    }
}

#[test]
fn an_island_is_a_box_of_its_own_with_its_placeholder_until_the_page_mounts_it() {
    // aio tests/island.test.ts: a box the view places, its placeholder shown
    // until the page mounts it, then the island's own drawing of its props.
    let mut ui = TestUi::new(island);
    assert_eq!(ui.attr("chart", "class").as_deref(), Some("aio-island wide"));
    assert!(ui.attr("chart", "data-aio-island").is_some_and(|id| id.starts_with("aio-island-")));
    ui.until_text("chart", "1,2");
    assert!(ui.absent("chart-wait"), "the placeholder went once it mounted");
}

/// A library the app doesn't vouch for: a frame of its own, events typed.
const PROBE: Island = Island::js(
    "return (el, p, emit) => { el.textContent = 'drawn'; let page = true; try { parent.document.title; } catch (e) { page = false; }
       emit({ series: [p.series.length, page ? 1 : 0] }); return { update: (q) => emit({ series: q.series }) }; };",
)
.sandboxed();

#[derive(aio::serde::Deserialize)]
#[serde(crate = "aio::serde")]
struct Said {
    series: Vec<i32>,
}

fn sandboxed() -> Element {
    let mut said = use_signal(String::new);
    let mut n = use_signal(|| 2);
    rsx! {
        WebIsland {
            island: PROBE,
            props: Chart { series: vec![0; n()] },
            onevent: move |s: Said| said.set(format!("{:?}", s.series)),
            "data-t": "probe",
        }
        button { "data-t": "more", onclick: move |_| n += 1, "more" }
        p { "data-t": "said", "{said}" }
    }
}

#[test]
fn a_sandboxed_island_runs_in_a_frame_of_its_own_and_still_talks_through_its_props_and_emit() {
    let mut ui = TestUi::new(sandboxed);
    ui.until_text("said", "[2, 0]");
    assert_eq!(ui.count("[data-t=probe] iframe[sandbox=allow-scripts]"), 1, "its own frame, its own origin");
    ui.click("more");
    ui.until_text("said", "[0, 0, 0]");
}

fn browser() -> Element {
    rsx! {
        Browser { src: "https://a.example/", class: "pane", "data-t": "web", host_keys: vec!["Escape".into()] }
    }
}

#[test]
fn a_browser_is_a_box_of_the_view_that_the_page_fills_with_its_guest() {
    // aio tests/ui-browser.test.ts "it renders a plain <webview>": the box is
    // the view's, the guest the page's (headless there is none, and nothing fails).
    let mut ui = TestUi::new(browser);
    ui.wait(Duration::from_millis(30));
    assert_eq!(ui.attr("web", "class").as_deref(), Some("aio-island aio-browser pane"));
    assert!(ui.attr("web", "data-aio-island").is_some());
}

fn coached() -> Element {
    let mut coach = use_signal(|| false);
    rsx! {
        GlobalKeys {
            Escapes {}
            if coach() {
                Coach {}
            }
            button { "data-t": "coach", onclick: move |_| coach.toggle(), "coach" }
            div {
                "data-t": "dialog",
                onkeydown: move |e| log(format!("dialog {}", e.key())),
                button { "data-t": "ok", "OK" }
                input { "data-t": "memo", aria_label: "memo" }
            }
        }
    }
}

#[component]
fn Escapes() -> Element {
    use_global_key("escape", |_| log("escape"));
    use_window_event(|e| {
        if let WindowEvent::KeyDown(k) = e {
            log(format!("window {}", k.key()));
        }
    });
    rsx! {}
}

#[component]
fn Coach() -> Element {
    use_key_claim("enter", |e| log(format!("coach {}", e.key())));
    use_key_claim(Chord::parse("f1").unwrap().in_fields(), |_| log("hint"));
    rsx! {}
}

#[test]
fn a_key_pressed_with_nothing_focused_reaches_the_root() {
    let mut ui = TestUi::new(coached);
    ui.focus("window");
    ui.press_key(Key::Escape, Modifiers::empty());
    assert_eq!(said(), ["escape"]);
    ui.press_key(Key::Character("x".into()), Modifiers::empty());
    assert_eq!(said(), ["window x"]);
    ui.press("ok", "x");
    assert_eq!(said(), ["dialog x", "window x"], "once, from where it was pressed");
}

#[test]
fn a_claimed_key_goes_to_its_claim_before_anything_hears_it() {
    let mut ui = TestUi::new(coached);
    ui.press("ok", "Enter");
    assert_eq!(said(), ["dialog Enter", "window Enter"], "no claim: the dialog's");
    ui.click("coach");
    ui.press("ok", "Enter");
    assert_eq!(said(), ["coach Enter"], "claimed: nobody else hears it");
    ui.focus("window");
    ui.press_key(Key::Enter, Modifiers::empty());
    assert_eq!(said(), ["coach Enter"], "with nothing focused too");
    ui.press("ok", "Enter");
    ui.press_with("ok", "Enter", Modifiers::CONTROL);
    assert_eq!(said(), ["coach Enter", "dialog Enter", "window Enter"], "its exact chord only");
    ui.press("memo", "Enter");
    assert_eq!(said(), ["dialog Enter", "window Enter"], "a field keeps its keys");
    ui.press("memo", "F1");
    assert_eq!(said(), ["hint"], "unless the chord says in_fields");
    ui.click("coach");
    ui.press("ok", "Enter");
    assert_eq!(said(), ["dialog Enter", "window Enter"], "the claim went with its component");
}

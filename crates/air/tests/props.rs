//! Props as the kit needs them: optional and default props, `Option`s that
//! take a value, handler props typed from a closure, attributes handed
//! through a component (`..attributes`), and props that follow state.
use air::testing::TestPage;
use air::*;
use std::cell::{Cell, RefCell};

thread_local! {
    static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static BODIES: Cell<u32> = const { Cell::new(0) };
}

fn said() -> Vec<String> {
    LOG.with(|l| std::mem::take(&mut *l.borrow_mut()))
}

#[component]
fn Badge(
    /// Required: no default.
    text: String,
    #[props(default = 3)] size: u8,
    #[props(default, into)] class: String,
    #[props(into)] title: Option<String>,
    onpick: Option<EventHandler<String>>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> View {
    rsx! {
        span {
            class: "badge {class}",
            "data-size": "{size}",
            title,
            onclick: move |_| if let Some(f) = onpick { f.call(text.clone()) },
            ..attributes,
            "{text}"
        }
    }
}

#[test]
fn left_out_props_take_their_defaults_and_options_take_a_value() {
    let p = TestPage::mount(|| {
        rsx! {
            Badge { "data-t": "a", text: "one" }
            Badge { "data-t": "b", text: "two", size: 5, class: "big", title: "Two" }
            Badge { "data-t": "c", text: "three", title: Some("Three".to_string()) }
        }
    });
    assert_eq!(p.text("a"), "one");
    assert_eq!(p.attr("a", "data-size").as_deref(), Some("3"), "`default = 3`");
    assert_eq!(p.attr("a", "class").as_deref(), Some("badge "), "`default`: empty");
    assert_eq!(p.attr("a", "title"), None, "an Option left out is None");
    assert_eq!(p.attr("b", "data-size").as_deref(), Some("5"));
    assert_eq!(p.attr("b", "class").as_deref(), Some("badge big"));
    assert_eq!(p.attr("b", "title").as_deref(), Some("Two"), "a value into an Option");
    assert_eq!(p.attr("c", "title").as_deref(), Some("Three"), "an Option as it is");
}

#[test]
fn a_closure_given_to_a_handler_prop_is_typed_by_it() {
    let mut p = TestPage::mount(|| {
        rsx! { Badge { "data-t": "a", text: "one", onpick: move |s| LOG.with(|l| l.borrow_mut().push(s)) } }
    });
    p.click("a");
    assert_eq!(said(), ["one"]);
}

#[test]
fn attributes_reach_the_element_and_win_over_its_own() {
    let mut p = TestPage::mount(|| {
        let on = signal(|| false);
        rsx! {
            Badge { "data-t": "a", id: "mine", aria_label: "Badge", "data-size": "9", text: "one",
                "aria-pressed": if on.get() { "true" } else { "false" } }
            button { "data-t": "flip", onclick: move |_| on.toggle(), "flip" }
        }
    });
    assert_eq!(p.attr("a", "id").as_deref(), Some("mine"), "a global attribute by name");
    assert_eq!(p.attr("a", "aria-label").as_deref(), Some("Badge"), "aria_label is aria-label");
    assert_eq!(p.attr("a", "data-size").as_deref(), Some("9"), "the caller's wins");
    assert_eq!(p.attr("a", "aria-pressed").as_deref(), Some("false"));
    p.click("flip");
    assert_eq!(p.attr("a", "aria-pressed").as_deref(), Some("true"), "an attribute handed in follows state");
}

#[component]
fn Forward(#[props(extends = GlobalAttributes)] attributes: Vec<Attribute>) -> View {
    rsx! { Badge { text: "fwd", ..attributes } }
}

#[test]
fn a_component_hands_its_attributes_on() {
    let p = TestPage::mount(|| rsx! { Forward { "data-t": "f", id: "x" } });
    assert_eq!((p.text("f"), p.attr("f", "id")), ("fwd".into(), Some("x".into())));
}

#[component]
fn Label(value: Prop<String>, #[props(default)] count: Prop<u32>, #[props(into)] hint: Prop<Option<String>>) -> View {
    BODIES.set(BODIES.get() + 1);
    rsx! {
        p { "data-t": "v", title: hint.get(), "{value} ({count})" }
    }
}

#[test]
fn a_prop_follows_what_it_was_given_and_the_body_runs_once() {
    BODIES.set(0);
    let mut p = TestPage::mount(|| {
        let name = signal(|| "a".to_string());
        let n = signal(|| 1u32);
        rsx! {
            Label { value: name.get(), count: n, hint: if n.get() > 1 { Some("many") } else { None } }
            button { "data-t": "name", onclick: move |_| name.set("b".into()), "name" }
            button { "data-t": "inc", onclick: move |_| n += 1, "inc" }
        }
    });
    assert_eq!((p.text("v"), p.attr("v", "title")), ("a (1)".into(), None));
    p.click("name");
    assert_eq!(p.text("v"), "b (1)", "an expression given to a Prop follows");
    p.click("inc");
    assert_eq!(p.text("v"), "b (2)", "a signal given to a Prop follows");
    assert_eq!(p.attr("v", "title").as_deref(), Some("many"), "a Prop<Option> takes a value");
    assert_eq!(BODIES.get(), 1, "no body re-ran");
}

#[test]
fn a_prop_given_a_constant_holds_no_state() {
    let p = TestPage::mount(|| rsx! { Label { value: "fixed", hint: "h" } });
    assert_eq!(p.text("v"), "fixed (0)");
    let before = air::live();
    drop(p);
    assert!(before.nodes > 0);
    assert_eq!(air::live(), air::Live::default(), "nothing left");
}

#[derive(Props, Clone)]
struct PanelProps {
    #[props(default = String::from("Panel"))]
    title: String,
    children: View,
}

#[allow(non_snake_case)]
fn Panel(p: PanelProps) -> View {
    let PanelProps { title, children } = p;
    rsx! { section { h2 { "data-t": "title", "{title}" } {children} } }
}

#[test]
fn a_hand_written_props_struct_gets_a_builder() {
    let p = TestPage::mount(|| rsx! { Panel { p { "data-t": "in", "inside" } } });
    assert_eq!((p.text("title"), p.text("in")), ("Panel".into(), "inside".into()));
}

#[test]
fn a_prop_without_a_closure_is_built_as_written() {
    let props = <BadgeProps as Props>::builder().text(|| "x").size(|| 7u8).build();
    assert_eq!((props.text.as_str(), props.size, props.title), ("x", 7, None));
}

#[component]
fn List<T: Clone + PartialEq + std::fmt::Display + 'static>(
    items: Prop<Vec<T>>,
    #[props(default)] lead: String,
) -> View {
    rsx! { ul { "data-t": "list", for (i, x) in items.get().into_iter().enumerate() { li { key: "{i}", "{lead}{x}" } } } }
}

#[test]
fn a_generic_component_takes_its_props_type_from_them() {
    let mut p = TestPage::mount(|| {
        let n = signal(|| vec![1u8, 2]);
        rsx! {
            List { items: n.get(), lead: "#" }
            button { "data-t": "more", onclick: move |_| n.write().push(3), "more" }
        }
    });
    assert_eq!(p.texts("list"), ["#1", "#2"]);
    p.click("more");
    assert_eq!(p.texts("list"), ["#1", "#2", "#3"]);
}

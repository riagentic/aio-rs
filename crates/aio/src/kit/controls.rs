//! The controls a dashboard needs beyond inputs (aio's `controls.ts`): radio
//! group, switch, tabs, progress, alert, tooltip, menu, breadcrumb, skeleton,
//! empty state, connection status. Keyboard first: each does its WAI-ARIA
//! pattern (tabs arrow between, a menu opens on ArrowDown and closes on
//! Escape back to its trigger, a tooltip closes on Escape).
use super::form::{Opt, use_field};
use super::{flag, join, use_id};
use dioxus::prelude::*;
use std::collections::HashMap;
use std::rc::Rc;

/// Native radios sharing one `name` in a `role="group"`: the browser gives
/// arrow keys, roving focus and "one of N" for free.
#[component]
pub fn RadioGroup(
    #[props(into)] options: Vec<Opt>,
    #[props(default, into)] value: String,
    /// The group's name; required unless a `Field` around it names it.
    #[props(into)]
    label: Option<String>,
    #[props(into)] name: Option<String>,
    #[props(default)] disabled: bool,
    onchange: Option<EventHandler<String>>,
    #[props(default, into)] class: String,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> Element {
    let own = use_id("radio");
    let name = name.unwrap_or(own);
    let w = use_field(false, true);
    rsx! {
        div {
            class: join("aio-radios", &class),
            role: "group",
            "aria-label": label.or(w.label),
            "aria-describedby": w.described,
            ..attributes,
            for o in options {
                label { key: "{o.value}", class: "aio-radio-row", "data-disabled": (disabled || o.disabled).then_some(""),
                    input {
                        r#type: "radio",
                        class: "aio-radio",
                        name: "{name}",
                        value: "{o.value}",
                        checked: o.value == value,
                        disabled: flag(disabled || o.disabled),
                        "aria-label": "{o.label}",
                        oninput: {
                            let v = o.value.clone();
                            move |_| if let Some(f) = onchange { f.call(v.clone()) }
                        },
                    }
                    span { "{o.label}" }
                }
            }
        }
    }
}

/// An on/off toggle: a real `<input type="checkbox" role="switch">` (Space
/// toggles it, a reader says on/off). `onchange` gets the new state.
#[component]
pub fn Switch(
    #[props(default)] checked: bool,
    #[props(default)] disabled: bool,
    #[props(into)] label: Option<String>,
    onchange: Option<EventHandler<bool>>,
    #[props(default, into)] class: String,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> Element {
    let w = use_field(label.is_some(), false);
    let own = label.as_deref().map(str::trim).filter(|l| !l.is_empty()).map(String::from);
    let input = rsx! {
        input {
            r#type: "checkbox",
            role: "switch",
            class: if label.is_none() { join("aio-switch", &class) } else { "aio-switch".into() },
            checked,
            disabled: flag(disabled),
            id: w.id,
            "aria-label": own,
            "aria-describedby": w.described,
            "aria-invalid": w.invalid,
            oninput: move |e| if let Some(f) = onchange { f.call(e.checked()) },
            ..attributes,
        }
    };
    match label {
        None => input,
        Some(l) => rsx! {
            label { class: join("aio-switch-row", &class),
                {input}
                span { "{l}" }
            }
        },
    }
}

/// One tab of [`Tabs`]: its id (what `value` and `onchange` speak), its
/// label, and its panel (rendered only while active).
#[derive(Clone, PartialEq)]
pub struct Tab {
    pub id: String,
    pub label: String,
    pub disabled: bool,
    pub panel: Element,
}

impl Tab {
    pub fn new(id: impl Into<String>, label: impl Into<String>, panel: Element) -> Self {
        Self { id: id.into(), label: label.into(), disabled: false, panel }
    }

    pub fn disabled(self) -> Self {
        Self { disabled: true, ..self }
    }
}

/// The enabled tab `delta` steps from `from` (wrapping, skipping disabled
/// ones); `from` may sit one past either end (Home: -1, End: len).
pub fn tab_step(disabled: &[bool], from: isize, delta: isize) -> Option<usize> {
    let n = disabled.len() as isize;
    (1..=n)
        .map(|step| (from + delta * step).rem_euclid(n.max(1)) as usize)
        .find(|&i| !disabled.get(i).copied().unwrap_or(true))
}

/// A tab id as a DOM id piece: whitespace and `%` percent-escaped, so
/// "General settings" stays one id and two ids never collide.
fn dom_id(id: &str) -> String {
    id.chars()
        .map(|c| if c == '%' || c.is_whitespace() { format!("%{:02x}", c as u32) } else { c.to_string() })
        .collect()
}

/// Which way the controls inside run: right to left reads mirrored
/// (`<Direction rtl>` sets it).
#[derive(Clone, Copy)]
struct Rtl(Signal<bool>);

/// A part of the page whose text runs right to left (`rtl`) or left to
/// right: a `dir` box with no layout of its own (`display: contents`), and
/// the kit controls inside follow it (aio read the closest `[dir]`). An app in
/// a right-to-left language wraps its root in one.
#[component]
pub fn Direction(rtl: bool, children: Element) -> Element {
    let Rtl(mut now) = use_context_provider(|| Rtl(Signal::new(rtl)));
    use_effect(use_reactive!(|rtl| if *now.peek() != rtl {
        now.set(rtl)
    }));
    rsx! {
        div { dir: if rtl { "rtl" } else { "ltr" }, style: "display:contents", {children} }
    }
}

/// Tabs: arrows move (wrapping, skipping disabled tabs; mirrored right to
/// left: `rtl`, else the [`Direction`] around them),
/// Home/End jump, and only the active tab is in the tab order, so Tab moves
/// on into the panel. Controlled with `value`; else it remembers its own,
/// starting at the first enabled tab (also when the chosen one is gone).
#[component]
pub fn Tabs(
    tabs: Vec<Tab>,
    #[props(into)] value: Option<String>,
    #[props(into)] label: Option<String>,
    rtl: Option<bool>,
    onchange: Option<EventHandler<String>>,
    #[props(default, into)] class: String,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> Element {
    let base = use_id("tabs");
    let around = try_use_context::<Rtl>();
    let rtl = rtl.unwrap_or_else(|| around.is_some_and(|Rtl(r)| r()));
    let first = tabs.iter().find(|t| !t.disabled).or(tabs.first()).map(|t| t.id.clone()).unwrap_or_default();
    let mut inner = use_signal(|| value.clone().unwrap_or(first.clone()));
    let mut els = use_signal(HashMap::<String, Rc<MountedData>>::new);
    let active = value.clone().unwrap_or_else(|| {
        let i = inner();
        if tabs.iter().any(|t| t.id == i) { i } else { first.clone() }
    });
    let shown = tabs.iter().find(|t| t.id == active);
    let stop = shown.map_or(first.clone(), |t| t.id.clone());
    let disabled: Vec<bool> = tabs.iter().map(|t| t.disabled).collect();
    let ids: Vec<String> = tabs.iter().map(|t| t.id.clone()).collect();
    let controlled = value.is_some();
    let mut select = move |id: String| {
        if !controlled {
            inner.set(id.clone());
        }
        if let Some(f) = onchange {
            f.call(id)
        }
    };
    let named_by = shown.map(|t| format!("{base}-tab-{}", dom_id(&t.id)));
    let panel = shown.map(|t| t.panel.clone());
    rsx! {
        div { class: join("aio-tabs", &class), ..attributes,
            div { class: "aio-tablist", role: "tablist", "aria-label": label,
                for (i, t) in tabs.iter().enumerate() {
                    button {
                        key: "{t.id}",
                        "data-t": "{t.id}",
                        r#type: "button",
                        id: "{base}-tab-{dom_id(&t.id)}",
                        class: if t.id == active { "aio-tab aio-tab--active" } else { "aio-tab" },
                        role: "tab",
                        "aria-selected": if t.id == active { "true" } else { "false" },
                        "aria-controls": "{base}-panel",
                        tabindex: if t.id == stop { "0" } else { "-1" },
                        disabled: flag(t.disabled),
                        onmounted: {
                            let id = t.id.clone();
                            move |e: MountedEvent| _ = els.write().insert(id.clone(), e.data())
                        },
                        onclick: {
                            let (id, off) = (t.id.clone(), t.disabled);
                            move |_| if !off { select(id.clone()) }
                        },
                        onkeydown: {
                            let (disabled, ids) = (disabled.clone(), ids.clone());
                            move |e: KeyboardEvent| {
                                let (fwd, back) = if rtl { (Key::ArrowLeft, Key::ArrowRight) } else { (Key::ArrowRight, Key::ArrowLeft) };
                                let k = e.key();
                                let (from, delta) = match k {
                                    _ if k == fwd || k == Key::ArrowDown => (i as isize, 1),
                                    _ if k == back || k == Key::ArrowUp => (i as isize, -1),
                                    Key::Home => (-1, 1),
                                    Key::End => (ids.len() as isize, -1),
                                    _ => return,
                                };
                                e.prevent_default();
                                if let Some(to) = tab_step(&disabled, from, delta) {
                                    select(ids[to].clone());
                                    // Roving focus: the next arrow must start from the new tab.
                                    if let Some(el) = els.peek().get(&ids[to]).cloned() {
                                        spawn(async move { _ = el.set_focus(true).await });
                                    }
                                }
                            }
                        },
                        "{t.label}"
                    }
                }
            }
            div {
                class: "aio-tabpanel",
                role: "tabpanel",
                id: "{base}-panel",
                "aria-labelledby": named_by,
                tabindex: "0",
                if let Some(p) = panel {
                    {p}
                }
            }
        }
    }
}

/// The percentage beside a bar, clamped as the bar is; `None` when there is
/// no value to show (indeterminate, a non-finite number, a zero max).
pub fn progress_pct(value: Option<f64>, max: f64) -> Option<u32> {
    let v = value.filter(|v| v.is_finite())?;
    (max.is_finite() && max != 0.0).then(|| (v / max * 100.0).round().clamp(0.0, 100.0) as u32)
}

/// A native `<progress>`; no `value` (or a non-finite one, an app's 0/0) is
/// the indeterminate bar, never a blank page.
#[component]
pub fn Progress(
    value: Option<f64>,
    #[props(default = 100.0)] max: f64,
    #[props(into)] label: Option<String>,
    #[props(default)] show_value: bool,
    #[props(default, into)] class: String,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> Element {
    let value = value.filter(|v| v.is_finite());
    let bar = rsx! {
        progress {
            class: join("aio-progress", &class),
            value: value.map(|v| v.to_string()),
            max: "{max}",
            "aria-label": label,
            ..attributes,
        }
    };
    match progress_pct(value, max).filter(|_| show_value) {
        None => bar,
        Some(p) => rsx! {
            span { class: "aio-progress-row",
                {bar}
                span { class: "aio-progress__value", "{p}%" }
            }
        },
    }
}

/// What a callout or toast says about itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tone {
    #[default]
    Info,
    Success,
    Warn,
    Error,
}

impl Tone {
    pub fn key(self) -> &'static str {
        ["info", "success", "warn", "error"][self as usize]
    }
}

/// A status callout in the page. Only an error interrupts (`role="alert"`);
/// the rest are notes. The dismiss button is there only with `ondismiss`.
#[component]
pub fn Alert(
    #[props(default)] tone: Tone,
    #[props(into)] title: Option<String>,
    ondismiss: Option<EventHandler<()>>,
    #[props(default, into)] class: String,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    rsx! {
        div {
            class: join(&format!("aio-alert aio-alert--{}", tone.key()), &class),
            role: if tone == Tone::Error { "alert" } else { "note" },
            ..attributes,
            div { class: "aio-alert__body",
                if let Some(t) = title {
                    strong { class: "aio-alert__title", "{t}" }
                }
                div { {children} }
            }
            if let Some(f) = ondismiss {
                button { r#type: "button", class: "aio-alert__x", "aria-label": "Dismiss", "data-t": "alertDismiss", onclick: move |_| f.call(()), "×" }
            }
        }
    }
}

/// What the connection pill says and how: `None` hides it. `lost`: the link
/// was down since the page opened (only then is "Connected" news).
pub fn conn_note(conn: crate::Conn, lost: bool) -> Option<(&'static str, Tone)> {
    use crate::Conn;
    match conn {
        Conn::Offline => Some(("Reconnecting…", Tone::Warn)),
        Conn::Blocked => Some(("Offline: too many changes wait, new ones are refused", Tone::Error)),
        Conn::Online | Conn::Syncing if lost => Some(("Connected", Tone::Success)),
        Conn::Online | Conn::Syncing => None,
    }
}

/// The page's connection pill (aio's built-in status overlay): "Reconnecting…"
/// while the link is down, then "Connected" for two seconds. Put it once in
/// the root: `ConnectionStatus { status: use_status()() }`.
#[component]
pub fn ConnectionStatus(
    status: crate::Status,
    #[props(default, into)] class: String,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> Element {
    let lost = use_hook(|| Rc::new(std::cell::Cell::new(false)));
    lost.set(lost.get() || matches!(status.conn, crate::Conn::Offline | crate::Conn::Blocked));
    let Some((text, tone)) = conn_note(status.conn, lost.get()) else { return rsx! {} };
    rsx! {
        div {
            class: join(&format!("aio-conn aio-conn--{}", tone.key()), &class),
            role: "status",
            "data-t": "connStatus",
            ..attributes,
            "{text}"
        }
    }
}

/// Where a tooltip sits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Placement {
    #[default]
    Top,
    Bottom,
}

/// The bubble id of the [`Tooltip`] around a kit control, which the control
/// is described by (`aria-describedby`).
#[derive(Clone, PartialEq)]
pub(super) struct Tip(pub String);

/// A tooltip on hover and on keyboard focus (CSS), closed by Escape and
/// back when the pointer or focus comes again. It describes the control a
/// screen reader is on, never its wrapper: a kit control inside takes the
/// bubble's id itself; a plain element takes `bubble_id` (yours, from
/// [`use_id`], kept from the first render) as its `aria-describedby`.
#[component]
pub fn Tooltip(
    #[props(into)] text: String,
    #[props(default)] placement: Placement,
    #[props(into)] bubble_id: Option<String>,
    #[props(default, into)] class: String,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let own = use_id("tip");
    let id = bubble_id.unwrap_or(own);
    use_context_provider(|| Tip(id.clone()));
    let mut hidden = use_signal(|| false);
    let side = if placement == Placement::Top { "top" } else { "bottom" };
    rsx! {
        span {
            class: join(&format!("aio-tip aio-tip--{side}"), &class),
            "data-hidden": hidden().then_some(""),
            onkeydown: move |e| if e.key() == Key::Escape { hidden.set(true) },
            onfocusout: move |_| hidden.set(false),
            onmouseleave: move |_| hidden.set(false),
            ..attributes,
            span { class: "aio-tip__trigger", {children} }
            span { class: "aio-tip__bubble", role: "tooltip", id: "{id}", "{text}" }
        }
    }
}

/// One action of a [`Menu`].
#[derive(Clone, Debug, PartialEq)]
pub struct MenuItem {
    pub id: String,
    pub label: String,
    pub disabled: bool,
    pub danger: bool,
}

impl MenuItem {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self { id: id.into(), label: label.into(), disabled: false, danger: false }
    }

    pub fn disabled(self) -> Self {
        Self { disabled: true, ..self }
    }

    pub fn danger(self) -> Self {
        Self { danger: true, ..self }
    }
}

/// The enabled item from `i` walking `dir` (wrapping); a disabled one can't take focus.
fn menu_step(items: &[MenuItem], i: isize, dir: isize) -> Option<usize> {
    let n = items.len() as isize;
    (0..n).map(|s| (i + dir * s).rem_euclid(n.max(1)) as usize).find(|&k| !items[k].disabled)
}

/// An actions menu: Enter, Space or ArrowDown open it on the first item
/// (ArrowUp on the last), arrows and Home/End move, Escape closes it and
/// puts focus back on the trigger, Tab or a click outside closes it.
#[component]
pub fn Menu(
    trigger: Element,
    items: Vec<MenuItem>,
    /// The trigger's name when it isn't text.
    #[props(into)]
    label: Option<String>,
    onselect: Option<EventHandler<String>>,
    #[props(default, into)] class: String,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> Element {
    let id = use_id("menu");
    let mut open = use_signal(|| false);
    let mut want = use_signal(|| None::<usize>);
    let mut els = use_signal(Vec::<Option<Rc<MountedData>>>::new);
    let mut button = use_signal(|| None::<Rc<MountedData>>);
    let focus = move |el: Option<Rc<MountedData>>| {
        if let Some(el) = el {
            spawn(async move { _ = el.set_focus(true).await });
        }
    };
    let n = items.len();
    // Focus item `i` walking `dir`, opening the menu first if need be.
    let go = {
        let items = items.clone();
        use_callback(move |(i, dir): (isize, isize)| {
            let to = menu_step(&items, i, dir);
            if *open.peek() {
                focus(to.and_then(|k| els.peek().get(k).cloned().flatten()));
            } else {
                want.set(to);
                open.set(true);
            }
        })
    };
    let mut close = move |refocus: bool| {
        if open() {
            open.set(false);
            if refocus {
                focus(button.peek().clone());
            }
        }
    };
    rsx! {
        div {
            class: join("aio-menu", &class),
            onkeydown: move |e| match e.key() {
                // Only an open menu takes Escape: a dialog around it gets the next one.
                Key::Escape if open() => {
                    e.prevent_default();
                    e.stop_propagation();
                    close(true);
                }
                Key::Tab => close(false),
                _ => {}
            },
            ..attributes,
            button {
                r#type: "button",
                class: "aio-menu__trigger",
                "data-t": "menuTrigger",
                "aria-haspopup": "menu",
                "aria-expanded": if open() { "true" } else { "false" },
                "aria-controls": open().then(|| id.clone()),
                "aria-label": label.clone(),
                onmounted: move |e| button.set(Some(e.data())),
                onclick: move |_| if open() { close(true) } else { go((0, 1)) },
                onkeydown: move |e| match e.key() {
                    Key::ArrowDown => {
                        e.prevent_default();
                        go((0, 1));
                    }
                    Key::ArrowUp => {
                        e.prevent_default();
                        go((n as isize - 1, -1));
                    }
                    _ => {}
                },
                {trigger}
            }
            if open() {
                div { class: "aio-menu__list", role: "menu", id: "{id}", "aria-label": label.clone(),
                    for (i, it) in items.iter().enumerate() {
                        button {
                            key: "{it.id}",
                            "data-t": "{it.id}",
                            r#type: "button",
                            role: "menuitem",
                            class: if it.danger { "aio-menu__item aio-menu__item--danger" } else { "aio-menu__item" },
                            tabindex: "-1",
                            disabled: flag(it.disabled),
                            onmounted: move |e: MountedEvent| {
                                let mut v = els.write();
                                v.resize(n, None);
                                v[i] = Some(e.data());
                                if *want.peek() == Some(i) {
                                    want.set(None);
                                    focus(Some(e.data()));
                                }
                            },
                            onclick: {
                                let (id, off) = (it.id.clone(), it.disabled);
                                move |_| if !off {
                                    close(true);
                                    if let Some(f) = onselect { f.call(id.clone()) }
                                }
                            },
                            onkeydown: move |e: KeyboardEvent| {
                                let (to, dir) = match e.key() {
                                    Key::ArrowDown => (i as isize + 1, 1),
                                    Key::ArrowUp => (i as isize - 1, -1),
                                    Key::Home => (0, 1),
                                    Key::End => (n as isize - 1, -1),
                                    _ => return,
                                };
                                e.prevent_default();
                                go((to, dir));
                            },
                            "{it.label}"
                        }
                    }
                }
                // After the list, under it by z-index: a click outside lands here.
                div { class: "aio-menu__scrim", "data-t": "menuScrim", onclick: move |_| close(false) }
            }
        }
    }
}

/// One step of a [`Breadcrumb`]: a link (`href` or `onclick`), or plain text.
#[derive(Clone, PartialEq)]
pub struct Crumb {
    pub label: String,
    pub href: Option<String>,
    pub onclick: Option<EventHandler<()>>,
}

impl Crumb {
    pub fn new(label: impl Into<String>) -> Self {
        Self { label: label.into(), href: None, onclick: None }
    }

    pub fn href(self, href: impl Into<String>) -> Self {
        Self { href: Some(href.into()), ..self }
    }

    pub fn onclick(self, f: EventHandler<()>) -> Self {
        Self { onclick: Some(f), ..self }
    }
}

/// A breadcrumb trail; the last crumb is the page you're on
/// (`aria-current="page"`), never a link to itself.
#[component]
pub fn Breadcrumb(
    items: Vec<Crumb>,
    #[props(default = String::from("/"), into)] separator: String,
    #[props(default = String::from("Breadcrumb"), into)] label: String,
    #[props(default, into)] class: String,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> Element {
    let last = items.len().saturating_sub(1);
    rsx! {
        nav { class: join("aio-crumbs", &class), "aria-label": label, ..attributes,
            ol { class: "aio-crumbs__list",
                for (i, c) in items.into_iter().enumerate() {
                    li { key: "{i}", class: "aio-crumbs__item",
                        if i == last || (c.href.is_none() && c.onclick.is_none()) {
                            span { "aria-current": (i == last).then_some("page"), "{c.label}" }
                        } else {
                            a { href: c.href, onclick: move |_| if let Some(f) = c.onclick { f.call(()) }, "{c.label}" }
                        }
                        if i != last {
                            span { class: "aio-crumbs__sep", "aria-hidden": "true", "{separator}" }
                        }
                    }
                }
            }
        }
    }
}

/// A loading placeholder, hidden from screen readers (put `aria-busy` on
/// the region instead). `width`/`height` are CSS lengths.
#[component]
pub fn Skeleton(
    #[props(into)] width: Option<String>,
    #[props(into)] height: Option<String>,
    #[props(default)] circle: bool,
    #[props(default = 1)] lines: u32,
    #[props(default, into)] class: String,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> Element {
    let cls = join(if circle { "aio-skel aio-skel--circle" } else { "aio-skel" }, &class);
    let style = [width.map(|w| format!("width:{w}")), height.map(|h| format!("height:{h}"))]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(";");
    if lines <= 1 {
        return rsx! {
            span { class: cls, "aria-hidden": "true", style, ..attributes }
        };
    }
    rsx! {
        span { class: "aio-skel-stack", "aria-hidden": "true", ..attributes,
            for i in 0..lines {
                span { key: "{i}", class: cls.clone(), style: style.clone() }
            }
        }
    }
}

/// What an empty list shows: what happened, and what to do about it.
#[component]
pub fn EmptyState(
    #[props(into)] title: String,
    /// A glyph; decorative.
    #[props(into)]
    icon: Option<String>,
    #[props(into)] description: Option<String>,
    action: Option<Element>,
    #[props(default, into)] class: String,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> Element {
    rsx! {
        div { class: join("aio-empty", &class), ..attributes,
            if let Some(i) = icon {
                div { class: "aio-empty__icon", "aria-hidden": "true", "{i}" }
            }
            p { class: "aio-empty__title", "{title}" }
            if let Some(d) = description {
                p { class: "aio-empty__desc", "{d}" }
            }
            if let Some(a) = action {
                div { class: "aio-empty__action", {a} }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_steps_wrap_and_skip_disabled_tabs() {
        let d = [false, true, false, false];
        assert_eq!(tab_step(&d, 0, 1), Some(2), "skips the disabled one");
        assert_eq!(tab_step(&d, 3, 1), Some(0), "wraps forward");
        assert_eq!(tab_step(&d, 0, -1), Some(3), "wraps back");
        assert_eq!(tab_step(&d, -1, 1), Some(0), "Home");
        assert_eq!(tab_step(&d, 4, -1), Some(3), "End");
        assert_eq!(tab_step(&[true, true], 0, 1), None);
        assert_eq!(tab_step(&[], 0, 1), None);
    }

    #[test]
    fn a_tab_id_is_one_dom_id() {
        assert_eq!(dom_id("General settings"), "General%20settings");
        assert_ne!(dom_id("a b"), dom_id("a%20b"));
    }

    #[test]
    fn the_percentage_never_contradicts_the_bar() {
        assert_eq!(progress_pct(Some(42.0), 100.0), Some(42));
        assert_eq!(progress_pct(Some(150.0), 100.0), Some(100));
        assert_eq!(progress_pct(Some(-20.0), 100.0), Some(0));
        assert_eq!(progress_pct(Some(f64::NAN), 100.0), None);
        assert_eq!(progress_pct(Some(1.0), 0.0), None);
        assert_eq!(progress_pct(None, 100.0), None);
    }

    #[test]
    fn menu_focus_skips_disabled_items() {
        let items = [MenuItem::new("a", "A"), MenuItem::new("b", "B").disabled(), MenuItem::new("c", "C")];
        assert_eq!(menu_step(&items, 1, 1), Some(2));
        assert_eq!(menu_step(&items, 1, -1), Some(0));
        assert_eq!(menu_step(&items, 3, 1), Some(0), "wraps");
    }
}

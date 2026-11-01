//! Buttons, inputs and the field that labels them (aio's `Button`, `Input`,
//! `Textarea`, `Select`, `Checkbox`, `Field`). A `Field` hands its label,
//! hint, error and `required` to the control inside it by context: the label
//! targets the control (`for`), the control is named, described by the hint or
//! error, `aria-invalid` and `aria-required`. A caller's own attribute wins.
use super::{flag, join, keys::field_key, use_id};
use dioxus::prelude::*;

/// What a button is for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Variant {
    Primary,
    #[default]
    Secondary,
    Ghost,
    Danger,
}

/// How big a button is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Size {
    Sm,
    #[default]
    Md,
    Lg,
}

impl Variant {
    pub fn key(self) -> &'static str {
        ["primary", "secondary", "ghost", "danger"][self as usize]
    }
}

impl Size {
    pub fn key(self) -> &'static str {
        ["sm", "md", "lg"][self as usize]
    }
}

/// A button: `type="button"` unless said, so it never submits a form by accident.
#[component]
pub fn Button(
    #[props(default)] variant: Variant,
    #[props(default)] size: Size,
    #[props(default = String::from("button"), into)] r#type: String,
    #[props(default)] disabled: bool,
    #[props(default, into)] class: String,
    onclick: Option<EventHandler<MouseEvent>>,
    /// Its element, to focus it later.
    onmounted: Option<EventHandler<MountedEvent>>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let cls = join(&format!("aio-btn aio-btn--{} aio-btn--{}", variant.key(), size.key()), &class);
    let tip = use_tip();
    rsx! {
        button {
            r#type,
            class: cls,
            "aria-describedby": tip,
            disabled: flag(disabled),
            onclick: move |e| if let Some(f) = onclick { f.call(e) },
            onmounted: move |e| if let Some(f) = onmounted { f.call(e) },
            ..attributes,
            {children}
        }
    }
}

/// What a `Field` tells the control inside it: signals, so a control that
/// didn't re-render (same props) still follows a new error.
#[derive(Clone, Copy)]
struct FieldCx {
    said: Signal<Said>,
    /// Whether a control took the label's `for` (the first one does).
    claimed: Signal<bool>,
}

#[derive(Clone, Default, PartialEq)]
struct Said {
    base: String,
    label: Option<String>,
    /// The id of the hint or error that describes the control.
    described: Option<String>,
    error: bool,
    required: bool,
}

/// The attributes a control takes from its field.
#[derive(Default)]
pub(super) struct Wiring {
    pub id: Option<String>,
    pub label: Option<String>,
    pub described: Option<String>,
    pub invalid: Option<&'static str>,
    pub required: Option<&'static str>,
}

/// The bubble id of the tooltip around a control, if any.
fn use_tip() -> Option<String> {
    try_use_context::<super::controls::Tip>().map(|t| t.0)
}

/// A control's wiring from the field around it, if any. `named`: it names
/// itself (its own label); `group`: a radio group (named by the field, no
/// `for`, no invalid or required).
pub(super) fn use_field(named: bool, group: bool) -> Wiring {
    let tip = use_tip();
    let cx = try_use_context::<FieldCx>();
    let mine = use_hook(|| {
        cx.is_some_and(|mut f| {
            let free = !named && !group && f.said.peek().label.is_some() && !*f.claimed.peek();
            if free {
                f.claimed.set(true);
            }
            free
        })
    });
    let Some(f) = cx else { return Wiring { described: tip, ..Wiring::default() } };
    let s = f.said.read();
    let described = match (s.described.clone(), tip) {
        (Some(a), Some(b)) => Some(format!("{a} {b}")),
        (a, b) => a.or(b),
    };
    Wiring {
        id: mine.then(|| format!("{}-control", s.base)),
        label: if group { s.label.clone() } else { None },
        described,
        invalid: (s.error && !group).then_some("true"),
        required: (s.required && !group).then_some("true"),
    }
}

/// A label, an optional hint, and an error that turns the field invalid.
#[component]
pub fn Field(
    #[props(into)] label: Option<String>,
    #[props(into)] hint: Option<String>,
    #[props(into)] error: Option<String>,
    #[props(default)] required: bool,
    #[props(default, into)] class: String,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let base = use_id("field");
    let label = label.map(|l| l.trim().to_owned()).filter(|l| !l.is_empty());
    let error = error.filter(|e| !e.is_empty());
    let described = match (&error, &hint) {
        (Some(_), _) => Some(format!("{base}-error")),
        (None, Some(_)) => Some(format!("{base}-hint")),
        _ => None,
    };
    let now = Said { base: base.clone(), label: label.clone(), described, error: error.is_some(), required };
    let cx = use_context_provider(|| FieldCx { said: Signal::new(now.clone()), claimed: Signal::new(false) });
    let mut said = cx.said;
    if *said.peek() != now {
        said.set(now);
    }
    let target = (cx.claimed)().then(|| format!("{base}-control"));
    rsx! {
        div { class: join("aio-field", &class), ..attributes,
            if let Some(l) = &label {
                label { class: "aio-field__label", r#for: target,
                    "{l}"
                    if required {
                        span { class: "aio-field__req", " *" }
                    }
                }
            }
            {children}
            if let Some(e) = &error {
                div { class: "aio-field__error", id: "{base}-error", "{e}" }
            } else if let Some(h) = &hint {
                div { class: "aio-field__hint", id: "{base}-hint", "{h}" }
            }
        }
    }
}

/// A one-line text input; `oninput` and `onchange` get the value.
#[component]
pub fn Input(
    #[props(into)] value: Option<String>,
    #[props(default = String::from("text"), into)] r#type: String,
    #[props(into)] placeholder: Option<String>,
    #[props(default)] disabled: bool,
    #[props(default)] invalid: bool,
    #[props(default, into)] class: String,
    oninput: Option<EventHandler<String>>,
    onchange: Option<EventHandler<String>>,
    onkeydown: Option<EventHandler<KeyboardEvent>>,
    #[props(extends = GlobalAttributes)] mut attributes: Vec<Attribute>,
) -> Element {
    let w = use_field(false, false);
    let invalid = invalid || w.invalid.is_some();
    // Only a controlled input has a value attribute: Dioxus rewrites `value`
    // on each render, and an absent one clears what was typed.
    if let Some(v) = value {
        attributes.push(Attribute::new("value", v, None, true));
    }
    rsx! {
        input {
            r#type,
            class: join(&format!("aio-input{}", if invalid { " aio-input--invalid" } else { "" }), &class),
            placeholder,
            disabled: flag(disabled),
            id: w.id,
            "aria-describedby": w.described,
            "aria-invalid": invalid.then_some("true"),
            "aria-required": w.required,
            oninput: move |e| if let Some(f) = oninput { f.call(e.value()) },
            onchange: move |e| if let Some(f) = onchange { f.call(e.value()) },
            onkeydown: move |e| {
                field_key(&e);
                if let Some(f) = onkeydown {
                    f.call(e)
                }
            },
            ..attributes,
        }
    }
}

/// A multi-line text input; `oninput` gets the value.
#[component]
pub fn Textarea(
    #[props(default, into)] value: String,
    #[props(into)] placeholder: Option<String>,
    #[props(default = 3)] rows: u32,
    #[props(default)] disabled: bool,
    #[props(default)] invalid: bool,
    #[props(default, into)] class: String,
    oninput: Option<EventHandler<String>>,
    onkeydown: Option<EventHandler<KeyboardEvent>>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> Element {
    let w = use_field(false, false);
    let invalid = invalid || w.invalid.is_some();
    rsx! {
        textarea {
            class: join(&format!("aio-input aio-textarea{}", if invalid { " aio-input--invalid" } else { "" }), &class),
            value,
            placeholder,
            rows: "{rows}",
            disabled: flag(disabled),
            id: w.id,
            "aria-describedby": w.described,
            "aria-invalid": invalid.then_some("true"),
            "aria-required": w.required,
            oninput: move |e| if let Some(f) = oninput { f.call(e.value()) },
            onkeydown: move |e| {
                field_key(&e);
                if let Some(f) = onkeydown {
                    f.call(e)
                }
            },
            ..attributes,
        }
    }
}

/// One choice of a [`Select`].
#[derive(Clone, Debug, PartialEq)]
pub struct Opt {
    pub value: String,
    pub label: String,
    pub disabled: bool,
}

impl<S: Into<String>> From<S> for Opt {
    fn from(v: S) -> Self {
        let value = v.into();
        Self { label: value.clone(), value, disabled: false }
    }
}

/// A dropdown; `onchange` gets the chosen value.
#[component]
pub fn Select(
    #[props(into)] options: Vec<Opt>,
    #[props(default, into)] value: String,
    #[props(default)] disabled: bool,
    #[props(default)] invalid: bool,
    #[props(default, into)] class: String,
    onchange: Option<EventHandler<String>>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> Element {
    let w = use_field(false, false);
    let invalid = invalid || w.invalid.is_some();
    rsx! {
        select {
            class: join(&format!("aio-input aio-select{}", if invalid { " aio-input--invalid" } else { "" }), &class),
            value: "{value}",
            disabled: flag(disabled),
            id: w.id,
            "aria-describedby": w.described,
            "aria-invalid": invalid.then_some("true"),
            "aria-required": w.required,
            onchange: move |e| if let Some(f) = onchange { f.call(e.value()) },
            onkeydown: move |e| field_key(&e),
            ..attributes,
            for o in options {
                option { key: "{o.value}", value: "{o.value}", disabled: flag(o.disabled), selected: flag(o.value == value), "{o.label}" }
            }
        }
    }
}

/// A checkbox, with an optional label beside it; `onchange` gets the new state.
#[component]
pub fn Checkbox(
    #[props(default)] checked: bool,
    #[props(default)] disabled: bool,
    #[props(into)] label: Option<String>,
    #[props(default, into)] class: String,
    onchange: Option<EventHandler<bool>>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> Element {
    let w = use_field(label.is_some(), false);
    let own = label.as_deref().map(str::trim).filter(|l| !l.is_empty()).map(String::from);
    let box_cls = if label.is_none() { join("aio-checkbox", &class) } else { "aio-checkbox".into() };
    let input = rsx! {
        input {
            r#type: "checkbox",
            class: box_cls,
            checked,
            disabled: flag(disabled),
            id: w.id,
            "aria-label": own,
            "aria-describedby": w.described,
            "aria-invalid": w.invalid,
            "aria-required": w.required,
            oninput: move |e| if let Some(f) = onchange { f.call(e.checked()) },
            ..attributes,
        }
    };
    match label {
        None => input,
        Some(l) => rsx! {
            label { class: join("aio-checkbox-row", &class),
                {input}
                span { "{l}" }
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_and_sizes_are_their_class_names() {
        assert_eq!((Variant::default().key(), Variant::Danger.key()), ("secondary", "danger"));
        assert_eq!((Size::default().key(), Size::Lg.key()), ("md", "lg"));
        assert_eq!(Opt::from("a"), Opt { value: "a".into(), label: "a".into(), disabled: false });
    }
}

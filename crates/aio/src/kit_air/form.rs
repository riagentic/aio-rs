//! Buttons, inputs and the field that labels them (aio's `Button`, `Input`,
//! `Textarea`, `Select`, `Checkbox`, `Field`). A `Field` hands its label,
//! hint, error and `required` to the control inside it by context: the label
//! targets the control (`for`), the control is named, described by the hint or
//! error, `aria-invalid` and `aria-required`. A caller's own attribute wins.
//! Every prop that may change is a [`Prop`]: the control follows it.
use super::{join, keys::field_key, use_id};
use crate::ui::prelude::*;

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

/// A kit button's classes.
pub(super) fn btn_class(variant: Variant, size: Size, class: &str) -> String {
    join(&format!("aio-btn aio-btn--{} aio-btn--{}", variant.key(), size.key()), class)
}

/// A button: `type="button"` unless said, so it never submits a form by accident.
#[component]
pub fn Button(
    #[props(default)] variant: Prop<Variant>,
    #[props(default)] size: Prop<Size>,
    #[props(default = String::from("button"), into)] r#type: Prop<String>,
    #[props(default)] disabled: Prop<bool>,
    #[props(default, into)] class: Prop<String>,
    onclick: Option<EventHandler<MouseEvent>>,
    /// Its element, to focus it later.
    onmounted: Option<EventHandler<MountedEvent>>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: View,
) -> View {
    let tip = use_tip();
    rsx! {
        button {
            r#type,
            class: btn_class(variant.get(), size.get(), &class.get()),
            "aria-describedby": tip,
            disabled,
            onclick: move |e| if let Some(f) = onclick { f.call(e) },
            onmounted: move |e| if let Some(f) = onmounted { f.call(e) },
            ..attributes,
            {children}
        }
    }
}

/// What a `Field` tells the control inside it.
#[derive(Clone, Copy)]
struct FieldCx {
    said: Memo<Said>,
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

/// What a control takes from the field around it (read in its holes:
/// each follows the field).
#[derive(Clone, Default)]
pub(super) struct Wiring {
    /// The control's id, if it took the label's `for`.
    pub id: Option<String>,
    cx: Option<FieldCx>,
    tip: Option<String>,
    group: bool,
}

impl Wiring {
    /// The hint's or error's id and the tooltip's, if any.
    pub fn described(&self) -> Option<String> {
        let own = self.cx.and_then(|f| f.said.with(|s| s.described.clone()));
        match (own, self.tip.clone()) {
            (Some(a), Some(b)) => Some(format!("{a} {b}")),
            (a, b) => a.or(b),
        }
    }

    /// The field's label, for a group (a group is named by its field).
    pub fn label(&self) -> Option<String> {
        self.cx.filter(|_| self.group).and_then(|f| f.said.with(|s| s.label.clone()))
    }

    /// The field has an error.
    pub fn invalid(&self) -> bool {
        !self.group && self.cx.is_some_and(|f| f.said.with(|s| s.error))
    }

    pub fn required(&self) -> Option<&'static str> {
        (!self.group && self.cx.is_some_and(|f| f.said.with(|s| s.required))).then_some("true")
    }
}

/// The bubble id of the tooltip around a control, if any.
fn use_tip() -> Option<String> {
    try_context::<super::controls::Tip>().map(|t| t.0)
}

/// A control's wiring from the field around it, if any. `named`: it names
/// itself (its own label); `group`: a radio group (named by the field, no
/// `for`, no invalid or required).
pub(super) fn use_field(named: bool, group: bool) -> Wiring {
    let tip = use_tip();
    let cx = try_context::<FieldCx>();
    let mine = once(|| {
        cx.is_some_and(|f| {
            let free = !named && !group && f.said.with_untracked(|s| s.label.is_some()) && !f.claimed.get_untracked();
            if free {
                f.claimed.set(true);
            }
            free
        })
    });
    let id = cx.filter(|_| mine).map(|f| f.said.with_untracked(|s| format!("{}-control", s.base)));
    Wiring { id, cx, tip, group }
}

/// A label, an optional hint, and an error that turns the field invalid.
#[component]
pub fn Field(
    #[props(into)] label: Prop<Option<String>>,
    #[props(into)] hint: Prop<Option<String>>,
    #[props(into)] error: Prop<Option<String>>,
    #[props(default)] required: Prop<bool>,
    #[props(default, into)] class: Prop<String>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: View,
) -> View {
    let base = use_id("field");
    let label = {
        let label = label.clone();
        memo(move || label.get().map(|l| l.trim().to_owned()).filter(|l| !l.is_empty()))
    };
    let error = {
        let error = error.clone();
        memo(move || error.get().filter(|e| !e.is_empty()))
    };
    let said = {
        let (base, hint, required) = (base.clone(), hint.clone(), required.clone());
        memo(move || Said {
            base: base.clone(),
            label: label.get(),
            described: match (error.with(Option::is_some), hint.with(Option::is_some)) {
                (true, _) => Some(format!("{base}-error")),
                (false, true) => Some(format!("{base}-hint")),
                _ => None,
            },
            error: error.with(Option::is_some),
            required: required.get(),
        })
    };
    let cx = provide(|| FieldCx { said, claimed: Signal::new(false) });
    let claimed = cx.claimed;
    rsx! {
        div { class: join("aio-field", &class.get()), ..attributes,
            if label.with(Option::is_some) {
                label { class: "aio-field__label", r#for: claimed.get().then(|| format!("{base}-control")),
                    {label.get()}
                    if required.get() {
                        span { class: "aio-field__req", " *" }
                    }
                }
            }
            {children}
            if error.with(Option::is_some) {
                div { class: "aio-field__error", id: "{base}-error", {error.get()} }
            } else if hint.with(Option::is_some) {
                div { class: "aio-field__hint", id: "{base}-hint", {hint.get()} }
            }
        }
    }
}

/// An input's classes.
fn input_class(base: &str, invalid: bool, class: &str) -> String {
    join(&format!("{base}{}", if invalid { " aio-input--invalid" } else { "" }), class)
}

/// A one-line text input; `oninput` and `onchange` get the value. Only a
/// controlled input (`value` given) has its value set by the view.
#[component]
pub fn Input(
    #[props(into)] value: Prop<Option<String>>,
    #[props(default = String::from("text"), into)] r#type: Prop<String>,
    #[props(into)] placeholder: Prop<Option<String>>,
    #[props(default)] disabled: Prop<bool>,
    #[props(default)] invalid: Prop<bool>,
    #[props(default, into)] class: Prop<String>,
    oninput: Option<EventHandler<String>>,
    onchange: Option<EventHandler<String>>,
    onkeydown: Option<EventHandler<KeyboardEvent>>,
    #[props(extends = GlobalAttributes)] mut attributes: Vec<Attribute>,
) -> View {
    let w = use_field(false, false);
    if value.is_live() || value.with_untracked(Option::is_some) {
        attributes.insert(0, Attribute::live("value", move || value.get().unwrap_or_default()));
    }
    let bad = {
        let w = w.clone();
        memo(move || invalid.get() || w.invalid())
    };
    rsx! {
        input {
            r#type,
            class: input_class("aio-input", bad.get(), &class.get()),
            placeholder,
            disabled,
            id: w.id.clone(),
            "aria-describedby": w.described(),
            "aria-invalid": bad.get().then_some("true"),
            "aria-required": w.required(),
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
    #[props(default, into)] value: Prop<String>,
    #[props(into)] placeholder: Prop<Option<String>>,
    #[props(default = 3)] rows: Prop<u32>,
    #[props(default)] disabled: Prop<bool>,
    #[props(default)] invalid: Prop<bool>,
    #[props(default, into)] class: Prop<String>,
    oninput: Option<EventHandler<String>>,
    onkeydown: Option<EventHandler<KeyboardEvent>>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> View {
    let w = use_field(false, false);
    let bad = {
        let w = w.clone();
        memo(move || invalid.get() || w.invalid())
    };
    rsx! {
        textarea {
            class: input_class("aio-input aio-textarea", bad.get(), &class.get()),
            value,
            placeholder,
            rows,
            disabled,
            id: w.id.clone(),
            "aria-describedby": w.described(),
            "aria-invalid": bad.get().then_some("true"),
            "aria-required": w.required(),
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
    #[props(into)] options: Prop<Vec<Opt>>,
    #[props(default, into)] value: Prop<String>,
    #[props(default)] disabled: Prop<bool>,
    #[props(default)] invalid: Prop<bool>,
    #[props(default, into)] class: Prop<String>,
    onchange: Option<EventHandler<String>>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> View {
    let w = use_field(false, false);
    let bad = {
        let w = w.clone();
        memo(move || invalid.get() || w.invalid())
    };
    rsx! {
        select {
            class: input_class("aio-input aio-select", bad.get(), &class.get()),
            disabled,
            id: w.id.clone(),
            "aria-describedby": w.described(),
            "aria-invalid": bad.get().then_some("true"),
            "aria-required": w.required(),
            onchange: move |e| if let Some(f) = onchange { f.call(e.value()) },
            onkeydown: move |e| field_key(&e),
            ..attributes,
            for o in options.get() {
                option { key: "{o.value}", value: "{o.value}", disabled: o.disabled, selected: o.value == value.get(), "{o.label}" }
            }
        }
    }
}

/// A checkbox, with an optional label beside it; `onchange` gets the new state.
#[component]
pub fn Checkbox(
    #[props(default)] checked: Prop<bool>,
    #[props(default)] disabled: Prop<bool>,
    #[props(into)] label: Prop<Option<String>>,
    #[props(default, into)] class: Prop<String>,
    onchange: Option<EventHandler<bool>>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> View {
    let labelled = label.with_untracked(Option::is_some);
    let w = use_field(labelled, false);
    let own = {
        let label = label.clone();
        memo(move || label.get().map(|l| l.trim().to_owned()).filter(|l| !l.is_empty()))
    };
    let input = rsx! {
        input {
            r#type: "checkbox",
            class: if labelled { "aio-checkbox".into() } else { join("aio-checkbox", &class.get()) },
            checked,
            disabled,
            id: w.id.clone(),
            "aria-label": own,
            "aria-describedby": w.described(),
            "aria-invalid": w.invalid().then_some("true"),
            "aria-required": w.required(),
            oninput: move |e| if let Some(f) = onchange { f.call(e.checked()) },
            ..attributes,
        }
    };
    if !labelled {
        return input;
    }
    rsx! {
        label { class: join("aio-checkbox-row", &class.get()),
            {input}
            span { {label.get()} }
        }
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

//! Components' props: the builder `rsx!` fills (`#[component]` and
//! `#[derive(Props)]` make it), optional and default props, props that
//! follow state ([`Prop`]), and attributes handed through a component
//! ([`Attribute`], `..attributes`).
//!
//! `Name { a: x, "data-t": "t", child }` in `rsx!` is
//! `Name(builder().a(move || x).__attr("data-t", move || "t").children(…).build())`:
//! every value is a closure, so a prop typed [`Prop`] (or an attribute)
//! follows what it reads and every other prop takes its value once. A
//! required prop left out is a compile error (`build` needs it).
use crate::dom::{AttrValue, IntoAttr, attr, set_attr_now};
use crate::reactive::{Callback, Memo, Signal, memo_now, reads};

/// A props struct `rsx!` can build: `#[component]` and `#[derive(Props)]` make one.
pub trait Props: Sized {
    type Builder;
    fn builder() -> Self::Builder;
}

/// A component with no props.
impl Props for () {
    type Builder = NoPropsBuilder;
    fn builder() -> NoPropsBuilder {
        NoPropsBuilder
    }
}

#[doc(hidden)]
pub struct NoPropsBuilder;

impl NoPropsBuilder {
    pub fn build(self) {}
}

/// A required prop that was given (the builder's type says which are).
#[doc(hidden)]
pub struct Set;

/// The props builder of component `c` (its props type found from its signature).
#[doc(hidden)]
pub fn builder_of<P: Props, M, C: crate::Component<P, M>>(_: &C) -> P::Builder {
    P::builder()
}

#[doc(hidden)]
pub struct ByVal;
#[doc(hidden)]
pub struct ByOpt;
#[doc(hidden)]
pub struct BySignal;
#[doc(hidden)]
pub struct ByMemo;
#[doc(hidden)]
pub struct ByProp;

/// What an optional prop (`Option<T>`) takes: a `T` (or what converts to
/// one), an `Option` of one, a signal of an `Option` (read, so followed
/// where the prop follows).
pub trait IntoOpt<T, M> {
    fn into_opt(self) -> Option<T>;
}
impl<T, X: Into<T>> IntoOpt<T, ByVal> for X {
    fn into_opt(self) -> Option<T> {
        Some(self.into())
    }
}
impl<T, X: Into<T>> IntoOpt<T, ByOpt> for Option<X> {
    fn into_opt(self) -> Option<T> {
        self.map(Into::into)
    }
}
impl<T: Clone + 'static> IntoOpt<T, BySignal> for Signal<Option<T>> {
    fn into_opt(self) -> Option<T> {
        self.get()
    }
}
impl<T: Clone + 'static> IntoOpt<T, ByMemo> for Memo<Option<T>> {
    fn into_opt(self) -> Option<T> {
        self.get()
    }
}
impl<T: Clone + 'static> IntoOpt<T, ByProp> for Prop<Option<T>> {
    fn into_opt(self) -> Option<T> {
        self.get()
    }
}

/// What a [`Prop`] takes: a `T` (or what converts to one), or a signal,
/// memo or prop of one (read: the prop follows it).
pub trait IntoValue<T, M> {
    fn into_value(self) -> T;
}
impl<T, X: Into<T>> IntoValue<T, ByVal> for X {
    fn into_value(self) -> T {
        self.into()
    }
}
impl<T: Clone + 'static> IntoValue<T, BySignal> for Signal<T> {
    fn into_value(self) -> T {
        self.get()
    }
}
impl<T: Clone + 'static> IntoValue<T, ByMemo> for Memo<T> {
    fn into_value(self) -> T {
        self.get()
    }
}
impl<T: Clone + 'static> IntoValue<T, ByProp> for Prop<T> {
    fn into_value(self) -> T {
        self.get()
    }
}

/// A handler prop's argument and answer, for typing a closure given to it.
#[doc(hidden)]
pub trait CallbackOf {
    type A;
    type R;
}
impl<A, R> CallbackOf for Callback<A, R> {
    type A = A;
    type R = R;
}
impl<C: CallbackOf> CallbackOf for Option<C> {
    type A = C::A;
    type R = C::R;
}

// ------------------------------------------------------------------ Prop

/// A prop that follows state: given `value: name.get()`, the component
/// sees each new value (read it in a hole: `"{value}"`, `value.get()` in an
/// attribute). Given a plain value, it is that value and costs nothing.
pub enum Prop<T: 'static> {
    Const(T),
    Live(Memo<T>),
}

impl<T: Clone> Clone for Prop<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Const(v) => Self::Const(v.clone()),
            Self::Live(m) => Self::Live(*m),
        }
    }
}
impl<T: Copy> Copy for Prop<T> {}

impl<T: PartialEq + Clone> PartialEq for Prop<T> {
    /// The same value now (untracked).
    fn eq(&self, other: &Self) -> bool {
        self.get_untracked() == other.get_untracked()
    }
}

impl<T: Default> Default for Prop<T> {
    fn default() -> Self {
        Self::Const(T::default())
    }
}

impl<T> From<T> for Prop<T> {
    fn from(v: T) -> Self {
        Self::Const(v)
    }
}

impl<T: PartialEq + Clone> From<Signal<T>> for Prop<T> {
    fn from(s: Signal<T>) -> Self {
        Self::derive(Box::new(move || s.get()))
    }
}

impl<T> From<Memo<T>> for Prop<T> {
    fn from(m: Memo<T>) -> Self {
        Self::Live(m)
    }
}

impl<T: 'static> Prop<T> {
    /// `f()`, again whenever what it read changes; a constant if it read nothing.
    pub fn derive(f: Box<dyn Fn() -> T>) -> Self
    where
        T: PartialEq,
    {
        let (v, live) = reads(&f);
        if live { Self::Live(memo_now(f)) } else { Self::Const(v) }
    }

    /// `f` of the value; subscribes.
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        match self {
            Self::Const(v) => f(v),
            Self::Live(m) => m.with(f),
        }
    }

    pub fn with_untracked<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        match self {
            Self::Const(v) => f(v),
            Self::Live(m) => m.with_untracked(f),
        }
    }

    /// The value, cloned; subscribes.
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.with(T::clone)
    }

    pub fn get_untracked(&self) -> T
    where
        T: Clone,
    {
        self.with_untracked(T::clone)
    }

    /// Does it follow something (else it is a constant)?
    pub fn is_live(&self) -> bool {
        matches!(self, Self::Live(_))
    }
}

impl<T: std::fmt::Display> std::fmt::Display for Prop<T> {
    /// Subscribes: `"{value}"` in a view follows the prop.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.with(|v| v.fmt(f))
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for Prop<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.with(|v| v.fmt(f))
    }
}

impl<T: IntoAttr + Clone> IntoAttr for Prop<T> {
    fn into_attr(self) -> AttrValue {
        self.get().into_attr()
    }
}

impl<T: std::fmt::Display> crate::IntoChild for Prop<T> {
    fn into_child(self) -> crate::Child {
        crate::Child::Text(self.to_string())
    }
}

// ------------------------------------------------------------- attributes

/// An attribute handed through a component (`"data-t": "save"`, `id`,
/// `aria-*`): a name and a value, constant or following state. A component
/// with `#[props(extends = GlobalAttributes)] attributes: Vec<Attribute>`
/// takes them; `..attributes` on an element sets them there (after the
/// element's own, so they win).
#[derive(Clone)]
pub struct Attribute {
    pub name: &'static str,
    value: AttrSrc,
}

#[derive(Clone)]
enum AttrSrc {
    Const(AttrValue),
    Live(std::rc::Rc<dyn Fn() -> AttrValue>),
}

impl Attribute {
    /// `name` set to `v`.
    pub fn new(name: &'static str, v: impl IntoAttr) -> Self {
        Self { name, value: AttrSrc::Const(v.into_attr()) }
    }

    /// `name` set to `f()`, again whenever what it read changes.
    pub fn derive(name: &'static str, f: Box<dyn Fn() -> AttrValue>) -> Self {
        let (v, live) = reads(&f);
        let value = if live { AttrSrc::Live(f.into()) } else { AttrSrc::Const(v) };
        Self { name, value }
    }

    /// Its value now (untracked).
    pub fn value(&self) -> AttrValue {
        match &self.value {
            AttrSrc::Const(v) => v.clone(),
            AttrSrc::Live(f) => crate::untrack(|| f()),
        }
    }
}

impl std::fmt::Debug for Attribute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Attribute({})", self.name)
    }
}

impl PartialEq for Attribute {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.value() == other.value()
    }
}

/// `..attributes` on element `id`: each set there, a following one as a hole.
#[doc(hidden)]
pub fn spread(id: u32, attrs: Vec<Attribute>) {
    for a in attrs {
        match a.value {
            AttrSrc::Const(v) => set_attr_now(id, a.name, &v),
            AttrSrc::Live(f) => attr(id, a.name, Box::new(move || f())),
        }
    }
}

/// The builder of a component that takes attributes (`#[props(extends =
/// GlobalAttributes)]`): `"data-t": …`, `id: …`, `aria_label: …`,
/// `..attributes` given to it land in its `attributes`.
#[diagnostic::on_unimplemented(
    message = "this component takes no attributes (`\"name\": value`, `id`, `..attributes`)",
    note = "a component takes them with `#[props(extends = GlobalAttributes)] attributes: Vec<Attribute>`"
)]
#[doc(hidden)]
pub trait GlobalAttributes: Sized {
    fn __push(&mut self, a: Attribute);

    fn __attr<X: IntoAttr, F: Fn() -> X + 'static>(mut self, name: &'static str, f: F) -> Self {
        self.__push(Attribute::derive(name, Box::new(move || f().into_attr())));
        self
    }

    fn __spread(mut self, attrs: Vec<Attribute>) -> Self {
        attrs.into_iter().for_each(|a| self.__push(a));
        self
    }
}

macro_rules! globals {
    ($($m:ident $n:literal),* $(,)?) => {
        /// The global attributes by name (`id: "x"`), for a component's builder.
        #[doc(hidden)]
        pub trait GlobalAttributeNames: GlobalAttributes {
            $(fn $m<X: IntoAttr, F: Fn() -> X + 'static>(self, f: F) -> Self { self.__attr($n, f) })*
        }
        impl<B: GlobalAttributes> GlobalAttributeNames for B {}
    };
}

globals!(
    accesskey "accesskey", autocapitalize "autocapitalize", autofocus "autofocus",
    contenteditable "contenteditable", dir "dir", draggable "draggable", enterkeyhint "enterkeyhint",
    hidden "hidden", id "id", inert "inert", inputmode "inputmode", is "is", itemid "itemid",
    itemprop "itemprop", itemref "itemref", itemscope "itemscope", itemtype "itemtype", lang "lang",
    nonce "nonce", part "part", popover "popover", role "role", slot "slot", spellcheck "spellcheck",
    style "style", tabindex "tabindex", title "title", translate "translate",
    aria_activedescendant "aria-activedescendant", aria_atomic "aria-atomic", aria_autocomplete "aria-autocomplete",
    aria_busy "aria-busy", aria_checked "aria-checked", aria_colcount "aria-colcount", aria_colindex "aria-colindex",
    aria_controls "aria-controls", aria_current "aria-current", aria_describedby "aria-describedby",
    aria_details "aria-details", aria_disabled "aria-disabled", aria_errormessage "aria-errormessage",
    aria_expanded "aria-expanded", aria_haspopup "aria-haspopup", aria_hidden "aria-hidden",
    aria_invalid "aria-invalid", aria_keyshortcuts "aria-keyshortcuts", aria_label "aria-label",
    aria_labelledby "aria-labelledby", aria_level "aria-level", aria_live "aria-live", aria_modal "aria-modal",
    aria_multiselectable "aria-multiselectable", aria_orientation "aria-orientation", aria_owns "aria-owns",
    aria_placeholder "aria-placeholder", aria_posinset "aria-posinset", aria_pressed "aria-pressed",
    aria_readonly "aria-readonly", aria_relevant "aria-relevant", aria_required "aria-required",
    aria_roledescription "aria-roledescription", aria_rowcount "aria-rowcount", aria_rowindex "aria-rowindex",
    aria_selected "aria-selected", aria_setsize "aria-setsize", aria_sort "aria-sort",
    aria_valuemax "aria-valuemax", aria_valuemin "aria-valuemin", aria_valuenow "aria-valuenow",
    aria_valuetext "aria-valuetext",
);

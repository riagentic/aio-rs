//! Templates and their holes, as DOM edits for the page.
//!
//! `rsx!` makes one static [`Template`] per element tree (its markup,
//! pre-encoded) and one call per dynamic hole: a text, an attribute, an
//! event, a child, a component, a list, a branch. A [`View`] is built when
//! it is placed (lazily: under the owner and the context of where it goes,
//! so a boundary catches what its children throw): [`tpl`] clones the
//! template in the page and names its holes' nodes, and each hole is an
//! effect that writes its node when what it read changes. No view
//! re-runs, nothing is diffed but a hole's last value and a list's keys.
//!
//! A built view is a [`Range`] of sibling nodes, first to last: what a hole
//! puts at a template's root lies inside it, so a view moves and goes
//! whole. Edits go out as one binary buffer per flush ([`take_edits`];
//! format in `docs/design/air.md`); the page's interpreter (`page.js`)
//! applies them and sends events back ([`Root::event`]).
use crate::event::{ASKED, EVAL, EVENT, Event, Flow, FromRaw, R, Raw};
use crate::reactive::{
    Callback, Frame, Owner, Signal, batch, body, child_scope, cleanup, current_owner, current_root, detached, in_root,
    memo_now, provide_on, render, render_comp, scope, set_root, try_context, untrack, with_owner,
};
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::rc::{Rc, Weak};

/// One element tree of an `rsx!`: its nodes in pre-order, encoded for the
/// page (`DEFINE`'s body). Marked nodes (roots, nodes with holes) get ids
/// `base..base + marks` in that order; the view is marks `first..=last`.
#[doc(hidden)]
pub struct Template {
    pub nodes: &'static [u8],
    pub count: u32,
    pub roots: u32,
    pub marks: u32,
    pub first: u32,
    pub last: u32,
    /// Where it is written (a hot reload's key; empty in a release build).
    pub at: crate::hot::At,
}

pub(crate) mod op {
    pub const DEFINE: u8 = 1;
    pub const CLONE: u8 = 2;
    pub const SET_TEXT: u8 = 3;
    pub const SET_ATTR: u8 = 4;
    pub const REMOVE_ATTR: u8 = 5;
    pub const SET_PROP: u8 = 6;
    pub const SET_BOOL: u8 = 7;
    pub const LISTEN: u8 = 8;
    pub const INSERT: u8 = 9;
    pub const APPEND: u8 = 10;
    pub const REMOVE: u8 = 11;
    pub const TEXT: u8 = 12;
    pub const PARK: u8 = 13;
    pub const EVAL: u8 = 14;
    pub const EVAL_SEND: u8 = 15;
    pub const STYLE: u8 = 16;
}

/// An event hole's handler: the page's event, and its flow (stop, prevent).
pub(crate) type Handler = Rc<RefCell<dyn FnMut(&Raw, &Rc<Flow>)>>;

struct Listening {
    name: &'static str,
    run: Handler,
    /// The element's view: the handler goes with it.
    owner: Option<Owner>,
    /// The component that drew it: what the handler makes and spawns is
    /// its (a scope of the component), so a branch drawn again cancels none of it.
    comp: Weak<Frame>,
}

/// What a handler of the element drawn now runs under: the component
/// that drew it, or the element's own owner outside any.
fn handler_owner(comp: &Weak<Frame>, owner: Owner) -> Owner {
    comp.upgrade().map_or(owner, |f| f.owner())
}

struct EvalState {
    inbox: VecDeque<String>,
    done: Option<Result<String, String>>,
    waker: Option<std::task::Waker>,
}

#[derive(Default)]
struct Dom {
    out: Vec<u8>,
    next: u32,
    /// Template address and its markup's edit (0: as built) → its id in the page.
    defined: HashMap<(usize, u64), u32>,
    handlers: HashMap<u32, Vec<Listening>>,
    evals: HashMap<u32, EvalState>,
    next_eval: u32,
    /// The mounted root's owner and the errors no boundary caught.
    root: Option<Owner>,
    uncaught: Vec<String>,
    /// A server root's page ([`Root::serving`]): no edits go out.
    server: Option<Box<dyn Serve>>,
}

/// A server root's page ([`crate::ssr::Page`]) behind a vtable: only a
/// build that renders on a server ([`Root::serving`]) links its code.
pub(crate) trait Serve {
    fn page(&mut self) -> &mut crate::ssr::Page;
    /// Template `t` cloned at `base` (`hot`: its markup sent live).
    fn clone_of(&mut self, t: &'static Template, hot: Option<(&[u8], u64)>, base: u32);
    /// A `Suspense` at `first..=at` (kids before anchor `at`), its fallback showing while `showing`.
    fn bound(&mut self, first: u32, at: u32, kids: Option<u32>, showing: Rc<Cell<bool>>);
    fn set_text(&mut self, id: u32, s: &str);
    /// A text node of its own, not in the page yet.
    fn text_node(&mut self, id: u32, s: &str);
    /// An attribute edit (`SET_ATTR`, `REMOVE_ATTR`, `SET_PROP`, `SET_BOOL`: `v` "true"/"false").
    fn edit(&mut self, id: u32, code: u8, name: &str, v: &str);
    /// The view `first..` put before node `at`.
    fn insert(&mut self, at: u32, first: u32);
    fn append(&mut self, first: u32);
    /// The view `first..` out of the page (removed or parked).
    fn unplace(&mut self, first: u32);
    /// CSS `property` of the first element of the view `first..` set to `v` (empty: removed).
    fn style(&mut self, first: u32, property: &str, v: &str);
}

thread_local! {
    /// Each root's page (0: code run outside any root).
    static DOMS: RefCell<HashMap<u32, Dom>> = RefCell::default();
    static NEXT_ROOT: Cell<u32> = const { Cell::new(1) };
}

impl Dom {
    fn op(&mut self, code: u8) -> &mut Self {
        self.out.push(code);
        self
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.out.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn str(&mut self, s: &str) -> &mut Self {
        self.u32(s.len() as u32);
        self.out.extend_from_slice(s.as_bytes());
        self
    }
    fn alloc(&mut self, n: u32) -> u32 {
        let base = self.next;
        self.next += n;
        base
    }
    fn insert(&mut self, at: u32, v: Range) {
        match (v.0, &mut self.server) {
            (Some((a, _)), Some(s)) => s.insert(at, a),
            (Some((a, b)), None) => _ = self.op(op::INSERT).u32(at).u32(a).u32(b),
            (None, _) => {}
        }
    }
    fn remove(&mut self, v: Range) {
        match (v.0, &mut self.server) {
            (Some((a, _)), Some(s)) => s.unplace(a),
            (Some((a, b)), None) => _ = self.op(op::REMOVE).u32(a).u32(b),
            (None, _) => {}
        }
    }
    fn park(&mut self, v: Range) {
        match (v.0, &mut self.server) {
            (Some((a, _)), Some(s)) => s.unplace(a),
            (Some((a, b)), None) => _ = self.op(op::PARK).u32(a).u32(b),
            (None, _) => {}
        }
    }
    fn set_text(&mut self, id: u32, s: &str) {
        match &mut self.server {
            Some(p) => p.set_text(id, s),
            None => _ = self.op(op::SET_TEXT).u32(id).str(s),
        }
    }
    /// A text node of its own, id `id`.
    fn text_node(&mut self, id: u32, s: &str) {
        match &mut self.server {
            Some(p) => p.text_node(id, s),
            None => _ = self.op(op::TEXT).u32(id).str(s),
        }
    }
    /// An attribute edit (`SET_BOOL`'s `v`: "true" or "false").
    fn edit(&mut self, code: u8, id: u32, name: &str, v: &str) {
        if let Some(p) = &mut self.server {
            return p.edit(id, code, name, v);
        }
        self.op(code).u32(id).str(name);
        match code {
            op::REMOVE_ATTR => {}
            op::SET_BOOL => self.out.push((v == "true") as u8),
            _ => _ = self.str(v),
        }
    }
}

/// Is the current root a server's ([`Root::serving`])? Its holes that read
/// no signal run once.
fn serving() -> bool {
    dom(|d| d.server.is_some())
}

/// A seeded resource's load under way in a server root: its stream waits
/// for it ([`crate::ssr::Streaming`]).
pub(crate) struct Loading(u32);

impl Loading {
    pub(crate) fn new() -> Option<Self> {
        let root = current_root();
        dom(|d| d.server.as_mut().map(|s| s.page().loading += 1)).map(|()| Self(root))
    }
}

impl Drop for Loading {
    fn drop(&mut self) {
        _ = DOMS.try_with(|ds| {
            if let Ok(mut ds) = ds.try_borrow_mut()
                && let Some(s) = ds.get_mut(&self.0).and_then(|d| d.server.as_mut())
            {
                s.page().loading -= 1;
            }
        });
    }
}

/// May a resource's value land now? Not a plain one's in a server root
/// whose page is out: the client loads it (its first frame waits too).
pub(crate) fn lands(seeded: bool) -> bool {
    seeded || dom(|d| d.server.as_mut().is_none_or(|s| !s.page().painted))
}

/// A seeded resource's value loaded in a server root, for its page.
pub(crate) fn loaded(key: &Rc<str>, json: String) {
    dom(|d| {
        if let Some(s) = &mut d.server {
            s.page().data.push((key.clone(), json));
        }
    });
}

/// The current root's page.
fn dom<R>(f: impl FnOnce(&mut Dom) -> R) -> R {
    let at = current_root();
    DOMS.with_borrow_mut(|ds| f(ds.entry(at).or_default()))
}

/// The edits made since the last call for the current root's page (the
/// last made, outside one; [`Root::take_edits`] with more than one).
pub fn take_edits() -> Vec<u8> {
    crate::ssr::end_seeds();
    dom(|d| std::mem::take(&mut d.out))
}

/// Built nodes in the page: a range of siblings, first to last (none: empty).
#[doc(hidden)]
#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub struct Range(Option<(u32, u32)>);

/// A view not built yet: built where it is placed, and again each time it
/// is placed (a dialog opened again, a tab shown again, a view placed in
/// two places: each place gets its own build and state). Cloning shares it.
#[derive(Clone, Default)]
pub struct Tree(Option<Rc<dyn Hole<Range>>>);

impl Tree {
    #[doc(hidden)]
    pub fn new(f: impl Fn() -> Range + 'static) -> Self {
        Self(Some(Rc::new(f)))
    }

    /// Nothing (`rsx! {}`).
    pub fn empty() -> Self {
        Self(None)
    }

    pub(crate) fn build(&self) -> Range {
        self.0.as_ref().map_or_else(Range::default, |f| f.run())
    }

    /// Is it nothing (`rsx! {}`, no children given)?
    pub fn is_empty(&self) -> bool {
        self.0.is_none()
    }
}

impl PartialEq for Tree {
    /// The same view (not the same markup).
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Some(a), Some(b)) => Rc::ptr_eq(a, b),
            (a, b) => a.is_none() && b.is_none(),
        }
    }
}

impl std::fmt::Debug for Tree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.is_empty() { "Tree(empty)" } else { "Tree" })
    }
}

/// What a view returns: a tree made by `rsx!`, built where it is placed.
/// A `Result`: a body may `?` an error to the nearest [`ErrorBoundary`]
/// and a [`Resource::suspend`](crate::Resource::suspend) to the nearest [`Suspense`].
pub type View = Result<Tree, ViewError>;

/// Is `v` nothing (`rsx! {}`, no children given, or an error)?
pub fn is_empty(v: &View) -> bool {
    v.as_ref().is_ok_and(Tree::is_empty) || v.is_err()
}

/// Why a view stopped: an error thrown to an [`ErrorBoundary`], or a
/// resource still loading.
#[derive(Clone)]
pub struct ViewError(Why);

#[derive(Clone)]
enum Why {
    Error(Rc<str>),
    Suspended(Signal<bool>),
}

impl ViewError {
    /// An error with this message.
    pub fn new(msg: impl Into<String>) -> Self {
        Self(Why::Error(msg.into().into()))
    }

    pub(crate) fn suspended(pending: Signal<bool>) -> Self {
        Self(Why::Suspended(pending))
    }

    /// Is it a resource still loading (not an error)?
    pub fn is_suspended(&self) -> bool {
        matches!(self.0, Why::Suspended(_))
    }
}

impl PartialEq for ViewError {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Why::Error(a), Why::Error(b)) => a == b,
            (Why::Suspended(a), Why::Suspended(b)) => a == b,
            _ => false,
        }
    }
}

impl<E: std::error::Error> From<E> for ViewError {
    fn from(e: E) -> Self {
        Self::new(e.to_string())
    }
}

impl std::fmt::Display for ViewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Why::Error(e) => f.write_str(e),
            Why::Suspended(_) => f.write_str("still loading"),
        }
    }
}

impl std::fmt::Debug for ViewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ViewError({self})")
    }
}

/// Where a thrown error goes: the nearest boundary's (or the root's).
#[derive(Clone)]
struct Catch(Rc<dyn Fn(String)>);

/// The nearest `Suspense`: how many views below wait.
#[derive(Clone)]
struct Waiting {
    count: Rc<Cell<usize>>,
    waiting: Signal<bool>,
}

/// An error or a wait a view ended with, told to the boundary above
/// (a wait also subscribes the running hole, so it is built again then).
fn raise(e: ViewError) {
    match e.0 {
        Why::Error(msg) => match try_context::<Catch>() {
            Some(c) => (c.0)(msg.to_string()),
            None => panic!("air: a view failed outside any app root: {msg}"),
        },
        Why::Suspended(pending) => {
            pending.track();
            if let Some(w) = try_context::<Waiting>() {
                w.count.set(w.count.get() + 1);
                w.waiting.set_quiet(true);
                cleanup(move || {
                    w.count.set(w.count.get() - 1);
                    if w.count.get() == 0 {
                        w.waiting.set_quiet(false);
                    }
                });
            }
        }
    }
}

/// Build `v` here (under the current owner), or raise what it ended with.
fn place(v: View) -> Range {
    match v {
        Ok(t) => untrack(|| t.build()),
        Err(e) => {
            raise(e);
            Range::default()
        }
    }
}

/// Clone `t` in the page: the base of its marked nodes' ids. The template
/// itself is sent once per page.
#[doc(hidden)]
pub fn tpl(t: &'static Template) -> u32 {
    // Markup `ax dev` sent for it (a hot reload): defined anew in the page.
    let hot = crate::hot::markup_of(t);
    dom(|d| {
        if let Some(s) = &mut d.server {
            let base = d.next;
            d.next += t.marks;
            s.clone_of(t, hot.as_ref().map(|o| (&o.nodes[..], o.generation)), base);
            return base;
        }
        let at = (t as *const Template as usize, hot.as_ref().map_or(0, |o| o.generation));
        let id = match d.defined.get(&at) {
            Some(id) => *id,
            None => {
                let id = d.defined.len() as u32;
                d.defined.insert(at, id);
                let (nodes, count) = hot.as_ref().map_or((t.nodes, t.count), |o| (&o.nodes[..], o.count));
                d.op(op::DEFINE).u32(id).u32(t.roots).u32(count);
                d.out.extend_from_slice(nodes);
                id
            }
        };
        let base = d.alloc(t.marks);
        d.op(op::CLONE).u32(id).u32(base);
        base
    })
}

/// The built range of a cloned template: its roots, first to last.
#[doc(hidden)]
pub fn view(t: &'static Template, base: u32) -> Range {
    Range(Some((base + t.first, base + t.last)))
}

/// Two empty comments: a range whose content goes before its second (an
/// anchor), for views made of one hole.
static SLOT: Template = Template {
    nodes: &[4, 4],
    count: 2,
    roots: 2,
    marks: 2,
    first: 0,
    last: 1,
    at: crate::hot::At { file: "", line: 0, col: 0, idx: 0 },
};

pub(crate) fn slot() -> (Range, u32) {
    let b = tpl(&SLOT);
    (view(&SLOT, b), b + 1)
}

// The holes take boxed closures, never generics: a hole costs its own
// closure and nothing else (generic holes copied these bodies into every
// call site: 4x the LLVM IR, measured, `docs/design/air.md`).

/// A hole's closure: `Fn() -> R` behind one vtable entry (a `dyn Fn`'s
/// vtable also holds `FnMut`'s and `FnOnce`'s shims, made per closure).
#[doc(hidden)]
pub trait Hole<R> {
    fn run(&self) -> R;
}

impl<R, F: Fn() -> R> Hole<R> for F {
    fn run(&self) -> R {
        self()
    }
}

/// A text hole: node `id` shows `f()`, re-written when it changes.
#[doc(hidden)]
pub fn text(id: u32, f: Box<dyn Hole<String>>) {
    if serving() {
        let (s, read) = crate::reactive::probe(|| f.run());
        if !read {
            return dom(|d| d.set_text(id, &s));
        }
    }
    let mut last: Option<String> = None;
    render(move || {
        let s = f.run();
        if fault::unbound() && last.is_some() {
            return;
        }
        if last.as_ref() != Some(&s) {
            dom(|d| d.set_text(id, &s));
            last = Some(s);
        }
    });
}

/// A text hole of one name shown as it is (`"{x}"`): its value's own
/// `Display`, followed as [`text`] follows it. Generic over the type
/// alone, so every such hole of a type shares its code (a closure each
/// was most of an `rsx!`-heavy crate's compile).
#[doc(hidden)]
pub fn text_of<T: std::fmt::Display + Clone + 'static>(id: u32, v: &T) {
    let v = v.clone();
    text(id, Box::new(move || v.to_string()));
}

/// What an attribute hole holds: a value, presence (`false`: none, but
/// `"false"` on `aria-*`/`data-*`), or none at all (an `Option`'s `None`).
/// `'a`: a value may borrow what the hole read (`title: t(&x)`).
#[derive(Clone, Debug, PartialEq)]
pub enum AttrValue<'a> {
    Str(Cow<'a, str>),
    Bool(bool),
    None,
}

/// What an attribute takes: a string (`&str`, `String`, `Cow`), a number,
/// a `bool` (present or not; `"true"`/`"false"` on `aria-*` and `data-*`),
/// an `Option` (none: not there), a `Signal` (followed).
pub trait IntoAttr<'a> {
    fn into_attr(self) -> AttrValue<'a>;
}

impl<'a> IntoAttr<'a> for &'a str {
    fn into_attr(self) -> AttrValue<'a> {
        AttrValue::Str(Cow::Borrowed(self))
    }
}
impl<'a> IntoAttr<'a> for String {
    fn into_attr(self) -> AttrValue<'a> {
        AttrValue::Str(Cow::Owned(self))
    }
}
impl<'a> IntoAttr<'a> for &'a String {
    fn into_attr(self) -> AttrValue<'a> {
        AttrValue::Str(Cow::Borrowed(self))
    }
}
impl<'a> IntoAttr<'a> for Cow<'a, str> {
    fn into_attr(self) -> AttrValue<'a> {
        AttrValue::Str(self)
    }
}
impl<'a> IntoAttr<'a> for Rc<str> {
    fn into_attr(self) -> AttrValue<'a> {
        AttrValue::Str(Cow::Owned(self.to_string()))
    }
}
impl<'a> IntoAttr<'a> for bool {
    fn into_attr(self) -> AttrValue<'a> {
        AttrValue::Bool(self)
    }
}
impl<'a, T: IntoAttr<'a>> IntoAttr<'a> for Option<T> {
    fn into_attr(self) -> AttrValue<'a> {
        self.map_or(AttrValue::None, T::into_attr)
    }
}
impl<'a, T: IntoAttr<'static> + Clone + 'static> IntoAttr<'a> for Signal<T> {
    /// Tracked: the hole follows the signal.
    fn into_attr(self) -> AttrValue<'a> {
        self.get().into_attr()
    }
}
impl<'a, T: IntoAttr<'static> + Clone + 'static> IntoAttr<'a> for crate::Memo<T> {
    fn into_attr(self) -> AttrValue<'a> {
        self.get().into_attr()
    }
}
macro_rules! num_attr {
    ($($t:ty),*) => {$(
        impl<'a> IntoAttr<'a> for $t {
            fn into_attr(self) -> AttrValue<'a> {
                AttrValue::Str(Cow::Owned(self.to_string()))
            }
        }
    )*};
}
num_attr!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize, f32, f64, char);

/// The attributes the page sets as properties (what a control shows now).
fn is_prop(name: &str) -> bool {
    matches!(name, "value" | "innerHTML") || is_flag_prop(name)
}

/// The properties that are on or off: `false` sets them off (a value's
/// `false`, an `Option`'s `None`, removes it).
fn is_flag_prop(name: &str) -> bool {
    matches!(name, "checked" | "selected" | "indeterminate")
}

/// The attributes whose `true`/`false` is text, not presence (`aria-*`,
/// `data-*` and HTML's enumerated ones): `aria-selected: false` writes
/// `"false"`, where `disabled: false` removes `disabled`.
pub(crate) fn is_text_bool(name: &str) -> bool {
    name.starts_with("aria-")
        || name.starts_with("data-")
        || matches!(name, "draggable" | "contenteditable" | "spellcheck")
}

/// An attribute hole's closure: it hands its value to the slot, so the
/// value may borrow what the closure read.
#[doc(hidden)]
pub trait AttrHole {
    fn run(&self, to: &mut AttrSlot);
}

impl<F: Fn(&mut AttrSlot)> AttrHole for F {
    fn run(&self, to: &mut AttrSlot) {
        self(to)
    }
}

/// An attribute hole's node, name and last value.
#[doc(hidden)]
pub struct AttrSlot {
    id: u32,
    name: &'static str,
    last: Option<AttrValue<'static>>,
}

impl AttrSlot {
    /// The attribute is `v` now: written if it changed, kept (owned) to
    /// compare with the next.
    pub fn set(&mut self, v: AttrValue<'_>) {
        let name = self.name;
        let same = match (&self.last, &v) {
            (Some(AttrValue::Str(a)), AttrValue::Str(b)) => a == b,
            (Some(AttrValue::Bool(a)), AttrValue::Bool(b)) => a == b,
            (Some(AttrValue::None), AttrValue::None) => true,
            (None, AttrValue::None) => !is_flag_prop(name),
            (None, AttrValue::Bool(false)) => !is_flag_prop(name) && !is_text_bool(name),
            _ => false,
        };
        if same {
            return;
        }
        set_attr_now(self.id, name, &v);
        self.last = Some(match (self.last.take(), v) {
            // A borrowed string goes into the last one's buffer.
            (Some(AttrValue::Str(Cow::Owned(mut s))), AttrValue::Str(Cow::Borrowed(b))) => {
                s.clear();
                s.push_str(b);
                AttrValue::Str(Cow::Owned(s))
            }
            (_, AttrValue::Str(s)) => AttrValue::Str(Cow::Owned(s.into_owned())),
            (_, AttrValue::Bool(b)) => AttrValue::Bool(b),
            (_, AttrValue::None) => AttrValue::None,
        });
    }
}

/// An attribute hole of one name shown as it is (`class: "{x}"`), as
/// [`text_of`]: no closure of its own per hole.
#[doc(hidden)]
pub fn attr_of<T: std::fmt::Display + Clone + 'static>(id: u32, name: &'static str, v: &T) {
    let v = v.clone();
    attr(id, name, Box::new(move |s: &mut AttrSlot| s.set(IntoAttr::into_attr(v.to_string()))));
}

/// An attribute hole: `name` of node `id` is what `f` hands its slot.
#[doc(hidden)]
pub fn attr(id: u32, name: &'static str, f: Box<dyn AttrHole>) {
    let mut slot = AttrSlot { id, name, last: None };
    if serving() && !crate::reactive::probe(|| f.run(&mut slot)).1 {
        return;
    }
    render(move || f.run(&mut slot));
}

/// `name` of node `id` set to `v` now (no hole).
pub(crate) fn set_attr_now(id: u32, name: &str, v: &AttrValue<'_>) {
    let bool = |b: bool| if b { "true" } else { "false" };
    let (code, v) = match (v, is_prop(name)) {
        (AttrValue::Bool(b), _) if is_text_bool(name) => (op::SET_ATTR, bool(*b)),
        (AttrValue::Str(s), false) => (op::SET_ATTR, &**s),
        (AttrValue::Str(s), true) => (op::SET_PROP, &**s),
        (AttrValue::Bool(b), true) if is_flag_prop(name) => (op::SET_BOOL, bool(*b)),
        (AttrValue::None, true) if is_flag_prop(name) => (op::SET_BOOL, "false"),
        (AttrValue::Bool(true), true) => (op::SET_PROP, ""),
        (AttrValue::Bool(true), false) => (op::SET_ATTR, ""),
        (AttrValue::Bool(false) | AttrValue::None, _) => (op::REMOVE_ATTR, ""),
    };
    dom(|d| d.edit(code, id, name, v));
}

/// What an event attribute takes: a closure of the event (that may be
/// `async`), or a handler prop.
pub trait IntoHandler<T, M = ()> {
    fn into_handler(self) -> Box<dyn FnMut(Event<T>)>;
}

impl<T, M, R: HandlerReturn<M>, F: FnMut(Event<T>) -> R + 'static> IntoHandler<T, (M, R)> for F {
    fn into_handler(mut self) -> Box<dyn FnMut(Event<T>)> {
        Box::new(move |e| self(e).run())
    }
}

impl<T: 'static> IntoHandler<T, Plain> for Callback<Event<T>> {
    fn into_handler(self) -> Box<dyn FnMut(Event<T>)> {
        Box::new(move |e| self.call(e))
    }
}

impl<T: 'static> IntoHandler<T, Plain> for Option<Callback<Event<T>>> {
    fn into_handler(self) -> Box<dyn FnMut(Event<T>)> {
        Box::new(move |e| {
            if let Some(c) = self {
                c.call(e)
            }
        })
    }
}

/// A handler for [`on`], typed by its event's payload (the macro names it).
#[doc(hidden)]
pub fn handler<T: FromRaw, M>(f: impl IntoHandler<T, M>) -> Handler {
    boxed(f.into_handler())
}

// A handler costs its closure and nothing else: what wraps it is generic
// over the payload alone, so every handler of one event shares it (a
// wrapper generic over the closure made 36% of a view crate's LLVM IR).
fn boxed<T: FromRaw>(mut f: Box<dyn FnMut(Event<T>)>) -> Handler {
    Rc::new(RefCell::new(move |r: &Raw, flow: &Rc<Flow>| f(Event::with_flow(T::from_raw(r), flow.clone()))))
}

/// What a handler closure returns: nothing, or work to run (`async move
/// { … }`: spawned under the component that drew the element, so it ends
/// with that component, not with a branch or a row of it drawn again).
#[doc(hidden)]
pub trait HandlerReturn<M> {
    fn run(self);
}

#[doc(hidden)]
pub struct Plain;
#[doc(hidden)]
pub struct Async;

impl HandlerReturn<Plain> for () {
    fn run(self) {}
}

impl<F: std::future::Future<Output = ()> + 'static> HandlerReturn<Async> for F {
    fn run(self) {
        _ = crate::task::spawn(self);
    }
}

/// [`handler`] of a closure (`rsx!` boxes it, and this type gives its
/// parameter's); it may be `async`.
#[doc(hidden)]
pub fn handler_fn<T: FromRaw, M, R: HandlerReturn<M> + 'static>(mut f: Box<dyn FnMut(Event<T>) -> R>) -> Handler {
    boxed::<T>(Box::new(move |e| f(e).run()))
}

/// An `onmounted` closure, typed; it may be `async`.
#[doc(hidden)]
pub fn mounted_fn<M, R: HandlerReturn<M>>(
    mut f: impl FnMut(Event<MountedData>) -> R + 'static,
) -> impl FnMut(Event<MountedData>) + 'static {
    move |e| f(e).run()
}

/// An `onmounted` handler: it runs once the element is drawn.
#[doc(hidden)]
pub fn mounted<M>(id: u32, f: impl IntoHandler<MountedData, M>) {
    // A server's page is never drawn.
    if serving() {
        return;
    }
    let mut f = f.into_handler();
    let (owner, comp) = (current_owner(), crate::tree::current_frame());
    crate::reactive::later(move || {
        if owner.is_none_or(|o| o.alive()) {
            let mut run = || batch(|| f(Event::new(MountedData { id })));
            match owner {
                Some(o) => with_owner(handler_owner(&comp, o), run),
                None => run(),
            }
        }
    });
}

/// An event hole: `name` on node `id` runs `h` (in a batch: its writes are drawn once).
#[doc(hidden)]
pub fn on(id: u32, name: &'static str, h: Handler) {
    // A server's page sends no event.
    if serving() {
        return;
    }
    let (owner, comp) = (current_owner(), crate::tree::current_frame());
    dom(|d| {
        d.handlers.entry(id).or_default().push(Listening { name, run: h, owner, comp });
        d.op(op::LISTEN).u32(id).str(name);
    });
    cleanup(move || {
        dom(|d| {
            if let Some(hs) = d.handlers.get_mut(&id) {
                hs.retain(|l| l.name != name);
                if hs.is_empty() {
                    d.handlers.remove(&id);
                }
            }
        })
    });
}

/// What a child hole shows.
pub enum Child {
    Text(String),
    Views(Vec<View>),
}

/// What a `{child}` takes: a view, views, text, a number, an `Option`, a signal (followed).
pub trait IntoChild {
    fn into_child(self) -> Child;
}

impl IntoChild for View {
    fn into_child(self) -> Child {
        Child::Views(vec![self])
    }
}
impl IntoChild for Tree {
    fn into_child(self) -> Child {
        Child::Views(vec![Ok(self)])
    }
}
impl IntoChild for Vec<View> {
    fn into_child(self) -> Child {
        Child::Views(self)
    }
}
impl<C: IntoChild> IntoChild for Option<C> {
    fn into_child(self) -> Child {
        self.map_or(Child::Views(vec![]), C::into_child)
    }
}
impl IntoChild for String {
    fn into_child(self) -> Child {
        Child::Text(self)
    }
}
impl IntoChild for &str {
    fn into_child(self) -> Child {
        Child::Text(self.to_owned())
    }
}
impl IntoChild for &String {
    fn into_child(self) -> Child {
        Child::Text(self.clone())
    }
}
impl IntoChild for Rc<str> {
    fn into_child(self) -> Child {
        Child::Text(self.to_string())
    }
}
impl<T: std::fmt::Display + 'static> IntoChild for Signal<T> {
    /// Tracked: the hole follows the signal.
    fn into_child(self) -> Child {
        Child::Text(self.to_string())
    }
}
impl<T: std::fmt::Display + 'static> IntoChild for crate::Memo<T> {
    fn into_child(self) -> Child {
        Child::Text(self.to_string())
    }
}
macro_rules! num_child {
    ($($t:ty),*) => {$(
        impl IntoChild for $t {
            fn into_child(self) -> Child {
                Child::Text(self.to_string())
            }
        }
    )*};
}
num_child!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize, f32, f64, char);

enum Shown {
    Text(u32, String),
    Views(Vec<Range>),
}

/// A child hole: what `f()` gives goes before anchor `at`; a text is
/// re-written in place, views are replaced (what made them is disposed).
#[doc(hidden)]
pub fn child(at: u32, f: Box<dyn Hole<Child>>) {
    if serving() {
        let (c, read) = crate::reactive::probe(|| f.run());
        if !read {
            show_child(at, c);
            return;
        }
    }
    child_live(at, f)
}

/// [`child`] as a hole that follows what it reads (its first run too).
fn child_live(at: u32, f: Box<dyn Hole<Child>>) {
    let mut shown = Shown::Views(vec![]);
    render(move || {
        let c = f.run();
        if let (Shown::Text(id, last), Child::Text(s)) = (&mut shown, &c) {
            if last != s {
                dom(|d| d.set_text(*id, s));
                *last = s.clone();
            }
            return;
        }
        dom(|d| match &shown {
            Shown::Text(id, _) => d.remove(Range(Some((*id, *id)))),
            Shown::Views(v) => v.iter().for_each(|v| d.remove(*v)),
        });
        shown = show_child(at, c);
    });
}

/// `c` put before anchor `at`.
fn show_child(at: u32, c: Child) -> Shown {
    match c {
        Child::Text(s) => dom(|d| {
            let id = d.alloc(1);
            d.text_node(id, &s);
            d.insert(at, Range(Some((id, id))));
            Shown::Text(id, s)
        }),
        Child::Views(v) => {
            let built: Vec<Range> = v.into_iter().map(place).collect();
            dom(|d| built.iter().for_each(|r| d.insert(at, *r)));
            Shown::Views(built)
        }
    }
}

/// Text as a view: `f()` in one text node, re-written when what it reads
/// changes (a label prop given a string or a format string).
pub fn text_view(f: impl Fn() -> String + 'static) -> View {
    let f: Rc<dyn Fn() -> String> = Rc::new(f);
    Ok(Tree::new(move || {
        let (s, live) = match serving() {
            true => crate::reactive::probe(|| f()),
            false => crate::reactive::reads(|| f()),
        };
        let id = dom(|d| {
            let id = d.alloc(1);
            d.text_node(id, &s);
            id
        });
        if live {
            let f = f.clone();
            text(id, Box::new(move || f()));
        }
        Range(Some((id, id)))
    }))
}

/// `view` with one style property of its first element (`"animation"`)
/// following `f()` (`None`: none): a component that animates or places
/// what it is given without a box of its own (aio's `Transition`).
pub fn styled(view: View, property: &'static str, f: impl Fn() -> Option<String> + 'static) -> View {
    let tree = view?;
    let f = Rc::new(f);
    Ok(Tree::new(move || {
        let r = tree.build();
        if let Some((a, b)) = r.0 {
            let f = f.clone();
            let mut last: Option<Option<String>> = None;
            let mut set = move |v: Option<String>| {
                if last.as_ref() != Some(&v) {
                    dom(|d| match &mut d.server {
                        Some(s) => s.style(a, property, v.as_deref().unwrap_or("")),
                        None => _ = d.op(op::STYLE).u32(a).u32(b).str(property).str(v.as_deref().unwrap_or("")),
                    });
                    last = Some(v);
                }
            };
            if serving() {
                let (v, read) = crate::reactive::probe(|| f());
                set(v);
                if !read {
                    return r;
                }
            }
            render(move || set(f()));
        }
        r
    }))
}

/// A component in a template: `f()` (its body, run once, untracked, with
/// its hooks kept) and its view before anchor `at`. A body that waits for
/// a resource runs again once it is loaded, its hooks' state kept.
/// `name` and `key` place it in the tree of mounted components.
#[doc(hidden)]
pub fn comp(at: u32, name: &'static str, key: Option<String>, f: Box<dyn Hole<View>>) {
    comp_as(at, Some(crate::tree::mount(name, key)), f)
}

fn comp_as(at: u32, id: Option<crate::tree::Id>, f: Box<dyn Hole<View>>) {
    let mut frame: Option<Rc<Frame>> = None;
    let mut shown = Range::default();
    // Its entry ends with its hole.
    let mounted = id.map(crate::tree::Entry);
    render_comp(move || {
        let _in = crate::tree::enter(mounted.as_ref().map(|m| m.0));
        dom(|d| d.remove(std::mem::take(&mut shown)));
        let fr = frame
            .get_or_insert_with(|| {
                let place = crate::hot::keeping().then(|| crate::tree::place(id));
                let fr = place.and_then(|p| crate::hot::adopt(&p)).unwrap_or_else(Frame::new);
                crate::tree::set_frame(id, &fr);
                fr
            })
            .clone();
        let t0 = crate::tree::start();
        let v = body(&fr, || f.run());
        shown = place(v);
        if let Some(m) = &mounted {
            crate::tree::rendered(m.0, shown.0, t0);
        }
        dom(|d| d.insert(at, shown));
    });
}

/// `then()` or `otherwise()` before anchor `at`, swapped only when `cond()` flips.
#[doc(hidden)]
pub fn show(at: u32, cond: Box<dyn Hole<bool>>, then: Box<dyn Hole<View>>, otherwise: Box<dyn Hole<View>>) {
    if serving() {
        let (c, read) = crate::reactive::probe(|| cond.run());
        if !read {
            let v = untrack(|| if c { then.run() } else { otherwise.run() });
            show_child(at, Child::Views(vec![v]));
            return;
        }
    }
    let c = memo_now(move || cond.run());
    child_live(
        at,
        Box::new(move || {
            Child::Views(vec![if c.get() { untrack(|| then.run()) } else { untrack(|| otherwise.run()) }])
        }),
    );
}

/// A branch hole (`if let`, `match` in `rsx!`): `f()` gives the matching
/// arm's key and view. The view before anchor `at` is made again only when
/// the key changes, and on every change for key `u32::MAX` (an arm that
/// binds names).
#[doc(hidden)]
pub fn branch(at: u32, f: Box<dyn Hole<(u32, View)>>) {
    if serving() {
        let ((_, v), read) = crate::reactive::probe(|| f.run());
        if !read {
            show_child(at, Child::Views(vec![v]));
            return;
        }
    }
    let next: Rc<RefCell<Option<View>>> = Rc::default();
    let n = next.clone();
    let mut runs = 0u32;
    let key = memo_now(move || {
        let (k, v) = f.run();
        *n.borrow_mut() = Some(v);
        runs = runs.wrapping_add(1);
        (k, if k == u32::MAX { runs } else { 0 })
    });
    // Its view is taken once made: a hole that runs once.
    child_live(
        at,
        Box::new(move || {
            key.with(|_| ());
            Child::Views(next.borrow_mut().take().into_iter().collect())
        }),
    );
}

/// A row in the page: what made it and where it is.
struct Placed {
    owner: Owner,
    view: Range,
}

/// A keyed list before anchor `at`: `items()` re-read when it changes, a
/// row made per new key, kept (moved if it must) for a key it had, made
/// anew for a key whose item changed, removed for a key gone. AIR's
/// `diffKeyed` (`aio/src/air/vdom-diff-children.ts:321`): rows on the
/// longest increasing run of old places stay, the others move.
#[doc(hidden)]
pub fn each<I, K>(
    at: u32,
    items: impl Fn() -> Vec<I> + 'static,
    key: impl Fn(&I) -> K + 'static,
    row: impl Fn(I) -> View + 'static,
) where
    I: PartialEq + Clone + 'static,
    K: Hash + Eq + Clone + 'static,
{
    // The closures' signatures from here; the keys' work once per item type.
    each_dyn(at, Box::new(items), Box::new(key), Box::new(row))
}

#[allow(clippy::type_complexity)]
fn each_dyn<I, K>(at: u32, items: Box<dyn Fn() -> Vec<I>>, key: Box<dyn Fn(&I) -> K>, row: Box<dyn Fn(I) -> View>)
where
    I: PartialEq + Clone + 'static,
    K: Hash + Eq + Clone + 'static,
{
    let (first, read) = match serving() {
        true => crate::reactive::probe(&items),
        false => (vec![], true),
    };
    if !read {
        // Each row once, in order, under the list's owner (nothing re-runs).
        let mut seen = std::collections::HashSet::with_capacity(first.len());
        for (i, item) in first.into_iter().enumerate() {
            unique(i, seen.insert(key(&item)), std::any::type_name::<K>());
            let r = place(row(item));
            dom(|d| d.insert(at, r));
        }
        return;
    }
    let outer = current_owner();
    let placed = placed_rows();
    // Row `i`'s key and item, as `placed[i]`.
    let mut rows: Vec<(K, I)> = vec![];
    render(move || {
        let next = items();
        untrack(|| {
            let mut by_key: HashMap<K, usize> = rows.iter().enumerate().map(|(i, (k, _))| (k.clone(), i)).collect();
            let mut src = Vec::with_capacity(next.len());
            let mut seen = std::collections::HashSet::with_capacity(next.len());
            let old = std::mem::take(&mut rows);
            for (at, item) in next.into_iter().enumerate() {
                let k = key(&item);
                unique(at, seen.insert(k.clone()), std::any::type_name::<K>());
                let kept = by_key.remove(&k).filter(|i| old[*i].1 == item);
                src.push(kept.map_or(-1, |i| i as isize));
                rows.push((k, item));
            }
            reconcile(at, outer, &mut placed.borrow_mut(), &src, &|i| row(rows[i].1.clone()));
        });
    });
}

/// Row `at`'s key is not one a row before it has (`new`).
fn unique(at: usize, new: bool, key: &str) {
    if !new {
        panic!(
            "air: row {at} of a list has the key of a row before it (a `{key}` key): keyed siblings must each have a \
             unique key (an id, not a place or a name)"
        );
    }
}

/// A list's rows in the page; the list's owner ending ends them (they
/// are its, not a run's).
fn placed_rows() -> Rc<RefCell<Vec<Placed>>> {
    let placed: Rc<RefCell<Vec<Placed>>> = Rc::default();
    let p = placed.clone();
    cleanup(move || p.borrow_mut().drain(..).for_each(|r| r.owner.dispose()));
    placed
}

/// A list's rows in their new order (not generic: one copy for all
/// lists): `src[i]`, row `i`'s old place (`-1`: new, made by `make(i)`);
/// the old rows not kept are removed and ended.
fn reconcile(at: u32, outer: Option<Owner>, placed: &mut Vec<Placed>, src: &[isize], make: &dyn Fn(usize) -> View) {
    let mut old: Vec<Option<Placed>> = std::mem::take(placed).into_iter().map(Some).collect();
    for (i, s) in src.iter().enumerate() {
        let p = match usize::try_from(*s) {
            Ok(s) => old[s].take().expect("a row kept once"),
            Err(_) => {
                // The list's, not this run's: a re-run keeps it, the list's end ends it.
                let mk = || child_scope(|| place(make(i)));
                let (view, owner) = match outer {
                    Some(o) => with_owner(o, mk),
                    None => detached(mk),
                };
                Placed { owner, view }
            }
        };
        placed.push(p);
    }
    dom(|d| old.iter().flatten().for_each(|r| d.remove(r.view)));
    old.into_iter().flatten().for_each(|r| r.owner.dispose_soon());
    let views: Vec<Range> = placed.iter().map(|r| r.view).collect();
    put(at, src, &views);
}

/// Put rows in their new order before `at` (not generic: one copy for all
/// lists). `src[i]`: row `i`'s old place, `-1` if new.
fn put(at: u32, src: &[isize], rows: &[Range]) {
    let stay = longest_increasing(src);
    let lose = fault::lost_row().then(|| src.iter().rposition(|s| *s < 0)).flatten();
    dom(|d| {
        let mut anchor = at;
        for i in (0..rows.len()).rev() {
            if !stay[i] && lose != Some(i) {
                d.insert(anchor, rows[i]);
            }
            if let Some((first, _)) = rows[i].0
                && lose != Some(i)
            {
                anchor = first;
            }
        }
    });
}

/// Which places of `seq` form one longest strictly increasing run,
/// ignoring entries `< 0` (AIR `_longestIncreasing`: patience sorting with
/// predecessor links, O(n log n)).
pub fn longest_increasing(seq: &[isize]) -> Vec<bool> {
    let mut tails: Vec<usize> = vec![];
    let mut prev = vec![usize::MAX; seq.len()];
    for (i, &v) in seq.iter().enumerate() {
        if v < 0 {
            continue;
        }
        let lo = tails.partition_point(|&t| seq[t] < v);
        if lo > 0 {
            prev[i] = tails[lo - 1];
        }
        if lo == tails.len() {
            tails.push(i);
        } else {
            tails[lo] = i;
        }
    }
    let mut out = vec![false; seq.len()];
    let mut k = tails.last().copied().unwrap_or(usize::MAX);
    while k != usize::MAX {
        out[k] = true;
        k = prev[k];
    }
    out
}

// ------------------------------------------------------------- boundaries

/// The props of [`ErrorBoundary`].
#[derive(air::Props)]
pub struct ErrorBoundaryProps {
    pub fallback: Callback<String, View>,
    pub children: View,
}

/// Its children, or `fallback(error)` once one of them throws (`?` in a
/// view's body): what the children made is ended in the same pass. The
/// fallback may [`retry`]: the children are made anew.
#[allow(non_snake_case)]
pub fn ErrorBoundary(p: ErrorBoundaryProps) -> View {
    let error = Signal::new(None::<String>);
    let owner = current_owner().expect("air: a boundary is made in a view");
    // What its own fallback throws goes to the boundary above, not to this
    // one: it would replace the fallback that threw (or loop).
    let (outer, failed) = (try_context::<Catch>(), Rc::new(Cell::new(false)));
    let f = failed.clone();
    provide_on(
        owner,
        Catch(Rc::new(move |e| match (f.get(), &outer) {
            (true, Some(c)) => (c.0)(e),
            (true, None) => panic!("air: a view failed outside any app root: {e}"),
            (false, _) => error.set_quiet(Some(e)),
        })),
    );
    provide_on(owner, Retry(error));
    let ErrorBoundaryProps { fallback, children } = p;
    Ok(Tree::new(move || {
        let (range, at) = slot();
        let (children, failed) = (children.clone(), failed.clone());
        child(
            at,
            Box::new(move || {
                let e = error.get();
                failed.set(e.is_some());
                match e {
                    None => children.clone().into_child(),
                    Some(e) => untrack(|| fallback.call(e)).into_child(),
                }
            }),
        );
        range
    }))
}

/// The nearest [`ErrorBoundary`]'s way back to its children ([`retry`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Retry(Signal<Option<String>>);

impl Retry {
    /// The boundary's children made anew (aio's boundary `reset`); one
    /// that throws again shows the fallback again.
    pub fn retry(self) {
        self.0.set(None);
    }
}

/// The [`Retry`] of the [`ErrorBoundary`] this view is in (call it in
/// the fallback's body, keep it for a handler).
pub fn retry() -> Retry {
    crate::reactive::try_context::<Retry>().expect("air: retry() outside an ErrorBoundary")
}

/// The props of [`Suspense`].
#[derive(air::Props)]
pub struct SuspenseProps {
    pub fallback: View,
    pub children: View,
}

/// Its children, or `fallback` while one of them waits for a resource
/// ([`Resource::suspend`](crate::Resource::suspend)). The children are
/// built at once and kept (parked off the page) while the fallback shows.
#[allow(non_snake_case)]
pub fn Suspense(p: SuspenseProps) -> View {
    let waiting = Signal::new(false);
    let owner = current_owner().expect("air: a boundary is made in a view");
    provide_on(owner, Waiting { count: Rc::default(), waiting });
    let SuspenseProps { fallback, children } = p;
    Ok(Tree::new(move || {
        let (range, at) = slot();
        let kids = place(children.clone());
        dom(|d| d.insert(at, kids));
        let fallback = fallback.clone();
        let fb: Rc<Cell<Option<Range>>> = Rc::default();
        let showing = Rc::new(Cell::new(false));
        // A server's stream sends the children later if they wait now.
        dom(|d| {
            if let (Some(s), Some((first, _))) = (&mut d.server, range.0) {
                s.bound(first, at, kids.0.map(|k| k.0), showing.clone());
            }
        });
        let (f, s) = (fb.clone(), showing.clone());
        // Whichever is parked when it ends leaves the page with it.
        cleanup(move || {
            let parked = if s.get() { Some(kids) } else { f.get() };
            if let Some(p) = parked {
                dom(|d| d.remove(p));
            }
        });
        render(move || {
            let w = waiting.get();
            if w == showing.get() {
                return;
            }
            showing.set(w);
            if w {
                dom(|d| d.park(kids));
                let r = match fb.get() {
                    Some(r) => r,
                    None => {
                        let v = fallback.clone();
                        let r = with_owner(owner, || place(v));
                        fb.set(Some(r));
                        r
                    }
                };
                dom(|d| d.insert(at, r));
            } else {
                dom(|d| {
                    if let Some(r) = fb.get() {
                        d.park(r);
                    }
                    d.insert(at, kids);
                });
            }
        });
        range
    }))
}

/// The props of [`Fragment`].
#[derive(air::Props)]
pub struct FragmentProps {
    pub children: View,
}

/// Its children, as they are.
#[allow(non_snake_case)]
pub fn Fragment(p: FragmentProps) -> View {
    p.children
}

// ------------------------------------------------------------- page calls

/// A script run in the page (`aio.send(v)` hands a JSON value to the view,
/// `await aio.recv()` takes one from it, `return v` ends it, `aio.node(id)`
/// is an element of the views'): focus, measuring, the page's own APIs.
/// Values are JSON text here; `aio::ui::eval` types them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Eval {
    id: u32,
}

/// Run `js` in the page; it ends with the current owner (its replies are dropped).
pub fn eval(js: &str) -> Eval {
    let id = dom(|d| {
        d.next_eval += 1;
        let id = d.next_eval;
        d.evals.insert(id, EvalState { inbox: VecDeque::new(), done: None, waker: None });
        if d.server.is_none() {
            d.op(op::EVAL).u32(id).str(js);
        }
        id
    });
    cleanup(move || dom(|d| _ = d.evals.remove(&id)));
    Eval { id }
}

/// Run `js` in the page and forget it (no reply is kept): what a view's
/// cleanup tells the page as it goes.
pub fn exec(js: &str) {
    dom(|d| {
        if d.server.is_none() {
            d.op(op::EVAL).u32(0).str(js);
        }
    });
}

impl Eval {
    /// Hand the script a JSON value (its `await aio.recv()`).
    pub fn send_json(&self, json: &str) -> Result<(), String> {
        dom(|d| match d.evals.contains_key(&self.id) {
            true if d.server.is_some() => Ok(()),
            true => Ok(_ = d.op(op::EVAL_SEND).u32(self.id).str(json)),
            false => Err("the script's view ended".into()),
        })
    }

    /// The next JSON value the script sent (`aio.send(v)`); an error once it
    /// failed or its view ended. A script that returned may still send (its
    /// listeners, its timers), so its return ends no `recv`.
    pub fn recv_json(&self) -> impl std::future::Future<Output = Result<String, String>> + use<> {
        let id = self.id;
        std::future::poll_fn(move |cx| {
            dom(|d| {
                let Some(e) = d.evals.get_mut(&id) else {
                    return std::task::Poll::Ready(Err("the script's view ended".into()));
                };
                if let Some(v) = e.inbox.pop_front() {
                    return std::task::Poll::Ready(Ok(v));
                }
                if let Some(Err(why)) = &e.done {
                    return std::task::Poll::Ready(Err(why.clone()));
                }
                e.waker = Some(cx.waker().clone());
                std::task::Poll::Pending
            })
        })
    }

    /// What the script returned (JSON), or why it failed.
    pub fn join_json(&self) -> impl std::future::Future<Output = Result<String, String>> + use<> {
        let id = self.id;
        std::future::poll_fn(move |cx| {
            dom(|d| {
                let Some(e) = d.evals.get_mut(&id) else {
                    return std::task::Poll::Ready(Err("the script's view ended".into()));
                };
                if let Some(done) = e.done.clone() {
                    return std::task::Poll::Ready(done);
                }
                e.waker = Some(cx.waker().clone());
                std::task::Poll::Pending
            })
        })
    }
}

/// What the page answered a script: `kind` 0 a value sent, 1 its return, 2 its failure.
fn eval_reply(b: &[u8]) -> bool {
    let mut r = R { b, p: 1 };
    let (Some(id), Some(kind), Some(json)) = (r.u32(), r.u8(), r.str()) else { return false };
    let waker = dom(|d| {
        let e = d.evals.get_mut(&id)?;
        match kind {
            0 => e.inbox.push_back(json),
            1 => e.done = Some(Ok(json)),
            _ => e.done = Some(Err(json)),
        }
        e.waker.take()
    });
    if let Some(w) = waker {
        w.wake()
    }
    true
}

/// An element as `onmounted` hands it out: page calls on it.
#[derive(Clone, Debug, PartialEq)]
pub struct MountedData {
    id: u32,
}

impl MountedData {
    fn call(&self, js: &str) -> impl std::future::Future<Output = Result<String, String>> + use<> {
        let e = eval(&format!("const n = aio.node({}); if (!n) throw new Error('gone'); {js}", self.id));
        async move { e.join_json().await }
    }

    /// Run `js` on the element (`el` in it) and forget it: a canvas's
    /// drawing, a video's play, a selection. One call is one script, so a
    /// frame's drawing is one call (`const c = el.getContext('2d'); c.fillRect(…); …`).
    pub fn run(&self, js: &str) {
        exec(&format!("const el = aio.node({}); if (el) {{ {js} }}", self.id));
    }

    /// A script on the element (`el`) that talks back ([`Eval`]: `aio.send`, `return`).
    pub fn eval(&self, js: &str) -> Eval {
        eval(&format!("const el = aio.node({}); if (!el) throw new Error('gone'); {js}", self.id))
    }

    /// Focus it (or take the focus away).
    pub fn set_focus(&self, on: bool) -> impl std::future::Future<Output = Result<(), String>> + use<> {
        let f = self.call(if on { "n.focus(); return true;" } else { "n.blur(); return true;" });
        async move { f.await.map(drop) }
    }

    /// Scroll it into view, nearest edge (`smooth`: animated).
    pub fn scroll_to(&self, smooth: bool) -> impl std::future::Future<Output = Result<(), String>> + use<> {
        let b = if smooth { "smooth" } else { "auto" };
        let f = self.call(&format!("n.scrollIntoView({{ block: 'nearest', behavior: '{b}' }}); return true;"));
        async move { f.await.map(drop) }
    }

    /// Scroll it to `to` (its scrollLeft and scrollTop).
    pub fn scroll(
        &self,
        to: crate::geometry::PixelsVector2D,
        behavior: crate::ScrollBehavior,
    ) -> impl std::future::Future<Output = Result<(), String>> + use<> {
        let b = if behavior == crate::ScrollBehavior::Smooth { "smooth" } else { "instant" };
        let f = self.call(&format!("n.scrollTo({{ left: {}, top: {}, behavior: '{b}' }}); return true;", to.x, to.y));
        async move { f.await.map(drop) }
    }

    /// Its box in the window.
    pub fn get_client_rect(&self) -> impl std::future::Future<Output = Result<crate::geometry::Rect, String>> + use<> {
        let f = self.call("const r = n.getBoundingClientRect(); return [r.x, r.y, r.width, r.height].join(',');");
        async move {
            let s = f.await?;
            let v: Vec<f64> = s.trim_matches('"').split(',').filter_map(|x| x.parse().ok()).collect();
            match v[..] {
                [x, y, width, height] => Ok(crate::geometry::Rect::new(
                    crate::geometry::PixelsPoint::new(x, y),
                    crate::geometry::PixelsSize::new(width, height),
                )),
                _ => Err(format!("a rect the page did not give: {s}")),
            }
        }
    }
}

// ------------------------------------------------------------------- root

/// A window's views: mounted into the page's root (node 0). One per thread.
pub struct Root {
    id: u32,
    owner: Owner,
    range: Rc<Cell<Range>>,
    /// What it shows, and the owner of its views (built again by [`Root::hot`]).
    app: RefCell<Option<Rc<dyn Fn() -> View>>>,
    shown: Rc<Cell<Option<Owner>>>,
    /// The places of what runs beside the app ([`Root::beside`]): not the
    /// app's, so [`Root::hot`] leaves them be.
    besides: RefCell<Vec<Box<str>>>,
}

impl Root {
    /// A root with nothing in it yet ([`show`](Self::show)): a page (a
    /// window) of its own, its globals and context fresh. Roots on one
    /// thread are apart: each one's views, edits, events and globals are
    /// its own. The new root is the current one outside any root.
    pub fn new() -> Self {
        let id = NEXT_ROOT.replace(NEXT_ROOT.get() + 1);
        // Pages of roots dropped and taken are forgotten.
        DOMS.with_borrow_mut(|ds| ds.retain(|&k, d| k == 0 || d.root.is_some()));
        set_root(id);
        let owner = in_root(id, || {
            dom(|d| *d = Dom { next: 1, ..Dom::default() });
            let ((), owner) = detached(|| scope(|| ()));
            let caught = Catch(Rc::new(|e: String| {
                eprintln!("air: a view failed and no ErrorBoundary is above it: {e}");
                dom(|d| d.uncaught.push(e));
            }));
            provide_on(owner, caught);
            dom(|d| d.root = Some(owner));
            owner
        });
        Self {
            id,
            owner,
            range: Rc::default(),
            app: RefCell::default(),
            shown: Rc::default(),
            besides: RefCell::default(),
        }
    }

    /// A root for a server's render ([`crate::ssr`]): it sends no edits,
    /// its page is kept to be written as markup ([`Self::markup`]).
    pub(crate) fn serving() -> Self {
        let r = Self::new();
        in_root(r.id, || dom(|d| d.server = Some(Box::<crate::ssr::Page>::default())));
        r
    }

    /// `f` of a server root's page (`None`: not a server's).
    pub(crate) fn page<R>(&self, f: impl FnOnce(&mut crate::ssr::Page) -> R) -> Option<R> {
        in_root(self.id, || dom(|d| d.server.as_mut().map(|s| f(s.page()))))
    }

    /// The edits this root made since the last call, for its page (one frame).
    pub fn take_edits(&self) -> Vec<u8> {
        in_root(self.id, take_edits)
    }

    /// Build `app` once (its body runs once, its holes follow); its edits wait in [`take_edits`].
    pub fn mount(app: impl Fn() -> View + 'static) -> Self {
        let r = Self::new();
        r.show(app);
        r
    }

    /// Put `app` in the page.
    pub fn show(&self, app: impl Fn() -> View + 'static) {
        let app: Rc<dyn Fn() -> View> = Rc::new(app);
        *self.app.borrow_mut() = Some(app.clone());
        let (range, shown) = (self.range.clone(), self.shown.clone());
        self.within(move || {
            let ((), owner) = scope(|| {
                let (r, at) = slot();
                range.set(r);
                if let Some((a, b)) = r.0 {
                    dom(|d| match &mut d.server {
                        Some(s) => s.append(a),
                        None => _ = d.op(op::APPEND).u32(0).u32(a).u32(b),
                    });
                }
                comp_as(at, None, Box::new(move || app()));
            });
            shown.set(Some(owner));
        });
    }

    /// `f` as a component of this root whose view never goes in the page:
    /// what runs beside the app with nothing to show (a dev build's
    /// tooling), so the page's markup stays the app's alone (a server's
    /// paint hydrates node for node). It ends with the root.
    pub fn beside(&self, f: impl Fn() -> View + 'static) {
        let before = crate::tree::frames(self.id);
        self.within(move || {
            let (_, at) = slot();
            comp_as(at, None, Box::new(f));
        });
        let new = crate::tree::frames(self.id).into_keys().filter(|k| !before.contains_key(k));
        self.besides.borrow_mut().extend(new.map(String::into_boxed_str));
    }

    /// Build the views again in place, each one's hooks handing back what
    /// they made (by its place among the mounted components): after a hot
    /// reload's markup ([`hot::set_markup`](crate::hot::set_markup)) or a
    /// patch of the code (the app's body runs it). Their page is drawn anew;
    /// what runs beside the app runs on as it was.
    pub fn hot(&self) {
        let Some(app) = self.app.borrow().clone() else { return };
        let besides = self.besides.borrow().clone();
        let beside =
            |k: &str| besides.iter().any(|b| k.strip_prefix(&**b).is_some_and(|r| r.is_empty() || r.starts_with('/')));
        in_root(self.id, || {
            let mut frames = crate::tree::frames(self.id);
            frames.retain(|k, _| !beside(k));
            crate::hot::keep(frames);
            dom(|d| d.remove(self.range.take()));
            if let Some(o) = self.shown.take() {
                o.dispose();
            }
            self.show(move || app());
            crate::hot::end_kept();
        });
    }

    /// `f` in the root's scope (where the app's globals and context live), in a batch.
    pub fn within<R>(&self, f: impl FnOnce() -> R) -> R {
        in_root(self.id, || with_owner(self.owner, || batch(f)))
    }

    pub fn owner(&self) -> Owner {
        self.owner
    }

    /// A frame from the page: an event (its handlers run, innermost first,
    /// in one batch) or a script's reply. False if nothing took it (the
    /// nodes are gone, or the frame is malformed).
    pub fn event(&self, frame: &[u8]) -> bool {
        in_root(self.id, || self.event_in(frame))
    }

    fn event_in(&self, frame: &[u8]) -> bool {
        crate::reactive::drop_left();
        match frame.first() {
            Some(&EVAL) => return eval_reply(frame),
            Some(&EVENT | &ASKED) => {}
            _ => return false,
        }
        let Some(raw) = Raw::decode(frame) else { return false };
        let flow: Rc<Flow> = Rc::default();
        let mut took = false;
        batch(|| {
            for id in &raw.ids {
                let h = dom(|d| {
                    let ls = d.handlers.get(id)?;
                    ls.iter().find(|l| l.name == raw.name).map(|l| (l.run.clone(), l.owner, l.comp.clone()))
                });
                let Some((h, owner, comp)) = h else { continue };
                let run = || (h.borrow_mut())(&raw, &flow);
                match owner {
                    Some(o) if o.alive() => with_owner(handler_owner(&comp, o), run),
                    Some(_) => continue,
                    None => run(),
                }
                took = true;
                if flow.stopped.get() {
                    break;
                }
            }
        });
        if !took && cfg!(feature = "dev") {
            stale(&raw.name, &raw.ids);
        }
        took
    }

    /// The errors no boundary caught (each also said on stderr), taken.
    pub fn uncaught(&self) -> Vec<String> {
        in_root(self.id, || dom(|d| std::mem::take(&mut d.uncaught)))
    }
}

/// An event the page sent for nodes whose handlers are gone (drawn again
/// or removed before it arrived) is dropped. In AIR an event on a removed
/// element never reaches a handler either (`vdom-events.ts:108-111`: the
/// root's listener walks the live path), but over the wire the page can
/// send it first: a dev build says so once per event name, as AIR's
/// handler that "silently never fires" is warned (`dev-events.ts:7-8,70-73`).
fn stale(name: &str, ids: &[u32]) {
    thread_local!(static SAID: RefCell<std::collections::HashSet<String>> = RefCell::default());
    if SAID.with_borrow_mut(|s| s.insert(name.to_owned())) {
        crate::warn(
            "ui:stale-event",
            &format!(
                "a `{name}` the page sent for nodes {ids:?} was dropped: their handlers are gone (the element was \
                 drawn again or removed before the event arrived; said once per event name)"
            ),
        );
    }
}

impl Default for Root {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        in_root(self.id, || {
            let r = self.range.get();
            dom(|d| d.remove(r));
            crate::global::reset();
            self.owner.dispose();
            dom(|d| d.root = None);
        });
        crate::task::end_root(self.id);
        crate::tree::end_root(self.id);
    }
}

/// The mounted root's owner on this thread, if any.
pub(crate) fn root_owner() -> Option<Owner> {
    dom(|d| d.root.filter(|r| r.alive()))
}

/// Handlers and scripts alive on this thread's page.
pub(crate) fn live_dom() -> (usize, usize) {
    DOMS.with_borrow(|ds| {
        ds.values().fold((0, 0), |(h, e), d| (h + d.handlers.values().map(Vec::len).sum::<usize>(), e + d.evals.len()))
    })
}

/// Planted renderer faults, for the gates that prove a broken render fails a test.
mod fault {
    #[cfg(feature = "fault")]
    thread_local!(pub(super) static PLANTED: std::cell::Cell<u8> = const { std::cell::Cell::new(0) });

    pub(super) fn unbound() -> bool {
        #[cfg(feature = "fault")]
        return PLANTED.get() == 1;
        #[cfg(not(feature = "fault"))]
        false
    }

    pub(super) fn lost_row() -> bool {
        #[cfg(feature = "fault")]
        return PLANTED.get() == 2;
        #[cfg(not(feature = "fault"))]
        false
    }
}

/// Plant a renderer fault on this thread (feature `fault`, tests only):
/// a text hole that stops following after its first value, or a list that
/// loses the last row it makes.
#[cfg(feature = "fault")]
#[doc(hidden)]
pub mod __fault {
    #[derive(Clone, Copy, Debug)]
    pub enum Fault {
        None = 0,
        UnboundHole = 1,
        LostRow = 2,
    }

    pub fn plant(f: Fault) {
        super::fault::PLANTED.set(f as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_longest_increasing_run_stays() {
        let pick = |s: &[isize]| {
            longest_increasing(s).iter().enumerate().filter(|(_, b)| **b).map(|(i, _)| i).collect::<Vec<_>>()
        };
        assert_eq!(pick(&[0, 1, 2]), [0, 1, 2]);
        assert_eq!(pick(&[2, 0, 1]), [1, 2], "one move: the first row to the end");
        assert_eq!(pick(&[-1, 0, -1, 1]), [1, 3], "new rows never stay");
        assert_eq!(pick(&[]), Vec::<usize>::new());
    }
}

//! Views without `rsx!` (aio's `h()`): an element is a builder,
//! `h("button").attr("class", "save").on("click", move |_: Event<MouseData>| n += 1).child("Save")`,
//! and a component is placed with [`comp`]. It makes the same templates and
//! holes `rsx!` does, one template per tag and child count, made once per
//! thread: what follows state is what reads it (a `Signal` given as an
//! attribute or a child, or an `attr_with` / `child_with` closure).
use crate::dom::{
    AttrHole, AttrSlot, Child, IntoAttr, IntoChild, IntoHandler, Range, Template, Tree, View, attr, child,
    comp as place, handler, on, tpl, view,
};
use crate::event::FromRaw;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

type Attr = Rc<dyn Fn(&mut AttrSlot)>;
type Listen = Rc<dyn Fn() -> crate::dom::Handler>;
type Kid = Rc<dyn Fn() -> Child>;

/// An element being described ([`h`]); a [`View`] once [`view`](H::view)
/// is called, or given as a child as it is.
#[derive(Clone)]
#[must_use = "an element shows once it is a view: .view(), or a child of another"]
pub struct H {
    tag: &'static str,
    attrs: Vec<(&'static str, Attr)>,
    events: Vec<(&'static str, Listen)>,
    children: Vec<Kid>,
}

/// Element `tag` (`"div"`, `"svg"`; inside an `svg`, SVG's own).
pub fn h(tag: &'static str) -> H {
    H { tag, attrs: vec![], events: vec![], children: vec![] }
}

impl H {
    /// Attribute `name` (`"class"`, `"aria-label"`, `"value"`): a string, a
    /// `bool` (present or not), an `Option`, or a `Signal` / `Memo` (followed).
    pub fn attr<V: IntoAttr<'static> + Clone + 'static>(mut self, name: &'static str, v: V) -> Self {
        self.attrs.push((name, Rc::new(move |s: &mut AttrSlot| s.set(v.clone().into_attr()))));
        self
    }

    /// Attribute `name` as `f()`, again whenever what it read changes.
    pub fn attr_with<V: IntoAttr<'static>>(mut self, name: &'static str, f: impl Fn() -> V + 'static) -> Self {
        self.attrs.push((name, Rc::new(move |s: &mut AttrSlot| s.set(f().into_attr()))));
        self
    }

    /// Event `name` (`"click"`, `"input"`, `"keydown"`): its data is the
    /// handler's (`|e: Event<MouseData>|`, `|e: Event<FormData>|`).
    pub fn on<T: FromRaw, M: 'static, F: IntoHandler<T, M> + Clone + 'static>(
        mut self,
        name: &'static str,
        f: F,
    ) -> Self {
        self.events.push((name, Rc::new(move || handler(f.clone()))));
        self
    }

    /// A child: text, a number, a view, an element, views, an `Option`, a
    /// `Signal` (followed).
    pub fn child<C: IntoChild + Clone + 'static>(mut self, c: C) -> Self {
        self.children.push(Rc::new(move || c.clone().into_child()));
        self
    }

    /// A child as `f()`, again whenever what it read changes.
    pub fn child_with<C: IntoChild>(mut self, f: impl Fn() -> C + 'static) -> Self {
        self.children.push(Rc::new(move || f().into_child()));
        self
    }

    /// Children, in order.
    pub fn children<C: IntoChild + Clone + 'static>(self, cs: impl IntoIterator<Item = C>) -> Self {
        cs.into_iter().fold(self, H::child)
    }

    pub fn view(self) -> View {
        let me = Rc::new(self);
        Ok(Tree::new(move || me.build()))
    }

    fn build(&self) -> Range {
        let outer = IN_SVG.get();
        let svg = outer || self.tag == "svg";
        let t = template(self.tag, self.children.len(), outer && self.tag != "svg");
        let b = tpl(t);
        for (name, f) in &self.attrs {
            let f = f.clone();
            attr(b, name, Box::new(move |s: &mut AttrSlot| f(s)) as Box<dyn AttrHole>);
        }
        for (name, f) in &self.events {
            on(b, name, f());
        }
        // Inside an `svg` (not its `foreignObject`), a child element is SVG's.
        let inner = svg && self.tag != "foreignObject";
        for (i, f) in self.children.iter().enumerate() {
            let f = f.clone();
            child(
                b + 1 + i as u32,
                Box::new(move || match f() {
                    Child::Views(v) => Child::Views(v.into_iter().map(|v| within(inner, v)).collect()),
                    text => text,
                }),
            );
        }
        view(t, b)
    }
}

thread_local! {
    /// An element built now is inside an `svg`.
    static IN_SVG: Cell<bool> = const { Cell::new(false) };
    /// The templates made, by tag, child count and namespace: one each per thread.
    static MADE: RefCell<HashMap<(&'static str, usize, bool), &'static Template>> = RefCell::default();
}

/// `v` built with [`IN_SVG`] at `svg`.
fn within(svg: bool, v: View) -> View {
    let t = v?;
    Ok(Tree::new(move || {
        let was = IN_SVG.replace(svg);
        struct Back(bool);
        impl Drop for Back {
            fn drop(&mut self) {
                IN_SVG.set(self.0);
            }
        }
        let _b = Back(was);
        t.build()
    }))
}

/// Element `tag` with `n` child anchors (`svg`: in SVG's namespace), its
/// marks the element then each anchor.
fn template(tag: &'static str, n: usize, svg: bool) -> &'static Template {
    MADE.with_borrow_mut(|m| {
        *m.entry((tag, n, svg)).or_insert_with(|| {
            let mut nodes = vec![if svg { 5 } else { 1 }];
            nodes.extend_from_slice(&(tag.len() as u32).to_le_bytes());
            nodes.extend_from_slice(tag.as_bytes());
            nodes.push(0);
            nodes.extend_from_slice(&(n as u16).to_le_bytes());
            nodes.push(1);
            nodes.extend(std::iter::repeat_n(4, n));
            let at = crate::hot::At { file: "", line: 0, col: 0, idx: 0 };
            let count = 1 + n as u32;
            Box::leak(Box::new(Template { nodes: nodes.leak(), count, roots: 1, marks: count, first: 0, last: 0, at }))
        })
    })
}

impl IntoChild for H {
    fn into_child(self) -> Child {
        Child::Views(vec![self.view()])
    }
}

impl From<H> for View {
    fn from(e: H) -> Self {
        e.view()
    }
}

/// Component `c` placed with `props` (`rsx!`'s `Row { .. }`):
/// `comp(Row, RowProps::builder().name("Ada").build())`.
pub fn comp<P: Clone + 'static, M: 'static>(c: impl crate::Component<P, M>, props: P) -> View {
    let name = std::any::type_name_of_val(&c).rsplit("::").next().unwrap_or("");
    let c = Rc::new(c);
    Ok(Tree::new(move || {
        let (range, at) = crate::dom::slot();
        let (c, props) = (c.clone(), props.clone());
        place(at, name, None, Box::new(move || c.call(props.clone())));
        range
    }))
}

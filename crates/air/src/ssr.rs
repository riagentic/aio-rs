//! A view rendered to HTML on the server (aio's `renderToString`), for a
//! page's first paint: the page shows it before its script runs, then
//! `page.js` hydrates it (`data-aio-painted` on `#main`): the client's first
//! frame is built apart and its nodes' ids, listeners and values move to
//! the server's nodes, which stay in the page. So the markup must parse
//! back into the nodes air built: a text node that is empty or follows
//! another text is written after a `<!--a-->` marker (HTML would merge or
//! drop it), and a control's properties are written as what shows them
//! before any script (`value`, `checked`, a `textarea`'s text).
//!
//! A server root sends no edits: its templates are kept as instances (a
//! template and the base of its ids) with what the holes wrote, and the
//! markup is written from them. A hole that reads no signal runs once and
//! leaves nothing behind; one that reads runs as in a page (a resource
//! loads, a boundary shows its fallback), so the markup is what
//! [`TestPage`](crate::testing::TestPage) shows once settled.
use crate::dom::{Root, Serve, Template, View, op};
use crate::testing::{Rd, set_attr, set_prop, set_style};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

/// `app` rendered once, its ready tasks run until it settles: the markup
/// of `#main`'s children. A view that failed with no `ErrorBoundary` above
/// it is the error. What waits (the network, a timer) shows as it is then
/// (a `Suspense` its fallback).
pub fn render_to_string(app: impl Fn() -> View + 'static) -> Result<String, String> {
    render(app, || ()).map(|(html, ())| html)
}

/// [`render_to_string`], and what `read` makes of the settled views in
/// their root's scope, before they end (aio's `collectHead`: the head they
/// asked for).
pub fn render<R>(app: impl Fn() -> View + 'static, read: impl FnOnce() -> R) -> Result<(String, R), String> {
    let root = shown(app, false)?;
    let html = root.page(|p| p.markup()).unwrap_or_default();
    Ok((html, root.within(read)))
}

/// A server root showing `app` (`stream`: its waiting boundaries marked), settled.
fn shown(app: impl Fn() -> View + 'static, stream: bool) -> Result<Root, String> {
    let root = Root::serving();
    root.page(|p| p.stream = stream);
    root.show(app);
    settle();
    let uncaught = root.uncaught();
    match uncaught.is_empty() {
        true => Ok(root),
        false => Err(format!("a view failed and no ErrorBoundary is above it: {}", uncaught.join("; "))),
    }
}

/// Run the ready tasks until none is left.
fn settle() {
    while crate::run_tasks() > 0 {
        crate::drop_left();
    }
    crate::drop_left();
}

/// [`render`] streamed (aio's `renderToStream`, and more): the markup of
/// `#main` now, each `Suspense` that waits showing its fallback between
/// `<!--s:n-->` and `<!--/s:n-->`; then [`Streaming`] hands each one's
/// children as it loads, a `<template data-aio-swap="n">` that
/// [`SWAP_JS`] puts in place, with the values of the seeded resources
/// they loaded ([`seeded_resource`](crate::seeded_resource)) for the
/// client's first frame (a JSON block, no script that runs).
pub fn stream<R>(app: impl Fn() -> View + 'static, read: impl FnOnce() -> R) -> Result<(String, R, Streaming), String> {
    let root = shown(app, true)?;
    let html = root.page(Page::markup_streamed).unwrap_or_default();
    let r = root.within(read);
    Ok((html, r, Streaming { root }))
}

/// A streamed page's views, kept while what they wait for loads ([`stream`]).
pub struct Streaming {
    root: Root,
}

impl Streaming {
    /// Does a boundary sent with its fallback still wait for a seeded resource?
    pub fn waiting(&self) -> bool {
        self.root.page(|p| !p.sent.is_empty() && p.loading > 0).unwrap_or_default()
    }

    /// The ready tasks run: a swap for each boundary that loaded since,
    /// and a block of the seeded values loaded since (empty: nothing new).
    pub fn ready(&mut self) -> String {
        self.root.within(settle);
        self.root.page(|p| p.swaps() + &p.data_block()).unwrap_or_default()
    }

    /// Wait on this thread, each part to `send` as it is ready, until no
    /// boundary waits or `until` (what still waits is the client's to load).
    pub fn finish(mut self, until: std::time::Instant, mut send: impl FnMut(String)) {
        let me = std::thread::current();
        crate::on_wake(move || me.unpark());
        loop {
            let part = self.ready();
            if !part.is_empty() {
                send(part);
            }
            let now = std::time::Instant::now();
            if !self.waiting() || now >= until {
                break;
            }
            std::thread::park_timeout(until - now);
        }
    }
}

/// The page's part of [`stream`] (no inline script runs: this is loaded
/// with the page's script, before the swaps arrive): each `<template
/// data-aio-swap="n">` takes the place of what lies between `<!--s:n-->`
/// and `<!--/s:n-->` in `#main` as soon as it is whole, all of them once
/// the page is read (before the view's script runs and hydrates).
pub const SWAP_JS: &str = r#"(() => {
  const swap = (t) => {
    const n = t.getAttribute('data-aio-swap'), main = document.getElementById('main')
    const w = main && document.createTreeWalker(main, NodeFilter.SHOW_COMMENT)
    let a = null, b = null
    for (let c; w && !b && (c = w.nextNode()); ) {
      if (c.data === 's:' + n) a = c
      else if (a && c.data === '/s:' + n) b = c
    }
    if (b) {
      while (a.nextSibling !== b) a.nextSibling.remove()
      b.before(t.content)
      a.data = b.data = ''
    }
    t.remove()
  }
  const all = (done) => document.querySelectorAll('template[data-aio-swap]').forEach((t) => (done || t.nextSibling) && swap(t))
  new MutationObserver(() => all(false)).observe(document.body || document.documentElement, { childList: true })
  document.addEventListener('readystatechange', () => document.readyState !== 'loading' && all(true))
})()"#;

thread_local! {
    /// The seeded resources' values the page carried (key → JSON), for the first frame.
    static SEEDS: RefCell<HashMap<String, String>> = RefCell::default();
}

/// The page's value of the seeded resource `key` (a streamed page's data
/// block), for the client's first frame: aio's web build hands them over
/// before its view is shown.
pub fn seed(key: String, json: String) {
    SEEDS.with_borrow_mut(|s| s.insert(key, json));
}

pub(crate) fn seed_of(key: &str) -> Option<String> {
    SEEDS.with_borrow_mut(|s| s.remove(key))
}

/// The first frame is out: values not taken are no one's.
pub(crate) fn end_seeds() {
    SEEDS.with_borrow_mut(|s| {
        if !s.is_empty() {
            s.clear();
        }
    });
}

/// `s` as a JSON string.
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// HTML's elements that have no end tag.
const VOID: &[&str] =
    &["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track", "wbr"];

/// The marker before a text node HTML would merge or drop.
pub const TEXT_MARK: &str = "<!--a-->";

/// `s` escaped into `out`: as text (`&`, `<`, `>`), or `quote`d (`&`, `"`).
fn escape(out: &mut String, s: &str, quote: bool) {
    let mut last = 0;
    for (i, b) in s.bytes().enumerate() {
        let to = match b {
            b'&' => "&amp;",
            b'<' if !quote => "&lt;",
            b'>' if !quote => "&gt;",
            b'"' if quote => "&quot;",
            _ => continue,
        };
        out.push_str(&s[last..i]);
        out.push_str(to);
        last = i + 1;
    }
    out.push_str(&s[last..]);
}

/// A name markup can hold (no space, quote, `>`, `/`, `=`, `<` or control).
fn name_ok(k: &str) -> bool {
    !k.is_empty() && k.chars().all(|c| !c.is_control() && !c.is_whitespace() && !"\"'>/=<".contains(c))
}

/// Where a run of sibling nodes is written: after a text (the next one is
/// marked), in a `script` or a `style` (text is raw).
#[derive(Default)]
pub(crate) struct Out {
    pub(crate) html: String,
    after_text: bool,
    raw: bool,
    /// Streamed: the waiting boundaries written (their first ids).
    sent: Option<Vec<u32>>,
}

impl Out {
    pub(crate) fn text(&mut self, s: &str) {
        if self.raw {
            self.html.push_str(s);
        } else {
            if s.is_empty() || self.after_text {
                self.html.push_str(TEXT_MARK);
            }
            escape(&mut self.html, s, false);
        }
        self.after_text = true;
    }

    pub(crate) fn comment(&mut self) {
        self.marker("");
    }

    fn marker(&mut self, data: &str) {
        self.html.push_str("<!--");
        self.html.push_str(data);
        self.html.push_str("-->");
        self.after_text = false;
    }

    /// An element: its start tag (`start`: as written already), its content
    /// (`inner` writes its children), its end tag.
    pub(crate) fn element(
        &mut self,
        tag: &str,
        attrs: &[(String, String)],
        props: &[(String, String)],
        start: Option<&str>,
        inner: impl FnOnce(&mut Self),
    ) {
        match start {
            Some(s) => self.html.push_str(s),
            None => start_tag(&mut self.html, tag, attrs, props),
        }
        self.after_text = false;
        if VOID.contains(&tag) {
            return;
        }
        let prop = |k: &str| props.iter().find(|(x, _)| x == k).map(|(_, v)| v.as_str());
        let value = || prop("value").or_else(|| attrs.iter().find(|(k, _)| k == "value").map(|(_, v)| v.as_str()));
        match prop("innerHTML") {
            Some(html) => self.html.push_str(html),
            None if tag == "textarea" => escape(&mut self.html, value().unwrap_or_default(), false),
            None => {
                let raw = std::mem::replace(&mut self.raw, tag == "script" || tag == "style");
                inner(self);
                (self.raw, self.after_text) = (raw, false);
            }
        }
        self.html.push_str("</");
        self.html.push_str(tag);
        self.html.push('>');
    }
}

/// An element's start tag: its attributes, and the properties that show
/// before any script (an input's `value`, `checked`, `selected`).
fn start_tag(out: &mut String, tag: &str, attrs: &[(String, String)], props: &[(String, String)]) {
    let prop = |k: &str| props.iter().find(|(x, _)| x == k).map(|(_, v)| v.as_str());
    out.push('<');
    out.push_str(tag);
    let shown = |k: &str| match k {
        "value" => tag != "textarea" && !(tag == "input" && prop("value").is_some()),
        "checked" | "selected" => prop(k).is_none(),
        _ => true,
    };
    let mut attr = |k: &str, v: &str| {
        out.push(' ');
        out.push_str(k);
        out.push_str("=\"");
        escape(out, v, true);
        out.push('"');
    };
    for (k, v) in attrs.iter().filter(|(k, _)| name_ok(k) && shown(k)) {
        attr(k, v);
    }
    if let (Some(v), "input") = (prop("value"), tag) {
        attr("value", v);
    }
    for k in ["checked", "selected"] {
        if prop(k) == Some("true") {
            out.push(' ');
            out.push_str(k);
        }
    }
    out.push('>');
}

// ------------------------------------------------------------ the server's page

const NONE: u32 = u32::MAX;

/// A template read once per thread: its nodes in pre-order.
struct Parsed {
    nodes: Vec<PNode>,
    roots: Vec<u32>,
}

struct PNode {
    kind: PKind,
    children: Vec<u32>,
    /// Its mark (its id is the base plus it), or [`NONE`].
    mark: u32,
}

enum PKind {
    /// An element, its attributes and its start tag as written with them.
    El(Rc<str>, Vec<(String, String)>, String),
    Text(String),
    Comment,
}

fn parse(b: &[u8], roots: u32) -> Parsed {
    fn node(r: &mut Rd, p: &mut Parsed, root: bool, marks: &mut u32) -> u32 {
        let (kind, children, mark) = match r.u8() {
            1 | 5 => {
                let tag = r.str();
                let mut attrs = vec![];
                for _ in 0..r.u8() {
                    let (k, v) = (r.str(), r.str());
                    set_attr(&mut attrs, k, v);
                }
                let children = r.u16();
                let mut start = String::new();
                start_tag(&mut start, &tag, &attrs, &[]);
                (PKind::El(tag.into(), attrs, start), children, r.u8() != 0)
            }
            2 => (PKind::Text(r.str()), 0, false),
            3 => (PKind::Text(String::new()), 0, true),
            4 => (PKind::Comment, 0, true),
            k => panic!("air: unknown template node {k}"),
        };
        let i = p.nodes.len() as u32;
        // Roots are always marked: their ids are the view's.
        let mark = if mark || root {
            *marks += 1;
            *marks - 1
        } else {
            NONE
        };
        p.nodes.push(PNode { kind, children: vec![], mark });
        for _ in 0..children {
            let c = node(r, p, false, marks);
            p.nodes[i as usize].children.push(c);
        }
        i
    }
    let (mut r, mut p, mut marks) = (Rd { b, p: 0 }, Parsed { nodes: vec![], roots: vec![] }, 0);
    for _ in 0..roots {
        let n = node(&mut r, &mut p, true, &mut marks);
        p.roots.push(n);
    }
    p
}

thread_local! {
    /// Templates read so far, by address and markup edit (a hot reload's).
    static PARSED: RefCell<HashMap<(usize, u64), Rc<Parsed>>> = RefCell::default();
}

/// What a node of a server root holds beyond its template: its text, its
/// attribute edits in order, the views put before it.
#[derive(Default)]
struct Extra {
    text: Option<String>,
    edits: Vec<(u8, String, String)>,
    before: VecDeque<u32>,
}

/// An id: the instance it is a node of ([`NONE`]: a text of its own, or
/// `#main`), the node it was put before (its view's first: [`NONE`] while
/// not in the page), what it holds.
struct Slot {
    inst: u32,
    at: u32,
    x: Option<Box<Extra>>,
}

const EMPTY: Slot = Slot { inst: NONE, at: NONE, x: None };

/// A `Suspense` of a server root: its children's view and whether its
/// fallback shows.
struct Bound {
    kids: Option<u32>,
    showing: Rc<std::cell::Cell<bool>>,
}

/// A server root's page: instances of templates and what holes wrote to
/// them, `#main` (id 0) holding what was appended.
#[derive(Default)]
pub(crate) struct Page {
    slots: Vec<Slot>,
    insts: Vec<(Rc<Parsed>, u32)>,
    /// Streamed ([`stream`]): its waiting boundaries are marked.
    pub(crate) stream: bool,
    /// Its first markup is out.
    pub(crate) painted: bool,
    /// Seeded loads under way.
    pub(crate) loading: usize,
    /// Seeded values loaded and not sent yet.
    pub(crate) data: Vec<(Rc<str>, String)>,
    /// Its boundaries by their first id, and by their anchor's.
    bounds: HashMap<u32, Bound>,
    anchors: HashMap<u32, u32>,
    /// The boundaries sent with their fallback, not filled in yet.
    sent: Vec<u32>,
}

impl Serve for Page {
    fn page(&mut self) -> &mut Page {
        self
    }

    fn clone_of(&mut self, t: &'static Template, hot: Option<(&[u8], u64)>, base: u32) {
        let key = (t as *const Template as usize, hot.map_or(0, |h| h.1));
        let p = PARSED.with_borrow_mut(|m| {
            m.entry(key).or_insert_with(|| Rc::new(parse(hot.map_or(t.nodes, |h| h.0), t.roots))).clone()
        });
        let inst = self.insts.len() as u32;
        self.insts.push((p, base));
        let end = (base + t.marks) as usize;
        if self.slots.len() < end {
            self.slots.resize_with(end, || EMPTY);
        }
        self.slots[base as usize..end].iter_mut().for_each(|s| s.inst = inst);
    }

    fn bound(&mut self, first: u32, at: u32, kids: Option<u32>, showing: Rc<std::cell::Cell<bool>>) {
        if self.stream {
            self.bounds.insert(first, Bound { kids, showing });
            self.anchors.insert(at, first);
        }
    }

    fn set_text(&mut self, id: u32, s: &str) {
        self.extra(id).text = Some(s.to_owned());
    }

    fn text_node(&mut self, id: u32, s: &str) {
        *self.slot(id) =
            Slot { inst: NONE, at: NONE, x: Some(Box::new(Extra { text: Some(s.into()), ..Extra::default() })) };
    }

    fn edit(&mut self, id: u32, code: u8, name: &str, v: &str) {
        self.extra(id).edits.push((code, name.into(), v.into()));
    }

    fn insert(&mut self, at: u32, first: u32) {
        self.unplace(first);
        if !self.heads(at) {
            self.extra(at).before.push_back(first);
            self.slots[first as usize].at = at;
            return;
        }
        // Before a view's first node: next to that view, where it is
        // (nowhere while it is not in the page, as a fragment keeps it).
        let list = self.slots[at as usize].at;
        if list == NONE {
            return;
        }
        let before = &mut self.extra(list).before;
        let i = before.iter().position(|x| *x == at).expect("a placed view is in its list");
        before.insert(i, first);
        self.slots[first as usize].at = list;
    }

    fn append(&mut self, first: u32) {
        self.unplace(first);
        self.extra(0).before.push_back(first);
        self.slots[first as usize].at = 0;
    }

    fn unplace(&mut self, first: u32) {
        let list = std::mem::replace(&mut self.slot(first).at, NONE);
        if list != NONE {
            let before = &mut self.extra(list).before;
            if let Some(i) = before.iter().position(|x| *x == first) {
                before.remove(i);
            }
        }
    }

    fn style(&mut self, first: u32, property: &str, v: &str) {
        if let Some(el) = self.first_el(first) {
            self.edit(el, op::SET_ATTR, &format!("style:{property}"), v);
        }
    }
}

impl Page {
    fn slot(&mut self, id: u32) -> &mut Slot {
        let i = id as usize;
        if self.slots.len() <= i {
            self.slots.resize_with(i + 1, || EMPTY);
        }
        &mut self.slots[i]
    }

    fn extra(&mut self, id: u32) -> &mut Extra {
        self.slot(id).x.get_or_insert_default()
    }

    /// Is `id` the first node of a view in a list (a root's first, a text)?
    fn heads(&self, id: u32) -> bool {
        let s = &self.slots[id as usize];
        s.inst == NONE || self.insts[s.inst as usize].1 == id
    }

    fn first_el(&self, first: u32) -> Option<u32> {
        let s = &self.slots[first as usize];
        if s.inst == NONE {
            return None;
        }
        let (p, base) = &self.insts[s.inst as usize];
        p.roots.iter().find_map(|&r| {
            let n = &p.nodes[r as usize];
            let id = base + n.mark;
            let before = self.slots[id as usize].x.as_ref().map(|x| &x.before);
            before.and_then(|b| b.iter().find_map(|v| self.first_el(*v))).or_else(|| match n.kind {
                PKind::El(..) if self.text_of(id).is_none() => Some(id),
                _ => None,
            })
        })
    }

    fn text_of(&self, id: u32) -> Option<&str> {
        self.slots.get(id as usize)?.x.as_ref()?.text.as_deref()
    }

    /// `#main`'s children as markup.
    pub(crate) fn markup(&mut self) -> String {
        let mut out = Out::default();
        self.before(0, &mut out);
        out.html
    }

    /// [`Self::markup`], its waiting boundaries marked and kept to send later.
    fn markup_streamed(&mut self) -> String {
        let mut out = Out { sent: Some(vec![]), ..Out::default() };
        self.before(0, &mut out);
        self.sent = out.sent.unwrap_or_default();
        self.painted = true;
        out.html
    }

    /// A swap for each boundary sent waiting that shows its children now.
    fn swaps(&mut self) -> String {
        let mut out = Out { sent: Some(vec![]), ..Out::default() };
        let mut still = vec![];
        for first in std::mem::take(&mut self.sent) {
            let b = &self.bounds[&first];
            if b.showing.get() {
                still.push(first);
                continue;
            }
            out.html.push_str(&format!("<template data-aio-swap=\"{first}\">"));
            out.after_text = false;
            if let Some(k) = b.kids {
                self.view(k, &mut out);
            }
            out.html.push_str("</template>");
        }
        still.extend(out.sent.unwrap_or_default());
        self.sent = still;
        out.html
    }

    /// The seeded values loaded since, as a JSON block (`<` escaped: no
    /// text in it ends the script).
    fn data_block(&mut self) -> String {
        if self.data.is_empty() {
            return String::new();
        }
        let all: Vec<String> = self.data.drain(..).map(|(k, v)| format!("{}:{v}", json_str(&k))).collect();
        let json = all.join(",").replace('<', "\\u003c");
        format!("<script type=\"application/json\" data-aio-data>{{{json}}}</script>")
    }

    /// A comment node `id`: a waiting boundary's start or end, streamed.
    fn comment(&self, id: Option<u32>, out: &mut Out) {
        if let (Some(id), Some(sent)) = (id, out.sent.as_mut()) {
            if self.bounds.get(&id).is_some_and(|b| b.showing.get()) {
                sent.push(id);
                return out.marker(&format!("s:{id}"));
            }
            if let Some(f) = self.anchors.get(&id).filter(|f| self.bounds[f].showing.get()) {
                return out.marker(&format!("/s:{f}"));
            }
        }
        out.comment()
    }

    fn before(&self, id: u32, out: &mut Out) {
        if let Some(x) = self.slots.get(id as usize).and_then(|s| s.x.as_ref()) {
            x.before.iter().for_each(|v| self.view(*v, out));
        }
    }

    fn view(&self, first: u32, out: &mut Out) {
        let s = &self.slots[first as usize];
        if s.inst == NONE {
            return out.text(self.text_of(first).unwrap_or_default());
        }
        let (p, base) = &self.insts[s.inst as usize];
        p.roots.iter().for_each(|r| self.node(p, *base, *r, out));
    }

    fn node(&self, p: &Parsed, base: u32, n: u32, out: &mut Out) {
        let n = &p.nodes[n as usize];
        let x = match n.mark {
            NONE => None,
            m => {
                self.before(base + m, out);
                self.slots[(base + m) as usize].x.as_deref()
            }
        };
        // A node whose text was set is that text (as the page's would be).
        if let Some(t) = x.and_then(|x| x.text.as_deref()) {
            return out.text(t);
        }
        match &n.kind {
            PKind::Text(t) => out.text(t),
            PKind::Comment => self.comment((n.mark != NONE).then(|| base + n.mark), out),
            PKind::El(tag, attrs, start) => {
                let kids = |out: &mut Out| n.children.iter().for_each(|c| self.node(p, base, *c, out));
                match x.filter(|x| !x.edits.is_empty()) {
                    None => out.element(tag, attrs, &[], Some(start), kids),
                    Some(x) => {
                        let (mut attrs, mut props) = (attrs.clone(), vec![]);
                        for (code, k, v) in &x.edits {
                            match *code {
                                op::SET_ATTR => set_attr(&mut attrs, k.clone(), v.clone()),
                                op::REMOVE_ATTR => match k.strip_prefix("style:") {
                                    Some(s) => set_style(&mut attrs, s, ""),
                                    None => attrs.retain(|(a, _)| a != k),
                                },
                                _ => set_prop(&mut props, k.clone(), v.clone()),
                            }
                        }
                        out.element(tag, &attrs, &props, None, kids)
                    }
                }
            }
        }
    }
}

impl crate::testing::TestPage {
    /// The page's markup inside `#main`, as [`render_to_string`] writes it
    /// (the page as the client would show it settled: the two must agree).
    pub fn markup(&self) -> String {
        let mut out = Out::default();
        self.markup_of(0, &mut out);
        out.html
    }

    fn markup_of(&self, n: usize, out: &mut Out) {
        use crate::testing::Kind;
        for &c in &self.nodes[n].children {
            match &self.nodes[c].kind {
                Kind::Text(s) => out.text(s),
                Kind::Comment => out.comment(),
                Kind::Frag => {}
                Kind::El(tag, attrs) => out.element(tag, attrs, &self.nodes[c].props, None, |o| self.markup_of(c, o)),
            }
        }
    }
}

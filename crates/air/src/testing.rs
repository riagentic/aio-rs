//! A page in Rust for tests: it applies the edits as `page.js` does, to a
//! small DOM of its own, and sends events back as the page would (to every
//! listening node on the path, innermost first). What a test reads
//! (`text`, `attr`, `html`) is what the window would show. Each step runs
//! the ready tasks and applies what they drew; a view's error that no
//! boundary caught fails the step.
use crate::dom::{Root, View, op};
use crate::event::Raw;
use std::collections::HashMap;

#[derive(Clone, Debug)]
enum Kind {
    El(String, Vec<(String, String)>),
    Text(String),
    Comment,
    /// A clone's roots until placed, or a parked view (a DocumentFragment).
    Frag,
}

#[derive(Clone, Debug)]
struct Node {
    kind: Kind,
    children: Vec<usize>,
    parent: Option<usize>,
    listen: Vec<String>,
    id: Option<u32>,
    /// Properties set (`value`, `checked`).
    props: Vec<(String, String)>,
}

struct Tpl {
    /// Nodes of the template in an arena of their own, roots and marks by path.
    nodes: Vec<Node>,
    roots: Vec<usize>,
    marks: Vec<usize>,
}

/// The page: `#main` and what the views put in it.
pub struct TestPage {
    nodes: Vec<Node>,
    ids: HashMap<u32, usize>,
    tpls: HashMap<u32, Tpl>,
    root: Root,
    /// Scripts the views asked the page to run (`eval`), in order: id and code.
    pub scripts: Vec<(u32, String)>,
    /// Bytes the views sent so far.
    pub bytes: usize,
    /// Edit frames applied so far.
    pub frames: usize,
}

struct Rd<'a> {
    b: &'a [u8],
    p: usize,
}

impl Rd<'_> {
    fn u8(&mut self) -> u8 {
        self.p += 1;
        self.b[self.p - 1]
    }
    fn u16(&mut self) -> u16 {
        self.p += 2;
        u16::from_le_bytes(self.b[self.p - 2..self.p].try_into().unwrap())
    }
    fn u32(&mut self) -> u32 {
        self.p += 4;
        u32::from_le_bytes(self.b[self.p - 4..self.p].try_into().unwrap())
    }
    fn str(&mut self) -> String {
        let n = self.u32() as usize;
        self.p += n;
        String::from_utf8(self.b[self.p - n..self.p].to_vec()).unwrap()
    }
}

fn node(kind: Kind) -> Node {
    Node { kind, children: vec![], parent: None, listen: vec![], id: None, props: vec![] }
}

impl TestPage {
    pub fn mount(app: impl Fn() -> View + 'static) -> Self {
        Self::on(Root::mount(app))
    }

    /// A page for `root` (made, maybe shown, by the test).
    pub fn on(root: Root) -> Self {
        let mut main = node(Kind::El("div".into(), vec![("id".into(), "main".into())]));
        main.id = Some(0);
        let mut p = Self {
            nodes: vec![main],
            ids: HashMap::from([(0, 0)]),
            tpls: HashMap::new(),
            root,
            scripts: vec![],
            bytes: 0,
            frames: 0,
        };
        p.flush();
        p
    }

    pub fn root(&self) -> &Root {
        &self.root
    }

    /// Run the ready tasks and apply the edits waiting, until none is left.
    pub fn flush(&mut self) {
        loop {
            crate::run_tasks();
            let b = self.root.take_edits();
            if b.is_empty() {
                break;
            }
            self.bytes += b.len();
            self.frames += 1;
            self.apply(&b);
            // The frame is out: what rows that left held goes, as a host does.
            crate::drop_left();
        }
        let uncaught = self.root.uncaught();
        assert!(uncaught.is_empty(), "a view failed and no ErrorBoundary is above it: {}", uncaught.join("; "));
    }

    /// `f` in the root's scope (its globals, its context), then flush.
    pub fn within<R>(&mut self, f: impl FnOnce() -> R) -> R {
        let r = self.root.within(f);
        self.flush();
        r
    }

    /// Answer script `id` as the page would (`kind` 0: a value it sent, 1: its return, 2: its failure).
    pub fn answer(&mut self, id: u32, kind: u8, json: &str) {
        let mut b = vec![2];
        b.extend_from_slice(&id.to_le_bytes());
        b.push(kind);
        b.extend_from_slice(&(json.len() as u32).to_le_bytes());
        b.extend_from_slice(json.as_bytes());
        self.root.event(&b);
        self.flush();
    }

    fn at(&self, r: &mut Rd) -> usize {
        let id = r.u32();
        *self.ids.get(&id).unwrap_or_else(|| panic!("air: an edit names node {id}, which the page doesn't have"))
    }

    fn apply(&mut self, b: &[u8]) {
        let mut r = Rd { b, p: 0 };
        while r.p < b.len() {
            match r.u8() {
                op::DEFINE => {
                    let id = r.u32();
                    let (roots, count) = (r.u32(), r.u32());
                    let mut t = Tpl { nodes: vec![], roots: vec![], marks: vec![] };
                    let mut left = count;
                    for _ in 0..roots {
                        let n = read_tnode(&mut r, &mut t, None, &mut left);
                        t.roots.push(n);
                    }
                    self.tpls.insert(id, t);
                }
                op::CLONE => {
                    let (tid, base) = (r.u32(), r.u32());
                    let t = &self.tpls[&tid];
                    let frag = self.nodes.len();
                    let off = frag + 1;
                    let marks: Vec<usize> = t.marks.iter().map(|m| m + off).collect();
                    let mut f = node(Kind::Frag);
                    f.children = t.roots.iter().map(|r| r + off).collect();
                    self.nodes.push(f);
                    self.nodes.extend(t.nodes.iter().cloned().map(|mut n| {
                        n.children.iter_mut().for_each(|c| *c += off);
                        n.parent = Some(n.parent.map_or(frag, |p| p + off));
                        n
                    }));
                    for (j, m) in marks.into_iter().enumerate() {
                        let id = base + j as u32;
                        self.nodes[m].id = Some(id);
                        self.ids.insert(id, m);
                    }
                }
                op::SET_TEXT => {
                    let n = self.at(&mut r);
                    self.nodes[n].kind = Kind::Text(r.str());
                }
                op::SET_ATTR => {
                    let n = self.at(&mut r);
                    let (k, v) = (r.str(), r.str());
                    if let Kind::El(_, a) = &mut self.nodes[n].kind {
                        set_attr(a, k, v);
                    }
                }
                op::REMOVE_ATTR => {
                    let n = self.at(&mut r);
                    let k = r.str();
                    if let Kind::El(_, a) = &mut self.nodes[n].kind {
                        match k.strip_prefix("style:") {
                            Some(p) => set_style(a, p, ""),
                            None => a.retain(|(x, _)| *x != k),
                        }
                    }
                }
                op::SET_PROP => {
                    let n = self.at(&mut r);
                    let (k, v) = (r.str(), r.str());
                    self.set_prop(n, k, v);
                }
                op::SET_BOOL => {
                    let n = self.at(&mut r);
                    let k = r.str();
                    let v = r.u8() != 0;
                    self.set_prop(n, k, v.to_string());
                }
                op::LISTEN => {
                    let n = self.at(&mut r);
                    let e = r.str();
                    self.nodes[n].listen.push(e);
                }
                op::INSERT => {
                    let at = self.at(&mut r);
                    let span = self.span(&mut r);
                    let parent = self.nodes[at].parent.expect("an anchor in the page");
                    for n in span {
                        self.detach(n);
                        let i = self.nodes[parent].children.iter().position(|c| *c == at).unwrap();
                        self.nodes[parent].children.insert(i, n);
                        self.nodes[n].parent = Some(parent);
                    }
                }
                op::APPEND => {
                    let parent = self.at(&mut r);
                    for n in self.span(&mut r) {
                        self.detach(n);
                        self.nodes[parent].children.push(n);
                        self.nodes[n].parent = Some(parent);
                    }
                }
                op::REMOVE => {
                    for n in self.span(&mut r) {
                        self.detach(n);
                        self.forget(n);
                    }
                }
                op::TEXT => {
                    let id = r.u32();
                    let mut n = node(Kind::Text(r.str()));
                    n.id = Some(id);
                    self.nodes.push(n);
                    self.ids.insert(id, self.nodes.len() - 1);
                }
                op::PARK => {
                    let frag = self.nodes.len();
                    self.nodes.push(node(Kind::Frag));
                    for n in self.span(&mut r) {
                        self.detach(n);
                        self.nodes[frag].children.push(n);
                        self.nodes[n].parent = Some(frag);
                    }
                }
                op::EVAL => {
                    let id = r.u32();
                    let js = r.str();
                    self.scripts.push((id, js));
                }
                op::EVAL_SEND => {
                    r.u32();
                    r.str();
                }
                op::STYLE => {
                    let span = self.span(&mut r);
                    let (k, v) = (r.str(), r.str());
                    let el = span.into_iter().find(|n| matches!(self.nodes[*n].kind, Kind::El(..)));
                    if let Some(Kind::El(_, a)) = el.map(|n| &mut self.nodes[n].kind) {
                        set_style(a, &k, &v);
                    }
                }
                o => panic!("air: unknown edit {o}"),
            }
        }
    }

    /// A view's siblings, first to last.
    fn span(&self, r: &mut Rd) -> Vec<usize> {
        let (a, b) = (self.at(r), self.at(r));
        let Some(p) = self.nodes[a].parent else { return vec![a] };
        let c = &self.nodes[p].children;
        let i = c.iter().position(|x| *x == a).unwrap();
        let j = c
            .iter()
            .position(|x| *x == b)
            .unwrap_or_else(|| panic!("air: a range's last node is not its first's sibling"));
        c[i..=j].to_vec()
    }

    fn set_prop(&mut self, n: usize, k: String, v: String) {
        let p = &mut self.nodes[n].props;
        match p.iter_mut().find(|(x, _)| *x == k) {
            Some(x) => x.1 = v,
            None => p.push((k, v)),
        }
    }

    fn detach(&mut self, n: usize) {
        if let Some(p) = self.nodes[n].parent.take() {
            self.nodes[p].children.retain(|c| *c != n);
        }
    }

    fn forget(&mut self, n: usize) {
        if let Some(id) = self.nodes[n].id {
            self.ids.remove(&id);
        }
        for c in self.nodes[n].children.clone() {
            self.forget(c);
        }
    }

    /// How many nodes the page can still name (a removed row's are forgotten).
    pub fn named(&self) -> usize {
        self.ids.len()
    }

    fn attrs(&self, n: usize) -> &[(String, String)] {
        match &self.nodes[n].kind {
            Kind::El(_, a) => a,
            _ => &[],
        }
    }

    /// The first element shown in the page with `data-t="t"`.
    fn find(&self, t: &str) -> usize {
        self.find_in(0, t).unwrap_or_else(|| panic!("no data-t={t:?} in {}", self.html()))
    }

    fn find_in(&self, n: usize, t: &str) -> Option<usize> {
        if self.attrs(n).iter().any(|(k, v)| k == "data-t" && v == t) {
            return Some(n);
        }
        self.nodes[n].children.iter().find_map(|c| self.find_in(*c, t))
    }

    /// Is `data-t="t"` shown?
    pub fn present(&self, t: &str) -> bool {
        self.find_in(0, t).is_some()
    }

    /// The text of `data-t="t"`.
    pub fn text(&self, t: &str) -> String {
        self.text_of(self.find(t))
    }

    fn text_of(&self, n: usize) -> String {
        match &self.nodes[n].kind {
            Kind::Text(s) => s.clone(),
            Kind::Comment | Kind::Frag => String::new(),
            Kind::El(..) => self.nodes[n].children.iter().map(|c| self.text_of(*c)).collect(),
        }
    }

    /// Attribute `name` of `data-t="t"` (a property set wins: what a control shows).
    pub fn attr(&self, t: &str, name: &str) -> Option<String> {
        let n = self.find(t);
        let p = self.nodes[n].props.iter().find(|(k, _)| k == name);
        p.or_else(|| self.attrs(n).iter().find(|(k, _)| k == name)).map(|(_, v)| v.clone())
    }

    /// The texts of the children of `data-t="t"`, in order.
    pub fn texts(&self, t: &str) -> Vec<String> {
        let n = self.find(t);
        self.nodes[n]
            .children
            .iter()
            .filter(|c| matches!(self.nodes[**c].kind, Kind::El(..)))
            .map(|c| self.text_of(*c))
            .collect()
    }

    /// The page's markup inside `#main` (comments as `<!---->`).
    pub fn html(&self) -> String {
        self.nodes[0].children.iter().map(|c| self.html_of(*c)).collect()
    }

    fn html_of(&self, n: usize) -> String {
        match &self.nodes[n].kind {
            Kind::Text(s) => s.clone(),
            Kind::Comment => "<!---->".into(),
            Kind::Frag => String::new(),
            Kind::El(tag, a) => {
                let attrs: String = a.iter().map(|(k, v)| format!(" {k}=\"{v}\"")).collect();
                let inner: String = self.nodes[n].children.iter().map(|c| self.html_of(*c)).collect();
                format!("<{tag}{attrs}>{inner}</{tag}>")
            }
        }
    }

    /// Fire `e` at `data-t="t"`: every node on its path up that listens for
    /// it gets it, innermost first (as the page's delegation sends it); then
    /// the edits apply. A field's `value` property is sent as its value.
    pub fn fire(&mut self, t: &str, e: Raw) {
        let e = self.aim(t, e);
        assert!(self.root.event(&e.encode()), "the views took the event");
        self.flush();
    }

    /// `e` as the page sends it for `data-t="t"` now (the nodes on its path
    /// that listen for it, a field's value), not sent: to send later, as an
    /// event that crossed a redraw on the wire.
    pub fn aim(&self, t: &str, mut e: Raw) -> Raw {
        let at = self.find(t);
        if e.value.is_empty()
            && let Some((_, v)) = self.nodes[at].props.iter().find(|(k, _)| k == "value")
        {
            e.value = v.clone();
        }
        let mut n = Some(at);
        e.ids.clear();
        while let Some(i) = n {
            if self.nodes[i].listen.contains(&e.name) {
                e.ids.push(self.nodes[i].id.expect("a listening node is named"));
            }
            n = self.nodes[i].parent;
        }
        assert!(!e.ids.is_empty(), "nothing listens for {} at data-t={t:?}", e.name);
        e
    }

    pub fn click(&mut self, t: &str) {
        self.fire(t, Raw { name: "click".into(), button: Some(0), ..Raw::default() });
    }

    pub fn key(&mut self, t: &str, key: &str) {
        self.fire(t, Raw { name: "keydown".into(), key: key.into(), ..Raw::default() });
    }

    /// Type `value` into `data-t="t"` (one `input` event with the whole value).
    pub fn input(&mut self, t: &str, value: &str) {
        let n = self.find(t);
        self.set_prop(n, "value".into(), value.into());
        self.fire(t, Raw { name: "input".into(), value: value.into(), ..Raw::default() });
    }

    /// Tick or untick the checkbox `data-t="t"` (its `change`).
    pub fn check(&mut self, t: &str, on: bool) {
        let n = self.find(t);
        self.set_prop(n, "checked".into(), on.to_string());
        self.fire(t, Raw { name: "change".into(), checked: on, ..Raw::default() });
    }
}

/// Attribute `k` set to `v` (`style:<property>`: that CSS property).
fn set_attr(a: &mut Vec<(String, String)>, k: String, v: String) {
    if let Some(p) = k.strip_prefix("style:") {
        return set_style(a, p, &v);
    }
    // A `style` string keeps the properties it doesn't set (as the page does).
    let v = match a.iter().find(|(x, _)| k == "style" && *x == "style") {
        Some((_, old)) => {
            let set: Vec<&str> = decls(&v).filter_map(|d| d.split(':').next()).map(str::trim).collect();
            let kept = decls(old).filter(|d| !set.contains(&d.split(':').next().unwrap_or("").trim()));
            decls(&v).chain(kept).collect::<Vec<_>>().join("; ")
        }
        None => v,
    };
    match a.iter_mut().find(|(x, _)| *x == k) {
        Some(x) => x.1 = v,
        None => a.push((k, v)),
    }
}

/// A `style` string's declarations (`k: v`), trimmed.
fn decls(s: &str) -> impl Iterator<Item = &str> {
    s.split(';').map(str::trim).filter(|d| !d.is_empty())
}

/// CSS property `k` of the `style` attribute set to `v` (empty: removed), as `style.setProperty`.
fn set_style(a: &mut Vec<(String, String)>, k: &str, v: &str) {
    let mut style: Vec<String> =
        a.iter().find(|(x, _)| x == "style").map(|(_, s)| decls(s).map(str::to_owned).collect()).unwrap_or_default();
    style.retain(|d| d.split(':').next().map(str::trim) != Some(k));
    if !v.is_empty() {
        style.push(format!("{k}: {v}"));
    }
    a.retain(|(x, _)| x != "style");
    a.push(("style".into(), style.join("; ")));
}

fn read_tnode(r: &mut Rd, t: &mut Tpl, parent: Option<usize>, left: &mut u32) -> usize {
    *left -= 1;
    let (kind, children, mark) = match r.u8() {
        // 5: the same, in SVG's namespace.
        1 | 5 => {
            let tag = r.str();
            let mut attrs = vec![];
            for _ in 0..r.u8() {
                let (k, v) = (r.str(), r.str());
                set_attr(&mut attrs, k, v);
            }
            let children = r.u16();
            (Kind::El(tag, attrs), children, r.u8() != 0)
        }
        2 => (Kind::Text(r.str()), 0, false),
        3 => (Kind::Text(String::new()), 0, true),
        4 => (Kind::Comment, 0, true),
        k => panic!("air: unknown template node {k}"),
    };
    let i = t.nodes.len();
    let mut n = node(kind);
    n.parent = parent;
    t.nodes.push(n);
    // Roots are always marked: their ids are the view's.
    if mark || parent.is_none() {
        t.marks.push(i);
    }
    for _ in 0..children {
        let c = read_tnode(r, t, Some(i), left);
        t.nodes[i].children.push(c);
    }
    i
}

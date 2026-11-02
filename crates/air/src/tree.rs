//! The mounted components (aio DevTools' `tree`, `devtools-tree.ts`; aio-rs's
//! `TestUi::components()`): each component an `rsx!` places, by name and
//! key, with what drew it: how many times its body ran (`renders`: once, or
//! again after a wait or a [`follow`](crate::follow)) and how many times one
//! of its holes wrote its part of the page again (`updates`: air's
//! re-render). Counts cost an increment, as in aio; the tree is walked only
//! when asked. The page's ids of each one's first and last node let a test
//! find what a component drew (`TestUi::find`). A debug build also logs
//! the last [`RENDERS_KEPT`] renders as they happened ([`renders`], aio
//! DevTools' `renders`): each body run and hole rewrite, timed.
use crate::reactive::Frame;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::{Rc, Weak};

/// A component instance's slot: its index and the generation of that index.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Id(u32, u32);

struct Comp {
    /// The root (window) it is in.
    root: u32,
    name: &'static str,
    key: Option<String>,
    parent: Option<Id>,
    seq: u64,
    renders: u32,
    updates: u32,
    nodes: Option<(u32, u32)>,
    /// What its hooks keep (a hot reload hands it to the view built in its place).
    frame: Weak<Frame>,
}

#[derive(Default)]
struct Tree {
    slots: Vec<(u32, Option<Comp>)>,
    free: Vec<u32>,
    seq: u64,
    /// Each root's own view's frame (the app's body), by root.
    roots: HashMap<u32, Weak<Frame>>,
    /// The last renders (debug builds), each with its root; and each
    /// root's count of them all.
    log: VecDeque<(u32, Render)>,
    logged: HashMap<u32, u64>,
}

/// How many renders [`renders`] keeps (aio DevTools' `MAX_RENDER_EVENTS`).
pub const RENDERS_KEPT: usize = 200;

/// One render as it happened ([`renders`]; aio DevTools' `RenderEvent`).
#[derive(Clone, Debug, PartialEq)]
pub struct Render {
    /// The component's name; "" for the root's own view (the app's body).
    pub component: &'static str,
    pub key: Option<String>,
    /// When it ended, in ms since the Unix epoch.
    pub at: u64,
    /// How long it took (a body's run: the children it placed included).
    pub ms: f64,
    /// `"mount"`: its body's first run; `"signal"`: a state change ran its
    /// body again (a wait, a `follow`) or one of its holes.
    pub trigger: &'static str,
}

/// Log a render that started at `t0` (debug builds; the release's views pay nothing).
fn log(id: Option<Id>, t0: std::time::Instant, trigger: &'static str) {
    let ms = t0.elapsed().as_secs_f64() * 1e3;
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    let (component, key) = with(id, |c| (c.name, c.key.clone())).unwrap_or(("", None));
    let root = crate::reactive::current_root();
    TREE.with_borrow_mut(|t| {
        if t.log.len() == RENDERS_KEPT {
            t.log.pop_front();
        }
        t.log.push_back((root, Render { component, key, at, ms, trigger }));
        *t.logged.entry(root).or_default() += 1;
    });
}

/// The time a render starts at, where renders are logged: debug builds,
/// but in a browser's wasm (no clock there: `Instant::now` panics).
pub(crate) fn start() -> Option<std::time::Instant> {
    cfg!(all(debug_assertions, not(target_arch = "wasm32"))).then(std::time::Instant::now)
}

/// Root `root` is gone: its log and its view's frame go too.
pub(crate) fn end_root(root: u32) {
    _ = TREE.try_with(|t| {
        let mut t = t.borrow_mut();
        t.log.retain(|(r, _)| *r != root);
        t.logged.remove(&root);
        t.roots.remove(&root);
    });
}

/// The current root's last renders, oldest first (at most [`RENDERS_KEPT`],
/// fewer where other roots of the thread rendered since), and how many it
/// rendered in all. Logged in debug builds only (not a browser's wasm):
/// a release's is empty.
pub fn renders() -> (Vec<Render>, u64) {
    let root = crate::reactive::current_root();
    TREE.with_borrow(|t| {
        let kept = t.log.iter().filter(|(r, _)| *r == root).map(|(_, e)| e.clone()).collect();
        (kept, t.logged.get(&root).copied().unwrap_or(0))
    })
}

thread_local! {
    static TREE: RefCell<Tree> = RefCell::default();
    /// The component whose view is being built or updated now.
    static CURRENT: Cell<Option<Id>> = const { Cell::new(None) };
}

fn with<R>(id: Option<Id>, f: impl FnOnce(&mut Comp) -> R) -> Option<R> {
    let Id(i, g) = id?;
    TREE.with_borrow_mut(|t| match t.slots.get_mut(i as usize) {
        Some((v, Some(c))) if *v == g => Some(f(c)),
        _ => None,
    })
}

/// A component placed now, under the one whose view is being built.
pub(crate) fn mount(name: &'static str, key: Option<String>) -> Id {
    let parent = CURRENT.get();
    TREE.with_borrow_mut(|t| {
        t.seq += 1;
        let root = crate::reactive::current_root();
        let c = Comp { root, name, key, parent, seq: t.seq, renders: 0, updates: 0, nodes: None, frame: Weak::new() };
        match t.free.pop() {
            Some(i) => {
                let slot = &mut t.slots[i as usize];
                slot.0 = slot.0.wrapping_add(1);
                slot.1 = Some(c);
                Id(i, slot.0)
            }
            None => {
                t.slots.push((0, Some(c)));
                Id(t.slots.len() as u32 - 1, 0)
            }
        }
    })
}

/// Ends the component's entry when its hole ends (kept by the hole).
pub(crate) struct Entry(pub(crate) Id);

impl Drop for Entry {
    fn drop(&mut self) {
        let Id(i, g) = self.0;
        _ = TREE.try_with(|t| {
            let mut t = t.borrow_mut();
            if let Some(slot) = t.slots.get_mut(i as usize).filter(|s| s.0 == g && s.1.is_some()) {
                slot.1 = None;
                t.free.push(i);
            }
        });
    }
}

/// The component whose view is built or updated now.
pub(crate) fn current() -> Option<Id> {
    CURRENT.get()
}

/// Restores the component that was current when dropped.
pub(crate) struct Enter(Option<Id>);

impl Drop for Enter {
    fn drop(&mut self) {
        CURRENT.set(self.0);
    }
}

/// `id` is current until the guard drops: what is built now is its.
pub(crate) fn enter(id: Option<Id>) -> Enter {
    Enter(CURRENT.replace(id))
}

/// Its body ran (from `t0`); its view is the page's nodes `first..=last`.
pub(crate) fn rendered(id: Id, nodes: Option<(u32, u32)>, t0: Option<std::time::Instant>) {
    let first = with(Some(id), |c| {
        c.renders += 1;
        c.nodes = nodes;
        c.renders == 1
    });
    if let (Some(first), Some(t0)) = (first, t0) {
        log(Some(id), t0, if first { "mount" } else { "signal" });
    }
}

/// One of its holes wrote the page again (from `t0`).
pub(crate) fn updated(id: Option<Id>, t0: Option<std::time::Instant>) {
    with(id, |c| c.updates += 1);
    if let Some(t0) = t0 {
        log(id, t0, "signal");
    }
}

/// The frame of component `id` (none: the current root's own view).
pub(crate) fn set_frame(id: Option<Id>, f: &Rc<Frame>) {
    let w = Rc::downgrade(f);
    match id {
        Some(_) => _ = with(id, |c| c.frame = w),
        None => TREE.with_borrow_mut(|t| _ = t.roots.insert(crate::reactive::current_root(), w)),
    }
}

/// The frame of the component whose view is built or updated now (the
/// root's own view's outside any): what its handlers' work belongs to.
pub(crate) fn current_frame() -> Weak<Frame> {
    TREE.with_borrow(|t| match CURRENT.get() {
        Some(Id(i, g)) => match t.slots.get(i as usize) {
            Some((v, Some(c))) if *v == g => c.frame.clone(),
            _ => Weak::new(),
        },
        None => t.roots.get(&crate::reactive::current_root()).cloned().unwrap_or_default(),
    })
}

/// Where component `id` is among the mounted ones (pure but for the tree):
/// each ancestor's name with its key, or its turn among its parent's
/// unkeyed children of that name; the root's own view is "".
pub(crate) fn place(id: Option<Id>) -> String {
    TREE.with_borrow(|t| place_in(t, id))
}

fn place_in(t: &Tree, mut id: Option<Id>) -> String {
    let get = |id: Id| match t.slots.get(id.0 as usize) {
        Some((v, Some(c))) if *v == id.1 => Some(c),
        _ => None,
    };
    let mut parts = vec![];
    while let Some(c) = id.and_then(get) {
        let part = match &c.key {
            Some(k) => format!("{}[{k}]", c.name),
            None => {
                let turn = t
                    .slots
                    .iter()
                    .filter_map(|(_, s)| s.as_ref())
                    .filter(|s| s.root == c.root && s.parent == c.parent && s.name == c.name && s.key.is_none())
                    .filter(|s| s.seq < c.seq)
                    .count();
                format!("{}#{turn}", c.name)
            }
        };
        parts.push(part);
        id = c.parent;
    }
    parts.reverse();
    parts.join("/")
}

/// The frames of root `root`'s mounted views by place (its own view's at "").
pub(crate) fn frames(root: u32) -> HashMap<String, Rc<Frame>> {
    TREE.with_borrow(|t| {
        let mut out: HashMap<String, Rc<Frame>> = t
            .slots
            .iter()
            .enumerate()
            .filter_map(|(i, (g, c))| {
                let c = c.as_ref().filter(|c| c.root == root)?;
                Some((place_in(t, Some(Id(i as u32, *g))), c.frame.upgrade()?))
            })
            .collect();
        if let Some(f) = t.roots.get(&root).and_then(Weak::upgrade) {
            out.insert(String::new(), f);
        }
        out
    })
}

/// One mounted component ([`components`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mounted {
    /// Its function's name (`TodoRow`).
    pub name: &'static str,
    /// Its `key:` in a list, as text.
    pub key: Option<String>,
    /// Times its body ran: once, and again after a wait or a `follow`.
    pub renders: u32,
    /// Times one of its own holes (not a child component's) wrote its part
    /// of the page again: what a state change cost it.
    pub updates: u32,
    /// The page's ids of the first and last node it drew (none: nothing).
    pub nodes: Option<(u32, u32)>,
    pub children: Vec<Mounted>,
}

/// The components mounted in the current root (window), as a tree, each level in the
/// order they were placed.
pub fn components() -> Vec<Mounted> {
    let root = crate::reactive::current_root();
    TREE.with_borrow(|t| {
        let mut all: Vec<(Id, &Comp)> = t
            .slots
            .iter()
            .enumerate()
            .filter_map(|(i, (g, c))| Some((Id(i as u32, *g), c.as_ref().filter(|c| c.root == root)?)))
            .collect();
        all.sort_by_key(|(_, c)| c.seq);
        fn under(all: &[(Id, &Comp)], parent: Option<Id>) -> Vec<Mounted> {
            all.iter()
                .filter(|(_, c)| c.parent == parent)
                .map(|(id, c)| Mounted {
                    name: c.name,
                    key: c.key.clone(),
                    renders: c.renders,
                    updates: c.updates,
                    nodes: c.nodes,
                    children: under(all, Some(*id)),
                })
                .collect()
        }
        under(&all, None)
    })
}

/// A list key as text: by `Display` where it has one, else `Debug`, else
/// none (`(&&KeyOf(&k)).key_text()`: method lookup tries `&KeyOf` first).
#[doc(hidden)]
pub struct KeyOf<'a, T: ?Sized>(pub &'a T);

#[doc(hidden)]
pub trait KeyByDisplay {
    fn key_text(&self) -> Option<String>;
}

impl<T: std::fmt::Display + ?Sized> KeyByDisplay for &KeyOf<'_, T> {
    fn key_text(&self) -> Option<String> {
        Some(self.0.to_string())
    }
}

#[doc(hidden)]
pub trait KeyByDebug {
    fn key_text(&self) -> Option<String>;
}

impl<T: std::fmt::Debug + ?Sized> KeyByDebug for &&KeyOf<'_, T> {
    fn key_text(&self) -> Option<String> {
        Some(format!("{:?}", self.0))
    }
}

#[doc(hidden)]
pub trait KeyByNone {
    fn key_text(&self) -> Option<String>;
}

impl<T: ?Sized> KeyByNone for KeyOf<'_, T> {
    fn key_text(&self) -> Option<String> {
        None
    }
}

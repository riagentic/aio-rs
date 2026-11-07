//! Hot reload: views built again in place with their state kept, after an
//! edit of their markup (`ax dev` sends the edited templates: nothing
//! compiles) or of their code (a patch: the root's body then runs the new
//! code). A view's hooks (`signal`, `memo`, `once`, …) hand back what they
//! made before, by the view's place among the mounted components (its
//! name, its key or its turn among its siblings) and by call order; a
//! view whose code changed its hooks starts afresh from the first that
//! differs. Cells keep their state as always (it is not the views').
//! What the page held (focus, scroll, a field's selection) is drawn anew.
use crate::dom::Template;
use crate::reactive::Frame;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

/// Where a template is written: its `rsx!` call (file as the compiler
/// names it, line and column, 1-based) and its index in that call.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct At {
    pub file: &'static str,
    pub line: u32,
    pub col: u32,
    pub idx: u32,
}

/// A template's markup sent by `ax dev` in place of the built one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Markup {
    /// The `rsx!` call's file as the compiler names it (`src/lib.rs`, or
    /// the path from the workspace's root), line and column (1-based).
    pub file: String,
    pub line: u32,
    pub col: u32,
    /// The template's index in that call (pre-order).
    pub idx: u32,
    /// Its nodes as `rsx!` encodes them, and how many.
    pub nodes: Vec<u8>,
    pub count: u32,
}

pub(crate) struct Override {
    pub nodes: Box<[u8]>,
    pub count: u32,
    /// Which edit: the page defines it anew.
    pub generation: u64,
}

type Key = (String, u32, u32, u32);

static ON: AtomicBool = AtomicBool::new(false);
static GENERATION: AtomicU64 = AtomicU64::new(0);
static MARKUP: RwLock<Option<HashMap<Key, Arc<Override>>>> = RwLock::new(None);

/// The templates of `files` (each as the compiler names it) are `markup`
/// from the next view built on (every thread's), where it names one, else
/// their built ones: [`Root::hot`](crate::Root::hot) builds a root's
/// views again. Other files' markup sent before stays.
pub fn set_markup(files: &[String], markup: Vec<Markup>) {
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    let mut m = MARKUP.write().unwrap_or_else(|e| e.into_inner());
    let all = m.get_or_insert_default();
    let files: Vec<String> = files.iter().map(|f| f.replace('\\', "/")).collect();
    all.retain(|k, _| !files.contains(&k.0));
    for t in markup {
        let o = Override { nodes: t.nodes.into(), count: t.count, generation };
        all.insert((t.file.replace('\\', "/"), t.line, t.col, t.idx), Arc::new(o));
    }
    ON.store(true, Ordering::Release);
}

/// Forget the markup sent: the running code's templates are current again
/// (a patch or a build brought them in).
pub fn clear_markup() {
    ON.store(false, Ordering::Release);
    *MARKUP.write().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The markup sent for `t`, if any (`ax dev`'s, air's `dev` feature: until
/// markup is sent, none; without the feature, never).
pub(crate) fn markup_of(t: &Template) -> Option<Arc<Override>> {
    if !cfg!(feature = "dev") || !ON.load(Ordering::Acquire) {
        return None;
    }
    let m = MARKUP.read().unwrap_or_else(|e| e.into_inner());
    m.as_ref()?.get(&(t.at.file.to_string(), t.at.line, t.at.col, t.at.idx)).cloned()
}

thread_local! {
    /// While a root is built again: the frames of its views before, by place.
    static KEPT: std::cell::RefCell<Option<HashMap<String, Rc<Frame>>>> = const { std::cell::RefCell::new(None) };
}

/// Keep `frames` (each detached from its hole) for the views built next.
pub(crate) fn keep(frames: HashMap<String, Rc<Frame>>) {
    for f in frames.values() {
        f.detach();
    }
    KEPT.set(Some(frames));
}

/// The frame kept for the view at `place`, taken over by it.
pub(crate) fn adopt(place: &str) -> Option<Rc<Frame>> {
    let f = KEPT.with_borrow_mut(|k| k.as_mut()?.remove(place))?;
    f.adopt();
    Some(f)
}

/// The frames no view took: their state ends.
pub(crate) fn end_kept() {
    if let Some(left) = KEPT.take() {
        for f in left.values() {
            f.end();
        }
    }
}

/// Whether a root is being built again now (a place is worth computing).
pub(crate) fn keeping() -> bool {
    KEPT.with_borrow(Option::is_some)
}

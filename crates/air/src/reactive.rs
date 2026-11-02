//! The reactive core: signals, effects and the owners that dispose them.
//! aio's AIR signal core (`aio/src/state/signal.ts`) in Rust: a read inside
//! an effect subscribes it (`_trackStart`/`_trackEnd`), a write that changes
//! the value queues its subscribers, and the queue flushes at once unless a
//! batch is open (`batch` :396). Unlike AIR, nothing re-runs a component:
//! the only subscribers are the holes of a template (`dom`), memos and the
//! app's effects, so a write re-runs exactly the code that read it.
//!
//! One runtime per thread (the window's), handles are `Copy` keys into its
//! arena (generation-checked), so a hole's `move` closure takes a signal
//! without a clone. An effect re-run first disposes what its last run made
//! (child effects, signals, cleanups): AIR's per-render collectors
//! (`_effectCollectStart`) as an owner tree. Owners also carry context
//! (`provide` / `context`), found by walking up to the root.
//!
//! Misuse fails in every build, never only in debug: a read of a signal
//! whose owner ended, a write while a hole or a memo computes (a render
//! writing state), an effect loop (AIR's round guard), a hook outside a
//! view's body.
use std::any::{Any, TypeId};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::Hash;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Key {
    idx: u32,
    ver: u32,
}

/// What a node is: it decides what may happen while it runs.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub(crate) enum Kind {
    /// A scope: owns, runs nothing.
    #[default]
    Owner,
    /// A value.
    Signal,
    /// An app's effect (and a resource's loader): may write.
    Effect,
    /// A template's hole: draws, never writes.
    Render,
    /// A memo's computation: computes, never writes.
    Memo,
}

type Run = Rc<RefCell<dyn FnMut()>>;

/// What few nodes have: context given to the views below, and the owners
/// a re-run keeps (a view's hooks, `Frame`).
#[derive(Default)]
struct Extra {
    ctx: Vec<(TypeId, Rc<dyn Any>)>,
    kept: Vec<Key>,
}

#[derive(Default)]
struct Node {
    ver: u32,
    live: bool,
    kind: Kind,
    /// Creation order: a flush runs owners before what they own.
    seq: u64,
    /// Its owner, for context (none: a root or detached).
    parent: Option<Key>,
    /// A signal's value (`RefCell<T>`), a callback's function.
    value: Option<Rc<dyn Any>>,
    /// A signal's subscribers, with the run they subscribed in (a stale
    /// entry is dropped when next notified: no removal on every re-run).
    subs: Vec<(Key, u32)>,
    /// An effect's body, and its run count.
    run: Option<Run>,
    runs: u32,
    deps: Vec<Key>,
    owned: Vec<Key>,
    cleanups: Vec<Box<dyn FnOnce()>>,
    extra: Option<Box<Extra>>,
}

/// AIR's guard (`signal.ts:418`): this many flush rounds in a row is a loop.
const ROUNDS: u32 = 1000;

#[derive(Default)]
struct Rt {
    nodes: Vec<Node>,
    free: Vec<u32>,
    seq: u64,
    observer: Option<Key>,
    owner: Option<Key>,
    batch: u32,
    queue: Vec<Key>,
    flushing: bool,
    /// Effects whose first run waits for the view to be drawn, and other
    /// work for then (`onmounted`), in the order asked.
    later: Vec<Later>,
    /// The bodies running now, innermost last; `None`: code that is no
    /// body (a hole, a memo, an effect) runs inside one.
    frames: Vec<Option<Rc<Frame>>>,
}

enum Later {
    Effect(Key),
    Call(Box<dyn FnOnce()>),
}

thread_local!(static RT: RefCell<Rt> = RefCell::default());

fn rt<R>(f: impl FnOnce(&mut Rt) -> R) -> R {
    RT.with_borrow_mut(f)
}

impl Rt {
    fn node(&self, k: Key) -> Option<&Node> {
        self.nodes.get(k.idx as usize).filter(|n| n.live && n.ver == k.ver)
    }

    fn node_mut(&mut self, k: Key) -> Option<&mut Node> {
        self.nodes.get_mut(k.idx as usize).filter(|n| n.live && n.ver == k.ver)
    }

    /// A new node, owned by the current owner.
    fn alloc(&mut self, mut n: Node) -> Key {
        self.seq += 1;
        n.seq = self.seq;
        n.live = true;
        n.parent = self.owner;
        let idx = match self.free.pop() {
            Some(i) => {
                n.ver = self.nodes[i as usize].ver.wrapping_add(1);
                self.nodes[i as usize] = n;
                i
            }
            None => {
                self.nodes.push(n);
                self.nodes.len() as u32 - 1
            }
        };
        let k = Key { idx, ver: self.nodes[idx as usize].ver };
        if let Some(o) = self.owner
            && let Some(o) = self.node_mut(o)
        {
            o.owned.push(k);
        }
        k
    }

    fn kind(&self, k: Key) -> Option<Kind> {
        self.node(k).map(|n| n.kind)
    }
}

/// Restores the observer and the owner when dropped (a panic unwinding too).
struct Restore(Option<Key>, Option<Key>);

impl Drop for Restore {
    fn drop(&mut self) {
        rt(|rt| (rt.observer, rt.owner) = (self.0, self.1));
    }
}

fn enter(observer: Option<Key>, owner: Option<Key>) -> Restore {
    rt(|rt| {
        let was = Restore(rt.observer, rt.owner);
        (rt.observer, rt.owner) = (observer, owner);
        was
    })
}

/// Pops a frame (a body, or the barrier of code that is no body) when dropped.
struct PopFrame;

impl Drop for PopFrame {
    fn drop(&mut self) {
        rt(|rt| _ = rt.frames.pop());
    }
}

fn push_frame(f: Option<Rc<Frame>>) -> PopFrame {
    rt(|rt| rt.frames.push(f));
    PopFrame
}

/// Dispose `k` and all it owns: their cleanups run, their slots are freed.
pub(crate) fn dispose(k: Key) {
    let (cleanups, trash) = rt(|rt| {
        let mut stack = vec![k];
        let (mut cleanups, mut trash) = (vec![], vec![]);
        while let Some(k) = stack.pop() {
            let Some(n) = rt.node_mut(k) else { continue };
            n.live = false;
            trash.push((n.value.take(), n.run.take(), n.extra.as_mut().map(|e| std::mem::take(&mut e.ctx))));
            n.subs = vec![];
            n.deps = vec![];
            stack.append(&mut n.owned);
            if let Some(e) = n.extra.take() {
                stack.extend(e.kept);
            }
            cleanups.append(&mut n.cleanups);
            rt.free.push(k.idx);
        }
        (cleanups, trash)
    });
    // Values, bodies and cleanups outside the runtime's borrow (they may touch it).
    drop(trash);
    cleanups.into_iter().rev().for_each(|f| f());
}

/// Dispose what `k`'s last run made, keeping `k` (and what it keeps).
fn reset(k: Key) {
    let (owned, cleanups) = rt(|rt| match rt.node_mut(k) {
        Some(n) => (std::mem::take(&mut n.owned), std::mem::take(&mut n.cleanups)),
        None => (vec![], vec![]),
    });
    owned.into_iter().rev().for_each(dispose);
    cleanups.into_iter().rev().for_each(|f| f());
}

fn track(s: Key) {
    rt(|rt| {
        let Some(e) = rt.observer else { return };
        let Some(n) = rt.node_mut(e) else { return };
        if n.deps.contains(&s) {
            return;
        }
        n.deps.push(s);
        let runs = n.runs;
        let Some(sig) = rt.node_mut(s) else { return };
        sig.subs.push((e, runs));
        // A signal never written keeps its readers' old runs: drop them as it doubles.
        let len = sig.subs.len();
        if len >= 64 && len.is_power_of_two() {
            let subs = std::mem::take(&mut sig.subs);
            let live = subs.into_iter().filter(|(e, r)| rt.node(*e).is_some_and(|n| n.runs == *r)).collect();
            if let Some(sig) = rt.node_mut(s) {
                sig.subs = live;
            }
        }
    });
}

/// Tell `s`'s readers it changed. `checked`: an app's write, refused
/// while a hole or a memo computes (the runtime's own writes are not).
fn notify(s: Key, checked: bool) {
    let flush = rt(|rt| {
        if checked
            && let Some(o) = rt.observer
            && let Some(k @ (Kind::Render | Kind::Memo)) = rt.kind(o)
        {
            let what = if k == Kind::Memo { "a memo" } else { "a view draws (a hole of rsx!)" };
            return Err(what);
        }
        let subs = match rt.node_mut(s) {
            Some(n) => std::mem::take(&mut n.subs),
            None => return Ok(false),
        };
        // Only subscriptions of an effect's current run stay.
        let live: Vec<(Key, u32)> =
            subs.into_iter().filter(|(e, r)| rt.node(*e).is_some_and(|n| n.runs == *r)).collect();
        rt.queue.extend(live.iter().map(|(e, _)| *e));
        if let Some(n) = rt.node_mut(s) {
            // Re-subscribed by the re-run; keep them so a write in a batch
            // before the flush still reaches them.
            n.subs = live;
        }
        Ok(rt.batch == 0 && !rt.flushing)
    });
    match flush {
        Err(what) => panic!(
            "air: a signal was written while {what} computes: drawing never writes state (write it in a handler, \
             an effect or a task, or compute it with a memo)"
        ),
        Ok(true) => drain(),
        Ok(false) => {}
    }
}

/// Run queued effects until none is left, owners first; one queued twice
/// runs once (AIR `_flushSubscribers`); then the effects and calls that
/// wait for the view to be drawn; until nothing is left of either.
fn drain() {
    loop {
        run_queue();
        let later = rt(|rt| std::mem::take(&mut rt.later));
        if later.is_empty() {
            break;
        }
        rt(|rt| rt.batch += 1);
        struct Unbatch;
        impl Drop for Unbatch {
            fn drop(&mut self) {
                rt(|rt| rt.batch -= 1);
            }
        }
        let _u = Unbatch;
        for l in later {
            match l {
                Later::Effect(e) => {
                    run_effect(e);
                    drop_if_idle(e);
                }
                Later::Call(f) => f(),
            }
        }
    }
}

fn run_queue() {
    struct Unflush;
    impl Drop for Unflush {
        fn drop(&mut self) {
            rt(|rt| rt.flushing = false);
        }
    }
    if rt(|rt| std::mem::replace(&mut rt.flushing, true)) {
        return;
    }
    let _u = Unflush;
    for round in 0.. {
        let next = rt(|rt| {
            if rt.queue.is_empty() {
                return None;
            }
            let mut q = std::mem::take(&mut rt.queue);
            q.sort_by_key(|k| rt.node(*k).map_or(0, |n| n.seq));
            q.dedup();
            Some(q)
        });
        let Some(q) = next else { break };
        if round >= ROUNDS {
            rt(|rt| rt.queue.clear());
            panic!(
                "air: effects still re-running after {ROUNDS} rounds: an effect writes a signal it reads, on and on \
                 (an effect loop)"
            );
        }
        for e in q {
            run_effect(e);
        }
    }
}

fn run_effect(e: Key) {
    reset(e);
    let body = rt(|rt| {
        let n = rt.node_mut(e)?;
        n.runs += 1;
        n.deps.clear();
        n.run.clone()
    });
    let Some(body) = body else { return };
    let _f = push_frame(None);
    let _r = enter(Some(e), Some(e));
    (body.borrow_mut())();
}

/// An effect that read nothing and made, kept and provided nothing is
/// dropped after its first run: a static hole costs nothing later.
fn drop_if_idle(e: Key) {
    let idle = rt(|rt| {
        rt.node(e)
            .is_some_and(|n| n.deps.is_empty() && n.owned.is_empty() && n.cleanups.is_empty() && n.extra.is_none())
    });
    if idle {
        dispose(e);
    }
}

/// A node of `kind` that runs `f` now and again whenever what it read
/// changes, owned by the current owner.
pub(crate) fn effect_of(kind: Kind, f: impl FnMut() + 'static) -> Key {
    let e = rt(|rt| rt.alloc(Node { kind, run: Some(Rc::new(RefCell::new(f))), ..Node::default() }));
    // In a batch: what its first run writes waits for it to end.
    batch(|| run_effect(e));
    drop_if_idle(e);
    e
}

/// `f()`, and whether it read a signal (a probe: nothing stays subscribed;
/// what `f` makes is the current owner's).
pub(crate) fn reads<R>(f: impl FnOnce() -> R) -> (R, bool) {
    let (k, owner) = rt(|rt| {
        let owner = rt.owner.take();
        let k = rt.alloc(Node { kind: Kind::Memo, ..Node::default() });
        rt.owner = owner;
        (k, owner)
    });
    let r = {
        let _r = enter(Some(k), owner);
        f()
    };
    let read = RT.with_borrow(|rt| rt.node(k).is_some_and(|n| !n.deps.is_empty()));
    dispose(k);
    (r, read)
}

/// A hole: `f` now and whenever what it read changes; it may not write.
pub(crate) fn render(f: impl FnMut() + 'static) {
    effect_of(Kind::Render, f);
}

/// `f` once the view being built is drawn (its edits out, before the
/// page applies them): `onmounted`, a hook's first effect run.
pub(crate) fn later(f: impl FnOnce() + 'static) {
    rt(|rt| rt.later.push(Later::Call(Box::new(f))));
    flush_if_idle();
}

fn flush_if_idle() {
    if rt(|rt| rt.batch == 0 && !rt.flushing) {
        drain();
    }
}

/// An owner of its own for what `f` makes (a row, a window), untracked;
/// [`Owner::dispose`] ends all of it.
pub fn scope<R>(f: impl FnOnce() -> R) -> (R, Owner) {
    let k = rt(|rt| rt.alloc(Node::default()));
    let r = with_owner(Owner(k), f);
    (r, Owner(k))
}

/// An owner for what `f` makes whose parent is the current owner (its
/// context), but which no one disposes but the caller: a list's row.
pub(crate) fn child_scope<R>(f: impl FnOnce() -> R) -> (R, Owner) {
    let k = rt(|rt| match rt.owner {
        Some(p) => rt.alloc_detached(p),
        None => rt.alloc(Node::default()),
    });
    let r = with_owner(Owner(k), f);
    (r, Owner(k))
}

/// An owner: what was made under it lives until it is disposed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Owner(pub(crate) Key);

impl Owner {
    /// End it and all it made (cleanups run, signals end, holes stop).
    pub fn dispose(self) {
        dispose(self.0)
    }

    /// Is it still there?
    pub fn alive(self) -> bool {
        RT.with_borrow(|rt| rt.node(self.0).is_some())
    }
}

/// The owner things made now belong to.
pub fn current_owner() -> Option<Owner> {
    RT.with_borrow(|rt| rt.owner.map(Owner))
}

/// `f` under `owner`, untracked.
pub fn with_owner<R>(owner: Owner, f: impl FnOnce() -> R) -> R {
    let _r = enter(None, Some(owner.0));
    f()
}

/// `f` under no owner, untracked: what it makes lives for the thread.
pub fn detached<R>(f: impl FnOnce() -> R) -> R {
    let _r = enter(None, None);
    f()
}

/// `f` when the current owner is disposed or re-runs (AIR `onCleanup`);
/// nothing to run it with no owner.
pub(crate) fn cleanup(f: impl FnOnce() + 'static) {
    rt(|rt| {
        if let Some(o) = rt.owner
            && let Some(n) = rt.node_mut(o)
        {
            n.cleanups.push(Box::new(f));
        }
    });
}

/// Writes in `f` flush once, at its end (AIR `batch`; every event handler runs in one).
pub fn batch<R>(f: impl FnOnce() -> R) -> R {
    struct Close;
    impl Drop for Close {
        fn drop(&mut self) {
            rt(|rt| rt.batch -= 1);
        }
    }
    rt(|rt| rt.batch += 1);
    let r = {
        let _c = Close;
        f()
    };
    flush_if_idle();
    r
}

/// `f`'s reads subscribe nothing (AIR `untrack`).
pub fn untrack<R>(f: impl FnOnce() -> R) -> R {
    let o = rt(|rt| rt.observer.take());
    struct Back(Option<Key>);
    impl Drop for Back {
        fn drop(&mut self) {
            rt(|rt| rt.observer = self.0);
        }
    }
    let _b = Back(o);
    f()
}

/// Is code of the app's views running on this thread now (a view's body,
/// a hole, a handler, an effect, a task, or a test's `within` its root)?
pub fn in_views() -> bool {
    RT.with_borrow(|rt| rt.owner.is_some())
}

// ---------------------------------------------------------------- context

fn ctx_of<T: 'static>(rt: &Rt, from: Option<Key>) -> Option<Rc<dyn Any>> {
    let mut at = from;
    while let Some(k) = at {
        let n = rt.node(k)?;
        if let Some(c) = n.extra.as_ref().and_then(|e| e.ctx.iter().find(|(t, _)| *t == TypeId::of::<T>())) {
            return Some(c.1.clone());
        }
        at = n.parent;
    }
    None
}

/// Give `v` to the views below owner `at` (replacing one of its type).
pub(crate) fn provide_on<T: Clone + 'static>(at: Owner, v: T) {
    rt(|rt| {
        let Some(n) = rt.node_mut(at.0) else { return };
        let ctx = &mut n.extra.get_or_insert_default().ctx;
        let v: Rc<dyn Any> = Rc::new(v);
        match ctx.iter_mut().find(|(t, _)| *t == TypeId::of::<T>()) {
            Some(c) => c.1 = v,
            None => ctx.push((TypeId::of::<T>(), v)),
        }
    });
}

/// What a view above provided, if one did (in a view, a handler, an effect).
pub fn try_context<T: Clone + 'static>() -> Option<T> {
    let found = RT.with_borrow(|rt| ctx_of::<T>(rt, rt.owner));
    found.and_then(|c| c.downcast_ref::<T>().cloned())
}

/// What a view above provided ([`provide`]); panics if none did.
#[track_caller]
pub fn context<T: Clone + 'static>() -> T {
    try_context().unwrap_or_else(|| {
        panic!(
            "air: no {} was provided above this view (provide it in a view above, or use try_context)",
            std::any::type_name::<T>()
        )
    })
}

/// What the views above the current owner provided, skipping its own.
#[doc(hidden)]
pub fn parent_context<T: Clone + 'static>() -> Option<T> {
    let found = RT.with_borrow(|rt| {
        let up = rt.owner.and_then(|o| rt.node(o)).and_then(|n| n.parent);
        ctx_of::<T>(rt, up)
    });
    found.and_then(|c| c.downcast_ref::<T>().cloned())
}

// ------------------------------------------------------------------ hooks

/// A view's body as it runs: what its hooks keep, by call order, so a
/// body run again (it waited for a resource) gets the same state back.
pub(crate) struct Frame {
    /// The view's owner (its hole): context goes there.
    hole: Key,
    slots: RefCell<Vec<Rc<dyn Any>>>,
    at: Cell<usize>,
    /// Owns what the hooks made; kept by the hole across its re-runs.
    keeper: Cell<Option<Key>>,
}

impl Frame {
    pub(crate) fn new() -> Rc<Self> {
        let hole = RT.with_borrow(|rt| rt.owner).expect("air: a view's frame is made under its hole");
        Rc::new(Self { hole, slots: RefCell::default(), at: Cell::new(0), keeper: Cell::new(None) })
    }

    /// The owner its hooks' state lives under.
    fn keeper(&self) -> Key {
        if let Some(k) = self.keeper.get() {
            return k;
        }
        let k = rt(|rt| {
            let k = rt.alloc_detached(self.hole);
            if let Some(h) = rt.node_mut(self.hole) {
                h.extra.get_or_insert_default().kept.push(k);
            }
            k
        });
        self.keeper.set(Some(k));
        k
    }
}

impl Rt {
    /// An owner node whose parent is `parent` but which no one's `owned` lists.
    fn alloc_detached(&mut self, parent: Key) -> Key {
        let was = self.owner.take();
        let k = self.alloc(Node::default());
        self.owner = was;
        if let Some(n) = self.node_mut(k) {
            n.parent = Some(parent);
        }
        k
    }
}

/// Run `body` as a view's body (its hooks in `frame`), untracked.
pub(crate) fn body<R>(frame: &Rc<Frame>, f: impl FnOnce() -> R) -> R {
    frame.at.set(0);
    let _p = push_frame(Some(frame.clone()));
    untrack(f)
}

fn frame(hook: &str) -> Rc<Frame> {
    let f = RT.with_borrow(|rt| rt.frames.last().cloned().flatten());
    f.unwrap_or_else(|| {
        panic!(
            "air: `{hook}` keeps something for a view, so it is called in a view's body (a component, the app's \
             root), at its top level; here no view's body runs (a handler, an effect, a hole, a loop's row or code \
             outside the views): make it in the view, or use Signal::new / spawn there"
        )
    })
}

/// `f()` the first time this view's body runs, kept for its life; later
/// runs get a clone (a view runs once, unless it waited for a resource).
/// What `f` makes lives as long as the view. The other hooks are made of it.
#[track_caller]
pub fn once<T: Clone + 'static>(f: impl FnOnce() -> T) -> T {
    hook("once", f)
}

pub(crate) fn hook<T: Clone + 'static>(name: &str, f: impl FnOnce() -> T) -> T {
    let fr = frame(name);
    let i = fr.at.get();
    fr.at.set(i + 1);
    if let Some(v) = fr.slots.borrow().get(i) {
        return v.downcast_ref::<T>().cloned().unwrap_or_else(|| {
            panic!("air: this view called its hooks in another order than before (`{name}` at place {i}): call them at the top of the body, never in a branch or a loop")
        });
    }
    let k = fr.keeper();
    let v = with_owner(Owner(k), f);
    fr.slots.borrow_mut().push(Rc::new(v.clone()));
    v
}

/// Is a view's body running now (not a hole, a handler, an effect)?
#[doc(hidden)]
pub fn in_body() -> bool {
    RT.with_borrow(|rt| rt.frames.last().is_some_and(Option::is_some))
}

/// The owner of the view whose body runs now (its hole), for `provide`.
pub(crate) fn view_owner(hook: &str) -> Owner {
    Owner(frame(hook).hole)
}

// ---------------------------------------------------------------- signals

/// A value that tells the views and effects that read it when it changes.
/// `Copy`: a handle into the thread's runtime, freed with its owner, so a
/// closure takes it without a clone. Reads in a hole, a memo or an effect
/// subscribe it (`get`, `with`, `read`, `"{count}"`); the `_untracked`
/// reads don't.
pub struct Signal<T> {
    key: Key,
    _t: PhantomData<fn() -> T>,
}

impl<T> Clone for Signal<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Signal<T> {}

impl<T> PartialEq for Signal<T> {
    /// The same signal (not the same value).
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}
impl<T> Eq for Signal<T> {}

/// A read guard: the value, borrowed while it lives.
pub struct ReadGuard<T: 'static> {
    cell: Rc<dyn Any>,
    // Borrowed from `cell`, which this guard keeps alive.
    guard: Option<std::cell::Ref<'static, T>>,
}

impl<T: 'static> Deref for ReadGuard<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.guard.as_ref().expect("air: a live guard")
    }
}

impl<T: 'static> Drop for ReadGuard<T> {
    fn drop(&mut self) {
        self.guard.take();
        let _ = &self.cell;
    }
}

/// A write guard: readers are told when it drops.
pub struct WriteGuard<T: 'static> {
    key: Key,
    cell: Rc<dyn Any>,
    guard: Option<std::cell::RefMut<'static, T>>,
}

impl<T: 'static> Deref for WriteGuard<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.guard.as_ref().expect("air: a live guard")
    }
}

impl<T: 'static> DerefMut for WriteGuard<T> {
    fn deref_mut(&mut self) -> &mut T {
        self.guard.as_mut().expect("air: a live guard")
    }
}

impl<T: 'static> Drop for WriteGuard<T> {
    fn drop(&mut self) {
        self.guard.take();
        let _ = &self.cell;
        notify(self.key, !std::thread::panicking());
    }
}

fn typed<T: 'static>(c: &Rc<dyn Any>) -> &RefCell<T> {
    c.downcast_ref::<RefCell<T>>().expect("air: a signal of another type")
}

impl<T: 'static> Signal<T> {
    /// A signal owned by the current owner (a view's, or none: the thread's).
    /// In a view's body, [`signal`] keeps it across a re-run.
    pub fn new(v: T) -> Self {
        let value: Rc<dyn Any> = Rc::new(RefCell::new(v));
        let key = rt(|rt| rt.alloc(Node { kind: Kind::Signal, value: Some(value), ..Node::default() }));
        Self { key, _t: PhantomData }
    }

    fn try_cell(&self) -> Option<Rc<dyn Any>> {
        RT.with_borrow(|rt| rt.node(self.key).and_then(|n| n.value.clone()))
    }

    #[track_caller]
    fn cell(&self) -> Rc<dyn Any> {
        self.try_cell().unwrap_or_else(|| {
            panic!(
                "air: a signal used after the view that made it ended (ask `alive()` or use try_write / \
                 try_read_untracked in work that may outlive its view)"
            )
        })
    }

    /// Is its value still there? A signal ends with the view that made it.
    pub fn alive(&self) -> bool {
        self.try_cell().is_some()
    }

    /// `f` of the value; subscribes.
    #[track_caller]
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        track(self.key);
        self.with_untracked(f)
    }

    /// `f` of the value, subscribing nothing.
    #[track_caller]
    pub fn with_untracked<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        let c = self.cell();
        let v = typed::<T>(&c).try_borrow().expect("air: a signal read while it is written (a write guard is held)");
        f(&v)
    }

    /// The value, cloned; subscribes.
    #[track_caller]
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.with(T::clone)
    }

    /// The value, cloned, subscribing nothing.
    #[track_caller]
    pub fn get_untracked(&self) -> T
    where
        T: Clone,
    {
        self.with_untracked(T::clone)
    }

    /// The value, borrowed until the guard drops; subscribes.
    #[track_caller]
    pub fn read(&self) -> ReadGuard<T> {
        track(self.key);
        self.read_untracked()
    }

    /// The value, borrowed, subscribing nothing.
    #[track_caller]
    pub fn read_untracked(&self) -> ReadGuard<T> {
        self.try_read_untracked().expect("air: a signal read after its view ended, or while it is written")
    }

    /// [`read_untracked`](Self::read_untracked), or `None` once the signal
    /// ended (or while it is written).
    pub fn try_read_untracked(&self) -> Option<ReadGuard<T>> {
        let cell = self.try_cell()?;
        let r = typed::<T>(&cell).try_borrow().ok()?;
        // SAFETY: the borrow is of `cell`'s RefCell, which the guard keeps
        // alive (an `Rc`) and drops only after the borrow (`Drop`).
        let guard = unsafe { std::mem::transmute::<std::cell::Ref<'_, T>, std::cell::Ref<'static, T>>(r) };
        Some(ReadGuard { cell, guard: Some(guard) })
    }

    /// The value, mutably borrowed; readers are told when the guard drops.
    #[track_caller]
    pub fn write(&self) -> WriteGuard<T> {
        self.try_write().expect("air: a signal written after its view ended, or while it is borrowed")
    }

    /// [`write`](Self::write), or `None` once the signal ended (or while it
    /// is borrowed): for work that may outlive its view.
    pub fn try_write(&self) -> Option<WriteGuard<T>> {
        let cell = self.try_cell()?;
        let w = typed::<T>(&cell).try_borrow_mut().ok()?;
        // SAFETY: as in `try_read_untracked`.
        let guard = unsafe { std::mem::transmute::<std::cell::RefMut<'_, T>, std::cell::RefMut<'static, T>>(w) };
        Some(WriteGuard { key: self.key, cell, guard: Some(guard) })
    }

    /// Change the value in place; readers are always told.
    #[track_caller]
    pub fn update<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        let r = {
            let c = self.cell();
            let mut v =
                typed::<T>(&c).try_borrow_mut().expect("air: a signal written while it is read (a read guard is held)");
            f(&mut v)
        };
        notify(self.key, true);
        r
    }

    /// The new value; readers are told only if it differs (AIR `set`'s
    /// `Object.is`): setting what is there already redraws nothing.
    #[track_caller]
    pub fn set(&self, v: T)
    where
        T: PartialEq,
    {
        if self.replace(v) {
            notify(self.key, true);
        }
    }

    /// The runtime's own write (a memo's value, a resource's state): no render check.
    pub(crate) fn set_quiet(&self, v: T)
    where
        T: PartialEq,
    {
        if self.try_cell().is_some() && self.replace(v) {
            notify(self.key, false);
        }
    }

    #[track_caller]
    fn replace(&self, v: T) -> bool
    where
        T: PartialEq,
    {
        let c = self.cell();
        let mut cur =
            typed::<T>(&c).try_borrow_mut().expect("air: a signal written while it is read (a guard is held)");
        let changed = *cur != v;
        if changed {
            *cur = v;
        }
        changed
    }

    /// Subscribe the running code to it without reading it.
    pub(crate) fn track(&self) {
        track(self.key);
    }
}

impl Signal<bool> {
    /// Flip it; readers are told.
    #[track_caller]
    pub fn toggle(&self) {
        self.update(|v| *v = !*v)
    }
}

impl<T: std::fmt::Display + 'static> std::fmt::Display for Signal<T> {
    /// Subscribes: `"{count}"` in a view follows `count`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.with(|v| v.fmt(f))
    }
}

impl<T: std::fmt::Debug + 'static> std::fmt::Debug for Signal<T> {
    /// Subscribes too: `"{pick:?}"` in a view follows `pick`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.with(|v| v.fmt(f))
    }
}

impl<T: Clone + std::ops::Add<Output = T> + 'static> std::ops::AddAssign<T> for Signal<T> {
    /// `count += 1` in a handler.
    fn add_assign(&mut self, by: T) {
        self.update(|v| *v = v.clone() + by)
    }
}

impl<T: Clone + std::ops::Sub<Output = T> + 'static> std::ops::SubAssign<T> for Signal<T> {
    fn sub_assign(&mut self, by: T) {
        self.update(|v| *v = v.clone() - by)
    }
}

/// A view's own state, made the first time its body runs: `let n = signal(|| 0);`.
#[track_caller]
pub fn signal<T: 'static>(init: impl FnOnce() -> T) -> Signal<T> {
    hook("signal", || Signal::new(init()))
}

// ------------------------------------------------------------------ memos

/// A value made of signals and cell parts, made again when one it read
/// changes; its readers are told only when the value differs.
pub struct Memo<T> {
    out: Signal<T>,
}

impl<T> Clone for Memo<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Memo<T> {}

impl<T> PartialEq for Memo<T> {
    fn eq(&self, other: &Self) -> bool {
        self.out == other.out
    }
}

impl<T: 'static> Memo<T> {
    /// The value, cloned; subscribes.
    #[track_caller]
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.out.get()
    }

    #[track_caller]
    pub fn get_untracked(&self) -> T
    where
        T: Clone,
    {
        self.out.get_untracked()
    }

    #[track_caller]
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.out.with(f)
    }

    #[track_caller]
    pub fn with_untracked<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.out.with_untracked(f)
    }

    #[track_caller]
    pub fn read(&self) -> ReadGuard<T> {
        self.out.read()
    }

    #[track_caller]
    pub fn read_untracked(&self) -> ReadGuard<T> {
        self.out.read_untracked()
    }

    pub fn alive(&self) -> bool {
        self.out.alive()
    }
}

impl<T: std::fmt::Display + 'static> std::fmt::Display for Memo<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.out.fmt(f)
    }
}

impl<T: std::fmt::Debug + 'static> std::fmt::Debug for Memo<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.out.fmt(f)
    }
}

/// A memo made now, under the current owner (not a hook).
pub(crate) fn memo_now<T: PartialEq + 'static>(mut f: impl FnMut() -> T + 'static) -> Memo<T> {
    let out: Rc<Cell<Option<Signal<T>>>> = Rc::default();
    let owner = current_owner();
    let o = out.clone();
    effect_of(Kind::Memo, move || {
        let v = f();
        match o.get() {
            Some(s) => s.set_quiet(v),
            // Made by the memo's owner, not this effect: a re-run keeps it.
            None => o.set(Some(match owner {
                Some(w) => with_owner(w, || Signal::new(v)),
                None => detached(|| Signal::new(v)),
            })),
        }
    });
    Memo { out: out.get().expect("air: a memo ran") }
}

/// `f`'s value, made again when what it read changes (a hook).
#[track_caller]
pub fn memo<T: PartialEq + 'static>(f: impl FnMut() -> T + 'static) -> Memo<T> {
    hook("memo", || memo_now(f))
}

// ---------------------------------------------------------------- effects

/// An app's effect made now, its first run once the view is drawn.
pub(crate) fn effect_now(f: impl FnMut() + 'static) {
    let e = rt(|rt| rt.alloc(Node { kind: Kind::Effect, run: Some(Rc::new(RefCell::new(f))), ..Node::default() }));
    rt(|rt| rt.later.push(Later::Effect(e)));
    flush_if_idle();
}

/// Run `f` after the view is drawn, and again whenever what it read changes (a hook).
#[track_caller]
pub fn effect(f: impl FnMut() + 'static) {
    hook("effect", || effect_now(f))
}

/// Run `f(deps)` once the view is drawn (a hook). A view runs once, so
/// `deps` are the values it was made with; what must be followed is read
/// from a signal or a cell part inside `f`, as in [`effect`].
#[track_caller]
pub fn watch<D: Clone + PartialEq + 'static>(deps: D, mut f: impl FnMut(D) + 'static) {
    hook("watch", move || effect_now(move || f(deps.clone())))
}

/// Run `f` when the view goes away (a hook).
#[track_caller]
pub fn on_cleanup(f: impl FnOnce() + 'static) {
    hook("on_cleanup", || cleanup(f))
}

/// Give the views below this one `f()`, made once; they read it with
/// [`context`]. Returns it (a hook).
#[track_caller]
pub fn provide<T: Clone + 'static>(f: impl FnOnce() -> T) -> T {
    let at = view_owner("provide");
    hook("provide", move || {
        let v = f();
        provide_on(at, v.clone());
        v
    })
}

/// Give every view of the app `v` (the root's context), now.
pub fn provide_root<T: Clone + 'static>(v: T) -> T {
    let root = RT.with_borrow(|rt| {
        let mut at = rt.owner?;
        while let Some(p) = rt.node(at).and_then(|n| n.parent) {
            at = p;
        }
        Some(at)
    });
    match root {
        Some(r) => provide_on(Owner(r), v.clone()),
        None => panic!("air: provide_root is called in the app's views (no view runs here)"),
    }
    v
}

// -------------------------------------------------------------- callbacks

/// A function a view hands down (a prop's handler): `Copy`, made once
/// ([`callback`]) or from a closure (`Into`), ended with its owner.
pub struct Callback<A = (), R = ()> {
    key: Key,
    _t: PhantomData<fn(A) -> R>,
}

/// A handler prop: `EventHandler<MouseEvent>`.
pub type EventHandler<T = ()> = Callback<T, ()>;

impl<A, R> Clone for Callback<A, R> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<A, R> Copy for Callback<A, R> {}

impl<A, R> PartialEq for Callback<A, R> {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

type CallFn<A, R> = RefCell<Box<dyn FnMut(A) -> R>>;

impl<A: 'static, R: 'static> Callback<A, R> {
    /// One owned by the current owner.
    pub fn new(f: impl FnMut(A) -> R + 'static) -> Self {
        let f: Rc<dyn Any> = Rc::new(RefCell::new(Box::new(f) as Box<dyn FnMut(A) -> R>));
        let key = rt(|rt| rt.alloc(Node { kind: Kind::Signal, value: Some(f), ..Node::default() }));
        Self { key, _t: PhantomData }
    }

    /// Call it (in a batch: what it writes is drawn once).
    #[track_caller]
    pub fn call(&self, a: A) -> R {
        let f = RT
            .with_borrow(|rt| rt.node(self.key).and_then(|n| n.value.clone()))
            .unwrap_or_else(|| panic!("air: a callback called after the view that made it ended"));
        let f = f.downcast_ref::<CallFn<A, R>>().expect("air: a callback of another type");
        let mut f = f.try_borrow_mut().expect("air: a callback called from inside itself");
        batch(|| f(a))
    }
}

impl<A: 'static, R: Default + 'static> Default for Callback<A, R> {
    /// One that does nothing (answers `R::default()`): a handler prop left out.
    fn default() -> Self {
        Self::new(|_| R::default())
    }
}

impl<A: 'static, R: 'static, F: FnMut(A) -> R + 'static> From<F> for Callback<A, R> {
    fn from(f: F) -> Self {
        Self::new(f)
    }
}

/// A handler made once (a hook), so a prop that takes it keeps it.
#[track_caller]
pub fn callback<A: 'static, R: 'static>(f: impl FnMut(A) -> R + 'static) -> Callback<A, R> {
    hook("callback", || Callback::new(f))
}

// ---------------------------------------------------------------- selector

/// Which key is selected, told per key (Solid's `createSelector`): a row
/// reading `sel.is(k)` re-runs only when its own answer flips, so moving a
/// cursor over 1000 rows re-runs two holes, not 1000.
pub struct Selector<K> {
    inner: Rc<SelectorInner<K>>,
}

struct SelectorInner<K> {
    current: RefCell<Option<K>>,
    buckets: RefCell<HashMap<K, Signal<bool>>>,
    owner: Option<Owner>,
}

impl<K> Clone for Selector<K> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

impl<K> PartialEq for Selector<K> {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.inner, &other.inner)
    }
}

/// A selector made now, under the current owner (not a hook).
pub fn selector<K: Hash + Eq + Clone + 'static>(src: impl Fn() -> K + 'static) -> Selector<K> {
    let inner =
        Rc::new(SelectorInner { current: RefCell::new(None), buckets: RefCell::default(), owner: current_owner() });
    let i = inner.clone();
    effect_of(Kind::Memo, move || {
        let k = src();
        let old = i.current.borrow_mut().replace(k.clone());
        if old.as_ref() == Some(&k) {
            return;
        }
        let set = |key: &K, on: bool| {
            let b = i.buckets.borrow().get(key).copied();
            if let Some(b) = b {
                b.set_quiet(on)
            }
        };
        untrack(|| {
            batch(|| {
                if let Some(o) = &old {
                    set(o, false)
                }
                set(&k, true)
            })
        });
    });
    Selector { inner }
}

impl<K: Hash + Eq + Clone + 'static> Selector<K> {
    /// Is `k` the selected key? Tracked per key.
    pub fn is(&self, k: K) -> bool {
        let b = self.inner.buckets.borrow().get(&k).copied();
        let b = b.unwrap_or_else(|| {
            let on = self.inner.current.borrow().as_ref() == Some(&k);
            let make = || Signal::new(on);
            let s = match self.inner.owner {
                Some(o) => with_owner(o, make),
                None => detached(make),
            };
            self.inner.buckets.borrow_mut().insert(k, s);
            s
        });
        b.get()
    }
}

// ----------------------------------------------------------------- counts

/// What is alive on this thread's runtime: a leak shows here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Live {
    /// Signals, effects, holes, memos, owners, callbacks.
    pub nodes: usize,
    /// Of them, the ones that run (effects, holes, memos).
    pub effects: usize,
    /// Subscriptions that still reach a running effect.
    pub subscriptions: usize,
    /// Cleanups waiting for their owner.
    pub cleanups: usize,
}

/// What is alive on this thread's runtime now.
pub fn live() -> Live {
    RT.with_borrow(|rt| {
        let mut l = Live::default();
        for n in rt.nodes.iter().filter(|n| n.live) {
            l.nodes += 1;
            l.effects += n.run.is_some() as usize;
            l.cleanups += n.cleanups.len();
            l.subscriptions += n.subs.iter().filter(|(e, r)| rt.node(*e).is_some_and(|n| n.runs == *r)).count();
        }
        l
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_effect_reruns_only_for_what_it_read() {
        let ((a, b, runs), o) = scope(|| {
            let (a, b) = (Signal::new(1), Signal::new(10));
            let runs = Rc::new(Cell::new(0));
            let r = runs.clone();
            effect_of(Kind::Effect, move || {
                a.get();
                r.set(r.get() + 1);
            });
            (a, b, runs)
        });
        assert_eq!(runs.get(), 1);
        b.set(11);
        assert_eq!(runs.get(), 1, "b was never read");
        a.set(1);
        assert_eq!(runs.get(), 1, "the same value tells no one");
        a.set(2);
        assert_eq!(runs.get(), 2);
        batch(|| {
            a.set(3);
            a.set(4);
        });
        assert_eq!(runs.get(), 3, "a batch flushes once");
        o.dispose();
        assert_eq!(live(), Live::default(), "an owner frees all it made");
    }

    #[test]
    fn a_rerun_disposes_what_the_last_run_made() {
        let (s, o) = scope(|| {
            let s = Signal::new(0);
            effect_of(Kind::Effect, move || {
                s.get();
                effect_of(Kind::Effect, move || {}); // dropped at once: it reads nothing
                Signal::new(s.get_untracked());
            });
            s
        });
        let before = live();
        s.set(1);
        s.set(2);
        assert_eq!(live(), before, "each run's signal replaced the last");
        o.dispose();
        assert_eq!(live(), Live::default());
    }

    #[test]
    fn a_selector_reruns_only_the_rows_that_flip() {
        let ((cur, runs), o) = scope(|| {
            let cur = Signal::new(0usize);
            let sel = selector(move || cur.get());
            let runs = Rc::new(Cell::new(0));
            for i in 0..100 {
                let (sel, r) = (sel.clone(), runs.clone());
                render(move || {
                    sel.is(i);
                    r.set(r.get() + 1);
                });
            }
            (cur, runs)
        });
        assert_eq!(runs.get(), 100);
        cur.set(1);
        assert_eq!(runs.get(), 102, "row 0 off, row 1 on");
        o.dispose();
        assert_eq!(live(), Live::default());
    }

    #[test]
    fn a_memo_tells_only_a_changed_answer() {
        let ((s, runs), o) = scope(|| {
            let s = Signal::new(1);
            let even = memo_now(move || s.get() % 2 == 0);
            let runs = Rc::new(Cell::new(0));
            let r = runs.clone();
            render(move || {
                even.get();
                r.set(r.get() + 1);
            });
            (s, runs)
        });
        s.set(3);
        assert_eq!(runs.get(), 1, "still odd");
        s.set(4);
        assert_eq!(runs.get(), 2);
        o.dispose();
        assert_eq!(live(), Live::default());
    }

    #[test]
    fn context_is_found_up_the_owners_and_the_nearest_wins() {
        let ((), o) = scope(|| {
            provide_on(current_owner().unwrap(), 1u8);
            let ((), inner) = scope(|| {
                assert_eq!(try_context::<u8>(), Some(1), "from the owner above");
                provide_on(current_owner().unwrap(), 2u8);
                assert_eq!(context::<u8>(), 2, "its own is nearer");
                assert_eq!(parent_context::<u8>(), Some(1), "skipping its own");
                assert_eq!(try_context::<u16>(), None);
            });
            inner.dispose();
        });
        o.dispose();
        assert_eq!(try_context::<u8>(), None, "outside any owner");
    }

    #[test]
    fn a_write_guard_tells_on_drop_and_a_dead_signal_says_so() {
        let ((s, runs), o) = scope(|| {
            let s = Signal::new(vec![1]);
            let runs = Rc::new(Cell::new(0));
            let r = runs.clone();
            render(move || {
                s.with(|v| v.len());
                r.set(r.get() + 1);
            });
            (s, runs)
        });
        s.write().push(2);
        assert_eq!(runs.get(), 2);
        assert_eq!(*s.read_untracked(), [1, 2]);
        o.dispose();
        assert!(!s.alive() && s.try_write().is_none() && s.try_read_untracked().is_none());
    }
}

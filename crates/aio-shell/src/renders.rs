//! The render-burst tripwire (aio's `air/dev-burst.ts`), in dev builds: a
//! component that renders [`LIMIT`] times within a second is said once, by
//! name. Dioxus names each render in a `tracing` span (`render`, with the
//! component as `scope`); [`Tripwire`] is the subscriber that counts them,
//! and listens to nothing else.
//!
//! The same spans keep the running app's component tree (aio DevTools'
//! `tree` and profile): each mounted component, its renders and the time
//! they took, for the `ui` door's `Aio.components` (`ax components`).
use aio::serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, field::Field, subscriber::Interest};

/// Renders within a second that make a burst (aio's limit).
pub const LIMIT: usize = 50;

/// Each component's renders in the last second; a burst is said once.
#[derive(Default)]
pub struct Bursts {
    recent: HashMap<String, VecDeque<Instant>>,
    said: HashSet<String>,
}

impl Bursts {
    /// `name` rendered at `now`: the line to say, the first time it bursts.
    pub fn render(&mut self, name: &str, now: Instant) -> Option<String> {
        if self.said.contains(name) {
            return None;
        }
        let q = self.recent.entry(name.into()).or_default();
        q.push_back(now);
        while q.front().is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(1)) {
            q.pop_front();
        }
        if q.len() < LIMIT {
            return None;
        }
        self.recent.remove(name);
        self.said.insert(name.into());
        Some(format!(
            "{name} rendered {LIMIT} times in under a second: a view that writes state it reads, \
             or a signal set on every tick, keeps it rendering (said once)"
        ))
    }
}

/// A component in a view: the view (its runtime) and its scope.
type Key = (usize, usize);

/// One mounted component.
#[derive(Clone, Debug, PartialEq)]
struct Node {
    name: String,
    parent: Option<usize>,
    renders: u32,
    took: Duration,
}

impl Node {
    /// Dioxus's own boundaries and the view's wrapper (no parent) are no
    /// component of the app's.
    fn boundary(&self) -> bool {
        self.parent.is_none() || self.name.starts_with("dioxus_core::")
    }
}

/// The mounted components of every view, as their renders tell them.
#[derive(Default)]
pub struct Tree {
    nodes: BTreeMap<Key, Node>,
    /// Renders running: by span, the component, when it began, what made it.
    open: HashMap<u64, (Key, Option<Instant>, &'static str)>,
    next: u64,
    /// The last [`RECENT`] renders (aio DevTools' render events), newest last.
    recent: VecDeque<Value>,
    total: u64,
}

/// Render events kept (aio's ring).
pub const RECENT: usize = 200;

impl Tree {
    /// Component `key` (a child of scope `parent`) renders as `name`
    /// (`by_parent`: in its parent's diff, which runs it again only for new
    /// props): the render's span. A slot taken over by another component
    /// starts over.
    fn start(&mut self, key: Key, parent: Option<usize>, name: &str, by_parent: bool) -> u64 {
        let fresh = || Node { name: name.into(), parent, renders: 0, took: Duration::ZERO };
        let n = self.nodes.entry(key).or_insert_with(fresh);
        if n.name != name || n.parent != parent {
            *n = fresh();
        }
        n.renders += 1;
        // aio DevTools' triggers: else its own state (a signal it reads) changed.
        let trigger = match (n.renders, by_parent) {
            (1, _) => "mount",
            (_, true) => "props",
            _ => "signal",
        };
        self.next += 1;
        self.open.insert(self.next, (key, None, trigger));
        self.next
    }

    fn enter(&mut self, span: u64, at: Instant) {
        if let Some(o) = self.open.get_mut(&span) {
            o.1 = Some(at);
        }
    }

    fn exit(&mut self, span: u64, at: Instant) {
        let Some((key, Some(from), trigger)) = self.open.get_mut(&span).map(|o| (o.0, o.1.take(), o.2)) else {
            return;
        };
        let Some(n) = self.nodes.get_mut(&key) else { return };
        let took = at.saturating_duration_since(from);
        n.took += took;
        if n.boundary() {
            return;
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        let e =
            json!({ "component": short(&n.name), "at": now.as_millis() as u64, "ms": ms(took), "trigger": trigger });
        if self.recent.len() == RECENT {
            self.recent.pop_front();
        }
        self.recent.push_back(e);
        self.total += 1;
    }

    /// The last renders, oldest first, and how many there were in all (pure).
    pub fn renders(&self) -> (Value, u64) {
        (self.recent.iter().cloned().collect(), self.total)
    }

    fn close(&mut self, span: u64) {
        self.open.remove(&span);
    }

    /// Component `key` unmounted.
    fn gone(&mut self, key: Key) {
        self.nodes.remove(&key);
    }

    /// Each view's components as a tree: name (without its module; the
    /// view's own is `root`), renders, their time in ms, children in mount
    /// order. Dioxus's own boundaries are no component of the app's (pure).
    pub fn json(&self) -> Value {
        Value::Array(self.under(None, None))
    }

    fn under(&self, view: Option<usize>, parent: Option<usize>) -> Vec<Value> {
        let mut out = vec![];
        for (&(v, scope), n) in &self.nodes {
            if view.is_some_and(|w| w != v) || n.parent != parent {
                continue;
            }
            let children = self.under(Some(v), Some(scope));
            if n.boundary() {
                out.extend(children);
                continue;
            }
            out.push(json!({ "name": short(&n.name), "renders": n.renders, "ms": ms(n.took), "children": children }));
        }
        out
    }
}

/// A component's name without its module.
fn short(name: &str) -> &str {
    name.rsplit("::").next().unwrap_or(name)
}

/// A time in ms, to 0.01.
fn ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 1e5).round() / 100.0
}

/// Kept in each component's own context: dropped with it, it takes the
/// component out of the tree.
#[derive(Clone)]
struct Alive {
    _gone: std::rc::Rc<Gone>,
}

struct Gone(Key, Arc<Mutex<Tree>>);

impl Drop for Gone {
    fn drop(&mut self) {
        self.1.lock().unwrap_or_else(|e| e.into_inner()).gone(self.0);
    }
}

/// The running app's tree, once its tripwire is up.
pub static TREE: std::sync::OnceLock<Arc<Mutex<Tree>>> = std::sync::OnceLock::new();

/// The `tracing` subscriber that hands each burst to `say`, and keeps the tree.
pub struct Tripwire {
    bursts: Mutex<Bursts>,
    tree: Arc<Mutex<Tree>>,
    say: Box<dyn Fn(&str) + Send + Sync>,
}

impl Tripwire {
    pub fn new(say: impl Fn(&str) + Send + Sync + 'static) -> Self {
        Self { bursts: Mutex::default(), tree: Arc::default(), say: Box::new(say) }
    }

    /// The tree it keeps.
    pub fn tree(&self) -> Arc<Mutex<Tree>> {
        self.tree.clone()
    }

    fn with<R>(&self, f: impl FnOnce(&mut Tree) -> R) -> R {
        f(&mut self.tree.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

fn is_render(m: &Metadata<'_>) -> bool {
    m.is_span() && m.name() == "render" && m.target().starts_with("dioxus_core")
}

/// A component's diff: a render inside one was run by its parent.
fn is_diff(m: &Metadata<'_>) -> bool {
    m.is_span() && m.name() == "VirtualDom::diff_scope" && m.target().starts_with("dioxus_core")
}

thread_local! {
    /// The diffs this thread (a view's) is inside.
    static DIFFING: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// The `scope` field of a render span.
struct Scope(String);

impl tracing::field::Visit for Scope {
    fn record_debug(&mut self, f: &Field, v: &dyn std::fmt::Debug) {
        if f.name() == "scope" {
            self.0 = format!("{v:?}");
        }
    }
}

/// A span outside any view (never entered by a render).
const NO_SPAN: u64 = u64::MAX;
/// Every diff's span.
const DIFF: u64 = u64::MAX - 1;

impl tracing::Subscriber for Tripwire {
    fn register_callsite(&self, m: &'static Metadata<'static>) -> Interest {
        if is_render(m) || is_diff(m) { Interest::always() } else { Interest::never() }
    }

    fn enabled(&self, m: &Metadata<'_>) -> bool {
        is_render(m) || is_diff(m)
    }

    fn new_span(&self, a: &Attributes<'_>) -> Id {
        if is_diff(a.metadata()) {
            return Id::from_u64(DIFF);
        }
        let mut s = Scope(String::new());
        a.record(&mut s);
        let line = self.bursts.lock().unwrap_or_else(|e| e.into_inner()).render(&s.0, Instant::now());
        if let Some(l) = line {
            (self.say)(&l);
        }
        // The render runs in its component's scope, on its view's thread.
        let Some((rt, id)) =
            dioxus_core::Runtime::try_current().and_then(|rt| rt.try_current_scope_id().map(|id| (rt, id)))
        else {
            return Id::from_u64(NO_SPAN);
        };
        let key = (std::rc::Rc::as_ptr(&rt) as usize, id.0);
        let span = self.with(|t| {
            // Its nearest ancestor that rendered (a suspense boundary renders unnamed).
            let mut up = rt.parent_scope(id);
            while let Some(p) = up.filter(|p| !t.nodes.contains_key(&(key.0, p.0))) {
                up = rt.parent_scope(p);
            }
            t.start(key, up.map(|p| p.0), &s.0, DIFFING.get() > 0)
        });
        if rt.has_context::<Alive>(id).is_none() {
            rt.provide_context(id, Alive { _gone: std::rc::Rc::new(Gone(key, self.tree.clone())) });
        }
        Id::from_u64(span)
    }

    fn record(&self, _: &Id, _: &Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, _: &Event<'_>) {}

    fn enter(&self, span: &Id) {
        match span.into_u64() {
            DIFF => DIFFING.set(DIFFING.get() + 1),
            s => self.with(|t| t.enter(s, Instant::now())),
        }
    }

    fn exit(&self, span: &Id) {
        match span.into_u64() {
            DIFF => DIFFING.set(DIFFING.get().saturating_sub(1)),
            s => self.with(|t| t.exit(s, Instant::now())),
        }
    }

    fn try_close(&self, span: Id) -> bool {
        self.with(|t| t.close(span.into_u64()));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn a_burst_is_fifty_renders_within_a_second_said_once_by_name() {
        let (mut b, t) = (Bursts::default(), Instant::now());
        let ms = |n: u64| t + Duration::from_millis(n);
        // 49 a second apart from the 50th: never a burst.
        for i in 0..LIMIT as u64 * 3 {
            assert_eq!(b.render("Slow", ms(i * 21)), None, "{i}");
        }
        for i in 0..LIMIT as u64 - 1 {
            assert_eq!(b.render("Fast", ms(i)), None);
        }
        assert_eq!(b.render("Other", ms(49)), None, "counted per component");
        let l = b.render("Fast", ms(49)).unwrap();
        assert!(l.starts_with("Fast rendered 50 times in under a second"), "{l}");
        assert!((0..200).all(|i| b.render("Fast", ms(50 + i)).is_none()), "said once");
    }

    #[test]
    fn the_tripwire_hears_dioxus_name_each_render() {
        fn app() -> dioxus_core::Element {
            dioxus_core::VNode::empty()
        }
        let said = Arc::new(Mutex::new(Vec::<String>::new()));
        let s = said.clone();
        let wire = Tripwire::new(move |l| s.lock().unwrap().push(l.into()));
        tracing::subscriber::with_default(wire, || {
            let mut dom = dioxus_core::VirtualDom::new(app);
            dom.rebuild_in_place();
            for _ in 0..LIMIT {
                dom.mark_dirty(dioxus_core::ScopeId::APP);
                dom.render_immediate(&mut dioxus_core::NoOpMutations);
            }
        });
        let said = said.lock().unwrap();
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].starts_with("root rendered 50 times"), "by its name (the app is the root): {said:?}");
    }

    /// aio DevTools' tree and per-component profile, in a running view: the
    /// mounted components with their renders and time; one unmounted goes.
    #[test]
    fn the_tree_is_each_mounted_component_with_its_renders_and_time() {
        use aio::ui::prelude::*;
        thread_local!(static SHOW: std::cell::Cell<bool> = const { std::cell::Cell::new(true) });
        #[component]
        fn Row(n: u32) -> View {
            rsx! { li { "{n}" } }
        }
        #[component]
        fn List() -> View {
            rsx! {
                ul {
                    Row { n: 1 }
                    Row { n: 2 }
                }
            }
        }
        fn app() -> View {
            rsx! {
                // The renderer's own components stay out of the tree.
                dioxus_core::SuspenseBoundary { fallback: |_| rsx! {}, List {} }
                if SHOW.get() {
                    Row { n: 3 }
                }
            }
        }
        let wire = Tripwire::new(|_| {});
        let tree = wire.tree();
        let names = |v: &Value| -> Value {
            fn walk(v: &Value) -> Value {
                let kids: Vec<Value> = v["children"].as_array().unwrap().iter().map(walk).collect();
                json!([v["name"], v["renders"], kids])
            }
            v.as_array().unwrap().iter().map(walk).collect()
        };
        tracing::subscriber::with_default(wire, || {
            let mut dom = dioxus_core::VirtualDom::new(app);
            dom.rebuild_in_place();
            let row = |n: u32| json!(["Row", n, []]);
            let got = tree.lock().unwrap().json();
            assert_eq!(names(&got), json!([["root", 1, [["List", 1, [row(1), row(1)]], row(1)]]]));
            assert!(got[0]["ms"].as_f64().is_some_and(|ms| ms >= 0.0), "{got}");
            dom.mark_dirty(dioxus_core::ScopeId::APP);
            SHOW.set(false);
            dom.render_immediate(&mut dioxus_core::NoOpMutations);
            let got = tree.lock().unwrap().json();
            assert_eq!(names(&got), json!([["root", 2, [["List", 1, [row(1), row(1)]]]]]), "the third row went");
            // aio DevTools' render events: each render, what made it, its time.
            let (renders, total) = tree.lock().unwrap().renders();
            let seen: Vec<_> =
                renders.as_array().unwrap().iter().map(|r| json!([r["component"], r["trigger"]])).collect();
            assert_eq!(seen.first(), Some(&json!(["root", "mount"])));
            assert_eq!(seen.last(), Some(&json!(["root", "signal"])), "marked dirty itself");
            assert_eq!((seen.len(), total), (6, 6), "root, List, three rows, root again: {seen:?}");
            assert!(renders[0]["ms"].as_f64().is_some() && renders[0]["at"].as_u64().is_some_and(|t| t > 0));
            drop(dom);
        });
        assert_eq!(tree.lock().unwrap().json(), json!([]), "the view gone, its tree too");
    }

    /// aio DevTools' triggers: a component runs again for a signal it reads
    /// (`signal`) or for new props from its parent (`props`); one whose
    /// props stayed is not run at all.
    #[test]
    fn a_render_says_whether_a_signal_or_new_props_made_it() {
        use aio::ui::prelude::*;
        thread_local!(static TICK: std::cell::Cell<Option<Signal<u32>>> = const { std::cell::Cell::new(None) });
        #[component]
        fn Label(n: u32) -> View {
            rsx! { b { "{n}" } }
        }
        #[component]
        fn Same() -> View {
            rsx! { i {} }
        }
        #[component]
        fn Ticker() -> View {
            let t = signal(|| 0);
            TICK.set(Some(t));
            rsx! { Label { n: t.get() } Same {} }
        }
        fn app() -> View {
            rsx! { Ticker {} }
        }
        let wire = Tripwire::new(|_| {});
        let tree = wire.tree();
        tracing::subscriber::with_default(wire, || {
            let mut dom = dioxus_core::VirtualDom::new(app);
            dom.rebuild_in_place();
            dom.in_runtime(|| TICK.get().unwrap().set(1));
            dom.render_immediate(&mut dioxus_core::NoOpMutations);
            let (renders, _) = tree.lock().unwrap().renders();
            let seen: Vec<_> =
                renders.as_array().unwrap().iter().map(|r| json!([r["component"], r["trigger"]])).collect();
            let after: Vec<_> = seen.iter().skip_while(|r| r[1] == "mount").cloned().collect();
            assert_eq!(after, [json!(["Ticker", "signal"]), json!(["Label", "props"])], "{seen:?}");
            drop(dom);
        });
    }
}

//! The tree of mounted components (aio DevTools' `tree`): names, keys,
//! body runs, hole updates; and `follow`, a body that runs again when what
//! it read changes.
use air::testing::TestPage;
use air::{
    Mounted, RENDERS_KEPT, Signal, View, component, components, follow, memo, provide_now, renders, rsx, try_context,
};

fn names(c: &[Mounted]) -> Vec<String> {
    c.iter()
        .map(|c| {
            let key = c.key.as_deref().map(|k| format!("[{k}]")).unwrap_or_default();
            let kids = names(&c.children);
            if kids.is_empty() { format!("{}{key}", c.name) } else { format!("{}{key}({})", c.name, kids.join(" ")) }
        })
        .collect()
}

fn find<'a>(c: &'a [Mounted], name: &str) -> &'a Mounted {
    fn walk<'a>(c: &'a [Mounted], name: &str) -> Option<&'a Mounted> {
        c.iter().find(|c| c.name == name).or_else(|| c.iter().find_map(|c| walk(&c.children, name)))
    }
    walk(c, name).unwrap_or_else(|| panic!("no {name} in {:?}", names(c)))
}

#[component]
fn Row(id: u32, count: Signal<u32>) -> View {
    rsx! { li { "data-t": "row-{id}", "{id}: {count}" } }
}

#[component]
fn Static() -> View {
    rsx! { p { "data-t": "static", "never changes" } }
}

#[component]
fn List(ids: Signal<Vec<u32>>, count: Signal<u32>) -> View {
    rsx! {
        ul { for id in ids.get() { Row { key: id, id, count } } }
        Static {}
    }
}

#[test]
fn the_tree_names_each_component_with_its_key_and_counts_its_work() {
    let ids = Signal::new(vec![1, 2, 3]);
    let count = Signal::new(0);
    let mut p = TestPage::mount(move || rsx! { main { List { ids, count } } });
    assert_eq!(names(&components()), ["List(Row[1] Row[2] Row[3] Static)"]);
    let row = find(&components(), "Row").clone();
    assert_eq!((row.renders, row.updates), (1, 0), "a body runs once");
    assert!(row.nodes.is_some(), "it drew a node");
    // A change re-runs the holes that read it: an update of each row, none of the list.
    p.within(|| count.set(1));
    let tree = components();
    let rows = &find(&tree, "List").children;
    assert!(rows.iter().filter(|r| r.name == "Row").all(|r| (r.renders, r.updates) == (1, 1)), "{tree:?}");
    assert_eq!((find(&tree, "List").updates, find(&tree, "Static").updates), (0, 0));
    // A row gone leaves the tree; a new one joins it, a kept one keeps its counts.
    p.within(|| ids.set(vec![3, 4]));
    assert_eq!(names(&components()), ["List(Row[3] Static Row[4])"], "in the order they were placed");
    let tree = components();
    let kept = find(&tree, "List").children.iter().find(|r| r.key.as_deref() == Some("3")).unwrap();
    assert_eq!(kept.updates, 1, "row 3 kept, with its count");
    // The list updated once (its `for` re-ran); its rows' bodies didn't run again.
    assert_eq!(find(&tree, "List").updates, 1);
    drop(p);
    assert!(components().is_empty(), "a dropped root leaves no entry");
}

/// The render log (aio DevTools' `renders`): each body run and hole
/// rewrite as it happened, its component, key, trigger and time.
#[test]
fn the_render_log_says_each_render_as_it_happened() {
    let ids = Signal::new(vec![1, 2]);
    let count = Signal::new(0);
    let mut p = TestPage::mount(move || rsx! { main { List { ids, count } } });
    let said = |r: &[air::Render]| -> Vec<String> {
        r.iter()
            .map(|e| {
                format!(
                    "{}{} {}",
                    e.component,
                    e.key.as_deref().map(|k| format!("[{k}]")).unwrap_or_default(),
                    e.trigger
                )
            })
            .collect()
    };
    let (log, total) = p.within(renders);
    assert_eq!(
        said(&log),
        ["Row[1] mount", "Row[2] mount", "Static mount", "List mount"],
        "each body's first run, as it ended (a parent's holds its children's)"
    );
    assert_eq!(total, 4);
    p.within(|| count.set(1));
    let (log, total) = p.within(renders);
    assert_eq!(said(&log[4..]), ["Row[1] signal", "Row[2] signal"], "the holes the change ran, nothing else");
    assert_eq!(total, 6);
    assert!(log.iter().all(|e| e.ms >= 0.0 && e.at > 1_600_000_000_000), "timed, stamped");
    for n in 2..2 + RENDERS_KEPT as u32 {
        p.within(|| count.set(n));
    }
    let (log, total) = p.within(renders);
    assert_eq!((log.len(), total), (RENDERS_KEPT, 6 + 2 * RENDERS_KEPT as u64), "the last ones kept, all counted");
    drop(p);
    let mut other = TestPage::mount(|| rsx! { p { "x" } });
    assert_eq!(other.within(renders), (vec![], 0), "a root's log is its own and ends with it");
}

#[test]
fn a_key_is_its_text_or_its_debug_form() {
    #[derive(Clone, PartialEq, Hash, Eq, Debug)]
    struct Id(u8, char);
    #[component]
    fn Cell(at: Id) -> View {
        rsx! { span { "{at:?}" } }
    }
    #[derive(Clone, PartialEq, Hash, Eq)]
    struct Opaque;
    let _p = TestPage::mount(|| {
        rsx! {
            for s in ["a b".to_string()] { Static { key: s } }
            for at in [Id(1, 'x')] { Cell { key: at.clone(), at } }
            for o in [Opaque] { Static { key: o } }
        }
    });
    let keys: Vec<_> = components().into_iter().map(|c| c.key).collect();
    assert_eq!(keys, [Some("a b".into()), Some("Id(1, 'x')".into()), None]);
}

#[derive(Clone)]
struct Depth(u32);

#[component]
fn Shaped(on: Signal<bool>, n: Signal<u32>) -> View {
    let runs = air::once(|| Signal::new(0u32));
    runs.update(|r| *r += 1);
    // Its shape follows `on` (not `n`: a memo keeps the re-runs to real changes).
    let open = memo(move || on.get());
    let shown = follow(|| open.get());
    provide_now(Depth(if shown { 1 } else { 0 }));
    if shown {
        rsx! { p { "data-t": "shape", "open {n}, run {runs}" } Inner {} }
    } else {
        rsx! { p { "data-t": "shape", "closed, run {runs}" } }
    }
}

#[component]
fn Inner() -> View {
    let d = try_context::<Depth>().map_or(9, |d| d.0);
    rsx! { span { "data-t": "depth", "{d}" } }
}

#[test]
fn a_followed_read_runs_the_body_again_with_its_hooks_kept() {
    let on = Signal::new(false);
    let n = Signal::new(0u32);
    let before = air::live();
    let mut p = TestPage::mount(move || rsx! { Shaped { on, n } });
    assert_eq!(p.text("shape"), "closed, run 1");
    p.within(|| n.set(1));
    assert_eq!(p.text("shape"), "closed, run 1", "not followed: no re-run");
    p.within(|| on.set(true));
    assert_eq!(p.text("shape"), "open 1, run 2", "followed: the body ran again, its signal kept");
    assert_eq!(p.text("depth"), "1", "what it provided this run");
    assert_eq!(find(&components(), "Shaped").renders, 2);
    p.within(|| n.set(2));
    assert_eq!(p.text("shape"), "open 2, run 2", "a hole follows on its own");
    p.within(|| on.set(false));
    assert!(!p.present("depth"), "what the last run made is gone");
    drop(p);
    assert_eq!(air::live(), before, "nothing left behind");
}

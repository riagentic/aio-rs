//! Views without `rsx!` (aio's `h()`), a keyed cache shared by views
//! (aio's `trackedMemo`) and an optimistic value (aio's `useOptimistic`).
use air::testing::TestPage;
use air::*;
use std::cell::Cell;

#[component]
fn Badge(label: String) -> View {
    let n = signal(|| 0);
    h("span")
        .attr("data-t", "badge")
        .child(label)
        .child(" ")
        .child(n)
        .on("click", move |_: Event<MouseData>| n.update(|v| *v += 1))
        .view()
}

#[test]
fn an_element_built_without_rsx_follows_its_state_and_hears_its_events() {
    let mut p = TestPage::mount(|| {
        let n = signal(|| 0);
        let wide = signal(|| false);
        h("div")
            .attr("data-t", "box")
            .attr_with("class", move || if wide.get() { "wide" } else { "narrow" })
            .child(
                h("button")
                    .attr("data-t", "add")
                    .on("click", move |_: Event<MouseData>| n.update(|v| *v += 1))
                    .child("Add"),
            )
            .child(
                h("button").attr("data-t", "wide").on("click", move |_: Event<MouseData>| wide.toggle()).child("Wide"),
            )
            .child(h("p").attr("data-t", "n").child(n))
            .child_with(move || (n.get() > 1).then(|| h("b").attr("data-t", "many").child("many")))
            .child(comp(Badge, BadgeProps { label: "hi".into() }))
            .view()
    });
    assert_eq!((p.text("n"), p.attr("box", "class").as_deref()), ("0".into(), Some("narrow")));
    assert!(!p.present("many"));
    p.click("add");
    p.click("add");
    p.click("wide");
    assert_eq!((p.text("n"), p.attr("box", "class").as_deref()), ("2".into(), Some("wide")));
    assert_eq!(p.text("many"), "many");
    p.click("badge");
    assert_eq!(p.text("badge"), "hi 1", "a component placed by comp() keeps its own state");
    let tree = p.within(components);
    assert_eq!(tree.iter().map(|c| c.name).collect::<Vec<_>>(), ["Badge"], "and is in the tree by name");
}

thread_local!(static MADE: Cell<u32> = const { Cell::new(0) });

static ROWS: Global<Vec<&'static str>> = Global::new(|| vec!["apple", "avocado", "banana"]);
static SHOWN: TrackedMemo<char, Vec<&'static str>> = TrackedMemo::new(|c| {
    MADE.set(MADE.get() + 1);
    ROWS.with(|r| r.iter().filter(|w| w.starts_with(*c)).copied().collect())
});

#[component]
fn Count(at: char, t: &'static str) -> View {
    rsx! { p { "data-t": t, {SHOWN.get(&at).len().to_string()} } }
}

#[test]
fn a_tracked_memo_is_made_once_per_key_and_a_hit_still_follows_what_it_read() {
    MADE.set(0);
    let mut p = TestPage::mount(|| {
        rsx! {
            Count { at: 'a', t: "one" }
            Count { at: 'a', t: "two" }
            Count { at: 'b', t: "three" }
            button { "data-t": "add", onclick: move |_| ROWS.write().push("almond"), "+" }
        }
    });
    assert_eq!((p.text("one"), p.text("two"), p.text("three")), ("2".into(), "2".into(), "1".into()));
    assert_eq!(MADE.get(), 2, "the second reader of 'a' hit");
    p.click("add");
    assert_eq!(
        (p.text("one"), p.text("two"), p.text("three")),
        ("3".into(), "3".into(), "1".into()),
        "the reader that hit followed the rows too"
    );
    assert_eq!(MADE.get(), 4, "each key made again once: the first reader of each re-ran on a stale entry");
}

static BOUNDED: TrackedMemo<u32, u32> = TrackedMemo::new(|n| n * 2).max(2);

#[test]
fn a_bounded_tracked_memo_lets_the_least_recently_used_go() {
    let p = TestPage::mount(|| rsx! { p { "data-t": "x", "{BOUNDED.get(&1)}" } });
    p.root().within(|| {
        assert_eq!((BOUNDED.get(&2), BOUNDED.get(&1), BOUNDED.get(&3)), (4, 2, 6));
        assert_eq!(BOUNDED.len(), 2, "2 went: used least lately");
    });
    assert_eq!(p.text("x"), "2");
}

#[test]
fn an_optimistic_value_shows_actions_until_the_base_changes_or_they_are_cleared() {
    let mut p = TestPage::mount(|| {
        let likes = signal(|| 10);
        let shown = use_optimistic(move || likes.get(), |n: &i32, d: &i32| n + d);
        rsx! {
            p { "data-t": "n", "{shown}" }
            p { "data-t": "wait", "{shown.pending()}" }
            button { "data-t": "like", onclick: move |_| shown.add(1), "+" }
            button { "data-t": "land", onclick: move |_| likes += 1, "landed" }
            button { "data-t": "fail", onclick: move |_| shown.clear(), "refused" }
        }
    });
    p.click("like");
    p.click("like");
    assert_eq!((p.text("n"), p.text("wait")), ("12".into(), "true".into()), "shown at once");
    p.click("land");
    assert_eq!((p.text("n"), p.text("wait")), ("11".into(), "false".into()), "the base moved: the actions are dropped");
    p.click("like");
    p.click("fail");
    assert_eq!(p.text("n"), "11", "a refusal that changed nothing: cleared");
}

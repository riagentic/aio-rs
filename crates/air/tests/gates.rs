//! The gates before Dioxus goes (docs/design/air.md), in the Rust page.
//! Gate 1: views removed, or replaced by an error boundary in the same
//! pass, leave nothing behind (signal, effect, subscription, owner,
//! handler, task, page script, page node). Gate 2: misuse fails loudly in
//! every build (`cargo test --release` runs these too): no `debug_assert`.
use air::testing::TestPage;
use air::*;
use std::cell::RefCell;

thread_local!(static WAIT: RefCell<Vec<std::task::Waker>> = const { RefCell::new(Vec::new()) });

/// A future that never ends (until its task is cancelled).
fn forever() -> impl std::future::Future<Output = u32> {
    std::future::poll_fn(|cx| {
        WAIT.with_borrow_mut(|w| w.push(cx.waker().clone()));
        std::task::Poll::Pending
    })
}

#[derive(Clone, PartialEq)]
struct Ctx(u32);

#[component]
fn Rich(n: Signal<u32>) -> View {
    let local = signal(|| 0u32);
    let double = memo(move || n.get() * 2);
    effect(move || _ = local.get() + n.get());
    provide(|| Ctx(1));
    task(forever);
    let waits = resource(forever);
    let _script = once(|| eval("await aio.recv();"));
    let sel = once(move || selector(move || n.get()));
    rsx! {
        div { "data-t": "rich", onclick: move |_| local += 1,
            "{double} {local} {waits.pending()}"
            for i in 0..5u32 {
                Row { key: i, i, on: sel.is(i) }
            }
            ErrorBoundary { fallback: |e: String| rsx! { p { "{e}" } },
                Thrower { when: n }
            }
            Suspense { fallback: rsx! { p { "waiting" } },
                Waits {}
            }
        }
    }
}

#[component]
fn Row(i: u32, on: bool) -> View {
    let c = context::<Ctx>();
    rsx! { span { class: if on { "on" } else { "" }, onclick: move |_| _ = c.0 + i, "{i}" } }
}

#[component]
fn Thrower(when: Signal<u32>) -> View {
    let kept = signal(|| 1u8);
    rsx! {
        p { onclick: move |_| kept += 1, "{kept}" }
        if when.get() % 2 == 1 {
            Throws {}
        }
    }
}

#[component]
fn Throws() -> View {
    Err(ViewError::new("odd"))
}

#[component]
fn Waits() -> View {
    let r = resource(forever);
    let v = r.suspend()?;
    rsx! { p { "{v}" } }
}

fn toggled() -> View {
    let open = signal(|| false);
    let n = signal(|| 0u32);
    rsx! {
        button { "data-t": "toggle", onclick: move |_| open.toggle(), "toggle" }
        button { "data-t": "n", onclick: move |_| n += 1, "n" }
        if open.get() {
            Rich { n }
        }
    }
}

#[test]
fn gate_1_views_mounted_and_removed_n_times_leave_nothing_behind() {
    let mut p = TestPage::mount(toggled);
    let (base, named) = (air::live(), p.named());
    for k in 0..25 {
        p.click("toggle");
        assert!(p.present("rich"), "round {k}");
        // An error boundary swaps its children for its fallback, and back on a remount.
        p.click("n");
        p.click("rich");
        p.click("toggle");
        assert!(!p.present("rich"));
        assert_eq!(air::live(), base, "round {k}: what the removed views made is gone");
        assert_eq!(p.named(), named, "round {k}: no node of theirs is named in the page");
    }
    drop(p);
    WAIT.with_borrow_mut(Vec::clear);
    assert_eq!(air::live(), Live::default(), "the root's end leaves nothing at all");
}

#[test]
fn gate_1_an_error_boundary_replacing_its_children_in_the_same_pass_leaves_nothing_of_them() {
    fn app() -> View {
        let n = signal(|| 0u32);
        rsx! {
            button { "data-t": "n", onclick: move |_| n += 1, "n" }
            ErrorBoundary { fallback: |e: String| rsx! { p { "data-t": "caught", "{e}" } },
                Thrower { when: n }
            }
        }
    }
    let mut p = TestPage::mount(app);
    let with_children = air::live();
    p.click("n");
    assert_eq!(p.text("caught"), "odd");
    let after = air::live();
    assert!(
        after.nodes < with_children.nodes && after.handlers < with_children.handlers,
        "{after:?} {with_children:?}"
    );
    drop(p);
    assert_eq!(air::live(), Live::default());
}

fn says(f: impl FnOnce() + std::panic::UnwindSafe) -> String {
    let e = std::panic::catch_unwind(f).expect_err("misuse must fail");
    e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default()
}

#[test]
fn gate_2_a_hook_outside_a_view_fails_saying_where_hooks_go() {
    let e = says(|| _ = signal(|| 0));
    assert!(e.contains("`signal` keeps something for a view") && e.contains("view's body"), "{e}");
    fn app() -> View {
        rsx! { button { "data-t": "b", onclick: move |_| _ = memo(|| 1), "b" } }
    }
    let e = says(|| TestPage::mount(app).click("b"));
    assert!(e.contains("`memo` keeps something for a view") && e.contains("a handler"), "{e}");
}

#[test]
fn gate_2_a_write_while_a_view_draws_fails() {
    fn app() -> View {
        let n = signal(|| 0);
        rsx! { p { {n.set(1); "drawn"} } }
    }
    let e = says(|| _ = TestPage::mount(app));
    assert!(e.contains("written while a view draws"), "{e}");
    fn in_memo() -> View {
        let n = signal(|| 0);
        let m = memo(move || {
            n.set(2);
            1
        });
        rsx! { p { "{m}" } }
    }
    let e = says(|| _ = TestPage::mount(in_memo));
    assert!(e.contains("written while a memo computes"), "{e}");
}

#[test]
fn gate_2_an_effect_loop_trips_the_round_guard() {
    fn app() -> View {
        let n = signal(|| 0u64);
        effect(move || n.set(n.get() + 1));
        rsx! { p { "{n}" } }
    }
    let e = says(|| _ = TestPage::mount(app));
    assert!(e.contains("1000 rounds") && e.contains("effect loop"), "{e}");
}

#[test]
fn gate_2_a_signal_used_after_its_view_ended_fails_unless_asked_first() {
    static KEPT: Global<Option<Signal<u32>>> = Global::new(|| None);
    #[component]
    fn Keeps() -> View {
        let mine = signal(|| 5u32);
        once(move || KEPT.set(Some(mine)));
        rsx! { p { "{mine}" } }
    }
    fn app() -> View {
        let open = signal(|| true);
        rsx! {
            button { "data-t": "drop", onclick: move |_| open.set(false), "drop" }
            if open.get() { Keeps {} }
        }
    }
    let mut p = TestPage::mount(app);
    let s = p.within(|| KEPT.get_untracked()).expect("kept");
    p.click("drop");
    assert!(!s.alive() && s.try_write().is_none() && s.try_read_untracked().is_none());
    let e = says(move || _ = s.get_untracked());
    assert!(e.contains("used after the view that made it ended"), "{e}");
}

#[test]
fn gate_2_hooks_called_in_another_order_on_a_rerun_fail() {
    thread_local!(static RUNS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) });
    #[component]
    fn Shifty() -> View {
        RUNS.set(RUNS.get() + 1);
        if RUNS.get() == 1 {
            once(|| 1u32);
        } else {
            once(|| "another");
        }
        // Ready on its second poll: the body waits once, then runs again.
        let r = resource(|| {
            let mut first = true;
            std::future::poll_fn(move |cx| {
                if std::mem::take(&mut first) {
                    cx.waker().wake_by_ref();
                    return std::task::Poll::Pending;
                }
                std::task::Poll::Ready(1)
            })
        });
        let v = r.suspend()?;
        rsx! { p { "{v}" } }
    }
    let e = says(|| _ = TestPage::mount(|| rsx! { Shifty {} }));
    assert!(e.contains("another order"), "{e}");
}

#[test]
fn gate_2_two_rows_with_one_key_fail() {
    fn app() -> View {
        let rows = signal(|| vec![1, 2]);
        rsx! {
            button { "data-t": "dup", onclick: move |_| rows.set(vec![1, 1]), "dup" }
            for r in rows.get() { p { key: r, "{r}" } }
        }
    }
    let mut p = TestPage::mount(app);
    let e = says(std::panic::AssertUnwindSafe(move || p.click("dup")));
    assert!(e.contains("has the key of a row before it"), "{e}");
}

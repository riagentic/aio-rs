//! What phase 1 adds to the spike, in the Rust page: views built where
//! they are placed, components that run once, context, boundaries,
//! resources and suspense, bubbling events, page scripts, globals.
use air::testing::TestPage;
use air::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

thread_local! {
    static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static BODIES: Cell<u32> = const { Cell::new(0) };
}

fn log(s: impl Into<String>) {
    LOG.with(|l| l.borrow_mut().push(s.into()));
}

fn said() -> Vec<String> {
    LOG.with(|l| std::mem::take(&mut *l.borrow_mut()))
}

#[component]
fn Counter(start: i32) -> View {
    BODIES.set(BODIES.get() + 1);
    let n = signal(|| start);
    rsx! {
        button { "data-t": "inc", onclick: move |_| n += 1, "+" }
        p { "data-t": "n", "{n}" }
    }
}

#[test]
fn a_components_body_runs_once_and_only_its_holes_follow() {
    let mut p = TestPage::mount(|| rsx! { Counter { start: 5 } });
    assert_eq!((p.text("n"), BODIES.get()), ("5".into(), 1));
    for _ in 0..3 {
        p.click("inc");
    }
    assert_eq!((p.text("n"), BODIES.get()), ("8".into(), 1), "three clicks, no body re-run");
}

#[derive(Clone, PartialEq, Debug)]
struct Theme(&'static str);

#[component]
fn Themed() -> View {
    let t = context::<Theme>();
    let none = try_context::<u8>().is_none();
    rsx! { p { "data-t": "theme", "{t.0} {none}" } }
}

#[component]
fn Provider(children: View) -> View {
    provide(|| Theme("dark"));
    rsx! { div { {children} } }
}

#[test]
fn children_are_built_inside_their_parent_so_they_see_its_context() {
    let p = TestPage::mount(|| rsx! { Provider { Themed {} } });
    assert_eq!(p.text("theme"), "dark true", "built under Provider, though made by the root");
}

#[component]
fn Fails(fail: Signal<bool>) -> View {
    let made = signal(|| 0u8);
    on_cleanup(|| log("children gone"));
    rsx! {
        p { "data-t": "fine", "fine {made}" }
        if fail.get() {
            Breaks {}
        }
    }
}

#[component]
fn Breaks() -> View {
    Err(std::io::Error::other("the disk is gone"))?;
    rsx! { p { "unreachable" } }
}

#[test]
fn an_error_thrown_in_a_view_swaps_in_the_nearest_boundarys_fallback_in_one_pass() {
    fn app() -> View {
        let fail = signal(|| false);
        rsx! {
            button { "data-t": "break", onclick: move |_| fail.set(true), "break" }
            ErrorBoundary { fallback: |e: String| rsx! { p { "data-t": "caught", "{e}" } },
                Fails { fail }
            }
        }
    }
    let mut p = TestPage::mount(app);
    assert!(p.present("fine") && !p.present("caught"));
    let frames = p.frames;
    p.click("break");
    assert_eq!(p.text("caught"), "the disk is gone");
    assert!(!p.present("fine"), "the children are gone");
    assert_eq!(said(), ["children gone"]);
    assert_eq!(p.frames - frames, 1, "one frame: the break and the swap");
}

#[test]
fn a_boundarys_fallback_retries_and_the_children_come_back() {
    // aio's boundary reset: the children made anew; one that fails again is caught again.
    thread_local!(static BROKEN: Cell<bool> = const { Cell::new(true) });
    #[component]
    fn Flaky() -> View {
        if BROKEN.get() {
            Err(std::io::Error::other("not yet"))?;
        }
        rsx! { p { "data-t": "ok", "ok" } }
    }
    fn app() -> View {
        rsx! {
            ErrorBoundary {
                fallback: |e: String| {
                    let r = retry();
                    rsx! { button { "data-t": "retry", onclick: move |_| r.retry(), "{e}" } }
                },
                Flaky {}
            }
        }
    }
    let mut p = TestPage::mount(app);
    assert_eq!(p.text("retry"), "not yet");
    p.click("retry");
    assert_eq!(p.text("retry"), "not yet", "still broken: caught again");
    BROKEN.set(false);
    p.click("retry");
    assert!(p.present("ok") && !p.present("retry"));
}

#[test]
fn an_error_no_boundary_catches_fails_the_page_step() {
    let r = std::panic::catch_unwind(|| TestPage::mount(|| rsx! { Breaks {} }));
    let e = r.err().and_then(|e| e.downcast::<String>().ok()).expect("a failure");
    assert!(e.contains("no ErrorBoundary") && e.contains("the disk is gone"), "{e}");
}

/// A future the test finishes by hand.
#[derive(Clone, Default)]
struct Gate(Rc<RefCell<(Option<u32>, Option<std::task::Waker>)>>);

impl Gate {
    fn open(&self, v: u32) {
        let w = {
            let mut g = self.0.borrow_mut();
            g.0 = Some(v);
            g.1.take()
        };
        w.expect("someone waits").wake();
    }

    fn wait(&self) -> impl std::future::Future<Output = u32> + use<> {
        let g = self.clone();
        std::future::poll_fn(move |cx| {
            let mut g = g.0.borrow_mut();
            match g.0 {
                Some(v) => std::task::Poll::Ready(v),
                None => {
                    g.1 = Some(cx.waker().clone());
                    std::task::Poll::Pending
                }
            }
        })
    }
}

thread_local!(static GATE: RefCell<Gate> = RefCell::default());

#[component]
fn Loads() -> View {
    BODIES.set(BODIES.get() + 1);
    let kept = signal(|| 7u32);
    let r = resource(|| GATE.with_borrow(Gate::wait));
    let v = r.suspend()?;
    rsx! { p { "data-t": "loaded", "{v} {kept}" } }
}

#[test]
fn suspense_shows_its_fallback_until_the_resource_is_loaded_then_the_body_runs_again_with_its_state() {
    BODIES.set(0);
    let mut p = TestPage::mount(|| {
        rsx! {
            Suspense { fallback: rsx! { p { "data-t": "loading", "…" } },
                Loads {}
            }
        }
    });
    assert!(p.present("loading") && !p.present("loaded"));
    let live = air::live();
    GATE.with_borrow(|g| g.open(41));
    p.flush();
    assert_eq!(p.text("loaded"), "41 7", "the hook's signal kept");
    assert!(!p.present("loading"));
    assert_eq!(BODIES.get(), 2, "once waiting, once loaded");
    assert!(air::live().tasks < live.tasks.max(1), "the loader ended");
}

#[test]
fn a_resource_follows_what_it_read_before_its_first_await_and_restarts() {
    thread_local!(static LOADS: Cell<u32> = const { Cell::new(0) });
    fn app() -> View {
        let q = signal(|| 1u32);
        let r = resource(move || {
            let q = q.get();
            LOADS.set(LOADS.get() + 1);
            async move { q * 10 }
        });
        rsx! {
            button { "data-t": "q", onclick: move |_| q += 1, "q" }
            button { "data-t": "again", onclick: move |_| r.restart(), "again" }
            p { "data-t": "v", {r.get().map(|v| v.to_string()).unwrap_or_default()} }
        }
    }
    let mut p = TestPage::mount(app);
    assert_eq!(p.text("v"), "10");
    p.click("q");
    assert_eq!(p.text("v"), "20");
    p.click("again");
    assert_eq!((p.text("v"), LOADS.get()), ("20".into(), 3));
}

#[test]
fn a_resource_shows_a_mutated_value_and_a_disposed_one_loads_no_more() {
    thread_local!(static LOADS: Cell<u32> = const { Cell::new(0) });
    fn app() -> View {
        let q = signal(|| 1u32);
        let r = resource(move || {
            let q = q.get();
            LOADS.set(LOADS.get() + 1);
            async move { q * 10 }
        });
        rsx! {
            button { "data-t": "q", onclick: move |_| q += 1, "q" }
            button { "data-t": "set", onclick: move |_| r.mutate(99), "set" }
            button { "data-t": "off", onclick: move |_| r.dispose(), "off" }
            p { "data-t": "v", {r.get().map(|v| v.to_string()).unwrap_or_default()} }
        }
    }
    let mut p = TestPage::mount(app);
    p.click("set");
    assert_eq!((p.text("v"), LOADS.get()), ("99".into(), 1), "aio's mutate: shown at once, no load");
    p.click("q");
    assert_eq!(p.text("v"), "20", "the next load replaces it");
    p.click("off");
    p.click("q");
    assert_eq!((p.text("v"), LOADS.get()), ("20".into(), 2), "disposed: no load, the value kept");
}

#[test]
fn an_event_bubbles_through_the_handlers_until_one_stops_it() {
    fn app() -> View {
        rsx! {
            div { "data-t": "outer", onclick: move |_| log("outer"),
                div { onclick: move |_| log("middle"),
                    button { "data-t": "go", onclick: move |_| log("button"), "go" }
                    button { "data-t": "stop", onclick: move |e: MouseEvent| { log("stopper"); e.stop_propagation() }, "stop" }
                }
            }
        }
    }
    let mut p = TestPage::mount(app);
    p.click("go");
    assert_eq!(said(), ["button", "middle", "outer"]);
    p.click("stop");
    assert_eq!(said(), ["stopper"]);
    p.click("outer");
    assert_eq!(said(), ["outer"]);
}

#[test]
fn an_effect_runs_after_the_view_is_drawn_and_follows_what_it_read() {
    fn app() -> View {
        let n = signal(|| 0);
        effect(move || log(format!("effect {n}")));
        watch(n.get_untracked(), |n| log(format!("watched {n}")));
        let made = once(|| {
            log("made");
            3
        });
        rsx! {
            button { "data-t": "n", onmounted: move |_| log("mounted"), onclick: move |_| n += made, "{n}" }
        }
    }
    let mut p = TestPage::mount(app);
    assert_eq!(said(), ["made", "effect 0", "watched 0", "mounted"]);
    p.click("n");
    assert_eq!(said(), ["effect 3"]);
}

#[test]
fn a_page_script_gets_values_and_its_return_comes_back() {
    thread_local!(static GOT: RefCell<Option<String>> = const { RefCell::new(None) });
    fn app() -> View {
        task(|| async {
            let e = eval("aio.send(42); return 'done';");
            let v = e.recv_json().await;
            let end = e.join_json().await;
            GOT.set(Some(format!("{v:?} {end:?}")));
        });
        rsx! { p { "page" } }
    }
    let mut p = TestPage::mount(app);
    let (id, js) = p.scripts[0].clone();
    assert!(js.contains("aio.send(42)"));
    p.answer(id, 0, "42");
    p.answer(id, 1, "\"done\"");
    assert_eq!(GOT.take().as_deref(), Some(r#"Ok("42") Ok("\"done\"")"#));
}

#[test]
fn a_page_script_that_returned_still_sends_what_its_timers_send() {
    thread_local!(static GOT: RefCell<Option<String>> = const { RefCell::new(None) });
    fn app() -> View {
        task(|| async {
            let e = eval("setTimeout(() => aio.send(1), 10);");
            let v = e.recv_json().await;
            GOT.set(Some(format!("{v:?}")));
        });
        rsx! { p { "page" } }
    }
    let mut p = TestPage::mount(app);
    let id = p.scripts[0].0;
    p.answer(id, 1, "null");
    assert_eq!(GOT.take(), None, "its return is no value: recv waits on");
    p.answer(id, 0, "1");
    assert_eq!(GOT.take().as_deref(), Some(r#"Ok("1")"#));
}

static LEFT: Global<u32> = Global::new(|| 0);
static RIGHT: Global<u32> = Global::new(|| 10);
static SUM: GlobalMemo<u32> = GlobalMemo::new(|| LEFT.get() + RIGHT.get());

fn two_globals() -> View {
    rsx! {
        button { "data-t": "left", onclick: move |_| LEFT.update(|n| *n += 1), "{LEFT}" }
        p { "data-t": "right", "{RIGHT}" }
        p { "data-t": "sum", "{SUM}" }
    }
}

#[test]
fn each_global_is_its_own_state_per_app_instance_and_a_global_memo_follows_them() {
    let mut p = TestPage::mount(two_globals);
    p.click("left");
    p.click("left");
    assert_eq!((p.text("left"), p.text("right"), p.text("sum")), ("2".into(), "10".into(), "12".into()));
    assert_eq!(p.within(|| LEFT.get_untracked()), 2);
    drop(p);
    let p = TestPage::mount(two_globals);
    assert_eq!(p.text("left"), "0", "a new app instance starts again");
}

#[test]
fn an_effect_older_than_a_global_memo_it_reads_never_sees_it_stale() {
    fn app() -> View {
        effect(|| log(format!("{} {}", LEFT.get(), SUM.get())));
        rsx! { button { "data-t": "left", onclick: move |_| LEFT.update(|n| *n += 1), "+" } }
    }
    said();
    let mut p = TestPage::mount(app);
    assert_eq!(said(), ["0 10"]);
    p.click("left");
    assert_eq!(said(), ["1 11"], "one run, with the sum of the new left");
}

static DOUBLE: GlobalMemo<u32> = GlobalMemo::new(|| SUM.get() * 2);

#[test]
fn an_effect_older_than_a_memo_of_a_memo_never_sees_it_stale() {
    fn app() -> View {
        effect(|| log(format!("{} {}", LEFT.get(), DOUBLE.get())));
        rsx! { button { "data-t": "left", onclick: move |_| LEFT.update(|n| *n += 1), "+" } }
    }
    said();
    let mut p = TestPage::mount(app);
    assert_eq!(said(), ["0 20"]);
    p.click("left");
    assert_eq!(said(), ["1 22"], "one run, with the double of the new sum");
}

#[test]
fn an_error_in_a_boundarys_own_fallback_reaches_the_boundary_above() {
    fn app() -> View {
        let fail = signal(|| true);
        rsx! {
            ErrorBoundary { fallback: |e: String| rsx! { p { "data-t": "outer", "{e}" } },
                ErrorBoundary { fallback: |_: String| -> View { Err(ViewError::new("fb")) },
                    Fails { fail }
                }
            }
        }
    }
    let p = TestPage::mount(app);
    assert_eq!(p.text("outer"), "fb");
}

/// The same named global as hot-patched code sees it after its type
/// changed (`ax dev`, air's `dev`): made again with its first value, never a
/// panic. Without `dev` there is no hot-patch: two types is a bug, said loudly.
static OLD_SHAPE: Global<u32> = Global::named(|| 7, "app::NOTE");
static NEW_SHAPE: Global<String> = Global::named(|| "fresh".into(), "app::NOTE");

#[test]
#[cfg_attr(not(feature = "dev"), should_panic(expected = "air: a global of another type"))]
fn a_global_whose_type_a_patch_changed_starts_again() {
    let mut p = TestPage::mount(two_globals);
    assert_eq!(p.within(|| OLD_SHAPE.get_untracked()), 7);
    assert_eq!(p.within(|| NEW_SHAPE.get_untracked()), "fresh");
    assert_eq!(p.within(|| OLD_SHAPE.get_untracked()), 7, "and back, each read as its own type");
}

#[test]
fn two_roots_on_one_thread_are_apart() {
    let mut a = TestPage::mount(two_globals);
    let mut b = TestPage::mount(two_globals);
    a.click("left");
    a.click("left");
    b.click("left");
    assert_eq!((a.text("left"), a.text("sum")), ("2".into(), "12".into()), "a's globals, a's page");
    assert_eq!((b.text("left"), b.text("sum")), ("1".into(), "11".into()), "b's own");
    drop(a);
    b.click("left");
    assert_eq!(b.text("left"), "2", "b lives on when a ends");
    drop(b);
    assert_eq!(air::live(), air::Live::default(), "nothing left");
}

#[test]
fn a_component_whose_root_is_a_list_moves_whole_as_a_keyed_row() {
    #[component]
    fn Pair(n: u32) -> View {
        rsx! {
            for k in [n * 10, n * 10 + 1] {
                span { key: k, "{k}" }
            }
        }
    }
    fn app() -> View {
        let order = signal(|| vec![1u32, 2, 3]);
        rsx! {
            button { "data-t": "flip", onclick: move |_| order.update(|o| o.reverse()), "flip" }
            div { "data-t": "rows",
                for n in order.get() {
                    Pair { key: n, n }
                }
            }
        }
    }
    let mut p = TestPage::mount(app);
    assert_eq!(p.texts("rows"), ["10", "11", "20", "21", "30", "31"]);
    let named = p.named();
    p.click("flip");
    assert_eq!(p.texts("rows"), ["30", "31", "20", "21", "10", "11"], "each row's whole range moved");
    assert_eq!(p.named(), named, "moved, none made");
}

#[test]
fn a_callback_prop_is_called_by_the_child_and_writes_the_parents_state() {
    #[component]
    fn Picker(on_pick: EventHandler<u32>) -> View {
        rsx! { button { "data-t": "pick", onclick: move |_| on_pick.call(7), "pick" } }
    }
    fn app() -> View {
        let picked = signal(|| 0u32);
        rsx! {
            Picker { on_pick: move |v| picked.set(v) }
            p { "data-t": "picked", "{picked}" }
        }
    }
    let mut p = TestPage::mount(app);
    p.click("pick");
    assert_eq!(p.text("picked"), "7");
}

#[test]
fn a_view_placed_twice_or_shown_again_is_built_again_each_with_its_own_state() {
    fn app() -> View {
        let open = signal(|| true);
        let v = rsx! { Counter { start: 1 } };
        let again = v.clone();
        rsx! {
            div { "data-t": "a", {v.clone()} }
            div { "data-t": "b", {again} }
            button { "data-t": "toggle", onclick: move |_| open.toggle(), "toggle" }
            if open.get() { section { "data-t": "c", {v.clone()} } }
        }
    }
    let mut p = TestPage::mount(app);
    let all = |p: &TestPage| [p.text("a"), p.text("b"), p.text("c")];
    assert_eq!(all(&p), ["+1", "+1", "+1"], "three places, three builds");
    p.click("inc");
    assert_eq!(all(&p), ["+2", "+1", "+1"], "each its own state");
    p.click("toggle");
    assert!(!p.present("c"));
    p.click("toggle");
    assert_eq!(all(&p), ["+2", "+1", "+1"], "shown again: built again");
}

#[test]
fn a_styled_view_sets_one_property_of_its_first_element_and_keeps_its_own() {
    #[component]
    fn Inner() -> View {
        rsx! { p { "data-t": "in", style: "color: red", "in" } }
    }
    fn app() -> View {
        let on = signal(|| false);
        let shown = styled(rsx! { Inner {} }, "opacity", move || on.get().then(|| "0.5".to_string()));
        rsx! {
            button { "data-t": "on", onclick: move |_| on.toggle(), "on" }
            {shown}
        }
    }
    let mut p = TestPage::mount(app);
    assert_eq!(p.attr("in", "style").as_deref(), Some("color: red"), "None: no property");
    p.click("on");
    assert_eq!(
        p.attr("in", "style").as_deref(),
        Some("color: red; opacity: 0.5"),
        "a component's element, its own style kept"
    );
    p.click("on");
    assert_eq!(p.attr("in", "style").as_deref(), Some("color: red"));
}

#[test]
fn an_element_handed_out_runs_scripts_on_itself() {
    fn app() -> View {
        let el = signal(|| None::<Rc<MountedData>>);
        rsx! {
            canvas { "data-t": "c", onmounted: move |e: MountedEvent| el.set(Some(e.data())) }
            button { "data-t": "draw", onclick: move |_| if let Some(c) = el.get() { c.run("el.getContext('2d').fillRect(0, 0, 4, 4)") }, "draw" }
        }
    }
    let mut p = TestPage::mount(app);
    let mounted = p.scripts.len();
    p.click("draw");
    let (id, js) = p.scripts.last().cloned().unwrap();
    assert_eq!((p.scripts.len(), id), (mounted + 1, 0), "one script, no reply kept");
    assert!(js.contains("el.getContext('2d').fillRect(0, 0, 4, 4)") && js.starts_with("const el = aio.node("), "{js}");
}

#[component]
fn Draft(saved: Signal<String>) -> View {
    let text = signal(|| "typed".to_string());
    let shown = memo(move || format!("{} chars", text.with(String::len)));
    // A view's cleanup reads its own state (Solid's `onCleanup`).
    on_cleanup(move || saved.set(format!("{} ({})", text.get_untracked(), shown.get_untracked())));
    rsx! { p { "data-t": "draft", "{text}" } }
}

#[test]
fn a_cleanup_reads_its_views_signals_and_memos() {
    let saved = Signal::new(String::new());
    let open = Signal::new(true);
    let mut p = TestPage::mount(
        move || rsx! { button { "data-t": "close", onclick: move |_| open.set(false), "x" } if open.get() { Draft { saved } } },
    );
    assert_eq!(p.text("draft"), "typed");
    p.click("close");
    assert!(!p.present("draft"));
    assert_eq!(saved.get_untracked(), "typed (5 chars)");
}

#[test]
fn style_attributes_are_css_properties_in_the_rust_page_too() {
    let wide = Signal::new(true);
    let mut p = TestPage::mount(move || {
        rsx! { div { "data-t": "box", display: "flex", min_width: wide.get().then_some("10px"), button { "data-t": "narrow", onclick: move |_| wide.set(false), "n" } } }
    });
    assert_eq!(p.attr("box", "style").as_deref(), Some("display: flex; min-width: 10px"));
    p.click("narrow");
    assert_eq!(p.attr("box", "style").as_deref(), Some("display: flex"));
    // A string after the properties keeps the ones it doesn't set, as the page does.
    let p = TestPage::mount(
        || rsx! { div { "data-t": "over", position: "fixed", color: "blue", style: "inset: 0; color: red" } },
    );
    assert_eq!(p.attr("over", "style").as_deref(), Some("inset: 0; color: red; position: fixed"));
}

#[test]
fn a_task_spawned_forever_in_an_app_ends_with_its_root() {
    struct Ends(Rc<Cell<bool>>);
    impl Drop for Ends {
        fn drop(&mut self) {
            self.0.set(true)
        }
    }
    let ended = Rc::new(Cell::new(false));
    let e = ended.clone();
    let page = TestPage::mount(move || {
        let held = Ends(e.clone());
        spawn_forever(async move {
            let _held = held;
            std::future::pending::<()>().await
        });
        rsx! { p { "app" } }
    });
    let live = live_tasks();
    assert!(!ended.get(), "it runs while the app does");
    drop(page);
    assert!(ended.get(), "the app's end ended it (a next app on the thread never polls it)");
    assert_eq!(live_tasks(), live - 1);
}

struct Ends(Rc<Cell<bool>>);
impl Drop for Ends {
    fn drop(&mut self) {
        self.0.set(true)
    }
}

thread_local! {
    static ENDED: Rc<Cell<bool>> = Rc::default();
    static FLIP: Cell<Option<(Signal<bool>, Signal<bool>)>> = const { Cell::new(None) };
}

#[component]
fn Saver(redraw: Signal<bool>) -> View {
    rsx! {
        if redraw.get() {
            p { "data-t": "other", "drawn again" }
        } else {
            for n in [1u32] {
                button {
                    key: "{n}",
                    "data-t": "go",
                    onclick: move |_| {
                        let held = Ends(ENDED.with(Rc::clone));
                        async move {
                            let _held = held;
                            std::future::pending::<()>().await
                        }
                    },
                    "save"
                }
            }
        }
    }
}

fn saver_app() -> View {
    let (shown, redraw) = (signal(|| true), signal(|| false));
    FLIP.set(Some((shown, redraw)));
    rsx! {
        if shown.get() {
            Saver { redraw }
        }
    }
}

#[test]
fn a_handlers_task_is_its_components_a_redrawn_branch_or_row_cancels_none_of_it() {
    let mut p = TestPage::mount(saver_app);
    let (shown, redraw) = FLIP.get().unwrap();
    let live = live_tasks();
    p.click("go");
    assert_eq!(live_tasks(), live + 1, "the click spawned its work");
    p.within(|| redraw.set(true));
    assert!(p.present("other"), "the branch and the row that drew the button are gone");
    assert!(!ENDED.with(|e| e.get()), "and the work the click started runs on (it is the component's)");
    assert_eq!(live_tasks(), live + 1);
    p.within(|| shown.set(false));
    assert!(ENDED.with(|e| e.get()), "the component's end ended it");
    assert_eq!(live_tasks(), live);
}

#[test]
fn an_event_for_a_redrawn_element_is_dropped_and_a_dev_build_says_so_once() {
    static HEARD: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());
    air::on_warn(|k, m| HEARD.lock().unwrap().push((k.into(), m.into())));
    let mut p = TestPage::mount(saver_app);
    let (_, redraw) = FLIP.get().unwrap();
    let click = || air::Raw { name: "click".into(), button: Some(0), ..air::Raw::default() };
    let late = p.aim("go", click()).encode();
    p.within(|| redraw.set(true));
    assert!(!p.root().event(&late), "no handler took it: its element was drawn again");
    assert!(!p.root().event(&late));
    let heard = HEARD.lock().unwrap().clone();
    if cfg!(feature = "dev") {
        assert_eq!(heard.len(), 1, "said once per event name: {heard:?}");
        assert_eq!(heard[0].0, "ui:stale-event");
        assert!(heard[0].1.contains("`click`") && heard[0].1.contains("dropped"), "{}", heard[0].1);
    } else {
        assert!(heard.is_empty(), "a release build says nothing");
    }
}

#[test]
fn a_components_handlers_leave_nothing_behind_per_event() {
    fn app() -> View {
        let n = signal(|| 0u32);
        let el = signal(|| None::<Rc<MountedData>>);
        rsx! {
            input { "data-t": "f", onmounted: move |e: MountedEvent| el.set(Some(e.data())) }
            button { "data-t": "add", onclick: move |_| async move { n += 1 }, "{n}" }
            button {
                "data-t": "focus",
                onclick: move |_| {
                    let el = el.get_untracked();
                    async move { if let Some(e) = el { _ = e.set_focus(true).await } }
                },
                "focus"
            }
            button { "data-t": "say", onclick: move |_| _ = eval("document.title = 'x'"), "say" }
        }
    }
    let mut p = TestPage::mount(app);
    let round = |p: &mut TestPage| {
        p.click("add");
        p.click("focus");
        let id = p.scripts.last().unwrap().0;
        p.answer(id, 1, "true");
        p.click("say");
    };
    round(&mut p);
    let live = air::live();
    for _ in 0..20 {
        round(&mut p);
    }
    assert_eq!(p.text("add"), "21");
    assert_eq!(air::live(), live, "no cleanup, task or script piles up in the component per event");
}

#[test]
fn a_script_ends_with_its_owner_while_a_handle_is_kept() {
    let kept = Rc::new(RefCell::new(None::<Eval>));
    let k = kept.clone();
    let open = Signal::new(true);
    #[component]
    fn Asker(kept: Rc<RefCell<Option<Eval>>>) -> View {
        let k = kept.clone();
        once(move || *k.borrow_mut() = Some(eval("aio.send(1)")));
        rsx! { p { "asking" } }
    }
    let mut p = TestPage::mount(move || rsx! { if open.get() { Asker { kept: k.clone() } } });
    let scripts = air::live().scripts;
    p.within(|| open.set(false));
    assert_eq!(air::live().scripts, scripts - 1, "its view ended: so did the script, though a handle is kept");
    let e = kept.borrow_mut().take().unwrap();
    let got = Rc::new(RefCell::new(None));
    let g = got.clone();
    p.within(|| _ = spawn_forever(async move { *g.borrow_mut() = Some(e.recv_json().await) }));
    assert_eq!(*got.borrow(), Some(Err("the script's view ended".to_string())));
}

#[test]
fn a_selector_over_keys_that_come_and_go_keeps_only_the_buckets_read() {
    thread_local!(static NEXT: Cell<Option<Signal<u32>>> = const { Cell::new(None) });
    fn app() -> View {
        let (base, cur) = (signal(|| 0u32), signal(|| 0u32));
        NEXT.set(Some(base));
        let sel = once(move || selector(move || cur.get()));
        rsx! {
            for k in base.get()..base.get() + 10 {
                p { key: "{k}", class: if sel.is(k) { "on" } else { "" }, "{k}" }
            }
        }
    }
    let mut p = TestPage::mount(app);
    let base = NEXT.get().unwrap();
    let live = air::live();
    let mut most = 0;
    for _ in 0..100 {
        p.within(|| base.update(|b| *b += 10));
        most = most.max(air::live().nodes);
    }
    assert!(most <= live.nodes + 64, "a bucket per key ever asked piled up: {most} nodes, {} at first", live.nodes);
}

#[test]
fn a_selector_read_in_the_batch_that_moved_it_answers_the_new_key() {
    let ((cur, sel), o) = air::scope(|| {
        let cur = Signal::new(0u32);
        (cur, selector(move || cur.get()))
    });
    let seen = batch(|| {
        cur.set(1);
        (sel.is(0), sel.is(1))
    });
    assert_eq!(seen, (false, true));
    o.dispose();
}

thread_local!(static MANY: RefCell<Vec<Signal<u64>>> = const { RefCell::new(Vec::new()) });

/// The best of 5 re-runs of one hole that reads `n` signals.
fn rerun_of_a_hole_reading(n: usize) -> std::time::Duration {
    let mut p = TestPage::mount(move || {
        let sigs: Vec<Signal<u64>> = (0..n).map(|i| Signal::new(i as u64)).collect();
        MANY.set(sigs.clone());
        rsx! { p { "data-t": "sum", {sigs.iter().map(|s| s.get()).sum::<u64>().to_string()} } }
    });
    let sigs = MANY.take();
    (0..5)
        .map(|k| {
            let t = std::time::Instant::now();
            p.within(|| sigs[k].set(u64::MAX / 8));
            t.elapsed()
        })
        .min()
        .unwrap()
}

#[test]
fn a_hole_reading_many_signals_reruns_in_time_linear_in_them() {
    let (small, big) = (rerun_of_a_hole_reading(2000), rerun_of_a_hole_reading(16_000));
    // 8 times the reads: about 8 times the time (a look through the hole's
    // reads per read made it 64).
    let ratio = big.as_secs_f64() / small.as_secs_f64();
    assert!(ratio < 24.0, "{small:?} for 2000 reads, {big:?} for 16000: {ratio:.0}x");
}

#[test]
fn a_signal_read_again_after_a_memo_of_it_reruns_its_hole_once() {
    thread_local!(static RUNS: Cell<u32> = const { Cell::new(0) });
    thread_local!(static A: Cell<Option<Signal<u32>>> = const { Cell::new(None) });
    let mut p = TestPage::mount(|| {
        let a = signal(|| 1u32);
        A.set(Some(a));
        let m = memo(move || a.get() * 10);
        rsx! {
            p { "data-t": "t",
                {
                    RUNS.set(RUNS.get() + 1);
                    format!("{}-{}-{}", a.get(), m.get(), a.get())
                }
            }
        }
    });
    let a = A.get().unwrap();
    let runs = RUNS.get();
    p.within(|| a.set(2));
    assert_eq!((p.text("t"), RUNS.get() - runs), ("2-20-2".to_string(), 1));
    p.within(|| a.set(3));
    assert_eq!((p.text("t"), RUNS.get() - runs), ("3-30-3".to_string(), 2));
}

#[component]
fn Caller(go: Callback<(), String>) -> View {
    let out = signal(String::new);
    rsx! {
        button { "data-t": "call", onclick: move |_| out.set(go.call(())), "Call" }
        p { "data-t": "out", "{out}" }
    }
}

/// `callback` hands back the same handler on every run of its body (a prop
/// keeps it), calling the closure of the latest run: what that run saw.
#[test]
fn a_callback_calls_the_closure_of_its_bodys_latest_run() {
    let mut p = TestPage::mount(|| {
        let n = signal(|| 0);
        let seen = follow(|| n.get());
        let go = callback(move |()| format!("run {seen}"));
        let first = once(|| go);
        rsx! {
            button { "data-t": "inc", onclick: move |_| n += 1, "+" }
            p { "data-t": "same", "{first == go}" }
            Caller { go }
        }
    });
    p.click("call");
    assert_eq!(p.text("out"), "run 0");
    p.click("inc");
    p.click("call");
    assert_eq!(
        (p.text("out"), p.text("same")),
        ("run 1".into(), "true".into()),
        "the latest closure, the same handler"
    );
}

/// A cleanup's script belongs to no view (the one whose cleanup runs is
/// ending): it lives as long as its handle, whatever owner is current as
/// the cleanup runs (here the ending view's own: its async handler closed it).
#[test]
fn a_script_a_cleanup_runs_ends_with_its_handle_not_with_a_view() {
    thread_local!(static GOT: RefCell<Option<Result<String, String>>> = const { RefCell::new(None) });
    #[component]
    fn Bye(close: Callback) -> View {
        on_cleanup(|| {
            let e = eval("return 1");
            spawn_forever(async move { GOT.set(Some(e.join_json().await)) });
        });
        rsx! { button { "data-t": "close", onclick: move |_| async move { close.call(()) }, "Close" } }
    }
    let open = Signal::new(true);
    let mut p = TestPage::mount(move || rsx! { if open.get() { Bye { close: move |()| open.set(false) } } });
    p.click("close");
    let (id, js) = p.scripts.last().cloned().unwrap();
    assert_eq!(js, "return 1");
    p.answer(id, 1, "1");
    assert_eq!(GOT.take(), Some(Ok("1".into())), "its answer comes back");
    assert_eq!(air::live().scripts, 0, "and it is gone with its handle");
}

thread_local!(static PROC_BODIES: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) });

#[component]
fn Toggle(at: u32) -> View {
    let on = signal(|| false);
    rsx! { button { "data-t": "toggle-{at}", onclick: move |_| on.toggle(), "{on}" } }
}

#[component]
fn Proc(pid: u32, data: Option<String>) -> View {
    PROC_BODIES.with_borrow_mut(|b| b.push(pid));
    let opened = signal(|| false);
    let shown = data.unwrap_or_else(|| "no data".into());
    rsx! {
        li {
            button { "data-t": "open-{pid}", onclick: move |_| opened.set(true), "{opened}" }
            input { "data-t": "note-{pid}", oninput: move |_| {} }
            span { "data-t": "data-{pid}", "{shown}" }
            Toggle { at: pid }
        }
    }
}

/// A component whose plain props read state (`data: lookup(p.pid)`) runs
/// its body again when what they read changes and the props differ: its
/// own state, its children's and the page's nodes (a field's typed text)
/// are kept. Props that come out the same run nothing.
#[test]
fn a_keyed_component_takes_new_props_and_keeps_its_state() {
    let info = Signal::new(std::collections::HashMap::<u32, String>::new());
    let mut p = TestPage::mount(move || {
        let lookup = move |pid: u32| info.with(|m| m.get(&pid).cloned());
        rsx! { ul { for pid in [1u32, 2] { Proc { key: "{pid}", pid, data: lookup(pid) } } } }
    });
    assert_eq!(PROC_BODIES.take(), [1, 2]);
    p.click("open-1");
    p.click("toggle-1");
    p.input("note-1", "typed");
    p.within(|| info.update(|m| _ = m.insert(1, "cpu 5%".into())));
    assert_eq!((p.text("data-1"), p.text("data-2")), ("cpu 5%".into(), "no data".into()), "the new props shown");
    assert_eq!(PROC_BODIES.take(), [1], "only the row whose props changed ran again");
    assert_eq!(
        (p.text("open-1"), p.text("toggle-1")),
        ("true".into(), "true".into()),
        "its state and its child's kept"
    );
    assert_eq!(p.attr("note-1", "value").as_deref(), Some("typed"), "the page's field kept, with what was typed");
    let live = air::live();
    p.within(|| info.update(|m| _ = m.insert(3, "not shown".into())));
    assert_eq!(PROC_BODIES.take(), [] as [u32; 0], "the same props: no body runs");
    for n in 0..5 {
        p.within(|| info.update(|m| _ = m.insert(1, format!("cpu {n}%"))));
    }
    assert_eq!((p.text("data-1"), PROC_BODIES.take().len()), ("cpu 4%".into(), 5));
    assert_eq!(air::live(), live, "nothing piles up across its runs");
}

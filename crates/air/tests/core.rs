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

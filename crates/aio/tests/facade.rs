//! `aio::ui::prelude`, the view API apps are written against: each name
//! does what its air shape says, on the renderer underneath today.
use aio::testing::TestUi;
use aio::ui::prelude::*;
use std::cell::{Cell, RefCell};

thread_local! {
    static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static RENDERS: Cell<u32> = const { Cell::new(0) };
}

fn log(s: impl Into<String>) {
    LOG.with(|l| l.borrow_mut().push(s.into()));
}

fn said() -> Vec<String> {
    LOG.with(|l| std::mem::take(&mut *l.borrow_mut()))
}

static LEFT: Global<u32> = Global::new(|| 0);
static RIGHT: Global<u32> = Global::new(|| 10);
static SUM: GlobalMemo<u32> = GlobalMemo::new(|| LEFT.get() + RIGHT.get());

fn two_globals() -> View {
    rsx! {
        button { "data-t": "left", onclick: move |_| LEFT.update(|n| *n += 1), "{LEFT}" }
        p { "data-t": "right", "{RIGHT}" }
        p { "data-t": "sum", {SUM.get().to_string()} }
    }
}

#[test]
fn each_global_is_its_own_state_and_a_global_memo_follows_them() {
    let mut ui = TestUi::new(two_globals);
    ui.click("left");
    ui.click("left");
    assert_eq!((ui.text("left"), ui.text("right"), ui.text("sum")), ("2".into(), "10".into(), "12".into()));
    assert_eq!(ui.read(&LEFT), 2);
}

fn counted() -> View {
    RENDERS.set(RENDERS.get() + 1);
    let mut n = signal(|| 1);
    rsx! {
        button { "data-t": "same", onclick: move |_| n.set(1), "same" }
        button { "data-t": "next", onclick: move |_| n += 1, "next" }
        button { "data-t": "write", onclick: move |_| *n.write() = 1, "write" }
        p { "data-t": "n", "{n}" }
    }
}

#[test]
fn set_tells_only_on_a_change_and_write_always_does() {
    let mut ui = TestUi::new(counted);
    let at = RENDERS.get();
    ui.click("same");
    assert_eq!(RENDERS.get(), at, "the value it had: nothing renders");
    ui.click("next");
    assert_eq!((RENDERS.get(), ui.text("n")), (at + 1, "2".into()));
    ui.click("write");
    ui.click("write");
    assert_eq!((RENDERS.get(), ui.text("n")), (at + 3, "1".into()), "a write guard always tells");
}

#[component]
fn Shown(n: u32) -> View {
    let made = once(move || {
        log(format!("made {n}"));
        n
    });
    watch(n, |n| log(format!("watched {n}")));
    on_cleanup(|| log("gone"));
    rsx! { p { "data-t": "shown", "{n} (first {made})" } }
}

fn lifetime() -> View {
    let mut n = signal(|| 1u32);
    let open = signal(|| true);
    rsx! {
        button { "data-t": "more", onclick: move |_| n += 1, "more" }
        button { "data-t": "close", onclick: move |_| open.toggle(), "close" }
        if open.get() {
            Shown { n: n.get() }
        }
    }
}

#[test]
fn once_is_made_once_watch_follows_its_deps_and_on_cleanup_ends_it() {
    let mut ui = TestUi::new(lifetime);
    assert_eq!(said(), ["made 1", "watched 1"]);
    ui.click("more");
    assert_eq!(ui.text("shown"), "2 (first 1)");
    assert_eq!(said(), ["watched 2"], "new props: the watch runs, once does not");
    ui.click("close");
    assert_eq!(said(), ["gone"]);
}

#[derive(Clone, PartialEq)]
struct Theme(&'static str);

#[component]
fn Themed() -> View {
    let t = context::<Theme>();
    let none = try_context::<u8>().is_none();
    rsx! { p { "data-t": "theme", "{t.0} {none}" } }
}

fn provider() -> View {
    provide(|| Theme("dark"));
    rsx! { Themed {} }
}

#[test]
fn a_view_reads_what_one_above_provided() {
    let ui = TestUi::new(provider);
    assert_eq!(ui.text("theme"), "dark true");
}

#[component]
fn Fails(fail: bool) -> View {
    if fail {
        Err(std::io::Error::other("the disk is gone"))?;
    }
    rsx! { p { "data-t": "fine", "fine" } }
}

fn guarded() -> View {
    let fail = signal(|| false);
    rsx! {
        button { "data-t": "break", onclick: move |_| fail.set(true), "break" }
        ErrorBoundary { fallback: |e: String| rsx! { p { "data-t": "caught", "{e}" } },
            Fails { fail: fail.get() }
        }
    }
}

#[test]
fn an_error_thrown_in_a_view_shows_the_nearest_boundarys_fallback() {
    let mut ui = TestUi::new(guarded);
    assert!(ui.present("fine"));
    ui.click("break");
    assert_eq!(ui.text("caught"), "the disk is gone");
    assert!(ui.absent("fine"));
}

thread_local! {
    static GATE: RefCell<Option<tokio::sync::oneshot::Receiver<u32>>> = const { RefCell::new(None) };
}

#[component]
fn Loads() -> View {
    let r = resource(|| async {
        let rx = GATE.with_borrow_mut(Option::take).expect("the test's gate");
        rx.await.unwrap_or(0)
    });
    let v = r.suspend()?;
    rsx! { p { "data-t": "loaded", "{v}" } }
}

fn waiting() -> View {
    rsx! {
        Suspense { fallback: rsx! { p { "data-t": "loading", "…" } },
            Loads {}
        }
    }
}

#[test]
fn suspense_shows_its_fallback_until_the_resource_is_loaded() {
    let (tx, rx) = tokio::sync::oneshot::channel();
    GATE.set(Some(rx));
    let mut ui = TestUi::new(waiting);
    assert!(ui.present("loading") && ui.absent("loaded"));
    tx.send(7).unwrap();
    ui.until_text("loaded", "7");
    assert!(ui.absent("loading"));
}

fn scripted() -> View {
    let got = signal(|| 0i64);
    task(move || async move {
        let mut e = eval("aio.send(41 + 1); const back = await aio.recv(); aio.send(back * 2);");
        if let Ok(v) = e.recv::<i64>().await {
            got.set(v);
            _ = e.send(got.get_untracked());
        }
        if let Ok(v) = e.recv::<i64>().await {
            got.set(v);
        }
    });
    rsx! { p { "data-t": "got", "{got}" } }
}

#[test]
fn a_page_script_talks_to_its_view_through_aio() {
    let mut ui = TestUi::new(scripted);
    ui.until_text("got", "84");
}

thread_local! {
    static LATER: RefCell<Option<tokio::sync::oneshot::Receiver<i64>>> = const { RefCell::new(None) };
}

fn waits() -> View {
    let got = signal(|| 0i64);
    task(move || async move {
        let mut e = eval("const v = await aio.recv(); const w = await aio.recv(); aio.send(v * 2 + w);");
        let v = LATER.with_borrow_mut(Option::take).expect("the test's gate").await.unwrap_or(0);
        _ = e.send(v);
        _ = e.send(0);
        if let Ok(v) = e.recv::<i64>().await {
            got.set(v);
        }
    });
    rsx! { p { "data-t": "got", "{got}" } }
}

#[test]
fn a_page_script_waits_for_a_value_sent_late_and_takes_a_falsy_one() {
    let (tx, rx) = tokio::sync::oneshot::channel();
    LATER.set(Some(rx));
    let mut ui = TestUi::new(waits);
    // Longer than the renderer's own `recv` looked before it held the page.
    std::thread::sleep(std::time::Duration::from_millis(200));
    tx.send(21).unwrap();
    ui.until_text("got", "42");
}

static KEPT: Global<Option<Signal<u32>>> = Global::new(|| None);

#[component]
fn Owner() -> View {
    let mine = signal(|| 5u32);
    once(move || KEPT.set(Some(mine)));
    rsx! { p { "data-t": "owner", "{mine}" } }
}

fn outlived() -> View {
    let open = signal(|| true);
    rsx! {
        button { "data-t": "drop", onclick: move |_| open.set(false), "drop" }
        if open.get() {
            Owner {}
        }
    }
}

#[test]
fn a_signal_outlived_by_its_view_says_so_instead_of_panicking() {
    let mut ui = TestUi::new(outlived);
    let s = ui.read(&KEPT).expect("the owner kept its signal");
    assert!(ui.run(|| s.alive() && s.try_read_untracked().is_some_and(|v| *v == 5)));
    ui.run(|| *s.try_write().expect("alive") = 6);
    assert_eq!(ui.text("owner"), "6");
    ui.click("drop");
    assert!(ui.run(|| !s.alive() && s.try_write().is_none() && s.try_read_untracked().is_none()));
}

#[test]
fn the_views_run_here_only_inside_the_app_and_a_key_from_elsewhere_reads_as_one() {
    assert!(!in_views(), "a test's own code");
    let mut ui = TestUi::new(two_globals);
    assert!(ui.run(in_views));
    let e = key_event(Key::ArrowDown, Modifiers::SHIFT, true);
    assert_eq!((e.key(), e.modifiers(), e.is_auto_repeating()), (Key::ArrowDown, Modifiers::SHIFT, true));
}

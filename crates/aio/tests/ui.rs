//! Views re-render only for the parts they read (AIR-level updates).
use aio::act;
use aio::prelude::*;
use aio::testing::{TestUi, fails};
use dioxus::prelude::*;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

#[cell]
pub struct Pair {
    pub left: u32,
    pub right: u32,
}

#[methods]
impl Pair {
    pub fn bump_left(&mut self) {
        self.left += 1;
    }

    pub fn bump_right(&mut self) {
        self.right += 1;
    }

    pub fn full(&mut self) -> aio::Result {
        reject!("no room left")
    }

    #[coalesce]
    pub fn set_left(&mut self, n: u32) {
        self.left = n;
    }
}

aio::client!(Pair, pair, pair_at);

static LEFT: AtomicUsize = AtomicUsize::new(0);
static RIGHT: AtomicUsize = AtomicUsize::new(0);
static WHOLE: AtomicUsize = AtomicUsize::new(0);

#[component]
fn Left() -> Element {
    LEFT.fetch_add(1, SeqCst);
    let n = pair_at(|p| p.left);
    rsx! { span { "data-t": "left", "{n}" } }
}

#[component]
fn Right() -> Element {
    RIGHT.fetch_add(1, SeqCst);
    let n = pair_at(|p| p.right);
    rsx! { span { "data-t": "right", "{n}" } }
}

#[component]
fn Whole() -> Element {
    WHOLE.fetch_add(1, SeqCst);
    let p = pair();
    rsx! { span { "{p.left + p.right}" } }
}

fn app() -> Element {
    use_live();
    rsx! {
        Left {}
        Right {}
        Whole {}
    }
}

#[test]
fn a_view_re_renders_only_when_its_part_changes() {
    let mut ui = TestUi::new(app);
    let counts = || (LEFT.load(SeqCst), RIGHT.load(SeqCst), WHOLE.load(SeqCst));
    let (l, r, w) = counts();
    ui.run(|| act!(bump_left()));
    ui.settle();
    assert_eq!(ui.text("left"), "1");
    assert_eq!(counts(), (l + 1, r, w + 1), "the right view didn't re-render; the whole-state reader did");
    // A commit made off the UI (an effect, a synced copy) is the same.
    let h = ui.run(|| HANDLE.peek().clone());
    std::thread::spawn(move || h.bump_right().unwrap()).join().unwrap();
    ui.until("the commit is seen", || RIGHT.load(SeqCst) > r);
    assert_eq!(ui.text("right"), "1");
    assert_eq!(counts(), (l + 1, r + 1, w + 2));
}

/// A view reading through the door (`Ui.read`, as an app's `with(|s| ..)`).
#[component]
fn ByDoor() -> Element {
    let n = aio::Door::read(&Ui, |p| p.right);
    rsx! { span { "data-t": "door", "{n}" } }
}

fn by_door() -> Element {
    use_live();
    rsx! { ByDoor {} }
}

#[test]
fn a_view_reading_through_the_door_follows_every_commit() {
    let mut ui = TestUi::new(by_door);
    ui.run(|| act!(bump_right()));
    assert_eq!(ui.text("door"), "1", "a call from the views");
    let h = ui.run(|| HANDLE.peek().clone());
    std::thread::spawn(move || h.bump_right().unwrap()).join().unwrap();
    ui.until_text("door", "2");
}

thread_local!(static QUIET: std::cell::Cell<u32> = const { std::cell::Cell::new(0) });

/// The whole state, read in a render (counted on this test's thread).
#[component]
fn Quiet() -> Element {
    QUIET.set(QUIET.get() + 1);
    let p = pair(); // the hint names this line
    rsx! { span { "data-t": "sum", "{p.left + p.right}" } }
}

fn quiet() -> Element {
    use_live();
    rsx! { Quiet {} }
}

#[test]
fn a_call_that_changes_nothing_re_renders_nothing() {
    let mut ui = TestUi::new(quiet);
    ui.run(|| act!(set_left(3)));
    ui.settle();
    let n = QUIET.get();
    for _ in 0..5 {
        ui.run(|| act!(set_left(3)));
    }
    ui.settle();
    assert_eq!(QUIET.get(), n, "Down on the last row: no render");
    assert_eq!(ui.text("sum"), "3");
}

#[test]
fn a_dev_build_says_where_a_render_reads_the_whole_state_once() {
    let (mut ui, lines) = aio::log::capture(|| {
        let mut ui = TestUi::new(quiet);
        ui.run(|| act!(bump_left()));
        ui.settle();
        ui
    });
    let hints: Vec<_> = lines.iter().filter(|l| l.contains("reads all of Pair")).collect();
    assert_eq!(hints.len(), 1, "once: {lines:?}");
    let line = include_str!("ui.rs").lines().position(|l| l.ends_with("// the hint names this line")).unwrap();
    let at = format!("tests/ui.rs:{}:", line + 1);
    assert!(hints[0].contains(&at) && hints[0].contains("part(|c| &c.field)"), "{at}: {}", hints[0]);
    ui.settle();
}

fn shows() -> Element {
    let mut on = use_signal(|| false);
    rsx! {
        button { "data-t": "toggle", onclick: move |_| on.toggle(), "toggle" }
        if on() {
            p { "data-t": "advice", "advice" }
        }
        p { "data-t": "styled", style: "display: none", "x" }
        p { "data-t": "attr", hidden: true, "x" }
        div { style: "display: none", p { "data-t": "inside", "x" } }
        p { "data-t": "ghost", style: "visibility: hidden", "x" }
        p { "data-t": "twice", style: "display: none", "x" }
        p { "data-t": "twice", "x" }
    }
}

#[test]
fn present_means_showing() {
    let mut ui = TestUi::new(shows);
    assert!(ui.absent("advice") && !ui.present("advice"), "not rendered yet");
    ui.click("toggle");
    assert!(ui.present("advice"));
    ui.click("toggle");
    assert!(ui.absent("advice"), "gone again");
    for t in ["styled", "attr", "inside", "ghost"] {
        assert!(ui.absent(t), "{t} is in the document but hidden");
    }
    assert!(ui.absent("nothing-by-that-name"));
    assert!(ui.present("twice"), "one of two is showing");
}

#[test]
fn a_hidden_element_cannot_be_clicked() {
    let why = fails(|| {
        TestUi::new(shows).click("inside");
    });
    assert!(
        why.contains("data-t=\"inside\" can't be used: it is hidden (display: none on an enclosing <div>)"),
        "{why}"
    );
}

#[test]
fn a_missing_handle_lists_the_ones_there() {
    let why = fails(|| {
        TestUi::new(shows).click("advise");
    });
    assert!(
        why.contains(
            "no element with data-t=\"advise\"; the handles there are: attr, ghost, inside, styled, toggle, twice"
        ),
        "{why}"
    );
}

static LABELLED: AtomicUsize = AtomicUsize::new(0);

#[component]
fn Labelled(t: &'static str) -> Element {
    LABELLED.fetch_add(1, SeqCst);
    let id = use_hook(aio::ui::unique_id);
    let left = pair().left;
    rsx! {
        label { r#for: "{id}", "data-t": t, "{id} {left}" }
        input { id: "{id}" }
    }
}

fn local() -> Element {
    use_live();
    let s = use_status()();
    let p = pair();
    rsx! {
        p { "data-t": "link", "{s.online} {s.pending} {s.degraded}" }
        p { "data-t": "n", "n={p.left}" }
        Labelled { t: "a" }
        Labelled { t: "b" }
    }
}

#[test]
fn a_local_cell_is_online_and_its_state_is_there_at_the_first_render() {
    let mut ui = TestUi::new(local);
    // aio's `ready` gate: nothing to wait for, on the first frame or after a call.
    assert_eq!(ui.text("n"), "n=0");
    assert_eq!(ui.text("link"), "true 0 false");
    ui.run(|| act!(bump_left()));
    assert_eq!(ui.text("n"), "n=1");
    assert_eq!(ui.text("link"), "true 0 false");
}

#[test]
fn ids_are_unique_and_stable_per_instance() {
    let mut ui = TestUi::new(local);
    let id = |ui: &TestUi, t| ui.text(t).split(' ').next().unwrap().to_string();
    let (a, b) = (id(&ui, "a"), id(&ui, "b"));
    assert_ne!(a, b);
    assert!(a.starts_with("aio-"), "{a}");
    let renders = LABELLED.load(SeqCst);
    ui.run(|| act!(bump_left()));
    assert!(LABELLED.load(SeqCst) > renders, "re-rendered");
    assert_eq!(ui.text("a"), format!("{a} 1"), "same id after a re-render");
    assert_eq!(id(&ui, "b"), b);
    // Another app instance (window) never reuses one.
    let other = TestUi::new(local);
    assert!(![a, b].contains(&id(&other, "a")));
}

#[cfg(feature = "server")]
mod synced {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use tokio_tungstenite::tungstenite;

    #[cell(shared)]
    pub struct Notes {
        pub n: u32,
    }

    #[methods]
    impl Notes {
        pub fn add(&mut self) {
            self.n += 1;
        }
    }

    aio::client!(Notes, notes);

    static ONLINE: AtomicBool = AtomicBool::new(false);

    fn app() -> Element {
        use_live();
        let s = use_status()();
        ONLINE.store(s.online, SeqCst);
        rsx! { p { "data-t": "link", "{s.online} {s.pending} {s.degraded}" } }
    }

    #[test]
    fn the_view_shows_calls_waiting_and_the_link_going_down() {
        // A fake server: the state, then silence until it hangs up.
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let (hang_up, told) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            let mut w = tungstenite::accept(l.accept().unwrap().0).unwrap();
            w.read().unwrap();
            let st = serde_json::json!({ "t": "state", "seq": 0, "state": Notes::default() });
            w.send(tungstenite::Message::text(st.to_string())).unwrap();
            _ = told.recv();
        });
        assert!(SOURCE.set(aio::Source::connect(&addr, "t", "u").unwrap()).is_ok());
        let mut ui = TestUi::new(app);
        assert_eq!(ui.text("link"), "true 0 false");
        ui.run(|| act!(add()));
        assert_eq!(ui.text("link"), "true 1 false", "sent, not confirmed");
        ui.run(|| (0..400).for_each(|_| _ = act!(add())));
        assert_eq!(ui.text("link"), "true 401 true", "past 80% of PENDING_MAX");
        hang_up.send(()).unwrap();
        ui.until("the link is down", || !ONLINE.load(SeqCst));
        assert_eq!(ui.text("link"), "false 401 true");
    }
}

fn controls() -> Element {
    rsx! {
        a { "data-t": "link", href: "/docs", "aria-pressed": "true", "docs" }
        input { "data-t": "box", r#type: "checkbox", checked: true }
        input { "data-t": "off", r#type: "checkbox", disabled: true }
        fieldset { disabled: true,
            input { "data-t": "fenced", r#type: "radio" }
        }
        input { "data-t": "plain" }
    }
}

#[test]
fn attributes_checked_and_disabled_read_as_a_person_sees_them() {
    let mut ui = TestUi::new(controls);
    assert_eq!(ui.attr("link", "href").as_deref(), Some("/docs"));
    assert_eq!(ui.attr("link", "aria-pressed").as_deref(), Some("true"));
    assert_eq!(ui.attr("link", "title"), None);
    assert!(ui.checked("box") && !ui.checked("fenced"));
    ui.click("box");
    assert!(!ui.checked("box"), "the click unchecked it");
    assert!(ui.disabled("off") && ui.disabled("fenced"), "its own, or its fieldset's");
    assert!(!ui.disabled("box") && !ui.disabled("plain"));
}

#[test]
fn checked_on_a_text_field_says_so() {
    let why = fails(|| {
        TestUi::new(controls).checked("plain");
    });
    assert!(why.contains("data-t=\"plain\" is no checkbox or radio"), "{why}");
}

mod spinner {
    use super::*;

    static GATE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(0);
    static SHOWN: AtomicUsize = AtomicUsize::new(usize::MAX);

    #[cell]
    pub struct Scan {
        pub found: u32,
    }

    #[methods]
    impl Scan {
        pub async fn scan(&mut self) -> aio::Result {
            let n: u32 = async {
                GATE.acquire().await.expect("open").forget();
                1
            }
            .await;
            self.found += n;
            Ok(())
        }
    }

    aio::client!(Scan, scan_state);

    fn app() -> Element {
        use_live();
        let n = use_pending(Some("scan"))();
        SHOWN.store(n, SeqCst);
        rsx! { p { "data-t": "busy", "{n}" } }
    }

    #[test]
    fn a_spinner_follows_the_calls_in_flight() {
        let mut ui = TestUi::new(app);
        assert_eq!(ui.text("busy"), "0");
        ui.run(|| act!(scan()));
        ui.until("the call shows", || SHOWN.load(SeqCst) == 1);
        ui.run(|| act!(scan()));
        ui.until("two calls show", || SHOWN.load(SeqCst) == 2);
        GATE.add_permits(2);
        ui.until("both are done", || SHOWN.load(SeqCst) == 0);
        assert_eq!(ui.text("busy"), "0");
        assert_eq!(ui.run(scan_state).found, 2);
    }
}

fn refusing() -> Element {
    use_live();
    rsx! { button { "data-t": "add", onclick: move |_| _ = act!(full()), "Add" } }
}

#[test]
fn a_refusal_nobody_looked_at_fails_the_test() {
    let why = fails(|| {
        TestUi::new(refusing).click("add");
    });
    assert!(why.contains("refused and never looked at (take them with `failures`): full: no room left"), "{why}");
}

#[test]
fn a_refusal_taken_is_looked_at() {
    let mut ui = TestUi::new(refusing);
    ui.click("add");
    ui.click("add");
    assert_eq!(ui.failures(), ["full: no room left", "full: no room left"]);
    assert!(ui.failures().is_empty(), "taken");
}

fn duplicated() -> Element {
    let mut rows = use_signal(|| vec![1, 2]);
    rsx! {
        button { onclick: move |_| rows.set(vec![1, 1]), "Dup" }
        for r in rows() {
            p { key: "{r}", "{r}" }
        }
    }
}

/// aio's duplicate-key tripwire: a debug build refuses keyed siblings that
/// share a key (Dioxus's own check), once the list changes.
#[cfg(debug_assertions)]
#[test]
fn siblings_with_one_key_are_refused() {
    let why = fails(|| {
        TestUi::new(duplicated).click("DupButton");
    });
    assert!(why.contains("keyed siblings must each have a unique key"), "{why}");
}

/// A flake seen in an app: a commit made off the UI thread is readable before
/// the view is told of it, so `until` saw the state and the next `click`
/// hit the screen from before it. `until` and every action now wait for
/// each commit's listeners, so the screen has caught up.
mod gate {
    use super::*;

    #[cell]
    pub struct Gate {
        pub open: bool,
    }

    #[methods]
    impl Gate {
        pub fn open(&mut self) {
            self.open = true;
        }

        pub fn close(&mut self) {
            self.open = false;
        }
    }

    aio::client!(Gate, gate);

    fn app() -> Element {
        use_live();
        rsx! {
            if gate().open {
                button { "data-t": "go", onclick: move |_| _ = act!(close()), "go" }
            }
        }
    }

    #[test]
    fn a_click_after_an_off_thread_change_meets_the_screen_it_made() {
        // A listener told first that takes its time: the view hears of
        // each commit 50 ms after its state is readable.
        let slow = || HANDLE.peek().listen(|_| std::thread::sleep(std::time::Duration::from_millis(50)));
        let mut ui = aio::testing::Mount::default().before(slow).app(app);
        for _ in 0..3 {
            let h = ui.run(|| HANDLE.peek().clone());
            let t = std::thread::spawn(move || h.open().unwrap());
            ui.until("open", || gate().open);
            ui.click("go");
            ui.until("closed", || !gate().open);
            assert!(ui.absent("go"));
            t.join().unwrap();
        }
    }
}

#[component]
fn Pictures(port: u16) -> Element {
    rsx! {
        img { alt: "far", src: "https://example.invalid/far.png" }
        img { alt: "near", src: "http://127.0.0.1:{port}/near.png" }
    }
}

/// aio's `testUI`: a UI test refuses the real network. A request elsewhere is
/// cancelled and fails the test unless taken; this machine is reached.
#[test]
fn a_ui_test_refuses_the_real_network_and_reaches_this_machine() {
    use std::io::Read;
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        if let Ok((mut c, _)) = l.accept() {
            let mut b = [0; 256];
            let n = c.read(&mut b).unwrap_or(0);
            _ = tx.send(String::from_utf8_lossy(&b[..n]).into_owned());
        }
    });
    let mut ui = TestUi::with(Pictures, PicturesProps { port });
    let near = rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap();
    assert!(near.starts_with("GET /near.png"), "{near}");
    let mut refused = vec![];
    for _ in 0..200 {
        refused.extend(ui.failures());
        if !refused.is_empty() {
            break;
        }
        ui.wait(std::time::Duration::from_millis(25));
    }
    assert_eq!(
        refused,
        [
            "network: the page asked for https://example.invalid/far.png: a UI test refuses the real network (allow its host with Mount::network)"
        ]
    );
}

static HEARD: AtomicUsize = AtomicUsize::new(0);

/// Two buttons whose box keeps Tab and ArrowDown, one outside it, and a
/// list whose cursor row is revealed by the page.
fn keyed() -> Element {
    let mut at = use_signal(|| 0usize);
    rsx! {
        div { "data-aio-prevent": "Tab ArrowDown",
            onkeydown: move |e| {
                if e.key() == Key::ArrowDown {
                    at += 1;
                }
                HEARD.fetch_add(1, SeqCst);
            },
            button { "data-t": "a", "A" }
            button { "data-t": "b", "B" }
            input { "data-t": "field" }
        }
        button { "data-t": "out", "Out" }
        button { "data-t": "far", onclick: move |_| at.set(80), "Far" }
        div { "data-t": "box", style: "height: 100px; overflow: auto",
            for i in 0..100usize {
                div { key: "{i}", style: "height: 20px", "data-aio-reveal": (i == at()).then(|| i.to_string()), "row {i}" }
            }
        }
    }
}

#[test]
fn the_page_keeps_the_keys_an_element_names_and_the_view_still_hears_them() {
    use keyboard_types::{Key, Modifiers};
    let mut ui = TestUi::new(keyed);
    ui.eval("document.querySelector('[data-t=a]').focus()");
    let before = HEARD.load(SeqCst);
    ui.press_key(Key::Tab, Modifiers::empty());
    ui.settle();
    assert_eq!(ui.count("[data-t=a]:focus"), 1, "Tab moved no focus inside the box");
    assert!(HEARD.load(SeqCst) > before, "and the view heard it");
    let prevented = ui.eval(
        "(() => { const e = new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true, cancelable: true }); \
         document.querySelector('[data-t=b]').dispatchEvent(e); return e.defaultPrevented; })()",
    );
    assert_eq!(prevented, serde_json::json!(true), "an arrow doesn't scroll from there");
    ui.eval("document.querySelector('[data-t=field]').focus()");
    ui.press_key(Key::Tab, Modifiers::empty());
    ui.settle();
    assert_eq!(ui.count("[data-t=out]:focus"), 1, "a field keeps its own Tab");
    ui.press_key(Key::Tab, Modifiers::empty());
    ui.settle();
    assert_eq!(ui.count("[data-t=far]:focus"), 1, "outside the box Tab is the browser's");
}

#[test]
fn the_row_with_the_cursor_is_scrolled_into_view_without_a_script_from_the_app() {
    let mut ui = TestUi::new(keyed);
    assert_eq!(ui.eval("document.querySelector('[data-t=box]').scrollTop"), serde_json::json!(0));
    ui.click("far");
    ui.settle();
    let top = ui.eval("document.querySelector('[data-t=box]').scrollTop").as_f64().unwrap();
    assert!((1520.0..=1600.0).contains(&top), "row 80 at the box's bottom edge: {top}");
    ui.eval("document.querySelector('[data-t=a]').focus()");
    ui.press_key(keyboard_types::Key::ArrowDown, keyboard_types::Modifiers::empty());
    ui.settle();
    let next = ui.eval("document.querySelector('[data-t=box]').scrollTop").as_f64().unwrap();
    assert!((next - top - 20.0).abs() < 1.0, "one row on: {top} → {next}");
}

/// `part`: a view follows the one field it reads (aio's `trackCell`), and
/// commits made off the UI thread render at most once per `BACKGROUND`.
mod parts {
    use super::*;

    #[cell]
    pub struct Board {
        pub rows: Vec<String>,
        pub cursor: u32,
        pub ticks: u32,
    }

    #[methods]
    impl Board {
        pub fn down(&mut self) {
            self.cursor += 1;
        }

        pub fn add(&mut self, row: String) {
            self.rows.push(row);
        }

        pub fn tick(&mut self) {
            self.ticks += 1;
        }
    }

    aio::client!(Board, board);

    thread_local! {
        static CURSOR: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
        static ROWS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
        static FIRST: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
        static TICKS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
        static VER: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    }

    #[component]
    fn Cursor() -> Element {
        CURSOR.set(CURSOR.get() + 1);
        let at = part(|b| &b.cursor);
        rsx! { span { "data-t": "cursor", "{at}" } }
    }

    #[component]
    fn Rows() -> Element {
        ROWS.set(ROWS.get() + 1);
        let rows = part(|b| &b.rows);
        VER.set(rows.ver());
        rsx! { span { "data-t": "rows", "{rows.len()}" } }
    }

    /// Into the list's heap: no field to name, so it follows every commit.
    #[component]
    fn First() -> Element {
        FIRST.set(FIRST.get() + 1);
        let first = part(|b| b.rows.first().map_or("", |r| r.as_str()));
        rsx! { span { "data-t": "first", "{first}" } }
    }

    #[component]
    fn Ticks() -> Element {
        TICKS.set(TICKS.get() + 1);
        let n = part(|b| &b.ticks);
        rsx! { span { "data-t": "ticks", "{n}" } }
    }

    fn app() -> Element {
        use_live();
        rsx! {
            Cursor {}
            Rows {}
            First {}
            Ticks {}
        }
    }

    #[test]
    fn a_view_re_renders_only_for_the_field_it_reads() {
        let mut ui = TestUi::new(app);
        let counts = || (CURSOR.get(), ROWS.get(), FIRST.get());
        let ((c, r, f), v) = (counts(), VER.get());
        for _ in 0..5 {
            ui.run(|| act!(down()));
        }
        assert_eq!(ui.text("cursor"), "5");
        assert_eq!(counts(), (c + 5, r, f + 5), "the rows' reader stayed; the heap reader followed");
        assert_eq!(VER.get(), v, "the rows' stamp holds while they don't change");
        ui.run(|| act!(add("one".into())));
        assert_eq!((ui.text("rows"), ui.text("first")), ("1".into(), "one".into()));
        assert_eq!(counts(), (c + 5, r + 1, f + 6), "the cursor's reader stayed");
        assert_ne!(VER.get(), v, "a new stamp for new rows");
    }

    #[test]
    fn commits_off_the_ui_thread_render_at_most_once_a_background_window() {
        let mut ui = TestUi::new(app);
        let h = ui.run(|| HANDLE.peek().clone());
        let t0 = (TICKS.get(), std::time::Instant::now());
        let feed = h.clone();
        std::thread::spawn(move || {
            for _ in 0..40 {
                feed.tick().unwrap();
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        })
        .join()
        .unwrap();
        ui.until_text("ticks", "40");
        let (renders, took) = (TICKS.get() - t0.0, t0.1.elapsed());
        let most = (took.as_millis() / aio::ui::BACKGROUND.as_millis()) as u32 + 2;
        assert!(renders <= most, "{renders} renders for 40 ticks in {took:?} (at most {most})");
        // A call from the views renders at once, with what came meanwhile.
        h.tick().unwrap();
        ui.run(|| act!(down()));
        assert_eq!((ui.text("ticks"), ui.text("cursor")), ("41".into(), "1".into()));
    }
}

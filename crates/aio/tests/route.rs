//! The router in a view: typed pages, links, Back (aio tests/router.test.ts,
//! router-navlink-outlet, router-link-class-and-onclick, standalone-router).
//! The same views on either renderer.
#![cfg(feature = "kit")]
use aio::testing::TestUi;
use aio::ui::prelude::*;

aio::router!();

aio::routes! {
    enum Page {
        Home = "/",
        User { id: u32 } = "/users/:id",
        Files { path: String } = "/files/*path",
    }
}

#[component]
fn Dialog() -> View {
    let open = signal(|| true);
    use_back(move || {
        let was = open.get_untracked();
        open.set(false);
        was
    });
    rsx! {
        if open.get() {
            p { "data-t": "dialog", "open" }
        }
    }
}

fn app() -> View {
    let closes = signal(|| 0);
    rsx! {
        // Links as blocks: each a whole row to click.
        style { "a {{ display: block }}" }
        nav {
            Link { to: "/", exact: true, "data-t": "home", "Home" }
            Link { to: Page::User { id: 7 }.href(), class: "nav", "data-t": "user", "User 7" }
            Link { to: "/files/a b/c.md", replace: true, "data-t": "file", "File" }
            Link { to: "/users/8", onclick: move |e: MouseEvent| { e.prevent_default(); closes.update(|c| *c += 1); }, "data-t": "kept", "Menu" }
            Link { to: "https://example.com/", "data-t": "out", "Out" }
        }
        p { "data-t": "closes", "{closes}" }
        // The page follows the place where it is drawn (a hole on air).
        {
            match Page::parse(&here().path) {
                Some(Page::Home) => rsx! { p { "data-t": "page", "home" } },
                Some(Page::User { id }) => rsx! { p { "data-t": "page", "user {id}" } Dialog {} },
                Some(Page::Files { path }) => rsx! { p { "data-t": "page", "file {path}" } },
                None => rsx! { p { "data-t": "page", "not found" } },
            }
        }
    }
}

#[test]
fn typed_pages_parse_and_link_back() {
    assert_eq!(Page::parse("/users/42/"), Some(Page::User { id: 42 }));
    assert_eq!(Page::parse("/users/x"), None, "a param that doesn't parse is no match");
    assert_eq!(Page::parse("/files/a%20b/c.md"), Some(Page::Files { path: "a b/c.md".into() }));
    assert_eq!(Page::Files { path: "a b/c.md".into() }.href(), "/files/a%20b/c.md");
    assert_eq!(Page::Home.href(), "/");
}

#[test]
fn links_navigate_in_the_app_and_back_retraces_them() {
    let mut ui = TestUi::new(app);
    assert_eq!(ui.text("page"), "home");
    assert_eq!(ui.count("a.active"), 1, "the home link is the current place's");
    ui.click("user");
    assert_eq!(ui.text("page"), "user 7");
    assert_eq!(ui.count("a.nav.active[aria-current=page]"), 1, "its own class kept, the active one added");
    assert_eq!(ui.count("a.active"), 1, "home is exact: not active under /users");
    // The author's onclick runs first, and its prevent_default keeps the page.
    ui.click("kept");
    assert_eq!((ui.text("closes"), ui.text("page")), ("1".into(), "user 7".into()));
    // A link out of the app is left to the system browser.
    ui.click("out");
    assert_eq!(ui.text("page"), "user 7");
    assert_eq!(ui.count("a[target=_blank][href='https://example.com/']"), 1);
    // A dialog over the page takes Back first; then history does.
    assert!(ui.present("dialog"));
    assert!(ui.run(back));
    assert!(ui.absent("dialog"));
    assert_eq!(ui.text("page"), "user 7");
    assert!(ui.run(back));
    assert_eq!(ui.text("page"), "home");
    assert!(!ui.run(back), "the first place: the shell leaves the app");
    // A redirecting link leaves no Back step; a deep link starts anywhere.
    ui.click("user");
    ui.click("file");
    assert_eq!(ui.text("page"), "file a b/c.md");
    ui.run(back);
    assert_eq!(ui.text("page"), "home");
    ui.run(|| NAV.write().replace("/nowhere"));
    assert_eq!(ui.text("page"), "not found");
    // The host's Back (Android's button, a mouse's back key) is the same
    // Back: handlers, then history, then the host's default. In this test,
    // not its own: the host's Back is one per process.
    let press = |ui: &mut TestUi| {
        use futures_util::FutureExt;
        let asked = aio::ui::back_pressed();
        ui.settle();
        asked.now_or_never().expect("answered by the router")
    };
    ui.run(|| NAV.write().replace("/"));
    ui.click("user");
    assert!(press(&mut ui));
    assert!(ui.absent("dialog"), "the dialog's handler first");
    assert!(press(&mut ui));
    assert_eq!(ui.text("page"), "home", "then history");
    assert!(!press(&mut ui), "at the first place: the host's default (leave)");
    // A link the app is opened with (Android's deep link) goes there, with a
    // Back step to where it was; one that leaves the app goes nowhere.
    aio::ui::open_link("myapp://users/8");
    ui.settle();
    assert_eq!(ui.text("page"), "user 8");
    aio::ui::open_link("myapp:https://example.com/");
    ui.settle();
    assert_eq!(ui.text("page"), "not found", "a scheme's path, never another site");
    ui.run(back);
    assert_eq!(ui.text("page"), "user 8", "Back retraces the links");
    ui.run(back);
    assert!(ui.absent("dialog"), "its dialog takes Back first");
    ui.run(back);
    assert_eq!(ui.text("page"), "home");
}

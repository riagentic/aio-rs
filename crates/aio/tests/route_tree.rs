//! A tree of routes in a view: layouts, outlets, index routes, params,
//! redirects, and a page's title (aio tests/router-navlink-outlet,
//! air-router-path-matching). Its own process: the host's Back in
//! tests/route.rs is one per process, asked of every live router.
#![cfg(feature = "kit")]
use aio::testing::TestUi;
use aio::ui::prelude::*;

aio::router!();

aio::routes! {
    enum Titled {
        Home = "/" => "Home",
        User { id: u32 } = "/users/:id" => "User {id}",
        Plain = "/plain",
    }
}

#[test]
fn a_page_names_its_title_with_its_fields() {
    assert_eq!(Titled::Home.head(), aio::head::Head::title("Home"));
    assert_eq!(Titled::parse("/users/7").unwrap().head().title.as_deref(), Some("User 7"));
    assert_eq!(Titled::Plain.head(), aio::head::Head::default(), "none: the page's own");
}

#[component]
fn Layout(name: String) -> View {
    rsx! {
        section { "data-t": "{name}", h1 { "{name}" } Outlet {} }
    }
}

#[component]
fn Who() -> View {
    let ps = params();
    rsx! { p { "data-t": "who", "{ps:?}" } }
}

fn tree() -> View {
    rsx! {
        div { "data-t": "bare", Outlet {} }
        Route { path: "/dash", element: rsx! { Layout { name: "dash" } },
            Route { index: true, element: rsx! { p { "data-t": "page", "dash index" } } }
            Route { path: "settings", element: rsx! { p { "data-t": "page", "settings" } } }
        }
        Route { path: "/users/:id", element: rsx! { Layout { name: "user" } },
            Route { index: true, element: rsx! { p { "data-t": "page", "overview" } Who {} } }
            Route { path: "posts/:post", element: rsx! { p { "data-t": "page", "posts" } Who {} } }
        }
        Route { path: "/a",
            Route { path: "b", element: rsx! { p { "data-t": "page", "a/b" } } }
        }
        Route { path: "/", element: rsx! { p { "data-t": "root", "root" } } }
        Route { path: "/old", element: rsx! { Redirect { to: "/dash/settings" } } }
    }
}

fn at(path: &'static str) -> TestUi {
    aio::testing::Mount::default().before(move || _ = NAV.write().replace(path)).app(tree)
}

#[test]
fn nested_routes_show_the_matching_inner_one_at_the_layouts_outlet() {
    // aio tests/router-navlink-outlet.test.ts, air-router-path-matching.test.ts.
    // One window's views per thread at a time (air): each dropped before the next.
    let ui = at("/dash/settings");
    assert_eq!((ui.text("dash"), ui.text("page")), ("dashsettings".into(), "settings".into()));
    assert_eq!(ui.text("bare"), "", "an outlet outside a route shows nothing");
    assert!(ui.absent("root"), "a page matches the whole path, not a prefix");
    drop(ui);
    for path in ["/dash", "/dash/"] {
        assert_eq!(at(path).text("page"), "dash index", "{path}: the index at the layout's own place");
    }
    let ui = at("/users/42");
    assert_eq!((ui.text("page"), ui.text("who")), ("overview".into(), r#"{"id": "42"}"#.into()), "under a param");
    drop(ui);
    let ui = at("/users/42/posts/9");
    assert_eq!(ui.text("who"), r#"{"id": "42", "post": "9"}"#, "the enclosing params joined");
    drop(ui);
    assert_eq!(at("/a/b").text("page"), "a/b", "no element: the inner routes in its place");
    let ui = at("/");
    assert!(ui.present("root") && ui.absent("page"));
    drop(ui);
    assert!(at("/dash/nope").absent("page"), "a layout with no matching inner route");
}

#[test]
fn a_route_follows_navigation_and_a_redirect_leaves_no_back_step() {
    let mut ui = at("/");
    ui.run(|| go("/old"));
    assert_eq!(ui.text("page"), "settings");
    assert_eq!(ui.run(|| here().path), "/dash/settings");
    ui.run(|| go("/dash"));
    assert_eq!(ui.text("page"), "dash index");
    ui.run(back);
    assert_eq!(ui.text("page"), "settings");
    ui.run(back);
    assert!(ui.present("root"), "the redirect replaced /old");
}

/// On air a route's body runs again only when its match changes: a query
/// or another route's place runs nothing; a new param makes its view anew.
#[cfg(feature = "kit")]
#[test]
fn on_air_a_route_runs_again_only_for_a_new_match() {
    fn routes(c: &[aio::testing::MountedComponent]) -> u32 {
        c.iter().map(|c| if c.name == "Route" { c.renders } else { 0 } + routes(&c.children)).sum()
    }
    let mut ui = at("/users/4");
    let before = routes(&ui.components());
    ui.run(|| go("/users/4?tab=1#top"));
    assert_eq!(routes(&ui.components()), before, "a query and a fragment: no route ran");
    ui.run(|| go("/users/5"));
    assert_eq!(ui.text("who"), r#"{"id": "5"}"#);
    assert!(routes(&ui.components()) > before, "a new param: its route ran again");
}

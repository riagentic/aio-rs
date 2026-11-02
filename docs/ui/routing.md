# Routing

Routes name the places of an app (`/`, `/users/7`, `/settings`) so a
view can show one, a link can lead to one, Back can return from one and
the app can open at one. Feature `kit` (the app template has it).

- **Use routes** when the app has pages a person moves between, links
  into it (a notification's click, a deep link), or Back to honour (a
  phone's button, a mouse's back key).
- **Not needed** for a single screen with dialogs: a signal or a cell
  field is simpler.

The history lives in Rust, one per window: the desktop page is the
app's own file and Android's is an asset, so one spelling works on every
target: a route is the path the app wrote. In a browser (a web build) the
page's address follows it too: the address bar shows the place, the
browser's Back and Forward step the app's history, and a reload or a
shared link opens where it points (the server answers every route-shaped
path with the app: the SPA fallback).

## Typed pages: `routes!`

```rust
aio::routes! {
    pub enum Page {
        Home = "/",
        User { id: u32 } = "/users/:id" => "User {id}",
        File { path: String } = "/files/*path",
    }
}

# fn main() {
assert_eq!(Page::parse("/users/7"), Some(Page::User { id: 7 }));
assert_eq!(Page::parse("/users/-1"), None, "a param that doesn't parse is no match");
assert_eq!(Page::parse("/files/a%20b/c.md"), Some(Page::File { path: "a b/c.md".into() }));
assert_eq!(Page::File { path: "a b/c.md".into() }.href(), "/files/a%20b/c.md");
assert_eq!(Page::User { id: 7 }.head().title.as_deref(), Some("User 7"));
# }
```

- Each variant is a page with its pattern; its fields are the pattern's
  params, parsed with `FromStr` and written back with `Display`.
- `parse(path)` gives the first variant, in written order, whose pattern
  matches and whose params parse; a query or fragment is ignored.
- `href()` is the path a link uses, params percent-encoded.
- `=> "User {id}"` is the page's title, formatted with its fields:
  `page.head()` feeds `aio_shell::page::use_head`, and the window's title
  follows it.

## Patterns

| Segment | Matches | Param |
|---|---|---|
| `users` | that literal segment | none |
| `:id` | one non-empty segment | `id`, decoded |
| `*` | the rest, any number of segments (last, maybe none) | `*` |
| `*path` | the same, named | `path` |

One trailing slash is allowed (`/users/` is `/users`); `/users//` is not.
A `*` segment may sit mid-pattern (`/a/*rest/edit`); `/a*b` is a literal.
`aio::route::match_path(pattern, path, exact)` is the matcher on its own,
`aio::route::fill` writes a pattern back, `aio::route::resolve` resolves a
link the way a URL does (`..`, `?q=1`, `#top`), and leaving the app (a
scheme, another host) is `None`.

## The router in views: `router!`

```rust
use aio::ui::prelude::*;

aio::router!(); // NAV, here(), go, redirect, back, use_back, Link, Route, Outlet, params, Redirect

aio::routes! {
    enum Page {
        Home = "/",
        User { id: u32 } = "/users/:id",
    }
}

#[component]
fn App() -> View {
    rsx! {
        nav {
            Link { to: "/", exact: true, "data-t": "home", "Home" }
            Link { to: Page::User { id: 7 }.href(), "data-t": "user", "User 7" }
            Link { to: "https://example.com/", "Help" } // leaves the app: the system browser
        }
        // Read where it is drawn: this hole follows the place.
        {
            match Page::parse(&here().path) {
                Some(Page::Home) => rsx! { p { "data-t": "page", "home" } },
                Some(Page::User { id }) => rsx! { p { "data-t": "page", "user {id}" } Sheet {} },
                None => rsx! { p { "data-t": "page", "not found" } },
            }
        }
    }
}

/// A sheet over the page: Back closes it before it leaves the page.
#[component]
fn Sheet() -> View {
    let open = signal(|| true);
    use_back(move || {
        let was = open.get_untracked();
        open.set(false);
        was // true: Back is handled here
    });
    rsx! { if open.get() { div { "data-t": "sheet", "details" } } }
}

# fn main() {
let mut ui = aio::testing::TestUi::new(App);
ui.click("user");
assert_eq!(ui.text("page"), "user 7");
assert!(ui.run(back), "the sheet takes Back first");
assert!(ui.run(back), "then history");
assert_eq!(ui.text("page"), "home");
# }
```

| Item | Does |
|---|---|
| `here()` | the current place (`aio::route::Loc`: `path`, `query`, `hash`, `.query("key")`); read in a hole (`{ .. }`, an attribute) or a `memo`, it follows the place; a body's read is a snapshot |
| `go(to)` | goes there (resolved from here); false if it leaves the app |
| `redirect(to)` | goes there without a Back step |
| `back()` | the newest `use_back` handler, then history; false at the first place |
| `use_back(f)` | `f` takes Back while the component lives; return true if it handled it |
| `Link { to, exact, replace, class, active_class }` | an `<a>` that routes in the app; the current place's link gets `active_class` (`active`) and `aria-current="page"`; a link out of the app opens in the system browser; the author's `onclick` runs first, and its `prevent_default()` keeps the place |
| `NAV` | the window's `aio::route::History` (50 places, as Chromium keeps) |

A `Route` runs again only when its match changes (its body runs again
then and its element is made anew; a query, a fragment or another
route's place runs nothing). A view that reads `params()` in its body
gets the new ones that way.

## The place, its query and history

```rust
use aio::route::{History, Loc, is_active, of_link, param, resolve, start};

# fn main() {
// A place is a path, a query and a fragment, as a URL spells them.
let at = Loc::parse("/search?q=fresh+milk&page=2#top").unwrap();
assert_eq!((at.path.as_str(), at.hash.as_str()), ("/search", "top"));
assert_eq!(at.query("q").as_deref(), Some("fresh milk"), "decoded, + is a space");
assert_eq!(at.query("page").and_then(|p| p.parse::<u32>().ok()), Some(2));

// Links resolve as a URL does; one that leaves the app is None.
assert_eq!(resolve("../notes", &Loc::parse("/a/b").unwrap()).unwrap().path, "/notes");
assert_eq!(resolve("?page=3", &at).unwrap().to_string(), "/search?page=3");
assert!(resolve("https://example.com/", &at).is_none());

// A param by name, parsed.
let p = aio::route::match_path("/users/:id", "/users/7", true).unwrap();
assert_eq!(param::<u32>(&p, "id"), Some(7));

// The history behind go, redirect and back.
let mut h = History::new(&start(["app".into(), "--route".into(), "/inbox".into()]));
assert!(h.push("/inbox/3"));
assert!(h.push("/inbox/3"), "going where it is replaces: no dead Back step");
assert_eq!(h.len(), 2);
assert!(h.go(-1) && h.here().path == "/inbox");
assert!(h.can_go(1), "Forward");
assert!(!h.push("mailto:a@example.com"), "leaving the app changes nothing");

// What a Link marks active; the place a deep link leads to.
assert!(is_active("/inbox", &Loc::parse("/inbox/3").unwrap(), false));
assert!(!is_active("/inbox", &Loc::parse("/inbox/3").unwrap(), true));
assert_eq!(of_link("myapp://users/7?tab=a"), "/users/7?tab=a");
# }
```

| `aio::route::` | Does |
|---|---|
| `Loc { path, query, hash }` | a place; `Loc::parse(s)`, `.query("key")`, `to_string()` |
| `resolve(to, from)` | `to` from `from` by URL rules (`..`, `?q`, `#x`); `None` if it leaves the app |
| `match_path(pattern, path, exact)` | the params, or `None`; `exact: false` matches a prefix at a segment |
| `param::<T>(&params, name)` | a param parsed, `None` if absent or bad |
| `fill(pattern, &[(name, value)])` | a pattern written back, values encoded |
| `is_active(to, here, exact)` | what `Link` marks active |
| `History` | `new(start)`, `here()`, `push`, `replace`, `go(delta)`, `can_go(delta)`, `sync(place)` (the browser's Back or Forward moved to `place`), `len()`; at most `HISTORY_MAX` (50) places |
| `start_place()`, `mirror(here, replace)`, `pops()`, `step_back(h)` | the router's browser side: the page's address at the start, the address bar following the place, the browser's Back and Forward, Back through the page's history; no-ops off a browser |
| `start(args)` | the launch's `--route`, else `/` |
| `of_link(url)` | the place a `myapp://…` link leads to |

In a view, `here()` is the place (a hole or `memo` reading it follows it) and
`NAV.write().go(1)` is Forward. A search page reads its query and writes
it back with `redirect`, so typing doesn't fill the history:

```rust
use aio::ui::prelude::*;

aio::router!();

#[component]
fn Search() -> View {
    let q = memo(|| here().query("q").unwrap_or_default());
    rsx! {
        input { "data-t": "q", "aria-label": "Search", value: "{q}",
            oninput: move |e| _ = redirect(&format!("/search?q={}", e.value())) }
        p { "data-t": "shown", "results for {q}" }
    }
}

# fn main() {
let mut ui = aio::testing::TestUi::new(Search);
ui.type_text("q", "milk");
assert_eq!(ui.text("shown"), "results for milk");
assert_eq!(ui.read(&NAV).len(), 1, "no Back step per key");
# }
```

## A page's head

A page owns the window's title, and `<meta>` / `<link>` tags, while it
is shown: `aio_shell::page::use_head(head)` at the top of its component
(`aio::head::use_head` in a web build). A typed page gives its own
(`page.head()`, from its `=> "…"` title). Owners nest: the innermost
title wins, and a tag of the same identity (`meta` by `name`, `property`,
`http-equiv`, `charset` or `itemprop`; `link` by `rel` and `href`, and
one `canonical`, `manifest`, `icon` per page) replaces the layout's.
When the last owner goes, the page's own title comes back.

```rust
use aio::head::{Head, Heads};

# fn main() {
let mut heads = Heads::default();
let (layout, page) = (heads.owner(), heads.owner());
heads.set(layout, Head::title("Notes").meta(&[("name", "description"), ("content", "All notes")]));
heads.set(page, Head::title("Groceries").meta(&[("name", "description"), ("content", "One note")]));
let m = heads.merged();
assert_eq!(m.title.as_deref(), Some("Groceries"));
assert_eq!(m.tags.len(), 1, "the page's description replaced the layout's");
heads.remove(page);
assert_eq!(heads.merged().title.as_deref(), Some("Notes"), "back to the layout's");
# }
```

## Pages without routes

A single window with a few screens and no links into it needs no
router: keep the screen in a signal (forgotten at a restart) or a cell
field (kept), and `match` on it. Use routes once there is Back to honour,
a deep link, or a notification that opens a screen.

## Nested routes

```rust
use aio::ui::prelude::*;

aio::router!();

#[component]
fn Dash() -> View {
    rsx! { section { h1 { "Dashboard" } Outlet {} } }
}

#[component]
fn Who() -> View {
    rsx! { p { "{params():?}" } } // the enclosing routes' params
}

fn App() -> View {
    rsx! {
        Route { path: "/dash", element: rsx! { Dash {} },
            Route { index: true, element: rsx! { p { "overview" } } }
            Route { path: "settings", element: rsx! { p { "settings" } } }
        }
        Route { path: "/users/:id", element: rsx! { Who {} } }
        Route { path: "/old", element: rsx! { Redirect { to: "/dash/settings" } } }
    }
}
# fn main() { let _ = aio::testing::TestUi::new(App); }
```

- A `Route` with routes inside is a layout: it matches a prefix, and its
  element shows the matching inner one at its `Outlet`; with no element,
  the inner routes show in its place.
- `path` joins the enclosing route's (`settings` inside `/dash` is
  `/dash/settings`); `index: true` is the enclosing route's own place.
- Every matching `Route` shows, as aio's `<Route>` did; a typed `routes!`
  enum shows exactly one page.
- `Redirect { to }` replaces the place once shown, so Back doesn't land on
  it again.

## Opening at a route

- A launch argument: `my-app --route /users/7` (or `--route=/users/7`)
  starts there (`aio::route::start`).
- A link the app is opened with (`myapp://users/7`): declare the scheme in
  `[build.android] schemes = ["myapp"]`; the link reaches the running app
  through `aio::ui::links()`, and the router goes there with a Back step.
- A notification or tray click with a `route` goes through the app's
  router: call `aio_shell::desktop::use_asks(go)` in the root.

## Back

Back is one path on every target: Android's Back button, a mouse's back
key and `back()` all ask the newest `use_back` handler first (a dialog, a
sheet), then step history; at the first place the host does its default
(Android leaves the app). A handler that panics counts as handled.
`aio::kit::use_back_button` is the same hook for kit components.

## Gaps against aio

- **A route base.** aio's router undoes the app's base path (an app
  served under `/assets`); an aio-rs web build is served at its site's
  root.
- **Typed `useRoute(pattern)`.** aio types the param names of a literal
  pattern; aio-rs's `routes!` types the values as well (`id: u32`), and
  `match_path` / `params()` give strings by name.
- **`navigate(-1)`, `navigate(to, { replace })`.** `back()` and
  `redirect(to)`; Forward is `NAV.write().go(1)`.
- **`NavLink`.** `Link` marks its own active state (`active_class`,
  `exact`).
- **`collectHead()` after a server render.** aio-rs has no server
  rendering of views; `aio::head::html(&merged)` writes the tags of a
  merged head for a page an app serves itself.

## Common mistakes

- Reading `NAV` in a view instead of `here()`: `here()` also wires the
  host's Back and the app's links the first time it is read.
- Pushing in a redirect: `go("/login")` from a page leaves a Back step
  that sends the person straight back; use `redirect` or `Redirect`.
- Expecting the browser's address bar to follow: in a web build the
  route lives in the app too, so the address bar and the browser's Back
  button don't follow it, and a reload starts at `/`.
- Two `routes!` variants whose patterns both match: the first written
  wins, so put `/notes/new` before `/notes/:id`.
- A param that must be a number typed `String`: give it its type in
  `routes!` (`id: u32`), and `/users/7a` is no match instead of a page
  that fails later.
- `go` on every key of a search field: a Back step per key. Use
  `redirect` while typing.
- Hooks after a `match` that returns early in a page component: hooks
  go at the top ([views](views.md#local-state-and-hooks)).

## API

`cargo doc -p aio-rs --no-deps --all-features --open`: `aio::routes!`,
`aio::router!`, `aio::route` (`Loc`, `History`, `match_path`, `resolve`,
`start`, `of_link`), `aio::head`.

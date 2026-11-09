# Islands and advanced views

Most views are `rsx!` all the way down ([views](views.md)). This page is
for the rest: a widget written in JavaScript (a chart, a map, an editor, a
React component), someone else's site inside the app, content drawn
outside its parent's box, and the patterns that sit between views and
cells (held resources, deferred content, sizes). Everything here is in
`aio::kit` (feature `kit`).

| Need | aio-rs | aio |
|---|---|---|
| a JS library or React component in a view | `WebIsland` + `Island::js` / `Island::react` | `island()`, `reactIsland()` |
| the island reading and calling the app's cells | the JS client (`useCell`, `useCall`; [JS client](../clients/js.md)) | `aio/state-core`, `useAio` |
| the same, for code the app doesn't vouch for | `Island::js(..).sandboxed()` | |
| someone else's site in a view | `Browser { src }` | `<Browser>` |
| content drawn at the app's root (a dialog out of an `overflow: hidden` box) | `Portal` + `PortalHost` | `Portal` |
| content later (on view, on hover, after a while) | `Defer { trigger }` | `Defer` |
| a camera, a socket, a file handle held while a view shows | `use_held` + `aio::held::Held` | `useResource` |
| data loaded for a view | `aio::ui::resource` | `resource()` |
| a reaction to a state change | `h.on_change(..)` on the cell's handle | `onChange` |
| an element's size | `onresize` | `useDimensions` |
| an element's DOM handle (focus, scroll, measure) | `onmounted` | `use=` directives, `ref` |

## Islands: JavaScript inside a view

An island is a box of the page that the app's own JavaScript draws. The
view owns where the box is and what goes in (props) and comes out
(events); the script owns what is inside, and the views never touch it.

```rust,no_run
use aio::kit::{Island, WebIsland};
use aio::ui::prelude::*;

#[derive(aio::serde::Serialize, Clone, PartialEq)]
#[serde(crate = "aio::serde")]
struct Count {
    n: i64,
}

#[derive(aio::serde::Deserialize)]
#[serde(crate = "aio::serde")]
struct Bump {
    by: i64,
}

/// The body of a function that returns `mount(el, props, emit)`.
const COUNTER: Island = Island::js(
    "return (el, p, emit) => {
       const b = document.createElement('button');
       b.textContent = 'n=' + p.n;
       b.onclick = () => emit({ by: 2 });
       el.appendChild(b);
       return { update: (q) => { b.textContent = 'n=' + q.n; }, unmount: () => b.remove() };
     };",
);

#[component]
fn Counter() -> View {
    let mut n = signal(|| 0);
    rsx! {
        WebIsland::<Count, Bump> {
            island: COUNTER,
            props: Count { n: n.get() },
            onevent: move |b: Bump| n += b.by,
            loading: rsx! { span { "loading" } },
            "data-t": "counter",
        }
        p { "data-t": "n", "{n}" }
    }
}

# fn main() {
let mut ui = aio::testing::TestUi::new(Counter);
ui.until_text("counter", "n=0");
ui.eval("document.querySelector('[data-t=counter] button').click()");
ui.until_text("n", "2");
ui.until_text("counter", "n=2");
# }
```

### The script

`Island::js(src)`: `src` is the body of a function that returns `mount`:

```text
mount(el, props, emit) -> { update(props), unmount() }   (both optional)
                       -> unmount                       (a function: unmount only)
```

- `el` is the box (`div.aio-island`), `props` the view's props as JSON,
  `emit(v)` sends `v` to the view's `onevent`, parsed as its `E`. An event
  that doesn't parse is said in the app's log.
- The script runs once per page and is shared by every box of that island;
  `mount` runs once per box.
- A prop change calls `update(props)` in place: no remount, the library
  keeps its state. With no `update`, the box is unmounted and mounted
  again with the new props. A new `island` value remounts.
- The box leaving the view calls `unmount`.
- `loading` shows until `mount` returned. A script that throws on load, or
  a `mount` that throws, leaves `loading` in place and says which one
  failed in the app's log (`[island] …`).
- `children` of the `WebIsland` are the view's, inside the box: a script
  that only acts on its box (a tooltip, a drag handle; aio's `use=`
  actions) leaves them be.

### React components

`Island::react(src)` (aio's `reactIsland`): `src` returns `{ react,
reactDomClient, component }`, React, `react-dom/client` and the component
(or a module with a `default`), all from the app's own bundle. The
component gets the props plus `emit`; a prop change re-renders it in
place; the box leaving unmounts its root. aio-rs itself depends on no
React.

The component reads and calls the app's cells itself with the JS
client's hooks, so it shows the same state as the page's own view, and
a call from either reaches both ([JS client](../clients/js.md#in-an-aio-rs-page-islands)):

```jsx
import { connect } from "aio-rs-client";
import { useCall, useCell } from "aio-rs-client/react";

const cell = connect().cell({ id: "tally", v: 1 });
function Tally() {
  const n = useCell(cell, (s) => s.n);
  const [add] = useCall(cell, "add", (s, by) => ({ ...s, n: s.n + by }));
  return <button onClick={() => add(2)}>{n}</button>;
}
```

In the desktop window, whose page may connect nowhere, `use_js_client`
in the root view carries the client over the window's own wire
([JS client](../clients/js.md#in-the-desktop-window)).

### Bundling

The page loads nothing from the network (its content security policy
allows no remote script), so bundle the library into one script and
include it in the binary:

```sh
esbuild chart.js --bundle --format=iife --global-name=m --footer:js='return m.default' --outfile=assets/chart.js
```

```text
const CHART: Island = Island::js(include_str!("../assets/chart.js"));
```

`chart.js` exports `default` as the `mount` function. The bundle ships
inside the app on every target (desktop, Android, the web build).

### Trust: `sandboxed()`

An island's script runs in the page itself: it shares the page's DOM and
its wire to the app, so it acts with the app's rights. The page's policy
still holds (no `eval`, no connections, no remote scripts). That is right
for the app's own code.

For a library the app doesn't vouch for, or one fed untrusted input, use
`Island::js(src).sandboxed()` (or `Island::react(src).sandboxed()`): each
box gets an iframe of an origin of its own, with no page DOM, no wire, no
storage and no cookies. Props, `update`, `unmount` and `emit` work as
before, carried by messages. `el` is the frame's `<body>`, the box's size,
and the app's CSS is not there: the script styles its own document.

## Someone else's site: `Browser`

`Browser { src }` shows a web page in the view, in Electron's `<webview>`,
sandboxed, in a session of its own, with no permissions and only a
preload the app declared. The window must allow it:
`aio_shell::Window { web: Web { browser: true, .. }, .. }` in the launcher
([views](views.md#the-window)).

| Prop | Does |
|---|---|
| `src` | the address; it navigates only when this prop changes, so an address bar that writes `Navigated` back into `src` never loops |
| `keep_alive` | an id: the next `Browser` with it opens where the last one was (cookies always stay; scroll and forms don't) |
| `partition` | its session's name: two `Browser`s with one share a login; `persist:` keeps it across starts; unset: the guests' shared `persist:aio-webview`, never the app's |
| `preload` | one of the window's declared preloads, by name |
| `referrer` | the `Referer` its loads send |
| `host_keys` | 1 to 16 key names (`"Escape"`) that come back to the app even while the guest has focus |
| `popups` | a link that opens a window goes to the system browser, after a real click |
| `onevent` | a `GuestEvent`: `Navigated { url, back, forward }`, `Title`, `Key(HostKey)`, `Message { channel, args }`, `Focus`, `Gone { reason }`, `Unresponsive`, `Responsive` |
| `id` | so `aio::kit::go(id, Go::Back)` can drive it: `Back`, `Forward`, `Reload`, `Stop`, `Send { channel, args }` |

A whole window for another site is `aio_shell::guest::open_window` (needs
`windows: true`). Both need Electron's `<webview>`, so they work in the
desktop window only: in a browser tab a `Browser` shows nothing and warns
in the console. More in [the browser and web views](../clients/web.md).

## Portals

```rust,no_run
use aio::kit::{Portal, PortalHost};
use aio::ui::prelude::*;

#[component]
fn App() -> View {
    let open = signal(|| false);
    rsx! {
        PortalHost {
            div { style: "overflow: hidden; height: 40px", "data-t": "clip",
                button { "data-t": "open", onclick: move |_| open.toggle(), "Details" }
                if open.get() {
                    Portal {
                        div { class: "sheet", "data-t": "sheet", "Drawn at the root, out of the clipping box" }
                    }
                }
            }
        }
    }
}

# fn main() {
let mut ui = aio::testing::TestUi::new(App);
ui.click("open");
assert_eq!(ui.count("[data-t=clip] [data-t=sheet]"), 0);
assert_eq!(ui.count(".aio-portals > [data-t=sheet]"), 1);
# }
```

- `PortalHost` goes around the app once; it draws every portal in a
  `div.aio-portals` at its end, out of every ancestor's `overflow`,
  `transform` and stacking order.
- A portal's state and handlers stay with the component that made it: its
  signals, its context, its clicks.
- Portals show in the order they mounted, so a menu opened inside a
  portalled dialog draws over it, never inside it.
- With no `PortalHost` above, or `disabled: true`, the content stays in
  place.
- The kit's `Modal`, `Menu` and toasts already handle their own layering
  and focus (a modal moves focus in, keeps Tab inside and gives it back on
  close): use them before a hand-made dialog ([kit](kit.md#dialogs-and-toasts)).

## Deferred content: `Defer`

```rust,no_run
use aio::kit::{Defer, Trigger};
use aio::ui::prelude::*;

#[component]
fn Report() -> View {
    rsx! {
        Defer { trigger: Trigger::Viewport, "data-t": "chart",
            placeholder: rsx! { div { class: "aio-skel", "Chart" } },
            div { "data-t": "drawn", "the heavy chart" }
        }
    }
}

# fn main() {
let mut ui = aio::testing::TestUi::new(Report);
ui.until_text("drawn", "the heavy chart");
# }
```

`placeholder` shows (in a box of its own) until the trigger fires, then
the children, for good: a new `key` defers again.
With `load` (aio's `load`), `load(|| async { … Ok(rsx! { … }) })` runs
once triggered, `loading` shows meanwhile (at least `loading_min_ms`),
then what it made, or `error` of what it said.

| `Trigger` | Fires | aio |
|---|---|---|
| `Now` | at once | `"immediate"` |
| `After(d)` | after `d` | a number of ms |
| `Idle` | once the first frame is drawn | `"idle"` |
| `Viewport` | when the placeholder comes into the window | `"viewport"` |
| `Hover` | when the pointer comes over it | `"hover"` |
| `Interaction` | at a click or a key on it | `"interaction"` |

What waits is the render, the code of a [lazy view](#lazy-views-lazy),
and any data the content loads itself.

## Lazy views: `lazy!`

A view used by few (a report, an editor, a settings page) need not be in
what a web build's page fetches first. `aio::lazy!(view)` is a `Load` for
`Defer` (aio's `lazy(() => import(…))` in a `Suspense`):

```rust,no_run
use aio::kit::{Defer, Trigger};
use aio::ui::prelude::*;

#[component]
fn Chart() -> View {
    rsx! { div { "data-t": "chart", "the heavy chart" } }
}

#[component]
fn Reports() -> View {
    rsx! {
        Defer { trigger: Trigger::Now,
            load: aio::lazy!(rsx! { Chart {} }),
            loading: rsx! { p { "Loading…" } },
            error: |e: String| rsx! { p { "{e}" } },
        }
    }
}

# fn main() {
let mut ui = aio::testing::TestUi::new(Reports);
ui.until_text("chart", "the heavy chart");
# }
```

- `ax build --web` (a release) moves the code only lazy views reach into
  chunks beside the bundle: one per view, and one for what two views
  share. The page fetches a view's chunk the first time the view shows
  (`loading` meanwhile), once. `ax build` says their size.
- The trigger is `Defer`'s: `Trigger::Now`, or fetch it when the
  placeholder scrolls into view (`Viewport`) or the pointer comes near
  (`Hover`).
- The view is made by a plain `fn` the macro writes: it captures nothing
  from around it, and reads its state from cells and hooks, as a
  component does.
- On the desktop, Android and in a dev build (`ax dev --web`) the view is
  in the one binary: nothing is fetched, and it shows at once.
- A chunk the page can't fetch (offline before the service worker held
  it) is `error`'s; the next show tries again. The service worker holds
  every chunk once the first view is drawn. A chunk of another build is
  refused, never run.
- Code the rest of the app also reaches stays in the main module, and so
  does data (strings, tables): a chunk holds code.
- Measured on a page with a heavy lazy view (PNG encoding and brotli,
  used by that view only), gzipped: the first download 762 → 543 KB
  (aio-rs 0.3: one module with its names). See
  [performance](../performance.md#measured).

How: a release keeps the linker's relocations. After `wasm-bindgen`, `ax`
finds each function only lazy views reach (by calls, function pointers
and vtables, which the relocations name), moves its code into the view's
chunk and leaves a stub in the main module. A chunk shares the main
module's memory, table and globals; instantiated, it puts its functions
into the table. aio's `lazy` defers evaluating a module, not its
download: aio ships one bundle.

## Held resources: `use_held`

A camera, a socket or a file handle has a close that matters, and a key
that says which one is held. `aio::held::Held` is a pool of them; a view
holds one with `use_held(&pool, key)`, on the desktop, Android and in a
browser page (a web build: the page's own timer and tasks):

```rust,no_run
use aio::held::{Held, State};
use aio::kit::use_held;
use aio::ui::prelude::*;
use std::sync::OnceLock;

/// Stands in for opening a device.
fn cameras() -> &'static Held<String, String> {
    static POOL: OnceLock<Held<String, String>> = OnceLock::new();
    POOL.get_or_init(|| {
        Held::new(
            |id: String| async move { Ok::<_, String>(format!("camera {id}")) },
            |_cam, _id| async {},
        )
    })
}

#[component]
fn Preview(camera: Prop<Option<String>>) -> View {
    // A `Prop`: the held key follows the parent's `cam`.
    let held = use_held(cameras(), camera);
    let text = memo(move || {
        held.with(|h| match h {
            None => "off".to_string(),
            Some(State::Opening) => "…".to_string(),
            Some(State::Open(c)) => c.to_string(),
            Some(State::Failed(why)) => why.clone(),
        })
    });
    rsx! { p { "data-t": "preview", "{text}" } }
}

#[component]
fn App() -> View {
    let cam = signal(|| Some("front".to_string()));
    rsx! {
        button { "data-t": "back", onclick: move |_| cam.set(Some("back".into())), "Back camera" }
        button { "data-t": "off", onclick: move |_| cam.set(None), "Off" }
        Preview { camera: cam.get() }
    }
}

# fn main() {
let mut ui = aio::testing::TestUi::new(App);
ui.until_text("preview", "camera front");
ui.click("back");
ui.until_text("preview", "camera back");
# }
```

- **One open per key**, however many views hold it; closed when the last
  one lets go.
- **The old one is closed first.** A new key waits for the old one's close
  (up to `aio::held::SETTLE`, 5 s, then it opens anyway, loudly), so two
  never fight over one device.
- **A stale open never wins.** An open that finishes after everyone let go
  is closed, never handed out.
- `None` as the key holds nothing. Unmounting lets go.
- Whatever changes the key (a method, an agent, a test) reopens it: keep
  the key in a cell when it should survive a restart.

## Data for a view

- **Data the app keeps** belongs in a cell, loaded by an effect or an async
  method ([effects](../state/effects.md)): a view reads it with `part(..)`.
- **Data only this view needs** (a preview, a lookup):
  `resource(move || async move { .. })`. It re-runs when a signal it
  reads changes; `.get()` is `None` while loading; `.restart()` runs it
  again; `.suspend()?` shows the nearest `Suspense`'s fallback meanwhile.
  `seeded_resource` is one a served page loads on the server and carries,
  streamed in ([views](views.md#server-rendering-and-hydration)).
- **A reaction to a cell's change** ("when the city changes, reconnect")
  is neither a render nor a handler: `h.on_change(immediate, |s|
  s.city.clone(), |city, old| ..)` on the cell's handle keeps what the
  reaction returns until the next change, so a state change from anywhere
  (a method, `ax trigger`, sync) reaches it
  ([effects](../state/effects.md#reacting-outside-a-cell)).

## Elements: size, focus, scroll

- **Size** (aio's `useDimensions`): `onresize: move |e: ResizeEvent|
  if let Ok(s) = e.get_border_box_size() { .. }` on the element; it fires
  when its box changes.
- **The element itself** (aio's `ref` and `use=`): `onmounted: move |e|
  ..` gives a `MountedData`: `set_focus(true)`, `scroll_to(..)`,
  `get_client_rect()`. Keep it in a signal to use it later. The kit's
  `Button` passes `onmounted` through.
- **Scroll into view**: `"data-aio-reveal"` on the element, applied with
  its frame ([views](views.md#the-page)).
- **Long lists**: `VirtualList` draws only the rows in view
  ([performance](../performance.md#4-long-lists)).
- **Online or not** (aio's `useConnected`): `use_status()` from the
  client; calls made offline are kept and sent on reconnect
  ([sync](../state/sync.md)).

## Accessibility checks

`ax lint` reads the views' `rsx!` and reports what aio's dev mode reported
in the page: a `button` with no accessible name, a `div` that takes clicks
but no keys, a `button` in a form with no `type`, an `a` with `onclick` and
no `href`, a positive `tabindex`, `aria-hidden` on something focusable,
`aria-disabled` with a click that still runs, a list item without a key.
`// aio-ok(a11y): why` above a line accepts it. A debug build also warns
once when a kit control renders on a page that doesn't style the kit.

## Gaps

- **An island is painted as its empty box.** A served page is rendered
  on the server and hydrated ([views](views.md#server-rendering-and-hydration));
  an island's script runs once the page's view has hydrated.
- **Another view library drives an island or a served page, not the
  window's root.** aio's `aio/state-core` let Svelte, Vue or other
  frameworks drive a page; aio-rs's JS client does the same for a page
  served to a browser and for an island in it, in a browser or in the
  desktop window (React, Solid and Vue bindings, Svelte's store
  contract, typed by `ax tsgen`; [JS client](../clients/js.md)). The
  window's root stays `rsx!`: one renderer keeps events, focus, the audits
  and `TestUi` paths the same on every page.

## Common mistakes

- An island whose library fetches from a CDN: the page's policy blocks it.
  Bundle it.
- Rebuilding a big props value on every commit: a changed value calls
  `update` each time. Derive props from `part(..)` so they change only
  when the data does.
- `Island::js` for a library fed what a stranger typed (Markdown, HTML):
  sandbox it, or use the kit's `Markdown`.
- `Portal` without a `PortalHost`: the content stays in place, inside the
  clipping box. Put one host around the app.
- Opening a device in `once` or a handler: nothing closes it when the
  view goes or the key changes. Use `use_held`.

## API

`cargo doc -p aio-rs --no-deps --all-features --open`: `aio::kit::WebIsland`,
`Island`, `Browser`, `GuestEvent`, `HostKey`, `Go`, `go`, `Portal`,
`PortalHost`, `Defer`, `Trigger`, `use_held`, `aio::held`;
`cargo doc -p aio-rs-shell --no-deps --open`: `Window`, `Web`, `guest`.

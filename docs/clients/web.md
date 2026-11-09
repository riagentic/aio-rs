# In a browser

An aio-rs app reaches a browser three ways, and shows other sites inside
its own window a fourth:

| Way | What runs in the browser | Make it |
|---|---|---|
| [The web build](#the-web-build) | the app's views and cells, as wasm: an installable web app (PWA) that works offline | `ax build <app> --web` |
| [A served app's page](#a-served-apps-page) | the web build, from the app's own server, its first paint made on the server | `Http::painted(App)` on the app's `Http` |
| [`[app] ui = "browser"`](#the-ui-in-the-system-browser) | a client-server app's UI in the system browser instead of a window | `aio.toml` |
| [Embedded pages](#embedded-pages) | someone else's site in a box of a view, or in a child window | `aio::kit::Browser`, `aio_shell::open_window` |

The views are the same `aio::ui` components everywhere
([views](../ui/views.md)); what changes is where the cells live.

## The web build

```rust,ignore
// ax create's template (src/lib.rs), compiled by ax's own tests:
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn web() {
    let id = option_env!("AIORT_APP_ID").unwrap_or(env!("CARGO_PKG_NAME"));
    _ = SOURCE.set(aio::Source::Local(aio::Handle::saved(id)));
    aio::web::launch(App);
}
```

```sh
ax dev notes --web        # a debug web build on loopback; a saved stylesheet shows at once
ax build notes --web      # dist/<id>-web/: the page, manifest, icons, service worker
ax build notes --web --dev   # a debug build that shows its errors on the page
```

- `ax build --web` builds the app's library for `wasm32-unknown-unknown`,
  binds it with `wasm-bindgen` (the version `Cargo.lock` names; `ax setup`
  fetches it) and writes `dist/<id>-web/`: the page, a web manifest, Apple
  Home Screen tags, icons from the app's hue and an offline service worker.
  Put it on any HTTPS host.
- `aio::web::launch(App)` mounts the root view into the page's
  `<div id="main">`. Set each `client!`'s `SOURCE` first.
- Inside the page the cells run in the browser. Code behind
  `#[cfg(not(target_arch = "wasm32"))]` stays out of it.

### Where the page's cells live

| `SOURCE` | The cell | Offline |
|---|---|---|
| `aio::Source::Local(aio::Handle::saved(id))` | the page's own, in memory, saved to `localStorage` after each call when it persists | always: there is no server |
| `aio::Source::Remote(aio::sync::Remote::<Cell>::open(id).await?)` | a synced copy of the cell of the server that served the page, over its `/ws` | calls wait in `localStorage` and are sent once linked again |

`aio::sync::Remote` in a page is the native copy's protocol over the
page's own socket ([sync](../state/sync.md)):

- It starts from the copy this browser kept, when there is one
  (offline-first), else it waits for the server's.
- What it shows is the confirmed copy plus its calls still waiting
  (`state()`, `confirmed()`, `pending()`, `pending_of(method)`); a refusal
  rolls the view back and is told to `on_refused(|method, why| ..)`.
- Calls made offline survive a reload: they wait in the browser's
  storage (at most `PENDING_MAX`, 500) and are sent once the page is
  linked again. Each has an id, so a call sent twice applies once.
- **Twin tabs** of the same app share that storage: each shows and sends
  the other's waiting calls; a write never takes out another tab's.
- A dropped link is retried with backoff and jitter, at most 10 s apart.
  `status()` (`online`, `pending`, `conn`) is what an offline badge reads,
  in the view through `use_status()`.
- `Remote::open_at(id, url)` names another server's
  `wss://host/ws?wire=2`; that server takes a page's socket only from its
  own host or one its `[server] hosts` lists (the page's `Origin`).
- `aio::web::warn_orphans(id, &["notes"])` says, in the console, which
  stored calls no copy in this page will ever send (a cell renamed or
  dropped); it leaves them in place.

### Signing in from a page

With `[accounts]` on, `aio::kit::SignIn` in a view signs in against the
server that served the page (`aio::web::client()`): the session lives in
an HttpOnly cookie, never in the page's storage or address.
`aio::kit::use_user()` (also `aio::web::use_user()`) is who the page
is (`None` while it asks, `Some(None)` signed out); `aio::web::sign_out().await` ends the session;
`aio::web::restart()` starts the page again, so its copies link as the
new user ([auth](../auth/README.md)).

### What a browser leaves out

The store's history, time travel, files, blobs, SQL tables, the server
and anything native. A page cell's schedules (`cx.after`, `every`, `at`,
`cron`, `#[every]`, `#[cron]`) run on the page's timer while it is open and
are kept with its state, so a reload keeps them and a missed one fires
once, as natively; its `#[ttl]` answers are kept as natively. A page has no log
files: `aio::info!` and its kin write to the console (`aio::web::log`).
`aio_shell::page` (dialogs, print) is the desktop's; use the browser's
own `<input type="file">` and `window.print()` in a page.

## A served app's page

A client-server app serves its own web build to browsers, each page
painted on the server for its viewer (no `#[server]` field, its
`visible` rows and role), so it shows before the script loads; then the
page's own view hydrates it in place, keeping the scroll, focus and typed
text of the paint ([views](../ui/views.md#server-rendering-and-hydration)).
A view waiting for a `seeded_resource` is streamed in once it loads,
before the script runs.

| Item | Does |
|---|---|
| `Http::shell(dist).painted(App)` | the web build in `dist` as the app's shell, each navigation painted for its viewer with the app's cells, its head and its seed |
| `aio::ui::paint(App, &viewer, seed)` | one page's `Paint`, for a server of the app's own making |
| `aio::ui::seeded_resource(..)` | a `resource` the server loads for the page, streamed in with its value; the page's first frame starts from it |
| `aio_shell::web::tabs(http, App, win)` | each browser tab a live view in this process, drawn over its socket (what `ax ui --browser` uses) |
| `aio_shell::web::page(&win)` | the tab page a `Window` describes |

The `Http` goes to `aio::Source::for_app_with(&cfg, &dirs, http)`. The
shell answers unknown route-shaped paths with its `index.html`, so a deep
link works, and needs the app's key, or with accounts a session
([sync](../state/sync.md#server-functions-and-routes)). The page connects
only to its own server (`connect-src 'self'`) and runs no `eval`.

A browser that opens the server with no key (a phone, a tablet) gets the
server's pairing page instead of a bare 401: a 6-digit code from `ax pair
<app> --plain` sets the key's cookie and opens the app. The page installs
to the Home Screen with its icon and an offline shell
([iOS](../build/targets.md#ios)). A tool that asks without a key still
gets `401 wrong or missing key`.

## The UI in the system browser

```toml
[app]
id = "board"
mode = "client-server"
ui = "browser"

[server]
listen = "127.0.0.1:8100"
```

The app starts its server, opens its page in the default browser by a
link that carries the key once (it sets the page's cookie), and serves
until asked to stop (`ax stop`, Ctrl-C). A browser that can't be opened is
said with the address; the server stays up. `ax open board` opens the page
again, `ax open board --print` prints its address.

The app must serve a page: an `Http` with a shell, given to
`Source::for_app_with`; without one the start fails, saying so.

## aio-rs's own tools in a browser

`ax ui --browser` serves the visual app manager on a loopback port,
behind a key made for the run, until Ctrl-C. Each tab is drawn by `ax`
itself over its socket; a link that drops shows `offline` and is retried.

## Embedded pages

`aio::kit::Browser` shows someone else's page in a box of a view
(Electron's `<webview>`): a reader, a help pane, a sign-in flow.

```rust,no_run
use aio::kit::{GuestEvent, Go, go};
use aio::kit::Browser;
use aio::ui::prelude::*;

#[component]
fn Reader() -> View {
    let at = signal(|| "https://example.org/".to_string());
    let title = signal(String::new);
    rsx! {
        nav {
            button { onclick: move |_| go("reader", Go::Back), "Back" }
            span { "data-t": "title", "{title}" }
        }
        Browser {
            id: "reader",
            src: "https://example.org/",
            keep_alive: "reader",
            partition: "persist:reader",
            host_keys: vec!["Escape".into()],
            onevent: move |e| match e {
                GuestEvent::Navigated { url, .. } => at.set(url),
                GuestEvent::Title { title: t } => title.set(t),
                _ => {}
            },
        }
    }
}
# fn main() {}
```

The window must allow it: `aio_shell::Window { web: Web { browser: true,
.. }, .. }`. Without it the guest doesn't load.

| Prop | Does |
|---|---|
| `src` | the page; it navigates only when the prop changes, so an address bar that writes `Navigated` back into its state never loops |
| `keep_alive` | an id: a guest dies with its element, and the next `Browser` under this id opens where it was (its cookies are kept; its scroll and forms aren't) |
| `partition` | its session: two `Browser`s with one share a login; `persist:` keeps it across starts. Unset: the guests' shared `persist:aio-webview`, never the app's own |
| `preload` | one of the window's declared preloads, by name ([below](#a-preload-for-the-guest)) |
| `referrer` | the `Referer` its loads send (an embed that checks it refuses to play without one) |
| `host_keys` | 1 to 16 `KeyboardEvent.key` names the guest hands back, even from its frames (`GuestEvent::Key`) |
| `popups` | a link that opens a window goes to the system browser after a real click; without it, nothing |
| `onevent` | `GuestEvent`: `Navigated { url, back, forward }`, `Title`, `Key(HostKey)`, `Message { channel, args }` (its preload's `ipcRenderer.sendToHost`), `Focus`, `Gone { reason }`, `Unresponsive`, `Responsive`, `PreloadRefused { preload, reason }` |
| `id` | the element's id, for `aio::kit::go(id, Go::Back \| Forward \| Reload \| Stop \| Send { channel, args } \| Css(sheet) \| Zoom(f) \| DevTools)` (`Css`: `insertCSS`, until it navigates; `Zoom`: `setZoomFactor`; `DevTools`: its own DevTools window) and `aio::kit::run_js(id, js).await` (`executeJavaScript`: its value as JSON, `None` when the guest isn't there or the script threw); every other `<webview>` method by name: `aio::kit::webview(id, "getURL", vec![]).await` (`WEBVIEW_METHODS` lists them; its value as JSON, a promise awaited, `capturePage` a PNG data URL, `printToPDF` base64; `None` when the guest isn't there, the name is none of them (said) or it threw) |

### A preload for the guest

A preload is a script Electron runs in the guest before its page: how an
app adds a provider to a site and hears back from it. It is declared in
the window and carried in the binary, so it is there in every build:

| `aio_shell::guest::Web` field | Does |
|---|---|
| `preloads` | `&[("reader/preload.js", include_str!("../guest/preload.js"))]`: names the guests may ask for. A name is `/`-separated plain segments, no `..` (`guest::preload_refusal` says why one isn't) |
| `fullscreen` | a guest may go fullscreen (a video's button); else it holds no permission at all |
| `downloads` | a guest may save files |
| `windows` | `open_window` may open child windows |
| `unsandboxed_windows` | a child window that asks (`Child { unsandboxed: true }`) opens without Chromium's sandbox: its preload runs with Node (aio's `unsandboxedChildWindows`) |

- A preload runs sandboxed, with context isolation and no Node:
  `require("electron")` gives it `ipcRenderer` and `contextBridge`.
  `ipcRenderer.sendToHost(channel, ...args)` reaches `onevent` as
  `Message`; `go(id, Go::Send { .. })` reaches its `ipcRenderer.on`.
- A name the window didn't declare is refused when the guest attaches:
  the guest loads with no preload, the log says
  `<Browser> preload REFUSED` with the fix, and the page hears it: the
  guest's `onevent` gets `GuestEvent::PreloadRefused { preload, reason }`
  and `window` the `aio:guest-preload-refused` event (aio's), so the app
  can show that the embedded page has no bridge.

### Child windows

`aio_shell::open_window(&Child { url, preload, partition, origins, size,
permissions, unsandboxed }).await` opens an `http(s)` page in a window of
its own (needs `Web { windows: true }`). Its session is
`persist:aio-child` unless `partition` names another (never the app's);
an `unsandboxed` one's is `persist:aio-child-unsandboxed`, never the
sandboxed ones' (that name is refused for it);
`origins` limits where it may go (unset: any site, never the app's scheme
or a file); `size` defaults to 1100 × 800. Every refusal comes back as an
error with its reason.

- `permissions` (`guest::PERMISSIONS`' names, `vec!["media".into()]`):
  what its page may hold on the origin it was opened on, never a frame of
  another site in it, nor the site it navigates to. Unset: nothing. aio
  gave a child window every permission of its own origin; aio-rs gives
  only the ones the app names.
- `unsandboxed: true` opens it without Chromium's sandbox (aio's
  `sandbox: false`, the page-world injection escape hatch). It is refused,
  with the reason, unless the window allows it (`Web {
  unsandboxed_windows: true }`): never quietly sandboxed instead. Each
  unsandboxed window is said in the log.

### Security

- A guest never gets Node, a weaker sandbox or the app's own session: a
  page in that session could read what the app serves on `aio://app`. One
  with no partition, or the app's, is moved to `persist:aio-webview`, and
  the log says so once.
- Guests hold no permission (no camera, location, notifications, devices)
  except fullscreen and downloads when the window allows them, and a
  child window's own `permissions`.
- Each blocked navigation, pop-up, download or permission is said once per
  kind and origin, never the full URL. Each kind stops after 32 different
  lines and says so once (`further pop-up lines are not said`), the
  other kinds go on: a page can neither flood the log nor silence it.
- A denied permission's line names what would grant it:
  `permission "midi" DENIED to the app's own page: Window { permissions:
  &["midi"] } grants it`, `Web { fullscreen: true }` for a guest's
  fullscreen, `Child { permissions }` for a child window.
- A site the app doesn't trust at all goes in a child window with
  `origins` set, or in an island's sandboxed iframe
  ([islands](../ui/islands.md)).

## The dev page's console

Under `ax dev <app> --web` the page's console comes to `ax dev`'s output,
as aio's client log forwarding did: every level (`console.log` is
`info`), uncaught errors (`[uncaught]`) and refused promises (`[unhandled
rejection]`), each line cut at 4 KB, sent in batches to the dev server's
`/aio-dev/log`; past 200 lines waiting the rest are counted and the count
said. The page's own console still shows them. A release page sends
them to the server that serves it (`/__aio/log`, aio's client log), into
the app's log as `[page]` lines, at most 100 a second for each client
(the rest counted and said), with no terminal escapes, colour included.
They are taken from who may read health: anyone on the server's own
machine, else a page with the key (its cookie) or a session.
`[server] page_log = false` takes none, and a page on a static
host stops sending at its first 404.

```text
ax dev: [page] warn: low disk
ax dev: [page] error: [uncaught] TypeError: x is undefined
ax dev: [page] error: [aio] panicked at src/app.rs:12:5: no rows
```

A panic of the page's Rust is said on its console with its place (the
wasm trap alone names none), so it reaches the same log, a release's too:
aio's source-mapped `client.log`, with no bundled JavaScript to map. A
dev page also shows it, open, in its overlay.

A dev web build (`ax dev --web`, `ax build --web --dev`) has aio-rs's
`dev` on, as `ax dev`'s native build: its overlay shows the page's
errors and its live accessibility and contrast findings, and its cells
show in Redux DevTools ([time travel](../debugging/time-travel.md#in-redux-devtools)).

## Notifications

A method's `cx.tell(Tell::Notify { .. })` shows in a web build as the
browser's own notification, with the app's icon (`icon-192.png`). A
browser grants notifications only from a user gesture: the first tell asks,
and one refused for lack of a gesture (or blocked in the site's settings)
is said once on the console with the fix: ask from a click, an "Enable
alerts" button whose `onclick` calls `aio::web::notify::request_permission()`
(a wasm32 build's).

A click on a notification brings the page forward and, with `route`, goes
there through `aio::web::notify::on_click(go)` (the app's `router!` `go`).
Where a page may not make a notification itself (Chrome on Android) the
app's service worker shows it. `aio::tell::show_with(f)` set before
`aio::web::launch` replaces all of this.

## Gaps

- A page's copy has no time travel, history or blobs.
- aio handed out the `<webview>` element itself; aio-rs calls each of
  its methods by name (`webview`, all of `WEBVIEW_METHODS`), with `go`
  and `run_js` for the common ones. `capturePage` needs a display: a
  headless runtime has no surface to capture a guest from (`None`).
- `Browser` is Electron's `<webview>`: a desktop window's only, not a
  web build's or Android's.

## Common mistakes

- `Browser` with the window's `web.browser` off: nothing renders.
- A guest preload given as a path on disk: give the declared name.
- Expecting `aio_shell::page::pick` in a page: it is the desktop's; use a
  file input.
- A `Remote::open_at` page served from another host that the server's
  `[server] hosts` doesn't name: the socket is refused.
- Two pages of one app with different `SOURCE`s for the same cell id:
  they write one storage document.

## API

`cargo doc -p aio-rs --no-deps --all-features --open`: `aio::web`
(`launch`, `sync::Remote`, `Handle`, `storage`, `client`, `use_user`),
`aio::kit::Browser`, `aio::kit::GuestEvent`, `Go`, `go`,
`aio::http::Http` (`shell`, `painted`, `paint`), `aio::ui::paint`,
`aio::ui::Seed`, `air::ssr`. `cargo doc -p aio-rs-shell
--no-deps --open`: `web` (`tabs`, `page`), `guest` (`Web`, `Child`,
`open_window`). `aio::web` is built for `wasm32` only: document it with
`--target wasm32-unknown-unknown`.

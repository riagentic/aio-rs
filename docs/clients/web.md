# In a browser

An aio-rs app reaches a browser three ways, and shows other sites inside
its own window a fourth:

| Way | What runs in the browser | Make it |
|---|---|---|
| [The web build](#the-web-build) | the app's views and cells, as wasm: an installable web app (PWA) that works offline | `ax build <app> --web` |
| [A served app's page](#a-served-apps-page) | the web build, from the app's own server, its first paint made on the server | `aio_shell::web::painted` on the app's `Http` |
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
`aio::web::use_user()` is who the page is (`None` while it asks,
`Some(None)` signed out); `aio::web::sign_out().await` ends the session;
`aio::web::restart()` starts the page again, so its copies link as the
new user ([auth](../auth/README.md)).

### What a browser leaves out

The store's history, time travel, files, blobs, SQL tables, the server,
schedules that outlive the page and anything native. A page has no log
files: `aio::info!` and its kin write to the console (`aio::web::log`).
`aio_shell::page` (dialogs, print) is the desktop's; use the browser's
own `<input type="file">` and `window.print()` in a page.

## A served app's page

A client-server app serves its own web build to browsers, each page
painted on the server for its viewer (no `#[server]` field, its
`visible` rows and role), so it shows before the script loads; then the
page's own view takes its place, keeping the scroll, focus and typed text
of the paint.

| `aio_shell::web` item | Does |
|---|---|
| `painted(http, handle, App, dist)` | `http` serving the web build in `dist` as the app's shell, each page painted for its viewer |
| `tabs(http, App, win)` | each browser tab a live view in this process, drawn over its socket (what `ax ui --browser` uses) |
| `page(&win)` | the tab page a `Window` describes |

The `Http` goes to `aio::Source::for_app_with(&cfg, &dirs, http)`. The
shell answers unknown route-shaped paths with its `index.html`, so a deep
link works, and needs the app's key, or with accounts a session
([sync](../state/sync.md#server-functions-and-routes)). The page connects
only to its own server (`connect-src 'self'`) and runs no `eval`.

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
| `onevent` | `GuestEvent`: `Navigated { url, back, forward }`, `Title`, `Key(HostKey)`, `Message { channel, args }` (its preload's `ipcRenderer.sendToHost`), `Focus`, `Gone { reason }`, `Unresponsive`, `Responsive` |
| `id` | the element's id, for `aio::kit::go(id, Go::Back \| Forward \| Reload \| Stop \| Send { channel, args })` |

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

- A preload runs sandboxed, with context isolation and no Node:
  `require("electron")` gives it `ipcRenderer` and `contextBridge`.
  `ipcRenderer.sendToHost(channel, ...args)` reaches `onevent` as
  `Message`; `go(id, Go::Send { .. })` reaches its `ipcRenderer.on`.
- A name the window didn't declare is refused when the guest attaches:
  the guest loads with no preload and the log says
  `<Browser> preload REFUSED` with the fix.

### Child windows

`aio_shell::open_window(&Child { url, preload, partition, origins, size
}).await` opens an `http(s)` page in a window of its own (needs `Web {
windows: true }`). Its session is `persist:aio-child` unless `partition`
names another (never the app's); `origins` limits where it may go (unset:
any site, never the app's scheme or a file); `size` defaults to 1100 ×
800. Every refusal comes back as an error with its reason.

### Security

- A guest never gets Node, a weaker sandbox or the app's own session: a
  page in that session could read what the app serves on `aio://app`. One
  with no partition, or the app's, is moved to `persist:aio-webview`, and
  the log says so once.
- Guests hold no permission (no camera, location, notifications, devices)
  except fullscreen and downloads when the window allows them.
- Each blocked navigation, pop-up or permission is said once per kind and
  origin.
- A site the app doesn't trust at all goes in a child window with
  `origins` set, or in an island's sandboxed iframe
  ([islands](../ui/islands.md)).

## The dev page's console

Under `ax dev <app> --web` the page's console comes to `ax dev`'s output,
as aio's client log forwarding did: every level (`console.log` is
`info`), uncaught errors (`[uncaught]`) and refused promises (`[unhandled
rejection]`), each line cut at 4 KB, sent in batches to the dev server's
`/aio-dev/log`; past 200 lines waiting the rest are counted and the count
said. The page's own console still shows them. A release page sends nothing.

```text
ax dev: [page] warn: low disk
ax dev: [page] error: [uncaught] TypeError: x is undefined
```

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

- The browser's address bar and Back don't follow the app's routes in the
  web build ([routing](../ui/routing.md#common-mistakes)).
- A page's copy has no time travel, history or blobs.
- A page's console reaches a log only under `ax dev --web` (its lines in
  `ax dev`'s output, [the dev page](#the-dev-pages-console)); a release
  page's lines stay in its console, where aio forwarded them in a release
  too. (The desktop window's lines do reach the app's log.)
- `Browser` exposes `go` and the events above, not every `<webview>`
  method (no `executeJavaScript`, `insertCSS` or DevTools of the guest).
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
`aio::http::Http` (`shell`, `paint`). `cargo doc -p aio-rs-shell
--no-deps --open`: `web` (`painted`, `tabs`), `guest` (`Web`, `Child`,
`open_window`). `aio::web` is built for `wasm32` only: document it with
`--target wasm32-unknown-unknown`.

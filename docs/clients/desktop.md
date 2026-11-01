# The desktop window

On Linux, Windows and macOS an aio-rs app's views are drawn by the shared
Electron runtime. The views themselves run in the app's Rust process: a
Dioxus `VirtualDom` per window, whose DOM edits and events travel over a
private socket to a small page in Electron. The page loads from
`aio://app/`, connects to nothing and runs no app code; Electron's main
process opens the window, the tray, notifications and dialogs when the app
asks.

- **Use it** for every window app: `ax create <name>` (the `app` template)
  is one, `ax dev <app>` runs it, `ax build <app>` packages it
  ([targets](../build/targets.md#desktop)).
- **Not it**: a browser tab ([web](web.md)), a headless service
  (`[app] mode = "server"`), a command-line tool ([cli apps](cli-apps.md)).

`aio_shell` is the crate that hosts the window. Its items are listed here
with their fields; the full API is `cargo doc -p aio-rs-shell --no-deps
--open`.

## Opening the window

The template's `main` does three things:

```rust,ignore
// ax create's template (app/src/main.rs), compiled by ax's own tests:
let cfg = aio::config!();
let Start::Run(dirs) = aio_shell::start(&cfg, aio::app_version!())? else { return Ok(()) };
_ = app::SOURCE.set(aio::Source::for_app(&cfg, &dirs)?);
aio_shell::run(app::App, aio_shell::Window { width: 900, height: 640, ..aio_shell::Window::of(&cfg) }, &dirs)
```

| Step | What it does |
|---|---|
| `aio_shell::start(&cfg, version)` | reads the app's flags (`--home`, `--profile`, `--instance`, `--route`), takes the app's lock and boots it (log, reports, health). `Start::Run(dirs)`: this process runs the app. `Start::HandedOver`: the app already runs, it got this launch's arguments and came to the front, so this process exits. `Start::Done`: nothing to run (`--version` answered, an update is installing). |
| `aio::Source::for_app(&cfg, &dirs)` | opens the cell as `[app] mode` says: this process's store, or a synced copy of `[client] server`'s |
| `aio_shell::run(App, Window { .. }, &dirs)` | opens the window and returns when it closes; then the stores finish their effects (5 s at most) and the app exits |

The window's fields (`Window::of(&cfg)` fills the title, `lang` and `dir`
from `[app]`) are listed in [views](../ui/views.md#the-window).

### One instance, later launches

A second launch of the same app (same data dir) never opens a second
window: it hands its working dir and arguments to the running instance
over a private socket and exits; the running one comes to the front.
The app hears those launches from `aio_shell::launches()`, a broadcast
receiver of `Launch { cwd, args }`; `launch.path(arg)` makes an argument
absolute against the launch's own dir (URLs and absolute paths stay as
they are). That is how "open this file with" reaches a running app.

`--takeover` asks the running instance to stop the graceful way and waits
up to 30 s for its lock: a new build replacing an old one.
`aio_shell::instance::quit(id)` asks the same from code.

### Closing and stopping

- Closing the window ends the app (`aio_shell::run` returns). There is no
  "keep the server running" switch: an app whose server must outlive its
  window is two apps, a `mode = "server"` service and a `mode = "client"`
  window ([sync](../state/sync.md)).
- `ax stop <app>`, SIGTERM or Ctrl-C ask the window to quit; the runtime
  gets 5 s, then it is killed. The stores then drain as on a close.
- A parent that goes (`ax dev`, a test harness) stops the app the same
  graceful way, even after `kill -9` of the parent.

## The shared runtime

aio-rs pins one Electron version (`aio_shell::runtime::VERSION`, the same
release aio ships). The app never chooses it; an aio-rs upgrade moves it.

Where the window's runtime comes from, first match wins:

| Source | When |
|---|---|
| `AIORT_RUNTIME=<dir>` | set by hand: a runtime unpacked somewhere else |
| `electron/` beside the app's binary | an offline build, `[build] runtime = "bundled"` |
| the shared store, one entry per version | everything else: `<version>-<os>-<arch>` in aio's Electron store, the one aio's apps start from (`~/.local/share/aio/electron` on Linux, `$XDG_DATA_HOME` first; `~/Library/Application Support/aio/electron` on macOS; `%LOCALAPPDATA%\aio\electron` on Windows; `AIO_SHARED_ELECTRON_DIR` moves it) |

- **First start**: a runtime that isn't there yet is downloaded from
  Electron's releases, checked against the SHA-256 this build pins for it
  (taken from the release's `SHASUMS256.txt` when aio-rs was built; a
  mismatch refuses, and nothing from the network can change the pin),
  unpacked beside its entry, fused and renamed into place, so a half-done
  download is never used. Meanwhile a "Getting ready" window
  shows progress and Cancel; a failure asks Retry or Quit. That window is
  drawn with what each OS always has (zenity or yad on Linux, Cocoa on
  macOS, Windows Forms on Windows, else a page in the default browser).
- `AIORT_ELECTRON_MIRROR=<url>` replaces
  `https://github.com/electron/electron/releases/download/`.
- `ax setup` and `ax runtime --fetch` fetch it ahead of time (CI, a
  machine about to go offline); `ax runtime` says where it is.
- An update that needs a newer runtime fetches it before it swaps, so the
  new version's first start never waits on a download.
- `ax prune` lists what of the store no installed app names and nothing
  started from in 30 days; `ax prune --yes` removes it.
- An offline build (`[build] runtime = "bundled"`) carries its runtime,
  trimmed to the `en-US` and `en-GB` locales; `[build] chromium_extras =
  "strip"` also drops Chromium's software Vulkan and DirectX shader
  compilers.

### One store with aio

The store is aio's, kept by aio's rules, so an aio app and an aio-rs app
that pin the same Electron start from one copy, and neither deletes what
the other needs:

| Rule | What both do |
|---|---|
| An entry is complete | it holds `.aio-complete` (`<url>`, `sha256:<hex>`) and its executable; anything else is not a runtime |
| One installer at a time | `<entry>.lock` holds the pid, `<entry>.lock.id` the pid with its namespace and start stamp, both changed under an OS lock on `<entry>.lock.mx`; a dead holder's lock is taken over, a live one's waited for (15 minutes) |
| Nothing half-done is used | the download unpacks into `<entry>.incoming.<pid>-<tag>`, kept fresh while it works; a dead one's stage is cleared by the next install |
| Which app needs which runtime | each start writes `apps/<key>-<version>.json` (`exe`, `version`, `slug`, a macOS `window`, `at`); an app keeps its newest two, plus one an update fetched before its swap |
| What may go | what no installed app's record names; `am prune`, `ax prune` and both apps' once-a-day clean-up (30 days unused, under the entry's lock) agree |
| When it was last used | `.aio-last-used` in the entry, an ISO time |

A store left in `aio-rt/electron` by an earlier development build is
moved: the first start (or `ax runtime`, `ax setup`, `ax prune`) moves
what is there into aio's store, each entry under its lock: deleted when
the store already has it complete, else moved in one rename, fused and
stamped complete. Its app records become aio's; its macOS windows are made
again at the next start. An entry a running program starts from stays
where it is until it doesn't.

## The window's behaviour

| What | How it works |
|---|---|
| Size and place | the first size is `Window { width, height }`; the size and place a person leaves it at are kept in `<data>/window.json` and used while the declared size is unchanged. A saved place off every screen moves onto the screen it overlaps most; a size as big as the screen opens maximized. `ax dev --size 390x844` opens one run at that size and saves nothing. |
| First frame | the window shows once it has painted (5 s at most), on `Window { background: Some([light, dark]) }` until then; `[build.splash]` shows the app's image first ([theming](../ui/theming.md)) |
| Menu | none (Electron's default menu carries DevTools and reload), except on macOS: the app menu (Hide, Quit), Edit (so Cmd+C works in a field) and Window, all system roles. An app can't add menus yet. |
| Keys | Ctrl+R / F5 reload and Ctrl+P print belong to the window unless `Window { keys: Keys { reload, print } }` gives them to the page. A debug build adds DevTools (F12, Ctrl+Shift+I) and a clean reload. |
| Links | the page never navigates; an `http(s)` link opened after a real click goes to the system browser. A pop-up a script opens by itself is blocked and said in the log. `AIORT_NO_OPEN=1` opens nothing (a contained run). |
| Permissions | the page may copy text and go fullscreen, nothing else, unless `Window { permissions }` names more (`aio_shell::guest::PERMISSIONS`: `clipboard-read`, `media`, `geolocation`…). Its own `Notification` is refused: notifications come from the app. |
| Unload | a page's `beforeunload` veto is ignored (said once in the log) unless `confirm_unload: true`, then Leave / Stay is asked |
| Page messages | the page's `console.warn` and `console.error` land in the app's log as `[page]` lines, with the file and line (8 KB each, 100 a second); `ax logs <app> --client` shows only those |
| Hangs and crashes | a page that stops answering marks the app's health degraded ("window") until it answers again. A crashed renderer is reloaded on a new connection; the fourth crash in a minute closes the app with a dialog, and a problem report is saved. |
| GPU | a runtime that died because the GPU failed is remembered (`<data>/electron/gpu-failed`): the next start draws in software. `AIORT_GPU=software` or `hardware` decides by hand. |
| Back | a mouse's back button (Windows, Linux) is the app's Back, as Android's is ([routing](../ui/routing.md)) |
| Identity | each app is its own app to the OS: its window class and Wayland app id, its taskbar entry and, on macOS, its own Dock icon (the window runs through the app's bundle) |

The runtime's profile lives in the app's own data (`<data>/electron/profile`),
never in a shared Chromium profile.

## The tray

Set `tray: Some(Tray { .. })` in the `Window` that `aio_shell::run` opens
to show a tray icon:

| `aio_shell::desktop::Tray` field | Does |
|---|---|
| `tooltip` | the text when the pointer rests on it; unset: the window's title. `desktop::tooltip(text)` changes it later. |
| `close_to_tray` | closing the window hides it to the tray; Quit (or the app stopping) still closes it. A Linux desktop with no tray minimizes instead. |
| `menu` | `&[Item { label, run, route }]`: `run` (a `fn()`) runs in the app's views, then the window comes forward and goes to `route`. An item with an empty label is a separator. The menu always ends with Show, Hide and Quit. |

A click on the icon shows or hides the window. A tray item and a click on
a notification reach the views through `aio_shell::desktop::use_asks(go)`,
called once in the root component with the router's `go` (`router!` makes
one): items then run where every button runs, with the cells, signals and
router at hand.

### The app's icon

Every window app has its icon at run time, in a dev build and an installed
one: `aio::config!()` builds in `assets/icon.png` beside `aio.toml`, else
the app's monogram (its name's first letter, deep on a pastel gradient,
aio's generated icon) in the app's hue (`Config::hue`: `[ui] hue`, else its
id's), the one its page, splash and download window share.
`Window::of(&cfg)` wears it: the title bar, the taskbar and Alt-Tab, the
tray, notifications, the window bundle in the macOS Dock, the download
window (Windows, macOS, GTK) and a browser tab's favicon. A `Window` with
no `icon` shows Electron's, and the log says so once. A first
`assets/icon.png` is built in the next time the crate compiles (an edit,
or `cargo clean -p <app>`); a changed one is noticed by itself.

### Changing the icon

`aio_shell::desktop::icon(Icon::Window | Icon::Tray, frames, fps)` changes
an icon from now on: one PNG, or up to `ICON_FRAMES` (64) PNG frames of at
most `ICON_BYTES` (512 KB) each, looped by the runtime at `fps` (at most
`ICON_FPS`, 12). No frames: the app's own icon again. A bad image is
refused where it is passed. A cell asks for the window's, the tray's and
the tray's tooltip in one with `cx.tell(Tell::Icon { window, tray,
tooltip, fps })` (aio's `appIcon()`: base64 frames; `None` leaves that
one as it is), so a server's state can badge every window showing it.
`desktop::icon_state()` says what each shows now (aio's `iconState()`):
`Own`, `Still` or `Animated { frames, fps }`, and the tooltip.

## Notifications

A method tells the people using the app with `cx.tell(..)`: the tell is
decided where the state changes, and shown by every window showing the
cell (a synced copy's too).

```rust
use aio::prelude::*;
use aio::tell::Tell;
use std::sync::{Arc, Mutex};

#[cell]
pub struct Uploads {
    pub done: u32,
}

#[methods]
impl Uploads {
    pub fn finished(&mut self, cx: &Cx, name: String) -> Result {
        self.done += 1;
        cx.tell(Tell::Notify {
            title: "Upload done".into(),
            body: name,
            tag: Some("upload".into()), // a later one replaces it
            silent: false,
            route: Some("/uploads".into()), // a click opens this page
        })
    }
}

# fn main() {
// The desktop shell shows tells as notifications; this test collects them.
let seen = Arc::new(Mutex::new(vec![]));
let s = seen.clone();
aio::tell::show_with(move |t| s.lock().unwrap().push(t.clone()));
let h = Handle::<Uploads>::memory();
h.finished("notes.txt".into()).unwrap();
assert!(matches!(&seen.lock().unwrap()[0], Tell::Notify { body, .. } if body == "notes.txt"));
# }
```

- A blank title is refused (`Tell::check`), a refusal of the call.
- From a view, `aio_shell::desktop::notify(&Notice { title, body, tag,
  silent, route }).await` shows one directly. A desktop that shows no
  notifications is said once in the log.
- A click brings the window forward and goes to `route`
  (`use_asks`, above).

## Dialogs, clipboard, print

`aio_shell::page` opens the system's dialogs from a view
([views](../ui/views.md#the-page)): `pick(&Pick { .. })` and
`pick_many` (files of some types, or a folder), `save(&Save { .. })` (a
path to write, after the dialog asked before replacing a file), `copy`,
`print`. A dialog is owned by the window; with no window (a test) the
desktop's own dialog tool answers. An error (no dialog tool, no desktop
session) is an error, never a cancel.

## Other sites in the app

Someone else's page shows in a view with `aio::kit::Browser` (needs
`Window { web: Web { browser: true, .. } }`) or in a window of its own
with `aio_shell::open_window(&Child { url, preload, partition, origins,
size })` (needs `windows: true`). Both run sandboxed, in a session that is
never the app's. See [web: embedded pages](web.md#embedded-pages).

## Running a program from the app

A desktop app that drives another program (a converter, `git`, a backup
tool) runs it in an effect: the call that asks for it commits at once,
the program runs off the cell's thread, and its result comes back as
another call, saved in the log like any input.

```rust
use aio::prelude::*;
use aio::testing::TestCells;
use std::time::Duration;

#[cell(persist)]
pub struct Jobs {
    pub running: bool,
    pub output: Option<String>,
    pub error: Option<String>,
}

/// `program args`, on a blocking thread: its trimmed output, or why it failed.
async fn run(program: String, args: Vec<String>) -> std::result::Result<String, String> {
    let ran = tokio::task::spawn_blocking(move || {
        std::process::Command::new(&program).args(&args).output().map_err(|e| format!("{program}: {e}"))
    });
    let out = ran.await.map_err(|e| e.to_string())??;
    let text = |b: &[u8]| String::from_utf8_lossy(b).trim().to_string();
    if out.status.success() {
        Ok(text(&out.stdout))
    } else {
        Err(format!("exited with {}: {}", out.status, text(&out.stderr)))
    }
}

#[methods]
impl Jobs {
    pub fn run_job(&mut self, cx: &Cx, program: String, args: Vec<String>) -> Result {
        ensure!(!self.running, "a job is running");
        self.running = true;
        self.error = None;
        cx.run(
            Effect::new("job", move |h: Handle<Jobs>| async move {
                let got = run(program, args).await;
                _ = h.ended(got);
            })
            .mode(Mode::First)
            .timeout(Duration::from_secs(600)),
        );
        Ok(())
    }

    pub fn ended(&mut self, got: std::result::Result<String, String>) {
        self.running = false;
        match got {
            Ok(out) => self.output = Some(out),
            Err(e) => self.error = Some(e),
        }
    }
}

# fn main() {
let mut t = TestCells::new();
let h = t.cell::<Jobs>();
let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
h.run_job(cargo, vec!["--version".into()]).unwrap();
assert!(h.run_job("x".into(), vec![]).is_err(), "one at a time");
t.settle();
assert!(h.state().output.as_deref().is_some_and(|o| o.starts_with("cargo")));
# }
```

- **Choosing the program**: run a path the app knows (a bundled tool, one
  the person picked with `page::pick`), never a string from the network.
  Pass arguments as a list, never through a shell.
- **Long jobs**: an effect's `.timeout(d)` is its limit; none means it
  runs until the app closes. `h.busy("job")` and `h.pending(..)` are what a
  spinner reads. A step-by-step `async fn` method is cut off after
  `aio::effect::CALL_CEILING` (30 s) per await unless marked `#[long]`
  ([effects](../state/effects.md#heavy-work-retries-and-timeouts)).
- **Talking to it**: stream its stdout line by line from the effect and
  hand progress back as calls (`h.progress(line)`), coalesced if they are
  many.
- **Cleaning up**: an aborted or timed-out effect is dropped at its next
  await, but a child it started keeps running. Keep the
  `std::process::Child` and kill it, or use `tokio::process::Command` with
  `kill_on_drop(true)` (tokio's `process` feature in the app's
  `Cargo.toml`), so the program dies with the effect.
- The program gets the app's environment; aio-rs's own settings
  (`AIORT_*`) are worth removing (`aio::lifecycle::scrub(&mut cmd,
  false)` does).

## Headless hosts, VMs and containers

`AIORT_ELECTRON_ARGS` adds Chromium switches, from an allow-list of the
display, GPU, locale and logging ones:

```sh
# a VM with no working GPU
AIORT_ELECTRON_ARGS="--disable-gpu" ax dev notes
# a container with a 64 MB /dev/shm
AIORT_ELECTRON_ARGS="--disable-dev-shm-usage --disable-gpu" ax dev notes
```

| Allowed | Refused, and the way instead |
|---|---|
| `disable-gpu`, `disable-gpu-compositing`, `disable-software-rasterizer`, `use-gl`, `use-angle`, `enable-unsafe-swiftshader`, `ignore-gpu-blocklist`, `force-device-scale-factor`, `force-color-profile`, `disable-lcd-text`, `disable-smooth-scrolling`, `disable-dev-shm-usage`, `ozone-platform`, `ozone-platform-hint`, the three background-throttling switches, `lang`, `enable-logging`, `log-level` | `remote-debugging-port` (`AIORT_CDP=<port>`, a debug build's only), `no-sandbox` (aio-rs decides, `AIORT_SANDBOX=1` keeps it strict), `disable-web-security`, `ignore-certificate-errors`, `proxy-server`, `load-extension`, `user-data-dir`, `js-flags`, `disable-features`… |

- A Linux kernel that forbids unprivileged user namespaces (and a
  `chrome-sandbox` that isn't setuid root) can't run Chromium's sandbox:
  the window then starts with `--no-sandbox` and the log says why, every
  time. The page is the app's own UI and holds no remote content.
  `AIORT_SANDBOX=1` refuses to start instead.
- An X server with too few client slots left would show the window
  empty: the app says so instead of opening.
- A machine with no display at all needs one: `ax dev --display isolated`
  (a nested display), or `ax test` and `ax build --smoke`, which need
  none.

## Inspecting a running window

A debug build's window opens a private door for aio-rs's tools: `ax
surface`, `ax trigger`, `ax shot`, `ax eval` and `ax components` reach it
with no flag and no port. `ax dev <app> --cdp` (or `AIORT_CDP=<port>`)
also opens Chrome's DevTools protocol on loopback for other tools. A
release build opens neither: nothing outside the app can reach its page.

```sh
ax surface notes              # what a person sees, by handle
ax trigger notes draft type milk
ax shot notes --size 390x844
ax clients notes              # its windows
```

## The thin client and other machines

A window on another machine's cells is `[app] mode = "client"` with
`[client] server` naming the server: the same views, over a synced copy
([sync](../state/sync.md)). `ax pin` and `ax pair` link a device without
typing the key; `ax discover` lists the aio-rs apps on this network that
answer.

## Gaps

What aio's Electron client has and aio-rs doesn't yet:

- No custom window chrome (aio's `ui.chrome`: frameless window, own title
  bar): the OS title bar is always there.
- No app menus beyond macOS's standard ones, and no context-menu API.
- One app window per process; more windows are only child windows of
  other sites (`open_window`).
- No `keepServer` / `--keep-server`: split the app into a server and a
  client, as above.
- No helper like aio's `runCommand` with its `stdin` mode: run programs
  with `std::process` in an effect, as shown.
- No `requireSandbox`, `allowLocalPeers` or `ignoreDebugEnv` options:
  `AIORT_SANDBOX=1` is the one switch; local peers reach the app only
  through its door ([transports](transports.md)).

## Common mistakes

- A blocking `Command::output()` in an effect's async body: it blocks a
  runtime thread. Wrap it in `spawn_blocking`.
- Expecting a closed window to leave the server up: it stops the app.
- Adding `--no-sandbox` or `--remote-debugging-port` to
  `AIORT_ELECTRON_ARGS`: both are refused and said; use `AIORT_SANDBOX`
  and `AIORT_CDP`.
- `window.open` from an island's library: blocked unless it follows a
  real click, and then it opens the system browser, never a window.
- Notifications from the page (`new Notification`): refused; use
  `cx.tell` or `desktop::notify`.

## API

`cargo doc -p aio-rs-shell --no-deps --open`: `Window`, `start`, `run`,
`launches`, `desktop` (`Tray`, `Item`, `Notice`, `notify`, `icon`, `icon_state`,
`use_asks`), `page`, `guest` (`Web`, `Keys`, `Child`, `open_window`,
`PERMISSIONS`), `runtime`, `switches`. `cargo doc -p aio-rs --no-deps
--all-features --open`: `aio::tell`, `aio::effect`, `aio::lifecycle`.

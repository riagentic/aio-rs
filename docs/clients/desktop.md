# The desktop window

On Linux, Windows and macOS an aio-rs app's views are drawn by the shared
Electron runtime. The views themselves run in the app's Rust process: a
view tree per window (air's, behind `aio::ui`), whose DOM edits and events travel over a
private socket to a small page in Electron. The page loads from
`aio://app/`, connects to nothing but its own origin (the app's routes,
when it has some: [transports](transports.md#routes-in-the-window)) and
runs no app code; Electron's main process opens the window, the tray,
notifications and dialogs when the app asks.

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
| `aio::Source::for_app(&cfg, &dirs)` | opens the cell as `[app] mode` says: this process's store, or a synced copy of its server's; a client with no server it can reach shows its connect page ([below](#the-thin-client-and-other-machines)) instead of failing (a CLI's client, without the kit, fails) |
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

- Closing the window ends the app (`aio_shell::run` returns), unless
  `Window { keep_server: true }` or the launch's `--keep-server` (aio's
  `keepServer`) keeps it serving: its cells, its port if it has one, its
  local door, until `ax stop`, SIGTERM or Ctrl-C. The log says where it
  serves. A window that crashed keeps it serving too (said), unless a
  required sandbox refused it.
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
  `[build.download_window]` sets its heading, text, question, colours and
  whether it has a Cancel button; with a `[build.splash]` it draws the
  splash's image above its bar (yad, Cocoa, Windows Forms, the page;
  zenity draws no image), in the splash's colours, its line the splash's
  `download_text` ([aio.toml](../reference/aio-toml.md#buildsplash)).
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

## The splash

With a `[build.splash]` ([aio.toml](../reference/aio-toml.md#buildsplash))
the OS draws the app's image before Electron starts, at every start (aio's
splash): Linux with GTK 3 (opened at run time, as Electron links it; no
link-time dependency), macOS with JavaScript for Automation over Cocoa,
Windows with Windows Forms. It opens where the app's window will
(`position`), above other windows, never takes the focus and has no
taskbar or Dock entry. The app's window shows the moment it has painted,
under the splash; the splash goes once it has been up `min_ms`, at its
`max_ms` cap with no window, or when Electron exits.

A first start's Electron download runs on it: the bar and its line
(`download_text`) under the text, a ✕ that cancels the start (`cancel`).
A failed download closes it and asks Retry or Quit.

- `show = "first-start"`: only while Electron downloads; `"never"`: none
  (the download window is then the plain one). `Window { splash_show:
  Some(f) }` decides at run time (aio's `splash.show`): `f` may take 50
  ms; one that panics or takes longer leaves the build's `show`, said.
- On Windows, when Electron is already here, Electron's main process draws
  it (as aio's: the Forms helper shows later than the app's window); the
  Forms splash opens for a download only.
- Where the OS cannot draw it (no GTK, no display) Electron draws it and a
  download shows the download window, the reason said once in the log.
- `ax build` builds it in for an app with a `[build.splash]` (aio-rs-shell's
  `native-splash` feature): an app without one carries none of it, and
  `ax dev` or a plain `cargo build` has Electron draw the splash.
- Each start logs `[aio] splash: visible N ms after it was asked for` and
  why it closed.

## The window's behaviour

| What | How it works |
|---|---|
| Title bar | the OS's, unless `Window { chrome }` (aio's `ui.chrome`) says otherwise: `Chrome::Themed` draws no OS frame and puts aio's title bar in the page (a drag region, minimize, maximize, close, a double click maximizes; its title follows `document.title`), styled by the app's CSS (`.aio-titlebar`, `.aio-titlebar-title`, `.aio-titlebar-button`, `--aio-titlebar-height`, `-bg`, `-fg`, `-hover`, `-close`, `-font`); `Chrome::None` draws no frame and no bar: the app draws its own, with `-webkit-app-region: drag` and `page::minimize`, `maximize`, `close` |
| Size and place | the first size is `Window { width, height }`; the size and place a person leaves it at are kept in `<data>/window.json` and used while the declared size is unchanged. A saved place off every screen moves onto the screen it overlaps most; a size as big as the screen opens maximized. `ax dev --size 390x844` opens one run at that size and saves nothing; so does a launch's `--width=420`, `--height=620` (aio's, either one; the other as declared). |
| First frame | the window shows once it has painted, at most `Window { show_cap_ms }` later (5000 unset; `Some(0)`: at once, aio's `showWhenPainted: false`; a page that never paints is said), on `Window { background: Some([light, dark]) }` until then; `[build.splash]` shows the app's image first ([theming](../ui/theming.md)): its `show` (`always`, `first-start`, `never`) or the app's say at run time, `Window { splash_show: Some(f) }` (aio's `splash.show`) |
| Menu | none (Electron's default menu carries DevTools and reload), except on macOS: the app menu (Hide, Quit), Edit (so Cmd+C works in a field) and Window, all system roles. An app can't add menus yet. |
| Keys | Ctrl+R / Ctrl+F5 reload and Ctrl+P print belong to the window (plain F5 is the page's) unless `Window { keys: Keys { reload, print } }` gives them to the page. An `ax dev` build adds DevTools (F12, Ctrl+Shift+I) and a clean reload; another build (a release, a plain `cargo build`) has DevTools only when its app says `Window { devtools: true }` ([below](#inspecting-a-running-window)). |
| Links | the page never navigates; an `http(s)` link opened after a real click goes to the system browser. A pop-up a script opens by itself is blocked and said in the log. `AIORT_NO_OPEN=1` opens nothing (a contained run). |
| Permissions | the page may copy text and go fullscreen, nothing else, unless `Window { permissions }` names more (`aio_shell::guest::PERMISSIONS`: `clipboard-read`, `media`, `geolocation`…). Its own `Notification` is refused: notifications come from the app. A denial is said once per permission, naming the fix: `permission "midi" DENIED to the app's own page: Window { permissions: &["midi"] } grants it`. |
| Unload | a page's `beforeunload` veto is ignored (said once in the log) unless `confirm_unload: true`, then Leave / Stay is asked |
| Page messages | the page's console lands in the app's log as `[page]` lines at its level (`log` and `info` as INFO, `debug` in a debug build only), with the file and line (8 KB each, 100 a second); `ax logs <app> --client` shows only those |
| Hangs and crashes | a page that stops answering marks the app's health degraded ("window") until it answers again. A crashed renderer is reloaded on a new connection; the fourth crash in a minute closes the app with a dialog, and a problem report is saved. |
| GPU | a runtime that died because the GPU failed is remembered (`<data>/electron/gpu-failed`): the next start draws in software. `AIORT_GPU=software` or `hardware` decides by hand. |
| Back | a mouse's back button (Windows, Linux) is the app's Back, as Android's is ([routing](../ui/routing.md)) |
| Identity | each app is its own app to the OS: its window class and Wayland app id, its taskbar entry and, on macOS, its own Dock icon (the window runs through the app's bundle) |
| macOS Dock | opening the running app again (its Dock icon, Finder, `open`) brings a window hidden to the tray, or minimized, back |
| Sandbox | Chromium's sandbox, always; a Linux host that can't run it (user namespaces closed, no setuid helper) starts the runtime with `--no-sandbox`, said, unless `Window { require_sandbox: true }` (aio's `requireSandbox`) or `AIORT_SANDBOX=1`: then the app stops with the reason |
| Menu entry | Linux: `Exec="…" %U`, so "Open with" hands the files to the app (`aio_shell::launches`) |

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
one: `aio::config!()` builds in `assets/icon.png` beside `aio.toml` and
`Window::of(&cfg)` wears it; a `Window` with no icon wears the app's
monogram (its name's first letter, deep on a pastel gradient, aio's
generated icon), drawn by the shell once and kept while its letter and hue
stay, in the app's hue (`Config::hue`: `[ui] hue`, else its id's), the one
its page, splash and download window share. It is on the title bar, the
taskbar and Alt-Tab, the tray, notifications, the window bundle in the
macOS Dock, the download window (Windows, macOS, GTK) and a browser tab's
favicon. A first `assets/icon.png` is built in the next time the crate
compiles (an edit, or `cargo clean -p <app>`); a changed one is noticed by
itself.

### Changing the icon

`aio_shell::desktop::icon(Icon::Window | Icon::Tray, frames, fps)` changes
an icon from now on: one PNG, or up to `ICON_FRAMES` (64) PNG frames of at
most `ICON_BYTES` (512 KB) each, looped by the runtime at `fps` (at most
`ICON_FPS`, 12). No frames: the app's own icon again. A bad image is
refused where it is passed. A cell asks for the window's, the tray's and
the tray's tooltip in one with `cx.tell(Tell::Icon { window, tray,
tooltip, fps })` (aio's `appIcon()`: each frame base64, a `data:` URL or
the app's URL path of a `[build] assets` file, `"/icons/busy.png"`; `None`
leaves that one as it is), so a server's state can badge every window
showing it.
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
- A synced client with no window (a CLI on a served cell, aio's
  `connectCli`) can show no card: it says the sentence in its log,
  `INFO [cli] notify: Export — the report is ready` (`Tell::line`). A
  server with no window and no client says `nobody to tell`.

## Dialogs, clipboard, print

`aio_shell::page` opens the system's dialogs from a view
([views](../ui/views.md#the-page)): `pick(&Pick { .. })` and
`pick_many` (files of some types, or a folder), `save(&Save { .. })` (a
path to write, after the dialog asked before replacing a file), `copy`,
`print` (CSS backgrounds and colours printed too). A dialog is owned by
the window; with no window (a test) the desktop's own dialog tool
answers. An error (no dialog tool, no desktop session) is an error, never
a cancel.

The window's verbs (aio's `__aioWindow`): `page::minimize()`,
`maximize()` (or back), `close()`, `hide()`, `show()`. `close` is the
title bar's own close: a close-to-tray app hides, another stops, and the
window's place is saved. The page's own `window.close()` takes the same
path.

## Other sites in the app

Someone else's page shows in a view with `aio::kit::Browser` (needs
`Window { web: Web { browser: true, .. } }`) or in a window of its own
with `aio_shell::open_window(&Child { url, preload, partition, origins,
size, permissions, unsandboxed })` (needs `windows: true`). Both run
sandboxed (a child window may ask not to, if `Web { unsandboxed_windows }`
allows it), in a session that is never the app's. `aio_shell::clear_partition("persist:name").await`
(aio's `clearPartition`) wipes what one of those sessions stored, its
cache and its HTTP sign-ins: an app that locks drops its guests' data. The
app's own session is refused, by any spelling. See
[web: embedded pages](web.md#embedded-pages).

## Running a program from the app

A desktop app that drives another program (a converter, `git`, a backup
tool) runs it with `aio::process` (aio's `spawn` and `runCommand`) in an
effect: the call that asks for it commits at once, the program runs off
the cell's thread, and its result comes back as another call, saved in
the log like any input.

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

#[methods]
impl Jobs {
    pub fn run_job(&mut self, cx: &Cx, program: String, args: Vec<String>) -> Result {
        ensure!(!self.running, "a job is running");
        self.running = true;
        self.error = None;
        cx.run(
            Effect::new("job", move |h: Handle<Jobs>| async move {
                // Its whole tree killed after 10 minutes; what it wrote kept.
                let ran = aio::process::Command::new(&program).args(&args).run(Some(Duration::from_secs(600))).await;
                let got = match ran {
                    Ok(out) if out.status.success() => Ok(out.stdout.trim().to_string()),
                    Ok(out) => Err(format!("exited with {}: {}", out.status, out.stderr.trim())),
                    Err(e) => Err(e.to_string()),
                };
                _ = h.ended(got);
            })
            .mode(Mode::First),
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

| `aio::process` | What it does |
|---|---|
| `Command::new(p).args(..).cwd(..).env(k, v)` | the program, its arguments as a list (never through a shell), its folder, its environment: the app's without aio-rs's own `AIORT_*` settings |
| `.run(timeout)` | to its end: `Output { status, stdout, stderr, timed_out }`. A timeout (on the wall clock, also in `TestCells`) kills the whole tree on time and keeps what it wrote. A program that fails is an `Output`; one that can't start is an error. |
| `.stdin()` | a pipe to its stdin: `child.write(bytes).await`, `child.close_stdin()` (a filter like `sort` goes on from its end). Off by default: a program reading a pipe nobody writes waits forever. |
| `.on_line(f)` | each line of its output (`\n`, `\r\n` or `\r`: a progress bar that rewrites its line) to `f(line, Stream)` as it comes, with `run` too |
| `.spawn()` | a `Child` while it runs |
| `child.pause()`, `resume()` | the whole tree stopped and resumed (SIGSTOP to its group; Windows: an error) |
| `child.kill().await`, `wait().await` | the tree asked to end (SIGTERM), killed after its grace (2 s, `.grace(d)`); its exit once its output is read |

- **The whole tree**: each program runs in a process group of its own (a
  job object on Windows), so a kill reaches what it started too, never
  only the process you can see.
- **Dropped is killed**: a `Child` dropped (an effect aborted or timed out
  at its next await) takes its tree with it. One still running when the
  app stops is killed then, and the log names it.
- **No console windows**: on Windows every program starts without a
  console window, and a release app (no console of its own) adopts one
  hidden console at start, so a console program started any other way
  (`std::process`, a library) opens none either.
- **Choosing the program**: run a path the app knows (a bundled tool, one
  the person picked with `page::pick`), never a string from the network.
- **Long jobs**: `h.busy("job")` and `h.pending(..)` are what a spinner
  reads. A step-by-step `async fn` method is cut off after
  `aio::effect::CALL_CEILING` (30 s) per await unless marked `#[long]`
  ([effects](../state/effects.md#heavy-work-retries-and-timeouts)).
- **Talking to it**: hand its lines back as calls (`h.progress(line)`)
  from `on_line`, coalesced if they are many; a stop is `cx.abort(key)`,
  which drops the effect and kills the tree ([cookbook](../basics/cookbook.md#22-a-subprocess-with-a-timeout-and-a-stop-button)).

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
| `disable-gpu`, `disable-gpu-compositing`, `disable-software-rasterizer`, `use-gl`, `use-angle`, `enable-unsafe-swiftshader`, `ignore-gpu-blocklist`, `force-device-scale-factor`, `force-color-profile`, `disable-lcd-text`, `disable-smooth-scrolling`, `disable-dev-shm-usage`, `ozone-platform`, `ozone-platform-hint`, the three background-throttling switches, `lang`, `enable-logging`, `log-level` | `remote-debugging-port` (`AIORT_CDP=<port>`, an `ax dev` build's only), `no-sandbox` (aio-rs decides, `AIORT_SANDBOX=1` keeps it strict), `disable-web-security`, `ignore-certificate-errors`, `proxy-server`, `load-extension`, `user-data-dir`, `js-flags`, `disable-features`… |

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

A dev build's window (`ax dev`, or a build with the `dev` features of
aio and aio-shell) opens a private door for aio-rs's tools: `ax
surface`, `ax trigger`, `ax shot`, `ax eval` and `ax components` reach it
with no flag and no port. `ax dev <app> --cdp` (or `AIORT_CDP=<port>`)
also opens Chrome's DevTools protocol on loopback for other tools. A
release build opens neither, and reads no debug setting from the
environment (aio's `ignoreDebugEnv`, always on): nothing outside the app can
reach its page.

A release whose app says `Window { devtools: true }` has DevTools on F12
and Ctrl+Shift+I, and opens the protocol for one launch on
`--cdp=<port>` (aio's `--cdp`, typed: a variable left over from another
app opens nothing), with a warning that any process of the machine can
drive the window while it is open. Without it a release refuses `--cdp`,
naming the setting. Security first: aio had DevTools in every build. Other processes of this user reach its cells through its
local door with a client's rights (`ax state`, other apps as peers);
`[server] local_peers = false` (aio's `allowLocalPeers` off) opens no door
in a release build.

```sh
ax surface notes              # what a person sees, by handle
ax trigger notes draft type milk
ax shot notes --size 390x844
ax clients notes              # its windows
```

## The thin client and other machines

A window on another machine's cells is `[app] mode = "client"`: the same
views, over a synced copy ([sync](../state/sync.md)). Its server is,
first to last: the launch's `--server-url=<url>`, the link the client
keeps (pairing or the connect page, `<data>/paired.json`, 0600), then
`[client] server`.

### The connect page

A window's client (`Source::for_app` in a binary with the kit, the
template's, unchanged since 0.2) never fails for want of a server. With
none named, `--connect`, one it can't reach (and no copy saved from an
earlier run) or one that refuses its key, the window
shows the connect page in place of the app, desktop and Android alike
(`aio::kit::ConnectPage`, aio's thin client):

- **An address** (`192.168.1.20:8100`), with the server's **pairing code**
  when it has a key (`ax pair <app>` on the server's machine makes one): a
  code pairs (PAKE, the root pinned), no code connects with no key, over
  plain sockets: to a server open on purpose (`[server] open`). The app's
  own key never goes to a server that nothing verified.
- **The app's servers on this network** (`[server] discover = true`), each
  asked on every network this machine is on: a click connects, one marked
  `needs its code` takes its address into the form for the code.
- **Recent servers** (ten, `<data>/servers.json`, 0600: a paired one's key
  is in it): a click connects again, with its key and pin, no code; `×`
  forgets one.
- **Why it shows**, at the top: no server yet, the reason it can't be
  reached, a refusal.

The server picked here is reached with no `[capabilities] network`
entry: the person's pick is the consent, for that host and port alone
(as aio's thin client); the app's other requests still follow the list.

The page goes the moment a server answers, and the app shows in the same
window. Another server picked while one is connected (a recent one, a
new pairing) takes over at once: the live link drops and the copy
follows the new server. A client offline with a copy saved shows the app, not the page:
offline first. A client without the kit (a CLI) still fails, naming `ax pair`.

```sh
my-client --connect                                   # the page first
my-client --server-url=192.168.1.20:8100              # this server, this run
AIORT_TOKEN=<key> my-client --server-url=192.168.1.20:8100   # with its key
```

`--server-url` takes `host:port` or an `http(s)://` / `ws(s)://` URL whose
`?token=` is the key (else the pairing's when it is that server, else the
client's own: `AIORT_TOKEN`, else `data/sync.token`). A key in the URL is
said as a warning: every user of the machine sees a command line, so give
it in `AIORT_TOKEN` or the file. A TLS server is reached only pinned to the root its pairing
pinned: pair with it first.

### A certificate that changed

A paired server is pinned to its root. One that shows a certificate the
root doesn't cover (its machine made a new root) is asked, before anything
else is sent, to prove it holds the app's key (aio's
`POST /__aio/pin-proof`: an HMAC over a fresh nonce and the certificate
it serves, so a relayed answer names the wrong one). A server that proves
it is pinned again and connected to, said in the log; one that can't is
refused, its key never sent, and the connect page says to pair again. A
weak key (one `ax` warns of) is never put to that proof: whoever answers
at the address could guess it from the client's half offline. Pair again.
`[client] strict_pin = true` (or `--strict-pin` for one launch, aio's
`strictPin`) refuses a changed certificate outright, until the server is
paired again. Unlike aio's, a client never connects without its key: a
copy without it would show nothing.

## Gaps

aio has no app menus beyond macOS's standard ones or a context-menu API,
and opens one app window per process, as aio-rs does.

## Common mistakes

- A blocking `std::process::Command::output()` in an effect's async body:
  it blocks a runtime thread, and a kill misses what the program started.
  Use `aio::process`.
- Expecting a closed window to leave the server up: it stops the app
  unless `keep_server`.
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
`use_asks`), `page` (the dialogs, `print`, the window's verbs), `guest`
(`Web`, `Keys`, `Child`, `open_window`, `clear_partition`,
`PERMISSIONS`), `runtime`, `switches`. `cargo doc -p aio-rs --no-deps
--all-features --open`: `aio::tell`, `aio::effect`, `aio::process`,
`aio::lifecycle`.

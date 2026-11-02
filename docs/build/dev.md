# Development mode

`ax dev <app>` is the loop you work in: it builds the app, runs it, and
puts every saved edit into the running app. This page is how that works,
every flag, the flags an app's own binary takes at launch, and which code
ends up where (the window, the web build, a server). Building what you
ship: [build](README.md).

## The loop

```sh
ax setup                         # once: fetches dx, which hot-patching needs
ax dev notes                     # build, run; edits hot-patch in
```

What happens to a saved file:

| You saved | With dx (`ax setup` fetched it) | Without dx |
|---|---|---|
| a view's `rsx!` markup | shown at once, nothing compiled | rebuild + restart |
| Rust in any workspace crate (views, cell methods, effects) | hot-patched into the running process in about a second; cells and view state stay | rebuild + restart |
| a stylesheet `aio::use_css!` reads | shown at once, no compile ([theming](../ui/theming.md)) | shown at once |
| a source `[build] css` reads (Tailwind, Sass) | its toolchain runs, then the stylesheet shows | the same |
| a `Cargo.toml` (a new dependency, a feature) | full rebuild, the app restarted | rebuild + restart |
| a change a patch can't take (a changed type layout, a new `static`) | dx says so; full rebuild + restart | rebuild + restart |

A burst of saves is one change: it settles after 100 ms of quiet, and
after 500 ms at most however busy the writer is (a formatter, a
`git checkout`). The terminal names the files that changed and times the
build. An editor's own files are no change: vim's `4913` probe and swap
files, emacs' `#x#`, `x~`, `*.tmp`, JetBrains' safe-write copies, hidden
files. After each burst the aio checks of each saved `.rs` file (keys,
accessible names, hooks: `ax lint`'s per-file ones) are said, warnings
and errors only.

Without dx, ax watches the app's `src`, `assets` and `Cargo.toml`, and
every file the last build read outside them (cargo's dep-info beside the
binary): `aio.toml`, `include_str!` files, the sources of an aio-rs
checkout the app depends on by path. `--web` does the same for the wasm
build. A watcher that went deaf (no event for 60 s, a sentinel touched
every 30 s) is made again, three times at most, and says so.

A rebuild that succeeds restarts the app: first it is asked to stop over
its door (calls in flight finish, effects commit, on every OS), then its
cells come back from their log, the views start again. A build that
changed nothing (the same binary) keeps the app running. A change that
comes within 1 s of each of 5 restarts in a row is a loop (a formatter,
a generator, another watcher), not a person: each next rebuild waits 5 s
and the terminal names the files. A build that fails leaves the running app as
it was and shows the compiler's errors on its page (an overlay, until a
good build or patch lands), in the terminal and in `logs/build.log`
(`ax errors notes` prints it). The next good build says `fixed` and takes
the errors off the page, restart or not. `ax errors notes` groups lines
under each start and each hot-patch (new code), and `--since reload`
reads from the newest of them. An error ax knows the fix for gets a hint
under it: a type a cell keeps that isn't serializable is told to wear
`#[aio::data]`.

Hot-patching needs three things, and `ax dev` says which one is missing
and falls back to rebuilds:

- dx on `PATH` or in aio's tool home, at the Dioxus version aio-rs is
  built on (0.7.10); another version is refused for patching.
- The app's launcher depending on `aio-rs-shell`: `ax dev` turns on its
  `dev` feature (`aio-shell/dev`), the part that takes patches in. A
  release build never has it.
- A debug build: patches go into the `desktop-dev` profile's binary.

### The session

- One `ax dev` per app. A dx an earlier session left running (its
  terminal closed) is stopped first, by its recorded pid and start time
  (on every OS: a pid the OS handed out again is no match), never by name.
- Closing the app's window ends the session; so does `ax stop notes`
  (the app stopped the graceful way, dx with it).
- `ax dev notes --detach`: the session in the background (aio's
  `am start` of a dev run), its output in the app's `logs/dev.out`; `ax
  status notes` adds `ax dev: watching`, or that the last build failed and
  it waits for a fix.
- `ax restart notes` while a session runs asks it for a full rebuild and
  a fresh start, with the same flags on the same display; the app is
  asked to stop over its door first (dx alone would give it 1 s).
- dx's own output goes to `logs/dev.log`; the terminal gets ax's compact
  lines (`--tui`: dx's screen and keys instead).
- All builds (`ax dev`, `ax test`, `ax lint`, `ax build`) share one build
  folder (`CARGO_TARGET_DIR`, else one per machine in aio's tool home), so
  aio-rs and its dependencies are compiled once for every app on the
  machine. Two apps with the same binary name in that folder: the first
  claims it, the second is refused until it is renamed.

### Its data

`ax dev` runs the app on the same home as the installed app with that
`[app] id` (`ax data notes` says where). To try something without
touching that data:

```sh
ax dev notes --instance try            # a private copy: its own data, lock and socket
ax dev notes --instance try --fresh    # the same copy, emptied first
```

`--fresh` only ever throws away a private instance's data, never the
app's own, and never while it runs: without `--instance` it is refused.
A port of an aio app may name its own debug home (`[home] dev`,
[files](../persistence/files.md)); then a debug build never opens the
real data.

## Every flag

| Flag | Does |
|---|---|
| `--instance <n>` | a private copy with its own data, lock and socket |
| `--fresh` | with `--instance`: that copy on new, empty data |
| `--size 390x844` | the window at that size for this run; its saved size and place untouched |
| `--display auto` | the default: a person at a terminal gets their own desktop; a caller with no terminal (an agent, a script) a nested display |
| `--display isolated` | always a nested display (Xephyr, else Xvfb); refused when neither is installed |
| `--display current` | the caller's display, as is (`AIORT_DISPLAY=current` too) |
| `--display :N` | a display you run yourself |
| `--watch=false`, `--no-watch` | build and run once, rebuild on nothing |
| `--watch=src/ui,assets` | rebuild on changes under those folders of the app only (with dx too: then ax rebuilds and restarts instead of hot-patching, since dx watches all of the app) |
| `--detach` | the session in the background, output in `logs/dev.out`; `ax stop` ends it |
| `--verbose` | the app's log at `debug` (`AIORT_LOG=debug`; the boot report's `log` row says so) |
| `--title "Notes (QA)"`, `--title=…` | the window's title for this run (`AIORT_TITLE`) |
| `--cdp[=port]` | the window opens the DevTools protocol on loopback (a free port unless given) |
| `--tui` | dx's own screen in the terminal |
| `--web [--port N] [--expose]` | the web build instead ([below](#the-web-build-while-you-work)) |
| `--android [--device serial] [--native]` | on an emulator or phone: the web build in a dev APK installed once, each edit reloading it (`--native`: the app itself, rebuilt for that CPU on every save) |

Words after the app go to the app (`ax dev notes -- --route /users/7`);
ax's own flags above are taken out first. `--display`, `--fresh`,
`--size`, `--tui`, `--verbose`, `--title` and `--detach` never reach the
app. Args with spaces reach it intact under dx too (dx splits its own
`--args` on spaces: ax hands those to the debug build in
`AIORT_DEV_ARGS`, which runs again with them).

A contained run (a nested display) opens nothing in the person's browser
(`AIORT_NO_OPEN`), gets software WebGL and a private D-Bus, so no
notification or portal dialog lands on the person's desktop. `ax shot`,
`ax eval`, `ax surface` and `ax trigger` reach a dev window without
`--cdp`; it is for other tools.

## The web build while you work

```sh
ax dev notes --web
```

Builds the app's library for `wasm32` (debug), serves it on a free
loopback port (`--port N`: that one) and prints the address. `--expose`
serves it to the network (`0.0.0.0`) over TLS behind a key made for the
run: open the printed `https://<lan-ip>:<port>/?token=…` link on a phone
or another machine (the browser asks to trust aio-rs's own certificate;
the link sets a cookie, so the page's own requests need no key in their
address). A saved stylesheet shows at once in
every open tab; any other saved source builds the bundle again and the
tabs reload. A failed build keeps the last page and shows the errors on
it. There is no hot-patching in the browser: every Rust edit is a new
bundle. It needs the `wasm32-unknown-unknown` target and the
wasm-bindgen the lock names (`ax setup` fetches it).

## On a phone while you work

```sh
ax dev notes --android                  # the attached emulator (before a phone), else AIO_AVD booted
ax dev notes --android --device R58N…   # one device of several (or ANDROID_SERIAL)
ax dev notes --android --native         # the app's own library, rebuilt on each save
```

As aio's `dev:android`: a thin dev APK, installed once beside the real
app ("Notes (dev)"), shows the page `ax dev --web` serves, reached over
`adb reverse`; each edit rebuilds the page on this machine and reloads
it on the device, no APK built again. `--native` (or an app without a web
build): a debug dev copy of the app, built for the device's CPU only and
installed again on every save. The app's log, its page's console and a
crash come to the terminal as `[device] …`. The details:
[targets](targets.md#the-dev-loop).

## The app's own launch flags

A built app's binary takes a few flags of aio's own, whatever else it
parses. They are the same under `ax dev`, `ax start`, an installed copy
and a shortcut:

| Flag | Does |
|---|---|
| `--home <dir>` | run from that data home |
| `--profile <name>` | a second home beside the first (`AIORT_PROFILE` too) |
| `--instance <name>` | a private copy (as `ax dev --instance`) |
| `--takeover` | the running copy is asked to stop, this one takes its place |
| `--port <n>` | serve on that port instead of `[server] listen`'s |
| `--host <addr>` | listen on that address |
| `--expose` | listen on every interface (`0.0.0.0`), over TLS |
| `--route <path>` | open the window at that route ([routing](../ui/routing.md#opening-at-a-route)) |
| `--quit` | ask the copy on these dirs to stop, the graceful way, and exit |
| `--version` | print `<id> <version>` and exit |
| `--install`, `--uninstall` | Windows: install for this user, or take it away |

A second launch of a running app hands its arguments to the first, which
comes to the front, and exits. aio takes its own flags out before the
app sees the rest, so the app's parser never refuses them:

```rust
use aio::paths::Ask;

# fn main() {
let args: Vec<String> = ["--instance", "try", "--port=9000", "--verbose", "notes.md"].map(String::from).into();
let ask = Ask::parse(&args).unwrap();
assert_eq!((ask.instance.as_deref(), ask.port.as_deref()), (Some("try"), Some("9000")));
assert_eq!(Ask::app_args(&args), ["--verbose", "notes.md"], "what the app's own parser reads");

// A launch that opens the app to the network turns TLS on: the key never crosses a LAN in the clear.
let server = aio::config::Server { listen: Some("127.0.0.1:7420".into()), ..Default::default() };
let l = aio::config::listen(&server, &Ask { expose: true, ..Ask::default() }).unwrap();
assert_eq!((l.addr.as_str(), l.exposed, l.tls), ("0.0.0.0:7420", true, true));
assert!(aio::config::listen(&server, &Ask { expose: true, host: Some("::1".into()), ..Ask::default() }).is_err());
# }
```

Everything else that varies per launch is a variable, never a config file
read at run time: `AIORT_DATA` (one folder for all of the app's files),
`AIORT_TOKEN` (a served app's key), `AIORT_UPDATE_CHANNEL`, `AIORT_GPU`
(`software` when a GPU driver draws wrong), `AIORT_LOG` (the log's level
for this run: `trace` … `error`), `AIORT_TITLE` (the window's title). The boot line in `logs/app.log`
says each setting and who decided it; `ax doctor notes` prints them.

## Which code goes where

An aio-rs app is one Rust codebase. What ends up in each build is decided
by the compiler, not by import rules:

| Code | The window (desktop, Android) | The web build (wasm) | A server |
|---|---|---|---|
| cells, methods, views | yes | yes | yes |
| `#[cfg(not(target_arch = "wasm32"))]` items | yes | left out | yes |
| `#[cfg(target_arch = "wasm32")]` items (the page's start) | no | yes | no |
| `#[server]` fields | in the process that owns the cell | never sent | yes |
| server functions | called over the link from a copy | called over the link | run here |
| a feature's modules (`aio::http`, `aio::auth`) | with the feature | with the feature, where it compiles | with the feature |

On the desktop and on Android the views run natively, in the app's own
Rust process; the window only draws them. Files, SQL, threads and every
crate work there. In the web build the views and cells run as wasm in
the page: what a browser has no way to do (files, the store's history,
SQL, threads, a server, anything native) is not in the page, so put it
behind `#[cfg(not(target_arch = "wasm32"))]`, or in a
[server function](../state/sync.md#server-functions-and-routes) the page
calls.

```rust
use aio::prelude::*;

#[cell(persist)]
pub struct Notes {
    pub items: Vec<String>,
}

#[methods]
impl Notes {
    pub fn add(&mut self, text: String) {
        self.items.push(text);
    }
}

/// Native only: the web build never compiles it, so it may touch files.
#[cfg(not(target_arch = "wasm32"))]
pub fn export(n: &Notes) -> std::io::Result<String> {
    let text = n.items.join("\n");
    // std::fs::write(path, &text)?;  a file is fine here
    Ok(text)
}

# fn main() {
let h = Handle::<Notes>::memory();
h.add("milk".into()).unwrap();
assert_eq!(export(&h.state()).unwrap(), "milk");
# }
```

The template does this split for you: the library (`src/lib.rs`) is the
app; `app/src/main.rs` is the desktop launcher, `app/src/lib.rs` the
Android one, and the `#[wasm_bindgen(start)]` function in `src/lib.rs`
the web build's. The library's `Cargo.toml` takes `aio` with `features =
["web"]` only for `cfg(target_arch = "wasm32")`
([project](../basics/project.md#cargo-features)).

### Files the views use

| What | How |
|---|---|
| stylesheets | `aio::use_css!(dark; "assets/style.css")`: built into the binary in a release, read from disk (and live) in a debug build |
| images, fonts, data the app ships | `include_bytes!` / `include_str!`: in the binary, on every target |
| an image on the user's disk, in the desktop window | `aio::ui::local_file_url("/abs/path.png")`: the window serves image files only, from `aio://app/file/…`; `file://` is blocked |
| a folder a served app hands out | `[server] dirs` (behind the key) or `[server] public_dirs` ([aio.toml](../reference/aio-toml.md#serverdirs-serverpublicdirs)) |
| files the user saves | `aio_shell::page::save` (a page's own download is never saved behind the app's back) |

```rust
# fn main() {
assert_eq!(aio::ui::local_file_url("/srv/a b.png"), "aio://app/file/srv/a%20b.png");
# }
```

A release build reads no file from the build machine: paths of the
build machine never end up in a shipped binary (`ax build` checks and
refuses one that holds the builder's home).

## Running an app from its source

There is no `ax` one-liner that takes a repository link. From a clone:

```sh
git clone https://github.com/example/notes && cd notes
ax setup                 # what the machine lacks
ax start notes           # a build of this checkout, in the background
ax install notes         # or: a release bundle, installed for this user
```

`ax start` builds in the workspace and runs the result, waiting until it
is up; `ax install` builds this OS's bundle and installs it as a user
would ([build](README.md#install-and-try-it)). The app's aio-rs comes from
git (or a checkout, `--aio`): [setup](../basics/setup.md#where-an-app-gets-aio-rs-from).

## Common mistakes

- `ax dev notes --fresh` without `--instance`: refused; `--fresh` never
  empties the app's real data.
- Expecting `ax dev` to keep in-memory view state across a restart: a
  hot patch keeps it, a rebuild starts the views again (the cells' state
  is saved).
- A dx of another version on `PATH`: ax says so and rebuilds instead of
  patching; `ax setup` fetches the right one.
- A CSS edit that doesn't show: the stylesheet is kept in a `once`
  (made once) instead of `aio::use_css!` in the render.
- Native-only code in a view the web build compiles: put it behind
  `#[cfg(not(target_arch = "wasm32"))]` or in a server function.
- Driving `ax dev` from a script or agent with `--display current`: the
  window lands on the person's screen. Leave `auto`.

## Limits

- No hot-patching in the web build or on Android: each edit is a build
  and the page reloads, slower than a patch (on Android, of the page only,
  unless `--native`).
- dx's own restarts (a change it can't hot-patch) give the app 1 s; ax's
  (a `Cargo.toml`, `ax restart`) ask it to stop first.
- No flag that turns persistence off for a run: use `--instance` with
  `--fresh`, or `AIORT_DATA` pointing at a throwaway folder.
- No run-from-a-link command: clone, then `ax start` or `ax install`.

## API

`cargo doc -p aio-rs --no-deps --all-features --open`: `aio::paths::Ask`,
`aio::config::listen`, `aio::use_css!`, `aio::ui::local_file_url`;
`cargo doc -p aio-rs-shell --no-deps --open`: `start`, `run`, `page::save`.

# Development mode

`ax dev <app>` is the loop you work in: it builds the app, runs it, and
puts every saved edit into the running app. This page is how that works,
every flag, the flags an app's own binary takes at launch, and which code
ends up where (the window, the web build, a server). Building what you
ship: [build](README.md).

## The loop

```sh
ax dev notes                     # build, run; edits reach it live
```

What happens to a saved file:

| You saved | What happens |
|---|---|
| a view's `rsx!` markup | sent to the window at the first save event (`Aio.hot`), its views built again with their state: ~3 ms, nothing compiled |
| Rust in any workspace crate (views, cell methods, effects) | a thin patch built by ax against the first ("fat") build, applied in the running process: ~0.3 s; cells and view state stay |
| a stylesheet `aio::use_css!` reads | read by the window within 20 ms; nothing built ([theming](../ui/theming.md)) |
| a source `[build] css` reads (Tailwind, Sass) | its toolchain runs, then the stylesheet shows |
| a `Cargo.toml` (a new dependency, a feature), or a patch that fails | full rebuild, the app restarted |

A burst of saves is one change: it settles after 100 ms of quiet, and
after 500 ms at most however busy the writer is (a formatter, a
`git checkout`). A Rust patch doesn't wait for the quiet: it compiles from
the first save, and is applied if the burst was just that save (each file
as the compile read it); else it is dropped and the settled burst
compiled. The terminal names the files that changed and times the
build. An editor's own files are no change: vim's `4913` probe and swap
files, emacs' `#x#`, `x~`, `*.tmp`, JetBrains' safe-write copies, hidden
files. After each burst the aio checks of each saved `.rs` file (keys,
accessible names, hooks: `ax lint`'s per-file ones) are said, warnings
and errors only.

ax watches the app's `src`, `assets` and `Cargo.toml`, and every file
the last build read outside them (cargo's dep-info beside the binary):
`aio.toml`, `include_str!` files, the sources of an aio-rs checkout the
app depends on by path. `--web` does the same for the wasm build. A
watcher that went deaf (no event for 60 s, a sentinel touched every
30 s) is made again, three times at most, and says so.

A view keeps its state by its place among the mounted components (its
name, its key, or its turn among its siblings); a view whose hooks changed
starts afresh from the first that differs. What the page held (focus,
scroll, a field's selection) is drawn anew.

A backtrace (a panic's) names the functions of patched code: with MSVC
each patch carries its PDB and the resolver learns of it; on Linux, macOS
and MinGW std's resolver lists the loaded libraries at its first
backtrace, so a patch loaded after that shows as `<unknown>` frames until
a restart.

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

### Hot-patching

`ax dev` builds and applies the patches itself; no other tool is
needed. It needs:

- The app's launcher depending on `aio-rs-shell`: `ax dev` turns on its
  `dev` feature (`aio-shell/dev`), the part that takes patches in
  (`aio-rs-hotpatch`). A release build never has it.
- A debug build: patches go into the `desktop-dev` profile's binary.
- Linux, macOS or Windows (x86_64; MSVC, or MinGW with its gcc on `PATH`
  or rustc's bundled one). On Windows the app runs at a fixed base without
  ASLR in `ax dev`; a system that forces ASLR gets a rebuild and restart.

An edit in a library crate (the template's app crate is one: its
launcher is a separate binary) is compiled alone and applied at once:
the launcher's running objects, and those of the crates built on the
edited one, are linked into the patch as they are, so their calls reach
the new code. Those crates and the launcher are compiled after, off the
time to the screen: when their objects come out as the running ones (the
same code and relocations, debug info apart: they compile in nothing of
the edit) nothing more is sent; when they differ (a generic or an
inlined function of the edited crate) a second patch follows with them,
and the terminal says so. So an edit in a page crate compiles that crate
alone, not the app above it. One exception: an edit that drops a symbol
an earlier patch added (a generic's copy the crates above may share) is
compiled with them, before the patch.

A patch links only the library objects (codegen units) it needs: those
whose code differs from the running binary's (section by section, its
bytes and relocations), the ones that call into those, in any of the
app's libraries built on the edited one (from the running binary's copy
when that library wasn't compiled), up to the roots, and any defining a
symbol the running binary's linker dropped. The rest the patch reaches in
the running binary. In a crate of about 74 000 lines that is 6 of its 256
objects, and the patch's stub and link take ~0.4 s where they took
0.8-4.6 s (the 140 MB library written out whole).

### The session

- The next start's build follows the session. Once a patched session has
  been quiet for 2 s, ax brings the fat build up to date behind it at low
  priority (nice 10; Windows' below normal), linked beside the running
  binary, its patches untouched; a save stops it. A session that ends
  behind it (its window closed, `ax stop`, Ctrl-C, the terminal or console
  closing) starts `ax dev <app> --warm` in the background, its words in
  the build folder's `aio/warm-<app>.log`. Either way the next `ax dev`
  launches what was built: ~0.35 s.
  On macOS the system checks a new binary whole at its first start, and
  a binary the linker just wrote isn't in memory yet, so that check reads
  it from disk slowly: ax reads it through once before it starts it
  (~40 ms for 60 MB), and that first start takes ~0.65 s instead of
  ~1.15 s (a 60 MB binary on Apple silicon; the check itself scales with
  its size). An ad-hoc signature (`codesign -s -`) gets the same for
  ~170 ms; the same path or inode gets nothing.
- A start after code edits made with `ax dev` off (a pull, an editor
  without a session): the last fat build starts at once, nothing built or
  linked, while the edited crates compile beside it; the patch goes in as
  its window comes up, state kept (~0.45 s for the template, where a
  build and a launch took 1.0 s). Code that ran as the app started (its
  `main`, a cell's first load) ran as built before, as with an edit in a
  session; `ax restart` runs it all anew. The next start's build follows
  behind, as after any patch. A change no patch takes builds as before: a
  `Cargo.toml`, the lock, `aio.toml` or another file the build read, a
  type's shape, a build that is running (`--warm`).
- One `ax dev` per app. An app an earlier session left running (its
  terminal closed) is stopped first, by its recorded pid and start time
  (on every OS: a pid the OS handed out again is no match), never by name.
- Closing the app's window ends the session; so does `ax stop notes`
  (the app stopped the graceful way).
- `ax dev notes --detach`: the session in the background (aio's
  `am start` of a dev run), its output in the app's `logs/dev.out`; `ax
  status notes` adds `ax dev: watching`, or that the last build failed and
  it waits for a fix.
- `ax restart notes` while a session runs asks it for a full rebuild and
  a fresh start, with the same flags on the same display; the app is
  asked to stop over its door first.
- A first build with crates to compile shows a splash window at once
  (150 ms after it starts): cargo's own count of what it builds and the
  crates it is on, then the app's window in its place. A build that fails
  shows its errors there until you save a fix; an app that doesn't start
  says so there, with its own reason (its last `Error:`, `ERROR` or panic
  line from `logs/app.log`), as the terminal's `ended` line does. A start
  with nothing to compile (the last build reused)
  shows none. The splash is ax's own window on the shared runtime, never
  part of the app; `AIORT_NO_SPLASH=1` leaves it out.
- All builds (`ax dev`, `ax test`, `ax lint`, `ax build`) share one build
  folder (`AIORT_TARGET_DIR` or `CARGO_TARGET_DIR`, else one per machine
  and toolchain in aio's cache home, `~/.cache/aio-rt/target/<toolchain>`
  on Linux), so aio-rs and its dependencies are compiled once for every
  app on the machine. The toolchain is the workspace root's
  (`RUSTUP_TOOLCHAIN`, else its toolchain file, else what rustup picks
  there), and ax runs every cargo on it: a member's own toolchain file
  never builds into another toolchain's folder. Builds at once wait for each other on cargo's lock. Two apps with the same binary name in that folder: the first
  claims it, the second is refused until it is renamed.
- Prebuilt libraries (a prototype): with `AIORT_PREBUILT_URL` (an
  `https://` server) and `AIORT_PREBUILT_KEY` (an `ed25519:` public key)
  set, the first build into an empty folder first fetches
  `<url>/<key>/SHA256SUMS`, its signature and `<key>.zip`. The key names
  the toolchain (`rustc -vV`), the host and its C library, the build's
  features, its rustflags, cargo config files and profiles, and every
  registry and git package in `Cargo.lock`. Without both variables
  nothing is fetched. The signature must verify, the zip's sum match, its
  entries stay in the build's folders, and it must unpack to under 1 GB a
  file and 8 GB in all within 5 minutes; else ax says why in one line and
  builds them here. The zip is unpacked apart, then each unit moved in
  under cargo's lock; a unit already here stays. cargo does not check a
  registry or git unit against its sources (it trusts its fingerprint), so
  the signature is what vouches for the files: fetch only from a server
  whose key you trust. Only registry and git packages come prebuilt, and
  none with native build output (`links`, a C library a build script
  builds or finds) nor any built on one: an aio-rs checkout (a path), the
  app and those always compile on the machine. The files are the same
  `desktop-dev` units a local build makes (the release-equal profile), so
  the app behaves the same either way. `ax dev <app> --warm --pack <dir>`
  writes them from a build in a folder of its own (signed with the key
  file named by `AIORT_PREBUILT_SIGN`), with no path of the machine that
  made them; a server of that folder is all it needs.

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
| `--display auto` | the default: a person at a terminal gets their own desktop; a caller with no terminal (an agent, a script) a nested display; with no desktop at all, Xvfb (none installed: the window can't open, said first) |
| `--display isolated` | always a nested display (Xephyr, else Xvfb); refused when neither is installed |
| `--display current` | the caller's display, as is (`AIORT_DISPLAY=current` too) |
| `--display :N` | a display you run yourself |
| `--watch=false`, `--no-watch` | build and run once, rebuild on nothing |
| `--watch=src/ui,assets` | reload, patch and rebuild on changes under those folders of the app only (`[dev] watch` in `aio.toml` says it for every run) |
| `--detach` | the session in the background, output in `logs/dev.out`; `ax stop` ends it |
| `--verbose` | the app's log at `debug` (`AIORT_LOG=debug`; the boot report's `log` row says so) |
| `--title "Notes (QA)"`, `--title=…` | the window's title for this run (`AIORT_TITLE`) |
| `--cdp[=port]` | the window opens the DevTools protocol on loopback (a free port unless given) |
| `--web [--port N] [--expose]` | the web build instead ([below](#the-web-build-while-you-work)) |
| `--android [--device serial] [--native]` | on an emulator or phone: the web build in a dev APK installed once, each edit reloading it (`--native`: the app itself, rebuilt for that CPU on every save) |

Words after the app go to the app (`ax dev notes -- --route /users/7`);
ax's own flags above are taken out first. `--display`, `--fresh`,
`--size`, `--verbose`, `--title` and `--detach` never reach the
app. Args with spaces reach it intact.

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
loopback port (`--port N`: that one) and prints the address. A dev build
turns on aio-rs's `dev` feature in the page (as the desktop window has it). `--expose`
serves it to the network (`0.0.0.0`) over TLS behind a key made for the
run: open the printed `https://<lan-ip>:<port>/?token=…` link on a phone
or another machine (the browser asks to trust aio-rs's own certificate;
the link sets a cookie, so the page's own requests need no key in their
address). A saved stylesheet shows at once in
every open tab. Markup alone (an `rsx!` text, a static attribute, as on
the desktop) goes to every open tab at the first save, nothing built: its
views are built again in place, their state kept (~10 ms). Any other saved
source builds the bundle again and the tabs reload; the cells come back
from the browser's storage; the build starts at the first save, while
a burst of saves settles. The dev build optimizes the libraries
(`opt-level = 1`) and links no debug info (wasm-bindgen drops it anyway):
half the module, so binding it and loading it take half the time. A tab asks the server and is answered as soon
as something changes (no polling interval). A failed build keeps the last
page and shows the errors on it. It needs the `wasm32-unknown-unknown`
target and the wasm-bindgen the lock names (`ax setup` fetches it).

A Rust edit of the page's crate is hot-patched into the open tabs, state
kept (~0.18 s for the template, to the page). rustc compiles the crate
again; ax links its objects into a small side module against the running
module (its statics' addresses, its functions' table slots) and the page
loads it into its own memory and function table. The views and the cell
methods are called through a jump table, so the next render and the next
call run the new code. wasm-bindgen doesn't run. For this the dev build
gives every function a table slot and each JS import a wrapper (~0.04 s
more per build). A patch's new statics take a 16 MB arena in the page.
An edit a patch can't take is a full build and the tabs reload, the
terminal says why: a new JS import (`#[wasm_bindgen]` extern), a function
called as another type, another crate's file, the arena full.

## On a phone while you work

```sh
ax dev notes --android                  # the attached emulator (before a phone), else AIO_AVD booted
ax dev notes --android --device R58N…   # one device of several (or ANDROID_SERIAL)
ax dev notes --android --native         # the app's own library, rebuilt on each save
```

As aio's `dev:android`: a thin dev APK, installed once beside the real
app ("Notes (dev)"), shows the page `ax dev --web` serves, reached over
`adb reverse`; each edit shows there as in a browser tab (Rust
hot-patched, markup and stylesheets live, state kept), no APK built
again. `--native` (or an app without a web
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
| `--client <how>` | `window`, `browser` or `server-only` (no window, served until asked to stop) for this run, over `[app] ui` (aio's `--client`; `electron` is `window`) |
| `--cdp <port>` | the window's DevTools protocol on loopback for this run: an `ax dev` build, or another whose app says `Window { devtools: true }` |
| `--server-url <addr>` | a client's server for this run, over its pairing and `[client] server` ([thin client](../clients/desktop.md#the-thin-client-and-other-machines)) |
| `--connect` | a client opens its connect page first |
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
let env = |k: &str| std::env::var_os(k).map(Into::into); // AIORT_PORT, if set
let l = aio::config::listen(&server, &Ask { expose: true, port: Some("7420".into()), ..Ask::default() }, &env).unwrap();
assert_eq!((l.addr.as_str(), l.exposed, l.tls), ("0.0.0.0:7420", true, true));
assert!(aio::config::listen(&server, &Ask { expose: true, host: Some("::1".into()), ..Ask::default() }, &env).is_err());
# }
```

Everything else that varies per launch is a variable, never a config file
read at run time: `AIORT_DATA` (one folder for all of the app's files),
`AIORT_TOKEN` (a served app's key), `AIORT_PORT` (its port), `AIORT_UPDATE_CHANNEL`, `AIORT_GPU`
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

`ax start` builds in the workspace and runs the result as the release
build runs (no operator door, no DevTools port, its process hardened;
`--cdp` asks for ax dev's tooling), waiting until it is up; `ax install` builds this OS's bundle and installs it as a user
would ([build](README.md#install-and-try-it)). The app's aio-rs comes from
git (or a checkout, `--aio`): [setup](../basics/setup.md#where-an-app-gets-aio-rs-from).

## Common mistakes

- `ax dev notes --fresh` without `--instance`: refused; `--fresh` never
  empties the app's real data.
- Expecting `ax dev` to keep in-memory view state across a restart: a
  hot patch keeps it, a rebuild starts the views again (the cells' state
  is saved).
- A CSS edit that doesn't show: the stylesheet is kept in a `once`
  (made once) instead of `aio::use_css!` in a hole of `rsx!`.
- Native-only code in a view the web build compiles: put it behind
  `#[cfg(not(target_arch = "wasm32"))]` or in a server function.
- Driving `ax dev` from a script or agent with `--display current`: the
  window lands on the person's screen. Leave `auto`.

## Limits

- In the web build only the page's crate is hot-patched: an edit of
  another crate is a build and the page reloads, its cells kept.
  `--android --native` builds and installs again on each save.
- `ax run <link>` builds and starts the app as `ax start` does; it
  installs nothing (aio's `run.sh` installed its build). A person gets
  an app from its signed release (`ax release`). It needs `ax`: no
  script is piped into a shell.

## API

`cargo doc -p aio-rs --no-deps --all-features --open`: `aio::paths::Ask`,
`aio::config::listen`, `aio::use_css!`, `aio::ui::local_file_url`;
`cargo doc -p aio-rs-shell --no-deps --open`: `start`, `run`, `page::save`.

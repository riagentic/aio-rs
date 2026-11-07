# aio-rs next to Tauri 2 and Dioxus 0.7

How building an app feels in aio-rs, Tauri and Dioxus. The same tiny
app in each: a counter, a text list, one stylesheet, and one Rust function the window shows (`label(n)`, for the "Rust logic" edit).
Measured once each on one Linux x86_64 machine (other work running beside
it), 2026-10-31; one run, not a benchmark: read the shape, not the decimals.

- aio-rs: `ax create` (app template), the cell holds count and items
  (saved, with history). aio-rs 0.1.0, which drew its views with Dioxus
  0.7.10, Electron 44. From 0.2.0 aio-rs draws with air, its own renderer,
  and has no Dioxus in its graph: the template's sizes and edit times on
  air are in [air's measurements](design/air.md#measured-scriptsdx-metrics-the-template)
  (a smaller binary, markup in ~3 ms, Rust patched in ~0.5 s). The live
  edit rows of aio-rs were measured again on 0.3.0 (2026-11-07,
  `scripts/dx-metrics --live`); the rest of this page wasn't.
- Tauri 2.12 (CLI 2.12.4): create-tauri-app's vanilla shape: `src/` HTML,
  CSS and JS, state in a `Mutex` behind three `#[tauri::command]`s.
- Dioxus 0.7.10 desktop (wry, WebKitGTK), signals, `asset!()` stylesheet.

Each in its own folder and target dir, cargo's defaults (incremental debug
builds); live edits through each one's own dev command (`ax dev`, `tauri
dev`, `dx serve --hot-patch`), timed from the save to the last change of
the screen (Xvfb, polled ~10/s).

## Numbers

| | aio-rs | Tauri 2 | Dioxus 0.7 |
|---|---|---|---|
| Crates (`cargo tree -e normal,build`, host, unique) | **157** | 262 | 354 |
| Clean debug build (`cargo build`) | **15.9 s** (82 CPU-s) | 34.3 s (194 CPU-s) | 30.3 s (225 CPU-s) |
| One-line rebuild (debug) | **0.6 s** | 3.3–5.6 s | 0.8–0.9 s |
| Live CSS edit, save → visible | **0.02 s** | none: frontend embedded, shows after a Rust rebuild | 0.03 s |
| Live markup edit (rsx / HTML) | **0.004 s** | none (as CSS) | 0.05 s |
| Live Rust logic edit | **0.6 s**, hot-patched, state kept | 3.3 s, rebuild + restart, state lost | 2.5 s, hot-patched, state kept |
| Typo in Rust while running (`c.cuont`) | rustc's error in the terminal and on the window (0.13 s), app keeps running; the fix takes it off (0.6 s) | rustc's error in the terminal; the window closes until it builds | rustc's message lines in the terminal; a toast "the build failed" with no detail |
| Debug target dir | **894 MB** (no dependency debug info) | 3.1 GB | 2.3 GB, + 1.4 GB for `dx serve`'s profile |
| Release build (from empty) | 65 s, bundles and smoke test included | 73 s compile; `tauri build` 298 s (fetches linuxdeploy) | 51 s compile; `dx bundle` +48 s (fetches linuxdeploy) |
| Release binary | 5.0 MB | 4.0 MB | **3.8 MB** |
| Package | `.tar.gz` 2.6 MB, AppImage **3.4 MB** | `.deb` 1.6 MB, AppImage 83 MB | `.deb` **1.4 MB**, AppImage 81 MB |
| Idle window memory, private / PSS | ~81 / ~167 MB (Electron, 7 processes) | 104 / 147 MB (WebKitGTK, 3) | 101 / 151 MB (WebKitGTK, 3) |

- Packages: aio-rs's leave out the window runtime (Electron, 284 MB unpacked),
  fetched and checked once per machine by `ax setup` and shared by every
  aio-rs app. A `.deb` of Tauri or Dioxus leans on the system's WebKitGTK; their
  AppImages carry it (~80 MB each).
- Memory: Electron's 6 processes measured 74 / 160 MB; the app's own
  process can't be read from outside (aio-rs makes it non-dumpable), aio-rs's
  smoke puts it at 7 MB. aio's PSS shrinks while other aio-rs apps share the
  runtime; WebKitGTK's PSS is low because the desktop already maps GTK.
- Builds: aio-rs's template builds dependencies without debug info
  (`debug = "line-tables-only"` for the app only); the other two use
  cargo's default (full). Tauri's rebuild relinks a `staticlib` + `cdylib`
  + binary (its template's crate types).
- `ax dev` builds into one folder per machine and toolchain (`~/.cache/aio-rt/target/<toolchain>`):
  dependencies built for one aio-rs app serve the next; `dx serve` and `tauri
  dev` build into the project.

## The edit loop next to aio's `am dev`

aio 1.0.25-beta's template app (`am create --client=electron`, its
`deno task dev`) beside aio-rs's (`ax create`, `ax dev`, release `ax`),
on one Linux machine under the same load, on Xvfb, both runs alternating.
One Deno script drives both over the DevTools protocol: each start is
timed from the command to the counter and its button on the page, each
edit from the file's write to the page showing it (polled every 5 ms).
Medians of 3 to 10 runs (aio's measured in the first round, aio-rs's
again after each change below):

| | aio `am dev` | aio-rs `ax dev` |
|---|---|---|
| First start on a fresh machine, before `ax setup`'s warm-up has run | **2.6 s** | 24.2 s: Rust's compile time for aio-rs and its dependencies, once per machine |
| First start of an app, aio-rs built (`ax setup`'s warm-up, an earlier app) | 2.6 s | **1.7 s** (the app's own crates compiled, linked) |
| First start while the warm-up still runs | 2.6 s | it waits on cargo's lock, then builds only its own crates: the window as the warm-up ends (20 s for one started 3 s into it) |
| Start, nothing changed | 1.1 s | **0.33 s** |
| Start after an edited session | 0.8 s | **0.35 s** (built behind the session, or by the warm-up its end started) |
| Start, the sources changed while `ax dev` was off (a pull) | **0.8 s** | 1.0 s (the changed crates compiled, linked) |
| A stylesheet line | 0.31 s, a reload | **0.01 s**, state kept |
| Markup (`App.tsx` text, an `rsx!` text) | 0.50 s | **0.008 s**, state kept |
| A view's logic | 0.41 s, the page patched | **0.27 s**, hot-patched, state kept |
| A cell method's logic | 0.84 s, the app restarted | **0.40 s**, hot-patched, state kept |
| A cell's field added | a restart | 2.1 s, rebuilt and restarted, the state back from its log |

aio-rs wins every edit and every start but two: a first start on a
machine where aio-rs was never built, and one after the sources changed
with `ax dev` off. Both compile Rust, which aio, running TypeScript from
source, never does; the first is ~25 s once per machine, so `ax setup` (the
installers' last step) builds aio-rs in the background at once, and
`ax create` builds each new app the same way. Prebuilt libraries
([dev](build/dev.md)) cut it further on a fresh machine.

What closed the other rows:

- Logic (0.51 s and 0.59 s before): a Rust patch compiles from the first
  save instead of after the burst's 100 ms of quiet, and a library edit
  is applied without compiling the launcher binary first; the launcher
  is compiled after, and patched again only if its own code changed
  ([dev](build/dev.md#hot-patching)).
- A start after an edited session (1.1 s before): the session brings the
  next start's build up to date behind it once it is quiet, at low
  priority, and one that ends behind it (Ctrl-C too) starts that build
  in the background ([dev](build/dev.md#the-session)).
- A first start (24 s before, without `ax setup --warm` by hand): `ax
  setup` at a terminal starts the warm-up itself.

On a Mac (Apple silicon, its own screen, one run each): a new app's first
start with aio-rs built 3.2 s, a stylesheet 0.03 s, markup 0.03 s, a
view's logic 0.62 s and a cell method's 0.72 s hot-patched, a cell's
field added 2.7 s; the start after an edited session launches the binary
built behind it (1.1 s: macOS checks a binary it hasn't run before; 0.85 s
after). Windows isn't in the table: its runs here have no desktop session
for the window.

## First run: from nothing to a window

| | aio-rs | Tauri 2 | Dioxus 0.7 |
|---|---|---|---|
| System packages (Linux) | none beyond a C compiler: Electron is fetched (desktop distros have its libraries) | `sudo apt install libwebkit2gtk-4.1-dev libxdo-dev libayatana-appindicator3-dev librsvg2-dev build-essential …` | `sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libxdo-dev …` |
| Steps | install ax (prebuilt, checksummed, or from source) → `ax setup` → `ax create x` → `ax dev x` | Node or `cargo install create-tauri-app` → `npm create tauri-app` (prompts) → `npm install` → `npm run tauri dev` | install dx (`cargo binstall` or its installer) → `dx new x` (prompts) → `dx serve` |
| What the new app has | state saved with history, a UI test, `AGENTS.md`/`CLAUDE.md`, `aio.toml` with every default | window + one command | window + a starter template |
| Docs | `ax agent` (brief for agents), `cargo doc -p aio-rs`, `docs/basics/setup.md`; no website | large site, guides per OS, API docs | site with guides (0.7), API docs |

- On this machine (runtime GTK/WebKit libraries, no `-dev` packages, no
  sudo) aio-rs built and ran as is; Tauri and Dioxus stopped at `pkg-config`
  (`gdk-x11-3.0`, `dbus-1`, `webkit2gtk-4.1`… not found) until their
  development packages were stood in for.
- Dioxus by hand (no `dx new`): `dx bundle` refused twice, one missing
  `Dioxus.toml` key at a time (`identifier`, then `publisher`).
- Tauri: an icon set is required before the first build (`tauri icon`); a
  JS call with a wrong argument name only rejects in the page's console.
- aio-rs: `act!()` returns `Option` (a refusal is logged), so `.is_ok()` on it
  is a plain rustc type error; the error says so.

## Where the others are ahead


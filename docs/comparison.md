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
| First start on a fresh install (`ax setup` at a terminal, then `ax create`, `ax dev`) | 2.6 s | **1.9 s** (0.4.0: aio-rs built by `ax setup` before it returns, the app's own crates by `ax create` behind it) |
| First start of an app, aio-rs built, the app not (`ax create --no-warm`) | 2.6 s | 2.6 s (median of 1.8, 2.6 and 5.0 s: the app's own crates compiled, linked, its window's first profile made) |
| First start with nothing built (no terminal at `ax setup`, `AIORT_NO_WARM=1`) | **2.6 s** | 24.2 s: Rust's compile time for aio-rs and its dependencies, once per machine |
| Start, nothing changed | 1.1 s | **0.33 s** |
| Start after an edited session | 0.8 s | **0.35 s** (built behind the session, or by the warm-up its end started) |
| Start, the sources changed while `ax dev` was off (a pull) | 0.8 s | **0.42 s** (0.4.0: the last build started, the edit patched in as it comes up; 1.0 s before, 1.3 s for a view's logic edit on 2026-11-08) |
| A stylesheet line | 0.31 s, a reload | **0.01 s**, state kept |
| Markup (`App.tsx` text, an `rsx!` text) | 0.50 s | **0.008 s**, state kept |
| A view's logic | 0.41 s, the page patched | **0.27 s**, hot-patched, state kept |
| A cell method's logic | 0.84 s, the app restarted | **0.40 s**, hot-patched, state kept |
| A cell's field added | a restart | 2.1 s, rebuilt and restarted, the state back from its log |

aio-rs wins every edit and every start but a first start on a machine
where aio-rs was never built: it compiles Rust, which aio, running
TypeScript from source, never does (~25 s once per machine). So `ax
setup` (the installers' last step) builds aio-rs before it returns, and
`ax create` builds each new app's own crates in the background.

What 0.4.0 changed, measured on 2026-11-08 the same way (aio-rs's release
`ax`, CARGO_BUILD_JOBS=4 on 12 shared cores, load 6-9; medians of 3 to 5
runs, before and after on the same machine the same hour):

- A start after an edit made with `ax dev` off (a view's logic, the
  template): 1.33 s → 0.42 s. The last fat build starts at once, nothing
  built or linked; the edited crates compile beside its start and the
  patch goes in as its window comes up ([dev](build/dev.md#the-session)).
  Code that ran as the app started ran as built before, as with an edit
  in a session.
- A fresh install's first start: `ax setup` built aio-rs in the
  background, so a first `ax dev` soon after waited for it; it builds it
  before it returns now (34 s here), then `ax create` builds the app's
  crates behind it (1.5 s): the first `ax dev` 1.46-1.89 s (median 1.9 s).
- `ax dev --web` (and the Android dev page, the same page), beside aio's
  page reload:

  | | aio `am dev` | `ax dev --web` before | `ax dev --web` 0.4.0 |
  |---|---|---|---|
  | A stylesheet line | 0.31 s, a reload | never shown (the page's live read was off) | **0.02 s**, state kept |
  | Markup | 0.50 s, a reload | 1.03 s, a build and a reload | **0.008-0.015 s**, state kept |
  | A view's logic | 0.41 s, a reload | 1.13 s, a build and a reload | **0.18 s**, hot-patched, state kept (0.58 s, a build and a reload, before the hot-patch) |
  | A cell method's logic | 0.84 s, a restart | 1.1 s | **0.17 s**, hot-patched, state kept (0.76 s, a build and a reload, before) |

  The page is told of a change as it comes (its question held by the
  server) instead of asking every 250 ms. A Rust edit of the page's crate
  is hot-patched: rustc compiles the crate again (~0.11 s), its objects
  are linked into a side module against the running module's addresses
  and table slots (~0.05 s), and the page loads it into its own memory
  and function table. The views and the cell methods are called through
  a jump table, so the new code runs on the kept state; wasm-bindgen
  doesn't run. Medians of 10 warm edits (the first, cold: 0.36 and 0.25
  s); a cell edit is timed to the patch applied, its next call the new
  code. Each dev build takes ~0.04 s more (a start 0.39 → 0.42 s): every
  function gets a table slot and each JS import a wrapper. An edit a patch
  can't take (a new JS import, a function called as another type, another
  crate's file) is a full build and a reload. Before the hot-patch, the
  follow-up built it from the first
  save (while the burst settles) and optimizes the dev build's libraries
  with no debug info linked: the bound module 4.7 → 2.2 MB, the build
  0.83-0.91 → 0.48-0.57 s (cargo ~0.3 s, wasm-bindgen ~0.2 s), the reload
  ~0.15 s. Medians of 3 (the first, cold, left out: 1.26 s). Under aio's
  0.41 s needed wasm-bindgen out of the loop, hence the hot-patch.
  Profiled: cargo 0.19 s (rustc 0.14 s, 0.06 s of it the link),
  wasm-bindgen 0.22 s, the reload ~0.15 s. wasm-bindgen has no flag that
  skips a pass, and its output can't be reused for a new module even with
  the same imports and exports: it drops ~3 000 of the module's 7 900
  functions (its descriptors and what only they reach), rewrites the
  imports and moves JS values into a table, so the bound module differs in
  every function index. Stripped names or a smaller build (`opt-level =
  "s"`) left it at 0.22-0.29 s.
- A crate of about 74 000 lines (generated: 660 modules, each an
  `#[component]` with a 60-line `rsx!` and plain Rust): a logic edit
  hot-patched in 7.4 s (0.3.0) and 7.0 s (0.4.0), rustc 5.4-6.8 s of it;
  aio, on an aio app of the same shape and size (660 generated modules,
  76 000 lines, 2.7 MB, `am create --client=electron` grown by the same
  script, one session): a view's logic edit 1.6-1.8 s (its bundle 1.0-1.25
  s, then a reload), a cell edit's restart ~1.3 s; no ~3 s. The follow-up: 4.9-6.9 s (median of 4: 6.0 s, the
  machine's load 9-17), rustc 4.4-6.3 s, stub 0.36-0.44 s, link 0.06-0.11
  s. A patch links only the objects it needs (6 of the crate's 256: the
  stub and link took 0.8-4.6 s writing its 140 MB library out whole), and
  a `"{x}"` hole of one name expands to a shared generic call instead of
  a closure each (the expansion 20 → 16.6 MB; air's bench crate: expanded
  -20%, metadata -18%, LLVM IR -18%, its view edit's build 1.5 → 1.1 s;
  the held-key latency unchanged, p50 1.0 / 1.5-1.6 ms). The front end
  stays the floor (`-Z time-passes` of one edit's rebuild, 4.5 s:
  expansion 0.9 s, names 0.33 s, type and borrow checks 0.64 s, the
  metadata with its monomorphization 1.0 s, the incremental cache 0.31
  s; aio-rs's macros optimized: expansion 0.2 s less, lost in the noise
  end to end, so not shipped): rustc's incremental build of one crate reruns its
  whole front end: every macro, name resolution, the crate's metadata.
  The same lines as plain Rust (no `rsx!`) rebuild in 2.6 s: the
  expansion of 660 `rsx!` (16.6 MB from 2.7 MB of source) is the rest.
  No rustc setting on stable moves it (the same rebuild, 3 runs each:
  4.7-5.1 s as built, 4.4-5.1 s with 64 codegen units, 4.6-4.8 s with
  1024, 4.6-4.7 s without debug info, 4.6-6.0 s without the metadata
  file). The same app in page crates is under aio (the same script grown
  into 10 crates of 66 modules on a kit crate each module's label goes
  through, `ax dev` headless (Chromium's own ozone), CDP 5 ms polls,
  2026-11-08):

  | ~74 000 lines | aio `am dev` | `ax dev` before | `ax dev` 0.4.0 |
  |---|---|---|---|
  | one crate, a view's logic | **1.6-1.8 s** | 5.7-6.2 s (median 6.0) | 5.0-6.0 s (median 5.5): unchanged, rustc 4.6-5.4 s |
  | page crates, a page's view logic | 1.6-1.8 s | 1.9 s (0.1's measure); 0.4.0 before this: patched in 0.6-0.9 s, the window kept the old view | **0.47-0.52 s** (median 0.48), rustc ~0.4 s |
  | page crates, the kit every page calls | 1.6-1.8 s | 8-14 s (0.1) | 1.0-1.9 s: rustc 0.03 s, the patch of every caller 0.45-0.55 s stub and 0.2 s link, its 94 000-function table |

  What changed: an edit compiles its own crate alone and is patched in;
  the crates built on it compile after, off the time to the screen,
  patched again only when their code changed. A patch's reach to the
  roots spans the app's libraries (a caller in the crate above, linked
  from the running copy), where it stopped at the edited crate's own (the
  view stayed old: the app's root view, in the crate above, never got the
  patch). An edit dropping a symbol an earlier patch added compiles the
  crates above with it (their running objects may use it). `ax add <app>
  page <name>` makes a page crate, and `ax lint` hints at a crate past
  15 000 lines ([what an edit costs](build/README.md#what-an-edit-costs)).

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


# Changelog

All notable changes to aio-rs are written here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions
follow [Semantic Versioning](https://semver.org/): before 1.0.0 a minor
version may break things.

## [Unreleased]

## [0.4.1] - 2026-11-09

A fix release: `ax upgrade` past a tag with no release files, `ax pin`
in an app with a `[patch]`, no zig fetched by setup, SQLite's security
fix, and the Windows (MSVC) and CI-runner fixes GitHub's CI found.

### Fixed

- `ax upgrade` takes the newest release that has its files: a tag whose
  release is still being built (or was never published) is skipped and
  named, never a 404; and `latest` never installs an older ax than the
  one running.
- `ax pin <version>` in an app with a `[patch]` of aio-rs: the `[patch]`
  stays as it is (it was moved too, so it pointed at its own source and
  cargo refused the move), and `ax pin` says the `[patch]` is what builds.
- `ax setup` (and so the installers and `ax upgrade`) no longer fetches
  zig: the first `ax build --platforms macos` away from a Mac does;
  setup says whether it is here.
- Windows: `ax dev` started again with nothing changed runs the last
  build as it is after a rebuild-and-restart too (the app still running
  held the kept binary, so it wasn't kept, and the next start built the
  app crate again).
- Windows with MSVC: a call made offline just before
  `std::process::exit` is saved (MSVC's C runtime skips `atexit` there;
  the program's TLS callback saves it now).

### Security

- SQLite: rusqlite 0.41 with libsqlite3-sys 0.39, past
  [RUSTSEC-2026-0459](https://rustsec.org/advisories/RUSTSEC-2026-0459)
  (C SQLite's CVE-2026-119375).

### Testing

- The runtime gate (`tests/rt_gate.rs`) is skipped on a CI runner (`CI`
  set), saying so: a shared machine's speed swings several-fold, so its
  one-machine ceilings gate in `./ci.sh` on that machine, for each
  release.
- Tests that depended on the machine: the Windows console probe gets a
  console of its own, the hot-patch stop waits for the grandchild's pid,
  `am`'s caller is read as the code reads it (`USERNAME`), a frozen sync
  link is frozen before the server changes, the pace test's ticker has
  ended before its last change, and the virtual-time timeout test's
  device answers well after virtual time ran out.

## [0.4.0] - 2026-11-08

aio-rs 0.4.0 has what aio had and 0.3 lacked, and is faster where an app
is edited. `ax dev --web` hot-patches the page's Rust with the state
kept (a view's logic on the page in 0.18 s, a cell method in 0.17 s;
aio's reload 0.41 and 0.84 s), and a big app in page crates (`ax add
<app> page`) takes a page's view edit in 0.47-0.52 s (aio's `am dev`
1.6-1.8 s on an app of the same size). New from aio: server rendering,
streaming and hydration (2.5-5.5× faster than aio's `renderToString`),
code splitting and Web Workers in a web build, the JS client for React,
Solid, Svelte and Vue with `ax tsgen`, Redux DevTools, path
subscriptions, render-staleness backpressure, an HTTP fallback
transport, `h()`, `TrackedMemo`, form schemas (JSON Schema too), macOS
apps built on Linux and Windows, an iPhone and iPad thin client, `am`
reaching an aio-rs app, and `ax run`, `ax where`, `ax check`, `ax
watch`, `ax add` and aio's other verbs. A window answers a key in the
frame after it, a draw with aio at the frame's floor with less CPU per
key; a new store opens in 13 ms (was 31). An app on 0.3 moves with the
[migration guide's 0.3 → 0.4
section](docs/migration/README.md#10-03--04).

### Breaking

- **New fields** in public structs: a struct literal without `..` names
  them ([migration](docs/migration/README.md#10-03--04)). aio: `Call`
  (`state`, `request`), `Inputs` (`made`), `http::Viewer` (`linked`,
  `path`), `report::Diagnostic` (`state`), `report::About` (`args`),
  `paths::Ask` (`transport`), `web::storage::Doc` (`paths`). aio-config:
  `Client` (`transport`), `Feedback` (`args`).
- **Changed signatures**
  ([migration](docs/migration/README.md#10-03--04)): `aio::paths::FLAGS`
  is `[&str; 21]`, naming `--transport=` too; the kit's `Form<T>` is
  `Form<T, O = T>`, `O` what its values parse into, and `use_async_rule`
  takes a `Form<T, O>` (a call that names its type parameters adds `O`).
- `aio::update::can_apply` takes an async closure (`can_apply(|| async {
  … })`) and `aio::update::Veto` is gone. A "not now" no longer refuses
  the `apply` call: the call answers, the release stays offered and
  `error` says `the app says not now: …`.
- An async call cut off at the call ceiling answers `EFFECT_TIMEOUT`,
  one whose awaited I/O panicked `EFFECT_ERROR` (both were
  `DISPATCH_ABORTED`); an `#[on_stop]` refusal's problem kind is
  `hook:stop` (was `hook`), its code `DESTROY_ERROR`.
- `aio::http::painted` marks a painted `#main` (`data-aio-painted`) and
  hides the splash.
- **The `updates` cell is a feature**: `aio::update::Updates`,
  `UpdatesCalls` and `share` need aio-rs's `updates`, which aio-rs-shell's
  `updates` (on by default, and turned on by ax for an `[updates] url`)
  turns on. An app with the shell's defaults off that names the cell adds
  `features = ["updates"]` to aio-rs-shell
  ([migration](docs/migration/README.md#10-03--04)). Without it a debug
  build of the template is 20 MB smaller.
- **`ax` answers a pipe in JSON** (aio's `am`), as with `--json`: a
  script that read its text through a pipe adds `--text` (or
  `AIORT_OUTPUT=text`; `AIORT_OUTPUT=json`: JSON at a terminal too).
  `dev`, `lint`, `agent`, `testgen`, `record`, `peer`, `snapshot`,
  `logs`, `help` and `version` stay text; `ax ui` and the repo's scripts
  set `AIORT_OUTPUT=text`.

### Added

- **`ax dev --web` hot-patches Rust** in the running page: an edit of
  the page's crate is compiled, linked into a side module against the
  running module (its statics' addresses, its functions' table slots)
  and loaded into the page's memory and function table; views and cell
  methods run the new code with their state kept, no reload, no
  wasm-bindgen run. A view's logic shows in 0.18 s (a build and a
  reload, 0.58 s, before; aio's reload 0.41 s), a cell method in 0.17 s
  (0.76 s; aio 0.84 s), medians of 10 warm edits. An edit a patch can't
  take (a new JS import, a function called as another type, another
  crate's file) is a build and a reload. The dev build gives every
  function a table slot and each JS import a wrapper (~0.04 s more a
  build).
- `ax dev --web` and the Android dev page take markup edits live: an
  `rsx!` text or static attribute reaches every open tab at the first
  save, nothing built, the views built again in place with their state
  kept (~10 ms; a build and a reload, 1.0 s, before).
- `ax dev` after edits made with it off (a pull, an editor without a
  session): the last fat build starts at once and the edit, compiled
  beside its start, is patched in as its window comes up, state kept:
  0.42 s for the template, 1.33 s before (aio's `am dev` 0.8 s). A
  change no patch takes (a manifest, the lock, `aio.toml`, a type's
  shape) builds as before.
- **`ax add <app> cell|server|page <name>`** (aio's `am add`): a saved
  cell with a method and its test, or a server function with its test,
  written into the app and wired in (its `mod` line, opened or served by
  each launcher in `app/src` and the browser build); or a page crate
  (`pages/<name>`, package `<app>-<name>`) with a view, a member of the
  app's workspace and a dependency of its library, which `ax dev`
  recompiles alone for an edit in it, however big the app grows. A name
  the app has is refused, nothing written.
- **Server rendering and hydration** (aio's `renderToString`,
  `collectHead`, `hydrate`): `Http::painted(App)` paints each navigation
  of a served web build on the server for its viewer (the route asked,
  its cells as that viewer's socket shows them, the head its views ask
  for) with the cells' state as a seed `Remote::open` starts from; the
  page's view then hydrates the painted nodes in place, with no flash or
  redraw, scroll, focus and typed text kept. `aio::ui::paint(App,
  &viewer, seed)` and `aio::ui::Seed` paint for a server of the app's
  own; a page asked again while its cells are unchanged reuses its
  copies. `air::ssr::render_to_string` and `render` write markup from
  template instances, a hole that reads no signal run once: 1.9 µs, 27
  µs and 0.45 ms for a table of 2, 50 and 1000 rows, 2.5-5.5× faster
  than aio's `renderToString` (`crates/air/examples/ssr_speed.rs`).
  `aio::route::StartAt` starts a server render's router there;
  `air::testing::TestPage::markup` is the settled page as the server
  writes it.
- **Streamed rendering** (aio's `renderToStream`, out of order): a
  painted page whose `Suspense` waits for a `seeded_resource` is sent at
  once with the fallback, then each one's children in place as they load
  (up to 5 s) with the values they loaded, swapped in before the view
  runs, so its first frame starts from them. No inline script
  (`<template data-aio-swap>` and a JSON block); the paint runs off the
  async threads, its scripts nonced before the first part.
  `air::ssr::stream`, `Streaming`, `SWAP_JS`, `aio::page::SWAP`,
  `aio::ui::seeded_resource`.
- **Code splitting of a web build** (aio's `lazy`): `aio::lazy!(view)`
  is a `Load` for `Defer`. `ax build --web` moves the code only lazy
  views reach into chunks beside the bundle (one per view, one for what
  two share) and says their sizes; the page fetches a view's chunk the
  first time it shows. A page with a heavy lazy view: first download 762
  → 543 KB gzipped (607 → 428 KB brotli); aio's `lazy` defers evaluating
  a module of its one bundle. On the desktop, Android and in a dev build
  the view is in the one binary
  ([islands](docs/ui/islands.md#lazy-views-lazy)).
- **Work off the page's thread** (aio's browser workers and worker
  cells): `aio::effect::off_thread(key, f, input)` runs a plain `fn` on
  a worker thread natively and in a Web Worker in a web build (the app's
  own module, a pool of a core less than the device has);
  `cancel_blocking(key)` stops it, in a page at once; a panic is a
  refusal with its message
  ([effects](docs/state/effects.md#heavy-work-retries-and-timeouts)).
- **The JS client for other view libraries** (aio's `aio/state-core`): a
  page drawn by React, Solid, Svelte, Vue or plain DOM holds a copy of
  an app's cells over wire 2 (every cell on one socket), follows their
  patches (a selector of an untouched part keeps its value), calls
  `#[client]` methods (predicted, by the call or by method name with
  `cell.predict`, a refusal rolled back with its code) and keeps calls
  made offline across reloads, shared by twin tabs and sent once. Each
  patch is checked by a digest JavaScript reproduces (`jsdigest`; a
  mismatch resyncs; `aio::patch::js_digest`). No dependency. Every
  served app hands it out at `/__aio/client.js`; the package
  `aio-rs-client` (not on npm yet) adds `/react` (`useCell`, `useCall`,
  `useStatus`), `/solid` and `/vue`, its stores keep Svelte's contract.
  In the desktop window `aio::kit::use_js_client(cells)` carries it over
  the window's own wire; an `Island::react` reads the page's own cells
  ([JS client](docs/clients/js.md)).
- `ax tsgen <app> [--out file]`: TypeScript types of an app's shared
  cells from its source (serde's `rename_all`, `rename`, `skip`,
  `flatten`, tagged and untagged enums; `#[server]` fields left out) and
  each `#[client]` method's arguments and answer, as a typed `CellRef`
  for the JS client.
- **Redux DevTools** in a dev build (`aio::kit::ReduxDevTools`, beside
  the app in `aio_shell`'s dev window and a dev web page): each
  `client!` cell an instance, each commit an action (a view's call with
  its arguments) with the state after it; a jump travels
  (`TimeAsk::Goto`), the dispatcher calls a method, pause, commit,
  revert and reset work. aio's bridge took no jump.
- `aio::kit::webview(id, method, args)`: every method of Electron's
  `<webview>` by name (`aio::kit::WEBVIEW_METHODS`), its value as JSON,
  `capturePage` as a PNG data URL, `printToPDF` in base64.
- `[feedback] args` (off by default): a report's timeline carries each
  call's arguments (aio's timeline payloads), a `#[redact]` method's as
  `[redacted]`, keys masked, 1 KB each.
- A web page's Rust panic is said on its console with its place (`[aio]
  panicked at src/app.rs:12:5: …`), so it reaches the server's log
  (`[server] page_log`) and a dev page's overlay.
- **Path subscriptions** (aio's `subs`, finer):
  `Remote::connect_paths(link, user, paths)` and
  `Remote::subscribe(paths)` hold only those dot paths of a cell; the
  server sends those parts and only their patches, calls predicted in
  the parts held, kept across reconnects and in a saved copy. The wire
  stays compatible both ways (`can` in the first state). aio pages'
  `subs {subs, exact: true}` (feature `compat`) narrows to the paths
  themselves.
- **Render-staleness backpressure** (aio's): a copy whose views render
  it (`use_live`, `Remote::rendered()`) tells its server each second how
  far its screen lags (`seen {ms}`; an aio page's `vitals-ping {ms}`
  counts the same); past 100 ms its patches go 2x apart, past 300 ms 4x,
  three reports under 100 ms step back. A price changed each 3 ms beside
  a 5000-row catalog: a frozen screen gets 20 patches (2.8 KB) where it
  got 78 (10.7 KB), and the process spends 256 ms of CPU against 405 ms.
- `[client] transport = "auto" | "ws" | "poll"`, `--transport=`,
  `aio::sync::set_transport` (aio's `transport`): a copy's links over
  plain HTTP requests (`/__aio/poll`, long polls carrying mux's frames,
  resends applied once) when a proxy or CDN drops the WebSocket upgrade;
  `auto` falls back on its own.
- `Cx::made()`: when a call was made on its device (bounded by the
  server's clock, kept for replay), for last-writer-wins by when an edit
  was made (aio's HLC `lww`).
- **The call gate sees the state and the request** (aio's
  `beforeReduce(action, state)`): `call.state::<T>()`, and for a network
  call `call.request` (`aio::Request`: the client's address and its
  link's headers). Neither is recorded; replay never asks the gate
  ([lifecycle](docs/state/lifecycle.md#one-gate-for-every-call)).
- `aio::visible_by_default(f)` (aio's `cellDefaults.visible`): one
  filter for every cell without its own `#[cell(visible = f)]`; a field
  it leaves out reaches the client as its default, one that panics shows
  the default state
  ([visibility](docs/state/visibility.md#one-filter-for-every-cell)).
- aio's `where` objects over `aio::db::Db`: `Select::from(table)` with
  `Where` (`eq`, `ne`, `lt`, `lte`, `gt`, `gte`, `like`, `is_in`, `and`,
  `or`), `order_by`, `limit`, `offset`, or aio's JSON object
  (`Where::object`); `db.select(&q)`
  ([SQL](docs/persistence/sql.md#where-objects)).
- `aio::kit::use_user()` everywhere (aio's `useUser`): in a page it asks
  the server, in a desktop or Android window it is whom `SignIn` signed
  in; `aio::kit::set_user(None)` signs a window out.
  `Peer::describe_all()` (aio's app-wide `describe()`): every cell whose
  door is open.
- air: `h(tag)` builds a view without `rsx!` (aio's `h()`): `.attr`,
  `.attr_with`, `.on`, `.child`, `.child_with`, `.children`, `.view()`,
  the same templates and holes `rsx!` makes; SVG inside `h("svg")`;
  `comp(Component, props)` places a component.
- air: `TrackedMemo` (aio's `trackedMemo`): values made from state, by
  key, one cache per app instance shared by every component; a hit
  subscribes the reader to what making it read, `.max(n)` keeps the `n`
  used last.
- `use_optimistic(base, apply)` in `aio::ui::prelude` (aio's
  `useOptimistic`): the base with each `add`ed action applied, dropped
  when the base changes; `clear()` drops them on a refusal that changed
  nothing, which aio's hook can't tell; `pending()`.
- kit: `Schema` and `use_form_schema` (aio's `useForm` with a Standard
  Schema): each field's rule declared once (`text()`, `rule::<T>()`,
  `.trim()`, `.required`, `.min_len`, `.max_len`, `.email`,
  `.parse::<N>()`, `.range`, `.check`, `.map`, `.then`, `optional(..)`),
  the values parsed into the app's type, each problem on its field,
  cross-field ones with `p.problem(field, msg)`, checks that wait
  (`.wait(field, debounce, check)`; aio refuses an async schema).
  `Form::parsed()`, `Form::submit_parsed()`.
- `Schema::json`: a form's schema read from JSON Schema as Zod
  (`z.toJSONSchema`), Valibot and ArkType export it, so one schema
  serves a TypeScript client and the Rust form: types, lengths,
  `pattern` (ECMAScript, lookahead too), `format`, bounds, `multipleOf`,
  `enum`, `const`, arrays, `required`, nullable fields, ajv-errors'
  `errorMessage`; a keyword it can't check is an error, never a rule
  skipped
  ([forms](docs/ui/forms.md#one-schema-with-a-typescript-client-json-schema)).
- kit: `UiStyles { hue }` (aio's `<UiStyles/>`): the kit's whole sheet
  near the root, light or dark as `scheme` says.
- Motions read the element's box (aio's `TransitionFn` got the element):
  `--aio-w` and `--aio-h`, measured before the first frame, so
  `Motion::of("open", |t| format!("height:calc(var(--aio-h) * {t})"))`
  opens to its own height. `Timing` as `options` on `Transition` and
  `TransitionGroup` (aio's `options`): one duration, delay and easing
  over both motions, a `const` shared by many; `Motion::timed`.
- `#[derive(aio::cli::Options)]`: a CLI's flags as a typed struct: each
  field's type is its flag's kind and value type (`bool`, `Option<T>`,
  `Vec<T>`, `T` required unless `#[cli(default = v)]`, any `FromStr`),
  `#[cli(positional)]`, `#[cli(rest)]`, `#[cli(command)]`, doc comments
  as help; a default of another type is a compile error.
  `Flag::typed::<T>()` refuses a value that doesn't parse (`--port
  expects a u16, got "70000"`).
- `TestCells::random_calls(&h, n)` (aio's `t.randomActions(n)`): a fresh
  seed each run, printed and named by a failure, so `fuzz_methods`
  replays it. `TestCells::real_time()` and `settle_for(ms)` (aio's
  `t.settle(ms)`): effects' timers on the wall clock.
  `TestCells::last_asked()` (aio's `t.getEffects()`): what the last call
  asked for as values (`aio::testing::AskedFor`). `Cassette::lenient()`:
  arguments that don't serialize kept as text and replayed in order
  (aio's `String(args)`).
- `ax check <app> [--release]` (aio's `am check`, `deno task check` and
  `test`): `cargo fmt --check`, `ax lint`, the library checked for
  `wasm32-unknown-unknown` where the app has a web build, `ax test`,
  stopping at the first that fails; `--release` adds `ax build --sizes`
  and `--smoke`.
- `ax lint`: a `perf` warning for blocking std I/O in async code (aio's
  sync-I/O hint) naming the async one or `aio::effect::blocking`; a
  `perf` hint at a crate of more than 15 000 lines (keep its views in
  page crates); a `css` warning for a class two stylesheets define with
  one property set two ways (aio's `aiol`); aio's `[fixable]` marks
  (`[fixable: --safe-fix]`, `[fixable: --fix]`, `fixable` in `--json`).
- `tests/rt_gate.rs`, the runtime's benchmark gate (aio's D12): a cell's
  start, a store's open, a call and a saved call fail at a p10 over
  twice what they measured (Linux x86_64, debug and release), run by
  `./ci.sh` in the release build; a new store's disk syncs are counted
  (at most 4 to open and close, 2 to reopen). A test holds every log
  within `keep × file_mb` at every write (aio's `logBudget`; `per_run`
  with `keep = 1` is `--no-backup-logs`).
- `report::Diagnostic::state` (aio's `onError` `stateSnapshot`): a
  failing call's, effect's or hook's cell state, screened as a problem
  report's; `Diagnostic::error_code()` gives aio's code. `aio::Code`
  gains aio's `EFFECT_ERROR`, `EFFECT_TIMEOUT`, `HOOK_ERROR` and
  `DESTROY_ERROR`.
- `ax call --as-server` (aio's `am dispatch --as-server`): a call as the
  app's own server code, `By::System`, on a debug build's door
  (`aio::sync::Live::as_server`); `ax call <app> --fn <name>` (aio's
  plain actions): a server function over any cell's door, `k=v` words
  its one object, `--body` too (`Live::run`).
- `ax kill --stale` (aio's): ends each app no verb reaches, one running
  on a data folder deleted or made again, found by the OS's open files.
  `ax report <app> --new [--title t]` (aio's `am report`): a report
  filed now, by the running app or from a stopped one's log. `ax watch
  <app> [dir…]` (aio's `am watch`): the app built and restarted after
  each burst of saves, a failed build said and the running app kept. `ax
  build --force`: the app's own crates compiled anew.
- `ax where <app> [item]` (aio's `am where`): where code runs (`server`,
  `both`, `window/page`, `window`, `wasm-only`, `native-only`,
  `tests-only`, `never`), from the crate graph and each `cfg` on the
  way, with the crates, modules and cfgs that put it there
  ([architecture](docs/basics/architecture.md#ask-ax-where)).
- `ax pin`'s preflight (aio's): a move to an aio-rs that removed what
  the app uses is refused, each use named `file:line: item` from both
  releases' API listings; `--force` moves all the same.
- **`am` reaches an aio-rs app** (feature `compat`): its lock record in
  aio's lock folder (`am instances` lists it) and aio's local socket,
  NDJSON lines of aio's envelope (`am state`, `am dispatch` through the
  app's doors).
- **`aio::compat::AioPeer`: an aio app on this machine, called as this
  app, proven** (aio's `peer()` over its local socket, a named pipe on
  Windows): `find(app)`, `call(cell, method, args)` (waits as long as the
  aio app takes, as aio's `ackTimeoutMs: 0`), `link()`. Its first use
  files the app's aio lock record (without a socket when it serves no
  cell) and the per-run `control.key` so aio can check its `x-aio-peer`
  proof. `AioLink::call_for(.., wait)` bounds a call's wait, or not.
- aio's variables: `AIORT_BUILD_VERSION` (and aio's `AIO_BUILD_VERSION`)
  for `--app-version`, `AIORT_SOCKET_TIMEOUT_MS`
  (`AIO_SOCKET_TIMEOUT_MS`), `ax test --video-scheme light|dark`
  (`AIO_TEST_VIDEO_SCHEME`, `AIO_VIDEO_SCHEME`).
- `ax run <link> [app]`: an app straight from its git repository (a git
  URL, `github:user/repo`, `user/repo` or a folder), cloned into ax's
  cache, one commit checked out (`--ref`), then started as `ax start`
  does (`--dev`: `ax dev`).
- `ax build --platforms macos,macos-arm64` on Linux and Windows: zig
  0.15.2 links the app (no Apple SDK), the arm64 binary signed ad hoc by
  its linker, the `.app` zipped with its modes and links; it opens on
  Apple silicon as it is. A `.dmg` still needs a Mac.
- `ax build <app> --ios` (kind `ios`, aio's `ios-client`): the Xcode
  project of an iPhone and iPad thin client in `dist/<id>-ios/`, a
  WKWebView on the app's server; Xcode builds and signs it. The server's
  pairing page (`[server] pairing`): a browser that opens a keyed server
  with no key pairs with a 6-digit code (`ax pair <app> --plain`), and
  installs to the Home Screen (manifest, icon, an offline shell).
- `ax release <app> --workflow --pages` (aio's `ship github`): the
  workflow deploys the channel to GitHub Pages too; `--web` puts the
  app's web build at the site's root.
- `irm <url>/install.ps1 | iex`: the Windows installer runs as aio's one
  line; a failure is said and leaves the window open. Offline install
  from a folder is documented (`AIO_DOWNLOAD=<folder>`).
- `air::ASKED` (a page event the page waits on) and `air::ANSWERED` (the
  frame that ends its answer).
- `scripts/dx-metrics` gates `dev_changed_s` (a start after an edit made
  with `ax dev` off) and `live_back_s` (an `rsx!` edit undone).

### Changed

- **A web build carries only what its app uses**: Web Workers'
  (`off_thread`) code and the lazy chunks' loader only with an app that
  calls them (the worker reaches the module through one small export);
  server rendering's page only in a server's build; the seeded values a
  streamed page carried read only by an app with a `seeded_resource`;
  one sort shared by every sort of aio and air (each element type's
  own was ~3 KB). The template's site: 496 → 431 KB.
- air's hooks, effects, `batch`, `untrack` and probes keep their
  type-free part in one copy, not one per closure: a debug build of the
  template is 9 MB smaller.
- **`ax dev` compiles an edited library crate alone** and patches it in;
  the crates built on it compile after, off the time to the screen, and
  are patched again only when their code changed. A ~74 000-line app in
  10 page crates and a shared kit: a page's view edit on the window in
  0.47-0.52 s (1.9 s before; aio 1.6-1.8 s on an app of the same size),
  an edit in the kit every page calls 1.0-1.9 s (8-14 s). The same app
  in one crate stays ~5.5 s (rustc's front end).
- A hot-patch links only the objects it needs (those whose code differs
  from the running binary's, their callers up to the roots, any defining
  a symbol the binary's linker dropped) and reaches the rest in the
  running binary: a 74 000-line crate's patch links 6 of its 256
  objects, stub and link ~0.4 s where they took 0.8-4.6 s.
- `rsx!`: a `"{x}"` text or attribute hole of one captured name expands
  to a shared generic call (`text_of`, `attr_of`): a 74 000-line crate's
  expansion 20 → 16.6 MB, its rebuild ~6.5 → ~4.8 s; air's bench crate a
  view edit's build 1.5 → 1.1 s.
- `ax dev --web`'s build optimizes the libraries (`opt-level = 1`) with
  no debug info (the module 4.7 → 2.2 MB), builds a Rust edit from its
  first save, and its page is told of a change as it comes (a held
  request) instead of asking every 250 ms.
- **A window answers a keydown before the page draws again**: the page
  waits for the key's edits (at most 50 ms) and applies them in its key
  handler, so the frame planned when the key came shows it, as in aio;
  the shell numbers the app's frames and the preload applies them in
  order. Over 10 sessions a side, every key pooled: a draw with aio on
  every key at the frame's floor, with less CPU per key on every key
  ([key to paint](docs/aio-vs-aio-rs.md#key-to-paint)).
- A new store opens in 13 ms, not 31 (p10, release; 13 to 4 disk syncs):
  made whole under a temp name and hard-linked in only if the path is
  still free (a crash leaves a temp file the next boot sweeps; made in
  place where hard links fail). The good copy (`VACUUM INTO`) is flushed
  once before its rename; an open no longer rewrites `user_version` that
  holds the current schema.
- `ax setup` at a terminal (the installers' last step) builds aio-rs for
  `ax dev` before it returns, cargo's progress on the terminal, instead
  of in the background: a first `ax dev` never races the warm-up.
  `AIORT_NO_WARM=1` and no terminal leave it out.
- `aio::update::can_apply` is awaited off the cell before every install,
  as aio's `canApply`: `apply`, `apply_anyway`, `start_fresh`, a
  server's unattended install and a staged release's install at the next
  start (a "not now" keeps it staged); a start that installs with none
  set says so. `aio::update::veto()` asks it.
- `ax release` refuses a version already laid out with other bytes,
  naming the fix (a commit raises the build number): an install that has
  the version would never be offered that build. The same bytes again go
  over it.
- A release web build's wasm has no name section and no zero data
  segments (2033 → 1717 KB raw, 753 → 713 KB gzipped on one page); the
  service worker registers once the first view is drawn.
- A client-server app's service worker asks the server for each
  navigation first (it may be painted); a standalone app's answers from
  its cache at once. A served shell is painted only for a navigation; a
  script's fetch gets the shell as built.
- `routes!` fails the build when a variant's fields are not its
  pattern's params by name (`User { idd: u32 } = "/users/:id"`), as
  aio's typed `useRoute(pattern)`.
- The dev overlay: rows newest first, a `clear` button, a warning level
  (an amber badge for warnings only), the live accessibility and
  contrast findings as warnings while they hold, on the desktop and in a
  web page. `__aioDiag(title, detail, show, level)`.
- `air::testing::TestPage` applies a frame without quadratic steps.

### Fixed

- A debug build (`cargo run`, `cargo test`) opens a ported app's `[home]
  dev` again, never its real data: 0.3.0 counted only `ax dev`'s `dev`
  feature.
- An aio app's journal is found where aio writes it: `<dbPath>.journal`
  for a store given by `dbPath` (`[data] store`), `<data>/journal`
  beside `state.db`. 0.3.0 read only `<data>/journal`, so the calls a
  crashed aio app left were lost without a word.
- A journal call on a cell that saves nothing (aio's `unstored`) is
  skipped, whichever cell it is on, as aio skips it: it no longer
  refuses the boot as another cell's call.
- A journal call this build has no method for refuses the boot before
  anything is written: a write first made aio drop that journal at its
  next start, so the refusal's advice (start the aio app once) lost it.
- A dev web build (`ax dev --web`, `ax build --web --dev`) turns on
  aio-rs's `dev` feature as the native one does: a saved stylesheet
  never reached the page (it does now, within ~0.25 s), and the page
  audits never ran.
- `ax dev`: an `rsx!` markup edit undone (saved back as the running
  build has it) never reached the window; the window shows the markup as
  built.
- `ax dev`: a patch reads the libraries it compiled from its own copy,
  so a refresh of the next start's build stopped by a save can't leave
  it without one (a rebuild and a restart before).
- `Defer` with `load` panicked in a web build (`Instant` has no clock in
  a browser); it runs on the page's clock.
- `ax dev --android`: Ctrl-C (or the terminal closing) removes ax's
  `adb reverse` from the device again; it stayed until the next start.

### Security

- `install.sh` runs only once all of it has come: a download cut short
  runs nothing.
- `ax run` shows the link, the commit and the apps and asks before any
  of the repository's code runs (`--yes`; a commit let run once is not
  asked again).
- `am`'s socket into an aio-rs app (feature `compat`) answers `am state`
  and `am dispatch` on a debug build only, behind the `X-Aio-Control`
  key in `<home>/data/control.key`; a release answers `/__aio/health`
  alone. `ax call --as-server` works on a debug build's door only.
- `aio::Request` leaves a link's `authorization` and `cookie` headers
  out. `Where` values are always parameters; table and column names must
  be plain identifiers, else refused.
- The zig that links a macOS app away from a Mac is fetched SHA-256
  pinned.

## [0.3.0] - 2026-11-07

aio-rs 0.3.0 is the dev loop, parity with aio and speed. `ax dev` from a
save to the window on the template app (one Linux machine,
`scripts/dx-metrics --live`): a stylesheet in 0.02 s, markup in 0.004 s,
a Rust edit hot-patched in ~0.3 s with the state kept (aio's `am dev`
0.41 and 0.84 s on the same machine), a changed type rebuilt and
restarted in 2.1 s, a start with nothing changed in 0.5 s, the start
after a session in 0.35 s, an app's first start after `ax setup` in
1.7 s (was 24 s). Much of aio that 0.2 lacked is here: the thin client's
connect page on desktop and Android, single sign-on, the dev time-travel
panel, `ax record`, the todo and canvas templates, `ax release
--min-from`, AppImage delta updates, a themed window frame, aio's error
codes and secret tiers, and aio's `aio.toml` keys and launch flags. Big
state is fast: `aio::Big<T>` shares a big cell field between states until
it is written (a held key on the wallet port 14.9 → 11.8 ms p50, aio
59.7), and key to paint (End over 1000 rows) is 13.5 / 19.6 ms, aio's
13.5 / 20.4. An app on 0.2 moves with the [migration guide's 0.2 → 0.3
section](docs/migration/README.md#9-02--03).

### Breaking

- **New fields** in public structs: a struct literal without `..` names
  them ([migration](docs/migration/README.md#new-fields-and-variants)).
  aio: `update::Release` (`min_from`, `commit`), `update::Offer`
  (`reason`, `migrates`), `update::Updates` (`progress_of`),
  `vitals::Loop` (`drained`), `health::Metrics` (`broadcast`),
  `health::MemoryReport` (`trend`), `report::Diagnostic` (`cell`, `call`,
  `numbers`), `db::Opts` (`readonly`), `sync::Options` (`app`,
  `page_log`), `auth::Session` (`extra`), `auth::client::Me` (`oidc`),
  `paths::Ask` (`cdp`, `channel`, `client`, `connect`, `height`,
  `server_url`, `strict_pin`, `width`), `kit::Motion` (`tick`),
  `kit::Opt` (`view`), `kit::CheckboxProps`, `SwitchProps`,
  `RadioGroupProps` (`bind`). aio-shell: `Window` (`chrome`, `devtools`),
  `guest::Child` (`permissions`, `unsandboxed`), `guest::Web`
  (`unsandboxed_windows`). aio-config: `auth::Policy` (`totp`, `cookie`),
  `Config` (`dev`), `App` (`flags`, `profiles`), `Accounts` (`totp`,
  `cookie`), `Build` (`out`), `Client` (`strict_pin`), `Data`
  (`only_persist`, `snapshot_every`), `History` (`travel`), `Home`
  (`dir`), `Log` (`dir`), `Server` (`page_log`), `Target` (`platforms`,
  `title`, `ui`), `Updates` (`sign_commit`).
- **New enum variants**: a `match` without `_` names them
  ([migration](docs/migration/README.md#new-fields-and-variants)):
  `aio::update::Step::Runtime`, `aio::merge::Merge::Keys` and
  `SetRemove`, `aio::kit::GuestEvent::PreloadRefused`, `aio::kit::Go::Css`,
  `Zoom` and `DevTools`, `aio::kit::Placement::Left` and `Right`.
- **Changed signatures**
  ([migration](docs/migration/README.md#changed-signatures)):
  `aio::discovery::probe_lan(port, wait)` takes the wait;
  `aio::config::listen(s, ask, env)` takes the environment it reads
  (`AIORT_PORT`); `aio_shell::runtime::prefetch(version, sha, step)` takes
  a progress callback (`&|_| {}`); `aio_shell::switches::cdp(typed, env,
  debug, devtools)` takes the launch's `--cdp` and the app's `Window {
  devtools }`; `aio::cli::watch` needs `T: Send + Sync`;
  `aio::paths::FLAGS` is `[&str; 20]`, naming `--channel`, `--width`,
  `--height`, `--cdp`, `--server-url`, `--connect`, `--strict-pin` and
  `--client` too.
- `aio::Code` is no longer `Copy` (`Code::Other(name)` keeps a code this
  build doesn't know) and `Code::as_str` takes `&self`: a `code` used
  twice is `code.clone()` or matched by reference
  ([migration](docs/migration/README.md#changed-signatures)).
- `air::AttrValue<'a>` and `air::IntoAttr<'a>` take a lifetime, so an
  attribute's value may borrow. An app's own `impl IntoAttr for T` is now
  `impl IntoAttr<'_> for T` returning `AttrValue<'_>`; `Attribute::new`,
  `live`, `derive`, `value` and a component's attributes take
  `IntoAttr<'static>` / `AttrValue<'static>`
  ([migration](docs/migration/README.md#changed-signatures)).
- `effect` returns its `Effect`: a closure that must return `()` writes
  `_ = effect(..)` ([migration](docs/migration/README.md#changed-signatures)).
- `Field { hint }`, `Breadcrumb { separator }` and `EmptyState { icon }`
  are view props (`Option<View>`; text in `rsx!` still works)
  ([migration](docs/migration/README.md#changed-signatures)).
- `Hidden<T>` deserializes for a `T: DeserializeOwned` (it reads the
  withheld marker first), so a `Hidden<&str>` no longer deserializes
  ([migration](docs/migration/README.md#changed-signatures)).
- `aio::cli::Args` reads each flag as the kind its spec gives it: a name
  the spec hasn't, or a read of another kind, panics naming it (was
  `None`) ([migration](docs/migration/README.md#behavior)).
- `aio::health::on_memory` hears every sample while memory is high or
  critical, not only each change of level
  ([migration](docs/migration/README.md#behavior)).
- `[updates] every`: the `dev` channel looks at least every minute and
  `test` every 5 minutes, unless `every_by_channel` names them (as aio)
  ([migration](docs/migration/README.md#behavior)).
- With accounts on, `Http::users` are service keys beside them: a
  `Bearer` that is no session is asked of them on sockets, routes and
  files (aio's `users` next to `auth: true`); before, they were ignored
  ([migration](docs/migration/README.md#behavior)).
- **`ax dev` builds (and `ax test`'s) run the app as its release build
  does**: no debug assertions, no overflow checks, so every
  `cfg!(debug_assertions)` reads as in release. What `ax dev` adds is tooling behind the `dev` features
  (aio, aio-shell, and a new air `dev`), inert until it is used. That
  tooling (the operator door `ax call`, `ax ui` and `ax timetravel` use,
  `AIORT_CDP`, `AIORT_WINDOW_SIZE`, the page's dev mode) is no longer in
  a plain debug build (`cargo run`)
  ([migration](docs/migration/README.md#builds-and-ax)).
- `ax start` builds the app as the release build runs: none of `ax
  dev`'s tooling (no operator door, no DevTools port, the process
  hardened); `--cdp` asks for that tooling
  ([migration](docs/migration/README.md#builds-and-ax)).
- `ax dev`, `ax test` and `ax lint` build for this machine without
  `--target`, in one shared build folder per machine and toolchain: the
  dev binary is `<target>/desktop-dev/<app>`, where it was
  `<target>/<triple>/desktop-dev/<app>`
  ([migration](docs/migration/README.md#builds-and-ax)).
- The connect page's address with no code (and a LAN server that needs
  no code, `aio::connect::open_at`) connects with no key: the app's own
  key never goes to a server nothing verified. A keyed server is paired
  with (a code), or named with its key (`--server-url` with
  `AIORT_TOKEN`, `[client] server`).
- A dev build is now any build with aio's `dev` feature too (`ax dev`'s
  and `ax test`'s, which have no debug assertions): there a log that no
  longer replays refuses the boot (quarantine only in a release), a
  method's panic shows the state it ran on, slow field compares are
  said, `[perf] method` defaults to 16 ms, and a problem report says
  `"build": "dev"` (`debug` and `release` as before).

### Added

- `ax dev` shows a splash window while the first build runs (150 ms after
  it starts): cargo's own count of what it builds and the crates it is
  on, a failed build's errors until a fix is saved, an app that didn't
  start, then the app's window in its place. It is ax's own window on the
  shared runtime, never part of the app; a start with nothing to compile
  shows none; `AIORT_NO_SPLASH=1` leaves it out.
- `ax dev <app> --warm` builds only what `ax dev` starts from; `ax setup
  --warm` does it once for the machine on a scratch app; `ax create` at a
  terminal warms the new app in the background, so its first `ax dev` is
  a start (`--no-warm` or `AIORT_NO_WARM=1`: not).
- Prebuilt libraries for the first build (a prototype): with
  `AIORT_PREBUILT_URL` (an `https://` server) and `AIORT_PREBUILT_KEY`, the
  registry and git libraries of an empty build folder are fetched, checked
  against a signed `SHA256SUMS` and moved in; any failure builds them
  here. `ax dev <app> --warm --pack <dir>` writes them.
- `AIORT_TARGET_DIR`: ax's build folder, winning over `CARGO_TARGET_DIR`.
- `ax prune` names ax 0.2's build folder (unused now) and removes it with
  `--yes`.
- `scripts/parity`: proof that `ax dev`'s build, `ax test` and a release
  build run the app the same; `scripts/dx-metrics --live` also times a
  temporary-copy save, a compile error, its fix, a changed type and a
  start with nothing changed.
- An `ax dev` window's overlay shows the app's own problems and warnings
  from the diagnostic bus (`app: <kind>`, with its tip) beside the page's
  errors.
- Every dev build's window has the time-travel panel on Ctrl+. (aio's
  in every dev app): each `client!` cell's history, Undo, Redo, Pause,
  with no code in the app (`aio::kit::DevTimeTravel`); an app mounting
  its own `TimeTravelPanel` keeps only its own. Proven in a real
  headless Electron window: Chromium's own Ctrl+. shows it, Undo steps
  the cell back.
- A dev build audits the running page as `TestUi` does (aio's dev mode):
  aio's accessibility rules and the WCAG AA contrast check, a second after
  the page changes, each finding said once in the log.
- A dev page warns, once each, of an `#id` its stylesheets style that no
  element has, naming `#main` where the app is mounted (aio's selector
  audit).
- A dev build names a cell field slow to compare: one whose comparison
  after a call takes 0.5 ms or more (`aio::big::SLOW_COMPARE`) is said
  once, ``[hint] field `rows` of cell `ledger` took 2.1 ms to compare:
  make it `aio::Big<…>` ``. Not in a browser page, which has no such
  clock.
- A method that panics says, in a debug build, the state it ran on with
  its log line (aio's error box): as saved, tokens masked, at most 2000
  characters; a cell with `#[redact]` methods keeps it to itself.
- **`aio::Big<T>`: a big cell field shared between states until written**
  (aio's Immer structural sharing). A call's copy of the state shares it,
  and two states that share it are equal without a look, so a step that
  didn't write it costs no copy and no comparison: not in the held run's
  write, the views' changed-field mask, nor an app's own `==`. It reads
  and writes as its value, is saved and sent as it, and may be a
  `#[aio_cell]` field. On the wallet port (1000 accounts) a step's
  comparisons went from 4-9 ms to 0.03 ms, a held key's p50 from
  14.9 to 11.8 ms (aio 59.7 in the same runs), its CPU per key from 19.7
  to 11.9 ms
  ([performance: share big fields](docs/performance.md#7-share-big-fields-aiobig)).
- `ax build --quick`: release builds without LTO, in 16 codegen units,
  for trying, never shipping: the template's `ax build` after a one-line
  change 2.2 s, from 27 s (its binary 7.4 MB, from 5.4). Every target's
  cargo takes the same profile overrides; `--sizes` and a Mac over ssh
  refuse it.
- `ax create --template todo|canvas` (aio's): a saved todo list (add,
  tick, remove, a window-only filter, clear done) and a canvas world
  (`step`, the pure function that moves it, tested; one script draws each
  frame), each with cell and window tests.
- `ax record <app> <file.rs> [range]`: a test of the whole app (aio's `am
  record`), every cell the range touched booted on its state before,
  their calls made in the log's order, each state after asserted.
- `ax ui` cell health says whether each cell is enabled or tripped and its
  effects running or queued; Metrics shows the effect backlog (amui's).
  `ax cost --json` carries both per cell (`enabled`, `effects`).
- `ax build --smoke` / `ax smoke` check the window's page as aio's smoke
  does: every file it names or loaded and every `[build.assets]` file
  answers 200, every declared guest preload attaches in a sandboxed guest
  (a real attach, not only present), and any `REFUSED` line fails it.
- `ax build --smoke` lists every artifact it didn't start, with why
  (`not smoke-tested: notes-1.0.0-windows-x64.zip: built for windows`);
  `--smoke-strict` fails the build on any of them.
- `ax build` and `ax release --app-version v`: built and released as that
  version (`1.2.3`), over `[app] version` and git (aio's `--version`).
- `[build] out` and `ax build`/`ax release --out dir` (aio's `build.out`,
  `--out`): where builds go, and where `ax release`, `ax install` and
  `ax lab` look; every place reads it through one helper.
- A labelled `[build] targets` entry takes its own `platforms`, `title`
  and `ui` (aio's `build.targets.<label>`), and `entry` as aio's spelling
  of `bin`; title and ui are stamped into that target's build.
- `[dev] watch` (aio's `watch: [..]`): the folders `ax dev` watches,
  when no `--watch` says otherwise.
- `ax lab <app> <os> --dist dir`: that folder at `/shared`; `--tunnel`: no
  port published, the viewer forwarded through `docker exec` by `ax` until
  Ctrl-C (aio's `am lab --dist`, `--tunnel`).
- `ax peer --types -`: the Rust consts on stdout (aio's bare `--types`).
- `ax help --all`: every verb, as `--commands` (aio's spelling).
- `ax release --key file`: signed by that key file (its
  `<name>.previous.key` beside it signs the key roster too), aio's
  `publish --key`.
- `ax release --commit` (or `[updates] sign_commit = true`) signs in the
  commit the build was made from.
- `ax release --min-from <v>`: the release installs only over `v` or
  newer. `minFrom` is signed; an older install finds the release
  `blocked` (`2.0.0 installs only over 1.4.10 or newer, and this is
  1.3.2: install 1.4.10 first, then update again`), and nothing gets past
  it, `apply_anyway` and `start_fresh` included. A `minFrom` that isn't a
  version refuses the release.
- Update manifests read forward: an install checks the signature over
  the release exactly as it was signed, then reads past a field it doesn't
  know (`aio::update::check_served`), so a later `ax release`'s fields
  never strand it; one added unsigned still refuses the release. A plain
  `ax release` writes the manifest aio-rs 0.2's installs read; only
  `--min-from` and `--commit` (`[updates] sign_commit = true`) add fields,
  and those need installs on aio-rs 0.3 or newer.
- AppImage delta updates: `ax release --appimage --patch-from <v>` makes
  a one-file patch (zstd against the image), listed as
  `<os>-<arch>.appimage.patches.json`; an app run from an AppImage patches
  its staged copy, else the image it runs from. Before, a patch list for
  an AppImage was written under the bundle's name, where no AppImage
  install looked.
- Updates from a folder: `[updates] url = "file:///mnt/releases/notes"`
  (a share, a USB stick), checked as a host is.
- The `updates` cell's offer says why it is offered (`reason`) and
  whether it moves the data to a newer cell version (`migrates`);
  `progress_of` says which download `progress` measures, the release's
  (`App`) or the Electron runtime it needs (`Runtime`, from 0 again).
- The `updates` cell syncs to the app's clients, served beside its own
  cells (aio syncs it to every client): server paths (`backup`, `retired`)
  stay on the server, any client checks and dismisses, and installing or
  switching channel is an admin's (`role = "admin"`).
  `aio::update::share` serves an app's own.
- The launch flag `--channel=<c>`: the update channel this run follows,
  over `AIORT_UPDATE_CHANNEL` (aio's `--channel`); `aio::paths::Ask`
  has `channel`, and `aio::paths::FLAGS` lists it.
- `--width=<px>` and `--height=<px>` (aio's): one launch's window size.
- **`--client=window|browser|server-only`** decides how one run shows
  the app, over `[app] ui` (aio's `--client`).
- `[app] flags` (aio's `appFlags`): the app's declared launch flags; a
  launch flag one typo from one of them or from aio's own is refused at
  start with the one meant (`aio::paths::misspelled`), any other still
  reaches the app.
- `[app] profiles = false` (aio's `profiles: false`): one home only; a
  launch naming another (`--profile`, `AIORT_PROFILE`, `AIO_PROFILE`,
  `--home`) is refused.
- `[home] dir` (aio's `appDir`): any app's fixed home, absolute or `~/…`,
  under `AIORT_DATA` and `--instance`; `aio::paths::plan_in` takes the
  app's `[home]` rules apart from its port.
- `AIORT_PORT`: the port a serving app listens on, the variable form of
  `--port` (aio's `AIO_PORT`); the boot report names it.
- `AIORT_INSTALL_ROOT` (aio's `AIO_INSTALL_ROOT`): where `install.sh`
  puts a Linux bundle; `uninstall.sh`, `ax installed`, `ax remove` follow
  it, and the installed app's updater finds its folder by where it runs
  from.
- `AIORT_APPS_DIR` (aio's `AIO_APPS_DIR` for any app): one root for
  every app's home, `<dir>/<id>`.
- `[log] dir` (aio's `logging.dir`): the logs of the app's own home
  elsewhere; a profile, an instance or a sandbox keeps its own `logs/`.
- The boot report says the platform (`linux/x86_64`) and the updates in
  one line: channel and who chose it, source, interval, auto or ask
  first; `not configured` without `[updates] url`.
- A second after start the boot report adds what the app opened and
  serves: `cells` (each with its data version), `routes` and
  `serverfns` (aio's boot listing of cells and routes).
- `/__aio/metrics`: `aio_broadcast_bytes_total` and
  `aio_broadcast_messages_total`, the states and patches sent to clients.
- `/__aio/vitals` on a server: each link's liveness (`status`, `gap`,
  `frozenFor`) and payloads (`lastPayloadBytes`, `totalBytes`), each
  served cell's size (`cellSizes`), the cells tripped (`breakers`), and
  the loop's `drainRate`.
- **Structured diagnostics** (aio's `DiagEvent`, `onError` context): a
  `report::Diagnostic` carries the `cell` and `call` it happened in and
  the `numbers` behind it (a budget's `took_ms`, `budget_ms`); a report
  carries them too ([errors](docs/debugging/errors.md)).
- **aio's error codes on refusals**: `REDUCE_ERROR` (a method panicked),
  `INIT_ERROR`, `QUEUE_OVERFLOW`, `DISPATCH_CLOSED`, `DISPATCH_ABORTED`,
  `DISPATCH_LOOP`, `PERSIST_ERROR` beside `ACCESS_DENIED`,
  `ACTION_REFUSED`, `CONNECTION_LOST` and `TX_CONFLICT`; a code this build
  doesn't know is kept as `Code::Other(name)` (aio's `errorCode`), and
  `aio::error_summary` gives one line (aio's `errorSummary`)
  ([errors](docs/debugging/errors.md#the-codes)).
- **A refusal's log line names its fix**
  (`permission "midi" DENIED to the app's own page: Window { permissions:
  &["midi"] } grants it`), and each kind of refusal stops after 32
  different lines, saying so once, while the others go on.
- `[data] snapshot_every`: the store's good copy made again while the app
  runs (aio's rolling snapshot that `checkIntegrityOnBoot` restores from),
  so a damaged store loses at most that much; each copy passes
  `quick_check` before it replaces the last.
- **`Db::snapshot` is aio's rolling snapshot**: the copy is made beside
  the target, checked with `quick_check`, synced and renamed over it, so
  one path on a timer always holds the last good copy (an existing
  target was refused before).
- **`aio::db::Opts::readonly`** (aio's `createDB({ readonly })`): a file
  opened read-only; every write is refused
  ([SQL](docs/persistence/sql.md)).
- **`[data] only_persist = true`** (aio's `cellDefaults: { persist:
  "none" }`): only `#[cell(persist)]` cells are saved; every other cell
  opens in memory ([aio.toml](docs/reference/aio-toml.md)).
- **`#[cell(persist = false)]`** (aio's `persist: "none"`): the cell is
  never saved, also opened on a file; it starts from its defaults on
  every launch. A cell without it is saved where it is opened, as before
  ([persistence](docs/persistence/README.md)).
- **`#[cell(recovered = f)]`** (aio's `onCheckpointRestore` returning a
  state): when the store came back from a damaged file, `f(state,
  &aio::Recovered)` returns the state to boot on, logged as a `$restore`
  event ([persistence](docs/persistence/README.md#how-the-store-works)).
- `#[cell(redact)]` (aio's `redactActions: ["vault:*"]`): every method of
  the cell is `#[redact]`; `aio::CellData::REDACT`.
- **`#[cell(transaction)]` tracks reads, as aio's `transaction: true`
  does**: `#[methods]` finds what an async method reads in its body. A
  field it only assigns (`self.f = …`) is a blind write: it never
  conflicts, the last write wins; with `serialize`, only a change to what
  it read is a conflict. A method that passes `self` on or calls a method
  of `self` counts as reading every field, as before
  ([transactions](docs/state/methods.md#transactions)).
- `#[long(ms)]`: an async method's own await ceiling (aio's per-method
  timeout), beside `#[long]` (none) and `[perf] call_ceiling` (the app's).
- **`#[skip_if_running]` on an async method** (aio's `skipIfRunning` on
  a schedule set from a method): a tick of any schedule of it, declared or
  set by `cx.every`/`cx.after`/`cx.cron`, is skipped while it runs; ten
  skipped in a row are said once (`schedule:skipping`)
  ([scheduling](docs/state/scheduling.md)).
- `#[skip_travel]` on a method (aio's `skipActions`): time travel's undo,
  redo and timeline pass over its events, which stay in the log.
- `[history] travel = false` (aio's `diagnostics.dev.timeTravel: false`):
  every time-travel step is refused.
- An async method sends its caller pieces while it works with
  `cx.part(x)`, and `Remote::stream(method, args, on_part)` receives them
  in order before the end (aio's `emitChunk` and `peer().stream()`);
  other callers get none.
- `aio::merge::Merge::Keys` (aio's `lww-per-key`, merged key by key) and
  `Merge::SetRemove` (aio's `set-remove`): a synced copy shows a record or
  a removing set merged while its call waits, with a conflict only where
  both sides changed one key or item their own way.
- **A notification raised in a signed-in user's call is said once to
  reach every user** (a warning on the bus, `tell:every-user`), as aio
  does ([telling people](docs/state/effects.md#telling-people)).
- **aio's two secret tiers on a shared cell**: a field named like a
  secret (`seed`, `token`, `secret`, `key`…) is a build warning, beside the
  refusal of an unambiguous credential; `#[public]` on a field says it is
  meant to be sent (aio's `publicFields`)
  ([visibility](docs/state/visibility.md#credentials-in-a-shared-cell)).
- A `Hidden<T>` field that isn't `#[server]` can be hidden per viewer: a
  `visible` view gives `Hidden::withheld()`, and a read on that viewer's
  copy panics naming the field (aio throws on reading a field the view
  hides), where a plain field reads as a default.
- **The thin client's connect page, on desktop and Android** (aio's
  thin client): a window's client (`Source::for_app` in a binary with
  the kit, as every window app's `main` already calls it) never fails for
  want of a server. With none named, `--connect`, one it can't reach (and
  no copy saved) or one that refuses, the window shows the page in place
  of the app: an address, the app's servers on the LAN, a pairing code,
  the recent servers (`aio::kit::ConnectPage`, `aio::connect`). It goes
  the moment a server answers. `--server-url` names the server for one
  launch ([desktop](docs/clients/desktop.md#the-connect-page)).
- **A paired server whose certificate changed proves it holds the key**
  (aio's `/__aio/pin-proof`) before the key is sent, and is pinned again;
  one that can't is refused. `[client] strict_pin` (or `--strict-pin`,
  aio's `strictPin`) refuses it outright until paired again
  (`aio::pairing::reprove`, `pin_holds`).
- **Discovery asks every network this machine is on** (the limited
  broadcast and each interface's own), lists a machine heard from two of
  its addresses once, and a probe nobody took is an error: `ax discover`
  says why and exits 1, naming macOS's Local Network permission there.
- `Remote::connect_later`, `relink` and `on_unreached`: a copy that waits
  for its server and can be pointed at another, a live link dropped for
  the new one at once.
- **`Remote::token_with(f)`** (aio's connection `token` as a function):
  the token is asked for before each reconnect.
- **A dead credential signs a synced copy out** (aio's "Signed out"):
  `Remote::signed_out()`; the calls waiting are refused with
  `ACCESS_DENIED` and taken off the copy, and new calls are refused until
  a link is made again. A page asks over HTTP whether its credential
  died, where it retried a refused socket forever
  ([offline](docs/persistence/offline.md)).
- **A CLI's synced client says the notifications it gets**
  (`INFO [cli] notify: …`, aio's `connectCli`), never drops them.
- `SignIn` offers single sign-on when the server has an identity
  provider (`sso`, `sso_label`, part `signinSso`). A page goes to the
  provider and back; a desktop or phone app opens the browser and gets the
  session back on a loopback port (`Client::sso`, `Client::sso_done`,
  `POST /__aio/auth/oidc/redeem`), bound to a verifier the app keeps. `Me`
  has `oidc`.
- `Users::resolve_with`: an async users resolver (aio's `resolveUser`)
  that reads the app's state through the handles it captures and may say
  more of the user (`Known::extra`, read by a route with `Req::extra`).
- `[accounts] totp = false` and `cookie = false` (aio's `auth.totp`,
  `auth.cookie`): no second-factor enrollment (`totp_disabled`), no
  session cookie on sign-in; `aio::auth::Policy` has `totp` and `cookie`.
- `[accounts] signup` with `require_verified` and no mail hook
  (`Http::mail`) refuses to serve, naming why (aio's boot refusal):
  no one who signed up could ever sign in.
- **`ax profile <app> --link`**: a browser link carrying the app's key
  (`https://host:port/?token=<key>`, aio's `share:` link), on demand; the
  boot's share line names it, and the key stays out of every log
  ([the key](docs/auth/README.md#the-key)).
- `TestIo::run(argv, main)`: a command-line app's whole run in a test
  (aio's `testIO()`), `args` reading `argv`, `--help`, a refusal and
  `fail` ending the run with their exit code instead of the process.
- `aio::cli::watch` hides the cursor on a terminal (shown again on exit,
  Ctrl+C or SIGTERM) and draws one frame per `TICK` (16 ms), the tick's
  last state.
- `Window { chrome }` (aio's `ui.chrome`): `Chrome::Themed` draws no OS
  frame and puts aio's title bar in the page (drag region, minimize,
  maximize, close, a double click maximizes, the page's title), styled by
  `.aio-titlebar*` and `--aio-titlebar-*`; `Chrome::None` leaves the app
  to draw its own.
- **DevTools in a release build, only where the app allows it**:
  `Window { devtools: true }` gives F12, Ctrl+Shift+I and the launch's
  `--cdp=<port>`; never from the environment.
- **Child windows as aio's**: `Child { unsandboxed: true }`, allowed by
  `Web { unsandboxed_windows: true }` and said in the log, else refused
  with the reason; `Child { permissions }`, what its page may hold on its
  own origin.
- **A refused guest preload reaches the page**:
  `GuestEvent::PreloadRefused { preload, reason }` and the window's
  `aio:guest-preload-refused` event.
- `aio::kit::run_js(id, js)` runs a script in a `Browser`'s guest and
  answers its value (`executeJavaScript`); `Go::Css` adds a stylesheet
  (`insertCSS`), `Go::Zoom` (`setZoomFactor`), `Go::DevTools`.
- An icon frame of `Tell::Icon` may be the app's URL path of a
  `[build] assets` file (`"/icons/busy.png"`, aio's `appIcon()` app URL).
- A web build's page sends its console to the server that serves it in a
  release too (aio's client log): `[page]` lines in the app's log, 100 a
  second at most; `[server] page_log = false` takes none.
- `if let` (and `else if let`, `let` chains) and `match` (guards,
  nested) in `rsx!`, which 0.2.0 refused: the arm's view is made again
  when another arm matches or what its pattern binds changes; an arm
  that binds nothing keeps its view and state. `ax lint` checks a
  `match`'s arms as branches. `while` and `loop` stay refused: a view
  says what shows, and a `for` over the items lists it.
- An attribute takes any string a hole makes: `title: t(&x)` (a borrow
  of the hole's own clone), a `Cow<str>`, an `Option<&String>`. A
  borrowed value is compared with the last one and copied only when it
  changed.
- `aio::ui::prelude::on_change(read, |next, prev| ..)` (aio's
  `watch(source, fn)` and `on()`) and `untrack`.
- `effect` returns an `Effect` (`dispose()` stops it); `effect_quiet` is
  deaf to its own writes (aio's default `effect`; air's `effect` is aio's
  `rerunOnSelfWrite: true`).
- `ErrorBoundary`'s fallback may try again: `retry()` gives its `Retry`,
  `r.retry()` makes the children anew (aio's boundary reset).
- `Resource::mutate(v)` shows a value at once and `Resource::dispose()`
  loads no more (aio's `resource()`).
- `Motion::ticking(f)`: `f(t, u)` each frame while a transition plays
  (aio's `TransitionFn` `tick`).
- `Field`'s `hint`, `Breadcrumb`'s `separator` and `EmptyState`'s `icon`
  take text, state or any view; `Opt::view` shows any view beside a radio
  (aio's any-view props).
- `Form::bind_check` binds a bool field to a `Checkbox` or `Switch`,
  `Form::bind_parsed` a number (any `FromStr`) to an `Input`, and
  `RadioGroup` takes `bind` (aio's `form.bind` for every field type).
- `Portal { target }` draws in a named `PortalHost { name }` anywhere on
  the page (aio's `Portal target`).
- `Link { active_style }` (aio's `activeStyle`); `Link`'s `to` and `class`
  follow state.
- `Redirect { push: true }` keeps the redirecting place in the history
  (aio's `replace: false`).
- A web page under a base path (`<base href="/app/">`) routes from it:
  the router takes the base off the address and puts it back (aio's route
  base).
- `Tooltip` `Placement::Left` and `Right` (aio had top and bottom only).
- `aio::kit::use_window_listener(kind, f)`: any DOM event type the window
  hears, each one, with no throttle (aio's `onWindowEvent(type)`).
- `aio::held::Held` and `aio::kit::use_held` in a browser page (a web
  build), as on the desktop (aio's `useResource` in a page).
- **`ax release` warns** in one line when a release names its commit
  (`--commit`, `[updates] sign_commit`) or a `minFrom`: installs on
  aio-rs older than 0.3 can't read it; publish a plain bridge release
  first ([releases](docs/build/release.md)).
- `ax release --aio --target <triple>`: another target this machine
  builds, so an arm64 Mac makes both Mac archives of a release by hand
  (`--target x86_64-apple-darwin`).

### Changed

- **`ax dev` lands a Rust edit in ~0.3 s** (the template's view logic
  0.51 → 0.27 s, a cell method 0.59 → 0.40 s, save to screen; aio's `am
  dev` 0.41 and 0.84 s on the same machine,
  [comparison](docs/comparison.md#the-edit-loop-next-to-aios-am-dev)). A
  patch compiles from the first save, kept if the burst was just that
  save as the compile read it. A library's patch goes in without
  compiling the launcher binary (its running objects are linked in as they
  are); the launcher is compiled after: when its objects come out as the
  running ones nothing more is sent, else a second patch carries them.
- **The start after an `ax dev` session launches at once** (0.35 s, was
  1.1 s): once a patched session is quiet for 2 s, ax brings the next
  start's fat build up to date behind it at low priority (nice 10;
  Windows below normal), linked beside the running binary, stopped by a
  save. A session that ends behind it, by Ctrl-C, SIGTERM, the terminal
  or console closing too, starts `ax dev <app> --warm` in the background.
- A start with nothing changed runs the last build as it is: no rebuild,
  no link (1.4 s → 0.5 s to the window).
- **`ax setup` builds aio-rs in the background** at a terminal (the
  installers' last step): an app's first start compiles only its own
  crates (1.7 s, was 24 s); one started meanwhile waits on cargo's lock
  and builds nothing twice. `--check`, no terminal and `AIORT_NO_WARM=1`
  leave it out.
- One shared build folder per machine and toolchain, in aio's cache home
  (`~/.cache/aio-rt/target/<toolchain>` on Linux), named by the
  toolchain of the workspace ax runs in: aio-rs is compiled once for
  every app; two toolchains never thrash one folder.
- `ax dev`, `ax test` and `ax lint` build for this machine without
  `--target`: the libraries a build script or macro shares with the app
  (libc, serde, base64…) build once, not twice. The template's first
  `ax dev` build 68 → 62 CPU-s, 26 → 22 s (4 jobs).
- A changed type, static, signature or item-level macro call restarts the
  app instead of patching it (its cells come back from their log); a new
  function in a module, a new test or a body edit is still patched, and
  items declared in a function's body count too.
- A save while a build runs stops that build (cargo and every rustc it
  started) and starts the next at once on the newer code.
- aio's lint of a save runs beside the patch, not before it.
- cargo's words as they come under `ax dev`, its progress bar on a
  terminal, a warning already shown said again in one line.
- The boot report in one line on `ax dev`'s terminal (`--verbose`: every
  row); Chromium's D-Bus chatter is left out.
- `ax uninstall --tools` also takes the shared build folders and ax 0.2's.
- An `am` verb typed from habit is refused with its exact `ax` form:
  `instances`, `watch`, `actions`, `client`, `compile`, `publish`,
  `migrate`, `link` (each naming its verb), `check`, `where` and `add`
  (what does their job), beside `dispatch`, `fix` and `theme`.
- Smaller code per view: a hole's closure is behind a one-entry vtable
  (no `FnMut`/`FnOnce` shims), `memo` is generic over its value, not
  its closure, and a list's row work is one copy for every item type.
  The bench's 50-view crate (`crates/air-bench`): release `.text`
  1.41 → 1.16 MB, debug binary 17.5 → 15.2 MB, LLVM IR 1.15 M → 0.96 M
  lines (`scripts/air-ir`'s limit 25 000 → 21 000 a view). Mounting and
  updating 1000 rows takes as long as before.
- air: the rows that leave a list stop at once and what they held is
  dropped after the frame is sent (`air::drop_left`, the next event at
  the latest), and a hole that reads nothing is run once and never
  boxed. A jump over 1000 rows (End on the wallet port): the 60 rows'
  mount 3.1-4.1 → 2.1-3.3 ms, the app's work for the key 4.7-8 →
  3.9-5.5 ms; key to paint, stamped, End 13.5 / 19.6 ms (aio 13.5 /
  20.4), Tab 13.1 → 11.6 ms (aio 9.8); `air-bench` held key p50 1.2 →
  1.1 ms.
- A web build (`ax build --web`) is 8% smaller: the kit's stylesheet and
  air's interpreter are built into the wasm without comments or
  indentation, the theme's tokens are printed without float formatting
  (its code no longer in the page), and wasm-bindgen's script loses its
  indentation and comment lines. The template's site 454 → 418 KB (wasm
  408 → 378 KB; `scripts/dx-metrics`' `air_web_b` 461,818 → 425,603 B,
  under its limit again); a screenshot of its page is the same, pixel
  for pixel.
- A desktop app's binary is 60 KB (1%) smaller: zstd, which reads delta
  updates, is built small (its `thin` feature); the template's 5.42 →
  5.36 MB, its AppImage 3.61 → 3.58 MB. `scripts/slim-metrics --check`
  fails when the feature is off (deps.toml `[kept-features]`).
- `aio::health::on_memory`'s `MemoryReport` says the `trend` (`Rising`,
  `Stable`, `Falling`), as aio's `onMemoryPressure`.
- **A client's server is reached without a `[capabilities] network`
  entry** (aio's thin client): the one the person picks on the connect
  page or pairs with, `--server-url`, or `[client] server`; that host and
  port alone (`aio::capabilities::grant`). Every other host still needs
  one, and `[client] server` no longer fails the build without it.
- An app's own update runtime (`aio::update::install` before start) keeps
  the `updates` cell, with or without `[updates] url`, and the built-in
  checks stay out; aio-rs 0.2 replaced it with the built-in one.
- A cell whose log no longer replays is quarantined in a release build
  (aio's): it runs at its last snapshot, refuses calls with
  `PERSIST_ERROR`, serves no synced client and keeps its log for a fixed
  build (`Store::quarantined`); a debug build still refuses the boot.
  `aio::testing::quarantine` sets it in a test.
- A problem report's state is screened by `visible` as a client with the
  key sees it (aio's screening); mid-panic a cell with `visible` is
  withheld, since its code is not run then.
- A `Hidden<T>` that a copy or view doesn't have serializes as
  `{"$hidden": true}` instead of `null`, so a `Hidden<Option<T>>` keeps
  its `None`.
- A known user whose re-checked token says another role keeps the open
  socket and sees the new role's view at once (aio's push); before, the
  socket closed and reconnected.
- `#[ttl(ms)]` answers network callers too, each from an answer kept for
  that caller (user and role): before, a network call always ran again.
- docs/build: release build time against binary size, per LTO setting
  (the template's fat LTO: 27 s for a one-line change, the smallest
  binary; without LTO: 2.1 s, 37% bigger), and how to try a release
  build quickly without shipping it.
- docs/aio-vs-aio-rs.md: key to paint by two clocks, the frame's stamp
  and when it ran (aio's End: stamped 12 ms, ran 224 ms after the key).
- DevTools refusals and docs say what decides: an `ax dev` build
  (feature `dev`) has DevTools and reads `AIORT_CDP`; any other build (a
  release, a plain `cargo build`) only with `Window { devtools: true }`
  and the typed `--cdp`.
- `aio::Big` and docs/performance.md state its rule: plain data only,
  never one that changes behind `&` (`Cell`, `RefCell`, `Mutex`,
  atomics), whose writes would reach the states before the call and stay
  unseen.
- A saved copy's calls that can't be sent are kept beside its file under
  a name with the time (`copy.orphan-<cell>-v<n>-<ms>`, in a page
  `<key>.orphan-v<n>`, then `-2`…), never over an earlier set.
- A timed snapshot (`[data] snapshot_every`) runs once per store,
  however many cells boot on it, and ends when the store's file is gone.
- **The template app's binary is 0.74 MB smaller** (5.71 → 4.97 MB, its
  AppImage 3.75 → 3.41 MB and `.tar.gz` 2.97 → 2.61 MB;
  `scripts/dx-metrics`): an app carries only what it uses. The updater
  (aio-rs-shell's `updates`) is in an app's build only when its aio.toml
  has an `[updates] url`: `ax build`, `ax release` and `ax dev` turn it
  on by themselves, as they do `native-splash` for a `[build.splash]`.
  An app that brings its own update runtime (`aio::update::install`) and
  turned the shell's default features off names `updates`
  ([migration](docs/migration/README.md#builds-and-ax)).
- The connect page is in a binary only with aio's `server` (a client's,
  the only one that shows it), and `DevTimeTravel`'s registry only with
  aio's `dev` (`ax dev`'s builds).
- `ax build --web`: a standalone app's page sends its console nowhere
  (it has no server to take it; it posted to whatever host served the
  site); an app with a server and `[server] page_log` still sends it.
- `Merge::Keys` builds its record without sorting the keys: ~13 KB less
  code in every app (the template's web build back under its limit).
- `ax create` that can't write `Cargo.lock` (offline, an aio-rs release
  not there yet) says cargo's cause in one line and what to do, not
  cargo's whole error chain.
- `ax release --aio` no longer lists the Android keystore among its
  signatures: `ax` has no APK.

### Fixed

- A compile error in a patch is said once and shown on the window; the
  app runs on and the next save patches again (it no longer went on to
  two full rebuilds that failed the same way).
- A save made as a temporary copy renamed over the file (`sed -i`,
  safe-write editors, agents' write tools) is no longer a rebuild and a
  restart.
- A panic in the window's views is no longer taken for the person closing
  it: the session waits for a fix.
- A first build that fails still opens the window, on the last good build
  (the fat binary kept from its last link, never a half-written file), the
  errors on it.
- Markup saved after a save made during the first build reaches the
  window (it was told against the code before that save).
- Ctrl-C at `ax dev`, or its terminal closing, now stops every cargo and
  rustc it started.
- `ax create`'s background warm-up and an `ax dev` no longer race on the
  same fat build (one waits for the other); on Windows the warm-up is
  detached from the console.
- `ax setup --warm` makes its scratch app again when another ax or
  toolchain pin made it.
- A global whose type a hot-patch changed starts again from its first
  value (air with `dev`); any other build still stops on it.
- `AIORT_NO_SPLASH=0` and `AIORT_NO_WARM=0` mean "on", as unset.
- A hole that borrows a temporary it makes (`{t(&sig.get())}`,
  `title: t(&sig.get())`) compiles (was E0716).
- An app whose `main` returned an error lost that line (`Error: …`,
  printed after `main`) from its log and terminal: the log's copier is now
  finished at the process's exit too (C's `exit`; a second at most). `ax
  dev` then says an app that ended by its own reason, in the terminal's
  `ended` line and on the splash.
- `ax dev` on macOS reads a freshly linked binary through before it
  starts it: the system's first-start check of a new binary no longer
  reads it from disk slowly (a 60 MB binary's first start ~0.65 s instead
  of ~1.15 s).
- A stop (`ax stop`, a closed window, SIGTERM, `ax dev`'s restart)
  snapshots each persisted cell, and `ax dev` snapshots them before every
  hot-patch (`Handle::checkpoint`): the next start no longer replays calls
  made before an edit through the edited code (a counter came back 116
  for 109 clicks).
- A launch while the app's instance is stopping (`ax dev` started again
  at once, a double-click during a quit) is no longer handed to the
  instance that is going away and lost, nor refused: that instance
  answers that it is ending (or its door is already gone), and the launch
  starts as soon as it has let go of the app's lock (30 s at most, then
  it says who holds it). It made `scripts/dx-metrics`' `dev_restart_s`
  fail now and then: the new app exited 0 at once, its launch handed
  over, or exited 1, "the app is running but not answering".
- **A replayed library's code is all in its patch** on macOS and Linux
  arm64: its rlib went to the linker whole, which takes only the members
  something in the patch calls, so code reached otherwise (a view's
  function, by pointer) stayed the old one; its objects are linked now,
  as on Linux x86_64 and Windows.
- **`ax dev` never keeps a hot patch's library as cargo's build.** A
  patch replays a library into the build folder; a source later put back
  with its old modification time (a `git checkout`, a copy) left cargo
  trusting the patched code, and the next session started on it. The
  replayed libraries' fingerprints are now dropped, so cargo builds them
  again.
- **A server names its app (`[app] id`) to a pairing device and on the
  LAN**, not its cell's id: `ax pair <app>` and the connect page pair with
  an app whose cell has another id (the template's `counter`), as aio's
  `appId`.
- **Calls queued offline on another version of the cell are kept**: a
  saved copy this build can't use keeps its waiting calls beside it
  (`<file>.orphan-<cell>-v<n>`, in a page `<key>.orphan-v<n>`) and says so
  (`sync:orphan-queue`, aio's `sync-orphan-queue`); they were dropped
  silently ([offline](docs/persistence/offline.md)).
- **`Oidc::redirect_uri` is checked at boot**: one that isn't an absolute
  `http(s)://` URL stops the boot (it reached the provider as it was),
  and the first sign-in logs the `redirect_uri` it sends, as aio does.
- **`#[mode(first)]` joins only a running call of the same caller with
  the same arguments**, as aio's `"first"`: `scan("/b")` no longer gets
  `scan("/a")`'s answer, it runs beside it. `cx.abort("scan")` still
  stops every call of `scan`.
- **A declared `skip_if_running` schedule of a `#[mode(first)]` method
  skips again**: its effect is keyed by its arguments since `#[mode(first)]`
  joins by them, and the check now finds it.
- **An exposed app answers to this machine's own name**
  (`http://<hostname>:<port>/` from the LAN), as aio does; it was 403
  unless listed in `[server] hosts`. `aio::gate::machine_name()` says it.
- **`Remote::merge` refuses a field the cell doesn't have** (a panic
  naming its fields, aio's typo check); a misspelt one merged nothing.
- **An aio `state.db` keeps the app's own `PRAGMA user_version`.** aio
  leaves it to the app; aio-rs stamped its own layout there, so a file
  the app had set to 1 failed to boot (a duplicate column), one above 2
  was refused as newer, and the app's migrations lost their stamp. In an
  aio file aio-rs now reads its layout from its own tables and never
  writes `user_version`; `ax events`, `ax update` and the data gate read
  it the same way.
- A server's door (`ax stop`, later launches) tells a launch that comes
  once a stop is asked that the app is ending, so it waits for the lock
  instead of being handed to an instance about to go. New:
  `aio::lifecycle::stopping()`.
- Rows that left a list are dropped once their frame is out on the web
  build and in `TestUi` and `air::testing::TestPage` too (as in a
  window), not at the next event: a page with no events kept them.
- A `match` or `if let` arm in `rsx!` whose pattern names a path
  (`crate::Tab::A`, `self::…`), a struct field (`{ x: 0 }`) or a tuple
  struct binds nothing, so its view and state are kept while it matches;
  only a real binding (`x`, `x @ …`, `{ x }`) makes it again on each
  change. The captures of a `match` share the rule.
- `effect_quiet` is deaf to its own writes made under `untrack` too:
  they no longer run it again (a loop).
- A `Users` resolver is always asked off the server's async threads
  (sockets, the UI socket, routes): an async resolver
  (`Users::resolve_with`) that awaits what a `current_thread` runtime
  drives no longer deadlocks it.
- **Calls made offline are never lost to a refusal.** Only a refused key
  (the server's 401) signs a synced copy out; another refusal (a server
  of another cell picked on the connect page, a plain link a phone won't
  use, a full server) keeps the waiting calls and stops until the copy
  is pointed elsewhere. A signed-out saved copy keeps its waiting calls
  beside its file (`copy.signed-out-<ms>`) and says so
  (`sync:signed-out-queue`) instead of deleting them
  ([offline](docs/persistence/offline.md)).
- A streamed call dropped from a full queue or refused at a sign-out
  lets go of its piece handler.
- A timed snapshot no longer races a cell's boot or another snapshot on
  the store's half-made good copy (a later recovery could restore a
  damaged one).
- A quarantined cell boots its snapshot repaired as any boot's
  (`restore`, `recovered`), labelled with the snapshot's event, not the
  log's last.
- A staged release keeps its manifest exactly as served, so its
  signature still holds over fields this build doesn't know.
- A served manifest's signature is checked before its release is read.
- A debug build logs what each call changed at `debug` level again
  (`[diff]`), as `ax dev`'s does.
- An update source `file://host/share` (another host) is refused, never
  read as a path relative to where the app runs.
- `ax dev` stops a stale build when ax was started with Ctrl-C ignored
  (`nohup`, a background job): cargo starts with SIGINT and SIGTERM at
  their defaults, so the stop reaches it and its rustc calls.
- `#[methods]` refuses a cell method named `checkpoint`: a handle's own
  `checkpoint` would hide it.
- `TestUi`'s window is drawn at 1 CSS pixel to a device pixel on a HiDPI
  host (a Retina Mac): shots, wheel steps and zoom are the same on every
  machine.
- `aio.toml`'s `[home] dir`, `[log] dir` and `[server] cert` / `key`
  take any OS's absolute path (`/srv/notes`, `C:\notes`, `\\host\share`)
  on every build host, as an app may be built for another OS; the app
  refuses a `[home] dir` or `[log] dir` that isn't absolute where it
  runs, naming the key. Before, a Linux path failed the build on Windows
  and the other way round.
- `[dev] watch` refuses a Windows drive or root (`C:x`, `\x`) on every
  build host, not only on Windows.
- `ci-windows.sh` no longer runs the hot-patch test on the remote
  Windows: it builds a fixture with cargo from the checkout, which isn't
  there. GitHub CI's Windows job runs it natively.
- A saved copy (`Remote::connect_saved`) no longer loses a call made
  offline when the process ends right after it: the last copy dropped,
  `aio::shutdown` (a SIGTERM, a window closed) and the process's exit (a
  return from `main`, `std::process::exit`, Windows included) write its
  last save first, in that thread, bounded. Saves are written by one
  writer per file, in order: a copy dropped and opened again at once
  starts from its newest save, and a copy still closing never saves an
  older state over the new one's. A call still never waits for the disk.
- Each sign-out's `sync:signed-out-queue` (and each `sync:orphan-queue`)
  reaches `report::subscribe`, even within `QUIET_MS` of another: the
  message that says where the kept calls are is held back only as an
  exact repeat.
- `ax build --sizes` builds with the release build's features: an app
  with a `[build.splash]` was sized without its splash.
- The accessibility check no longer flags a `<button>` without `type`
  outside a `<form>` (there it submits nothing): the template's buttons
  no longer warn on its first `ax dev`.
- The kit's header and footer rules matched `#root>header`, an element
  aio never makes: they match the app's `#main` now, and the page no
  longer warns that the stylesheet styles `#root`.
- Chromium's tray asking a missing D-Bus for a tray host is dropped as
  the rest of its no-bus chatter is, in that exact shape only.
- App bundles (`.tar.gz`) and aio-rs's own archives store their files as
  owned by 0:0, with no user or group name: the packer's account stays
  on their machine. The `ax` in aio-rs's archives is `rwxr-xr-x`
  whatever the packing machine's umask (a strict one left it runnable
  by its owner only).
- Two `ax build` runs of one app no longer pack over each other in its
  `dist/`: the second waits for the first, and says so.

### Security

- Prebuilt libraries are fetched only over `https://` and only with a
  signature key to check (`AIORT_PREBUILT_KEY`); an archive is checked
  whole (its paths, no links, at most 1 GB a file and 8 GB in all, within
  5 minutes) and unpacked apart before anything reaches the build folder.
  A pack holds one build's units only, with no path of the machine that
  made it, and leaves out every package with native build output and
  those built on it.
- `POST /__aio/log` (`[server] page_log`) takes lines only from who may
  read health: anonymous on this machine alone, else the key (a bearer,
  `?token=` or the page's key cookie) or a session. Its pace is per
  client, and a page's line holds no escapes, colour included.
- A paired server whose certificate changed is never asked to prove a
  weak app key (`aio::pairing::reprove`): the client's half would let
  whoever answers guess it offline. Pair again.
- A pairing from the connect page that fails (or finds another app) no
  longer leaves its address reachable past `[capabilities] network`.
- An app's provider sign-in (`oidc/start?port=…&claim=…`) asks the
  person in the browser first; going on needs a tag tied to that
  browser's binder cookie, the port and the claim. A link made by
  someone else can no longer send a session to another user's port on a
  shared machine.
- `--server-url …?token=<key>` warns: a command line is seen by every
  user of the machine. The docs give the key in `AIORT_TOKEN` (or
  `data/sync.token`) instead.
- An unsandboxed child window (`Child { unsandboxed: true }`) no longer
  shares the sandboxed ones' session: unset, its partition is
  `persist:aio-child-unsandboxed`, and `persist:aio-child` is refused
  for it.
- A client of the `updates` cell no longer sees server paths in `error`
  or `failed` (an install dir, a `file://` source): each word naming a
  path is said as `…`.
## [0.2.0] - 2026-11-02

aio-rs draws its views with air, its own fine-grained renderer, and
Dioxus is gone: a view's body runs once and its holes follow state. An
app on 0.1 moves with the [migration guide's 0.1 → 0.2
section](docs/migration/README.md#8-01--02-dioxus-is-gone) and
`scripts/aio-ui-migrate`.

### Breaking

- **Dioxus is gone: air is aio-rs's one renderer** (air phase 5,
  docs/design/air.md). Every Dioxus crate (dioxus, dioxus-*,
  generational-box, subsecond) is out of the graph: the lock holds 253
  packages, from 327; an app's whole graph (`app kit`) 76 crates, from
  141; aio-rs-cli 146, from 211 (`deps.toml [ceilings]`). Feature `kit` is
  air; `air` stays, a name for `kit`; `liveview` is gone. aio-rs-shell has
  no `dioxus` feature, `ax dev` needs no dx (`ax setup` installs none),
  and no `--dioxus` lane is left in `scripts/dx-metrics`,
  `scripts/android-gate` or GitHub CI. No public signature names a Dioxus
  type ([migration: 0.1 → 0.2](docs/migration/README.md#8-01--02-dioxus-is-gone),
  a before → after table).
- A view's body runs once; what shows state is a hole (a text, an
  attribute, a `{child}`) that runs again when what it reads changes. A
  value read in the body (`let s = shop();` then `"{s.stock}"`) is read
  once: a dev build says where (`hint`). A component's prop follows its
  caller only as a `Signal`, a `Part` or a `Prop<T>`.
- `rsx!` refuses `if let`, `match`, `while` and `loop` nodes (a bool, a
  `{match ..}` hole) and a `for` row without `key:` on its first node.
- `aio-rs-hotpatch` (`aio_hotpatch::HotFn`, `JumpTable`, `apply_patch`)
  replaces subsecond in an app's dev build: the same jump table as
  dioxus-cli 0.7.10's, read by `ax dev`.
- `ax components`: each component's key, how many times its body ran and
  how many times its holes wrote (`updates`), where 0.1 gave renders and
  their time.
- The page scripts made for Dioxus's interpreter are gone from
  `aio::page`: `WIRE`, `TYPING`, `SET_FOCUS`, `FILE_INPUT`.

### Added

- air: a file input's picked files reach the views, read whole by the
  page: `e.files()` on its `input`/`change` and on its form's `submit`
  (`FileData`: `name`, `size`, `content_type`, `last_modified`, `field`,
  `read_bytes().await`, `read_string().await`, as 0.1's), in order
  with the events after them; `TestUi::pick_files` picks them in a test.
- `ax components --renders` on air: the last 200 renders as they happened
  (each body run, `mount` or `signal`, and each hole rewrite, `signal`),
  by component and key, timed, and how many in all; `air::renders()`
  gives the same in a test. A debug build logs them (a browser's wasm
  and a release don't).
- **TS21: the published crates' public API is checked in**
  (`scripts/api/<crate>.txt`, from rustdoc's JSON): `scripts/public-api
  --check` in `./ci.sh lint` fails on a change not written there, and on
  any item naming aio-rs 0.1's renderer.
- **The app's routes in its window with no TCP port** (aio's `aio://app/<path>`
  over the app socket): a standalone or client-server app whose
  `Source::for_app_with` serves a path (routes, `[server] dirs`, blobs)
  answers its window's `fetch`, `<img>` and streams over a private socket
  (a Unix socket, a named pipe on Windows) that admits only the Electron
  process aio-shell spawned; no window, no answer. The window's side is
  aio's `socketFetch` (6 connections, small and error bodies read at once,
  streams both ways, 504 after 30 s with no first byte,
  `AIO_SOCKET_TIMEOUT_MS`). New: `aio::http::serve_window`,
  `aio::ipc::Only::window`, `aio::ipc::window_http`. No new dependency
  (axum's own server, already built for `[server] listen`).
- **`ax dev` hot-patches Rust on Windows too** (an app on air, x86_64,
  MSVC or MinGW): the fat binary is linked at a fixed base below 4 GiB
  with a COFF symbol table (rust-lld for MSVC, its PDB kept), each patch
  is a DLL linked at a fixed base the app names (`Aio.aslr`'s `base`, a
  free span near the binary), and patched objects use the running
  binary's writable statics and thread-locals, so state is kept as on
  Linux. A patch that can't load there fails to load, never misresolves:
  ax rebuilds. Measured on a Windows 11 VM: 0.34–0.44 s a patch against a
  0.64 s rebuild that also restarts the app.
- **The splash drawn by the OS before Electron starts, at every start**
  (aio's splash): GTK 3 over FFI on Linux (opened at run time, no new
  crate), Cocoa on macOS, Windows Forms on Windows (for a download; when
  Electron is here its main process draws it, as aio's). A first start's
  Electron download runs on it, with its bar and a ✕ that cancels it.
  New `[build.splash]` keys: `position`, `follow_window`, `shadow`,
  `window_shadow`, `cancel`, `progress`, `bar_style`, `bar_height`,
  `bar_position`, `bar_radius`. `ax build` builds it in for an app with a
  `[build.splash]` (aio-rs-shell's `native-splash` feature), so an app
  without one carries none of it. `Window { splash_show }` may take 50 ms;
  one that panics or takes longer leaves the build's `show`, said
  ([desktop](docs/clients/desktop.md#the-splash)).
- **Prebuilt `ax` for macOS** (arm64 and x86_64): the release workflow
  (`.github/workflows/release.yml`) builds and smokes `ax` on a macOS
  runner of each CPU beside Linux and Windows, pins every archive in
  `install.sh` / `install.ps1`, writes `SHA256SUMS`, installs it with the
  one-line installer on each OS, and on a tag makes a draft release; a
  manual or pull-request run is a dry run.
- `scripts/update-proof`, in GitHub CI on Linux, macOS and Windows: a
  sample app installed from its bundle, updated through a signed patch,
  then a broken update rolled back with its data read back.
- `scripts/android-gate`, required before a release: a throwaway AVD on
  port 5600, a sample APK started and upgraded in place with its data kept.
- `scripts/numbers`: the README's and docs' numbers measured again, beside
  the published ones, with the machine.
- [When to use aio-rs, and when not](docs/basics/when.md), linked from the
  README's top: what it is for, what not (SEO, CDN, edge), pinning the tag,
  which parts are experimental.
- **Sessions without accounts** (aio's `sessions: true`): `[accounts]
  sessions = true` keeps the session store with no users and no login
  doors; the app issues sessions (`auth::serving()`'s `issue`), and a
  session is that user on every door (`Users::sessions`, composing with
  `Http::users`).
- **A user's token is re-checked on its open socket** (aio's
  `users:`/`resolveUser` re-check): every 5 s and at once on a revoke; one
  no longer resolved (or a panicking resolver, or a changed role) closes
  the socket; a resolver slower than 3 s keeps the last verdict.
- `--tls-cert`, `--tls-key` and `--no-tls` launch flags (aio's), over
  `[server] cert`, `key` and `tls`.
- **Vitals on the bus, as one snapshot** (aio's `onDiagnostic`, hints):
  each layer's step is `vitals:loop`, `vitals:transport` or
  `vitals:render` on the bus; the loop, the worst link (and whether it is
  on loopback) and the window's paint feed one snapshot, so the
  transport, client-main-thread, slow-call and freeze-recover hints fire.
  `GET /__aio/vitals` and `ax vitals <app>` say every layer and the
  likely cause (also in `health.json`).
- Bus diagnostics carry `severity` and `tip`, and `Diagnostic::code()`
  types them (aio's severity, fix and error object); a degraded part
  that works again is `degraded-recovered:<part>` (`Code::Recovered`).
- `aio::health::on_memory(|report| ..)` (aio's `onMemoryPressure`) and
  `[memory] every` (aio's `memory.interval`).
- `[log] per_run`: each run starts fresh log files (aio's per-run
  `debug.log`).
- Correlation ids: a call's log lines end with `call=<id>`, and its
  timeline entry (`id`) and a report it causes (`call`) carry it.
- An exposed boot prints a `share:` line (the address at this machine's
  LAN IP and what a caller shows; the key file, never the key) and, on
  Windows and macOS, a `firewall:` line (aio's).
- **The mutation ledger** (aio's `check:mutations`, carried to the
  package): `scripts/mutants` plants each break of `scripts/mutants.toml`
  alone, in the source or in the template app's built `.deb` (its menu
  entry, its icon, its bundled page), and needs the named test or
  `ax smoke` red; `scripts/mutants-ledger.csv` keeps the last whole run.
  `./ci.sh` checks the ledger, GitHub CI runs its fast breaks
  ([proving the tests](docs/testing/proving.md)).
- `ax lint` names tests that check nothing (no assertion, `?`, `unwrap` or
  harness wait) and `assert_eq!(x, x)` (aio's `check:vacuous`);
  `ax test <app> --mutants` runs `cargo mutants` over the app.
- `aio::testing::TestIo` (aio's `testIO`): a command-line run with its
  stdin, terminal or not, colour, stdout and stderr.
- `TestCells::fuzz_methods` (aio's `fuzz({ skip })`): the cell's own
  methods, arguments made from their types; `last_effects` (aio's
  `expect.effects`), `reset(&h, seed)` (aio's `t.init`, `t.destroy`),
  `budget::<C>(method, ms)` (aio's `perfBudget`, failing the test).
- `aio::testing::TestApps` (aio's `testApps`), `patches(&copy)` with
  `until` (aio's `patches`, `waitForPatch`), `call_all` (aio's
  `dispatchAll`), `Remote::on_patch` (aio's `onPatch`).
- `Mount::fetch(host, answer)` (aio's `testUI({ fetch })`): the page's
  requests to a host answered by the test, never the network.
- `aio::testing::smoke(bundle)` (aio's `smoke()`): `ax smoke` in a test.
- `Cassette::wrap(id, f)` (aio's `tape.wrap`); `AIO_CASSETTE=rerecord`
  records over a tape that is there, even a damaged one.
- **air, aio-rs's own renderer, phase 1** (docs/design/air.md): crates
  `aio-rs-air` and `aio-rs-air-macro`, and feature `air` of aio-rs, where
  `aio::ui::prelude` and `TestUi` run on air with no Dioxus in the build
  (50 crates, against `kit`'s 116). A view's body runs once; each hole
  of `rsx!` follows exactly the signals it read. Views are built where
  they are placed (their context and boundaries), components are holes
  whose hooks keep their state if the body waits for a resource, plus
  keyed lists, context, `ErrorBoundary`, `Suspense`, resources and tasks,
  typed W3C-shaped event payloads that bubble through the handlers
  (`stop_propagation` works), page scripts (`eval`), `onmounted` and
  globals per app instance. Dioxus stays the default renderer; with
  `kit` on too the facade is Dioxus's (the kit moves in step 2).
  Measured again on the spike's bench (`crates/air-bench`, now in the
  workspace): a one-line edit rebuilds in 0.54x Dioxus's time, a clean
  build in 0.30x with 8 crates instead of 77, a held key over 10 000 rows
  answers in 0.32x.
- air's props, as the kit and the router need them: `#[component]` and
  `#[derive(Props)]` make a builder, so `#[props(default)]` and
  `#[props(default = e)]` props may be left out, an `Option<T>` prop is
  `None` unless given and takes a `T`, a `View` prop is empty, a closure
  given to a handler prop is typed by it, and a required prop left out is
  a compile error naming it. `#[props(extends = GlobalAttributes)]
  attributes: Vec<Attribute>` takes the attributes given to a component
  (`"data-t": …`, `id`, `aria_label`, `..attributes`), and `..attributes`
  on an element sets them after its own, so the caller's win; one that
  reads state follows it. A prop typed `Prop<T>` follows what it is given
  (`value: name.get()`) while the component's body still runs once; given
  a constant it holds no state. A `View` prop takes a view or text (a
  string, a number, a format string, a signal: text that reads state
  follows it), an `Option<View>` `None` too. Components may be generic
  (`List<T>`), and a handler may be async.
- air: a `View` is built again each time it is placed (a dialog opened
  again, a tab shown again, one view in two places), each place with its
  own state, as a Dioxus `Element`; it no longer fails when placed twice.
- air: roots on one thread are apart (a test's two windows): each `Root`
  has its own page, edits (`Root::take_edits`), events, globals and
  tasks; the "one Root per thread" limit is gone.
- **The kit on air** (`aio::kit` with feature `air`, air phase 2a): every
  control, as aio's AIR draws it, each body run once and its holes
  following state; the kit, facade, ui and sign-in tests run on air in
  CI, none ignored. On air only, so far (aio's, gap package WA1):
  - labels and titles take any content (Field, Checkbox, Switch, Alert,
    Modal, Confirm, ConfirmButton, EmptyState, Tab, MenuItem, Crumb,
    Table headers; `toast_view` for a toast);
  - forms: `form.bind(aio::field!(name))` wires an Input, Textarea or
    Select (its value, edits and blur: a field is touched when left),
    `set_field`, `reset_field`, `touched`, `use_field_array`, and
    `use_on_change` / `use_on_change_now` (aio's `onChange`, with a
    cleanup);
  - `Defer` loads: `load`, `loading`, `error`, `loading_min_ms`;
  - `Transition` draws no box of its own (`air::styled`: one style
    property of its content's element), `Motion::of` writes a motion as
    a function of its progress (aio's `TransitionFn`);
  - scoped CSS: `aio::ui::css(rule)` is a class named by its rule's
    hash, put in the page once; `collect_css` gives them all;
  - an element handed out by `onmounted` runs scripts on itself
    (`MountedData::run`, `eval`): a canvas, a video, a selection.
- Dark mode follows the OS: `[ui] scheme = "system"` (the default) keeps
  the sheets' dark rules as written, so the page follows
  `prefers-color-scheme` as it changes; `"light"` and `"dark"` fix one,
  and a `bool` given to `use_css!` stays the app's own choice. The
  template passes `aio::ui::scheme()`; `aio::ui::use_dark()` is a
  `Memo<bool>` for code that draws its own colours;
  `TestUi::set_os_dark` turns a test's OS dark.
- `[ui] neutral = "soft"` keeps the page's greys near grey (aio's
  `"neutral": "soft"`).
- The router on air (`router!`): `Route`, `Outlet`, `Link`, `Redirect`,
  `params()` and Back are the same views on either renderer; on air a
  `Route`'s body runs again only when its match changes (a query or
  another route's place runs nothing), and a `Link`'s active class
  follows the place in its own holes. `tests/route.rs` and
  `tests/route_tree.rs` run on both.
- The router in a browser (a web build): the address bar shows the place,
  the browser's Back and Forward step the app's history
  (`History::sync`), and a reload or a shared link opens where it points
  (`route::start_place`); the server already answers routes with the app.
- `TestUi` on air: `components()` is the tree of mounted components (name,
  key in its list, body runs, and the updates its holes made: what a
  change cost each view); `find(component, key)` and `comp=Row[42]` paths
  name a component, `within(t, f)` takes a test's paths inside one, and
  `surface_tree()` is the surface as a tree of components (aio's
  `UISurfaceNode`).
- `TestUi::advance(d)`: the views' clock moves `d` on and their timers
  due by then fire, each at its moment (aio's `ui.advance`); from then on
  it is virtual.
- `TestUi::a11y()`: aio's run-time accessibility rules (`dev-a11y.ts`)
  over the page; every mount says each new finding once.
- air: `components()` (aio DevTools' `tree`), `follow` (a body that runs
  again when what it read changes), `provide_now`, and
  `Event::default_action_enabled`.
- air's gates, each shown failing on a planted bug first: views removed
  (or replaced by an error boundary in the same pass) leave no node,
  effect, subscription, cleanup, handler, task, page script or page node;
  misuse fails in release builds too (a hook outside a view's body, a
  write while a hole or a memo computes, an effect loop, a signal used
  after its view, two rows with one key, a missing
  `key:` or an `if let` in `rsx!` at compile time); a renderer fault
  planted in air fails `TestUi` in the Rust page and the Electron page.
- **air phase 3: `ax dev`, the web build and Android on air**
  (docs/design/air.md). For an app on air `ax dev` runs no dx: an `rsx!`
  markup edit goes to the running window (`Aio.hot`) and its views are
  built again with their state kept (~3 ms); a Rust edit is built by ax
  as a thin patch against a first "fat" build (dioxus-cli 0.7.10's
  builder, ported) and applied in the running process with `subsecond`
  (~0.45 s on Linux and macOS, ~0.35 s on Windows), views keeping their
  hook state by place and, on Linux x86_64 and Windows, the patched code reading the
  running binary's statics (an atomic, a `OnceLock`); a stylesheet edit
  needs no build. The shell draws
  its windows on air with `default-features = false, features = ["air"]`
  (desktop and Android: no Dioxus in the graph), and `aio::web::launch`
  runs air in the browser. `scripts/android-gate --air` and
  `scripts/dx-metrics --live` (the live edits on Dioxus and on air,
  side by side) check and time them.
- **air phase 4: the gates in CI** (docs/design/air.md). `scripts/air-ship
  <app> [artifacts]` fails on any Dioxus crate an app ships (its
  `cargo metadata` graph for the desktop, wasm32 and Android: crates named
  so or from DioxusLabs' repositories) and on the word in any of its
  packages, byte for byte (the release binary, the `.tar.gz`, the
  AppImage's squashfs, the web build's folder, each APK member).
  `scripts/dx-metrics` measures the template on air beside the Dioxus one:
  `air_binary_b`, `air_targz_b`, `air_appimage_b`, `air_web_b` gated like
  the other sizes (5% over the base measured on the same runner, or this
  machine's best), `air_rebuild_cpu` and `air_live_css_s` /
  `air_live_rsx_s` / `air_live_logic_s` against this machine's rows only;
  each run first plants a Dioxus dependency in the air app and requires
  air-ship to fail on it. `--apk`: the air template's APKs alone
  (`air_apk_b`, the arm64 one), GitHub CI's new `air-apk` job
  ([CI](docs/build/ci.md#airs-package-gates)).
- **Transactions** (aio's `transaction: true`): `#[cell(transaction)]`
  makes each async method read the state as it was when it was called
  and commit what it wrote, ran and scheduled at its end as one event, or
  nothing; a field another call changed meanwhile refuses it with
  `Code::TxConflict` (`TX_CONFLICT`). `transaction(serialize)` runs them
  one at a time, `conflict = "warn"` commits over a conflict;
  `cx.commit()`, `cx.commit_every(ms)` and `cx.live_state::<Self>()` are
  aio's `$commit` and `$live` ([methods](docs/state/methods.md#transactions)).
- `cx.on::<Other>().after/every/at/cron(..)`: a schedule that calls another
  cell's method (aio's `schedule.after(id, ms, other.method.action())`).
- `aio::on_action(f)` (aio's `onAction`): every committed call heard;
  `Plugin::on_action` and `Plugin::allowed_origins` (aio's plugin
  `onAction`, `allowedOrigins`, merged with `[server] hosts`).
- A cell of `#[aio_cell]` fields may `retire` its own id: aio's slice of
  that name and its stamp go, its history stays.
- `Updates::retired` (and `Ready::retired`): where a fresh start set the
  data aside, for the new version's first start to say; its `backup` is
  the store set aside (aio's `updates-retire.ts`).

- **Async methods answer their caller** (aio's `await cell.method()`): a
  typed call of a `pub async fn` returns `Result<aio::Answer<R>>`, whose
  await is the value its last part returned or the refusal that ended it
  after an await. A synced copy's `Answer` and `call_confirmed` wait for
  the server's end, `ax call` prints it, and an aio page's action is
  acked on completion ([effects](docs/state/effects.md#async-methods)).
- **Awaits anywhere a statement goes** in an async method: in `if`,
  `if let`, `match`, `loop`, `while` and blocks (with `break` and
  `continue`), as `let x: T = io.await?`, `x = io.await`, `io.await`, a
  `let` bound to a branch's awaited value, or the last expression. Each
  part stays one atomic, logged call; replay does no I/O.
- `#[mode(newest)]` and `#[mode(first)]` on async methods (aio's
  `concurrency`): a new call supersedes the running one, or shares its
  answer.
- `[perf] call_ceiling` (aio's `effectTimeoutMs`); `[app] tz`.
- `aio::effect::blocking(key, f)` with a `Cancel` token and
  `cancel_blocking(key)` (aio's `blocking`, `blocking.cancel`).
- `Cells::on_start`, `on_restore`, `fatal_on_start`, `get`, `view`, and
  `AIORT_ISOLATE` (aio's `onStart`, `onRestore`, `fatalOnStart`,
  `getFullState`, `isolate`); `Plugin::on_start`, `on_stopping`, `on_stop`.
- `aio::rewrite_call` (aio's `beforeReduce` changing a call),
  `aio::on_recovered` (aio's `onCheckpointRestore`),
  `TestCells::held_effects` (aio's `t.getEffects()`).
- `aio::ui` beside the renames: `Signal::toggle`, `key_event` (a key heard
  elsewhere handed to the views), `Fragment`, `Component`; `ci.sh` fails on
  `dioxus` named in the template, the kit, `ax ui`, the tests or the guide.
- [docs/design/air.md](docs/design/air.md): air's design, the spike's
  measurements, the facade's API and what can't match air yet.
- **Persistence as aio's** (docs/persistence): `#[cell(persist(exclude =
  ["feeds.*.busy"]))]` and `persist(include = [...])`, nested paths with
  `*` (aio's `persist` filter); `[data] persist = false` and the launch
  flag `--no-persist`; `[data] coalesce_ms` (aio's `persistDebounceMs`);
  timed backups (`[data] backup_every`, `backup_keep`); aio's default size
  guard (warn over 1 MB, error over 16 MB) where `[budgets] state` declares
  none; `GET`/`POST /__aio/snapshot` in a dev build, `Handle::export_snapshot` and
  `Handle::load_snapshot` (aio's `snapshot`, `loadSnapshot`).
- `aio::columns`: typed table columns (`pk`, `text`, `integer`, `real`,
  `refs`, `boolean`, `json`, `.nullable()`, `.unique()`, defaults), for
  `#[table(…, columns = CONST)]` and `aio::columns::create`. `#[table]` rows
  hold bools (`BOOLEAN`, 0/1), lists and nested values (`JSON` text).
- `Blobs::stream(id, range)`: a blob's bytes as a reader (aio's `stream`).
- A live query wakes on writes through any `Db` of its file in the process.
- A ported app replays aio's journal past its save at boot (a crash
  before aio's save), moving `__journal_wm` with each call.

- `ax create --aio-version <v|main|latest>`, and `ax pin [app] [version]`
  (aio's `am pin`): what the app's Cargo.toml files ask for, what its
  Cargo.lock built, this ax's version and whether they match, aio-rs's
  releases, drift (exit 1), how far behind; `ax pin <v>|main|latest|<checkout>`
  moves every aio-rs dependency (workspace deps too) with `cargo update -p`
  for the aio-rs crates only, offline from cargo's cache when needed, all
  or nothing; `--major`, `--json` ([ax](docs/clients/ax.md#ax-pin)).
- `ax create` makes a git repository with its first commit (not inside
  another), a `README.md`, `.gitattributes` (`eol=lf`), a `.gitignore`
  that keeps `.env` and release keys out; `--target a,b` writes
  `[build] targets`, `--css tailwind` wires Tailwind; a failure leaves
  nothing behind (`--force`: what it wrote over is put back); `--json`
  gives the files and next steps; aio.toml no longer claims to show every
  default, it links them.
- `ax upgrade [version] [--aio dir]`: `ax` from aio-rs's newest (or that)
  release by its install script, checked against `SHA256SUMS`, or built
  from a checkout; `ax uninstall [--tools]`: `ax` (and what it fetched)
  gone, apps and data kept ([setup](docs/basics/setup.md#upgrading-ax)).
- `aio::process` (aio's `spawn` and `runCommand`): a program in a process
  group of its own (a job object on Windows), its lines as they come, its
  stdin, pause and resume, a timeout and a kill that reach the whole
  tree, killed when its `Child` is dropped and at the app's stop (named).
  On Windows children open no console window, and a release app adopts a
  hidden console at start ([desktop](docs/clients/desktop.md#running-a-program-from-the-app)).
- The window's verbs: `aio_shell::page::minimize`, `maximize`, `close`,
  `hide`, `show`; the page's `window.close()` takes the window's close
  path. macOS: opening the running app again brings a hidden window back.
- `aio_shell::clear_partition(name)` wipes a guest's session (aio's
  `clearPartition`); the app's own is refused.
- `Window { require_sandbox, show_cap_ms, keep_server, splash_show }`
  (aio's `requireSandbox`, `showWhenPainted`, `keepServer`, `splash.show`)
  and the launch flag `--keep-server`.
- `[build.splash]` look keys: `show`, `download_text`, `size`, `fit`,
  `text_position`, `align`, `margin`, `radius`, `border`, `fade_ms`,
  `font`, `font_size`, `bold`, `text_color`, `bar`, `track`. A first start's
  download draws the splash's image above its bar.
- `[build.download_window]` (aio's `electron.downloadWindow`): heading,
  text, question, colours, Cancel.
- `[build.windows] shortcut` (aio's `build.windows.shortcut`).
- `[server] local_peers` (aio's `allowLocalPeers`): `false` opens no
  local door in a release build.
- Every window app has its icon at run time, dev and installed: `aio::config!()`
  builds in `assets/icon.png` and `Window::of` wears it; a window with none
  wears the app's monogram (aio's generated icon, now `aio::icon`). It is on
  the title bar, taskbar, Alt-Tab, tray, notifications, the macOS Dock, the
  download window and a browser tab's favicon.
- `Config::hue()` and `aio::hue!()`: the app's one hue (`[ui] hue`, else its
  id's) for its page, icon, splash and download window; the template's page
  takes it.
- `desktop::icon_state()` (aio's `iconState()`).
- `ax build`: the default icon at 512 px plus an SVG (AppImage, deb); a macOS
  `.icns` built in Rust with every slot from 16 to 1024 px (no `sips`);
  `assets/icon.ico` used as is for the Windows exe; a warning for a
  non-square `icon.png`, an `icon.svg` or an `icon.png` outside `assets/`.
- **A port installs over the aio APK, its data with it**: the version
  code is aio's (`major·10⁸ + minor·10⁶ + patch`; every earlier aio-rs
  code is lower, so 0.1 APKs update too), a port (`[app] port_of`) takes
  aio's application id (`app.aio.<id>`) and, with no keystore named, the
  machine's Android debug key aio signs with, and at its first start the
  aio APK's state (`filesDir/aio-store`) becomes its `state.db` once
  (`aio::compat::data::import_android`).
- `ax test <app> --android`: the emulator lane.
- `ax build --android --debug` (debuggable, `chrome://inspect`);
  `armeabi-v7a` APKs; `[build.android] universal`; builds on macOS and
  Windows (the NDK's host folder, `.cmd`/`.bat`/`.exe` tools, the OS's SDK
  and JDK places); a JDK 17 to 23 found by its `javac`; any Gradle 8.9+
  (9 too) writes a wrapper pinned to 8.14.3.
- `ax install --android` takes the app's own APK exactly (`notes` never
  installs `notes-pro`'s) and a universal one when there is none for the
  CPU.
- Android: the label is escaped as aapt reads it (`\`, a leading `?`/`@`), an
  unfilled `{{TOKEN}}` is named, an own `MainActivity.java` is checked for
  what the template's does, and `[build.android] strict` turns an overlay
  warning into an error.
- Android downloads: an http(s) link goes to Downloads (DownloadManager), a
  `data:` one where the user picks. The phone's images show
  (`aio::ui::local_file_url`). `[build.splash]` takes `font`, `font_size`,
  `bold`, `fade_ms`.

- `ax` covers aio's `am` verbs and flags ([ax](docs/clients/ax.md#coming-from-aios-am)):
  `--json` answers objects on every verb scripts branch on (`start`,
  `stop`, `kill`, `errors`, `health`, `shot`, `doctor`, `expect`,
  `backup`, `build`), a failure's object whole; `--quiet`; `ax help` the
  everyday verbs on one screen, `--commands` all; `ax timeline` (every
  cell's events with their diffs) and `ax record` on a stopped app or
  another store, replayed by the app's own code; `ax record` calls as each
  event's caller and names the app's library; `ax testgen` (a typed name
  table for UI tests); `ax persist [--drop cell]`; `ax top [secs]`;
  `ax control` (the verbs over loopback HTTP behind a key, aio's
  `/__aio/trojan/*`); `ax lab --boot` (aio's Docker labs: Windows and
  macOS VMs, an XFCE desktop, the Android emulator with the APK
  installed; `--status`, `--reset`, `--ram`, `--cpus`, `--disk`,
  `--os-version`, `--apk`, `--port`); `errors --since 15m|reload`;
  `surface --component`, `--depth`, `--full`; `shot --json` and a
  video's `recording` event with its `t0`; `snapshot load --force` (another
  cell set refused without it); `call` `key=value` words, `--body`,
  `--wait`; `start`/`stop --no-wait`; `stop` throws a `--fresh`
  instance's data away; `cost --window 5m`; `discover --timeout`; `peer`
  of a profile's remote app and `--types`; `lint --from <version>`,
  `--tests`; `build --platforms all`; `doctor` hints (an old Electron, an
  undecided security switch) and `--harden`; `trigger` acts on the newest
  window.
- The local door answers `flush` and `drop`; a network caller may ask
  `peer` of itself (its own user and role, never a claimed one).
- **Schedules in the web build**: a page cell's `cx.after`, `every`, `at`,
  `cron`, `backoff` and its declared `#[every]`/`#[cron]` run on the page's
  timer, kept with its state in the browser's storage (a reload keeps them,
  missed ones fire once, re-armed as natively); `skip_if_running` holds.
- **`#[ttl]` answers in the web build**: a page cell answers a repeat
  within its ttl without running it, with the native answer cache.
- `ax dev <app> --web --port N --expose`: a fixed port, and the LAN over
  TLS behind a key made for the run, carried once by the printed link.
- **Patches carry a resized array's items** (aio's narrowed Immer ops): a
  push, a spread or a filter sends one `insert` per item put in or `del`
  per item taken out, not the array; a sort or a rebuild still sends it
  whole. aio pages get them as `add` and `remove`.
- **Pacing and the full-state switch** (aio's `syncIntervalMs`,
  `fullStateThreshold`): `[server.limits] sync_interval_ms` (50) folds a
  link's background changes into one patch per window, a person's call
  going at once; `full_state_percent` (50) sends the view instead of a
  bigger patch. Both on aio pages too.
- **Every shared cell on one server**: `Http::cell(handle)` serves another
  cell on the same `/ws`, key and port; a link's hello names its cell.
- **One socket for every cell** (aio's one socket per page): a process's or
  page's copies of one server's cells share one socket, each a link of its
  own on it (`x-aio-mux: 1`, the page's protocol `aio-mux`; frames
  `<link>:<message>`); a server that doesn't say yes gets a socket per copy.
  Three cells' copies: 3 connections → 1, patch latency unchanged.
- **aio pages see every shared cell**: an aio page's `state`, `patches`,
  `subs` and `<cell>:<method>` actions cover the cells `Http::cell` serves
  too, each by its own rules.
- **A whole app on one `TestServer`** (aio's `testServer({ cells })`):
  `TestServer::cell::<C>()`, `client_of::<C>(user)`, and `converged` for a
  copy of any served cell.
- `Http::resolve_peer(f)` (aio's `resolvePeer`): who a peer app on this
  machine is here, its user and role; `Http::door_links(true)` (aio's
  `localConnectHooks`): `on_link` hears the local door's links too.
- `Reply::stream(type, parts)`: a route's answer sent part by part
  (server-sent events, NDJSON), never compressed or buffered.
- **Binary side channels** (aio's binary streams): `Route::socket(path, f)`
  upgrades once `f` accepts the request; `aio::http::side` frames
  `[kind][seq:u24][payload]` with a counter per kind, drops a droppable
  frame or closes (1011) on a slow receiver, and says each frame's gap.
- `Remote::on_conflict(f)` (aio's `onConflict`): where a server change met
  a waiting call of the copy, live and in a catch-up (`Synced::conflicts`);
  `Remote::merge(field, Merge::Text | Counter | SetAdd(id))` (aio's
  `sync.merge`) shows that field merged while the call waits. In the page
  too.
- `[build.assets]` (aio's `assets`): folders beside `aio.toml` built into
  the app by URL prefix, answered at the same path in the window
  (`aio://app/…`), by the app's server (`Http::assets`, public), in the web
  build (cached offline) and in the APK; `ax build`, `ax dev` and `ax
  start` build a file added later in.
- `web` is a `[build] targets` kind: one `ax build` makes the web app
  beside the other targets (aio's `build-all`).
- A server's Linux CLI bundle gets a systemd system unit beside its user
  unit (`<bundle>.system.service`, aio's `build-service`): it starts at
  boot with no login or linger, as a user systemd makes, its data in
  `/var/lib/<id>`.
- `ax install <app> --appimage` (aio's installer): the AppImage in
  `aio-rt/install/<id>-appimage/` and the app menu, its entry making a
  private `TMPDIR` for the AppImage to unpack into; `ax installed` and
  `ax remove` know it.
- The web build makes iOS's launch images from `[build.splash]` (aio's
  `iosLaunchImages`): a Home Screen app starts on the splash.
- `ax dev --web`: the page's console (every level, uncaught errors,
  refused promises) in `ax dev`'s output (aio's console forwarding).
- A web page shows a `notify` tell as the browser's notification, asking
  for the permission at the first one and saying once when it can't;
  `aio::web::notify::request_permission()` for a click handler, `on_click`
  for its route.
- **`ax ui` as amui, on every OS**: the Tasks tab runs the project's own
  tasks (its cargo aliases) beside the `ax` verbs, its output shown as it
  comes; CPU, the project folder, the executable and the runtime kind on
  macOS and Windows too (from `health.json`'s `cpuMs`, `exe`, `cwd`);
  each linked client's health (and for how long a part is down), bytes a
  second and in all, and the changes it fell behind on; each action's
  method time and the refused ones with why; per cell its refused calls
  and last action; the runtime config card from the live app
  (`health.json`'s `config`: title, auth, mode).
- `ax events --json` of a running app says each event's method time
  (`ms`); `--refused` adds its refused calls of late. The local door
  answers `took`; a client's listing (`ax clients --json`) has `sent`,
  `behind` and its `degraded` parts.
- **`ax dev` watches as aio does** ([dev](docs/build/dev.md#the-loop)): a
  burst of saves settles after 100 ms of quiet, 500 ms at most; the files
  the last build read outside the app's folders are watched too (cargo's
  dep-info: `aio.toml`, `include_str!` files, an aio-rs checkout), in
  `--web` as well; a deaf watcher is made again (a sentinel, three times
  at most); the terminal names the changed files and times each build; a
  storm of restarts (a change within 1 s of each of 5 in a row) waits 5 s
  before each and says why; the aio checks of each saved `.rs` file are
  said after each burst (`ax lint`'s per-file ones).
- `ax dev --detach` (the session in the background, `logs/dev.out`),
  `--no-watch`, `--verbose`, `--title`; `ax stop` ends a session (the app
  stopped the graceful way), `ax status` adds `ax dev: watching` or that
  it waits for a fix; `ax errors` groups lines under each hot-patch too.
- `AIORT_LOG` (the log's level for one run, said in the boot report's
  `log` row) and `AIORT_TITLE` (the window's title for one run).
### Changed

- **A held key's step no longer waits on copying the state or on the write
  of the run before it** (the wallet's 1000 accounts: per step 6.3 → ~4.4 ms
  of the app's time; p95 of a held ↓ 32 → 20 ms). A state slow to copy
  (250 µs or more) is copied ahead on a thread of its own after each call,
  so a call starts on that copy (`clone` ~0 in `perf.log`). A commit in an
  aio file serializes only the `#[aio_cell]` slices whose field changed
  (aio's per-cell serialization cache), not the whole state: a cursor step
  writes its one small slice. A `#[coalesce]` run's write weighs the run
  (its copy, the call again, the comparisons) before it takes the store,
  so the views' calls go on meanwhile.

- GitHub CI runs `./ci.sh`'s parts side by side (`./ci.sh lint|test`
  runs one alone; no part: all): `check (lint)`, `check (test)`,
  `mutants`, and `dx (dioxus)`, `dx (air)`, `dx (apk)`
  (`scripts/dx-metrics --dioxus|--air|--apk`, each with its own base
  measurement). The base's kept row is keyed by commit and runner
  machine (`scripts/dx-metrics --machine`), so a run on a runner of the
  same machine skips the base's build. Rust caches are saved by main's
  runs only; main's runs are never cancelled by a newer push.
- aio-rs-shell's Dioxus page is its default feature `dioxus` (aio-rs
  `kit` with it); `default-features = false, features = ["air"]` is the
  shell on air. aio-rs's Dioxus page crates are its feature `liveview`
  (`web` is renderer-neutral). `use_css!` re-reads its sheets every 20 ms
  in a dev build (was 100 ms), and follows them from a hole on air.
- `ax create`'s template follows the OS's light or dark (`[ui] scheme`,
  `use_css!(aio::ui::scheme(), …)`) instead of a fixed light page;
  `page_css` and `use_css!` take a `Scheme` or, as before, a `bool`.
- A server's default size budget is 7 MB (was 6): the service template
  is 6.03 MB on Linux after 0.2's additions (sync, vitals, routes, auth,
  updates: 50 KB, no crate more than 7 KB), and the budget is that plus the
  size gate's 5% headroom, to the next whole MB.
- **Fail closed**: `/__aio/health` and `/__aio/metrics` need the key, a
  session or a user's token beyond this machine, also without accounts
  (they answered anyone before); on loopback they stay open on an app
  without accounts or users, and `[server] open` opens them. Every
  endpoint and its default: [accounts](docs/auth/README.md#every-endpoint-and-what-opens-it).
- `aio::testing::MountedComponent` has `key` and `updates`; route tests
  and the guide read `here()` where it is drawn (a hole), which follows
  the place on either renderer.
- **Breaking: `aio::testing` is the `testing` feature.** The test kit is
  a dev-dependency now, so an app's builds never compile it (the
  template's debug build 952 → 934 MB, dx-metrics' `debug_target_mb`). Fix: one line in the
  `Cargo.toml` holding the tests, `[dev-dependencies] aio = { package =
  "aio-rs", <its source>, features = ["testing"] }`; `ax create` writes it,
  and `ax lint` names tests that use `aio::testing` without it
  ([migration](docs/migration/README.md#the-test-kit-is-a-dev-dependency-in-02)).
  `video` brings `testing`; `ax lint --fix` no longer drops `testing`.
- `ax smoke` fails a `.deb` whose menu entry starts a program it doesn't
  ship or names an icon it doesn't ship, and any page script that throws
  (`ERROR [page] Uncaught …`).
- A cassette whose arguments don't serialize is refused naming the call
  and why; a tape that doesn't read names `AIO_CASSETTE=rerecord`.
- Key chords (`Chord`, `normalize_key`, `keycap_match`) are parsed in
  `aio::ui::chord`, renderer-free, so `TestUi` needs no kit; `aio::kit`
  re-exports them as before.
- **A reaction commits with its call** (aio's `listensTo` in one reduce
  and one save): a `listens_to` reaction runs inside its source call's
  commit, and its own calls go into the same SQLite transaction, so a
  crash or a refused commit leaves both or neither, and listeners and
  views see both at once. Once some cells of a file react to each other,
  that file's calls run one at a time (a file without reactions as before)
  ([persistence](docs/persistence/README.md#how-the-store-works)).
- **Breaking: `ax pin` and `ax pair` named as aio names them.** The
  pairing code is `ax pair <app> [--plain]` (aio's `am pair`), no longer
  `ax pin <app>`; `ax pair <app> <host:port> <code>` pairs a device as
  before. `ax pin` is aio's framework version (above); the unreleased
  framework verb is gone into it ([migration](docs/migration/README.md#ax-pin-and-ax-pair-in-02)).
- `ax ui` reads in its own process (no `ax` child and no `cargo
  metadata` per reading), all at once, each within 5 s; it lists the
  running apps first, finds an app by its display name, shows a refusal
  of who asked before a transport failure, and drops a process's readings
  once it stops or restarts. `ax errors` keeps a panic's lines together
  (`--json`: one string).
- **Cron schedules read UTC**, as aio's did, so a port's jobs fire when
  they did; `[app] tz = "local"` reads this machine's local time (the
  0.1.0 behaviour).
- **A `#[server]` field of a shared cell is an `aio::Hidden<T>`**: a
  synced copy never has it, and any use there panics naming the field
  (aio throws), instead of reading a default. A method that touches it is
  `#[client(predict = false)]`. Write `#[server] pub key: Hidden<String>`
  and `*self.key = k`.
- A `#[ttl]` call answered from the cache returns the kept answer, not `null`.
- An async method returns `aio::Result<R>` for any `R`.
- **Breaking: views import `aio::ui`, not `dioxus`.** `aio::ui::prelude`
  is the whole view API, renderer-neutral and shaped as air (aio-rs's own
  renderer, [design](docs/design/air.md)) provides it, so the swap to air
  needs no app change. It runs on Dioxus 0.7 until then. An app drops its
  `dioxus` dependency (`aio` with feature `kit` brings it) and moves as
  below ([migration guide](docs/migration/README.md), "0.1 → 0.2"):

  | 0.1 | 0.2 |
  |---|---|
  | `use dioxus::prelude::*;` | `use aio::ui::prelude::*;` |
  | `-> Element` | `-> View` |
  | `use_signal(\|\| v)` | `signal(\|\| v)` |
  | `count()`, `s.cloned()` | `count.get()`, `s.get()` |
  | `s.peek()` | `s.read_untracked()` (`s.get_untracked()` for a copy) |
  | `s.with_mut(f)` | `s.update(f)` |
  | `s.try_write()`, `s.try_peek()` (`Result`) | `s.try_write()`, `s.try_read_untracked()` (`Option`), `s.alive()` |
  | `s.set(v)` (always re-renders) | `s.set(v)` (re-renders only on a change; `v: PartialEq`, else `*s.write() = v`) |
  | `use_memo(f)` | `memo(f)` |
  | `use_effect(f)` | `effect(f)` |
  | `use_effect(use_reactive!(\|d\| ..))` | `watch(d, \|d\| ..)` |
  | `use_hook(f)` | `once(f)` |
  | `use_drop(f)` | `on_cleanup(f)` |
  | `use_callback(f)` | `callback(f)` |
  | `use_context_provider(f)` | `provide(f)` |
  | `use_context::<T>()`, `try_use_context`, `consume_context`, `try_consume_context` | `context::<T>()`, `try_context::<T>()` |
  | `provide_root_context(v)` | `provide_root(v)` |
  | `use_future(f)` | `task(f)` |
  | `use_resource(f)` | `resource(f)` |
  | `GlobalSignal<T>`, `Signal::global(f)` | `Global<T>`, `Global::new(f)` |
  | `GlobalMemo<T>`, `Memo::global(f)` | `GlobalMemo<T>`, `GlobalMemo::new(f)` |
  | `document::eval(js)`, `dioxus.send(v)` / `dioxus.recv()` in the script | `eval(js)`, `aio.send(v)` / `aio.recv()` |
  | `ErrorBoundary { handle_error: \|e: ErrorContext\| .. }` | `ErrorBoundary { fallback: \|e: String\| .. }` |
  | `SuspenseBoundary { fallback: \|_\| .. }` | `Suspense { fallback: rsx! { .. } }` |
  | `*part(\|c\| &c.f)` (a value) | `part(\|c\| &c.f).get()` (a handle: `.get()`, `.with(\|v\| ..)`, `"{p}"`, `.ver()`) |
  | `Runtime::try_current().is_some()` | `in_views()` |
  | `aio::dioxus` | gone |
  | `TestUi::read(&GlobalSignal<T>)` | `TestUi::read(&Global<T>)` |

  The template, the kit, `ax ui`, the tests and the guide use it; the kit's
  own hooks keep their names (`use_form`, `use_global_key`, …).
- `ax lint`: naming `dioxus` (a path, a dependency, a page script's
  `dioxus.send`) is an error (area `renderer`); the facade's hooks
  (`signal`, `memo`, `effect`, `watch`, `once`, …) are held to a view's top
  level like `use_…` ones; a `for` row without `key:` is an error (air
  requires one); an `if let` in `rsx!` warns (air refuses it).
- **An older build boots on a newer build's data** (a rollback), as aio's
  downgrade does: the fields it doesn't declare are kept in every write,
  the stamp is never lowered, an aio slice is parked under
  `__downgraded:<cell>`; only data its fields can't hold refuses the boot.
  Rolled forward again, the newer build boots and drops the fields only
  the older build declares, as aio's restore merge does.
- An aio home with a journal is no longer refused at the claim: the store
  replays it, or says why it can't.
- **`ax create` writes nothing of the machine it ran on**: the app takes
  aio-rs from git at this `ax`'s release tag (`Cargo.lock` pins the
  commit), so a clone builds anywhere and a prebuilt `ax` needs no
  checkout; `--aio <dir>` names a checkout relative to the app, refused
  through the root. `AGENTS.md` holds the rules and the loop inline and
  links the guide by URL, no local path
  ([where an app gets aio-rs from](docs/basics/setup.md#where-an-app-gets-aio-rs-from)).
- Print keeps CSS backgrounds and colours.
- The page's console reaches the log at every level (`log`/`info` as
  INFO, `debug` in a debug build).
- Linux menu entries take `%U`: "Open with" hands the files to the app.
- `AIORT_SANDBOX=1` on a host that can't sandbox now stops the app with
  the reason, as `require_sandbox` does.
- `Tell::Icon { window, tray, tooltip, fps }` (was `{ tray, frames, fps }`):
  aio's `appIcon()` in one effect; `None` leaves that part as it is.
- The hue is hashed over the slug the id makes (aio's `appHue`), so
  `My Notes`, `my_notes` and `my-notes` are one colour.
- `aio_shell::runtime::ensure_as` takes the icon of its windows and the
  splash image its download window draws.
- **`ax dev --android` installs once**: a thin dev APK beside the real app
  (`<id>.dev`, "(dev)") shows the page `ax dev --web` serves over
  `adb reverse`, and each edit reloads it; `--native` (or an app without a
  web build) rebuilds a debug dev copy for the device's CPU only. An
  emulator is booted when none is attached (`AIO_AVD`, `AIO_AVD_PORT`), a
  ready emulator is taken before a phone, `ANDROID_SERIAL` names one, and
  the app's log comes to the terminal (`[device]`).
- `ax build`'s Linux bundle says the AppImage beside it (`--appimage`,
  aio's default artifact).

### Fixed

- `ax setup` on Linux with Xephyr alone and no desktop says the nested
  display is missing (Xephyr has nothing to nest in) and how to install
  Xvfb, where it gave a ✓.
- `ax create`'s comments: an app made on a checkout (`--aio`) says it
  needs that checkout beside it, not that it builds anywhere from a tag;
  the template names `ax dev`, not dx.
- README and setup: a source install clones the release tag, whose
  templates match the tag a new app pins.
- `ax build --smoke` (and `ax smoke`) removes the record its start left
  in the shared runtime's app list (`apps/*.json`), on Linux, macOS and
  Windows, and on macOS the window bundle no other app names: a smoke
  leaves nothing behind for `ax prune` to show.
- **An `rsx!` hole may return a borrow of what it reads again**: `{t(&x)}`
  or `{t(&r.name)}` (a `&str` of the hole's own clone of `x`) failed with
  E0716 "temporary value dropped while borrowed" since holes clone their
  names per run; the clones now live until the value is converted.
- **On air, a handler's work is its component's** (Dioxus's scope): a task
  an `async` click handler spawned ended when the `if` branch or `for`
  row that drew its button was drawn again; it now ends with the
  component. An event the page sent for an element drawn again before it
  arrived is still dropped (AIR never runs a removed element's handler);
  a dev build says so once per event name on the diagnostic bus
  (`ui:stale-event`, `air::on_warn`). `TestPage::aim` makes such an event.
- air: every `exec` of one batch runs (an island's update, a second
  island's mount): on a page without `__aioRun` all but the first found
  the slot the first had freed, and did nothing.
- air: `aria-*`, `data-*` and HTML's enumerated attributes take
  `true`/`false` as text (`aria-selected="false"`), as ARIA reads them;
  an `Option`'s `None` writes no attribute at all.
- air: a constant `dangerous_inner_html` is set as the `innerHTML`
  property at mount (the page's policy sees it), not as an attribute.
- air: a hole may take what it reads by value (`{match who { .. }}`,
  `{note.map(..)}`): its names are cloned in per run, as a component's
  props.
- air: attributes given to a component that takes none say so (the
  `GlobalAttributes` message), not "no method `__attr_const`".
- `kit::Browser`'s `src` follows its caller's state (a `Prop`): a changed
  `src` navigates the guest, on air as it did on Dioxus.
- `TestUi::html` leaves out air's anchor comments.
- A reply that sets the key cookie (a `?token=` link's page) is
  `cache-control: no-store` with no `etag`; on air the shell's page was
  otherwise served revalidatable (no server paint makes it `no-store`).
- `ax dev` on Windows reloads an `rsx!` edit as markup again (~ms)
  instead of hot-patching it (~1.5 s): the watcher named a changed file
  with the `\\?\` prefix `canonicalize` adds, so it never matched the
  build's snapshot; it names it as the snapshot does now. Found by
  `dx-metrics --live` on the Windows runner (live_rsx_s 1.5 s, its line
  "hot-patched in 1316 ms").
- A backtrace in hot-patched code on Windows (MSVC) names its functions:
  each patch DLL is linked with its PDB (`/DEBUG` beside `/DEBUG:SYMTAB`)
  and std's resolver (dbghelp) is told the modules again after each
  patch, under its own lock. Linux, macOS and MinGW name a patch's frames
  only when it loaded before the first backtrace was resolved (std lists
  the libraries once); the hotpatch test checks both, on each OS.
- `scripts/dx-metrics --live` runs on Windows (it imported the Unix-only
  `resource`): CPU and the build check through the Win32 API, `ax dev`'s
  tree stopped with `taskkill`, edits written byte for byte; the Windows
  CI job prints its live-edit times.
- The splash's `text` takes aio's full set on every splash (Electron's,
  the OS's, the web's and Android's): `{title}`, `{version}` (the app's,
  what `--version` prints), and `{got}`, `{total}`, `{pct}` as `0`, `?`,
  `?`; Electron's, the web's and Android's filled only `{title}`. A
  `download_text`'s `{version}` is the app's too, as aio's, not Electron's.
- A cell's breaker never lets a caller see it enabled past its trip: the
  schedule thread's count could take the failure that tripped it while the
  caller's own count found none and returned before the cell was disabled
  (`a_trip_keeps_saved_data_through_enable_and_restart`). One count at a
  time per cell now.
- A synced client's call the server refused stays refused when it is sent
  again (its answer went with a cut link): it was applied on the later
  state its client was never told of.
- `TestUi::html` on air leaves out air's anchors (the empty comments where
  a hole's content goes), so a test reads the markup a person sees.
- air: Dioxus's style attributes (`display: "flex"`, `align_items`,
  `min_width`, …, dioxus-html's list) set their CSS property on an HTML
  element, as on Dioxus; they were attributes the page ignored. A `style`
  string keeps the properties set before it that it doesn't set
  (`position: "fixed", style: "inset: 0"` is both), as on Dioxus; it
  replaced them.
- air: a task spawned with `spawn_forever` in an app ends with its root (a
  test's next mount on the thread polled it, on a dead tokio runtime: a
  panic); `follow_sheets` starts its watcher again for the next root.
- air: a page script that returned still hands `recv` what its listeners
  and timers send later, as on Dioxus; its return was an error to `recv`.
- Enter in an open combobox (`role="combobox"`, `aria-expanded="true"`)
  picks, never submits its form: aio's Enter submitted it once the pick
  enabled the form's button (the feedback form's country sent it).
- air: a view's `on_cleanup` reads its own signals and memos (they were
  freed first: a panic in a destructor, the process aborted).
- air: SVG drawn by a component, a `for` row or an `if` branch (a template
  whose root is `circle`, `g`, `path`, …, or one made inside an `svg` of
  the same `rsx!`) is in SVG's namespace; it was an HTML element, drawn
  as nothing.
- air: an `rsx!` value that is a constant or a static alone
  (`d: CLOUD`) compiles; it was rebound as a local (a refutable pattern).
- air's code size: an event handler costs its closure alone (its wrappers
  were generic over it, 36% of the bench's IR): 50 views make 1.13 M lines
  of LLVM IR, 0.31x Dioxus's (phase 1: 0.49x), a held key over 10 000
  rows answers in 0.21x; `scripts/air-ir` in `ci.sh` fails past 25 000
  lines a view (docs/design/air.md).
- `ax lint`: the `testing` check no longer errs on a workspace-inherited
  aio-rs: `[dev-dependencies] aio = { workspace = true, features = ["testing"] }`,
  or `features = ["testing"]` on the `[workspace.dependencies]` entry, renamed
  (`package = "aio-rs"`) or not; dotted and `[target.….dev-dependencies]`
  tables count too.
- A Unix socket path past the OS limit (108 bytes with the NUL on Linux, 104
  on macOS): the app's socket dir falls back to `/tmp/aio-rt-<uid>/` when
  `XDG_RUNTIME_DIR`/`TMPDIR` is too long, as aio's `resolveSocketPath` fell
  back to `/tmp/aio`, and a path still too long is refused at once in words
  (the path, its length, the limit, the fix) by the app, the shell (before
  Electron connects) and `ax`, instead of EINVAL and no window.
- A cell's or a synced copy's listeners (a view, `watch`) never hear an
  older state after a newer one: a tell from another thread, or from a
  listener itself, between reading the state and telling it, was told
  first, and the older one after it (a watch on a copy saw `[0, 4, 0, 5]`).
  One teller at a time now tells until nothing is left untold.
- An app's first update after its install is made from the patch again:
  a bundle's `install.sh` kept only the binary in `versions/<v>/`, so the
  patch (made over the whole bundle) missed its other files and the whole
  release was downloaded. It now installs the bundle whole, as an update
  stages one.
- On macOS and Windows the first update after an install is made from the
  patch too: it starts from the installed `.app` or `Programs\<id>` when
  no staged copy of the running version is kept, as there is none until
  the first update staged one.
- On macOS, Windows and from an AppImage, a new version whose binary dies
  as it starts no longer leaves no version running: its binary is the
  install helper, so the old app exited for a helper that never ran. The
  old app now exits only once the helper says it is up (20 s); else it
  runs on, and that release is never offered again.
- `ax release` of the service template on Windows: its default size
  budget is half again there (a server 10 MB, a desktop app 15 MB), as its
  build is about a third bigger (7.1 MB against 5.3 on Linux).
- `scripts/dx-metrics --check` compares like with like: every gated
  number, sizes and crate counts too, is judged against the same machine's
  rows only (a CI runner's debug target was failing against the
  developer machine's best: debug info holds absolute paths). `--base
  <rev>` measures the base commit's sizes in a checkout beside this one,
  and GitHub CI gates against its PR's base measured on the same runner
  (`AIO_DX_BASE`, rows kept per commit in `AIO_DX_ROWS` and cached); its
  times are said, not gated. Accepted growth stays a committed row of the
  same machine.

- `ax smoke <app>_<version>_<arch>.deb`: a Debian package's own name is
  read for its app id.
- The app quitting (it stops, its socket closes) right after the page
  vetoed a close (`window.close()` with a beforeunload handler) quits: the
  vetoed close, still settling, took the quit's own close of that window
  with it and the runtime stayed up; the windows left are closed again
  until they are gone.
- The desktop splash shows a second after its page loaded when its window
  never paints while hidden (a loaded machine): it stayed hidden for good;
  a splash already gone never comes back.
- A copy (`Remote`) hears this process's diagnostic bus from before it is
  online: a problem said just as the link came up was never relayed to the
  server's bus.
- `aio::testing::TestUi` counts every render however its tests share the
  process: with one tracing listener alive, tracing-core asks only the
  registering thread's own, so a render first met on a thread with none
  muted it for all; a second listener is kept for good.
- `aio::testing::TestBrowser` waits up to 120 s for a browser's DevTools
  port (a hang guard): the first system Chrome of a Windows CI runner took
  more than 30 s while running.
- An app that isn't running or has no server, asked for a pairing code or
  its profile, is told so plainly with what to type ("notes isn't running:
  start it with `ax start notes`"; no server: the aio.toml lines that
  give it one), not "the app isn't serving (no serving.json)".
- `ax dev <app> --web` with any other flag ran the desktop app; it now
  takes `--port` and `--expose` in any order and refuses other flags.
- The desktop window's page policy lets WebAssembly compile
  (`'wasm-unsafe-eval'`, as aio's `script-src`; still no `eval`).
- A clean stop leaves no socket behind: a cell's door served on the
  runtime was never dropped, so its `cell-<id>.sock` (and its folder)
  outlived the app.
- `ax ui` lists the clients of every cell, not the first 8; the Overview's
  tiles wrap instead of cutting their value ("runni…").
- A page script's `await aio.recv()` (`aio::ui::eval`) no longer hangs the
  window when the view sends later than 50 ms after it asked, and takes a
  falsy value (`0`, `false`): the renderer's own `recv` waited in a busy
  loop the message could never get past.
- `ax dev` restarts ask the app to stop over its door first (calls in
  flight finish, effects commit; Windows too, which has no SIGTERM), dx's
  restarts on a `Cargo.toml` change included; a build that left the binary
  as it was no longer restarts the app.
- `ax dev`: editor files (vim's `4913`, `#x#`, `*.tmp`, safe-write copies)
  no longer start builds; an app inside some `target` folder is heard.
- `ax dev --watch=<dirs>` narrowed nothing under dx: ax now rebuilds and
  restarts for it (dx watches all of the app).
- `ax dev` without dx: a good build after a failed one says `fixed` and
  takes the errors off the page even when the app keeps running.
- `ax dev`'s session and dx pid files hold pid and start time: a pid the
  OS handed out again is no longer taken for the session, on every OS,
  and a stray dx is stopped on Windows and macOS too.
- `ax dev` app args with spaces reach the app intact under dx (dx splits
  its `--args`; they go in `AIORT_DEV_ARGS`).
- `scripts/dx-metrics`: the live-edit times no longer include the page
  poller's start or the previous edit's patch build under load (each
  save waits until the loop is quiet).
- A signed (Authenticode) one-click Windows download finds its runtime:
  the mark is looked for before the certificate table.
- **Android: a crash at start on Android 10.** The activity's insets
  called API-30 methods with minSdk 29; Android 10 now takes the older
  path. A WebView too old for the page (Android 10's stock 74) says so on
  the screen instead of a blank page.
- **The dev page's scripts ran into each other** (`ax dev --web`,
  and so `--android`): the splash script's `(() => …)()` called the live
  reload's on its result, an error on every dev page.
- `aio::testing::TestBrowser` starts one browser at a time per process
  (started together, eight took 22 s each on an 8-core Windows, one 0.5 s)
  and reads its DevTools port only once `DevToolsActivePort` is whole.
- The app's scripts in the page (Dioxus queries: an island's mount and
  unmount, a focus, a rect) run in the order the app sent them; a big one
  could run after a small one sent later, so two `<Browser>`s could leave
  one empty and an island swapped for another could stay the old one.
- The test suite holds under load: timing tests wait on what they check
  (a condition, a gate the test opens, a page-side record of each style)
  instead of a fixed sleep or a wall-clock window; bounds on time are hang
  guards well short of what they guard against.

## [0.1.0] - 2026-11-01

The first release of aio-rs (alpha): a Rust framework for apps whose state
is cells with an event log, and its command `ax`. It is the Rust
counterpart of aio, the Deno-based framework (command `am`): a port runs
on the aio app's data (`state.db`) as it is, aio can still open it, and
aio pages and tools talk to it, so an aio app can be ported one piece at
a time ([migration guide](docs/migration/README.md)).

### Added

- **Cells** (`#[aio::cell]`, `#[aio::methods]`): state changed only by
  its methods, each call an event in a log; replay (`ax replay`), time
  travel (`ax timetravel`), derived values, invariants, state machines,
  many cells per app, versioned migrations
  ([cells](docs/state/cells.md)).
- **Persistence**: an SQLite store with snapshots and the log (folded
  past `[history] keep`), coalesced
  writes (`#[coalesce]`, `#[cell(coalesce)]`), backups, blobs, SQL,
  merging offline edits ([persistence](docs/persistence/README.md)).
- **Effects**: async methods, effects and their modes, schedules,
  notifications, reactions ([effects](docs/state/effects.md)).
- **Sync**: servers and copies over TLS (a machine root for the LAN, the
  operator's certificate for a public server), PAKE pairing, LAN
  discovery, `#[client]` rules, `visible`, server functions, routes,
  peers, plugins ([sync](docs/state/sync.md)); accounts, TOTP, OIDC and
  secrets ([accounts](docs/auth/README.md)).
- **Views**: Dioxus views drawn by a pinned Electron runtime, kept in
  the same shared store as aio's (one download for both, by aio's lock,
  completeness and use-record rules, SHA-256 pinned);
  `client!` with field-level reads (`part`), the kit (components, keys,
  forms, windowed lists, dialogs), themes, routes
  ([views](docs/ui/views.md), [kit](docs/ui/kit.md),
  [performance](docs/performance.md)).
- **Targets**: one app as a desktop window (Linux, Windows, macOS), in a
  browser (wasm, installable as a PWA), on Android, as a headless service
  or as a command-line tool (`aio::cli`).
- **Testing**: `TestCells`, `TestUi` (the real views in the runtime,
  headless), servers, sync simulation, cassettes
  ([testing](docs/testing/README.md)).
- **`ax`**: `create`, `dev` (hot-patch, live CSS), `test`, `build`
  (tar.gz, .deb, AppImage, Windows installer, macOS .app, APK, web),
  `release` (signed releases, delta updates, a GitHub workflow), `setup`,
  `doctor`, logs, reports and the running app's state, `ax ui` (the
  visual app manager, aio's amui screen for screen)
  ([ax](docs/clients/ax.md), [debugging](docs/debugging/README.md));
  `ax agent` is the brief for coding agents ([agents](docs/agents.md)).
- **Updates**: signed, gated on data compatibility, rolled back by an
  external helper when a new version fails to start
  ([build](docs/build/README.md)).
- **Working with aio** (`aio::compat`): a port's store in aio's layout
  (`[app] port_of`, `Store::open_aio`, `#[aio_cell]` fields); with feature
  `compat`, aio pages and tools on the app's socket, `AioLink` to call an
  aio app, and `#[on_aio]` to settle what aio changed
  ([data](docs/migration/data.md), [wire](docs/migration/wire.md)).
- **Install**: a prebuilt `ax` for Linux x86_64 and Windows x86_64, each
  archive's SHA-256 pinned in `install.sh` / `install.ps1`; or from
  source, on macOS too ([setup](docs/basics/setup.md)).

### Known limits

- Apps build on an aio-rs source checkout (the crates are not on
  crates.io yet): a prebuilt `ax` needs `ax create --aio <checkout>`.
- No prebuilt `ax` for macOS yet: build it from source.
- No iOS client yet.
- The prebuilt archives and app bundles are not OS-signed yet: Windows
  SmartScreen and macOS Gatekeeper warn on first start.

[0.4.1]: https://github.com/riagentic/aio-rs/releases/tag/v0.4.1
[0.4.0]: https://github.com/riagentic/aio-rs/releases/tag/v0.4.0
[0.3.0]: https://github.com/riagentic/aio-rs/releases/tag/v0.3.0
[0.2.0]: https://github.com/riagentic/aio-rs/releases/tag/v0.2.0
[0.1.0]: https://github.com/riagentic/aio-rs/releases/tag/v0.1.0

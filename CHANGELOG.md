# Changelog

All notable changes to aio-rs are written here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions
follow [Semantic Versioning](https://semver.org/): before 1.0.0 a minor
version may break things.

## [Unreleased]

### Added

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

- `ax create --aio-version <v|main|latest>`, and `ax framework [version]
  [--aio dir]`: the app's aio-rs said, or moved with `Cargo.lock` (aio's
  `am pin`; all or nothing).
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
### Changed

- **Fail closed**: `/__aio/health` and `/__aio/metrics` need the key, a
  session or a user's token beyond this machine, also without accounts
  (they answered anyone before); on loopback they stay open on an app
  without accounts or users, and `[server] open` opens them. Every
  endpoint and its default: [accounts](docs/auth/README.md#every-endpoint-and-what-opens-it).
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

### Fixed

- A page script's `await aio.recv()` (`aio::ui::eval`) no longer hangs the
  window when the view sends later than 50 ms after it asked, and takes a
  falsy value (`0`, `false`): the renderer's own `recv` waited in a busy
  loop the message could never get past.
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
- `aio::testing::TestUi` counts every render (`components()`) when tests
  run in parallel: a render callsite another thread registered at the same
  moment could be cached as of no interest, and its renders went uncounted.
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

[0.1.0]: https://github.com/riagentic/aio-rs/releases/tag/v0.1.0

# Changelog

All notable changes to aio-rs are written here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions
follow [Semantic Versioning](https://semver.org/): before 1.0.0 a minor
version may break things.

## [Unreleased]

### Added

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
- `Tell::Icon { window, tray, tooltip, fps }` (was `{ tray, frames, fps }`):
  aio's `appIcon()` in one effect; `None` leaves that part as it is.
- The hue is hashed over the slug the id makes (aio's `appHue`), so
  `My Notes`, `my_notes` and `my-notes` are one colour.
- `aio_shell::runtime::ensure_as` takes the icon of its windows.
- **`ax dev --android` installs once**: a thin dev APK beside the real app
  (`<id>.dev`, "(dev)") shows the page `ax dev --web` serves over
  `adb reverse`, and each edit reloads it; `--native` (or an app without a
  web build) rebuilds a debug dev copy for the device's CPU only. An
  emulator is booted when none is attached (`AIO_AVD`, `AIO_AVD_PORT`), a
  ready emulator is taken before a phone, `ANDROID_SERIAL` names one, and
  the app's log comes to the terminal (`[device]`).

### Fixed

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

[0.1.0]: https://github.com/riagentic/aio-rs/releases/tag/v0.1.0

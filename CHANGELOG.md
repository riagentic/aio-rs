# Changelog

All notable changes to aio-rs are written here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions
follow [Semantic Versioning](https://semver.org/): before 1.0.0 a minor
version may break things.

## [Unreleased]

### Added

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
  (~0.45 s on Linux and macOS; Windows rebuilds), views keeping their
  hook state by place; a stylesheet edit needs no build. The shell draws
  its windows on air with `default-features = false, features = ["air"]`
  (desktop and Android: no Dioxus in the graph), and `aio::web::launch`
  runs air in the browser. `scripts/android-gate --air` and
  `scripts/dx-metrics --live` (the live edits on Dioxus and on air,
  side by side) check and time them.
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

- air's code size: an event handler costs its closure alone (its wrappers
  were generic over it, 36% of the bench's IR): 50 views make 1.13 M lines
  of LLVM IR, 0.31x Dioxus's (phase 1: 0.49x), a held key over 10 000
  rows answers in 0.21x; `scripts/air-ir` in `ci.sh` fails past 25 000
  lines a view (docs/design/air.md).
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

[0.1.0]: https://github.com/riagentic/aio-rs/releases/tag/v0.1.0

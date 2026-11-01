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
- `aio::ui` beside the renames: `Signal::toggle`, `key_event` (a key heard
  elsewhere handed to the views), `Fragment`, `Component`; `ci.sh` fails on
  `dioxus` named in the template, the kit, `ax ui`, the tests or the guide.
- [docs/design/air.md](docs/design/air.md): air's design, the spike's
  measurements, the facade's API and what can't match air yet.
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

### Fixed

- A page script's `await aio.recv()` (`aio::ui::eval`) no longer hangs the
  window when the view sends later than 50 ms after it asked, and takes a
  falsy value (`0`, `false`): the renderer's own `recv` waited in a busy
  loop the message could never get past.

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

# Changelog

All notable changes to aio-rs are written here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions
follow [Semantic Versioning](https://semver.org/): before 1.0.0 a minor
version may break things.

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

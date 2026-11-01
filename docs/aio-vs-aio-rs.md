# aio and aio-rs compared

aio is the Deno-based framework (TypeScript, command `am`); aio-rs is its
Rust counterpart (command `ax`), with the same app model: cells, methods,
effects, views, one app on many targets. This page says where each is
ahead, what works between them, how aio-rs behaves differently, and
which of aio's features aio-rs has. Moving an app over:
[the migration guide](migration/README.md).

## At a glance

| | aio | aio-rs |
|---|---|---|
| Language, runtime | TypeScript on Deno | Rust, native code |
| Renderer | AIR (React-shaped, fine-grained signals) | `aio::ui` views (`rsx!`), drawn by Dioxus 0.7 until air; the window is a shared, pinned Electron runtime (desktop), the system WebView (Android), wasm in a browser |
| State model | cells, methods on an Immer draft, selectors | cells, methods on `&mut self` (a copy), `#[derived]`; every call an event in the store's log |
| Data compatibility | `state.db` | the same `state.db`, both ways: a port opens aio's file as it is, and aio opens it after ([below](#data-the-same-statedb)) |
| Wire compatibility | protocol 3 on `/ws`, NDJSON on a local socket | its own wire 2; with feature `compat` also aio's protocol 3 on `/ws`; pairing, discovery, peers and blobs in aio's formats always ([below](#the-wire)) |
| Electron store | per user, `apps/` records, `am prune` | the same store, the same rules: one copy per version for both ([below](#the-electron-store)) |
| Targets | browser, electron, android, web, cli, server, server-app; thin clients electron-client, android-client, ios-client, cli-client | desktop (Linux, Windows, macOS), web (wasm, PWA), Android, service, CLI; a thin client is `[app] mode = "client"`; no iOS ([targets](build/targets.md)) |
| Install | `install.sh`: installs Deno, clones aio, puts `am` on `PATH` | a prebuilt `ax` (Linux x86_64, Windows x86_64) or from source; apps build on an aio-rs checkout ([setup](basics/setup.md)) |
| Dev loop | `am dev`: `App.tsx` patched, other edits reload the page, a cell edit restarts the app (~3 s) | `ax dev`: Rust hot-patched with the state kept, `rsx!` markup at once, CSS live; a rebuild and restart when a patch can't take it ([dev](build/dev.md)) |
| Tests | `testCell`, `testUI` (happy-dom), `testServer`, cassettes, `deno test` | `TestCells`, `TestUi` (the real views in the runtime), `TestServer`, cassettes, `cargo test` ([testing](testing/README.md)) |
| Maturity | beta (1.0.27-beta): the 1.x surface frozen, additions only, an API snapshot gate in CI | early alpha, first release (0.1.0); a minor version may break ([versions](basics/versioning.md)) |

## App compatibility

An aio app and its port (the aio-rs app made from it) share data and
talk to each other, so an app can move one piece at a time.

### Data: the same state.db

- **Where.** `[app] port_of = "<the aio app's appId>"` makes the port
  resolve the home exactly as aio does (`--home` > `--profile` >
  `$AIO_APPS_DIR/<appId>` > `~/.<appId>`, plus the app's own `[home]`
  rules) and open `<home>/data/state.db`, or `[data] store` (aio's
  `dbPath`) ([data](migration/data.md#where-the-data-lives)).
- **What it reads.** aio's snapshot in `aio_kv`, in either layout (`single`:
  one `state` row; `multi`: a row per cell), each cell at its
  `<appId>:__versions` stamp. An older stamp migrates (after a backup); a
  newer one refuses the boot and keeps the file.
- **What it writes.** On every commit, in the same transaction as its own
  log: the cell's row in aio's layout with `JSON.stringify` semantics,
  without `#[transient]` fields, keeping unknown cells and `__*` keys, and
  stamping `__schema`, `__persistKey` and `__versions`. Its own history
  goes in tables of its own (`log`, `snap`, `floor`, `sched`, `ops`),
  which aio ignores.
- **Tables.** aio's `db:` tables are `#[table("name")]` fields (written in
  aio's shapes, marked in `aio_synced_tables`); the app's own tables stay
  plain SQL through `aio::db::Db` on the same file
  ([SQL](persistence/sql.md)).
- **Several aio cells in one.** `#[aio_cell] settings: Settings` makes a
  field one aio cell's whole slice, read and migrated at its own stamp.
- **What aio changed.** A change aio made since the port last ran comes in
  as one `$rewind` event; the cell's `#[on_aio]` method (feature `compat`)
  settles it before the invariants check it.
- **Locks.** Both take `<home>/.aio-instance.lock` (an exclusive OS lock),
  so aio and the port never run on the same data at once: the second one
  to start finds the home held. aio-rs keeps `meta.json`'s `appId` and
  `profile`.
- **Not ported:** aio's `profiles: false` and the `__shapes` stamps of
  aio's dev-mode shape check. An aio journal not yet folded into the
  snapshot is replayed by aio only: start the aio app once.

### The wire

- **aio pages, `am` and aio tools → an aio-rs app.** A served aio-rs app
  built with feature `compat` speaks aio's protocol 3 on `/ws`: state,
  patches, subscriptions, calls and acks, and aio's offline sync (`op`,
  `sync-req`), an op taken as a call that is never run twice
  ([wire](migration/wire.md#aio-rss-feature-compat)). Without `compat`,
  a client that doesn't send `x-aio-wire: 2` gets HTTP 426.
- **An aio-rs app → an aio app.** `aio::compat::AioLink` reads an aio
  server's cells, follows its patches, makes `<cell>:<method>` calls and
  runs its server functions; nothing is predicted
  ([plugins](state/plugins.md#aio-apps)).
- **Always in aio's formats** (no feature): LAN discovery (UDP 8099),
  pairing (plain PIN and CPace), the same-machine `x-aio-peer` proof, and
  blobs (`/__aio/blobs`).

### The Electron store

aio-rs's windows start from aio's shared Electron store
(`~/.local/share/aio/electron`, `~/Library/Application Support/aio/electron`,
`%LOCALAPPDATA%\aio\electron`, or `AIO_SHARED_ELECTRON_DIR`), kept by aio's
rules: the same `.aio-complete` entries, the same download lock, the same
`apps/` records and the same pinned SHA-256 check. An aio app and an aio-rs
app on one Electron share one copy, and `am prune` keeps what an aio-rs app
records as `ax prune` keeps what an aio app does
([desktop](clients/desktop.md#one-store-with-aio)).

### What does not interoperate

- aio's local socket and its NDJSON framing: aio-rs's local door is its
  own (length-prefixed frames), so `am` reaches an aio-rs app over `/ws`
  only, on a served app built with `compat`.
- Lock records: aio-rs writes none of aio's `<appId>.lock` files, so `am
  instances` doesn't list a running aio-rs app (`ax apps` does).
- A desktop-only (`standalone`) aio-rs app has no port at all, as an aio
  desktop app has none.
- aio's routes over its local socket (`aio://`): an aio-rs app serves
  routes only with `[server] listen`.
- Code: an aio cell or component is translated, not loaded.

## Where aio-rs is better

### Measured

A large desktop app (about 74 000 lines, lists of up to 1000 rows), in
aio and in aio-rs, measured on one machine; key-to-paint in ms (p50 /
p95 where two), aio-rs as a release build:

| | aio | aio-rs |
|---|---|---|
| Hold ↓, 19 rows | 25.4 / 27.3 | **16 / 17.7** |
| Hold ↓, 121 rows | 25.6 / 38.7 | **22.2 / 24.5** |
| Hold ↓, 1000 rows | 32.6 / 85.5 | **20.6 / 23.6** |
| End, 1000 rows | 167.5 | **17.1** |
| Home, 1000 rows | 183.5 | **23.2** |
| Tab | 29–38 | **12–22** |
| A dialog opens | 25 | **11.5** |
| A page opens | 47 | **18.6** |
| Memory (PSS) | 1509 MB | **420 MB** |
| Start to the first screen | 3.0–3.2 s | **0.54–1.1 s** |

After a held key is released, aio-rs shows the last step at once: 0 ms
of lag.

### Not in numbers

- **Types end to end.** Cell fields, method arguments, props and routes
  are checked by the compiler: what aio's `aiol` lints (types, imports,
  the client/server split, state shapes, dropped effects) is a compile
  error ([lint](testing/lint.md#what-aio-has-that-aio-rs-does-without)),
  and `aio.toml` is checked as the app builds, where aio checks its
  options at `aio.run()`.
- **Memory safety, no garbage collector.** No V8 heap to size or
  snapshot; memory is the process's resident set
  ([production](debugging/production.md)).
- **Durability.** A call returns once its event is committed; aio writes
  its snapshot 100 ms later. Only `#[coalesce]` methods are held
  ([data](migration/data.md#durability-what-each-saves-when)).
- **History.** Time travel reads the store: in a release build, over the
  whole history, after a restart; aio's is dev-only, 2000 entries in
  memory ([time travel](debugging/time-travel.md)).
- **An offline queue that survives a restart** (`connect_saved`); aio's
  lives in the page's memory ([sync](state/sync.md)).
- **Real UI tests.** `TestUi` runs the real views in the real engine:
  rects, styles, screenshots and scrolling measure for real, where aio's
  happy-dom has no layout ([UI tests](testing/ui.md)).
- **Hot-patching with the state kept** for Rust, views and methods
  alike, and a compile error shown on the window while the app keeps
  running ([dev](build/dev.md)).
- **Smaller first steps on an existing machine.** One native binary per
  app (5.3 MB for the template), the window runtime shared by every app.

## Where aio is better

### The edit loop

aio restarts an app from source in ~3 s whatever its size. aio-rs
recompiles the crate an edit is in:

| Edit | aio-rs |
|---|---|
| a small crate (the template, a few thousand lines) | 0.6–2 s, hot-patched |
| a crate of about 74 000 lines | ~10 s |
| a first (cold) build | 15 s (template) to ~60 s (big app) |
| that app split into crates: a page edit | ~1.9 s |
| … an edit in its shared kit or core | 8–14 s |

So a big aio-rs app is kept fast to edit by splitting it into crates
([what an edit costs](build/README.md#what-an-edit-costs)). The web build
and Android have no hot-patching: each edit is a build.

### Feature gaps

Every gap the guide names against aio, by area. ➖ = no workaround needed
beyond the aio-rs form.

| Area | aio | aio-rs | Workaround | Page |
|---|---|---|---|---|
| State | `aio.run({ onStart })`, an app-wide start hook | none | `#[on_start]` on a cell | [lifecycle](state/lifecycle.md#differences-from-aio) |
| State | `fatalOnStart: true` | none: a refused `#[on_start]` is reported, the boot goes on | check in `main` before `run` | [lifecycle](state/lifecycle.md#differences-from-aio) |
| State | `isolate` (run only some cells) | none | open only those cells (`TestCells`) | [lifecycle](state/lifecycle.md#differences-from-aio) |
| State | `app.getFullState()` in a hook | none: a hook sees its own cell | pass what it needs | [lifecycle](state/lifecycle.md#differences-from-aio) |
| State | `beforeReduce` may change an action | `before_call` lets in or refuses | change it in the method | [lifecycle](state/lifecycle.md#differences-from-aio) |
| State | `beforeReduce`/`onAction` on every call, `am where` | no module-graph answer of where code runs | the compiler and `cfg(target_arch = "wasm32")` | [architecture](basics/architecture.md#gaps-against-aio) |
| Methods | `transaction: true`, `$commit`, `$live`, transactions across cells | none: each part of an async method is atomic | take what you need before the await | [methods](state/methods.md#differences-from-aio) |
| Methods | `cellDefaults`, `persist: false` app-wide | none: flags per cell | `#[cell(persist)]` where wanted | [persistence](persistence/README.md#limits-compared-with-aio) |
| Effects | `blocking.cancel(id)` stops a task | `spawn_blocking` can't be stopped | check a flag between chunks | [effects](state/effects.md#limits) |
| Effects | `t.getEffects()`, effect objects in tests | `t.effects()` lists keys | `t.settle()` runs them | [effects](state/effects.md#limits) |
| Effects | notifications in a browser page (`requestNotificationPermission`) | none in a web build | `aio::tell::show_with(f)` | [effects](state/effects.md#limits) |
| Persistence | `persist: { include, exclude }` path lists | none | `#[transient]`, `save = f` | [persistence](persistence/README.md#limits-compared-with-aio) |
| Persistence | `persistMode: "single" \| "multi"` | log + snapshots per cell; aio's layout read and written as found | ➖ | [persistence](persistence/README.md#limits-compared-with-aio) |
| Persistence | state export and import over HTTP | none | `ax snapshot save`, `ax snapshot load` | [persistence](persistence/README.md#limits-compared-with-aio) |
| Persistence | timed backups, a WAL-checkpoint setting | none | the OS's scheduler | [files](persistence/files.md#limits-compared-with-aio) |
| Persistence | `launch.json`, a restart with the last flags | none | pass the flags again | [files](persistence/files.md#limits-compared-with-aio) |
| Persistence | typed schema builder (`ColumnOpts`) | `columns = "…"` in SQL, no query builder | plain SQL | [SQL](persistence/sql.md#limits-compared-with-aio) |
| Persistence | a size guard (warn 1 MB, error 16 MB) | only against `[budgets] state` | declare `[budgets] state` | [SQL](persistence/sql.md#limits-compared-with-aio) |
| Persistence | `bool`, lists, nested values in table rows | flat rows | numbers or strings | [SQL](persistence/sql.md#limits-compared-with-aio) |
| Persistence | — | blob reads by range or file, no async stream | `read`, `path` | [SQL](persistence/sql.md#limits-compared-with-aio) |
| Sync | merge strategies per field (`lww`, `counter`, `set-add`, `text`…), HLC | methods replayed in order on the server | methods; `aio::merge::text3` for prose | [offline](persistence/offline.md#limits-compared-with-aio) |
| Sync | peer-to-peer, server-less sync | a copy needs a server | a server | [offline](persistence/offline.md#limits-compared-with-aio) |
| Sync | Immer add/remove ops for arrays | a `Vec` re-sent whole when its length changes | a map for a growing collection | [offline](persistence/offline.md#limits-compared-with-aio) |
| Sync | `fullStateThreshold`, `syncIntervalMs`, staleness multipliers | none: a patch per event | `#[coalesce]` | [offline](persistence/offline.md#limits-compared-with-aio), [scaling](build/scaling.md#not-in-aio-rs) |
| Sync | path subscriptions | a copy gets every patch of its cell | split cells | [offline](persistence/offline.md#limits-compared-with-aio) |
| Sync | binary side channels, framing, backpressure | JSON only; no binary route | blobs or a route | [transports](clients/transports.md#gaps), [sync](state/sync.md#limits) |
| Sync | every cell served on one socket | `aio::sync::serve` serves one `Handle` | one served cell, or one app per cell | [CLI and service](examples/cli-service.md#limits) |
| Sync | routes over the local socket (`aio://`) | routes only with `[server] listen` | listen | [transports](clients/transports.md#gaps) |
| Sync | a `transport` switch | none | ➖ | [transports](clients/transports.md#gaps) |
| Sync | `allowLocalPeers` opt-out | a release build's door serves this user's processes | ➖ | [transports](clients/transports.md#gaps) |
| Auth | sessions without accounts | sessions come with `[accounts]` | `aio::auth::serving()`, `issue(user, role)` | [accounts](auth/README.md#limits) |
| Auth | `--tls-cert`, `--tls-key`, `--no-tls` | `[server] cert`, `key`; `tls = false` | `aio.toml` | [accounts](auth/README.md#limits) |
| Auth | a share link printed at boot | none | `ax profile`, pairing | [accounts](auth/README.md#limits) |
| Auth | `useUser()` | `Client::me`, `SignIn`'s `on_signed` | ➖ | [accounts](auth/README.md#limits) |
| Auth | a `firewall:` line at boot | none | allow the app in the OS firewall | [setup](basics/setup.md) |
| Views | server rendering, hydration, `collectHead` | none | `ax preview --html`; `aio::head::html` | [views](ui/views.md#gaps-against-aio), [islands](ui/islands.md#gaps) |
| Views | code splitting, `lazy` | one binary | `Defer` defers the render | [islands](ui/islands.md#gaps) |
| Views | other view libraries (`aio/state-core`), `h()`, React compat | `aio::ui` `rsx!` only | an island, or a client over the wire | [islands](ui/islands.md#gaps), [views](ui/views.md#gaps-against-aio) |
| Views | accessibility checked in the running page | static (`ax lint`) | `TestUi` audits | [islands](ui/islands.md#gaps) |
| Views | Redux DevTools | none | `TimeTravelPanel`, `ax timetravel`, `ax events` | [views](ui/views.md#gaps-against-aio) |
| Views | `watch()`, `on()`, `trackedMemo()` | `watch(deps, f)` | `effect`, `memo` | [views](ui/views.md#gaps-against-aio) |
| Views | a 12 ms frame deadline mid-render | one batch per tick | keep renders small | [performance](performance.md#gaps-against-aio) |
| Views | `Worker` script bundling | none | Rust off the thread (`spawn_blocking`) | [performance](performance.md#gaps-against-aio) |
| Kit | `<UiStyles/>` | `theme_css`, `theme::sheet` | ➖ | [kit](ui/kit.md#gaps-against-aio) |
| Kit | `Tooltip` to the side | `Top` or `Bottom` only | ➖ | [kit](ui/kit.md#gaps-against-aio) |
| Forms | Standard Schema (Zod, Valibot, ArkType), `parsed()` | a Rust check function | parse in the check | [forms](ui/forms.md#gaps) |
| Forms | `bind(name)` | two handlers per input | ➖ | [forms](ui/forms.md#gaps) |
| Routing | the address bar, Back and reload follow the route (History API, SPA fallback) | history kept in the app; a web reload starts at `/` | keep the screen in a cell | [routing](ui/routing.md#gaps-against-aio) |
| Theming | scoped `css` classes, a duplicate-class warning | none | prefix class names | [theming](ui/theming.md#gaps-against-aio) |
| Theming | the contrast check in the running dev page | in `TestUi` | a `TestUi` audit | [theming](ui/theming.md#gaps-against-aio) |
| Theming | `neutral = "soft"` key | `theme::tokens_css(soft)`, no `[ui]` key | pass `soft` to `tokens_css` | [theming](ui/theming.md#gaps-against-aio) |
| Animation | a `TransitionFn` (`css(t)`) | CSS `@keyframes` by name | write keyframes | [animation](ui/animation.md#gaps) |
| Desktop | custom window chrome (`ui.chrome`) | the OS title bar | ➖ | [desktop](clients/desktop.md#gaps) |
| Desktop | app menus, a context-menu API | macOS's standard menus only | `kit::Menu`, the tray | [desktop](clients/desktop.md#gaps) |
| Desktop | several app windows | one; child windows for other sites | ➖ | [desktop](clients/desktop.md#gaps) |
| Desktop | `keepServer`, `--keep-server` | none | a server and a client | [desktop](clients/desktop.md#gaps) |
| Desktop | `runCommand` with `stdin` | none | `std::process` in an effect | [desktop](clients/desktop.md#gaps) |
| Desktop | `requireSandbox`, `allowLocalPeers`, `ignoreDebugEnv` | `AIORT_SANDBOX=1` | ➖ | [desktop](clients/desktop.md#gaps) |
| Desktop | `appIcon()` images by app URL (`"/icons/busy.png"`) | PNG bytes or `data:` URLs | `include_bytes!` the image | [desktop](clients/desktop.md#changing-the-icon) |
| Web | the page's console forwarded to the server's log | stays in the page | ➖ | [web](clients/web.md#gaps) |
| Web | every `<webview>` method | `go` and events; desktop only | ➖ | [web](clients/web.md#gaps) |
| CLI apps | `testIO()` over a whole run | none | test `parse`, `failure`, or the binary | [CLI apps](clients/cli-apps.md#gaps-against-aio) |
| CLI apps | `watch` hiding the cursor, one frame per tick | a frame per change | ➖ | [CLI apps](clients/cli-apps.md#gaps-against-aio) |
| CLI apps | flag types inferred from the spec | read by name (`a.number("n")`) | ➖ | [CLI apps](clients/cli-apps.md#gaps-against-aio) |
| Testing | `t.init(seed)`, `t.destroy()` mid-test | none | another cell on the same `TestCells` | [cells](testing/cells.md#what-aio-has-that-aio-rs-does-without) |
| Testing | `t.randomActions(n)`, an unseeded fuzz | every fuzz seeded | ➖ | [cells](testing/cells.md#what-aio-has-that-aio-rs-does-without) |
| Testing | `t.settle(ms)` on real timers | virtual timers | `TestServer` for real I/O | [cells](testing/cells.md#what-aio-has-that-aio-rs-does-without) |
| Testing | `ui.advance(ms)` under the page | `TestUi` time is real | `TestCells::advance` | [UI](testing/ui.md#what-aio-has-that-aio-rs-does-without) |
| Testing | `am testgen` typed clients | none | `ui.names()`, `ax surface --names` | [UI](testing/ui.md#what-aio-has-that-aio-rs-does-without) |
| Testing | `waitForPatch`, `clients[i].patches` | none | assert on each copy's `state()` | [multi-app](testing/multi-app.md#what-aio-has-that-aio-rs-does-without) |
| Testing | a corrupt cassette re-recorded; unserializable args as text | refused | fix or delete it | [cassettes](testing/cassettes.md#what-aio-has-that-aio-rs-does-without) |
| Testing | `am lab windows\|macos\|linux\|android`: VMs and containers it boots, a noVNC viewer | `ax lab` over ssh to a machine you have | an emulator you run | [labs](testing/labs.md#what-aio-has-that-aio-rs-does-without) |
| Testing | `deno task lab`, `deno task test:wine` | none | by hand on a fresh machine | [labs](testing/labs.md#what-aio-has-that-aio-rs-does-without) |
| Testing | `aiol` perf hints, `[fixable]` marks | `TestUi::components`, `ax components` | ➖ | [lint](testing/lint.md#what-aio-has-that-aio-rs-does-without) |
| Testing | an API snapshot gate, an `experimental` marker | review and the CHANGELOG | ➖ | [versions](basics/versioning.md#gaps-against-aios-policy) |
| Debugging | correlation ids, the error box with the state | the call's name; reports carry the state | `ax events` | [errors](debugging/errors.md#not-in-aio-rs) |
| Debugging | `onError` with an error object, many error codes | `on_problem(kind, message)`, `Rejected` | ➖ | [errors](debugging/errors.md#not-in-aio-rs) |
| Debugging | a source-mapped `client.log`, `am logs` budgets | `[page]` lines in `app.log`; `file_mb` × `keep` | ➖ | [errors](debugging/errors.md#not-in-aio-rs) |
| Debugging | `skipActions`, a switch turning time travel off | every event kept | `diagnostics = false`, `[log] suppress` | [time travel](debugging/time-travel.md#not-as-in-aio) |
| Debugging | V8 heap snapshots, the health overlay | none | `ax heap`, `ax health` | [production](debugging/production.md#not-in-aio-rs) |
| Debugging | `perfCheck: "off"`, a runtime benchmark gate | budgets always on | ➖ | [scaling](build/scaling.md#not-in-aio-rs) |
| Debugging | reports screened by `visible`, timeline payloads | carried as saved, never arguments | `#[redact]` | [feedback](debugging/feedback.md#not-as-in-aio) |
| Build | hot reload in a browser | no hot-patching in the web build or on Android | a build per edit | [dev](build/dev.md#limits) |
| Build | a run-from-a-link command | none | clone, `ax start` | [dev](build/dev.md#limits) |
| Build | ios-client (a WKWebView thin client) | no iOS | ➖ | [targets](build/targets.md#limits) |
| Build | `AIO_BUILD_VERSION`, `AIO_SOCKET_TIMEOUT_MS`, `AIO_VIDEO_SCHEME`, an Android emulator test lane | none | `[app] version` and git | [environment](reference/environment.md#aio-variables-aio-rs-doesnt-read) |
| Build | `deno task check`/`test`, one-command release matrix | `ax lint`, `ax test`; the GitHub workflow from `ax release` | ➖ | [CI](build/ci.md#not-in-aio-rs) |
| Release | `canApply` awaited before every install, a warning with none | `can_apply` a plain function, not asked for an `auto` install | ➖ | [release](build/release.md#not-in-aio-rs) |
| Release | a custom runtime refused beside `updates:` | replaced silently | ➖ | [release](build/release.md#not-in-aio-rs) |
| `am` | `persist`, `persist --drop` | none | `#[cell(retire = [..])]` | [ax](clients/ax.md#what-am-has-that-ax-hasnt) |
| `am` | `snapshot load --force`, `dispatch` extras (`key=value`, `--body`, `--as-server`) | none | JSON arguments | [ax](clients/ax.md#what-am-has-that-ax-hasnt) |
| `am` | `kill --stale`, `start --no-wait`, `errors --since=reload` | none | `--wait` | [ax](clients/ax.md#what-am-has-that-ax-hasnt) |
| `am` | `surface --component\|--depth\|--full`, `peer --types` | `ax surface` by path, `ax peer` signatures | ➖ | [ax](clients/ax.md#what-am-has-that-ax-hasnt) |
| `am` | `migrate`, `add cell\|server`, `report` collecting a new one | the compiler; by hand | ➖ | [ax](clients/ax.md#what-am-has-that-ax-hasnt) |
| `am` | the control REST API, `--quiet`; the manager UI's V8 heap and per-action readings | the local door, `--json`; `ax ui`'s aio-rs counterparts | ➖ | [ax](clients/ax.md#what-am-has-that-ax-hasnt) |

## How aio-rs deviates from aio

What a port meets that works differently, and what to do. The
construct-by-construct tables are in the
[migration guide](migration/README.md#2-construct-by-construct).

- **Rust types instead of TypeScript.** Arguments are typed and serde
  refuses a wrong one; ranges go in `ensure!`. A field aio left
  `undefined` is an `Option`. [migration](migration/README.md#cells-and-methods)
- **`aio::ui` views instead of AIR/TSX.** `rsx!` instead of JSX; a
  component re-renders when a signal it read changes or its props
  differ. Read fields with `part`, pass rows as props.
  [views](ui/views.md#coming-from-aios-air-or-react)
- **A refusal discards only its part.** In an async method each part
  between awaits is atomic, and earlier parts stay; no transactions.
  Take what you need before the await. [methods](state/methods.md#differences-from-aio)
- **Methods are pure.** `Date.now()`, ids and randomness come from
  `cx.now()`, `cx.id()`, `cx.rand()`; I/O goes in effects. `ax lint`
  refuses the rest. [checklist](migration/README.md#7-checklist)
- **A method named like a `Handle` method** (`start`, `state`, `call`…)
  is refused at build time: rename it. [migration](migration/README.md#cells-and-methods)
- **Disabling keeps the state** (aio resets it). [lifecycle](state/lifecycle.md)
- **A listener runs only for a committed call** (aio's also on a
  refusal). [composition](state/composition.md#differences-from-aio)
- **A hidden field reads as its default on a copy** (aio throws). Publish
  a fact field beside the secret (`has_password`); mark a method that
  needs it `#[client(predict = false)]`. [visibility](state/visibility.md)
- **A typed call on a copy returns the prediction**; `call_confirmed` is
  the server's answer (aio's `await` gives the server's).
  [sync](state/sync.md#limits)
- **The offline queue holds 500 and refuses when full** (aio: 1000,
  dropping the oldest) unless `[client] retention` is set; it can be kept
  on disk. [sync](state/sync.md#limits)
- **Merges are methods replayed on the server**, not per-field
  strategies; a copy never merges with another copy.
  [offline](persistence/offline.md#limits-compared-with-aio)
- **A list is re-sent whole when its length changes**: keep a growing
  collection in a map. [offline](persistence/offline.md#limits-compared-with-aio)
- **Writes are committed before a call returns**; only `#[coalesce]`
  methods are held (aio debounces every write). Coalesce the methods a
  held key or a pointer calls. [data](migration/data.md#durability-what-each-saves-when)
- **`#[transient]` instead of `persist: { exclude }`** path lists.
  [persistence](persistence/README.md#limits-compared-with-aio)
- **`aio.toml` is built in**, never read at run time: rebuild after an
  edit; per-launch settings are flags or variables. [FAQ](basics/faq.md)
- **Forms touch a field on change**, not on blur: a field left untouched
  shows its error after a submit. [forms](ui/forms.md#gaps)
- **Routes live in the app**, not the browser's history: a web reload
  starts at `/`. [routing](ui/routing.md#gaps-against-aio)
- **`Transition` takes children and `show`**, not a conditional child,
  and motions are CSS keyframes. [animation](ui/animation.md)
- **Tests boot every cell's `#[on_start]`** (aio's `testCell` skipped
  `onInit`), timers in `TestCells` are virtual, `TestUi` time is real,
  and an unread refusal fails the test. [testing cells](testing/cells.md#what-aio-has-that-aio-rs-does-without)
- **A corrupt cassette fails** instead of being re-recorded.
  [cassettes](testing/cassettes.md#what-aio-has-that-aio-rs-does-without)
- **Time travel keeps every event**: no `skipActions`.
  [time travel](debugging/time-travel.md#not-as-in-aio)
- **Problem reports are always on** and never carry arguments.
  [feedback](debugging/feedback.md#not-as-in-aio)
- **`ax start` always waits** (bound it with `--wait`).
  [ax](clients/ax.md#what-am-has-that-ax-hasnt)
- **Variables are `AIORT_*`** where aio's were `AIO_*` (`AIO_CDP` →
  `AIORT_CDP`…). [environment](reference/environment.md#aio-variables-aio-rs-doesnt-read)
- **Keep aio's speed tricks.** aio gave many for free (one signal per
  cell, debounced writes, a broadcast coalescer); a port spells them out.
  [speed tricks](migration/README.md#4-the-speed-tricks-aio-apps-rely-on)

## Feature comparison

✅ the same · 🔄 different (how) · ❌ missing (workaround) · ➕ aio-rs only.
Each ❌ is a row of [feature gaps](#feature-gaps) with its page.

**State**

| Feature | aio | aio-rs | Status |
|---|---|---|---|
| Cells and methods | `cell(name, { state, methods })` | `#[cell]` + `#[methods]` | ✅ |
| Mutation | Immer draft | `&mut self` on a copy | 🔄 |
| Refusals | `throw` | `reject!`, `ensure!`, `aio::Result` | 🔄 |
| Selectors, memoized | `selectors`, `createSelector` | `#[derived]`, `#[derived(reads = [..])]` | ✅ |
| Validation | `validate` | `#[invariant]` | ✅ |
| Argument schemas | `args` | the Rust types + `ensure!` | 🔄 |
| State machines | `machine:` | `aio::machine!`, `#[cell(machine = field)]` | ✅ |
| Many cells, order | `dependsOn`, `composeCells` | `Cells::with`, `#[aio_cell]` fields | ✅ |
| Reactions across cells | `listensTo`, `cancelOn` | `listens_to`, `cx.abort` (committed calls only) | 🔄 |
| Transactions | `transaction: true`, across cells | ❌ (each part atomic; read before the await) | ❌ |
| Lifecycle hooks | `onInit`, `onDestroy`, `onStopping`, `onStop` | `#[on_start]`, `#[on_stop]`, `aio::on_stopping`, `aio::on_stop` | ✅ |
| App-wide start hook, `fatalOnStart`, `isolate` | yes | ❌ (`#[on_start]` per cell) | ❌ |
| Disable, breaker | resets state; `circuitBreaker` | keeps state; `Cells::trip_after` | 🔄 |
| Call hook | `beforeReduce` (may change) | `before_call` (lets in or refuses) | 🔄 |
| Worker cells | `worker: true` | not needed: native code, `spawn_blocking` | 🔄 |
| View-only state | `scope: "client"` | a cell without `persist`/`shared`, or a `Global` | ✅ |
| Visibility | `visible`, `forUser` | `#[cell(visible = f)]`, `#[server]` fields (hidden reads as default) | 🔄 |
| Plugins | `definePlugin` | `aio::plugin::Plugin` | ✅ |
| Retired cells | `am persist --drop` | `#[cell(retire = [..])]` | 🔄 |

**Methods, effects, scheduling**

| Feature | aio | aio-rs | Status |
|---|---|---|---|
| Async methods | `async` on a draft | `async fn`, split at its awaits | 🔄 |
| Effects and modes | `$do`, `concurrency` | `cx.run(Effect::new(..))`, `Mode` | ✅ |
| `ttl`, `long` | yes | `#[ttl(ms)]`, `#[long]` | ✅ |
| Retries, timeouts | `call({ timeoutMs, retries })` | `aio::effect::retry`, `Effect::timeout`, `cx.backoff` | ✅ |
| Schedules | `every`, `cron`, `after`, `backoff` | `#[every]`, `#[cron]`, `cx.after`, `cx.backoff` | ✅ |
| Clock, ids, randomness in methods | allowed, linted | `cx.now()`, `cx.id()`, `cx.rand()` | 🔄 |
| CPU work | `blocking(id, fn)`, `cancel` | `spawn_blocking` (not cancellable) | 🔄 |
| Notifications | `notify()` | `cx.tell(..)`; a web build needs `show_with` | 🔄 |
| Pending calls | `$pending()` | `h.pending(..)`, `use_pending` | ✅ |

**Persistence**

| Feature | aio | aio-rs | Status |
|---|---|---|---|
| SQLite store | `state.db` | the same file | ✅ |
| Event log, history | journal (opt-in) | every commit an event | ➕ |
| Write debounce | every write, 100 ms | `#[coalesce]` methods only | 🔄 |
| Excluded fields | `persist: { include, exclude }` | `#[transient]`, `save = f` | 🔄 |
| Store layout | `single` / `multi` | log + snapshots; aio's layout kept | 🔄 |
| App-wide `persist: false`, `cellDefaults` | yes | ❌ (per cell) | ❌ |
| Migrations | `version`, `onMigrate` | `#[cell(version, migrate)]` | ✅ |
| Integrity check | `checkIntegrityOnBoot` | every boot, with recovery | ✅ |
| Backups, restore | `am backup`, `am restore` | `ax backup`, `ax restore` | ✅ |
| Timed backups | yes | ❌ (the OS's scheduler) | ❌ |
| State export over HTTP | snapshot endpoints | ❌ (`ax snapshot save`/`load`) | ❌ |
| Bound tables | `db:` tables | `#[table]` (flat rows) | 🔄 |
| Schema builder | column helpers | ❌ (`columns = "…"` SQL) | ❌ |
| Raw SQL, live queries | `createDB`, `reactiveDB` | `aio::db::Db`, live queries | ✅ |
| Blobs | `app.blobs` | `aio::blobs::Blobs` | ✅ |
| Size guard | 1 MB warn, 16 MB error | ❌ (`[budgets] state`) | ❌ |
| Homes, profiles, instances | `--home`, `--profile`, `--instance` | the same rules | ✅ |

**Sync and offline**

| Feature | aio | aio-rs | Status |
|---|---|---|---|
| Shared cells | `sync`, broadcast | `#[cell(shared)]` | ✅ |
| Optimistic calls | `useOptimistic`, local-first | built in: copies predict | ✅ |
| Access per method | `access` | `#[client(role, allow)]` | ✅ |
| Merging | per-field CRDT strategies, HLC | ❌ (methods replayed on the server) | ❌ |
| Server-less, peer-to-peer | yes | ❌ (a server confirms) | ❌ |
| Offline queue | 1000, in memory, drops oldest | 500, refuses when full, on disk if asked | 🔄 |
| Array patches | Immer ops | whole `Vec` on a length change | 🔄 |
| Throttles | `syncIntervalMs`, `fullStateThreshold` | ❌ (a patch per event; `#[coalesce]`) | ❌ |
| Path subscriptions | yes | ❌ (split cells) | ❌ |
| Binary wire, side channels | yes | ❌ (blobs, routes) | ❌ |
| Server functions, routes | `serverFns`, `routes` | `aio::fns::ServerFn`, `aio::http` | ✅ |
| One socket, every cell | yes | ❌ (`sync::serve` takes one `Handle`) | ❌ |
| Pairing, discovery, peers | PIN, CPace, UDP 8099, `x-aio-peer` | the same formats | ✅ |
| Calling another app | `peer()`, `call` | `Peer::find`, `AioLink` | ✅ |
| Server limits | `maxConnections`, `wsLimits` | `[server.limits]` | ✅ |

**Auth and security**

| Feature | aio | aio-rs | Status |
|---|---|---|---|
| Accounts, roles, TOTP, OIDC | `auth`, `sessions` | `[accounts]` | ✅ |
| Sessions without accounts | yes | ❌ (`auth::serving()`, `issue`) | ❌ |
| TLS | `--tls-cert`/`--tls-key`, own LAN CA | `[server] cert`/`key`, a machine root for the LAN | 🔄 |
| Security headers | yes | `[server.security]` | ✅ |
| Origins, proxies | `allowedOrigins`, `trustProxyHeader` | `[server] hosts`, `trust_proxy` | ✅ |
| CORS, passkeys | none | none | ✅ |
| Share link at boot, firewall line | yes | ❌ (`ax profile`, pairing) | ❌ |
| Outbound network list | `nativeFetch` | `[capabilities] network` | ✅ |
| Secrets in logs | `redactActions` | `#[redact]`, masked logs | ✅ |

**UI: views, kit, forms, routing, theming, animation**

| Feature | aio | aio-rs | Status |
|---|---|---|---|
| Renderer | AIR, TSX | `aio::ui`, `rsx!` (Dioxus underneath until air) | 🔄 |
| Field-level reads | one signal per cell | `part(\|c\| &c.field)` | 🔄 |
| Hooks | AIR and React names | `signal`, `memo`, `effect`, `watch`, `once`, `on_cleanup`: AIR's names, no dependency arrays | 🔄 |
| Server rendering, hydration | yes | ❌ (`ax preview --html`) | ❌ |
| Code splitting, `lazy` | yes | ❌ (`Defer`) | ❌ |
| Other view libraries, `h()` | `aio/state-core`, React compat | ❌ (islands) | ❌ |
| Islands, React islands, `<Browser>` | yes | `WebIsland`, `Island::react`, `kit::Browser` | ✅ |
| Kit components | `aio/ui` | `aio::kit`, the same names | ✅ |
| Side tooltips | yes | ❌ (`Top`, `Bottom`) | ❌ |
| Keys | `onGlobalKey`, `REPEATABLE_KEYS` | `use_global_key`, `Chord::repeating` | ✅ |
| Long lists | `useVirtualList` | `kit::VirtualList` | ✅ |
| Forms | `useForm` + Standard Schema, touched on blur | `use_form` + a Rust check, touched on change | 🔄 |
| Routing | History API, address bar | in-app history, typed `routes!` | 🔄 |
| Head | `useHead` | `aio::head::use_head` | ✅ |
| Theme, hue, CSS variables | `ui.theme`, `hue` | `theme_css(hue)`, `--aio-*` | ✅ |
| Scoped CSS classes | `css` tag | ❌ (prefix by hand) | ❌ |
| Live CSS in dev | yes | yes, plus `[build] css` toolchains | ✅ |
| Transitions | `TransitionFn`, shared `options` | CSS keyframes, a `Motion` each | 🔄 |
| Runtime a11y check | dev mode | ❌ (`ax lint`, `TestUi`) | ❌ |
| Window chrome, app menus, several windows | yes | ❌ (OS title bar, `kit::Menu`, child windows) | ❌ |
| Redux DevTools | yes | ❌ (`TimeTravelPanel`) | ❌ |
| Worker bundling | yes | ❌ (`spawn_blocking`) | ❌ |

**Testing**

| Feature | aio | aio-rs | Status |
|---|---|---|---|
| Cell tests | `testCell`, `bootCells` | `TestCells` (virtual time, seeded fuzz) | ✅ |
| UI tests | `testUI` on happy-dom | `TestUi` on the real engine | ➕ |
| Virtual clock under the page | `ui.advance` | ❌ (`TestCells::advance`) | ❌ |
| Typed UI clients | `am testgen` | ❌ (`ax surface --names`) | ❌ |
| Server and multi-client | `testServer`, `testMultiClient` | `TestServer`, `Sim` | ✅ |
| Patch assertions | `waitForPatch` | ❌ (each copy's `state()`) | ❌ |
| Cassettes | `openCassette` | `Cassette` (stricter) | 🔄 |
| Browser tests | `testBrowser` | `TestBrowser` | ✅ |
| Lint | `aiol` | the compiler + `ax lint` | 🔄 |
| Labs | VMs and containers it boots | `ax lab` over ssh | 🔄 |
| CLI run capture | `testIO()` | ❌ (test `parse`, the binary) | ❌ |
| API snapshot gate | yes | ❌ (review, CHANGELOG) | ❌ |

**Debugging**

| Feature | aio | aio-rs | Status |
|---|---|---|---|
| Time travel | dev only, 2000 entries | the whole store, release builds too | ➕ |
| Replay, record | `am replay`, `am record` | `ax replay`, `ax record` (writes a Rust test) | ✅ |
| Logs, errors, health, vitals, budgets | yes | `ax logs`, `ax errors`, `ax health`, `[vitals]`, `[perf]`, `[budgets]` | ✅ |
| Correlation ids, error box with state | yes | ❌ (`ax events`, reports) | ❌ |
| Heap snapshots | V8 | not needed: no GC heap; `ax heap` | 🔄 |
| Problem reports | `feedback: true` | always on, no arguments | 🔄 |
| Manager UI | `am ui` | `ax ui` (the same tabs) | ✅ |

**Build, targets, dev**

| Feature | aio | aio-rs | Status |
|---|---|---|---|
| Desktop | electron | Linux, Windows, macOS on a shared runtime | ✅ |
| Web, PWA | `web` | `ax build --web` | ✅ |
| Android | `android` | APK, built on Linux | ✅ |
| Service, CLI | `server`, `cli` | `mode = "server"`, `cli` | ✅ |
| Thin clients | electron-, android-, cli-client | `mode = "client"` | ✅ |
| iOS | ios-client | ❌ | ❌ |
| Dev reload | patch, reload, restart (~3 s) | hot-patch, state kept | 🔄 |
| Hot reload in web, Android | page reload | ❌ (a build per edit) | ❌ |
| Single instance, takeover | `singleton`, `takeover` | always one per home, `--takeover` | ✅ |

**Release and updates**

| Feature | aio | aio-rs | Status |
|---|---|---|---|
| Signed releases | `ship keygen`, `publish` | `ax keygen`, `ax release` | ✅ |
| Delta updates, channels, data gate, rollback | yes | yes | ✅ |
| Update veto | `canApply` (awaited, every install) | `can_apply` (plain, not for `auto` at start) | 🔄 |
| GitHub workflow | — | from `ax release` | ➕ |

**CLI: `am` and `ax`**

| Feature | aio | aio-rs | Status |
|---|---|---|---|
| App control | `start`, `stop`, `kill`, `restart`, `status` | the same | ✅ |
| State and history | `state`, `snapshot`, `migrations`, `replay`, `timetravel` | the same | ✅ |
| Calls | `dispatch` | `ax call` | 🔄 |
| Inspection | `surface`, `trigger`, `shot`, `eval`, `preview`, `sql`, `heap`, `cost` | the same | ✅ |
| Pairing, trust | `discover`, `profile`, `pair`, `auth`, `trust` | the same (`ax pin` makes a pairing code; aio's `am pin` is a framework version: `ax framework`) | ✅ |
| `where`, `check` | yes | the compiler | 🔄 |
| `persist`, `testgen`, `add`, control REST API | yes | ❌ | ❌ |
| `keygen`, `release`, `update`, `smoke`, `install`, `lint`, `test`, `events`, `components`, `adopt`, `import`, `runtime`, `setup` | — | yes | ➕ |

**Tooling**

| Feature | aio | aio-rs | Status |
|---|---|---|---|
| Agent brief | `am agent` | `ax agent`, `AGENTS.md`/`CLAUDE.md` in new apps | ✅ |
| Framework version per app | `dep/aio` link, `am pin`, `am fix` | aio-rs from git at a release tag in `Cargo.toml`, the commit in `Cargo.lock`: a clone builds as it is; `ax create --aio-version`, `ax framework <v>\|main\|latest`, `--aio <dir>` for a checkout | ✅ |
| Upgrading the CLI | `am upgrade`, `am uninstall` | `ax upgrade` (the release's checked install script, or a checkout), `ax uninstall` | ✅ |
| Install | `install.sh` (Deno + a clone) | prebuilt `ax` (Linux, Windows) or source; no prebuilt macOS | 🔄 |
| API reference | `api-reference.md` | `cargo doc -p aio-rs` | ✅ |

## Plans

- **0.2.0: aio-rs's own renderer, "air".** It replaces Dioxus with a
  fine-grained renderer of aio-rs's own: a cell's fields as signals,
  direct DOM edits for what changed, and a lean macro so rebuilds get
  faster ([design](design/air.md)). Apps are already written against
  `aio::ui`, which names no renderer, so the swap is a framework change,
  not an app migration.

## Moving

- [Migrating from aio to aio-rs](migration/README.md): tables, a worked
  example, the speed tricks, a checklist.
- [aio app data](migration/data.md) and [aio wire formats](migration/wire.md).
- [Why aio-rs, not plain Rust](basics/why.md).

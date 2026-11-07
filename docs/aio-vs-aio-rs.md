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
| Renderer | AIR (React-shaped, fine-grained signals) | `aio::ui` views (`rsx!`), drawn by air (aio-rs's own, fine-grained signals: a body runs once, holes follow state, [air](design/air.md)); the window is a shared, pinned Electron runtime (desktop), the system WebView (Android), wasm in a browser |
| State model | cells, methods on an Immer draft, selectors | cells, methods on `&mut self` (a copy), `#[derived]`; every call an event in the store's log |
| Data compatibility | `state.db` | the same `state.db`, both ways: a port opens aio's file as it is, and aio opens it after ([below](#data-the-same-statedb)) |
| Wire compatibility | protocol 3 on `/ws`, NDJSON on a local socket | its own wire 2; with feature `compat` also aio's protocol 3 on `/ws`; pairing, discovery, peers and blobs in aio's formats always ([below](#the-wire)) |
| Electron store | per user, `apps/` records, `am prune` | the same store, the same rules: one copy per version for both ([below](#the-electron-store)) |
| Targets | browser, electron, android, web, cli, server, server-app; thin clients electron-client, android-client, ios-client, cli-client | desktop (Linux, Windows, macOS), web (wasm, PWA), Android, service, CLI; a thin client is `[app] mode = "client"`; no iOS ([targets](build/targets.md)) |
| Install | `install.sh`: installs Deno, clones aio, puts `am` on `PATH` | a prebuilt `ax` (Linux x86_64, Windows x86_64, macOS arm64 and x86_64) or from source; an app takes aio-rs from git at a release tag ([setup](basics/setup.md)) |
| Dev loop | `am dev`: `App.tsx` patched, other edits reload the page, a cell edit restarts the app (~3 s) | `ax dev`: Rust hot-patched with the state kept, `rsx!` markup at once, CSS live; a rebuild and restart when a patch can't take it ([dev](build/dev.md)) |
| Tests | `testCell`, `testUI` (happy-dom), `testServer`, cassettes, `deno test` | `TestCells`, `TestUi` (the real views in the runtime), `TestServer`, cassettes, `cargo test` ([testing](testing/README.md)) |
| Maturity | beta (1.0.27-beta): the 1.x surface frozen, additions only, an API snapshot gate in CI | early alpha, third release (0.3.0); a minor version may break ([versions](basics/versioning.md)) |

## App compatibility

An aio app and its port (the aio-rs app made from it) share data and
talk to each other, so an app can move one piece at a time.

### Data: the same state.db

- **Where.** `[app] port_of = "<the aio app's appId>"` makes the port
  resolve the home exactly as aio does (`--home` > `--profile` >
  `$AIO_APPS_DIR/<appId>` > `~/.<appId>`, plus the app's own `[home]`
  rules) and open `<home>/data/state.db`, or the file `[data] store`
  names in `data/` (aio's `dbPath` could be any path, or `:memory:`) ([data](migration/data.md#where-the-data-lives)).
- **What it reads.** aio's snapshot in `aio_kv`, in either layout (`single`:
  one `state` row; `multi`: a row per cell), each cell at its
  `<appId>:__versions` stamp. An older stamp migrates (after a backup); a
  newer one (a rollback) boots on what this build declares, keeps the
  rest in every write and parks a copy under `__downgraded:<cell>`, as aio
  does; one that doesn't fit refuses the boot and keeps the file. Rolled
  forward again, it boots and drops the older build's own fields, as aio
  does.
- **What it writes.** On every commit, in the same transaction as its own
  log: the cell's row in aio's layout with `JSON.stringify` semantics,
  without `#[transient]` fields and `persist(exclude)` paths, keeping unknown cells and `__*` keys, and
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
- **aio's journal.** Calls a crash left in aio's journal past its save are
  replayed at the port's boot, as aio's next boot would, and the
  watermark moved with them; a tail aio-rs can't replay (time-travel or
  write-set lines, calls of cells of another store) refuses the boot:
  start the aio app once.
- **Not ported:** the `__shapes` stamps of aio's dev-mode shape check.
  aio's `profiles: false` is `[app] profiles = false`, `appDir` `[home]
  dir`.

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
- Code: an aio cell or component is translated, not loaded.

## Where aio-rs is better

### Measured

A large desktop app (about 74 000 lines, a list of 1000 rows), in aio
and in aio-rs on one machine, both release builds pinned to the same
cores; the median of 5 runs:

| | aio | aio-rs |
|---|---|---|
| Start, cold: first window | 732 ms | **149 ms** |
| Start, cold: UI ready | 1112 ms | **468 ms** |
| Start, warm: first window | 707 ms | **282 ms** |
| Memory in a session (PSS, every process) | 724 MB | **430 MB** |
| A page of 1000 rows opens | 104 ms | **72 ms** |
| CPU per held key | 30 ms | **19–21 ms** |
| The app's AppImage | 97 MB | **18.5 MB** |

After a held key is released, both show the last step at once: 0 ms
of lag. Key to paint: [below](#key-to-paint), by two clocks.

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
- **Real UI tests.** `TestUi` runs the real views in the real engine:
  rects, styles, screenshots and scrolling measure for real, where aio's
  happy-dom has no layout ([UI tests](testing/ui.md)).
- **Hot-patching with the state kept** for Rust, views and methods
  alike, and a compile error shown on the window while the app keeps
  running ([dev](build/dev.md)). `ax dev` does it alone: a markup edit
  shows in ~4 ms, a stylesheet in ~20 ms, a Rust edit in ~0.3 s.
- **Smaller first steps on an existing machine.** One native binary per
  app (5.0 MB for the template), the window runtime shared by every app.

## Where aio is better

### Key to paint

The same app as [above](#measured), keydown to the first frame that
shows it, in ms, p50 / p95: 5 sessions of each side, taken in turn on
one machine (load 11-12 on its 12 cores). aio-rs runs the app with its
30 big fields held as [`aio::Big`](performance.md#7-share-big-fields-aiobig).

Two clocks. *Stamp* is the time a frame is given (`requestAnimationFrame`'s
argument): the vsync it was planned for. *Ran* is when that frame's work
ran, the earliest it can be on screen. aio renders in the page: a key
that holds the page for 200 ms still gets the frame planned before it,
stamped early. aio-rs renders in the app's process; its frames run when
stamped.

| Key | aio stamp | aio-rs stamp | aio ran | aio-rs ran | CPU per key, aio / aio-rs |
|---|---|---|---|---|---|
| Hold ↓ (60 keys, 33 ms apart) | 13.1 / 25.0 | **10.3 / 18.4** | 23.2 / 38.7 | **12.1 / 20.7** | 35.0 / **12.2** |
| Hold ↑ | 13.9 / 26.9 | **10.9 / 18.5** | 26.4 / 49.5 | **13.3 / 21.5** | 35.5 / **14.4** |
| End, 1000 rows | 13.5 / 20.4 | **13.5 / 19.6** | 217.6 / 333.6 | **19.6 / 23.4** | 380 / **48** |
| Home, 1000 rows | **9.1 / 16.8** | 13.4 / 19.7 | 226.3 / 394.6 | **19.4 / 26.8** | 315 / **17** |
| Tab | **9.8 / 18.7** | 11.6 / 19.0 | 26.5 / 46.6 | **17.8 / 23.0** | 80 / **58** |

- By when the frame ran, aio-rs is ahead on every key, End and Home
  ten times over, with a fifth to a third of aio's CPU.
- By the stamp, aio is ahead on Home and Tab. A frame planned before
  aio-rs's answer reaches the page can't show it: an aio-rs key costs
  ~1.2 ms of messages between the page and the app before its work,
  where aio's work starts in the page at once. Tab's work in the app is
  1.6 ms (most of it the app's own method and invariant), Home's 4-5 ms
  (the 60 rows that enter).
- Where the time went before: each step compared the 1000-row state
  deeply (4-9 ms; 0.03 ms with `Big`), a row's static holes were boxed
  and freed (now run once, never boxed), and the rows that left were
  freed before the frame went out (now after).

### The edit loop

`ax dev` on the template app beside `am dev` on aio's (aio-rs 0.3.0,
aio 1.0.25-beta, one Linux machine, Xvfb, one script driving both,
2026-11-07; the method and every row are in
[comparison](comparison.md#the-edit-loop-next-to-aios-am-dev)):

| | `am dev` | `ax dev` |
|---|---|---|
| First start, nothing built yet | **2.6 s** | 24.2 s (aio-rs and its dependencies compiled once per machine; a splash with cargo's count after 0.35 s); `ax setup` builds that in the background at install, then **1.7 s** |
| Start again, nothing changed | 1.1 s | **0.33 s** (the last build runs as it is) |
| Start after an edited session | 0.8 s | **0.35 s** (built behind the session) |
| Start, sources changed with `ax dev` off | **0.8 s** | 1.0 s (the changed crates compiled) |
| A stylesheet saved → on the window | 0.31 s, a reload | **0.01 s**, state kept |
| Markup saved → on the window | 0.50 s | **0.008 s**, state kept |
| View logic saved → on the window | 0.41 s | **0.27 s** hot-patched, state kept |
| A cell method saved → on the window | 0.84 s, the app restarted | **0.40 s** hot-patched, state kept |
| A changed type (a cell's field) | a restart | **2.1 s** rebuild and restart, state back from the log |
| A compile error | – | on the window in 0.13 s, the app running on; its fix 0.6 s |

`ax create` at a terminal builds the new app in the background, so its
first `ax dev` is a start, not that first build.

aio restarts an app from source in ~3 s whatever its size. aio-rs
recompiles the crate an edit is in:

| Edit | aio-rs |
|---|---|
| a small crate (the template, a few thousand lines) | 0.3–2 s, hot-patched |
| a crate of about 74 000 lines | ~10 s |
| a first (cold) build | 15 s (template) to ~60 s (big app) |
| that app split into crates: a page edit | ~1.9 s |
| … an edit in its shared kit or core | 8–14 s |

So a big aio-rs app is kept fast to edit by splitting it into crates
([what an edit costs](build/README.md#what-an-edit-costs)). The web build
and Android have no hot-patching: each edit is a build (on Android, of the
page only: the dev APK is installed once and reloads, as aio's `dev:android`).

### Feature gaps

Every gap the guide names against aio, by area. ➖ = no workaround needed
beyond the aio-rs form.

| Area | aio | aio-rs | Workaround | Page |
|---|---|---|---|---|
| State | `am where` | no module-graph answer of where code runs; the compiler and `cfg(target_arch = "wasm32")` enforce it | ➖ | [architecture](basics/architecture.md#gaps-against-aio) |
| State | worker cells (`worker: true`), `crashWorker`, `workers: "real"` | native code: heavy work goes off the thread with `aio::effect::blocking` | ➖ | [effects](state/effects.md#heavy-work-retries-and-timeouts) |
| Effects | `t.getEffects()`, effect objects in tests (`isScheduleEffect`) | an effect is a closure: `t.held_effects()` (key, mode, timeout); schedules `Store::schedules` | ➖ | [effects](state/effects.md#limits) |
| Persistence | `persistMode: "single" \| "multi"` | log + snapshots per cell; aio's layout read and written as found | ➖ | [persistence](persistence/README.md#limits-compared-with-aio) |
| Persistence | `cellDefaults.visible` (a filter for every cell) | each cell's `visible = f`, checked against its own type; `persist: "none"` is `[data] only_persist = true` or `#[cell(persist = false)]` | ➖ | [persistence](persistence/README.md#limits-compared-with-aio) |
| Persistence | a query builder (`where` objects) | plain SQL (`aio::db::Db`) | ➖ | [SQL](persistence/sql.md#limits-compared-with-aio) |
| Sync | an HLC ordering peers' edits | the server orders every call: no clocks to agree or skew; `Remote::merge` shows `Text`, `Counter`, `SetAdd`, `Keys` (`lww-per-key`), `SetRemove` | ➖ | [offline](persistence/offline.md#limits-compared-with-aio) |
| Sync | render-staleness backpressure | every link paced the same (`sync_interval_ms`) | ➖ | [offline](persistence/offline.md#limits-compared-with-aio), [scaling](build/scaling.md#not-in-aio-rs) |
| Sync | path subscriptions | a copy gets every patch of its cell, and predicts; `visible` sends each viewer only what it may see, and a part some screens never read is its own cell | ➖ | [offline](persistence/offline.md#limits-compared-with-aio) |
| Sync | a `transport` switch | none | ➖ | [transports](clients/transports.md#gaps) |
| Sync | `allowLocalPeers` (off unless set): a release socket only for the window aio spawned | the window's sockets are always the window's; `[server] local_peers` (on unless `false`) keeps `ax state` and `ax call` working on a release, with a client's rights (by design: a process of the user can read the app's key and store anyway) | `[server] local_peers = false` | [transports](clients/transports.md#gaps) |
| Sync | `useOptimistic` | a synced copy shows every call at once (it predicts); `#[client(predict = false)]` waits | ➖ | [views](ui/views.md#gaps-against-aio) |
| Auth | `useUser()` everywhere | `aio::web::use_user()` in a browser; `Client::me`, `SignIn`'s `on_signed` elsewhere | ➖ | [accounts](auth/README.md#limits) |
| Views | server rendering, hydration, `collectHead` | none | `ax preview --html`; `aio::head::html` | [views](ui/views.md#gaps-against-aio), [islands](ui/islands.md#gaps) |
| Views | code splitting, `lazy` | one binary | `Defer` defers the render and loads (`load`, `loading`, `error`, `loading_min_ms`) | [islands](ui/islands.md#gaps) |
| Views | other view libraries (`aio/state-core`), `h()`, React compat | `aio::ui` `rsx!` only | an island, or a client over the wire | [islands](ui/islands.md#gaps), [views](ui/views.md#gaps-against-aio) |
| Views | Redux DevTools | none | `TimeTravelPanel`, `ax timetravel`, `ax events` | [views](ui/views.md#gaps-against-aio) |
| Views | `trackedMemo()`, a keyed cache shared by components | `memo`, `GlobalMemo` | ➖ | [views](ui/views.md#gaps-against-aio) |
| Views | a 12 ms frame deadline mid-render | one batch of holes per tick: 10 000 woken holes run in 0.8 ms, so a deadline is never reached | ➖ | [performance](performance.md#gaps-against-aio) |
| Views | `useOptimistic`, a general hook | none: a synced copy predicts each call and rolls a refusal back | ➖ | [views](ui/views.md#gaps-against-aio) |
| Views | typed `useRoute(pattern)` (param names) | `routes!` (names and parsed values), `match_path` | ➖ | [routing](ui/routing.md#gaps-against-aio) |
| Views | `TransitionFn` given the element | `Motion::of` (`css(t)`, `u = 1 - t`), `.ticking(tick)`; a transform's percentages are the element's own box | ➖ | [animation](ui/animation.md#gaps) |
| Views | `Worker` script bundling | none | Rust off the thread (`spawn_blocking`) | [performance](performance.md#gaps-against-aio) |
| Kit | `<UiStyles/>` | `theme_css`, `theme::sheet` | ➖ | [kit](ui/kit.md#gaps-against-aio) |
| Forms | Standard Schema (Zod, Valibot, ArkType), `parsed()` | a Rust check function | parse in the check | [forms](ui/forms.md#gaps) |
| Theming | a duplicate-class warning for scoped `css` classes | `aio::ui::css(rule)`, no warning (a class is its rule's hash) | ➖ | [theming](ui/theming.md#gaps-against-aio) |
| Web | the page's console forwarded to the server's log, in a release too | in `ax dev --web`'s log; a release page's stays in the page | ➖ | [web](clients/web.md#gaps) |
| Web | the `<webview>` element itself (every method) | `go`, `run_js`, `Css`, `Zoom`, `DevTools` and events; desktop only | `run_js` | [web](clients/web.md#gaps) |
| CLI apps | flag types inferred from the spec, checked at compile time | read by name and kind (`a.number("n")`), a misread panics on the first run | ➖ | [CLI apps](clients/cli-apps.md#gaps-against-aio) |
| Testing | `t.randomActions(n)`, an unseeded fuzz | every fuzz seeded | ➖ | [cells](testing/cells.md#what-aio-has-that-aio-rs-does-without) |
| Testing | `t.settle(ms)` on real timers | virtual timers (a 10 s backoff settles at once); `TestServer` for real I/O | ➖ | [cells](testing/cells.md#what-aio-has-that-aio-rs-does-without) |
| Testing | a corrupt cassette re-recorded silently; unserializable args as text | refused unless `AIO_CASSETTE=rerecord`; args refused, naming the call | ➖ | [cassettes](testing/cassettes.md#what-aio-has-that-aio-rs-does-without) |
| Testing | `deno task test:wine` | Windows bundles smoked on a real Windows (`./ci-windows.sh`, GitHub's Windows runner) | ➖ | [labs](testing/labs.md#what-aio-has-that-aio-rs-does-without) |
| Testing | `aiol` perf hints, `[fixable]` marks | `TestUi::components`, `ax components` | ➖ | [lint](testing/lint.md#what-aio-has-that-aio-rs-does-without) |
| Testing | an `experimental` marker | every 0.x item carries the same promise | ➖ | [versions](basics/versioning.md#gaps-against-aios-policy) |
| Debugging | `onError`'s `stateSnapshot`, `EFFECT_*`/`HOOK_*` codes | `aio::Code` (`REDUCE_ERROR`, `INIT_ERROR`, `QUEUE_OVERFLOW`, `DISPATCH_*`, `PERSIST_ERROR`, …, unknown ones kept), `error_summary`; the bus's typed `Diagnostic` (cell, call, numbers); the state in a report, and in a debug build's panic line | ➖ | [errors](debugging/errors.md#not-in-aio-rs) |
| Debugging | a source-mapped `client.log`, `am logs` budgets | `[page]` lines in `app.log`; `file_mb` × `keep` | ➖ | [errors](debugging/errors.md#not-in-aio-rs) |
| Debugging | V8 heap snapshots; a health overlay in its docs | no garbage-collected heap to snapshot: memory is the resident set (`ax heap` per cell, `ax health`, `/__aio/health`); aio's overlay was never put in a page (its own source says so), and a debug page's badge lists its errors | ➖ | [production](debugging/production.md#not-in-aio-rs) |
| Debugging | `perfCheck: "off"`, a runtime benchmark gate | budgets always on | ➖ | [scaling](build/scaling.md#not-in-aio-rs) |
| Debugging | timeline payloads in reports | never a call's arguments (a report may leave the machine); its state screened by `visible` as aio's | ➖ | [feedback](debugging/feedback.md#not-as-in-aio) |
| Build | hot-patching in a browser | `ax dev --web` rebuilds and reloads its tabs per edit, slower than a patch (on Android the dev page reloads) | ➖ | [dev](build/dev.md#limits) |
| Build | server diagnostics, live a11y and contrast in the dev overlay | build failures and page errors | `ax logs`, `ax errors`; `TestUi` audits | [dev](build/dev.md) |
| Build | an unsigned macOS `.app` built on any host | built on a Mac (or one over ssh, `[build.macos] host`) | ➖ | [targets](build/targets.md#limits) |
| Build | a run-from-a-link command (a script piped into a shell) | a person installs the signed release; a developer runs `git clone <link> && cd <name> && ax start` | ➖ | [dev](build/dev.md#limits) |
| Build | ios-client (a WKWebView thin client) | no iOS | ➖ | [targets](build/targets.md#limits) |
| Build | `AIO_BUILD_VERSION`, `AIO_SOCKET_TIMEOUT_MS`, `AIO_VIDEO_SCHEME` | none (the emulator lane is `ax test --android`) | `ax build --app-version v` | [environment](reference/environment.md#aio-variables-aio-rs-doesnt-read) |
| Build | `deno task check`/`test`, one-command release matrix | `ax lint`, `ax test`; the GitHub workflow from `ax release` | ➖ | [CI](build/ci.md#not-in-aio-rs) |
| Release | `canApply` awaited before every install, a warning with none | `can_apply` a plain function, not asked for an `auto` install | ➖ | [release](build/release.md#not-in-aio-rs) |
| Release | a new build of the same version offered | only a newer version (a new commit is one) | ➖ | [release](build/release.md#not-in-aio-rs) |
| Release | `ship github` deploys a GitHub Pages site | GitHub releases read directly (`github:`) | ➖ | [release](build/release.md#not-in-aio-rs) |
| `am` | `dispatch` plain actions, `--as-server` | none | cell methods; a debug build's door | [ax](clients/ax.md#what-am-has-that-ax-hasnt) |
| `am` | `kill --stale` | none | the OS's lock | [ax](clients/ax.md#what-am-has-that-ax-hasnt) |
| `am` | `add cell\|server`, `report` collecting a new one | by hand (`am migrate` is `ax lint --from`) | ➖ | [ax](clients/ax.md#what-am-has-that-ax-hasnt) |
| `am` | `publish --no-data` | a release always carries its data contract | `--data file` | [ax](clients/ax.md#what-am-has-that-ax-hasnt) |
| `am` | the manager UI's V8 heap; piped output as JSON; `am watch`, `build --force`; `am pin`'s preflight of removed APIs | `ax ui`'s aio-rs counterparts (per-action time and refusals as aio's); text output (`--json` on any verb); `cargo check` names a removed API | ➖ | [ax](clients/ax.md#what-am-has-that-ax-hasnt) |

## How aio-rs deviates from aio

What a port meets that works differently, and what to do. The
construct-by-construct tables are in the
[migration guide](migration/README.md#2-construct-by-construct).

- **Rust types instead of TypeScript.** Arguments are typed and serde
  refuses a wrong one; ranges go in `ensure!`. A field aio left
  `undefined` is an `Option`. [migration](migration/README.md#cells-and-methods)
- **`aio::ui` views instead of AIR/TSX.** `rsx!` instead of JSX; where AIR
  re-renders a component that read a changed signal, air runs its body
  once and only what read the signal (its holes) again. Read fields with `part`, pass rows as props, what
  must follow as a `Prop`.
  [views](ui/views.md#coming-from-aios-air-or-react)
- **A refusal discards only its part.** In an async method each part
  between awaits is atomic, and earlier parts stay, unless the cell is
  `#[cell(transaction)]` (aio's `transaction: true`): then the method
  commits all or nothing. [methods](state/methods.md#transactions)
- **Methods are pure.** `Date.now()`, ids and randomness come from
  `cx.now()`, `cx.id()`, `cx.rand()`; I/O goes in effects. `ax lint`
  refuses the rest. [checklist](migration/README.md#7-checklist)
- **A method named like a `Handle` method** (`start`, `state`, `call`…)
  is refused at build time: rename it. [migration](migration/README.md#cells-and-methods)
- **Disabling keeps the state** (aio resets it). [lifecycle](state/lifecycle.md)
- **A listener runs only for a committed call** (aio's also on a
  refusal). [composition](state/composition.md#differences-from-aio)
- **A `#[server]` field of a shared cell is an `aio::Hidden<T>`**: a copy
  never has it and any use there panics naming it (aio throws on a read);
  a method that touches it is `#[client(predict = false)]`.
  [visibility](state/visibility.md)
- **A typed call of a plain method on a copy returns the prediction**;
  `call_confirmed` is the server's answer (aio's `await` gives the
  server's). An async method's `Answer` is the server's end.
  [sync](state/sync.md#limits)
- **A full offline queue refuses new calls** (500, as aio's `pendingCap`;
  aio drops the new op) unless `[client] retention` lets the oldest past
  it go; it can be kept on disk. [sync](state/sync.md#limits)
- **Merges are methods replayed on the server** (as aio's CRDT ops run
  through its methods); a copy may show a field merged while its call
  waits (`Remote::merge`) and hears where a server change met it
  (`on_conflict`). [offline](persistence/offline.md#conflicts-and-fields-shown-merged)
- **Writes are committed before a call returns**; only `#[coalesce]`
  methods are held (aio debounces every write). Coalesce the methods a
  held key or a pointer calls. [data](migration/data.md#durability-what-each-saves-when)
- **`aio.toml` is built in**, never read at run time: rebuild after an
  edit; per-launch settings are flags or variables. [FAQ](basics/faq.md)
- **A form field with its own handlers is touched on change**, not on
  blur; a bound input (`form.bind`) is touched on blur, as aio's.
  [forms](ui/forms.md#bound-fields)
- **`Transition` takes children and `show`**, not a conditional child;
  motions are CSS keyframes or a function (`Motion::of`), and it draws no
  box of its own. [animation](ui/animation.md)
- **Tests boot every cell's `#[on_start]`** (aio's `testCell` skipped
  `onInit`), timers in `TestCells` are virtual, `TestUi` time is real until `advance`,
  and an unread refusal fails the test. [testing cells](testing/cells.md#what-aio-has-that-aio-rs-does-without)
- **A corrupt cassette fails** instead of being re-recorded, unless
  `AIO_CASSETTE=rerecord` asks. [cassettes](testing/cassettes.md#what-aio-has-that-aio-rs-does-without)
- **Time travel keeps every event in the log**: `#[skip_travel]` (aio's
  `skipActions`) only steps over a method's events.
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
| Mutation | Immer draft (untouched parts shared) | `&mut self` on a copy; big fields shared as `aio::Big` | 🔄 |
| Refusals | `throw` | `reject!`, `ensure!`, `aio::Result` | 🔄 |
| Selectors, memoized | `selectors`, `createSelector` | `#[derived]`, `#[derived(reads = [..])]` | ✅ |
| Validation | `validate` | `#[invariant]` | ✅ |
| Argument schemas | `args` | the Rust types + `ensure!` | 🔄 |
| State machines | `machine:` | `aio::machine!`, `#[cell(machine = field)]` | ✅ |
| Many cells, order | `dependsOn`, `composeCells` | `Cells::with`, `#[aio_cell]` fields | ✅ |
| Reactions across cells | `listensTo`, `cancelOn` (in the call's reduce and save) | `listens_to` (inside the call's commit: one SQLite transaction), `cx.abort`; committed calls only, and a refused reaction never undoes its call | 🔄 |
| Transactions | `transaction: true` / `{ serialize, conflict }`, `$commit`, `$live`, `TX_CONFLICT` | `#[cell(transaction)]` / `transaction(serialize, conflict = "warn")`, `cx.commit()`, `cx.live_state()`, `Code::TxConflict`; reads found in the method's body (a blind write never conflicts; `self` passed on reads all) | ✅ |
| Lifecycle hooks | `onInit`, `onDestroy`, `onStopping`, `onStop` | `#[on_start]`, `#[on_stop]`, `aio::on_stopping`, `aio::on_stop` | ✅ |
| App-wide start hook, `fatalOnStart`, `isolate`, `onRestore`, `getFullState` | yes | `Cells::on_start`, `fatal_on_start`, `AIORT_ISOLATE`, `on_restore`, `view` | ✅ |
| Disable, breaker | resets state; `circuitBreaker` | keeps state; `Cells::trip_after` | 🔄 |
| Call hook | `beforeReduce` (may change; sees the state and the request) | `before_call` (lets in or refuses), `rewrite_call` (may change); the call and its `Caller`, no state, no IP or headers | 🔄 |
| Worker cells | `worker: true` | not needed: native code, `spawn_blocking` | 🔄 |
| View-only state | `scope: "client"`, `persist: "none"` | `#[cell(persist = false)]`, or a `Global` | ✅ |
| Visibility | `visible`, `forUser`; a hidden read throws | `#[cell(visible = f)]`; a `Hidden<T>` field it hides (`Hidden::withheld()`) and `#[server]` fields panic when read on that copy, naming the field | ✅ |
| Plugins | `definePlugin` (`onAction`, `allowedOrigins`, …) | `aio::plugin::Plugin` (`on_action`, `allowed_origins`, …) | ✅ |
| Retired cells | `am persist --drop` | `ax persist --drop`, `#[cell(retire = [..])]` | ✅ |

**Methods, effects, scheduling**

| Feature | aio | aio-rs | Status |
|---|---|---|---|
| Async methods | `async` on a draft; `await cell.method()` gives its value or throw | `async fn`, split at its awaits (in branches and loops too); `h.m(..)?.await` gives its end | ✅ |
| Effects and modes | `$do`, `concurrency` (`first` joins a call with the same arguments) | `cx.run(Effect::new(..))`, `Mode`; `#[mode(newest \| first)]` on async methods (`first` joins any running call of the method) | 🔄 |
| `ttl`, `long`, a method's own await timeout | `ttl` reuses an answer for every caller | `#[ttl(ms)]`, an answer kept per network caller, never handed to another; `#[long]`, `#[long(ms)]`; no `"warn"` (a ceiling cuts off) | 🔄 |
| Retries, timeouts | `call({ timeoutMs, retries })` | `aio::effect::retry`, `Effect::timeout`, `cx.backoff` | ✅ |
| Schedules | `every`, `cron` (UTC), `after`, `backoff`, another cell's action | `#[every]`, `#[cron]` (UTC; `[app] tz = "local"`), `cx.after`, `cx.backoff`, `cx.on::<Other>()` | ✅ |
| Clock, ids, randomness in methods | allowed, linted | `cx.now()`, `cx.id()`, `cx.rand()` | 🔄 |
| CPU work | `blocking(id, fn)`, `cancel` | `aio::effect::blocking(key, f)`, `cancel_blocking` | ✅ |
| Notifications | `notify()`; a warning when a signed-in user's call notifies everyone | `cx.tell(..)`, no such warning; a web page shows the browser's own, asking for the permission | 🔄 |
| Pending calls | `$pending()` | `h.pending(..)`, `use_pending` | ✅ |

**Persistence**

| Feature | aio | aio-rs | Status |
|---|---|---|---|
| SQLite store | `state.db` | the same file | ✅ |
| Event log, history | journal (opt-in) | every commit an event | ➕ |
| Write debounce | every write, `persistDebounceMs` | `#[coalesce]` methods only, `[data] coalesce_ms` | 🔄 |
| Excluded fields | `persist: { include, exclude }`, `persist: "none"` | `#[cell(persist(include, exclude))]`, `#[transient]`, `save = f`; no per-cell `"none"` (`Handle::memory()`) | 🔄 |
| Store layout | `single` / `multi` | log + snapshots; aio's layout kept | 🔄 |
| App-wide `persist: false`, `--no-persist` | yes | `[data] persist = false`, `--no-persist` | ✅ |
| Data from a newer build (rollback) | boots, parks `__downgraded:<cell>` | the same; refused only when it doesn't fit | ✅ |
| aio's journal tail after a crash | replayed | replayed by the port | ✅ |
| Migrations | `version`, `onMigrate` | `#[cell(version, migrate)]` | ✅ |
| Integrity check | `checkIntegrityOnBoot`, from a rolling snapshot | every boot, with recovery from the good copy (daily, or every `[data] snapshot_every`, each copy checked) | ✅ |
| Quarantine | a cell whose log no longer folds runs at its last snapshot, refusing writes (prod); dev refuses the boot | the same: a release build quarantines, a debug build refuses (`Store::quarantined`) | ✅ |
| Backups, restore | `am backup`, `am restore` | `ax backup`, `ax restore` | ✅ |
| Timed backups | a recipe | `[data] backup_every`, `backup_keep` | ➕ |
| State export over HTTP | `/__aio/snapshot` (dev) | `/__aio/snapshot` (dev), `Handle::export_snapshot`, `load_snapshot` | ✅ |
| Bound tables | `db:` tables (flat rows) | `#[table]`; bools, lists, nested values in `BOOLEAN`/`JSON` columns | ➕ |
| Schema builder | column helpers | `aio::columns` (`pk`, `text`, `refs`, …) | ✅ |
| Raw SQL, live queries | `createDB`, `reactiveDB` | `aio::db::Db`, live queries woken by any `Db` of the file | ✅ |
| Blobs | `app.blobs`, `stream` | `aio::blobs::Blobs`, `stream` | ✅ |
| Size guard | 1 MB warn, 16 MB error | the same, unless `[budgets] state` | ✅ |
| Homes, profiles, instances | `--home`, `--profile`, `--instance` | the same rules | ✅ |
| App launch flags | `appFlags` | `[app] flags`; a near-miss of a declared or aio flag is refused with the one meant | ✅ |

**Sync and offline**

| Feature | aio | aio-rs | Status |
|---|---|---|---|
| Shared cells | `sync`, broadcast | `#[cell(shared)]` | ✅ |
| Optimistic calls | `useOptimistic`, local-first | built in: copies predict; no general `use_optimistic` hook | 🔄 |
| Access per method | `access` | `#[client(role, allow)]` | ✅ |
| Merging | per-field view merges, `onConflict`, HLC | methods replayed on the server in one order (no HLC needed); `Remote::merge` (`Text`, `Counter`, `SetAdd`, `Keys`, `SetRemove`), `on_conflict` | ✅ |
| Offline queue | 500 per `sync:` cell, in the page's storage, `retention` | 500, refuses when full, `retention`, on disk if asked: written in order, kept at a drop, `aio::shutdown` and the exit | ✅ |
| Array patches | Immer `add`/`remove`, narrowed | `insert`/`del` per item, else the array | ✅ |
| Throttles | `syncIntervalMs`, `fullStateThreshold` | `sync_interval_ms`, `full_state_percent` | ✅ |
| Path subscriptions | yes | ❌ (split cells) | ❌ |
| Binary side channels | a raw WebSocket route, a recipe | `Route::socket` with framing, drop or close, gap checks | ✅ |
| Server functions, routes | `serverFns`, `routes`, streamed `Response` | `aio::fns::ServerFn`, `aio::http`, `Reply::stream` | ✅ |
| One socket, every cell | yes | `Http::cell`: one `/ws`, port and socket for every cell, a link per cell on it; an aio page sees every shared cell | ✅ |
| Peer identity, door hooks | `resolvePeer`, `localConnectHooks` | `Http::resolve_peer`, `Http::door_links` | ✅ |
| Pairing, discovery, peers | PIN, CPace, UDP 8099 on every interface's broadcast, one machine listed once, a probe nobody took an error (macOS's Local Network named), `x-aio-peer` | the same | ✅ |
| Calling another app | `peer()`, `call`, `stream()`, an app-wide `describe()` | `Peer::find`, `AioLink`; `Remote::stream` with `cx.part` in the method, `describe` one cell at a time | 🔄 |
| Server limits | `maxConnections`, `wsLimits` | `[server.limits]` | ✅ |

**Auth and security**

| Feature | aio | aio-rs | Status |
|---|---|---|---|
| Accounts, roles, TOTP, OIDC | `auth`, `sessions`, `auth.totp`, `auth.cookie`, `requireVerified` without `sendMail` refused, `SignIn` with SSO | `[accounts]` (`totp`, `cookie`; `signup` with `require_verified` needs `Http::mail`); `SignIn` with SSO in a page, and in a desktop or phone app through the browser (RFC 8252 loopback, confirmed in the browser first) | ✅ |
| Sessions without accounts | `sessions: true` | `[accounts] sessions`, `auth::serving()`'s `issue` | ✅ |
| Users tokens re-checked on open sockets | every 5 s, a changed role pushed | every 5 s, and at once on a revoke; a changed role pushed to the open socket | ✅ |
| `users`, `resolveUser` | a map, an async hook with the state and extra fields; service keys beside `auth: true` | `Users::map`, `Users::resolve_with` (async, the state through captured handles, `Known::extra` read by routes); service keys beside `[accounts]` | ✅ |
| TLS | `--tls-cert`/`--tls-key`, `--no-tls`, own LAN CA | the same flags, `[server] cert`/`key`, a machine root for the LAN | ✅ |
| Health and metrics beyond loopback | open without a key | a credential, always (fail closed); the page log (`/__aio/log`) too | ➕ |
| Security headers | yes | `[server.security]` | ✅ |
| Origins, proxies | `allowedOrigins`, `trustProxyHeader` | `[server] hosts`, `trust_proxy` | ✅ |
| CORS, passkeys | none | none | ✅ |
| Share link at boot, firewall line | yes | `share:` (names the key file) and `firewall:` lines; the link with the key: `ax profile <app> --link` | ✅ |
| Outbound network list | `nativeFetch` | `[capabilities] network` | ✅ |
| Secrets in logs | `redactActions`, `"vault:*"` | `#[redact]`, `#[cell(redact)]`, masked logs | ✅ |

**UI: views, kit, forms, routing, theming, animation**

| Feature | aio | aio-rs | Status |
|---|---|---|---|
| Renderer | AIR, TSX | `aio::ui`, `rsx!`, drawn by air (fine-grained, a body runs once, holes follow state) | 🔄 |
| Field-level reads | one signal per cell | `part(\|c\| &c.field)` | 🔄 |
| Hooks | AIR and React names | `signal`, `memo`, `effect`, `watch`, `on_change`, `untrack`, `once`, `on_cleanup`: AIR's names, no dependency arrays | 🔄 |
| Server rendering, hydration | yes | ❌ (`ax preview --html`) | ❌ |
| Code splitting, `lazy` | yes | ❌ (`Defer` with `load`) | ❌ |
| Other view libraries, `h()` | `aio/state-core`, React compat | ❌ (islands) | ❌ |
| Islands, React islands, `<Browser>` | yes | `WebIsland`, `Island::react`, `kit::Browser` | ✅ |
| Kit components | `aio/ui` | `aio::kit`, the same names; some props that take any view in aio take a `String` (`Field` hint, `EmptyState` icon, …) | 🔄 |
| Side tooltips | no (`top`, `bottom`) | `Placement::Left`, `Right` | ➕ |
| Keys | `onGlobalKey`, `REPEATABLE_KEYS` | `use_global_key`, `Chord::repeating` | ✅ |
| Long lists | `useVirtualList` | `kit::VirtualList` | ✅ |
| Forms | `useForm` + Standard Schema, touched on blur | `use_form` + a Rust check; `bind`, touched on blur, `reset_field`, `use_field_array`, `use_on_change` | 🔄 |
| Routing | History API, address bar | history in the app; in a browser the address bar, Back and a reload follow it; typed `routes!` | ✅ |
| Head | `useHead` | `aio::head::use_head` | ✅ |
| Theme, hue, CSS variables | `ui.theme`, `hue`, `neutral`, dark follows the OS | `theme_css(hue)`, `--aio-*`, `[ui] neutral`, `[ui] scheme` (the OS's by default) | ✅ |
| Scoped CSS classes | `css` tag | `aio::ui::css` (no duplicate-class warning) | 🔄 |
| Live CSS in dev | yes | yes, plus `[build] css` toolchains | ✅ |
| Transitions | `TransitionFn`, shared `options` | CSS keyframes or `Motion::of`, a `Motion` each, no box | 🔄 |
| Runtime a11y and contrast checks | dev mode | the running dev page's, said once each in the log; `TestUi::a11y()`, `contrast()` | ✅ |
| Window chrome | `ui.chrome`: `standard`, `themed`, `none` | `Window { chrome }`: `Chrome::Standard`, `Themed`, `None` | ✅ |
| App menus | macOS's standard ones only | the same | ✅ |
| Redux DevTools | yes | ❌ (`TimeTravelPanel`) | ❌ |
| Worker bundling | yes | ❌ (`spawn_blocking`) | ❌ |

**Testing**

| Feature | aio | aio-rs | Status |
|---|---|---|---|
| Cell tests | `testCell`, `bootCells` | `TestCells` (virtual time, seeded fuzz, `fuzz_methods`, `reset`, `last_effects`, `budget`) | ✅ |
| UI tests | `testUI` on happy-dom | `TestUi` on the real engine | ➕ |
| Virtual clock under the page | `ui.advance` | `TestUi::advance` (the views' timers; schedules: `TestCells::advance`) | ✅ |
| Components by name and key | `ui.find(c, key)`, `surface()` tree | `ui.find(c, key)`, `within`, `surface_tree()` | ✅ |
| Typed UI clients | `am testgen` | `ax testgen` (a Rust name table) | ✅ |
| Server and multi-client | `testServer`, `testMultiClient` | `TestServer`, `Sim` | ✅ |
| Patch assertions | `patches`, `waitForPatch`, `dispatchAll` | `testing::patches` (`all`, `until`), `call_all` | ✅ |
| Several apps | `testApps` | `TestApps` | ✅ |
| Cassettes | `openCassette`, `wrap` | `Cassette` (`wrap`; stricter, `AIO_CASSETTE=rerecord`) | 🔄 |
| Page fetch stubs | `testUI({ fetch })` | `Mount::fetch(host, answer)` | ✅ |
| Boot smoke in a test | `smoke()` | `testing::smoke(bundle)`, `ax smoke` (also a `.deb`'s menu entry, page errors) | ✅ |
| Proving the tests | `check:mutations`, `check:vacuous` | `scripts/mutants` (source and a built `.deb`), `ax lint` (no-assertions, self-comparison) | ✅ |
| Browser tests | `testBrowser` | `TestBrowser` | ✅ |
| Lint | `aiol` | the compiler + `ax lint` | 🔄 |
| Labs | VMs and containers it boots | the same images (`ax lab --boot`), or a machine over ssh | ✅ |
| CLI run capture | `testIO()`, the exit too | `TestIo::run(argv, main)`: its output and exit code | ✅ |
| Smoke page checks | every asset URL answers 200, guest preloads attachable, no `REFUSED` line | every file the page names or loaded and every built-in asset answers 200; each guest preload attached in a sandboxed guest; no `REFUSED` line | ✅ |
| Smoke coverage | `--smoke=strict`, "not smoke-tested" rows | `--smoke-strict`, `not smoke-tested` rows | ✅ |
| API snapshot gate | yes | `scripts/public-api --check` in `./ci.sh` | ✅ |

**Debugging**

| Feature | aio | aio-rs | Status |
|---|---|---|---|
| Time travel | dev only, 2000 entries; `skipActions`, `timeTravel: false` | the whole store, release builds too; `#[skip_travel]`, `[history] travel = false` | ➕ |
| Time-travel panel | Ctrl+. in every dev app | Ctrl+. in every dev build's window: each `client!` cell's history; `TimeTravelPanel` to place it yourself | ✅ |
| Replay, record | `am replay`, `am record` | `ax replay`, `ax record` (writes a Rust test; of one cell or, as aio, of every cell the range touched) | ✅ |
| Logs, errors, health, vitals, budgets | yes, `logging.dir` | `ax logs`, `ax errors`, `ax health`, `ax vitals`, `/__aio/vitals`, `[vitals]`, `[perf]`, `[budgets]`; logs always in `<home>/logs` | 🔄 |
| Boot report | config, cells and routes at start | every setting with who decided it, `platform`, `updates`; a second on: `cells` (with data versions), `routes`, `serverfns` | ✅ |
| Diagnostic bus | `onDiagnostic`, severity, fix; a typed `onError` with many codes | `report::subscribe`: `severity`, `tip`, `code()`, `cell`, `call`, `numbers`; `vitals:<layer>`, `degraded-recovered:<part>`; `aio::Code`'s aio codes, `error_summary` | ✅ |
| Memory pressure hook | `onMemoryPressure` (each sample under pressure, `trend`), `interval` | `health::on_memory` (the same, `trend`), `[memory] every` | ✅ |
| Prometheus metrics | uptime, memory, clients, cells, broadcast bytes and messages | the same, and calls, degraded parts, gauges and budgets | ✅ |
| Vitals per client | `clients` (liveness), `payloadStats`, `cellSizes`, `drainRate`, `circuitBreakers` | `clients` (liveness and payloads in one row), `cellSizes`, `drainRate`, `breakers` | ✅ |
| Per-run log files | `debug.log` per run | `[log] per_run` | ✅ |
| Correlation ids | yes | `call=<id>` on a call's lines, its timeline entry and reports | ✅ |
| Error box with state | yes | reports carry the state | 🔄 |
| Heap snapshots | V8 | not needed: no GC heap; `ax heap` | 🔄 |
| Problem reports | `feedback: true` | always on, no arguments | 🔄 |
| Manager UI | `am ui` | `ax ui` (the same tabs) | ✅ |

**Build, targets, dev**

| Feature | aio | aio-rs | Status |
|---|---|---|---|
| Desktop | electron | Linux, Windows, macOS on a shared runtime | ✅ |
| Web, PWA | `web` | `ax build --web` | ✅ |
| Android | `android` | APKs (3 CPUs, universal optional, debug), built on Linux, macOS, Windows; a port installs over aio's APK, its state with it | ✅ |
| Service, CLI | `server`, `cli` | `mode = "server"`, `cli` | ✅ |
| Thin clients | electron-, android-, cli-client, with a connect page | `mode = "client"`, its connect page on desktop and Android (an address, the LAN's servers, a pairing code, recent servers; no code: no key sent), `--server-url`, `--connect`; the server picked needs no `[capabilities] network` entry; a CLI's client says the notifications it gets | ✅ |
| Changed server certificate | pin-proof, re-pinned; else opened without the key; `strictPin` refuses | pin-proof (never for a weak key), re-pinned; else refused, the key never sent; `[client] strict_pin` refuses | ✅ |
| Run's client | `--client=electron\|browser\|cli\|server-only` | `--client=window\|browser\|server-only` (`electron` is `window`; a CLI is its own binary) | ✅ |
| Child windows | `openWindow`: sandboxed, `sandbox: false` with `unsandboxedChildWindows`; its own origin keeps every permission | the same opt-in (`Web { unsandboxed_windows }`, `Child { unsandboxed }`, in a session apart from the sandboxed ones); its own origin holds only what `Child { permissions }` names | ➕ |
| Guest refusals | `aio:guest-preload-refused`; each refusal kind capped at 32, a denial naming its fix | the same, and `GuestEvent::PreloadRefused` | ✅ |
| DevTools in a release | F12 and `--cdp` always | only where the app says `Window { devtools: true }`; never from the environment | ➕ |
| iOS | ios-client | ❌ | ❌ |
| Dev reload | patch, reload, restart (~3 s) | hot-patch, state kept: `ax dev` reloads markup (4 ms) and patches Rust (~0.3 s) itself; a changed type restarts in ~2 s (Linux, macOS, Windows); the in-page overlay shows build failures, page errors and the app's own problems; an `#id` the CSS styles and no element has is warned | 🔄 |
| Dev watcher | 100 ms debounce, 500 ms ceiling, editor files ignored, served files watched, health sentinel, restart drain and storm guard, files named | the same (cargo's dep-info for the files a build read); an unchanged binary isn't restarted | ✅ |
| Dev session | `am start` of a dev run, `--no-watch`, `--verbose`, `--title`, `am stop` ends it | `ax dev --detach`, `--no-watch`, `--verbose` (`AIORT_LOG`), `--title` (`AIORT_TITLE`), `ax stop` ends it, `ax status` says it waits for a fix | ✅ |
| Hot reload in web, Android | page reload | 🔄 the dev APK reloads the page each edit (`ax dev --android`); the web build reloads its tabs | 🔄 |
| Single instance, takeover | `singleton`, `takeover` | always one per home, `--takeover`; a launch while the running one stops starts after it | ✅ |
| Release build to try | none (a bundle build) | `ax build --quick`: no LTO, a one-line change in ~2 s, never shipped | ✅ |

**Release and updates**

| Feature | aio | aio-rs | Status |
|---|---|---|---|
| Signed releases | `ship keygen`, `publish` | `ax keygen`, `ax release` | ✅ |
| Delta updates, channels, data gate, rollback | yes; AppImage deltas, the same version's new build | yes, AppImage deltas too; only a newer version is offered | 🔄 |
| Stepping-stone releases | `--min-from`, `minFrom` signed | `ax release --min-from`, signed, not past the data gate's buttons either | ✅ |
| Release sources | https, `file:///`, `github:`, a git repository | the same | ✅ |
| Offer details | `reason`, `migrates`, `progressOf` | `reason`, `migrates`, `progress_of` | ✅ |
| Build in the manifest | `buildNumber`, `commit` signed | `commit` signed with `--commit`; the build number is the version's third part | ✅ |
| Custom update runtime | `installUpdatesRuntime`, the cell kept | `aio::update::install`, the cell kept, the built-in checks out | ✅ |
| `updates` cell on clients | synced to every client; `--expose`d: signed-in only | synced to every client, server paths hidden; installs and channel an admin's | ✅ |
| Update veto | `canApply` (awaited, every install) | `can_apply` (plain, not for `auto` at start) | 🔄 |
| GitHub workflow | — | from `ax release` | ➕ |

**CLI: `am` and `ax`**

| Feature | aio | aio-rs | Status |
|---|---|---|---|
| App control | `start`, `stop`, `kill`, `restart`, `status` | the same | ✅ |
| State and history | `state`, `snapshot`, `migrations`, `replay`, `timetravel` | the same | ✅ |
| Calls | `dispatch` (`k=v`, `--body`, `--wait`) | `ax call` (the same) | 🔄 |
| Inspection | `surface`, `trigger`, `shot`, `eval`, `preview`, `sql`, `heap`, `cost` | the same | ✅ |
| Pairing, trust | `discover`, `profile`, `pair`, `auth`, `trust` | the same (`ax pair <app>` makes a pairing code, `ax pair <app> <host:port> <code>` pairs this device) | ✅ |
| `where`, `check` | yes | the compiler | 🔄 |
| `persist`, `testgen`, `timeline`, `top`, control REST API | yes | `ax persist`, `ax testgen`, `ax timeline`, `ax top`, `ax control` (loopback HTTP) | ✅ |
| `add cell\|server` | yes | ❌ (written by hand) | ❌ |
| `keygen`, `release`, `update`, `smoke`, `install`, `lint`, `test`, `events`, `components`, `adopt`, `import`, `runtime`, `setup` | — | yes | ➕ |

**Tooling**

| Feature | aio | aio-rs | Status |
|---|---|---|---|
| Templates | `am create --template=counter\|todo\|cli\|canvas` | `ax create --template app\|todo\|canvas\|cli\|service`: the todo list (add, tick, filter in the window, clear done) and the canvas (a pure `step` tested, a frame drawn per move) as aio's, each with its tests | ✅ |
| Agent brief | `am agent` | `ax agent`, `AGENTS.md`/`CLAUDE.md` in new apps | ✅ |
| Framework version per app | `dep/aio` link, `am pin`, `am fix` | aio-rs from git at a release tag in `Cargo.toml`, the commit in `Cargo.lock`: a clone builds as it is; `ax create --aio-version`; `ax pin` says what Cargo.toml asks for, what Cargo.lock built, this ax and the releases (drift: exit 1), `ax pin <v>\|main\|latest\|<checkout>` moves it, `--major`, `--json`; no preflight of removed APIs (`cargo check` names them) | ✅ |
| Upgrading the CLI | `am upgrade`, `am uninstall` | `ax upgrade` (the release's checked install script, or a checkout), `ax uninstall` | ✅ |
| Install | `install.sh` (Deno + a clone) | prebuilt `ax` (Linux, Windows, macOS) or source | 🔄 |
| API reference | `api-reference.md` | `cargo doc -p aio-rs` | ✅ |

## Plans

- **Done in 0.2.0: aio-rs's own renderer, "air", is the only one.** A
  fine-grained renderer of aio-rs's own: a cell's fields as signals,
  direct DOM edits for what changed, and a lean macro so rebuilds get
  faster ([design](design/air.md)). Apps are written against `aio::ui`;
  what a 0.1 app changes is in the migration guide's section 8, [0.1 → 0.2](migration/README.md).
- **Next: key-to-paint at or under aio's by the frame's stamp on Home
  and Tab** ([key to paint](#key-to-paint)): the rows that enter drawn
  faster, and the page's messages to the app shorter.

## Moving

- [Migrating from aio to aio-rs](migration/README.md): tables, a worked
  example, the speed tricks, a checklist.
- [aio app data](migration/data.md) and [aio wire formats](migration/wire.md).
- [Why aio-rs, not plain Rust](basics/why.md).

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
| Wire compatibility | protocol 3 on `/ws`, NDJSON on a local socket | its own wire 2; with feature `compat` also aio's protocol 3 on `/ws`, aio's local socket and lock records (`am instances`, `am state`, `am dispatch`); pairing, discovery, peers and blobs in aio's formats always ([below](#the-wire)) |
| Electron store | per user, `apps/` records, `am prune` | the same store, the same rules: one copy per version for both ([below](#the-electron-store)) |
| Targets | browser, electron, android, web, cli, server, server-app; thin clients electron-client, android-client, ios-client, cli-client | desktop (Linux, Windows, macOS; macOS built on any host), web (wasm, PWA), Android, service, CLI; a thin client is `[app] mode = "client"`; iOS: an Xcode project of a WKWebView thin client (`--ios`), or the server's pairing page added to the Home Screen ([targets](build/targets.md)) |
| Install | `install.sh`: installs Deno, clones aio, puts `am` on `PATH` | a prebuilt `ax` (Linux x86_64, Windows x86_64, macOS arm64 and x86_64): `curl \| sh`, `irm \| iex`, from a folder offline, each archive's SHA-256 checked; or from source; an app takes aio-rs from git at a release tag ([setup](basics/setup.md)) |
| Dev loop | `am dev`: `App.tsx` patched, other edits reload the page, a cell edit restarts the app (0.4-0.8 s; 1.3-1.8 s for a ~74 000-line app) | `ax dev`: Rust hot-patched with the state kept, `rsx!` markup at once, CSS live; a rebuild and restart when a patch can't take it ([dev](build/dev.md)); a ~74 000-line app's view edit 0.5 s in page crates, 5.5 s in one crate ([what an edit costs](build/README.md#what-an-edit-costs)) |
| Tests | `testCell`, `testUI` (happy-dom), `testServer`, cassettes, `deno test` | `TestCells`, `TestUi` (the real views in the runtime), `TestServer`, cassettes, `cargo test` ([testing](testing/README.md)) |
| Maturity | beta (1.0.27-beta): the 1.x surface frozen, additions only, an API snapshot gate in CI | early alpha, fourth release (0.4.0); a minor version may break ([versions](basics/versioning.md)) |

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
- **`am` → a running aio-rs app, without `/ws`.** With feature `compat`
  the app writes aio's lock record for itself (aio's lock folder, aio's
  key), so `am instances` lists it, and answers on aio's local socket
  (NDJSON lines of aio's envelope): `am state` and `am dispatch`
  (`--as-server` too) reach it through its own doors, with the same gates,
  behind the `X-Aio-Control` key `am` reads from `<home>/data/control.key`.
  As aio's trojan, only a debug build answers those; a release build
  answers `/__aio/health` ([ax](clients/ax.md#what-am-sees-of-an-aio-rs-app)).
- **An aio-rs app → an aio app on this machine, as itself.**
  `aio::compat::AioPeer` is aio's `peer()`: found by aio's lock record,
  reached on its local socket (a named pipe on Windows), proven with
  `x-aio-peer` as this app, so the aio app's `resolvePeer` sees its id;
  `call` waits as long as the aio app takes (aio's `ackTimeoutMs: 0`).
  An aio app reachable only on a TCP port is refused, said
  ([sync](state/sync.md#other-apps-other-machines)).
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

- Without feature `compat`: no lock record and no aio socket, so `am
  instances` doesn't list the app (`ax apps` does).
- aio's control routes past `state`, `client-cells` and `dispatch` (its
  time travel, `am shot` over the trojan): `ax` has each of them.
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
  shows in ~8 ms, a stylesheet in ~10 ms, a Rust edit in ~0.3 s
  ([the edit loop](#the-edit-loop)); one big crate's edit is the
  exception ([below](#one-big-crates-edit)).
- **Code splitting that splits.** A web build's lazy views are chunks
  the page fetches when they first show; aio's `lazy` defers evaluating
  a module of its one bundle ([islands](ui/islands.md#lazy-views-lazy)).
- **Smaller first steps on an existing machine.** One native binary per
  app (5.0 MB for the template), the window runtime shared by every app.

### Key to paint

Keydown to the first frame that shows it, in ms, p50 / p95, by two
clocks. *Stamp* is the time a frame is given (`requestAnimationFrame`'s
argument): the vsync it was planned for. *Ran* is when that frame's work
ran, the earliest it can be on screen. aio renders in the page: a key
that holds the page for 200 ms still gets the frame planned before it,
stamped early.

Since 0.4.0 an aio-rs window does the same: the page waits for a key's
edits (at most 50 ms) and applies them in its key handler, so the frame
planned when the key came shows it ([the wire](design/air.md#the-wire)).
Before, the answer reached the page after the page had drawn that frame,
and showed a frame later.

**The bench app** (`crates/air-bench/keys`: the same app written for
both, two panels of 1000 and 200 rows, a window of 60 rows around each
cursor), both release builds on Electron 44.9.3, Xvfb, keys and frames
read by `keys.ts`; 10 sessions of each side in turn, load 3-13 on the
machine's 12 cores, every key of every session pooled (`table.py`): a
hold is 60 keys, 33 ms apart; End and Home 20 presses a session, Tab 40.
Δ is aio-rs minus aio at p50, with its 95% interval (a bootstrap): below
0 aio-rs is ahead, an interval across 0 is a draw. CPU per key: every
thread of the app's processes, less their idle rate.

| Key | n | aio stamp | aio-rs stamp | Δ stamp | aio ran | aio-rs ran | Δ ran | CPU per key, aio / aio-rs |
|---|---|---|---|---|---|---|---|---|
| Hold ↓ | 600 | 7.8 / 15.0 | 7.7 / 15.4 | -0.1 (-1.1, +0.8) | 8.8 / 16.2 | 8.7 / 16.1 | -0.1 (-1.0, +0.9) | 6.5 / **4.6** |
| Hold ↑ | 600 | 8.1 / 15.3 | 7.6 / 15.3 | -0.5 (-1.4, +0.7) | 8.8 / 16.2 | 8.5 / 16.0 | -0.3 (-1.4, +0.6) | 5.3 / **4.4** |
| End | 200 | 8.0 / 15.2 | 8.8 / 15.3 | +0.8 (-1.4, +1.8) | 9.4 / 16.2 | 9.6 / 16.3 | +0.2 (-1.7, +1.7) | 15.3 / **10.9** |
| Home | 200 | 7.3 / 15.2 | 6.5 / 14.8 | -0.8 (-2.3, +0.7) | 8.6 / 16.2 | 7.6 / 15.9 | -1.0 (-2.5, +0.4) | 11.1 / **9.5** |
| Tab | 400 | 8.3 / 15.3 | 8.0 / 15.3 | -0.3 (-1.4, +0.7) | 8.8 / 16.0 | 8.7 / 15.9 | -0.1 (-1.5, +0.7) | 5.4 / **4.6** |

On aio-rs 0.3.0's wire (5 sessions) the same app was 8.8-10.0 ms by the
stamp and 10.0-11.6 ms ran at p50, 16.1-17.2 / 17.1-19.1 at p95.

- Both show a key in the first frame after it: a key lands anywhere in a
  16.7 ms frame, so p50 is ~8 ms and p95 ~15 ms for both, and that is
  the floor. Every Δ is within its interval of 0: a draw on every key,
  aio-rs's p50 lower on 8 of the 10, with less CPU on every key.
- Below the floor neither can go: a frame that started between the
  key's moment and its handler shows it a frame later on both sides
  (1 to 2.5 keys in 100 on each, measured).
- Where an End's 3.9 ms in the page go with aio-rs: 0.6 ms of messages
  through the window's main process (each way 0.2-0.3 ms), 0.5 ms of
  the app's handler, 0.3 ms applying its edits, ~0.9 ms of layout for
  the scroll to the cursor; aio spends ~1.5 ms in its renderer and the
  same layout.

**A large app** (aio-rs 0.3.0, before the answer above; the app of
[measured](#measured), its 30 big fields held as
[`aio::Big`](performance.md#7-share-big-fields-aiobig); 5 sessions each,
load 11-12). Its work per key is heavy in aio: End and Home hold aio's
page for 200 ms or more.

| Key | aio stamp | aio-rs stamp | aio ran | aio-rs ran | CPU per key, aio / aio-rs |
|---|---|---|---|---|---|
| Hold ↓ (60 keys, 33 ms apart) | 13.1 / 25.0 | **10.3 / 18.4** | 23.2 / 38.7 | **12.1 / 20.7** | 35.0 / **12.2** |
| Hold ↑ | 13.9 / 26.9 | **10.9 / 18.5** | 26.4 / 49.5 | **13.3 / 21.5** | 35.5 / **14.4** |
| End, 1000 rows | 13.5 / 20.4 | **13.5 / 19.6** | 217.6 / 333.6 | **19.6 / 23.4** | 380 / **48** |
| Home, 1000 rows | **9.1 / 16.8** | 13.4 / 19.7 | 226.3 / 394.6 | **19.4 / 26.8** | 315 / **17** |
| Tab | **9.8 / 18.7** | 11.6 / 19.0 | 26.5 / 46.6 | **17.8 / 23.0** | 80 / **58** |

- By when the frame ran, aio-rs was ahead on every key, End and Home
  ten times over, with a fifth to a third of aio's CPU. By the stamp,
  aio was ahead on Home and Tab: the frame planned before aio-rs's
  answer reached the page could not show it, which is what the answer
  above fixes.
- Where the time went before: each step compared the 1000-row state
  deeply (4-9 ms; 0.03 ms with `Big`), a row's static holes were boxed
  and freed (now run once, never boxed), and the rows that left were
  freed before the frame went out (now after).

### The edit loop

`ax dev` on the template app beside `am dev` on aio's (aio-rs 0.3.0,
aio 1.0.25-beta, one Linux machine, Xvfb, one script driving both,
2026-11-07; the rows aio-rs 0.4.0 changed measured again the same way on
2026-11-08; the method and every row are in
[comparison](comparison.md#the-edit-loop-next-to-aios-am-dev)):

| | `am dev` | `ax dev` |
|---|---|---|
| First start, nothing built yet | 2.6 s | **1.9 s** on a fresh install: `ax setup` (the installers' last step) builds aio-rs before it returns (~30 s once per machine), `ax create` the app's own crates in the background (~1.5 s); 2.6 s median without the latter (`--no-warm`); 24 s with neither (no terminal, `AIORT_NO_WARM=1`) |
| Start again, nothing changed | 1.1 s | **0.33 s** (the last build runs as it is) |
| Start after an edited session | 0.8 s | **0.35 s** (built behind the session) |
| Start, sources changed with `ax dev` off | 0.8 s | **0.42 s** (the last build started at once, the edit patched in as its window opens; 1.3 s before) |
| A stylesheet saved → on the window | 0.31 s, a reload | **0.01 s**, state kept |
| Markup saved → on the window | 0.50 s | **0.008 s**, state kept |
| View logic saved → on the window | 0.41 s | **0.27 s** hot-patched, state kept |
| A cell method saved → on the window | 0.84 s, the app restarted | **0.40 s** hot-patched, state kept |
| A changed type (a cell's field) | a restart | **2.1 s** rebuild and restart, state back from the log |
| A compile error | – | on the window in 0.13 s, the app running on; its fix 0.6 s |

`ax create` at a terminal builds the new app in the background, so its
first `ax dev` is a start, not that first build.

The web build and the Android dev page (`ax dev --web`, `--android`),
beside aio's page reload:

| | `am dev` | `ax dev --web` |
|---|---|---|
| A stylesheet saved → on the page | 0.31 s, a reload | **0.02 s**, state kept (never shown before 0.4.0) |
| Markup saved → on the page | 0.50 s, a reload | **0.01 s**, state kept (1.0 s, a build and a reload, before) |
| View logic saved → on the page | 0.41 s, a reload | **0.18 s**, hot-patched, state kept (0.58 s, a build and a reload, before) |
| A cell method saved → on the page | 0.84 s, a restart | **0.17 s**, hot-patched, state kept (0.76 s, a build and a reload, before) |

Rust in the page's crate is hot-patched in the browser: the page loads
the patch into its own memory and function table, wasm-bindgen doesn't
run. An edit a patch can't take (a new JS import, another crate) is a
new bundle and a reload.

The Android dev page is the same page in the device's WebView, patched
the same way: on an emulator, save to screen, markup 0.01 s, a stylesheet
0.02 s, view logic 0.20 s, a cell method 0.29 s, each with the state kept.

## Where aio is better

### One big crate's edit

aio bundles the app again and reloads the page (a cell edit restarts
it); aio-rs recompiles the crate an edit is in. The same-size app in
each (660 generated modules of the same shape, ~74 000 lines, 2.7 MB of
source), one session on one machine, 2026-11-08:

| Edit | aio `am dev` | aio-rs `ax dev` |
|---|---|---|
| a small app (the template) | 0.41 s (view), 0.84 s (cell) | **0.27 s**, **0.40 s**, hot-patched |
| ~74 000 lines in one crate, a view's logic | **1.6-1.8 s**, a bundle and a reload | 5.2-6.1 s, hot-patched (rustc 4.4-5.3 s of it; 7 s before 0.4.0) |
| ~74 000 lines in one crate, a cell | **~1.3 s**, a restart | as above |
| ~74 000 lines in page crates (`ax add <app> page`), a page's view logic | 1.6-1.8 s (its one bundle) | **0.47-0.52 s**, hot-patched, state kept (1.9 s before 0.4.0) |
| … an edit in the shared kit every page calls | 1.6-1.8 s | **1.0-1.9 s** (8–14 s before 0.4.0) |
| a first (cold) build | **none** | 15 s (template) to ~60 s (big app) |

One big crate stays slower than aio: rustc reruns the whole crate's
front end (macros, names, metadata) for any edit in it, and the `rsx!`
expansion is most of that crate (the same 74 000 lines of plain Rust:
2.6 s); no rustc setting on stable moves it (codegen units 64 or 1024,
no debug info, no metadata file: all within 4.4-5.1 s). A markup edit is
instant whatever the size: nothing compiles.

So a big aio-rs app is kept fast to edit by keeping its views in page
crates: `ax add <app> page <name>` makes one, `ax lint` hints at a crate
past 15 000 lines, and an edit compiles its own crate alone, the crates
above it after the patch ([what an edit
costs](build/README.md#what-an-edit-costs)). That beats aio's bundle and
reload on the same-size app by 3x.

### What aio has that aio-rs doesn't

| Area | aio | aio-rs | Workaround | Page |
|---|---|---|---|---|
| Forms | a Zod, Valibot or ArkType object run as it is (its `.refine`, `.transform` code too) | its JSON Schema export read by `Schema::json`; a rule written as code is written again in Rust | the rule as a `.check` on the field | [forms](ui/forms.md#gaps) |
| `am` | `publish --no-data` (an update with no data check) | a release always carries its data contract, so an update never installs over data it can't read | `--data file` | [ax](clients/ax.md#what-am-has-that-ax-hasnt) |

What aio-rs does without by design, each with its reason on its page:
worker cells (native code; [`off_thread`](state/effects.md#heavy-work-retries-and-timeouts)
for heavy work), the `single` / `multi` store layouts (aio's
[read and written as found](persistence/README.md#limits-compared-with-aio)),
an HLC (the server orders every call; [`cx.made()`](persistence/offline.md#limits-compared-with-aio)
for the later edit), a 12 ms frame deadline (10 000 woken holes run in
0.8 ms: [performance](performance.md#gaps-against-aio)), an
`experimental` marker (the API snapshot gate fails on any change:
[versions](basics/versioning.md#gaps-against-aios-policy)) and a V8
heap ([ax](clients/ax.md#what-am-has-that-ax-hasnt)).

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
  `AIO_CASSETTE=rerecord` asks; arguments that don't serialize are
  refused unless the tape is `lenient()`. [cassettes](testing/cassettes.md#what-aio-has-that-aio-rs-does-without)
- **A new build is a new version**: `ax release` refuses a version
  already laid out with other bytes, naming the commit that raises the
  build number; aio offers "the same version, new build" by digest, which
  an install with no recorded digest never sees.
  [release](build/release.md#publishing)
- **Time travel keeps every event in the log**: `#[skip_travel]` (aio's
  `skipActions`) only steps over a method's events.
  [time travel](debugging/time-travel.md#not-as-in-aio)
- **Problem reports are always on**, and carry the calls' arguments only
  with `[feedback] args`.
  [feedback](debugging/feedback.md#not-as-in-aio)
- **A release build's local door is on** (`[server] local_peers`, on
  unless `false`): `ax state` and `ax call` reach a release with a
  client's rights, never the operator's; aio's `allowLocalPeers` is off
  unless set. A process of the user can read the app's key and store
  anyway, so off guards nothing; `local_peers = false` opens no door.
  [transports](clients/transports.md#gaps)
- **`ax start` always waits** (bound it with `--wait`).
  [ax](clients/ax.md#what-am-has-that-ax-hasnt)
- **Variables are `AIORT_*`** where aio's were `AIO_*` (`AIO_CDP` →
  `AIORT_CDP`…); `AIO_BUILD_VERSION`, `AIO_SOCKET_TIMEOUT_MS` and
  `AIO_VIDEO_SCHEME` are read as aio reads them. [environment](reference/environment.md#aio-variables-aio-rs-doesnt-read)
- **Keep aio's speed tricks.** aio gave many for free (one signal per
  cell, debounced writes, a broadcast coalescer); a port spells them out.
  [speed tricks](migration/README.md#4-the-speed-tricks-aio-apps-rely-on)

## Feature comparison

✅ the same · 🔄 different (how) · ➕ aio-rs only, or more than aio. No
row is missing; what aio still has is [above](#what-aio-has-that-aio-rs-doesnt).

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
| Call hook | `beforeReduce` (may change; sees the state and the request) | `before_call` (lets in or refuses), `rewrite_call` (may change); the call, its `Caller`, the state it runs on (`call.state::<T>()`) and a network call's `request` (address, headers without credentials) | ✅ |
| Worker cells | `worker: true` | not needed: native code, `off_thread` (in a browser a Web Worker), `spawn_blocking` | 🔄 |
| View-only state | `scope: "client"`, `persist: "none"` | `#[cell(persist = false)]`, or a `Global` | ✅ |
| Visibility | `visible`, `forUser`, `cellDefaults.visible`; a hidden read throws | `#[cell(visible = f)]`, `aio::visible_by_default(f)` for every cell without its own (a field it leaves out reaches the copy as its default); a `Hidden<T>` field it hides (`Hidden::withheld()`) and `#[server]` fields panic when read on that copy, naming the field | ✅ |
| Plugins | `definePlugin` (`onAction`, `allowedOrigins`, …) | `aio::plugin::Plugin` (`on_action`, `allowed_origins`, …) | ✅ |
| Retired cells | `am persist --drop` | `ax persist --drop`, `#[cell(retire = [..])]` | ✅ |

**Methods, effects, scheduling**

| Feature | aio | aio-rs | Status |
|---|---|---|---|
| Async methods | `async` on a draft; `await cell.method()` gives its value or throw | `async fn`, split at its awaits (in branches and loops too); `h.m(..)?.await` gives its end | ✅ |
| Effects and modes | `$do`, `concurrency` (`first` joins a call with the same arguments) | `cx.run(Effect::new(..))`, `Mode`; `#[mode(newest \| first)]` on async methods (`first` joins a running call with the same arguments, and a network caller only their own) | ✅ |
| `ttl`, `long`, a method's own await timeout | `ttl` reuses an answer for every caller | `#[ttl(ms)]`, an answer kept per network caller, never handed to another; `#[long]`, `#[long(ms)]`; no `"warn"` (a ceiling cuts off) | 🔄 |
| Retries, timeouts | `call({ timeoutMs, retries })` | `aio::effect::retry`, `Effect::timeout`, `cx.backoff` | ✅ |
| Schedules | `every`, `cron` (UTC), `after`, `backoff`, another cell's action | `#[every]`, `#[cron]` (UTC; `[app] tz = "local"`), `cx.after`, `cx.backoff`, `cx.on::<Other>()` | ✅ |
| Clock, ids, randomness in methods | allowed, linted | `cx.now()`, `cx.id()`, `cx.rand()` | 🔄 |
| CPU work | `blocking(id, fn)`, `cancel` | `aio::effect::blocking(key, f)`, `cancel_blocking` | ✅ |
| Notifications | `notify()`; a warning when a signed-in user's call notifies everyone | `cx.tell(..)`; the same warning, once per app (log and `tell:every-user` diagnostic); a web page shows the browser's own, asking for the permission | ✅ |
| Pending calls | `$pending()` | `h.pending(..)`, `use_pending` | ✅ |

**Persistence**

| Feature | aio | aio-rs | Status |
|---|---|---|---|
| SQLite store | `state.db` | the same file | ✅ |
| Event log, history | journal (opt-in) | every commit an event | ➕ |
| Write debounce | every write, `persistDebounceMs` | `#[coalesce]` methods only, `[data] coalesce_ms` | 🔄 |
| Excluded fields | `persist: { include, exclude }`, `persist: "none"` | `#[cell(persist(include, exclude))]`, `#[transient]`, `save = f`; `#[cell(persist = false)]` | ✅ |
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
| Query builder | `where` objects, `orderBy`, `limit`, `offset` | `aio::db::Select` with `Where` (`eq`, `ne`, `lt`, `lte`, `gt`, `gte`, `like`, `is_in`, `and`, `or`) or aio's JSON object (`Where::object`); values always parameters, names plain identifiers only | ✅ |
| Blobs | `app.blobs`, `stream` | `aio::blobs::Blobs`, `stream` | ✅ |
| Size guard | 1 MB warn, 16 MB error | the same, unless `[budgets] state` | ✅ |
| Homes, profiles, instances | `--home`, `--profile`, `--instance` | the same rules | ✅ |
| App launch flags | `appFlags` | `[app] flags`; a near-miss of a declared or aio flag is refused with the one meant | ✅ |

**Sync and offline**

| Feature | aio | aio-rs | Status |
|---|---|---|---|
| Shared cells | `sync`, broadcast | `#[cell(shared)]` | ✅ |
| Optimistic calls | `useOptimistic`, local-first | built in: copies predict; `use_optimistic` for the rest (a server fn's effect, a `predict = false` call), and `clear()` on a refusal, which aio's can't tell | ✅ |
| Access per method | `access` | `#[client(role, allow)]` | ✅ |
| Merging | per-field view merges, `onConflict`, HLC | methods replayed on the server in one order (no HLC needed; `cx.made()` for last-writer-wins by when an edit was made); `Remote::merge` (`Text`, `Counter`, `SetAdd`, `Keys`, `SetRemove`), `on_conflict` | ✅ |
| Offline queue | 500 per `sync:` cell, in the page's storage, `retention` | 500, refuses when full, `retention`, on disk if asked: written in order, kept at a drop, `aio::shutdown` and the exit | ✅ |
| Array patches | Immer `add`/`remove`, narrowed | `insert`/`del` per item, else the array | ✅ |
| Throttles | `syncIntervalMs`, `fullStateThreshold` | `sync_interval_ms`, `full_state_percent` | ✅ |
| Render-staleness backpressure | a page says its paint lag (`vitals-ping {ms}`): 2x past 100 ms, 4x past 300 ms | the same steps, for aio pages and aio-rs copies (`seen {ms}`, said off the UI thread, so a frozen screen says it too); measured: a frozen screen gets 20 patches (2.8 KB, each folding 47 changes) where it got 78 (10.7 KB, 12 each), 256 ms of CPU against 405 ms | ✅ |
| Path subscriptions | `subs`: whole cells | `Remote::subscribe`, `connect_paths`: dot paths inside a cell, only their patches, predicted where held, kept in a saved copy; aio pages' `subs` as aio's, `exact: true` for paths | ➕ |
| Transport | `transport: uds \| ws \| auto`, the window's wire | the window's is always its private sockets; `[client] transport = auto \| ws \| poll` (`--transport`): a copy's links over plain HTTP when a proxy drops the WebSocket | ➕ |
| Binary side channels | a raw WebSocket route, a recipe | `Route::socket` with framing, drop or close, gap checks | ✅ |
| Server functions, routes | `serverFns`, `routes`, streamed `Response` | `aio::fns::ServerFn`, `aio::http`, `Reply::stream` | ✅ |
| One socket, every cell | yes | `Http::cell`: one `/ws`, port and socket for every cell, a link per cell on it; an aio page sees every shared cell | ✅ |
| Peer identity, door hooks | `resolvePeer`, `localConnectHooks` | `Http::resolve_peer`, `Http::door_links` | ✅ |
| Pairing, discovery, peers | PIN, CPace, UDP 8099 on every interface's broadcast, one machine listed once, a probe nobody took an error (macOS's Local Network named), `x-aio-peer` | the same | ✅ |
| Calling another app | `peer()`, `call`, `stream()`, an app-wide `describe()` | `Peer::find`, `AioLink`, `AioPeer` (proven as this app); `Remote::stream` with `cx.part` in the method; `describe(cell)` and the app-wide `describe_all()` | ✅ |
| Server limits | `maxConnections`, `wsLimits` | `[server.limits]` | ✅ |

**Auth and security**

| Feature | aio | aio-rs | Status |
|---|---|---|---|
| Accounts, roles, TOTP, OIDC | `auth`, `sessions`, `auth.totp`, `auth.cookie`, `requireVerified` without `sendMail` refused, `SignIn` with SSO | `[accounts]` (`totp`, `cookie`; `signup` with `require_verified` needs `Http::mail`); `SignIn` with SSO in a page, and in a desktop or phone app through the browser (RFC 8252 loopback, confirmed in the browser first) | ✅ |
| Who is signed in, in a view | `useUser()` (a page; `null` on a standalone build) | `aio::kit::use_user()` in a page, a desktop window and Android: the page asks its server, a window knows whom its `SignIn` signed in (`set_user(None)` signs out) | ➕ |
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
| Server rendering, hydration | `renderToString`, `collectHead`, `hydrate` | `Http::painted(App)`: each navigation per viewer (its cells as its socket shows them, its route, its head), the state seeded into the page's copies, hydrated in place (typed text kept and told); `aio::ui::paint`, `air::ssr`; 2.5-5.5× faster than aio's `renderToString` (2, 50, 1000 rows) | ✅ |
| Streamed rendering | `renderToStream`: in order, a lazy's `Suspense` sent as its fallback | the page at once, each `Suspense` waiting for a `seeded_resource` sent in place as it loads with its values, the page's first frame starting from them (no inline script); `air::ssr::stream` | ➕ |
| Code splitting, `lazy` | `lazy` defers evaluating a module; the page downloads one bundle | `aio::lazy!` in a `Defer`: a web build's lazy views in chunks, each fetched when its view first shows (first download 762 → 543 KB gzipped, a page with a heavy view) | ➕ |
| Other view libraries | `aio/state-core`, React compat | the JS client: `/__aio/client.js` on every server, `aio-rs-client` with real React hooks (`useCell`, `useCall`, `useStatus`), Solid's and Vue's, Svelte's store contract, each tested on the real library; each patch checked by a digest JavaScript reproduces (`jsdigest`, a full resync on mismatch); predicted calls (by method too, so a reloaded call shows), an offline queue kept across reloads and shared by twin tabs, one socket for every cell; typed by `ax tsgen`; in a browser, and in the desktop window over its own wire (`use_js_client`) | ➕ |
| Views without JSX | `h()` | `h(tag)` (`attr`, `on`, `child`, followed by state), `comp` | ✅ |
| Keyed cache shared by views | `trackedMemo()` | `TrackedMemo` (a hit follows what it read; `max`) | ✅ |
| Kit components | `aio/ui`, `<UiStyles/>` | `aio::kit`, the same names; every prop that takes any view in aio takes a `View` (text and state too); `UiStyles` | ✅ |
| Islands, React islands, `<Browser>` | yes | `WebIsland`, `Island::react`, `kit::Browser` (every `<webview>` method by name: `kit::webview`) | ✅ |
| Side tooltips | no (`top`, `bottom`) | `Placement::Left`, `Right` | ➕ |
| Keys | `onGlobalKey`, `REPEATABLE_KEYS` | `use_global_key`, `Chord::repeating` | ✅ |
| Long lists | `useVirtualList` | `kit::VirtualList` | ✅ |
| Forms | `useForm` + Standard Schema, `parsed()`, touched on blur | `use_form` + a Rust check, or `use_form_schema` + a `Schema` (`parsed()` typed, checks that wait, where aio refuses an async schema), or one read from a Zod, Valibot or ArkType JSON Schema export (`Schema::json`); `bind`, touched on blur, `reset_field`, `use_field_array`, `use_on_change` | ✅ |
| Routing | History API, address bar; typed `useRoute(pattern)` (param names) | history in the app; in a browser the address bar, Back and a reload follow it; typed `routes!`: param names checked as it builds, values parsed (`id: u32`) | ✅ |
| Head | `useHead` | `aio::head::use_head` | ✅ |
| Theme, hue, CSS variables | `ui.theme`, `hue`, `neutral`, dark follows the OS | `theme_css(hue)`, `--aio-*`, `[ui] neutral`, `[ui] scheme` (the OS's by default) | ✅ |
| Scoped CSS classes | `css` tag; `aiol` warns of a class two sheets define differently | `aio::ui::css`; `ax lint` warns of it | ✅ |
| Live CSS in dev | yes | yes, plus `[build] css` toolchains | ✅ |
| Transitions | `TransitionFn` (given the element), shared `options` | CSS keyframes or `Motion::of` (the element's box as `--aio-w`, `--aio-h`), no box; `options: Timing` over both motions, a `const` shared by many | ✅ |
| Runtime a11y and contrast checks | dev mode, warned on the console | the running dev page's (desktop and web), said once each in the log and shown in the page's overlay while they hold; `TestUi::a11y()`, `contrast()` | ➕ |
| Window chrome | `ui.chrome`: `standard`, `themed`, `none` | `Window { chrome }`: `Chrome::Standard`, `Themed`, `None` | ✅ |
| App menus | macOS's standard ones only | the same | ✅ |
| Redux DevTools | actions and state; a jump only points to the Ctrl+. panel | a dev build's `client!` cells, one instance each: actions with their arguments, state, jumps that travel, dispatch, pause, commit, reset; where the page has the extension (a browser), not a desktop window's `aio://` page | ➕ |
| Worker bundling | `new Worker(new URL("./w.ts", import.meta.url))` scripts bundled | `aio::effect::off_thread`: the app's own module in Web Workers (a pool), the same call on every target, stopped at once when cancelled; a script with its own message protocol is the app's to ship as a file | ✅ |

**Testing**

| Feature | aio | aio-rs | Status |
|---|---|---|---|
| Cell tests | `testCell`, `bootCells`, `t.getEffects()` | `TestCells` (virtual time, seeded fuzz, `fuzz_methods`, `reset`, `last_effects`, `budget`); `t.last_asked()`: the last call's effects, schedules, tells and aborts as values (`is_schedule`, `is_tell`, ...) | ✅ |
| Unseeded fuzz | `t.randomActions(n)` | `random_calls(&h, n)`: a fresh seed each run, printed and named by a failure, so `fuzz_methods` replays it (aio's can't be replayed) | ➕ |
| Real timers | `t.settle(ms)` | `TestCells::real_time()`, `settle_for(ms)` (virtual time stays the default) | ✅ |
| UI tests | `testUI` on happy-dom | `TestUi` on the real engine | ➕ |
| Virtual clock under the page | `ui.advance` | `TestUi::advance` (the views' timers; schedules: `TestCells::advance`) | ✅ |
| Components by name and key | `ui.find(c, key)`, `surface()` tree | `ui.find(c, key)`, `within`, `surface_tree()` | ✅ |
| Typed UI clients | `am testgen` | `ax testgen` (a Rust name table) | ✅ |
| Server and multi-client | `testServer`, `testMultiClient` | `TestServer`, `Sim` | ✅ |
| Patch assertions | `patches`, `waitForPatch`, `dispatchAll` | `testing::patches` (`all`, `until`), `call_all` | ✅ |
| Several apps | `testApps` | `TestApps` | ✅ |
| Cassettes | `openCassette`, `wrap`, unserializable args as text | `Cassette` (`wrap`; a damaged tape refused unless `AIO_CASSETTE=rerecord`; unserializable args with `lenient()`) | ✅ |
| Page fetch stubs | `testUI({ fetch })` | `Mount::fetch(host, answer)` | ✅ |
| Boot smoke in a test | `smoke()` | `testing::smoke(bundle)`, `ax smoke` (also a `.deb`'s menu entry, page errors) | ✅ |
| Proving the tests | `check:mutations`, `check:vacuous` | `scripts/mutants` (source and a built `.deb`), `ax lint` (no-assertions, self-comparison) | ✅ |
| Browser tests | `testBrowser` | `TestBrowser` | ✅ |
| Lint | `aiol`, perf hints, `[fixable]` marks | the compiler + `ax lint`: a `perf` warning for blocking I/O in async code, `[fixable: --safe-fix]` / `[fixable: --fix]` per finding | ✅ |
| One-command check | `deno task check`, `deno task test` | `ax check <app>` (fmt, lint, tests; `--release`: size and smoke too) | ✅ |
| Labs | VMs and containers it boots | the same images (`ax lab --boot`), or a machine over ssh | ✅ |
| Windows bundles | `deno task test:wine`: the `.exe` under Wine | smoked on a real Windows (`./ci-windows.sh`, `AIO_WINDOWS_SSH`, GitHub's Windows runner): what Wine passes and Windows refuses can't pass here | ➕ |
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
| Logs, errors, health, vitals, budgets | yes, `logging.dir` | `ax logs`, `ax errors`, `ax health`, `ax vitals`, `/__aio/vitals`, `[vitals]`, `[perf]`, `[budgets]`; logs in `<home>/logs` or `[log] dir` | ✅ |
| Log budgets | `logBudget` (a folder ceiling checked at boot), `--no-backup-logs` | each log within `keep` × `file_mb` at every write, so a long run never outgrows it (`the_logs_stay_within_budget_at_every_write_and_keep_one_per_run_wipes`); `per_run` with `keep = 1` | ➕ |
| Runtime benchmark gate | `check:bench` (D12): p10 under 2× baselines | `tests/rt_gate.rs`: a start, a store's open, a call, a saved call, p10 under 2× measured, debug and release, in `./ci.sh` | ✅ |
| Boot report | config, cells and routes at start | every setting with who decided it, `platform`, `updates`; a second on: `cells` (with data versions), `routes`, `serverfns` | ✅ |
| Diagnostic bus | `onDiagnostic`, severity, fix; a typed `onError` with many codes and `stateSnapshot` | `report::subscribe`: `severity`, `tip`, `code()`, `error_code()` (aio's `REDUCE_ERROR`, `EFFECT_TIMEOUT`, `HOOK_ERROR`, `INIT_ERROR`, `DESTROY_ERROR`), `cell`, `call`, `numbers`, `state` (screened as a report's); `vitals:<layer>`, `degraded-recovered:<part>`; `aio::Code`'s aio codes (`EFFECT_ERROR`, `EFFECT_TIMEOUT` to an async call's caller, `DESTROY_ERROR`), `error_summary` | ✅ |
| Memory pressure hook | `onMemoryPressure` (each sample under pressure, `trend`), `interval` | `health::on_memory` (the same, `trend`), `[memory] every` | ✅ |
| Prometheus metrics | uptime, memory, clients, cells, broadcast bytes and messages | the same, and calls, degraded parts, gauges and budgets | ✅ |
| Vitals per client | `clients` (liveness), `payloadStats`, `cellSizes`, `drainRate`, `circuitBreakers` | `clients` (liveness and payloads in one row), `cellSizes`, `drainRate`, `breakers` | ✅ |
| Per-run log files | `debug.log` per run | `[log] per_run` | ✅ |
| Correlation ids | yes | `call=<id>` on a call's lines, its timeline entry and reports | ✅ |
| Error box with state | yes | each failing call's, effect's or hook's diagnostic carries its cell's state (`Diagnostic::state`), and reports carry every cell's | ✅ |
| Heap | `am heap`: its V8 heap against the ceiling, the resident set, each cell's size | `ax heap`: the resident set, each cell's state, events and argument bytes; no GC heap to size or snapshot | ✅ |
| Problem reports | `feedback: true`; timeline payloads | always on (`auto = false`: the user's only); arguments with `[feedback] args` | ✅ |
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
| macOS app away from a Mac | an unsigned `.app` (Deno cross-compiled); its arm64 one refused by Apple silicon until a Mac signs it | `ax build --platforms macos,macos-arm64` on Linux or Windows: zig links it (fetched, SHA-256 checked; no Apple SDK), the arm64 binary signed ad hoc by its linker: the zip opens on Apple silicon as it is (macOS 26, tested); a `.dmg` needs a Mac | ➕ |
| iOS | ios-client: an Xcode project (a WKWebView, the connect page bundled); Xcode builds it | `ax build --ios` (kind `ios`): the same Xcode project, its Swift type-checked against UIKit and WebKit on a Mac without Xcode; it opens the server, whose own page pairs the device; Xcode builds and signs it. Without Xcode: that page in Safari, added to the Home Screen (manifest, icon, offline shell), driven in WebKit | ➕ |
| Run from a link | `run.sh` / `run.ps1`: a script piped into a shell clones, builds, installs and runs | `ax run <link>`: cloned into the cache, one commit checked out, the link, commit and apps shown and asked before any of its code runs, a commit pinned by `--ref`; built and started as `ax start`; installs nothing | ✅ |
| Dev reload | patch, reload, restart (0.4-0.8 s; 1.3-1.8 s for a ~74 000-line app) | hot-patch, state kept: `ax dev` reloads markup (8 ms) and patches Rust (~0.3 s) itself (a ~74 000-line crate 5-6 s; in page crates 0.5 s); a changed type restarts in ~2 s (Linux, macOS, Windows); the in-page overlay shows build failures, page errors, the app's own problems and the live a11y and contrast findings; an `#id` the CSS styles and no element has is warned | 🔄 |
| Dev watcher | 100 ms debounce, 500 ms ceiling, editor files ignored, served files watched, health sentinel, restart drain and storm guard, files named | the same (cargo's dep-info for the files a build read); an unchanged binary isn't restarted | ✅ |
| Dev session | `am start` of a dev run, `--no-watch`, `--verbose`, `--title`, `am stop` ends it | `ax dev --detach`, `--no-watch`, `--verbose` (`AIORT_LOG`), `--title` (`AIORT_TITLE`), `ax stop` ends it, `ax status` says it waits for a fix | ✅ |
| Hot reload in web, Android | page reload | stylesheets and markup live, state kept (0.01-0.02 s against aio's 0.3-0.5 s reload); Rust in the page's crate hot-patched, state kept (0.17-0.18 s against aio's 0.41-0.84 s); an edit a patch can't take rebuilds and reloads (~1 s), the cells kept (`ax dev --web`, `--android`'s page) | ➕ |
| Single instance, takeover | `singleton`, `takeover` | always one per home, `--takeover`; a launch while the running one stops starts after it | ✅ |
| Release build to try | none (a bundle build) | `ax build --quick`: no LTO, a one-line change in ~2 s, never shipped | ✅ |

**Release and updates**

| Feature | aio | aio-rs | Status |
|---|---|---|---|
| Signed releases | `ship keygen`, `publish` | `ax keygen`, `ax release` | ✅ |
| Delta updates, channels, data gate, rollback | yes; AppImage deltas, the same version's new build | yes, AppImage deltas too; a new build is a new version (a same-version rebuild is refused at `ax release`, naming the commit that numbers it), so every install sees it | ✅ |
| Stepping-stone releases | `--min-from`, `minFrom` signed | `ax release --min-from`, signed, not past the data gate's buttons either | ✅ |
| Release sources | https, `file:///`, `github:`, a git repository | the same | ✅ |
| Offer details | `reason`, `migrates`, `progressOf` | `reason`, `migrates`, `progress_of` | ✅ |
| Build in the manifest | `buildNumber`, `commit` signed | `commit` signed with `--commit`; the build number is the version's third part | ✅ |
| Custom update runtime | `installUpdatesRuntime`, the cell kept | `aio::update::install`, the cell kept, the built-in checks out | ✅ |
| `updates` cell on clients | synced to every client; `--expose`d: signed-in only | synced to every client, server paths hidden; installs and channel an admin's | ✅ |
| Update veto | `canApply` (awaited, every install; a warning with none) | `can_apply(\|\| async { … })`: awaited off the cell before every install, the one at start too; a start installing with none set says so | ✅ |
| GitHub workflow | `ship github` | `ax release <app> --workflow` | ✅ |
| GitHub Pages | `ship github` deploys the channel there | `--workflow --pages` (and `--web`: the app's web build at the site's root) | ➕ |

**CLI: `am` and `ax`**

| Feature | aio | aio-rs | Status |
|---|---|---|---|
| App control | `start`, `stop`, `kill`, `restart`, `status` | the same | ✅ |
| State and history | `state`, `snapshot`, `migrations`, `replay`, `timetravel` | the same | ✅ |
| Calls | `dispatch` (`k=v`, `--body`, `--wait`, `--as-server`) | `ax call` (the same; `--as-server` on a debug build's door) | ✅ |
| Plain (non-cell) actions | `dispatch <Type> k=v` | `ax call <app> --fn <name> k=v`: a server function (aio-rs's non-cell call), `k=v` its one object as aio's payload; `--args`, `--body` too | ✅ |
| `kill --stale`, `watch`, `report` (a new one), `build --force` | yes | `ax kill --stale` (found by the OS's open files, not a socket's claim), `ax watch`, `ax report --new`, `ax build --force` | ✅ |
| Output into a pipe | JSON | JSON (`--text`, `AIORT_OUTPUT=text` keep the text) | ✅ |
| Inspection | `surface`, `trigger`, `shot`, `eval`, `preview`, `sql`, `heap`, `cost` | the same | ✅ |
| Pairing, trust | `discover`, `profile`, `pair`, `auth`, `trust` | the same (`ax pair <app>` makes a pairing code, `ax pair <app> <host:port> <code>` pairs this device) | ✅ |
| `where`, `check` | yes | `ax where <app> [item]`: from the crate graph and each `cfg` (`server`, `both`, `window/page`, `wasm-only`, `native-only`), with the crates, modules and cfgs that put it there; `ax check`: fmt, lint, the web build's code checked for wasm32 (aio's `am check`), tests | ✅ |
| `persist`, `testgen`, `timeline`, `top`, control REST API | yes | `ax persist`, `ax testgen`, `ax timeline`, `ax top`, `ax control` (loopback HTTP) | ✅ |
| `add cell\|server` | yes | `ax add <app> cell\|server\|page <name>`: written, wired into the launchers, with its test; a page: a crate of its own, which `ax dev` recompiles alone | ➕ |
| `keygen`, `release`, `update`, `smoke`, `install`, `lint`, `test`, `events`, `components`, `adopt`, `import`, `runtime`, `setup` | — | yes | ➕ |

**Tooling**

| Feature | aio | aio-rs | Status |
|---|---|---|---|
| Templates | `am create --template=counter\|todo\|cli\|canvas` | `ax create --template app\|todo\|canvas\|cli\|service`: the todo list (add, tick, filter in the window, clear done) and the canvas (a pure `step` tested, a frame drawn per move) as aio's, each with its tests | ✅ |
| Agent brief | `am agent` | `ax agent`, `AGENTS.md`/`CLAUDE.md` in new apps | ✅ |
| Framework version per app | `dep/aio` link, `am pin`, `am fix` | aio-rs from git at a release tag in `Cargo.toml`, the commit in `Cargo.lock`: a clone builds as it is; `ax create --aio-version`; `ax pin` says what Cargo.toml asks for, what Cargo.lock built, this ax and the releases (drift: exit 1), `ax pin <v>\|main\|latest\|<checkout>` moves it, `--major`, `--json`; aio's preflight: a move to a release that removed what the app uses is refused, each use named, `--force` | ✅ |
| Upgrading the CLI | `am upgrade`, `am uninstall` | `ax upgrade` (the release's checked install script, or a checkout), `ax uninstall` | ✅ |
| Install | `install.sh`, `install.ps1` (Deno + a clone; `AIO_REPO`, `AIO_REF`; re-run updates) | prebuilt `ax` (Linux, Windows, macOS), SHA-256 checked: `curl \| sh`, `irm \| iex` (a failure leaves the window open), offline from a folder (`AIO_DOWNLOAD`), a cut-short download runs nothing; a release by its script, `ax upgrade [v]`, `ax uninstall`; or from source | ✅ |
| API reference | `api-reference.md` | `cargo doc -p aio-rs` | ✅ |

## Plans

- **Done in 0.2.0: aio-rs's own renderer, "air", is the only one.** A
  fine-grained renderer of aio-rs's own: a cell's fields as signals,
  direct DOM edits for what changed, and a lean macro so rebuilds get
  faster ([design](design/air.md)). Apps are written against `aio::ui`;
  what a 0.1 app changes is in the migration guide's section 8, [0.1 → 0.2](migration/README.md).
- **Done in 0.4.0: a key's answer in the frame after it**, as in aio
  ([key to paint](#key-to-paint)): a draw with aio on every key at the
  frame's floor, with less CPU.

## Moving

- [Migrating from aio to aio-rs](migration/README.md): tables, a worked
  example, the speed tricks, a checklist.
- [aio app data](migration/data.md) and [aio wire formats](migration/wire.md).
- [Why aio-rs, not plain Rust](basics/why.md).

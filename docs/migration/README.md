# Migrating from aio to aio-rs

Before you port: [compatibility](../aio-vs-aio-rs.md#app-compatibility),
[deviations](../aio-vs-aio-rs.md#how-aio-rs-deviates-from-aio),
[feature matrix](../aio-vs-aio-rs.md#feature-comparison).

aio is the Deno/TypeScript framework (command `am`); aio-rs is its Rust
counterpart (command `ax`). The two run side by side, and an app's data is
the same file for both: a port (the aio-rs app made from an aio app) opens
the aio app's `state.db` as it is, and the aio app can still open it
afterwards. aio's pages, `am` and other aio tools keep talking to the port
([wire](wire.md)).

**A port is a translation, not a rewrite.** Keep the aio app's structure,
its cells, its methods, its components, *and every optimization it has*:
ts becomes rs, tsx becomes rsx, construct by construct, using the tables
below. A port that re-implements the app "the simple way" (views reading
the whole state, a commit per keypress, rows rebuilt per render) works,
and is many times slower than the aio app it replaces.

1. [Project and boot](#1-project-and-boot)
2. [Construct by construct](#2-construct-by-construct)
3. [A worked example](#3-a-worked-example)
4. [The speed tricks aio apps rely on](#4-the-speed-tricks-aio-apps-rely-on)
5. [Data: the same state.db](#5-data-the-same-statedb)
6. [The wire: aio pages, tools and apps](#6-the-wire-aio-pages-tools-and-apps)
7. [Checklist](#7-checklist)
8. [0.1 → 0.2: aio::ui instead of dioxus](#8-01--02-aioui-instead-of-dioxus)

## 1. Project and boot

| aio | aio-rs |
|---|---|
| `deno.json` tasks, imports | a Cargo workspace: the app's library (cells, views) and a small launcher crate ([project](../basics/project.md)) |
| `aio.run({ appId, cells, ... })` options | `aio.toml` (`[app] id`, `[data]`, `[server]`, `[history]`…), parsed as the app builds by `aio::config!()` |
| `appId` | `[app] id`; for a port also `[app] port_of = "<the aio app's appId>"`, so it runs on that app's home and data |
| `main.ts` | `app/src/main.rs`: `aio_shell::start`, `aio::Source::for_app`, `aio_shell::run` |
| `src/cell/*.ts` | `src/*.rs` modules, one per aio cell file |
| `src/ui/*.tsx` | `src/ui/*.rs` modules, one per aio component file |
| `am create` | `ax create <name>` (`--template app`, `cli`, `service`; `--aio-version`, `--target`, `--css tailwind`) |
| `am pin`, `am fix` | `ax framework <version>`: the app's aio-rs; a clone needs no repair |
| `am upgrade`, `am uninstall` | `ax upgrade`, `ax uninstall` |

`ax create` makes the whole layout; copy the aio app's cells and components
into it file by file.

### Every `aio.run` option

aio's boot options become `aio.toml` keys ([reference](../reference/aio-toml.md)),
cell flags, launch flags or nothing (built in). "None" says what to do instead.

| `aio.run` option | aio-rs |
|---|---|
| `appId`, `cells` | `[app] id`; the launcher opens each cell (`Source::for_app`), `aio::Cells::with(h, after)` orders their start |
| `localFirst` | built in: a synced copy predicts every call and confirms it with the server |
| `cellDefaults` | none: flags are per cell (`#[cell(persist, visible = f)]`) |
| `port`, `host`, `expose` | `[server] listen`; `--port`, `--host`, `--expose` per launch |
| `tls` | `[server] tls`, `cert`, `key` |
| `appDir`, `profiles`, `dbPath` | the home rules ([files](../persistence/files.md)), `--home`, `--profile`, `--instance`; `[data] store` names the file; `Handle::memory()` is aio's `":memory:"`. No switch turns profiles off |
| `dbPragmas` | `[data] pragmas` |
| `checkIntegrityOnBoot` | built in: every boot runs `quick_check`, and a damaged store is set aside and recovered |
| `updates`, `feedback` | `[updates]`, `[feedback]` |
| `persist`, `persistKey`, `persistMode` | `#[cell(persist)]`; a port reads and writes aio's `aio_kv` rows in whichever layout the file has ([data](data.md)) |
| `persistDebounceMs` | `#[coalesce]` on a method, `#[cell(coalesce)]` on a cell (writes held up to `aio::PERSIST_DEBOUNCE`) |
| `ui` (title, size, entry, head, lang, dir, theme, chrome, tray) | `[app] name`, `lang`, `dir`; `[ui]`; `aio_shell::Window` in the launcher; `aio::head::use_head` ([desktop](../clients/desktop.md)) |
| `baseDir`, `serveDirs`, `assets` | the app's `assets/` (built in), `[server.dirs]`, `[server.public_dirs]` |
| `client` (`electron`, `browser`, `cli`, `server-only`) | `[app] mode` (`standalone`, `client-server`, `server`, `client`, `cli`) and `[app] ui = "browser"` |
| `keepServer` | none: a served app is `mode = "server"` (no window) or `client-server` (stops with its window) |
| `localConnectHooks`, `onConnect`, `onDisconnect` | `Http::on_link(f)`, `Plugin::on_link(f)`: each link's `Presence` |
| `transport` | built in: the window talks over IPC, other local tools over the local door, the network over `/ws` |
| `takeover`, `singleton` | always one instance per home; `--takeover` per launch; `--instance <n>` for a second, private one |
| `serverUrl` | `mode = "client"` with `[client] server`, or a pairing (`ax pair`) |
| `users`, `key` | the app's key (`AIORT_TOKEN`, else `data/sync.token`), `[server] open`; per-user tokens: `[accounts]` ([accounts](../auth/README.md)) |
| `pairing` | `[server] pairing`, `ax pin`, `ax pair` |
| `resolveUser`, `sessions`, `auth` | `[accounts]` (users, roles, sessions, TOTP, OIDC) |
| `resolvePeer`, `peerTypes` | `aio::peer::Peer::find(app, ..)`: a peer on this machine calls as a client; `ax peer <app>` prints what it may call, as Rust signatures |
| `db` | `#[table("name")]` fields, `aio::db::Db` ([SQL](../persistence/sql.md)) |
| `perfCheck`, `perfBudget` | `[perf]`, `[perf.methods]`, `[perf.effects]` |
| `budgets`, `memory`, `vitals`, `renderBudget` | `[budgets]`, `[memory]`, `[vitals]` (`render`, `transport`, `loop`, `queue`) |
| `dispatchStorm` | `[perf.storm]` |
| `circuitBreaker` | `aio::Cells::trip_after(max, window)` |
| `effectTimeoutMs` | `Effect::timeout(..)` per effect |
| `watch` | `ax dev --watch=false`, `--watch=src/ui` |
| `wire` (`omitTrailingUndefined`, `refuseShortCalls`) | built in: arguments are typed, a short call is refused by serde |
| `freezeState`, `strictCells`, `guardDispatches`, `refusalsReject` | built in: Rust ownership freezes the state; a cell is a type, opened or not at compile time; a panic refuses its call and the app goes on; a refusal is always `Err` |
| `journal` | built in: every commit is an event in the store, so a kill loses nothing committed |
| `singleton: false`, `libraryMode` | open cells in a library or a test with `Handle::memory()` / `Store::open(..)`: no boot, no lock |
| `childWindows`, `electron` (sandbox, permissions, `webviewTag`) | `aio::kit::Browser` for other sites; the window's security is aio-shell's (its fuses, `AIORT_ELECTRON_ARGS`' allow-list), not an app option |
| `splash` | `[build.splash]` |
| `syncIntervalMs`, `fullStateThreshold` | built in: pushes are patches, batched (`aio::ui::BACKGROUND`); a full state goes only to a client that fell behind or came back |
| `maxConnections`, `wsLimits` | `[server.limits]` (`clients`, `max_message`, `calls_per_sec`, `bytes_per_sec`) |
| `allowedOrigins`, `strictOrigin`, `trustProxyHeader` | `[server] hosts`, `strict_origin`, `trust_proxy` |
| `security` | `[server.security]` |
| `appFlags` | not needed: `aio::paths::Ask::app_args` hands the app every argument aio didn't take |
| `routes` | `aio::http::Http`, `Route::get(..)` |
| `plugins` | `aio::plugin::Plugin` |
| `schedules` | `#[every]`, `#[cron]` on a cell's methods |
| `isolate` | none: open only the cells you want in a test (`TestCells`) |
| `beforeReduce`, `onAction` | `aio::before_call(f)`: it sees each call and may refuse it |
| `onEffect` | `aio::on_effect(f)` |
| `onStart`, `onStopping`, `onStop` | `#[on_start]`, `#[on_stop]` on a cell's methods |
| `fatalOnStart` | none: an `#[on_start]` refusal is logged and reported; the app boots on |
| `onError` | `aio::report::on_problem(f)` ([errors](../debugging/errors.md)) |
| `onRestore` | `#[cell(restore = f)]` |
| `logging` | `[log]` |
| `diagnostics`, `redactActions` | `#[cell(diagnostics = false)]`, `#[redact]` |
| `onCheckpointRestore` | none: there is no checkpoint; the store is the record |

## 2. Construct by construct

### Cells and methods

| aio | aio-rs |
|---|---|
| `cell("todo", { state: {..}, methods: {..} })` | `#[aio::cell(id = "todo")] pub struct Todo {..}` + `#[aio::methods] impl Todo {..}` |
| a method `add(s, title)` mutating `s` | `pub fn add(&mut self, title: String)` mutating `self` |
| `throw new Error("why")` in a method | `reject!("why")`, `ensure!(cond, "why")`; return `aio::Result` |
| a method's return value | `-> aio::Result<T>` |
| `persist: true` | `#[cell(persist)]` |
| `persist: { exclude: [..] }` | `#[transient]` on those fields |
| `version: N`, `onMigrate` | `#[cell(version = N, migrate = f)]` |
| `selectors: { f(s) {..} }` | `#[derived] fn f(&self) -> R` |
| `createSelector(s => s.a, s => s.b, (a, b) => ..)` | `#[derived(reads = [a, b])]` |
| a selector with `deps: ["other"]` | `#[derived] fn f(&self, other: &Arc<Other>) -> R` |
| `s.$call("other", ..)` | call the method directly, or `listens_to` across cells |
| `machine:` / guard lines on a status field | `aio::machine!` + `#[cell(machine = field)]`, or `ensure!` |
| `transaction: true` | nothing: every call is atomic |
| `onInit` / `onDestroy` | `#[on_start]` / `#[on_stop]` |
| `beforeReduce` | `aio::before_call(..)` |
| `onEffect` | `aio::on_effect(..)` |
| `scope: "client"` (view state) | a cell without `persist`/`shared` in the view's module, or a `Global` |
| `redactActions` | `#[redact]` |
| `diagnostics: false` | `#[cell(diagnostics = false)]` |
| `definePlugin` | `aio::plugin::Plugin` |
| `validate: s => .. \|\| "why"` | `#[invariant] fn f(&self) -> aio::Result` (checked after every call; a failing one refuses it) |
| `args: { m: [z.number()..] }` | typed arguments: serde refuses the wrong type; ranges with `ensure!` |
| `listensTo` | `h.listens_to(&other, "method", f)` |
| `long: ["m"]` | an effect without `.timeout(..)`: it runs as long as it needs |
| `sync: true`, merge strategies | `#[cell(shared)]`; methods merge by replay (see the sync table) |
| `onPersist`, `onRestore` | `#[cell(save = f, restore = f)]` |
| `retired cells`, `am persist --drop` | `#[cell(retire = ["old"])]` |
| `worker: true`, `workerRespawn` | none: every cell is native code on its own lock; blocking work goes to `tokio::task::spawn_blocking` in an effect |
| `bindCell`, `composeCells` | `aio::client!(Cell, name)`; one cell holding others as fields (`#[aio_cell]` in a port) |

A method named like one of `Handle`'s own (`start`, `state`, `call`,
`travel`, `pause`, `store`, `listen`, `events`…) would be shadowed by it on
a handle, so `#[methods]` refuses it at build time, naming the clash.
Rename it (`begin`) when porting.

### Async, effects, schedules

| aio | aio-rs |
|---|---|
| `async save(s) { s.x = ..; await io(); s.y = .. }` | `pub async fn save(&mut self, ..) -> Result` with `let v: T = io().await;` (split into atomic steps) |
| `s.$do(effect)` | `cx.run(Effect::new("key", move \|h\| async move { .. h.done(v) }))` |
| `concurrency: "latest" / "queue" / "drop"` | `.mode(Mode::Newest / Queue / First)` |
| `cancelOn`, an abort signal | `cx.abort("key")` |
| `ttl` | `#[ttl(ms)]` |
| `Date.now()`, `crypto.randomUUID()`, `Math.random()` in a method | `cx.now()`, `cx.id()`, `cx.rand()` |
| `schedules: { every: "5m" }`, `schedule.every` | `#[every("5m")]`, `cx.every(..)` |
| `schedule.cron` | `#[cron("0 9 * * 1")]`, `cx.cron(..)` |
| `schedule.after`, a debounce | `cx.after(key, ms, method, args)` (the same key replaces) |
| `schedule.backoff` / `poll` | `cx.backoff(key, attempt, aio::Backoff::from(ms), ..)` |
| `notify()`, `appIcon()` | `cx.tell(aio::tell::Tell::notify(..))`, `cx.tell(Tell::Icon { window, tray, tooltip, fps })` |
| `onChange`, `useResource` | `h.on_change(..)`, `aio::kit::use_held` |

### Sync, access, server code

| aio | aio-rs |
|---|---|
| `shared` cells, the server broadcast | `#[cell(shared)]` |
| `visible` / `visible.forUser` | `#[cell(visible = f)]`, `fn(&Self, &By)` or `fn(&Self, &Caller)` |
| server-only fields | `#[server]` |
| `access` per method | `#[client]`, `#[client(role = "admin", allow = f)]` |
| optimistic calls, `useOptimistic` | built in: copies predict; `#[client(predict = false)]` to wait |
| CRDT strategies (`counter`, `set-add`, `text`) | methods merge by replay; `aio::merge::text3` for prose |
| `serverFns`, `serverFn("pay").charge(..)` | `aio::fns::ServerFn` + `Http::server_fn`, `remote.run(F, args)` |
| `routes`, `route()` | `aio::http::Http`, `Route::get(..)` |
| `peer()` | `aio::peer::Peer::find(..)` |
| `app.blobs` | `aio::blobs::Blobs` |
| `createDB` | `aio::db::Db` |
| `vitals`, `budgets` | `[vitals]`, `[perf]`, `[budgets]` in `aio.toml` |

### Components, signals, hooks

| aio (AIR, TSX) | aio-rs (`aio::ui`, RSX) |
|---|---|
| `function Row(props) { return <li>..</li> }` | `#[component] fn Row(name: String) -> View { rsx! { li { .. } } }` |
| `<li class="row" data-t="x">{name}</li>` | `li { class: "row", "data-t": "x", "{name}" }` |
| `{items.map(i => <Row key={i.id} .. />)}` | `for i in items.get() { Row { key: "{i.id}", .. } }` (`key:` required) |
| `{cond && <X />}`, `cond ? <A/> : <B/>` | `if cond { X {} }`, `if cond { A {} } else { B {} }` |
| `onClick={() => ..}` | `onclick: move \|_\| ..` |
| reading `todo.items` in a render | `part(\|t\| &t.items)`, a handle: `.get()`, `.with(\|v\| ..)`, `"{p}"` (after `aio::client!(Todo, todo)`) |
| calling `todo.add(x)` from a handler | `act!(add(x))` |
| reading the whole cell | `todo()`: in handlers only |
| `signal(v)` at module level | `static S: Global<T> = Global::new(\|\| v);` |
| `useLocal(v)` | `signal(\|\| v)` |
| `computed(..)` | `memo(..)` |
| `effect(..)` | `effect(..)` |
| `onMount`, `onCleanup`, `onUnmount` | `once`, `on_cleanup` |
| `afterRender(() => el.scrollIntoView(..))` | `"data-aio-reveal"` on the row, or `onmounted` + `MountedData::scroll_to` |
| `memo(Component)` | nothing: components with `PartialEq` props skip equal renders |
| `batch(..)` | nothing: a handler's writes render once |
| `useVirtualList` | `aio::kit::VirtualList`, `aio::kit::window` |
| `useForm`, `useFieldArray` | `aio::kit::use_form`, `move_item` |
| `useRoute`, `navigate`, `<Route>`, `<Outlet>`, `<Link>` | `aio::routes!`, `aio::router!()`: `here()`, `go`, `Route`, `Outlet`, `Link` ([routing](../ui/routing.md)) |
| `useHead`, `collectHead` | `aio::head::use_head` (`aio_shell::page::use_head` re-exports it) |
| `useUser`, `<SignIn>` | `aio::kit::SignIn`, the session's `Caller` |
| `ErrorBoundary` | `ErrorBoundary { fallback: \|e: String\| rsx! { .. }, .. }` |
| `aio/ui`: `Alert` `Avatar` `Breadcrumb` `Button` `Card` `Checkbox` `Confirm` `ConfirmButton` `EmptyState` `Field` `Input` `Markdown` `Menu` `Modal` `Pagination` `Progress` `RadioGroup` `Row` `Select` `Skeleton` `Spinner` `Stack` `Switch` `Table` `Tabs` `Textarea` `ToastHost` `toast` `Tooltip` `ZoomPan` `fitView` `zoomAt` | `aio::kit`, the same names (`fit_view`, `zoom_at`) ([kit](../ui/kit.md)) |
| `UI_CSS`, `UiStyles`, `css`, `collectCss`, `cx` | `aio::kit::CSS`, `aio::use_css!`, `aio::ui::cx(&[("on", cond)])` |
| `guestPreload` | built in: `aio::kit::Browser` loads aio-shell's guest script |
| aio's theme, `ui.theme`, `ui.hue`, `ui.layout` | `aio::kit::theme_css(hue)`, `--aio-*` tokens, `[ui]` ([theming](../ui/theming.md)) |
| CSS files, `build.css` | `aio::use_css!(dark, &theme; "assets/style.css")`, `[build] css` |
| `<Transition>`, `useSpring` | `aio::kit::Transition`, `use_spring` |
| `island`, `reactIsland`, `<Browser>` | `aio::kit::WebIsland` (`Island::js`, `Island::react`), `aio::kit::Browser` |

### The rest of `aio/air`

| aio | aio-rs |
|---|---|
| `h`, `Fragment` | `rsx!`, `Fragment {}` |
| `mount`, `hydrate`, `page` | `aio_shell::run(App, ..)` (the window), `aio::web::launch(App)` (the web build) |
| `renderToString`, `renderToStream` | `ax preview <app> <component> --html`; a served page paints its first view itself ([web](../clients/web.md)) |
| `Show`, `Suspense`, `lazy` | `if`; `Suspense { fallback, .. }` with `resource(..).suspend()?`; no lazy component loading (one binary) |
| `Defer` | `aio::kit::Defer` |
| `Portal` | `aio::kit::Portal`, `PortalHost` |
| `NavLink`, `Redirect`, `routePath`, `routeSearch`, `useNavigate` | from `aio::router!()`: `Link`, `Redirect`, `here()`, `params()`, `go(..)` |
| `TransitionGroup`, `fade`, `scale`, `slide` | `aio::kit::TransitionGroup`, `Motion` |
| `createContext`, `useContext`, `useContextSelector` | `provide(\|\| v)`, `context::<T>()`; `aio::kit::use_context_select` |
| `useSignal`, `untrack`, `trackedMemo` | `signal`; `s.get_untracked()`; `part(..)` / `name_at(..)` |
| `useRef` | `signal`, or `onmounted` for an element |
| `useId` | `aio::kit::use_id(prefix)` |
| `useInterval`, `useRaf` | `aio::kit::use_interval`, `use_frame` |
| `onWindowEvent`, `useDimensions` | `aio::kit::use_window_event(WindowEvent::Resize, ..)` |
| `onBackButton` | `aio::kit::use_back_button` |
| `useProjection` | `aio::client!(Cell, name, name_at)`: `name_at(\|c\| value)` |
| `useAio` | `act!(..)`, `aio::call!(door, m(..))` |
| `useConnected`, `isConnectionDegraded` | `use_status()`; `aio::kit::ConnectionStatus` |
| `useResource`, `resource` | `aio::kit::use_held`, or an effect writing the result into the cell |
| `on`, `watch` | `h.on_change(..)`, `h.listen(..)`; in a view `use_effect` |
| `useState`, `useMemo`, `useEffect`, `useCallback` (React names) | `use_signal`, `use_memo`, `use_effect`; a closure (no `useCallback` needed) |
| `useTimeTravel` | `use_time_travel()` + `aio::kit::TimeTravelPanel` ([time travel](../debugging/time-travel.md)) |
| `bindInput` | `value: "{v}", oninput: ..`, or `aio::kit::use_form` |
| `signOut` | `aio::auth::client::Client::logout` |
| `appVersion` | `aio::app_version!()` |
| `nativeFetch` | `aio::capabilities::agent(..)` (feature `outbound`), held to `[capabilities] network` |
| `print` | none: the window's own print (Ctrl+P) |
| `requestNotificationPermission` | not needed: `cx.tell(Tell::notify(..))` asks the OS |
| `connectAioDevTools`, `connectReduxDevTools`, `setDevMode` | `ax components`, `ax events`, `ax ui`; a debug build is dev mode |

### Other exports of `aio`

| aio | aio-rs |
|---|---|
| `call` | `aio::call!(door, method(..))` |
| `self("tick")`, `own` | a method calls its own cell's method directly, or `cx.after(key, ms, "tick", args)` |
| `blocking` | `tokio::task::spawn_blocking` inside an effect |
| `isCellWorker` | none (no worker cells) |
| `table`, `pk`, `integer`, `text`, `real`, `bytes`, `count`, `ref` | `#[table("name")]` on a `Vec`/map field of `#[data]` rows; columns are the row's Rust types |
| `serverUser`, `serverRequest` | `cx.by()`, `cx.role()` in a method; the request in a route or server function |
| `serverAuth`, `createAuthClient`, `authClient` | `[accounts]`; `aio::auth::client::Client` |
| `serverImport`, `aio/server-only`, `aio/client-only` | `#[cfg(..)]` and Cargo features: server code isn't in the web build ([dev](../build/dev.md)) |
| `degraded`, `degradedReport` | `aio::health::part(name)`, `aio::health::degraded()` |
| `errorCode`, `errorSummary` | `Rejected::code()`, `Rejected::why` |
| `log` | `aio::info!`, `aio::warn!`, … ([errors](../debugging/errors.md#logs)) |
| `dur` | `aio::fmt::dur` |
| `VERSION` | `aio::VERSION` |
| `notify`, `appIcon` | `cx.tell(aio::tell::Tell::notify(..))`, `Tell::Icon { .. }` |
| `route`, `serverFn`, `serverFns`, `schedule` | see the tables above |
| `aio/updates` (`updates` cell) | `aio::update::Updates` |
| `aio/feedback` (`feedback` cell) | `aio::feedback::Feedback`, `aio::feedback::cell()` |
| `aio/cli` (`EXIT`, `fail`, `parseCli`) | `aio::cli` (`OK`, `ERROR`, `USAGE`, `fail`, `Spec`, `parse`) ([CLI apps](../clients/cli-apps.md)) |
| `aio/db` (`DB`, `createDB`, `reactiveDB`) | `aio::db::Db`, live queries ([SQL](../persistence/sql.md)) |
| `aio/sync` (HLC, `mergeField`, `rebase`, `compactSyncOps`) | internal: `aio::merge` for app merges; aio's ops are read by `compat` |
| `aio/extras` (`teachMessage`, `checkCells`, `deepFreeze`) | `aio::report::teach`; `ax lint`; not needed |
| `aio/build`, `aio/ship`, `aio/build-all`, `aio/dev-android`, `aio/android-install`, `aio/electron-install` | `ax build`, `ax release`, `ax dev --android`, `ax install --android`, `ax setup` |
| `aio/am`, `aio/amui`, `aio/doctor` | `ax`, `ax ui`, `ax doctor` |
| `aio/aiol` | none |
| `aio/state-core`, `aio/jsx-runtime`, `aio/air/compat` | not needed |

### Keys

| aio | aio-rs |
|---|---|
| `onGlobalKey("mod+k", f)` | `aio::kit::use_global_key("mod+k", f)` under `GlobalKeys` |
| a window keydown listener in capture mode | `use_key_claim("Tab", f)` |
| `REPEATABLE_KEYS` | built in: only movement repeats; `Chord::repeating(true)` |
| `e.preventDefault()` on arrows, Tab | `"data-aio-prevent": "ArrowUp ArrowDown Tab"` on the element |
| `form.requestSubmit()` on Enter | built in (`page::ENTER`) |
| keys held while a dialog opens | not needed: calls are synchronous |

### Tests

| aio | aio-rs |
|---|---|
| `testCell(cell, name, t => t.send.add(..))` | `#[test]` + `Handle::<C>::memory()`, `h.add(..)` |
| `t.expect.state(s => ..)` | `assert!(..h.state()..)` |
| `bootCells`, `advance(ms)` | `TestCells::new()`, `t.advance(ms)`, `t.settle()` |
| `testUI(App, ..)`, `ui.SaveButton.click()` | `TestUi::new(App)`, `ui.click("SaveButton")` |
| `ui.expectCell(todo, ..)` | `HANDLE.peek().state()` or `ui.expect_cell(..)` |
| `testServer`, `testMultiClient` | `TestServer`, `Sim` |
| `cassette`, `stub` | `aio::cassette::Cassette` |
| `testComponent(C, props)` | `aio::testing::Mount::default().component(C, props)`; `ax preview <app> <component>` |
| `testBrowser`, `findChromium` | `aio::testing::TestBrowser` |
| `testPageFetch` | `TestServer::page()` |
| `totpCode` | `aio::testing::totp_code` |
| `uiNames`, `uiRects`, `generateUITypes`, `testGen` | `ax surface <app> --names`, `--rects`; names are checked when a test runs (no generated types) |
| `smoke`, `testApps` | `ax build <app> --smoke`, `ax smoke` |
| `crashWorker` | none (no worker cells) |
| `freePort` | bind `127.0.0.1:0` |
| `deno test` | `ax test <app>` (`cargo test`) |

### Build, release, CLI

| aio `am` | aio-rs `ax` |
|---|---|
| `am dev` | `ax dev <app>` (edits hot-patch in) |
| `am build`, `am compile` | `ax build <app>` (`--deb`, `--appimage`, `--windows`, `--android`, `--web`) |
| `am release`, `am publish`, signing keys | `ax keygen <app>`, `ax release <app>` ([release](../build/release.md)) |
| `build.targets`, `--platforms` | `[build] targets`, `[build] platforms` ([targets](../build/targets.md)) |
| the `updates` cell, `canApply` | `aio::update::Updates`, `aio::update::can_apply` |

`am` verb by verb:

| `am` | `ax` |
|---|---|
| `start`, `stop`, `kill`, `restart`, `status` | the same |
| `watch` | `ax dev <app>` (edits patch in or restart it) |
| `instances` | `ax apps`, `ax status <app>` |
| `state`, `expect`, `snapshot`, `migrations`, `replay`, `record`, `timetravel` | the same |
| `timeline`, `actions` | `ax events <app> [cell] --diff`, `ax timetravel` |
| `dispatch` | `ax call <app> <cell> <method> [arg…]` (`--as user` as a user; the operator's own door otherwise) |
| `persist` | not needed: every commit is saved; `--drop`: `#[cell(retire = [..])]` |
| `ui`, `open`, `data`, `config`, `backup`, `restore`, `report`, `clients` | the same |
| `client <index>` | `ax components <app>` |
| `surface`, `trigger`, `shot`, `eval`, `preview`, `peer`, `feedback` | the same |
| `where`, `check` | none: `cargo build` checks every build; which code runs where is `#[cfg]` ([dev](../build/dev.md)) |
| `migrate`, `fix`, `link` | `ax lint --fix` (aio-rs's own layout upgrades); no `dep/aio` symlink |
| `testgen` | none: `TestUi` names are checked at run time; `ax record` writes a test from the running app |
| `sql`, `tables`, `schedules`, `logs`, `errors`, `metrics`, `heap`, `cost`, `health`, `doctor` | the same |
| `top` | `ax heap <app> --watch` |
| `discover`, `profile`, `pair`, `pin`, `auth`, `trust` | the same |
| `create`, `build`, `dev`, `lab`, `theme`, `prune`, `remove`, `installed`, `version`, `agent`, `help` | the same |
| `compile` | `ax build <app>` |
| `publish` | `ax keygen <app>`, `ax release <app>` |
| `add cell`, `add server` | none: add a module by hand |
| `upgrade <app>` | `ax install <app>`; `ax update <app>` for a signed release |
| `upgrade` (am itself), `uninstall` | rebuild or remove `ax` the way it was installed ([setup](../basics/setup.md)) |

`ax` has these without an `am` twin: `keygen`, `release`, `update`, `smoke`,
`install`, `lint`, `test`, `events`, `call`, `components`, `adopt`,
`import`, `runtime`, `setup`.

Every verb: [ax](../clients/ax.md).

## 3. A worked example

A list of contacts with a keyboard cursor and a filter, as aio apps write
it: a persisted cell with a cursor, a memoized filtered list, a windowed
view and a held-arrow binding.

### aio

```ts
// src/cell/contacts.ts
import { cell, createSelector } from "aio";

export interface Person { id: number; name: string; email: string }

export const contacts = cell("contacts", {
  version: 2,
  persist: true,
  state: { people: [] as Person[], filter: "", activeIndex: 0, nextId: 1 },
  methods: {
    add(s, name: string, email: string) {
      if (!name.trim()) throw new Error("a name, please");
      s.people.push({ id: s.nextId++, name, email });
    },
    setFilter(s, text: string) {
      s.filter = text;
      s.activeIndex = 0;
    },
    moveDown(s, max: number) {
      s.activeIndex = Math.min(Math.max(0, max - 1), s.activeIndex + 1);
    },
    moveUp(s) {
      s.activeIndex = Math.max(0, s.activeIndex - 1);
    },
  },
  selectors: {
    // Recomputed only when people or filter change: a cursor step reuses it.
    shown: createSelector(
      (s) => s.people,
      (s) => s.filter,
      (people, filter) =>
        people.filter((p) => p.name.toLowerCase().includes(filter.toLowerCase())),
    ),
  },
});
```

```tsx
// src/ui/ContactList.tsx
import { afterRender, onGlobalKey } from "aio/air";
import { contacts } from "../cell/contacts.ts";

const SHOWN = 40;

function Row({ p, active }: { p: Person; active: boolean }) {
  return (
    <li id={`c-${p.id}`} data-cursor={active || undefined}>
      {p.name} <span class="email">{p.email}</span>
    </li>
  );
}

export function ContactList() {
  const rows = contacts.shown();
  const active = Math.min(contacts.activeIndex, Math.max(0, rows.length - 1));
  onGlobalKey("ArrowDown", () => contacts.moveDown(rows.length));
  onGlobalKey("ArrowUp", () => contacts.moveUp());
  afterRender(() =>
    document.getElementById(`c-${rows[active]?.id}`)
      ?.scrollIntoView({ block: "nearest", behavior: "auto" })
  );
  const from = Math.max(0, active - SHOWN / 2);
  return (
    <ul t="contacts">
      {rows.slice(from, from + SHOWN).map((p, i) => (
        <Row key={p.id} p={p} active={from + i === active} />
      ))}
    </ul>
  );
}
```

The app ran with aio's default `persistDebounceMs` (100 ms), so the cursor's
steps were written at most ten times a second.

### aio-rs

The cell. Field names stay aio's on disk (`camelCase`), the id and version
stay aio's, `moveDown` stays a method, and the cursor methods are
`#[coalesce]` (aio's debounced writes). `createSelector` over `people` and
`filter` becomes `#[derived(reads = [people, filter])]`.

```rust
use aio::prelude::*;
use std::sync::Arc;

#[data]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Person {
    pub id: u32,
    pub name: String,
    pub email: String,
}

#[cell(persist, id = "contacts", version = 2)]
#[serde(rename_all = "camelCase")]
pub struct Contacts {
    pub people: Vec<Person>,
    pub filter: String,
    pub active_index: usize,
    pub next_id: u32,
}

#[methods]
impl Contacts {
    pub fn add(&mut self, name: String, email: String) -> Result {
        ensure!(!name.trim().is_empty(), "a name, please");
        self.people.push(Person { id: self.next_id, name, email });
        self.next_id += 1;
        Ok(())
    }

    pub fn set_filter(&mut self, text: String) {
        self.filter = text;
        self.active_index = 0;
    }

    #[coalesce]
    pub fn move_down(&mut self, max: usize) {
        self.active_index = (self.active_index + 1).min(max.saturating_sub(1));
    }

    #[coalesce]
    pub fn move_up(&mut self) {
        self.active_index = self.active_index.saturating_sub(1);
    }

    /// Kept, the same `Arc`, while `people` and `filter` stay the same.
    #[derived(reads = [people, filter])]
    pub fn shown(&self) -> Arc<Vec<Person>> {
        Arc::new(matching(&self.people, &self.filter))
    }
}

/// aio's selector body, shared by the cell and the view.
pub fn matching(people: &[Person], filter: &str) -> Vec<Person> {
    let f = filter.to_lowercase();
    people.iter().filter(|p| p.name.to_lowercase().contains(&f)).cloned().collect()
}

# fn main() {
// aio's testCell(contacts, ..) as a plain test.
let h = Handle::<Contacts>::memory();
h.add("Ada".into(), "ada@example.org".into()).unwrap();
h.add("Alan".into(), "alan@example.org".into()).unwrap();
assert_eq!(h.add(" ".into(), "".into()).unwrap_err().why, "a name, please");
let rows = h.state().shown();
h.move_down(rows.len()).unwrap();
assert_eq!(h.state().active_index, 1);
assert!(Arc::ptr_eq(&rows, &h.state().shown()), "the cursor step reused the rows");
h.set_filter("ala".into()).unwrap();
assert_eq!(h.state().shown().len(), 1);
# }
```

The view. `contacts.activeIndex` read in a render becomes
`part(|c| &c.active_index)`; `contacts.shown()` becomes the same selector
body behind an `aio::Cache` keyed on the stamps of the two fields it reads,
so the rows are built once per change of `people` or `filter`, never per
cursor step; the row takes its data by props; `afterRender` +
`scrollIntoView` becomes `data-aio-reveal`; the arrows are kept from the
browser with `data-aio-prevent`.

```rust,no_run
# use aio::prelude::*;
# use std::sync::Arc;
# #[data]
# #[derive(Clone, Debug, Default, PartialEq)]
# pub struct Person { pub id: u32, pub name: String, pub email: String }
# #[cell(persist, id = "contacts", version = 2)]
# #[serde(rename_all = "camelCase")]
# pub struct Contacts { pub people: Vec<Person>, pub filter: String, pub active_index: usize, pub next_id: u32 }
# #[methods]
# impl Contacts {
#     #[coalesce]
#     pub fn move_down(&mut self, max: usize) { self.active_index = (self.active_index + 1).min(max.saturating_sub(1)); }
#     #[coalesce]
#     pub fn move_up(&mut self) { self.active_index = self.active_index.saturating_sub(1); }
# }
# pub fn matching(people: &[Person], filter: &str) -> Vec<Person> {
#     let f = filter.to_lowercase();
#     people.iter().filter(|p| p.name.to_lowercase().contains(&f)).cloned().collect()
# }
use aio::act;
use aio::kit::{GlobalKeys, use_global_key};
use aio::ui::prelude::*;
// Both preludes name a `Result`; in the cell's file the methods mean `aio::Result`.
use aio::Result;

aio::client!(Contacts, contacts);

const SHOWN: usize = 40;

/// The filtered rows per (people, filter) stamp: aio's createSelector.
static ROWS: aio::Cache<(u64, u64), (), Arc<Vec<Person>>> = aio::Cache::new();

/// aio's `Row`: its data by props, so an unchanged row doesn't render.
#[component]
fn Row(name: String, email: String, active: bool, at: usize) -> View {
    rsx! {
        li {
            "data-cursor": active.then_some(""),
            "data-aio-reveal": active.then(|| at.to_string()),
            "{name} "
            span { class: "email", "{email}" }
        }
    }
}

#[component]
pub fn ContactList() -> View {
    // Re-renders when people, filter or the cursor change; nothing else.
    let (people, filter) = (part(|c| &c.people), part(|c| &c.filter));
    let rows = ROWS.get((people.ver(), filter.ver()), (), || Arc::new(people.with(|p| filter.with(|f| matching(p, f)))));
    let active = part(|c| &c.active_index).get().min(rows.len().saturating_sub(1));
    let len = rows.len();
    use_global_key("ArrowDown", move |_| _ = act!(move_down(len)));
    use_global_key("ArrowUp", |_| _ = act!(move_up()));
    let from = active.saturating_sub(SHOWN / 2);
    rsx! {
        ul { "data-t": "contacts", "data-aio-prevent": "ArrowUp ArrowDown",
            for (i, p) in rows.iter().enumerate().skip(from).take(SHOWN) {
                Row { key: "{p.id}", name: p.name.clone(), email: p.email.clone(), active: i == active, at: i }
            }
        }
    }
}

#[component]
pub fn App() -> View {
    use_live();
    rsx! { GlobalKeys { ContactList {} } }
}
# fn main() {
let mut ui = aio::testing::TestUi::new(App);
// The keys are the window's (GlobalKeys): pressed on the focused page.
ui.burst(&["ArrowDown"]);
# }
```

What changed is syntax; what was kept is every decision: the same cell,
methods, memoized rows, windowed list, CSS cursor, debounced cursor writes
and key handling. A held ↓ costs the list and the two rows that changed,
and one write per 100 ms.

## 4. The speed tricks aio apps rely on

aio made most of these for free, so an aio app's code doesn't show them.
Each has its aio-rs form; the [performance guide](../performance.md) has
the details.

| aio | What it gave | aio-rs |
|---|---|---|
| one signal per cell (`trackCell`): a read subscribes to that cell | a cursor step re-renders only its readers | `part(\|c\| &c.field)`; split fast fields from slow ones |
| `persistDebounceMs` (100 ms) | no fsync per keystroke | `#[coalesce]`, `#[cell(coalesce)]` |
| Immer patches on the wire | a step costs bytes | built in: views get DOM edits for what changed |
| the broadcast coalescer (~150 ms), `flushAllUrgent` | background churn batched; a keystroke never waits | built in: `aio::ui::BACKGROUND` |
| `createSelector`, `memoLast` | derived rows not rebuilt per step | `#[derived]`, `#[derived(reads = [..])]` |
| caches on an array's identity (`groupBy`, indexes, a per-frame panel cache) | indexes built once per change | `aio::Cache` keyed on `Ver::of(&state)` or `part(..).ver()` |
| reading a cell's arrays without copying | no copy per read | `Arc` state; big fields as `Arc<…>` or `aio::imbl` |
| shallow-equal props, `memo` | unchanged rows skip | `#[component]` props (`PartialEq`) |
| renders batched per microtask | many writes, one render | built in (`aio::ui`) |
| event delegation | few listeners | built in (`aio::ui`) |
| windowed lists, stable keys | bounded DOM | `kit::VirtualList`, or a window by hand with `key:` |
| `scrollIntoView({ block: "nearest", behavior: "auto" })` only when the cursor moved | no smooth scrolls piling up | `data-aio-reveal` |
| scroll re-anchoring per frame | at most one window change per frame | built in: `VirtualList` renders only when its window moves |
| selection drawn by CSS attributes, `transition: none` on rows | a step changes two attributes | the same CSS, `"data-cursor": active` |
| cursor clamped in the view | no write when a list shrinks | `.min(len - 1)` in the render |
| `REPEATABLE_KEYS` | held keys can't re-fire actions | built in; `Chord::repeating` |
| mousemove throttled | idle timers cost nothing | built in: `kit::MOVE_MS` |
| measure only on resize, publish after two equal reads | no measuring loops | the same logic, in `onresize` / `onmounted` handlers |
| drags painted into CSS variables, one write at the end | 60 fps drags | the same: CSS variables from the page, one call at the end |
| `ResizeObserver` + rAF | one refit per frame | the same, in an `Island` or `use_window_event(Resize)` |
| prefetch after the cursor rests, idle back-off | no I/O while navigating | `cx.after("prefetch", 180, ..)` (the same key replaces), `Mode::Newest` |
| work ranked by distance to the cursor, visible rows first | what is looked at resolves first | the same order in the effect that does it |
| object-URL caches, posters for animated media, a few loads at once | fast galleries | the same in the view; bytes in `aio::blobs` |
| theme switch without transitions for two frames | no flash | the same CSS rule |

## 5. Data: the same state.db

- `[app] port_of = "<the aio app's appId>"` runs the port on that app's home and store
  (`--home`, `--profile`, `$AIO_APPS_DIR`, `~/.<appId>`, and the app's own
  `[home]` rules). Both take the same lock, so the aio app and its port never run on
  the same data at once.
- A cell's `id` is its aio cell name, its serde names are aio's field names
  (`#[serde(rename_all = "camelCase")]`), and its `version` is aio's
  `__versions` stamp. An older stamp migrates (after a backup); a newer
  one refuses the boot.
- Every commit also writes the cell into aio's `aio_kv` rows, in aio's
  layout, in the same transaction, so the aio app reads what aio-rs wrote. aio-rs
  keeps its own history in tables of its own beside them.
- One aio-rs cell can own many aio cells (`#[aio_cell] settings: Settings`), and
  aio's `db:` tables map to `#[table("name")]` fields.

```rust
use aio::prelude::*;

#[cell(persist, id = "contacts", version = 2)]
#[serde(rename_all = "camelCase")]
pub struct Contacts {
    pub people: Vec<String>,
    pub active_index: usize,
}

#[methods]
impl Contacts {
    pub fn add(&mut self, name: String) {
        self.people.push(name);
    }
}

# fn main() {
let dir = tempfile::tempdir().unwrap();
let file = dir.path().join("state.db");
// The file as the aio app left it: its snapshot in aio_kv, the cell's version stamp.
let db = rusqlite::Connection::open(&file).unwrap();
db.execute_batch(r#"
    CREATE TABLE aio_kv (k TEXT PRIMARY KEY, v TEXT NOT NULL);
    INSERT INTO aio_kv VALUES ('state', '{"contacts":{"people":["Ada"],"activeIndex":0}}');
    INSERT INTO aio_kv VALUES ('my-app:__versions', '{"contacts":2}');
"#).unwrap();

let h = Handle::<Contacts>::open_aio(&file, "my-app").unwrap();
assert_eq!(h.state().people, ["Ada"], "aio's state, as it was");
h.add("Alan".into()).unwrap();

// What the aio app reads back: its own row, in its own shape.
let v: String = db.query_row("SELECT v FROM aio_kv WHERE k = 'state'", [], |r| r.get(0)).unwrap();
let state: serde_json::Value = serde_json::from_str(&v).unwrap();
assert_eq!(state["contacts"]["people"], serde_json::json!(["Ada", "Alan"]));
# }
```

The whole format, the home rules and what makes aio refuse a file:
[data](data.md).

## 6. The wire: aio pages, tools and apps

With aio-rs's feature `compat`, a served aio-rs app speaks aio's protocol
3 on `/ws`, so aio's pages, `am` and other aio tools keep working against
it, and `aio::compat::AioLink` lets an aio-rs app call an aio app.
Pairing, discovery, peers and blobs use aio's formats without the feature.
Details: [wire](wire.md).

## 7. Checklist

- [ ] `ax create <name>`; `[app] id` set; `[app] port_of` set to the aio
      app's appId if the port must open its data.
- [ ] Every aio cell is a `#[cell]` with the same `id`, `version` and field
      names on disk (`rename_all = "camelCase"`), the same methods, the
      same refusals (`throw` → `reject!`/`ensure!`).
- [ ] Methods are pure: `Date.now()`, ids and randomness come from `cx`;
      I/O moved into effects or async methods.
- [ ] Every method called per keypress, scroll or pointer move is
      `#[coalesce]` (or the cell is `#[cell(persist, coalesce)]`).
- [ ] Every selector is `#[derived]`; every memoized one (`createSelector`,
      `memoLast`, an identity cache) is `#[derived(reads = [..])]` or an
      `aio::Cache`.
- [ ] No render reads the whole cell (`name()`): each reads its fields with
      `part`. The dev build's `[hint]` lines are gone.
- [ ] Rows take their data as props; long lists draw a window with stable
      keys; the cursor is a CSS attribute with `data-aio-reveal`.
- [ ] Keys: `use_global_key` under `GlobalKeys`, `data-aio-prevent` where
      aio called `preventDefault`, `use_key_claim` where aio captured.
- [ ] Every aio test has its aio-rs test (`Handle`, `TestCells`, `TestUi`).
- [ ] The port opens a copy of real aio data, and the aio app opens it back
      afterwards.
- [ ] Measured against the aio app on the same machine: keypress to paint, CPU,
      memory, start time. The port is not done while it is slower.
- [ ] `ax lint` clean; `ax build <app> --smoke` passes.

## 8. 0.1 → 0.2: aio::ui instead of dioxus

aio-rs 0.2 gives the views one renderer-neutral API, `aio::ui::prelude`,
in the shape air (aio-rs's own renderer, [design](../design/air.md)) has,
so the later swap from Dioxus to air is a framework change, not an app
migration. An app names `aio::ui` only: `ax lint` refuses a `dioxus` path,
a `dioxus` dependency, a page script's `dioxus.send`, and a `for` row
without `key:`. Most of the move is mechanical; the compiler finds every
place left. `scripts/aio-ui-migrate <app dir>` (in aio-rs; a clean git
tree) does the mechanical part: the renames below, then `cargo check`
again and again, fixing each `count()`, `.cloned()`, `.peek()` and
uncomparable `.set(v)` where the compiler points; it lists what is left
for a person (a `try_write` match, a `*part(..)` value).

| 0.1 (dioxus) | 0.2 (`aio::ui`) |
|---|---|
| `dioxus = { version = "=0.7.10", .. }` in `Cargo.toml` | gone: `aio = { .., features = ["kit"] }` brings the views |
| `use dioxus::prelude::*;` | `use aio::ui::prelude::*;` |
| `-> Element` | `-> View` |
| `use_signal(\|\| v)` | `signal(\|\| v)` (a `Copy` `Signal`: no `let mut` but for `+=`) |
| `count()`, `count.cloned()` | `count.get()` |
| `count.peek()` | `count.read_untracked()`, or `count.get_untracked()` |
| `count.with_mut(f)` | `count.update(f)` |
| `count.set(v)` | `count.set(v)`: readers are told only when `v` differs; a value without `PartialEq`: `*count.write() = v` |
| `use_memo(f)` | `memo(f)` (a `Memo`: `.get()`, `.with(..)`, `.read()`) |
| `use_effect(f)` | `effect(f)` |
| `use_effect(use_reactive!(\|d\| ..))` | `watch(d, move \|d\| ..)` |
| `use_hook(f)` | `once(f)` |
| `use_drop(f)` | `on_cleanup(f)` |
| `use_callback(f)` | `callback(f)` |
| `use_context_provider(f)` | `provide(f)` |
| `use_context::<T>()`, `try_use_context::<T>()`, `consume_context::<T>()` | `context::<T>()`, `try_context::<T>()` |
| `provide_root_context(v)` | `provide_root(v)` |
| `use_future(f)` | `task(f)` |
| `use_resource(f)` (`r()`, `r.read()`) | `resource(f)` (`r.get()`, `r.with(..)`, `r.suspend()?`) |
| `GlobalSignal<T>`, `Signal::global(f)` | `Global<T>`, `Global::new(f)` |
| `ErrorBoundary { handle_error: \|e: ErrorContext\| .. }` | `ErrorBoundary { fallback: \|e: String\| .. }` |
| `SuspenseBoundary { fallback: \|_\| .. }` | `Suspense { fallback: rsx! { .. } }` |
| `document::eval(js)` | `eval(js)` |
| `dioxus.send(v)`, `await dioxus.recv()` in a page script | `aio.send(v)`, `await aio.recv()` |
| `*part(\|c\| &c.f)`, `part(..).len()`, `part(..)[i]` | `part(\|c\| &c.f).get()`, `part(..).with(\|v\| v.len())`, `part(..).with(\|v\| v[i].clone())` |
| `name_at(..)()`, `use_status()()`, `use_pending(..)()` | `.get()` on the `Memo` they give |
| `TestUi::read(&GLOBAL)` of a `GlobalSignal` | the same, of a `Global` |
| `for x in xs { li { .. } }` | `for x in xs { li { key: "{x.id}", .. } }` |

What `aio::ui` already shapes for air, though Dioxus doesn't need it yet:
a view reads state where it shows it (`"{n}"`, `.get()` in `rsx!` or a
`memo`), a prop that must follow state is a `Signal`, `Memo` or part
handle, no `if let` inside `rsx!` (`{opt.map(|x| rsx! { .. })}`), and
`once` is the only one-time setup ([views](../ui/views.md#ready-for-air)).

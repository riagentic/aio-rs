# API map

Every public module and item of aio-rs's crates, one line each, grouped
by task, with the feature it needs. The full reference (every function,
its arguments, its examples) is built from the code:

```sh
cargo doc --no-deps -p aio-rs -p aio-rs-shell --open        # in an aio-rs checkout
cargo doc --no-deps -p aio-rs --open                        # in an app
```

An item's page is at its path: `aio::Handle` is `aio/struct.Handle.html`,
`aio::sync::Remote` is `aio/sync/struct.Remote.html`. A gated item shows
its feature in the docs ("Available on crate feature `server` only").

| Crate | Used as | Holds |
|---|---|---|
| `aio-rs` | `aio` | everything an app's code calls |
| `aio-rs-shell` | `aio_shell` | the process around the app: start, the window, serving, updates, Android |
| `aio-rs-macros` | through `aio` | `#[cell]`, `#[methods]`, `#[data]` |
| `aio-rs-config`, `aio-rs-config-macro` | through `aio` | `aio.toml`'s types (`aio::config`), `aio::config!()` |
| `aio-rs-cli` | the `ax` binary | no library API: [ax](../clients/ax.md) |

Most apps need one line: `use aio::prelude::*;`.

## Start here

| Item | What it is |
|---|---|
| `aio::prelude` | `By`, `Caller`, `Cell`, `CellData`, `Code`, `Cx`, `Door`, `Effect`, `Handle`, `Mode`, `Rejected`, `Result`, `Store`, `Uuid`, and the macros `act!`, `call!`, `cell`, `data`, `ensure!`, `methods`, `reject!` |
| `#[aio::cell]` | a struct becomes a cell; flags `persist`, `shared`, `id = "…"`, `version = n`, `migrate = f`, `visible = f`, `machine = field`, `coalesce`, `diagnostics = false`, `save`, `restore`, `retire`; field attributes `#[server]`, `#[transient]`, `#[table]` ([cells](../state/cells.md)) |
| `#[aio::methods]` | its `impl` becomes its methods, typed (`<Name>Calls`) and by name; method attributes `#[client]` (`role`, `allow`, `predict`), `#[invariant]`, `#[derived]`, `#[every]`, `#[cron]`, `#[ttl]`, `#[on_start]`, `#[on_stop]`, `#[on_aio]` |
| `#[aio::data]` | serde and the usual derives for your own types a cell holds |
| `aio::Handle<T>` | a cell in this process: `memory()`, `open(store)`, `state()`, typed calls, `call(name, args)`, `watch`, `listen`, `events`, `travel`, `done`, `shutdown` |
| `aio::Source<T>` | where the app's first cell comes from: `for_app`, `for_server`, a local store or a synced `Remote` |
| `aio::config!()` | `aio.toml`, parsed as the app builds ([reference](aio-toml.md)) |
| `aio::app_version!()` | the version `ax build` numbered |

```rust
use aio::prelude::*;

#[cell]
pub struct Counter {
    pub n: i64,
}

#[methods]
impl Counter {
    pub fn add(&mut self, by: i64) -> Result {
        ensure!(by > 0, "add a positive number");
        self.n += by;
        Ok(())
    }
}

# fn main() {
let h = Handle::<Counter>::memory();
h.add(2).unwrap();
let refused = h.add(0).unwrap_err();
assert_eq!(refused.code, None); // a method's own refusal has no code
assert_eq!(h.state().n, 2); // the refused call changed nothing
# }
```

## Cells, methods and refusals

| Item | What it is |
|---|---|
| `aio::Cell`, `aio::CellData` | the traits `#[cell]` implements: id, flags, methods by name, `project` (what a viewer sees) |
| `aio::Cx` | a method's context: `now()`, `id()`, `rand()`, `by()`, `role()`, `run(effect)`, `after`, `every`, `at`, `cron`, `cancel`, `abort`, `tell`, `backoff` |
| `aio::By`, `aio::Caller` | who called: a person, a schedule, an effect, a peer |
| `aio::Result<T>`, `aio::Rejected`, `aio::Code` | a method's result; a refusal (`why`, `code`); aio's refusal kinds (`ACCESS_DENIED`, …), stable |
| `ensure!`, `reject!` | refuse with a reason |
| `aio::Door` | what every door (handle, synced copy, test client) implements |
| `aio::Derived`, `Cache`, `Reads`, `Ver`, `Watching` | derived values, memoized on the fields they read |
| `aio::machine`, `machine!` | a cell's state machine: states and the methods that move between them |
| `aio::merge` | merging concurrent offline edits (`text3`, tokens) |
| `aio::before_call`, `aio::rewrite_call`, `aio::on_action`, `aio::on_effect` | hooks on every call (`aio::Call`: its state, and a network call's `aio::Request`), every committed call and every effect (aio's `beforeReduce`, `onAction`, `onEffect`) |
| `aio::visible` | the rows a caller may see |
| `aio::visible_by_default` | one `visible` for every cell without its own (aio's `cellDefaults.visible`) |
| `aio::keys!` | an enum of names usable as keys, CSS classes and handles |

## Effects, schedules, time

| Item | What it is |
|---|---|
| `aio::effect` (`Effect`, `Mode`, `retry`) | I/O a method asks for; its result comes back as a call. `Mode::Queue`, `Newest`, `First` decide a second run with the same key |
| `aio::Backoff`, `aio::Plan`, `aio::Inputs` | retries and their spacing |
| `aio::cron` | cron expressions, in UTC or local time (`[app] tz`) |
| `aio::time` | `sleep`, `sleep_until`, `timeout`, `Instant` for async methods and effects |
| `aio::tell` | telling people: a toast, a notification, the app icon's badge |
| `aio::held` | a resource held open per key (camera, socket) while anyone uses it |
| `aio::cassette` | record an effect's I/O once, replay it in tests |
| `aio::deciding` | pause server-call deadlines while a person decides |

## Running the app: `aio_shell`

| Item | What it is |
|---|---|
| `aio_shell::start` → `Start` | claims the app's home (a second launch hands over), starts its log, health and reports |
| `aio_shell::run(app, Window, dirs)` | the window on the shared Electron runtime |
| `aio_shell::serve(dirs)` | a headless server |
| `aio_shell::Window` | the window's size and options (`Window::of(&cfg)`) |
| `aio_shell::open_window`, `Child`, `Web` | a second window, a guest page |
| `aio_shell::desktop` | notifications and the tray menu |
| `aio_shell::os` | open a link, a file or a folder in the OS's own app |
| `aio_shell::page` | what only the window can do: scroll, caret, copy, print, file pickers |
| `aio_shell::update`, `delta`, `swap` | updates: check, download (whole or delta), stage, swap, roll back |
| `aio_shell::runtime`, `prune`, `fuses`, `switches` | the shared runtime: fetch, prune, its fuses, allowed Chromium switches |
| `aio_shell::splash`, `place`, `macmove`, `oneclick` | splash, window placement, macOS move to Applications, the Windows one-click exe |
| `aio_shell::instance`, `inspect` | one instance per app; the dev DevTools door `ax shot` uses |
| `aio_shell::android` | the Android host (target `android`) |
| `aio_shell::web` | serving the browser build (feature `web` of `aio-rs-shell`) |

In `aio`: `aio::boot` (the boot report), `aio::paths` (where files are),
`aio::lifecycle` (stop, restart, children), `aio::on_stopping`,
`aio::on_stop`, `aio::shutdown`, `aio::rt` (the background runtime).

## Views (feature `kit`)

| Item | What it is |
|---|---|
| `aio::client!` | the views' glue: `part(\|c\| &c.field)`, `use_live()` |
| `aio::act!` | call a method from a view; a refusal is logged |
| `aio::ui` | page helpers: sheets, `use_css!`, Back, links, ids |
| `aio::kit` | controls, forms (`use_form`), dialogs, toasts, `VirtualList`, `Transition`, `Spring`, `Portal`, `Island`, `SignIn`, `TimeTravelPanel`, `Markdown` ([kit](../ui/kit.md)) |
| `aio::route`, `routes!`, `router!` | routes, params, history, links, nested routes ([routing](../ui/routing.md)) |
| `aio::head` | the page's title and meta tags |
| `aio::colors`, `aio::kit::theme` | the colours from one hue, the `--aio-*` tokens |
| `aio::page` | the page the views are drawn in (with `kit` or `web`) |
| `aio::ui::prelude` | the view API (with `kit`): `rsx!`, `#[component]`, `View`, `Signal`, `signal`, `memo`, `effect`, `watch`, `once`, `on_cleanup`, `provide`/`context`, `callback`, `task`, `spawn`, `resource`, `Global`, `ErrorBoundary`, `Suspense`, `eval`, the event types, `Key`, `Modifiers` ([views](../ui/views.md)) |
| `aio::web` | the app in the browser's own page (feature `web`, `ax build --web`) |

## Sync, server and HTTP (feature `server`)

| Item | Feature | What it is |
|---|---|---|
| `aio::sync` (`serve`, `Remote`, `Options`, `Link`, `Live`) | `server` | serving cells on `/ws`; a synced copy |
| `aio::http` (`Route`, `Req`, `Reply`, `serve_file`, cookies) | `server` | HTTP routes beside sync |
| `aio::fns` (`ServerFn`) | `server` | typed server functions |
| `aio::gate`, `aio::security` | `server` / none | the request gate (Host, Origin, limits); security headers |
| `aio::peer` | `server` | calling another aio-rs app on this machine |
| `aio::door`, `aio::ipc` | none | the local door (`ax state`, `ax call`) over a private socket |
| `aio::tls`, `aio::pairing`, `aio::cpace`, `aio::discovery` | `tls` | TLS, PIN pairing, LAN discovery |
| `aio::auth` | none (`oidc` for `auth::oidc`) | accounts, roles, passwords, TOTP, sessions |
| `aio::capabilities` | none (`outbound` for the client) | the hosts outbound requests may reach |
| `aio::plugin` | none | a crate's cells, routes and hooks as one value |
| `aio::compat` | `compat` (data: none) | aio's `state.db`, aio clients on `/ws`, `AioLink` |

## Data

| Item | What it is |
|---|---|
| `aio::Store`, `Event`, `Position`, `Travel`, `Steps` | the store: state and its event log, one SQLite file |
| `aio::db` (`Db`, `Opts`, `Rows`, `Tx`, `Live`, `Select`, `Where`) | raw SQL in the app's own file, live queries, aio's `where` objects |
| `aio::blobs` | content-addressed bytes beside the cells |
| `aio::backup` | backup and restore of the store |
| `aio::patch` | JSON patches (what travels on the wire) |
| `aio::imbl` | persistent collections (feature `imbl`) |

## Testing, debugging, the command line

| Item | What it is |
|---|---|
| `aio::testing::TestCells` | cells in a sandbox: `cell`, `start`, `effects`, `settle`, `advance`, `fuzz`, `as_client`, `saved` |
| `aio::testing::TestUi` | the real views headless (feature `kit`) |
| `aio::testing::TestServer`, `Sim`, `TestBrowser` | a server, simulated sync, a browser page (feature `server`) |
| `aio::testing::{expect, seeded, totp_code}` | helpers |
| `aio::log`, `trace!` … `error!` | the app's log (`logs/app.log`, `error.log`, `warning.log`) |
| `aio::report`, `aio::feedback` | problem reports; a "Report a problem" UI |
| `aio::health`, `aio::vitals`, `aio::perf`, `aio::budgets`, `aio::cost` | health, vitals, perf budgets, size budgets, what moves on links |
| `aio::video` | frames into a video (`ax test --video`, `ax shot --video`) |
| `aio::cli` | an app's own command line: `args`, `Spec`, prompts, tables, progress ([CLI apps](../clients/cli-apps.md)) |
| `aio::fmt` | sizes, durations, counts as people read them |
| `aio::update` | release manifests, signatures, the data gate, the `updates` cell |
| `aio::VERSION`, `uuid_v4`, `uuid_v7`, `random_bytes`, `sha256` | utilities |

## aio's API in aio-rs

| aio | aio-rs |
|---|---|
| `cell(name, config)` | `#[cell]` + `#[methods]` |
| `aio.run(config)` | `aio_shell::start` + `Source::for_app` + `run`/`serve`; options in `aio.toml` |
| `call(opts, fn)`, `errorCode(err)` | `h.call(name, args)`, `Rejected::code` |
| `markAsync`, async methods | `async fn` methods, effects |
| `notify`, `appIcon` | `cx.tell(…)` (`aio::tell`), `aio_shell::desktop` |
| `route`, `serveFile`, `serverUser`, `serverRequest` | `aio::http` (`Route`, `serve_file`, `Req`) |
| `serverFns`, `serverFn` | `aio::fns::ServerFn` |
| `generateTotpSecret`, `verifyTotp` | `aio::auth` |
| `composeCells` | several `Handle::open` on one store; `aio::plugin` |
| `until`, `race`, `sleep` | `Handle::until`, `aio::time::{sleep, timeout}`, `tokio::select!` in an async method |
| `blocking(id, fn)` | an effect (it runs off the cell's thread) |
| cell config `selectors` | `#[derived]` (`aio::Derived`) |
| `listensTo` | `Handle::listens_to` |
| `validate` | `#[invariant]` |
| `persist`, `visible`, `access`, `sync` | `#[cell(persist)]`, `#[cell(visible = f)]`, `#[client(role = …)]` and `#[server]` fields, `#[cell(shared)]` |
| `version`, `onMigrate` | `#[cell(version = n, migrate = f)]` |
| `useAio`, `useProjection` | `client!`'s `part` |
| `useLocal` | `aio::ui::signal` |
| `useTimeTravel` | `kit::TimeTravelPanel`, `Handle::travel` |
| `useRoute`, `navigate`, `<Route>`, `<Link>`, `<Redirect>` | `routes!`, `router!` |
| `useHead` | `aio::head` |
| `createDB`, `query`, `execute`, `transaction` | `aio::db::Db` |
| `table(...)` columns | `#[table]` fields ([SQL](../persistence/sql.md)) |
| `createSelector` | `aio::Derived` |
| `beforeReduce` | `aio::before_call` |
| `testCell`, `t.send`, `t.settle`, `t.randomActions` | `TestCells`, typed calls, `settle` (`settle_for` under `real_time`), `random_calls` |
| `testUI`, `testServer`, `testBrowser` | `TestUi`, `TestServer`, `TestBrowser` |
| `log.info(tag, msg)` | `aio::info!` |
| `parseCli` | `aio::cli::args` |
| `VERSION` | `aio::VERSION` |

Gaps, aio items with no aio-rs equivalent:

- `bindCell`, `deepFreeze`, `freezeState`, `guardDispatches`: no frozen
  state to guard; a method gets a copy and its state is `Arc`'d.
- `connectReduxDevTools`, `disconnectReduxDevTools`: none; `ax
  timetravel` and `kit::TimeTravelPanel` instead.
- `createSliceSelector`: none; `part` reads one field.
- `checkCells`, `instances()`, `resolveAppId`: none as library calls;
  `ax lint`, `ax status` and `aio::paths` cover the uses.
- `findChromium`, `freePort`: none; `TestUi` brings its own runtime and
  ports.
- Another view layer: none; views are `aio::ui` (`rsx!`).

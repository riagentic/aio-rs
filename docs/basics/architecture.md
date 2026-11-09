# Architecture

How an aio-rs app is put together, where each part of its code runs, and
the two shapes most apps take. Read [concepts](concepts.md) first: cells,
methods, effects, views and the log.

## The big picture

```text
 ┌──────────────────────────── the app's process (Rust) ─────────────────────────────┐
 │                                                                                   │
 │  views (aio::ui) ──act!──▶ cells: state + methods ◀── effects, schedules (aio::rt) │
 │     ▲        part()          │       ▲                                            │
 │     │                        │       └── doors: Handle, local door (ax, peers),   │
 │     │ holes update           ▼           /ws for synced copies (mode server)      │
 │     │               the store: state.db (snapshots + the event log, SQLite)       │
 │     │                                                                             │
 └─────┼─────────────────────────────────────────────────────────────────────────────┘
       │ DOM edits ↓  events ↑   (frames on a private socket: no port, no network)
 ┌─────┴──────────────────────┐
 │ the page: shared Electron  │   Android: the system WebView. Web build: the views
 │ runtime (desktop)          │   and cells run as wasm in the browser's page.
 └────────────────────────────┘
```

- **One process owns the state.** The cells, their store, effects,
  schedules and (for a server) the network all live in the app's Rust
  process. Nothing else writes a cell; everything else calls a method
  through a door.
- **The page only draws.** On the desktop, the view tree lives in Rust
  (air's, behind `aio::ui`), and the shared Electron runtime renders it from DOM
  edits sent over a private socket (a 0700 folder, loaded from
  `aio://app`). The page has no Node, no network and no key.
- **A synced copy is a cell too.** A window in `client` mode, a browser
  tab or another app holds a `Remote`: the server's state, its own calls
  predicted on top ([sync](../state/sync.md)).

## One call, end to end

1. A view's `act!(add(text))` (or an effect, a schedule, a synced
   client, `ax call`) asks the cell's door for `add`.
2. The method runs on a copy of the state; its `#[invariant]`s are
   checked. A refusal changes nothing and goes back to the caller.
3. `Ok`: the event (method, arguments, caller, time, the ids and random
   numbers used) and the new state commit in one SQLite transaction
   (coalesced for `#[coalesce]` methods, [persistence](../persistence/README.md)).
   `listens_to` reactions run inside that commit: their calls commit in
   the same transaction.
4. After the commit: the views' holes that read a changed field run again, synced copies
   get a patch, effects the call asked for start, listeners run.
5. An effect's result comes back as a call, so it goes through 1-4 too.

## Where each part of the code runs

| Code | Runs | It may | What surprises people |
|---|---|---|---|
| a component body (`#[component] fn`) | in the app's process (desktop, Android), or as wasm in the page (web build) | read cells (`part`, `name()`), keep local state (`signal`) | only reads made while it renders subscribe; a read in a handler doesn't |
| a handler (`onclick: move \|_\| ..`) | the same, after the render | call methods (`act!`), set signals, spawn | a refusal gives `None` and is logged; check input before calling |
| a method (`pub fn m(&mut self, ..)`) | where the cell lives; on a synced copy **also** locally, as a prediction | change `self`, refuse, ask for effects and schedules, read `cx` | it may run twice (the copy's prediction, then the server): no I/O, no clock, no randomness but `cx` ([methods](../state/cells.md)) |
| an async method (`pub async fn`) | split at each `let x: T = io.await;`: the parts are methods, each await an effect | everything a method may, plus I/O in the awaited futures | each await is cut off after `[perf] call_ceiling` (30 s by default) unless `#[long]` |
| an effect (`Effect::new(..)`) | on aio-rs's background runtime (`aio::rt()`), after the commit | any I/O; hands its result back as a call | it never runs during replay; blocking work goes in `spawn_blocking` |
| a schedule (`#[every]`, `#[cron]`, `cx.after`) | `aio::rt()`, calling a method when due | nothing itself: it is a call | it survives restarts; a missed one runs once at the next start |
| a server function, an HTTP route | the server process (`aio::fns`, `aio::http`) | any I/O; read cells; call methods | not on a copy, never queued offline ([sync](../state/sync.md#server-functions-and-routes)) |
| a listener (`h.listen`, `h.watch`, `h.on_change`) | after each commit, in the app's process | anything; it is outside the cell | it is not in the log, so replay doesn't run it |
| `main` (the launcher) | once, at start | parse flags, open sources, run the window or the server | `aio.toml` is already built in; nothing to read |

In a web build (`ax build --web`) the cells and views run in the
browser's page: no store history, files, blobs, SQL, server or anything
native. Code that must stay out of the page goes behind
`#[cfg(not(target_arch = "wasm32"))]`, which is aio-rs's form of aio's
`*.server.ts` files: the compiler, not a naming rule, keeps it out
([targets](../build/targets.md#web)).

CPU-heavy work (aio's `worker: true` cells) runs in
`tokio::task::spawn_blocking` inside an effect; its result comes back as
a call. No cell needs its own thread or isolate.

### Ask: `ax where`

`ax where <app> [item]` (aio's `am where`) answers from the app's crate
graph and the `cfg`s on the way to each item, never a guess: which
builds compile its crate (the launcher: the app's process only; the
library the web build binds, and the crates it uses: the page too), each
`cfg` from the crate's root through its modules to the item evaluated
for both builds, then what the item is (a component, a method, a
`#[client]` method) and the app's `mode`.

```text
$ ax where notes Counter::add
  item        Counter::add (src/lib.rs:17)
  runs        both: a method: it runs where the cell lives: the app's process, or the page in a web build (its cells run there)
  reached by  notes-app → notes: src/lib.rs
  cfg         none
  · it may run twice (a copy's prediction, then the server): no I/O, no clock, no randomness but cx
  · I/O goes in an effect, an awaited future, a route or a server function
  · in the page: no store history, files, blobs, SQL, server or anything native; keep that behind #[cfg(not(target_arch = "wasm32"))]
```

| Verdict | Means |
|---|---|
| `server` | a method of a served app (`mode` `server`, `client-server`, `client`) that isn't `#[client]`: only the server runs it |
| `both` | in every build, or a `#[client]` method: the server, and each synced copy as a prediction |
| `window/page` | a view (`#[component]`, a fn returning a `View`): the window, and the page in a web build |
| `window` | a view of an app with no web build |
| `wasm-only` | only the web build has it (`cfg(target_arch = "wasm32")`) |
| `native-only` | the app's process only: the launcher, or `cfg(not(target_arch = "wasm32"))` on the way to it |
| `tests-only` | `cfg(test)` |
| `never` | its cfgs rule out every build |

A `cfg` that depends on a feature or the OS is said with the item
(`cfg(feature = "pg")`). The item is a name (`add`), `Type::method`, a
module path's tail, or a file (`src/server.rs`: its items); with none,
every item, a line each. `--json` for a script.

When lost:

1. Is it a method? It runs where the cell lives, and on a copy as a
   prediction too. Keep it pure.
2. Is it inside an effect, an awaited future, a route or a server
   function? That is the place for I/O.
3. Is it in a component? It runs where the views run; it reads, and calls
   methods.
4. Is it behind `cfg(target_arch = "wasm32")`? Only the web build has it.

## The crates

| Crate (used as) | Depends on | What it holds |
|---|---|---|
| `aio-rs` (`aio`) | the macros, the config | cells, the store, the log, effects, schedules, doors, sync and its server, auth, the kit, `aio::cli`, `aio::testing` |
| `aio-rs-macros` | none of the others | `#[cell]`, `#[methods]`, `#[data]` |
| `aio-rs-config`, `aio-rs-config-macro` | none of the others | `aio.toml`'s types and checks; `aio::config!()` |
| `aio-rs-shell` (`aio_shell`) | `aio-rs` | the process around the app: `start`, `run` (the window), `serve`, Android, updates, the runtime |
| `aio-rs-cli` (`ax`) | `aio-rs`, `aio-rs-shell` | makes, runs, inspects and ships apps |

The core never depends on the shell: a test, a CLI tool or a server uses
`aio` alone. Inside `aio-rs` each capability is a Cargo feature (`kit`,
`server`, `tls`, `oidc`, `outbound`, `web`, `compat`, ...), so an app
compiles only what it uses ([project](project.md#cargo-features)). Every
dependency in the graph is listed with its reason in `deps.toml`, and CI
holds each feature set under a crate-count ceiling.

The items of `aio`, grouped (the full list: [API map](../reference/api.md)):

| Area | Items |
|---|---|
| cells | `aio::cell`, `aio::methods`, `aio::Cx`, `aio::Handle`, `aio::Cells`, `aio::machine`, `aio::lifecycle` |
| the store | `aio::Store`, `aio::db`, `aio::blobs`, `aio::backup`, `aio::paths`, `aio::merge` |
| effects and time | `aio::effect`, `aio::cron`, `aio::tell`, `aio::held`, `aio::cassette` |
| doors and sync | `aio::door`, `aio::ipc`, `aio::sync`, `aio::http`, `aio::fns`, `aio::peer`, `aio::plugin`, `aio::patch` |
| security | `aio::auth`, `aio::capabilities`, `aio::security`, `aio::tls`, `aio::pairing`, `aio::discovery` |
| views | `aio::kit`, `aio::ui`, `aio::page`, `aio::head`, `aio::route`, `aio::web` |
| health | `aio::log`, `aio::health`, `aio::vitals`, `aio::budgets`, `aio::perf`, `aio::report`, `aio::feedback` |
| tools | `aio::cli`, `aio::testing`, `aio::config`, `aio::update`, `aio::compat` |

## Extension points

| You want | Use | Page |
|---|---|---|
| a rule that always holds | `#[invariant]` on the cell | [cells](../state/cells.md) |
| who may call a method | `#[client(role = .., allow = ..)]` | [sync](../state/sync.md#who-may-call-what) |
| what each viewer sees | `#[cell(visible = f)]` | [sync](../state/sync.md#what-each-viewer-sees) |
| SQL rows in a cell | `#[table("name")]` fields | [SQL](../persistence/sql.md) |
| an older data shape | `#[cell(version = N, migrate = f)]` | [persistence](../persistence/README.md) |
| a call on one cell runs one on another | `listens_to` | [effects](../state/effects.md#reacting-outside-a-cell) |
| HTTP endpoints, typed server calls | `aio::http::Route`, `aio::fns::ServerFn` | [sync](../state/sync.md#server-functions-and-routes) |
| reusable cells, routes and hooks | `aio::plugin::Plugin` | [sync](../state/sync.md#other-apps-other-machines) |
| another app on this machine | `aio::peer::Peer` | [sync](../state/sync.md#other-apps-other-machines) |
| an aio app | `aio::compat` (feature `compat`) | [wire](../migration/wire.md) |

A rule on every call (aio's `beforeReduce`) is `aio::before_call` (or an
invariant, an access rule on the cell); an observer of every call (aio's
`onAction`) is `aio::on_action`, of one cell a listener or `h.events()`.

## The shapes an app takes

Most apps are one of two shapes. What differs is where the cells live.

| | 1. One app, many surfaces | 2. Service and rich clients |
|---|---|---|
| What it is | one app; its process owns the cells; windows, tabs and phones are views of it | two apps: a headless service, and client apps with cells of their own that call it |
| Cells live | in the one app (`mode = "client-server"` or `"server"`) | in each app: the service has its cells, each client its own |
| Clients hold | a synced copy (`Remote`) of the app's cells | their own state, plus a `Remote` or a server function call for the shared part |
| Works offline | calls wait and are sent on reconnect | fully, on the client's own cells |
| Example | a shop: one catalog and orders server, shoppers on desktop, browser and Android | a notes app with local drafts and a sync service |

Rule of thumb: a client useful only while connected is a surface (shape 1).
A client that must work on its own and only *uses* the server is a rich
client (shape 2). If unsure, start with 1: it is one app and no protocol
of your own.

### 1. One app, many surfaces

One codebase, one set of cells, one server:

```toml
# aio.toml of the app
[app]
id = "shop"
mode = "client-server"   # its own window, and served to others

[server]
listen = "0.0.0.0:8100"
tls = true
pairing = true           # ax pair hands the key to a phone
```

- The same app built with `mode = "client"` and `[client] server` is the
  thin window of another machine; `ax build --web` and `--android` are the
  other surfaces ([targets](../build/targets.md)).
- A `#[cell(shared)]` cell is the one served; only `#[client]` methods are
  callable from the network, and `visible` decides what each viewer sees.
- Two gates, not one: the shared key (pairing, `AIORT_TOKEN`) lets a
  device connect; accounts (`[auth]`) say who the user is and what role
  they have ([accounts](../auth/README.md)).

### 2. Service and rich clients

Two apps, two `aio.toml`s (each its own `[app] id`, so its own data, lock
and log), one repository. The service is `mode = "server"`; the client is
a full app whose own cells hold its own data, and which calls the
service for the shared part. Both halves in one process, as a test runs
them:

```rust
use aio::prelude::*;
use aio::sync::{Remote, serve};
use std::time::Duration;

/// The service's cell: what every client shares.
#[cell(persist, shared)]
pub struct Inbox {
    pub notes: Vec<String>,
}

#[methods]
impl Inbox {
    #[client]
    pub fn post(&mut self, text: String) -> Result {
        ensure!(!text.trim().is_empty(), "empty note");
        self.notes.push(text);
        Ok(())
    }
}

/// The client's own cell: its drafts, which never leave it.
#[cell(persist)]
pub struct Drafts {
    pub open: Vec<String>,
}

#[methods]
impl Drafts {
    pub fn write(&mut self, text: String) {
        self.open.push(text);
    }

    pub fn sent(&mut self, i: usize) -> Result {
        ensure!(i < self.open.len(), "no draft {i}");
        self.open.remove(i);
        Ok(())
    }
}

async fn until(f: impl Fn() -> bool) {
    for _ in 0..300 {
        if f() {
            return;
        }
        aio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out");
}

# fn main() {
let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
rt.block_on(async {
    // The service: its cell, served on /ws behind a key.
    let inbox = Handle::<Inbox>::memory();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(serve(inbox.clone(), l, "secret".into()));

    // The client: its own cell, and a synced copy of the service's.
    let drafts = Handle::<Drafts>::memory();
    let shared = Remote::<Inbox>::connect(&addr, "secret", "ana").await.unwrap();

    drafts.write("buy tea".into()).unwrap(); // local: works offline
    let text = drafts.state().open[0].clone();
    shared.post(text).unwrap(); // predicted at once, confirmed by the service
    until(|| shared.pending() == 0).await;
    drafts.sent(0).unwrap();

    assert_eq!(inbox.state().notes, ["buy tea"]);
    assert!(drafts.state().open.is_empty());
});
# }
```

In the real apps none of the wiring is by hand: the service's launcher
is `Source::for_server` and `aio_shell::serve` (the `service` template),
and the client opens its own cells with `Source::for_app` and the service
with `Remote::connect_saved(link, user, file)`, which keeps the copy and
its waiting calls on disk ([sync](../state/sync.md#offline-and-reconnects)).

- Shared types (the cell structs both sides name) go in a crate both
  apps depend on; the service's cell type is just a Rust type, so
  naming it pulls in no server code.
- A call that must answer with a value no cell keeps (a search) is a
  server function (`remote.run(F, args)`), not a method.
- Payload limits are the service's `[server.limits]`: raise
  `max_message` there, never split messages in secret.
- Testing the pair: as above, or `aio::testing::TestServer` for the
  service with real clients ([testing](../testing/README.md)).

### Many cells, one process

A process may open many cells and many stores: each `Handle` is its own
cell on its own store (`Handle::open(path)`), so a test or an embedding
runs several apps side by side without a global registry. What is per
process: the background runtime (`aio::rt()`) and the app's home claimed
by `aio_shell::start` (one running copy per home; a second launch hands
over to the first).

## Gaps against aio

- No module-boundary gate inside `aio-rs`: the crate split and Cargo
  features are the boundaries.

## API

`cargo doc -p aio-rs -p aio-rs-shell --no-deps --all-features --open`:
`aio::Handle`, `aio::Source`, `aio::sync` (`serve`, `Remote`), `aio::rt`,
`aio_shell::start`, `aio_shell::run`, `aio_shell::serve`.

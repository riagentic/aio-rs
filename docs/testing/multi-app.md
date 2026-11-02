# Servers, sync and many apps

A `TestCells` test calls the cell in-process, as the app's own code
does. What a network client meets is more: the key, the `#[client]`
rules, the wire encoding, sign-in, the app's HTTP routes, a link that
drops and comes back. `aio::testing::TestServer` and `aio::testing::Sim`
(feature `server`) test that, with nothing faked: the server, the wire
and the synced copies are the app's own code, on loopback.

| You test | Use |
|---|---|
| HTTP routes, files, `/__aio/health`, `/__aio/metrics` | `TestServer::fetch` / `fetch_with` |
| sign-up, sign-in, cookies, sign-out | `TestServer::page()` |
| copies of a cell that stay in step | `TestServer::client(user)` + `converged` |
| a view against the real wire | [`TestServer::ui_source`](ui.md#a-served-cell-under-the-page) |
| copies under cuts, heals and lag | `Sim` |
| a service and an app that is its client | two `TestServer`s, each with its own dirs |

Both need a tokio runtime: `#[tokio::test(flavor = "multi_thread")]` (or
build one, as the examples do). Each `TestServer` holds a sandbox, so no
test reaches the person's data.

## `TestServer`: the app's server on loopback

```rust
use aio::http::{Http, Reply, Route};
use aio::prelude::*;
use aio::testing::TestServer;

#[cell(shared)]
pub struct Shop {
    pub sold: u32,
}

#[methods]
impl Shop {
    #[client]
    pub fn sell(&mut self) {
        self.sold += 1;
    }

    pub fn restock(&mut self) {}
}

# fn main() {
let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
rt.block_on(async {
    let h = Handle::<Shop>::memory();
    let shop = h.clone();
    let http = Http::default().route(Route::post("/buy", move |_| {
        let h = shop.clone();
        async move { Reply::text(if h.sell().is_ok() { "sold" } else { "no" }) }
    }));
    let s = TestServer::start(h, http).await.unwrap();
    assert!(s.url().starts_with("ws://127.0.0.1:") && !s.token.is_empty());

    // As the app's own page sends it: with the key.
    let f = s.fetch("POST", "/buy", "").await.unwrap();
    assert_eq!((f.status, f.body.as_str()), (200, "sold"));
    assert!(f.header("content-type").unwrap().starts_with("text/plain"));
    assert_eq!(s.state().sold, 1);

    // A stranger: exactly the headers given, here no key.
    let json = [("Content-Type", "application/json")];
    assert_eq!(s.fetch_with("POST", "/buy", &json, "").await.unwrap().status, 401);
    assert_eq!(s.state().sold, 1, "refused before it ran");

    // The monitor routes every app serves.
    let health = s.fetch("GET", "/__aio/health", "").await.unwrap();
    assert!(health.body.contains(r#""id":"shop""#), "{}", health.body);
    assert_eq!(s.fetch("GET", "/__aio/nope", "").await.unwrap().status, 404);
});
# }
```

| Item | Does |
|---|---|
| `TestServer::start(handle, http)` | serves `handle` as the app does (sync, `http`'s routes and files) on a free loopback port with a fresh key; `Http::default()` for none. A cell without `shared` gets its routes alone, no sync |
| `TestServer::start_with(handle, opts)` | the same with `aio::sync::Options`: accounts (`auth`), `limits`, `security`, `open`, `hosts`, … |
| `s.addr`, `s.url()`, `s.token`, `s.handle` | `127.0.0.1:<port>`, `ws://…`, the key, the served handle |
| `s.state()` | the served state now |
| `s.fetch(method, path, body)` | a request with the key (`Authorization: Bearer`), the body as JSON unless GET or HEAD |
| `s.fetch_with(method, path, headers, body)` | a request with exactly these headers (`&[]`: a stranger) |
| `s.page()` | a browser page: its requests carry the cookies the server set it |
| `s.client(user)` | a real synced copy (`aio::sync::Remote`) as `user` |
| `s.converged(&[&a, &b])` | waits until every copy is online, has nothing waiting and shows the server's state (5 s) |
| `s.ui_source(user, latency)` | a copy for a `TestUi` view, with latency each way |

A `Fetched` answer has `status`, `headers` (names lowercase), `body`, and
`header(name)` (any case). Dropped, the server stops.

Assert the body as well as the status: a 200 says the server answered,
the body says what it decided.

## Sign-in, cookies and routes

A `TestPage` is one browser: the cookies the server sets on it are kept
and sent back, to this server only. A new `page()` is a new browser.

```rust
use aio::prelude::*;
use aio::testing::TestServer;
use std::sync::Arc;

#[cell(shared)]
pub struct Notes {
    pub list: Vec<String>,
}

#[methods]
impl Notes {}

# fn main() {
let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
rt.block_on(async {
    let auth = aio::auth::Auth::memory(aio::auth::Policy { signup: true, ..Default::default() }).unwrap();
    let opts = aio::sync::Options { auth: Some(Arc::new(auth)), ..Default::default() };
    let s = TestServer::start_with(Handle::<Notes>::memory(), opts).await.unwrap();
    let json = [("Content-Type", "application/json")];

    let mut page = s.page();
    let made = page.fetch_with("POST", "/__aio/auth/signup", &json, r#"{"id":"ada","password":"password123"}"#);
    assert_eq!(made.await.unwrap().status, 201);
    assert_eq!(page.cookies.len(), 1, "the session cookie is kept");
    let me = page.fetch_with("GET", "/__aio/auth/me", &[], "").await.unwrap();
    assert!(me.body.contains(r#""id":"ada""#), "{}", me.body);

    let other = s.page().fetch_with("GET", "/__aio/auth/me", &[], "").await.unwrap();
    assert!(other.body.contains(r#""user":null"#), "another browser is nobody");

    assert_eq!(page.fetch_with("POST", "/__aio/auth/logout", &json, "").await.unwrap().status, 200);
    assert!(page.cookies.is_empty(), "signing out clears it");
});
# }
```

- `aio::auth::Auth::memory(policy)` is an account store with no file;
  `Policy` sets sign-up, password rules, second factors
  ([Auth](../auth/README.md)). Sign a second factor in with
  [`totp_code`](cells.md#logging-in-with-a-second-factor).
- `page.fetch` sends the key as well; `page.fetch_with` sends only the
  headers given, plus the page's cookies (unless the headers carry a
  `Cookie` of their own).
- For a `#[client]` rule alone, without the wire, use
  [`TestCells::as_client`](cells.md#access-rules-as-a-network-caller).

## Synced copies

`s.client(user)` connects a real `Remote`: the copy a window or another
app holds. Its calls are predicted at once and confirmed by the server;
the server's changes reach every copy.

```rust
use aio::prelude::*;
use aio::testing::TestServer;

#[cell(shared)]
pub struct Tally {
    pub n: i64,
}

#[methods]
impl Tally {
    #[client]
    pub fn add(&mut self, by: i64) {
        self.n += by;
    }

    pub fn reset(&mut self) {
        self.n = 0;
    }
}

# fn main() {
let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
rt.block_on(async {
    let s = TestServer::start(Handle::<Tally>::memory(), Default::default()).await.unwrap();
    let (ana, bo) = (s.client("ana").await.unwrap(), s.client("bo").await.unwrap());
    for _ in 0..5 {
        ana.add(1).unwrap(); // each on its own copy, at once
        bo.add(2).unwrap();
    }
    s.converged(&[&ana, &bo]).await;
    assert_eq!((s.state().n, ana.state().n, bo.state().n), (15, 15, 15), "no lost update");

    s.handle.reset().unwrap(); // the server's own code
    s.converged(&[&ana, &bo]).await;
    assert_eq!(bo.state().n, 0, "a change on the server reaches every copy");

    // Not a #[client] method: predicted, then refused by the server and undone.
    let refused = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    let seen = refused.clone();
    ana.on_refused(move |m, e| seen.lock().unwrap().push(format!("{m}: {}", e.why)));
    _ = ana.reset();
    s.converged(&[&ana, &bo]).await;
    assert_eq!(refused.lock().unwrap().len(), 1, "{:?}", refused.lock().unwrap());
});
# }
```

- `converged` waits for the work, not for equality: right after a call
  every copy may still agree with the server because the call hasn't
  arrived. It needs each copy online, with no call waiting, showing the
  server's state. After 5 s it fails naming each copy that lags, its link
  and its state.
- `Remote::online()`, `pending()`, `pending_of(method)`, `status()` and
  `on_refused(f)` are what a view reads for its "offline" and "saving…"
  states ([Sync](../state/sync.md)).
- A copy shows what the server lets that user see: a per-user view
  ([Visibility](../state/visibility.md)) is tested by asserting on each
  client's `state()`.
- `aio::testing::patches(&copy)` records what the copy receives from then
  on (aio's `patches`): each `{"t":"patch","seq","prev","ops"}` delta and
  each whole `{"t":"state"}` (a resync). `all()` is the list so far;
  `until(|p| ..).await` waits for one (5 s, then fails naming those that
  came). So "the field was left out of the delta" and "nothing was resent
  whole" are assertable. `Remote::on_patch(f)` is the hook under it.
- `aio::testing::call_all(&[&ana, &bo], "add", json!([1]))` makes the same
  call from every copy at once (aio's `dispatchAll`), each predicted and
  sent; then `converged`.

## `Sim`: copies under cuts, heals and lag

`Sim` is a seeded script against real copies over lines that hold every
chunk up to 8 ms, and that the script cuts (every connection closed, new
ones refused) and heals:

```rust
use aio::prelude::*;
use aio::testing::{RandomCall, Rng, Sim};
use serde_json::{Value, json};

#[cell(shared)]
pub struct Ledger {
    pub ids: Vec<u64>,
}

#[methods]
impl Ledger {
    #[client]
    pub fn add(&mut self, id: u64) {
        self.ids.push(id);
    }

    #[client]
    pub fn undo_last(&mut self) -> Result {
        ensure!(self.ids.pop().is_some(), "nothing to undo");
        Ok(())
    }
}

fn add(r: &mut Rng) -> Value {
    json!([r.number()])
}

fn none(_: &mut Rng) -> Value {
    Value::Null
}

/// Each call counted once, however often its line was cut.
fn once(l: &Ledger) -> bool {
    let mut ids = l.ids.clone();
    ids.sort_unstable();
    ids.windows(2).all(|w| w[0] != w[1])
}

# fn main() {
let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
rt.block_on(async {
    let calls: [RandomCall; 2] = [("add", &add), ("undo_last", &none)];
    let mut sim = Sim::start(Handle::<Ledger>::memory(), 3, 7).await.unwrap();
    let script = sim.run(120, &calls, once).await;
    assert!(script.iter().any(|s| s.starts_with("cut c")), "{script:?}");
    for c in sim.clients() {
        assert_eq!(*c.state(), *sim.server.state(), "every copy settled on the server's state");
    }
});
# }
```

- `Sim::start(handle, clients, seed)` serves `handle` and links
  `clients` copies (as users `c0`, `c1`, …), each over its own line.
- `run(steps, &calls, holds)`: each step is, by the seed, a client's call
  (predicted at once, sent when its line is up), a call by the server's
  own code, a cut, a heal, or a short wait. `holds` is checked on the
  server's state after every step. At the end every line heals and each
  copy must settle (online, nothing waiting, the server's state) within
  30 s, and `holds` is checked again.
- A failure panics with the seed and the script up to it:
  `sim seed 5: the server's state no longer holds … script: c0.add([…]),
  cut c1, …`. The same seed is the same script whatever the timing, so
  that run can be repeated.
- `run` returns the script; `sim.server` is the `TestServer` and
  `sim.clients()` the copies, for more assertions.
- The calls are `RandomCall`s, as for [`TestCells::fuzz`](cells.md#fuzzing-seeded-random-calls).

## A service and its client app

An app may be the client of another app's service: its own cells, store,
port and key, and a `Remote` of the service. Start both in one test, each
in its own dirs:

```rust
use aio::http::Http;
use aio::paths::AppDirs;
use aio::prelude::*;
use aio::testing::TestServer;
use std::sync::OnceLock;

/// The service's cell.
#[cell(shared, persist)]
pub struct Ledger {
    pub balance: u64,
}

#[methods]
impl Ledger {
    #[client]
    pub fn credit(&mut self, n: u64) {
        self.balance += n;
    }
}

/// The desk app's client of the service.
static LEDGER: OnceLock<aio::sync::Remote<Ledger>> = OnceLock::new();

/// The desk app's own cell: it pays through its client of the service.
#[cell(shared, persist)]
pub struct Desk {
    pub bought: u32,
}

#[methods]
impl Desk {
    pub fn buy(&mut self, cx: &Cx, n: u64) {
        self.bought += 1;
        cx.run(Effect::new("pay", move |_: Handle<Desk>| async move { _ = LEDGER.get().map(|l| l.credit(n)) }));
    }
}

# fn main() {
let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
rt.block_on(async {
    let world = aio::paths::sandbox().unwrap();
    let dirs = |id: &str| AppDirs::at(&world.root().join(id), id).unwrap();
    let (sd, dd) = (dirs("service"), dirs("desk"));
    assert!(AppDirs::at(&world.root().join("desk"), "desk").is_err(), "each app holds its own lock");

    let service = TestServer::start(Handle::<Ledger>::open(&sd.store).unwrap(), Http::default()).await.unwrap();
    let desk = TestServer::start(Handle::<Desk>::open(&dd.store).unwrap(), Http::default()).await.unwrap();
    assert!(service.addr != desk.addr && service.token != desk.token, "own port, own key");

    LEDGER.set(service.client("desk").await.unwrap()).ok().unwrap();
    desk.handle.buy(5).unwrap();
    desk.handle.done("pay").await;
    service.converged(&[LEDGER.get().unwrap()]).await;
    assert_eq!((service.state().balance, desk.state().bought), (5, 1), "the call landed on the service");

    // Closing the desk leaves the service running; each kept its own file.
    drop(desk);
    let late = service.client("bo").await.unwrap();
    service.converged(&[&late]).await;
    assert_eq!(late.state().balance, 5);
    assert!(sd.store.is_file() && dd.store.is_file());
});
# }
```

- `aio::paths::sandbox()` is the test world the harnesses use; its
  `root()` is a temp folder, removed when the last one is dropped.
- `AppDirs::at(root, id)` gives one app its `data`, `logs` and `cache`
  under `root` and takes its lock, as `AIORT_DATA=<root>` does for a
  running app: a second app with the same dirs is refused.
- `Handle::open(&dirs.store)` opens that app's store; a persisted cell is
  a file of its own app, never the other's.
- `TestApps` is the same in one harness (aio's `testApps`):
  `world.boot::<Ledger>("service").await?` opens the cell from its own
  dirs and serves it; `world.get::<Ledger>("service")` is its
  `TestServer`, `world.dirs("service").logs` its logs folder,
  `world.connect::<Ledger>("service", "desk").await?` a client of it. A
  name not booted fails naming the booted ones; dropped, the apps stop,
  the last booted first.
- Logs, budgets and health are per process in aio-rs: two apps in one
  test binary share the process's log and report hooks. Test what must
  stay apart (logs, crash reports) with each app as its own process
  (`ax dev <app> --instance <n>`).

## How close to production a test runs

| What the app does | `Handle` / `TestCells` | `TestServer`, `Sim`, `ui_source` |
|---|---|---|
| methods, invariants, effects, the log | the app's own code | the app's own code |
| `#[client]` rules and roles | only with `as_client` | asked of every call, as the server does |
| the key, sessions, cookies | none | real: a stranger gets 401 |
| the wire encoding of arguments and answers | none (Rust values in-process) | real: JSON over a WebSocket |
| a call predicted, then confirmed or undone | none | real (`Remote`) |
| a link that drops, backoff, resync | none | `Sim` cuts and heals real sockets |
| time | virtual (`advance`) | real |
| HTTP routes, files, health, metrics | none | real, on loopback |
| TLS | none | none: `TestServer` serves plain `ws://` on loopback |
| a second process, a crash, an OS limit | none | none: start the app as its own process |

The boot rules are the same in every harness: `TestCells::start` refuses
what the app's boot refuses (a duplicate id, an unknown dependency, a
cycle), and `saved` refuses data the app's boot would refuse
([Testing cells](cells.md#several-cells-and-the-apps-boot)). A cell's
methods run as Rust: there is no worker boundary to reproduce, and an
argument a method takes is a type the wire must encode, so a value that
can't cross doesn't compile.

## What aio has that aio-rs does without

| aio | aio-rs |
|---|---|
| `testServer(config)` | `TestServer::start(handle, http)` / `start_with(handle, opts)` |
| `testMultiClient(config, n)` | `TestServer::client(user)` per copy, `converged` |
| `clients[i].patches`, `onPatch`, `waitForPatch` | `aio::testing::patches(&copy)`: `all()`, `until(pred)`; `Remote::on_patch` |
| `m.dispatchAll(action)` | `aio::testing::call_all(&[&a, &b], method, args)` |
| `testApps(specs)`: N apps, each its own dirs, logs, port | `TestApps`: `boot::<C>(name)`, `get`, `dirs(name).logs`, `connect`; problem reports are per process |
| `testServer(config)`: a whole app, every cell | one cell per `TestServer` until one socket carries every cell (the sync work, SY3) |
| `testServer({ workers: "real" })` | none needed: no worker cells |
| boot refusal of a credential field visible to the UI | none at boot: [`ax lint`](lint.md) flags it |
| `freePort()` | none needed: `TestServer` binds port 0 |

## Common mistakes

- A `#[tokio::test]` on the default current-thread runtime with
  `converged` or `Sim`: use `flavor = "multi_thread"`.
- Asserting right after a call that the copies agree: they agree because
  nothing arrived yet. Use `converged`.
- Checking only `status == 200`: assert the body too.
- A `Sim` that fails now and then with no seed in the message: it
  always names its seed; run that seed again rather than raising a
  timeout.
- Testing a per-user view on the server's state: read each client's
  `state()`.
- Two apps in a test on the same `AppDirs`: each needs its own, as on a
  machine.

## API

`cargo doc -p aio-rs --no-deps --all-features --open`:
`aio::testing::TestServer`, `TestPage`, `Fetched`, `Sim`, `RandomCall`,
`Rng`; `aio::sync` (`Remote`, `Options`, `Limits`), `aio::http` (`Http`,
`Route`, `Reply`), `aio::auth` (`Auth`, `Policy`), `aio::paths`
(`sandbox`, `AppDirs`).

# Sync, clients and access

A `#[cell(shared)]` cell is served to clients: each gets what its user may
see of the state, then patches after every change. The server is the
truth. A client predicts its own calls (shows them at once) and the server
confirms them; a refusal rolls the prediction back. Calls made offline
wait and are sent on reconnect, each once.

Use sync when the same data is used in more than one place: a phone and
a desktop, several people on one board, a headless server and its
windows. A standalone app (the default) needs none of it.

Feature `net` (or `server`) on aio-rs; `[app] mode = "client-server"`,
`"server"` or `"client"` in `aio.toml`.

| `[app] mode` | Cells live | Window | Listens |
|---|---|---|---|
| `standalone` (default) | in the app | yes | no |
| `client-server` | in the app | yes | `[server] listen` |
| `server` | in the app | no (headless) | `[server] listen` |
| `client` | on `[client] server` | yes | no |
| `cli` | in the command | no | no ([command-line apps](../clients/cli-apps.md)) |

## A server and two copies

```rust
use aio::prelude::*;
use aio::sync::{Remote, serve};
use std::collections::BTreeMap;
use std::time::Duration;

#[cell(persist, shared)]
pub struct Board {
    pub notes: BTreeMap<Uuid, String>,
}

#[methods]
impl Board {
    /// `#[client]`: the network may call it.
    #[client]
    pub fn add(&mut self, cx: &Cx, text: String) -> Result<Uuid> {
        ensure!(!text.is_empty(), "empty note");
        let id = cx.id();
        self.notes.insert(id, text);
        Ok(id)
    }

    /// Not `#[client]`: only the server's own code may call it.
    pub fn clear(&mut self) {
        self.notes.clear();
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
    let h = Handle::<Board>::memory();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(serve(h.clone(), l, "secret".into()));

    let ana = Remote::<Board>::connect(&addr, "secret", "ana").await.unwrap();
    let bo = Remote::<Board>::connect(&addr, "secret", "bo").await.unwrap();
    let id = ana.add("hello".into()).unwrap(); // predicted: shown at once
    assert_eq!(ana.state().notes[&id], "hello");
    until(|| ana.pending() == 0).await; // confirmed by the server
    until(|| *bo.state() == *h.state()).await; // and on every copy
    _ = ana.clear(); // not #[client]: the server refuses it, the copy rolls back
    until(|| ana.pending() == 0).await;
    assert_eq!(ana.state().notes.len(), 1);
});
# }
```

In an app none of this is written by hand: `aio::Source::for_app(&cfg,
&dirs)` opens the cell the way `[app] mode` says (local, served, or a
synced copy of `[client] server`), and `aio_shell::serve(&dirs)` serves a
headless one. The wire is a WebSocket at `/ws` (aio's path), one JSON
message per frame, the key in `Authorization: Bearer`:

```toml
[app]
id = "board"
mode = "client-server"

[server]
listen = "0.0.0.0:8100"
tls = true          # this machine's own root; `ax trust` says how browsers trust it
pairing = true      # `ax pair` hands the key to a device
discover = true     # answers `ax discover` on the LAN
```

A window in `client` mode names its server in `[client] server`, or the
person picks one on its connect page; either is reached with no
`[capabilities] network` entry.

## What crosses the wire

| Thing | Crosses? |
|---|---|
| a call's method name and arguments | yes, as JSON (`#[data]` types), with the ids and random numbers the copy's prediction used |
| a method's return value | the copy's call returns its own prediction at once; the server's answer comes back in the call's reply (`call_confirmed`) |
| the state | the viewer's view of it (`visible`, no `#[server]` fields): in full on connect, then patches of what changed |
| a refusal | `aio::Rejected { why, code }`: the method's own reason, or a code (below) |
| effects, schedules | no: a predicted call runs none on the copy; the server runs the call and its effects |
| tells (`cx.tell`) | yes: the server sends them to every client of the cell |
| server functions | their arguments, parts and answer, on the same link ([server functions](#server-functions-and-routes)) |

The copy runs the same method on the same state the server will (as far
as it knows), so its prediction is the server's answer in the common
case. When they differ, the server's state wins: its patch replaces the
copy's view and the prediction is dropped.

## Calling from a copy

```rust
use aio::prelude::*;
use aio::sync::{Remote, serve};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cell(persist, shared)]
pub struct Tally {
    pub n: u32,
}

#[methods]
impl Tally {
    #[client]
    pub fn add(&mut self, by: u32) -> Result<u32> {
        ensure!(by > 0, "add something");
        self.n += by;
        Ok(self.n)
    }

    pub fn reset(&mut self) {
        self.n = 0;
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
    let h = Handle::<Tally>::memory();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(serve(h.clone(), l, "secret".into()));

    let ana = Remote::<Tally>::connect(&addr, "secret", "ana").await.unwrap();
    let refused = Arc::new(Mutex::new(vec![]));
    let r = refused.clone();
    ana.on_refused(move |method, why| r.lock().unwrap().push((method.to_string(), why.code())));

    assert_eq!(ana.add(2).unwrap(), 2, "the copy's answer, at once");
    assert!(ana.add(0).is_err(), "refused on the copy: never sent");

    let a = ana.clone(); // call_confirmed blocks: off the async threads
    let answer = tokio::task::spawn_blocking(move || a.call_confirmed("add", serde_json::json!([3])));
    assert_eq!(answer.await.unwrap().unwrap(), serde_json::json!(5), "the server's answer");

    ana.reset().unwrap(); // predicted; the server refuses it: not #[client]
    until(|| ana.pending() == 0).await;
    assert_eq!(*refused.lock().unwrap(), [("reset".to_string(), Some(Code::AccessDenied))]);
    assert_eq!(ana.state().n, 5, "the copy rolled back");
});
# }
```

- A typed call on a copy (`ana.add(2)`) runs the method on the copy and
  returns what it returned there. A refusal there is returned at once and
  nothing is sent. Otherwise the call waits in `pending()` until the
  server's reply; a refusal then reaches `on_refused(|method, why| ..)`.
- `remote.call_confirmed(method, args)` waits for the server's answer
  (15 s, `aio::sync::CALL_TIMEOUT`, unless `[client] timeouts` says
  otherwise) and returns it as JSON. A link that drops meanwhile doesn't
  fail it: the call is sent again on reconnect and applies once. With no
  answer in time it fails with `no answer in …` and stays queued. It
  blocks its thread: call it from `spawn_blocking` or a plain thread.
- A `#[client(predict = false)]` method returns nothing, by rule: the
  copy can't know its answer, so it only sends it, and it shows once the
  server ran it (`pending_of(method)` counts it until then).

| `Rejected::code()` | Means |
|---|---|
| `None` | the method's own refusal (`ensure!`, `reject!`) |
| `Some(Code::AccessDenied)` | not `#[client]`, or its `role` or `allow` rule said no |
| `Some(Code::ActionRefused)` | no such method |
| `Some(Code::ConnectionLost)` | the link dropped or never came up: it may have run (server functions) |

## Offline and reconnects

A copy's link, from start to end:

1. `Remote::connect(addr, key, user)` (or `connect_to(Link)`, over TLS
   when the link has a root to pin) opens the WebSocket, shows the key
   and waits up to 15 s for the first state. A wrong key fails at once.
2. Then it stays linked until every clone of the `Remote` is dropped. It
   pings every 2 s (`aio::sync::PING`; `rtt()` is the last round trip) and
   drops a link it has heard nothing on for 5 s (`DEAD`).
3. A dropped link is opened again with backoff: 250 ms, doubling up to
   10 s, plus up to half a second of jitter so many clients don't return
   at once. A refused key is not retried. A server of another wire
   version is said once in the log and tried again (it may be updated).
4. On each reconnect, and whenever a patch doesn't follow the last one,
   the copy takes the whole view again; `on_sync(|s: &Synced| ..)` hears
   each catch-up (events it brought, how long it took).

While offline:

- Calls still apply to the copy at once and wait to be sent:
  `remote.state()` is the confirmed state (`confirmed()`) plus the calls
  still waiting (`pending()`, `pending_of(method)`). Each has an id, so a
  call sent twice applies once.
- At most `aio::sync::PENDING_MAX` (500) calls wait. A full queue refuses
  new calls, unless `[client] retention = "7d"` lets a new call drop the
  oldest one that has waited longer than that (told to `on_refused`, with
  what is known: the server may or may not have it).
- `Remote::connect_saved(link, user, file)` keeps the copy and its
  waiting calls in `file` (0600): with the server unreachable the app
  starts from it (offline-first), and waiting calls survive a restart.
  Without it the queue lives in memory and a restart loses it.
- A server function fails at once (`ConnectionLost`): it is never queued.

`remote.status()` (`aio::Status`) is what an "offline" badge reads:

| Field | |
|---|---|
| `conn` | `Conn::Online` (linked, nothing waiting), `Offline`, `Syncing` (linked, calls on their way), `Blocked` (the queue is full) |
| `online`, `pending` | linked now; calls waiting |
| `degraded` | more than 80% of `PENDING_MAX` waits |
| `last_sync` | when the server's state last reached the copy (`None`: not in this run) |
| `rtt` | the last ping's round trip |

A server drops a client that takes more than 10 s to take one message
(`SEND_TIMEOUT`); that client reconnects and catches up. A call that
arrives while the server shuts down is not run: the client keeps it and
sends it to the next run. [Offline and merging](../persistence/offline.md)
has what happens when several copies edit the same data while apart.

## Who may call what

```rust
use aio::prelude::*;
use aio::testing::TestCells;
use std::collections::BTreeMap;

#[cell(persist, shared)]
pub struct Docs {
    pub owners: BTreeMap<String, String>, // doc -> its owner
    pub edited: Vec<String>,
}

/// Row-level: only a doc's owner may edit it, as the state says now.
fn own_doc(d: &Docs, who: &Caller, doc: &str) -> bool {
    matches!(&who.by, By::User(u) if d.owners.get(doc) == Some(u))
}

#[methods]
impl Docs {
    #[client(allow = own_doc)]
    pub fn edit(&mut self, doc: &str) {
        self.edited.push(doc.into());
    }

    #[client(role = "admin")]
    pub fn give(&mut self, doc: String, to: String) {
        self.owners.insert(doc, to);
    }
}

# fn main() {
let mut t = TestCells::new();
let h = t.cell::<Docs>();
let admin = t.as_client(&h, Caller { by: By::User("root".into()), role: Some("admin".into()) });
let ana = t.as_client(&h, Caller { by: By::User("ana".into()), role: None });
admin.give("plan".into(), "ana".into()).unwrap();
ana.edit("plan").unwrap();
assert!(ana.give("plan".into(), "bo".into()).is_err(), "not an admin");
assert!(ana.edit("budget").is_err(), "not her doc");
# }
```

- Only `#[client]` methods are callable from the network. `role = "x"`
  needs a signed-in session with that role now; `allow = f`
  (`fn(&Self, &Caller, <the method's arguments>) -> bool`) decides on the
  state the call would run on. Both: both must pass.
- `predict = false`: a copy never shows the call before the server ran it
  (payments, ledgers).
- A call's ids (`cx.id()`) are derived from the call's own id on a synced
  server, so a client can't choose them to overwrite another's row.
- `ax peer <app> [--as user] [--role r]` lists what a caller may call,
  with each method's signature and its rule.
- The app's own code, effects, schedules and `ax call` in a debug build
  call any method; a release build's local door gets what a client gets.

## What each viewer sees

```rust
use aio::prelude::*;

#[cell(shared, visible = by_role)]
pub struct Payroll {
    pub names: Vec<String>,
    pub salaries: Vec<u32>,
    #[server]
    pub bank_token: Hidden<String>, // saved, never sent
}

fn by_role(p: &Payroll, who: &Caller) -> Payroll {
    match who.role.as_deref() {
        Some("admin") => p.clone(),
        _ => Payroll { salaries: vec![], ..p.clone() },
    }
}

#[methods]
impl Payroll {}

# fn main() {
use aio::CellData;
let p = Payroll { names: vec!["Ada".into()], salaries: vec![10], bank_token: Hidden::new("t".into()) };
let staff = p.project(&Caller { by: By::User("bo".into()), role: None });
assert_eq!(staff["salaries"], serde_json::json!([]));
assert!(staff.get("bank_token").is_none(), "#[server] fields never leave");
# }
```

`visible` runs again on every change and for every viewer, with their
role now, and a `#[server]` field is never sent; clients get patches of
their own view only, so what a viewer can't see costs them no bytes
(`ax cost <app>`). [Visibility](visibility.md) has the rules, per-user
views and the contexts a method runs in.

## Server functions and routes

A server function is typed async code that runs only on the server and
answers with a value no cell keeps (a search, a payment): declared once
as `aio::fns::ServerFn<(Args,), Answer>`, served with an access rule and
called by `remote.run(F, args).await`. The app's own HTTP routes sit on
the same server, behind the same key unless `Route::public`.

```rust
use aio::fns::{Access, ServerFn};
use aio::http::{Http, Reply, Route};
use aio::prelude::*;
use aio::testing::TestServer;

#[cell(persist, shared)]
pub struct Shop {
    pub orders: u32,
}

#[methods]
impl Shop {
    #[client]
    pub fn order(&mut self) {
        self.orders += 1;
    }
}

/// One declaration types both ends: (item, quantity) -> cents.
const QUOTE: ServerFn<(String, u32), u64> = ServerFn::new("shop.quote");

fn http(h: Handle<Shop>) -> Http {
    Http::default()
        .server_fn(QUOTE, Access::Open, |_req, (item, qty)| async move {
            ensure!(qty > 0, "how many?");
            Ok(item.len() as u64 * 100 * u64::from(qty))
        })
        .route(Route::get("/orders/:shop", move |req| {
            let n = h.state().orders;
            let shop = req.param("shop").unwrap_or_default().to_string();
            async move { Reply::json(&serde_json::json!({ "shop": shop, "orders": n })) }
        }))
}

# fn main() {
let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
rt.block_on(async {
    let h = Handle::<Shop>::memory();
    h.order().unwrap();
    let server = TestServer::start(h.clone(), http(h.clone())).await.unwrap();

    let ana = server.client("ana").await.unwrap();
    assert_eq!(ana.run(QUOTE, ("tea".into(), 2)).await, Ok(600));
    assert!(ana.run(QUOTE, ("tea".into(), 0)).await.unwrap_err().why.contains("how many?"));

    let got = server.fetch("GET", "/orders/main", "").await.unwrap();
    let body: serde_json::Value = serde_json::from_str(&got.body).unwrap();
    assert_eq!((got.status, body["shop"].as_str(), body["orders"].as_u64()), (200, Some("main"), Some(1)));
    let stranger = server.fetch_with("GET", "/orders/main", &[], "").await.unwrap();
    assert_eq!(stranger.status, 401, "routes need the key");
});
# }
```

- **Access** fails closed: `Access::Open` (anyone the app lets connect),
  `Signed` (a signed-in user), `Role("billing")`, or `Check(rule)` on the
  caller and the arguments. Without accounts the shared key proves no
  one, so only `Open` lets a call in. The function gets an
  `aio::fns::Req`: who (`Caller`), from where (`ip`), the link's headers.
- A function that fails, panics or answers what JSON can't carry fails
  its call, in words; the server goes on. A client waits
  `ServerFn::timeout` (30 s by default) and fails at once offline: a
  server function is never queued.
- `Http::server_fn_streamed(F, access, |req, args, parts| ..)` sends parts
  as it goes (`parts.send(&x).await`, an LLM's tokens), received with
  `remote.run_streamed`.
- **Routes**: `Route::get("/users/:id", f)`, `Route::post`, `Route::new(&
  ["PUT"], ..)`; `aio::http::Req` has `param`, `query`, `cookie`,
  `header`, `json`, `user`, `role`, `ip`; `Reply::text | json | redirect`
  with `.status`, `.header`, `.cookie`. `.max_body(bytes)`,
  `.streamed()`, `.public()` (a webhook with its own signature check).
  The app's routes never take `/ws` or `/__aio…`.
- **A streamed answer**: `Reply::stream(content_type, parts)` sends each
  item of a `Stream` as it comes (server-sent events, NDJSON, a long
  download); never compressed, cached or held by a proxy, and dropped
  when the client leaves.
- **A binary side channel**: `Route::socket("/stream", f)` upgrades to a
  WebSocket once `f` accepts the request ([transports](../clients/transports.md#binary-data)).
- **More cells on one server, one socket**: `Http::cell(handle)` serves
  another `#[cell(shared)]` cell on the same `/ws`, key and port. A
  client's copies of every cell share one socket, as aio's page does: each
  copy is a link of its own on it (its hello, user, rules, view, pace),
  numbered in the socket's frames (`<link>:<message>`,
  [transports](../clients/transports.md#one-socket-for-every-cell)). An aio
  page (feature `compat`) sees every shared cell in its `state`.
- **Files**: `[server.dirs]` `"/media" = "media"` serves a folder of
  `data/files` behind the key, `[server.public_dirs]` without it (only
  what a page shown before sign-in needs).
- **Limits** (`[server.limits]`): `clients` (256), `max_message` (4 MB),
  `calls_per_sec` (200) and `bytes_per_sec` (8 MB) per link, bursts up to
  twice that. A refused client gets 503 with the reason, a too-big
  message a close naming the limit, a fast sender is read more slowly:
  nothing is dropped. `sync_interval_ms` (50) paces what a link is sent
  (a person's call goes at once) and `full_state_percent` (50) sends the
  whole view for a bigger patch ([patches](../persistence/offline.md#what-travels-patches)).

## Pairing and discovery

A device gets a served app's key without anyone typing it:

```sh
ax pair board                      # on the server's machine: an 8-digit code (3 minutes, once)
ax pair board 192.168.1.20:8100 12345678   # on the device
ax discover                        # the aio-rs apps on this LAN that answer
```

- The default code is a PAKE (CPace bound to the TLS certificate the
  device sees): the code never crosses the network and a relay learns
  nothing. `ax pair <app> --plain` makes aio's plain 6-digit PIN, sent to
  `POST /__aio/pair`.
- 8 wrong guesses from one address (one /64 for IPv6) lock that address
  out; 20 wrong in all burn the code. Needs `[server] pairing = true`.
- Discovery is aio's UDP format on port 8099: nothing is announced on a
  timer, only private and loopback senders get answers, at most 5 a
  second each. Needs `[server] discover = true`. A probe goes to every
  network this machine is on (the limited broadcast and each interface's
  own); each answer carries the answering machine's addresses, so one
  machine heard twice is listed once. A probe nobody could be sent is an
  error, never an empty list.
- `ax profile <app> --out file` writes the app's client profile (host,
  port, root, key: a credential, written 0600); `ax import <app> <file>`
  links a device with it, as `pair` does. On a phone a key never crosses
  a network in the clear: pair for a pinned TLS link.

## Settings

`[server]` in `aio.toml`, for `client-server` and `server` mode:

| Key | Default | |
|---|---|---|
| `listen` | none | `host:port` to serve on; a port in use stops startup |
| `tls` | `false` | serve `wss://` with this machine's root (`ax trust`) |
| `cert`, `key` | none | with `tls`: your own certificate chain and key (absolute PEM paths) |
| `pairing` | `false` | let devices pair with a code ([pairing](#pairing-and-discovery)) |
| `discover` | `false` | answer LAN discovery |
| `hosts` | `[]` | names it is reached as besides IPs, `localhost` and `.local` (a reverse proxy's); any other Host is refused |
| `strict_origin` | `false` | a socket upgrade must say its Origin |
| `trust_proxy` | none | the header a reverse proxy overwrites with the client's address (`x-forwarded-for`) |
| `open` | `false` | no key at all: anyone who reaches the port reads and calls (said loudly at boot and by `ax doctor`; not with `[accounts]`) |
| `[server.limits]` | | per-link limits ([server functions](#server-functions-and-routes)) |
| `[server.dirs]`, `[server.public_dirs]` | | folders served behind the key, or without it |

`[client]`, for `client` mode:

| Key | Default | |
|---|---|---|
| `server` | none | `host:port` of the server; a kept link (`ax pair`, the connect page) wins over it, `--server-url` over both |
| `retention` | `"off"` | how long a waiting call is kept once the queue is full (`"7d"`) |
| `strict_pin` | `false` | a paired server whose certificate changed is refused until paired again, never re-pinned by proof ([desktop](../clients/desktop.md#a-certificate-that-changed)) |
| `[client.timeouts]` | 15 s | per method, `"board.add" = "60s"` or `"off"`: how long `call_confirmed` waits |

The key a client shows is the app's: `AIORT_TOKEN`, else
`data/sync.token`, made once at random (0600), never in `aio.toml` or a
log; a short one set by hand is warned about. `ax profile <app>` prints it with the host, port and root, and pairing hands
it over without anyone typing it. With `[accounts]` on, a client shows a
session instead and the user is the session's ([accounts](../auth/README.md)).

## Other apps, other machines

- **Peers**: `aio::peer::Peer::find("billing", "shop")` reaches the
  running app `billing` on this machine through its local door (no port,
  no key: the OS admits only this user), as user `shop` (this app's id),
  and calls it as a client would: `describe`, `call` by name, typed with
  `cell::<Ledger>()`, and its blobs (`blobs()`). What it lets a client do
  is all a peer may. `Http::resolve_peer(|app| ..)` (aio's `resolvePeer`)
  says who a peer app is here: the `Caller` (user and role) its calls,
  views and server functions get, or `None` for no one in particular.
  `Http::door_links(true)` (aio's `localConnectHooks`) makes `on_link`
  hear the door's links too (`from` is `door`). An app on another
  machine is a paired server: a `Remote` of an `aio::sync::Link { addr,
  token, root }`.
- **Tools on this machine**: `aio::sync::Live` and `Link::door(app, cell)`
  are what `ax state`, `ax call` and `ax clients` use; `ax clients <app>
  [cell]` lists the links open to a cell (user, wire `aio-rs`, `tool` or
  `aio`, address, round trip).
- **Plugins**: a crate returns an `aio::plugin::Plugin` (cells, routes,
  server functions, hooks); `aio::plugin::compose(cells, http, plugins)`
  adds them under the app's own. The app wins every route and server
  function; any other collision (two plugins on one route, one cell
  twice) refuses the boot naming both sides. A plugin is linked by Cargo:
  nothing is loaded at run time.
- **aio apps**: with feature `compat` a served aio-rs app speaks aio's
  protocol to aio pages and tools, and `aio::compat::AioLink` calls an
  aio app ([wire](../migration/wire.md)).

## Merging

A server replays each method in order, so what a method expresses merges
by itself: `n += 1` from two offline clients counts twice, a set's
`insert` keeps both, and a method that checks the state (`ensure!(..)`)
decides on the state it really runs on. There are no merge strategies to
pick: write the intent as a method. Prose is the one value that needs
help: [offline and merging](../persistence/offline.md) has it, with what a
patch on the wire holds. A copy hears where a server change met its
waiting call (`on_conflict`) and may show a field merged while it waits
(`merge`, [conflicts](../persistence/offline.md#conflicts-and-fields-shown-merged)).

## Common mistakes

- A method the client calls without `#[client]`: the server refuses it
  and the copy rolls back. Mark it, with a rule.
- Trusting `cx.by()` without accounts: a key holder can say any name.
  Turn `[accounts]` on and decide on `cx.role()` or an `allow` rule.
- Filtering sensitive data in the view: a client got it already. Filter
  with `visible` or `#[server]`.
- `predict = true` (the default) on a call whose result the client can't
  know (a payment's approval): use `predict = false` or a server function.
- `call_confirmed` on an async task's thread: it blocks; wrap it in
  `spawn_blocking`, or use `remote.run` for a server function.
- Reading a copy's typed call result as the server's word: it is the
  copy's prediction. Use `call_confirmed` when the server's answer counts.
- Sending one user's data in a tell: every client of the cell shows it.
- Expecting a refused prediction to reach the caller: it was accepted on
  the copy; the server's refusal reaches `on_refused`, so listen there.

## Limits

What aio has and aio-rs does not, or does differently:

- **JSON only.** One JSON message per WebSocket frame; there is no binary
  wire for state or calls. Bytes go through
  [blobs](../persistence/sql.md#blobs-files-and-big-data), a route or a
  side channel (`Route::socket`).
- **A link per cell on the socket.** aio's page has one subscription over
  every cell; an aio-rs client has one link per cell, all on one socket,
  each counted against `[server.limits] clients`. Time travel and aio's
  CRDT ops (`op`, `sync-req`) from an aio page reach the app's own cell.
- **A full offline queue refuses the call.** Both keep 500 waiting calls
  (aio's `pendingCap`) and let ones older than `retention` go; past that,
  aio drops the new op and aio-rs refuses the call, said to the caller.
- **Typed calls return the prediction.** aio's `await cell.method()` in a
  page resolves with the server's answer; aio-rs's typed call returns at
  once with the copy's own, and `call_confirmed` is the server's answer.
- **Tells go to every client of the cell**, as aio's `notify` does: no
  per-user tells.

## API

`cargo doc -p aio-rs --no-deps --all-features --open`: `aio::sync`
(`serve`, `serve_with`, `Options`, `Remote`, `Link`, `Live`, `Synced`,
`CALL_TIMEOUT`, `PENDING_MAX`, `PING`, `DEAD`, `SEND_TIMEOUT`), `aio::Status`,
`aio::Conn`, `aio::Code`, `aio::Rejected`, `aio::http`
(`Http`, `Route`, `Req`, `Reply`), `aio::fns`, `aio::peer`,
`aio::plugin`, `aio::pairing`, `aio::discovery`, `aio::Source`,
`aio::testing::TestServer`.

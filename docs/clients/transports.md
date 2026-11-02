# Transports and streams

What an aio-rs app listens on, who may connect, and how bytes travel: the
window's socket, the local door every app opens, and the network server
a served app adds. A standalone desktop app opens no TCP port at all.

## What listens, where

| Door | What it carries | Where | Who may connect | When |
|---|---|---|---|---|
| the window's wire | the views' DOM edits and events, length-prefixed frames | a socket in the app's private socket dir | only the Electron process this app spawned (its pid and start time) | a window app, while its window is open |
| the window's control line | focus, quit, dialogs, notifications, tray clicks, bounds; one JSON per line | the same dir | only that process | likewise |
| later launches | a second launch's working dir and arguments | the same dir | this user | every app, from `aio_shell::start` |
| the local door, one per cell | the sync protocol: state, patches, calls, server functions | the same dir (`cell-<id>`) | any process of this user | every app whose cell lives in it, in every build |
| the inspector door | Chrome's DevTools protocol, one JSON per line | the same dir (`ui`) | this user | a debug build's window only |
| the server | HTTP: `/ws` (sync), `/__aio/…` (pairing, sign-in, the UI tabs), the app's routes, files | `[server] listen` (TCP) | whoever shows the key, or a session | `[app] mode = "client-server"` or `"server"` |
| discovery | aio's LAN probe and answer | UDP 8099 | private and loopback senders, 5 answers a second each | `[server] discover = true` |
| `ax ui --browser` | the app manager's tabs | a loopback port | a key made for the run | while it runs |

- **The private socket dir** is `$XDG_RUNTIME_DIR/aio-rt/<id>-<hash of the
  data dir>` (else the temp dir), mode 0700, so no other user can open a
  socket in it. When that base is too long for a socket in it (a Unix socket
  path takes 107 bytes on Linux, 103 on macOS), it is
  `/tmp/aio-rt-<uid>/<id, 40 bytes at most>-<hash>` instead, as aio fell back
  to `/tmp/aio`. Each door is `<dir>/<name>.sock`
  (`aio::ipc::addr(dir, name)`; `aio::paths::sockets(id, data)` makes the
  dir). Two instances of one app (`--instance`) have different data dirs,
  so different doors.
- **On Windows** each door is a named pipe,
  `\\.\pipe\aio-rt-<id>-<hash>-<name>`, whose access list admits only this
  user and no remote machine. The protocol on it is the same.
- **Zero TCP ports is the default.** `standalone` and `client` apps
  listen on nothing but sockets; the page loads from `aio://app/`
  (Electron reads it from the app's shell dir) and talks only to its
  socket. A port opens only in the serving modes (`client-server`,
  `server`), where `[server] listen` names it; `--port`, `--host` and
  `--expose` move it per launch.

`ax data <app>` says where an app's files are; `ax clients <app> [cell]`
lists its windows or the links open to a cell (user, wire, address,
round trip).

## The local door

Every cell that lives in the app is served to this user's tools on its
door: `ax state`, `ax call`, `ax events`, `ax timetravel`, scripts and
other apps ([peers](../state/sync.md#other-apps-other-machines)). No key
and no network: the OS admits only this user.

| Build | A tool on the door may |
|---|---|
| debug (`ax dev`) | everything an operator may: any method, `#[server]` fields, time travel, loading a state, the list of open links (`aio::door::OPERATOR` is true) |
| release | what a client with the key may: `#[client]` methods as their rules say, what `visible` shows, server functions; nothing else |

```rust
use aio::prelude::*;
use aio::sync::Remote;
use std::time::Duration;

#[cell(persist)]
pub struct Notes {
    pub items: Vec<String>,
}

#[methods]
impl Notes {
    #[client]
    pub fn add(&mut self, text: String) -> Result {
        ensure!(!text.trim().is_empty(), "empty note");
        self.items.push(text);
        Ok(())
    }
}

# fn main() {
let dir = tempfile::tempdir().unwrap();
let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
rt.block_on(async {
    let h = Handle::<Notes>::memory();
    // What the app does at boot: its cell on a door in its private dir.
    let addr = aio::ipc::addr(dir.path(), "cell-notes");
    let l = aio::ipc::Listener::bind(&addr).unwrap();
    tokio::spawn(aio::door::serve(h.clone(), l, Default::default()));

    // A tool of this user: no key, the same protocol as a network copy.
    let tool = Remote::<Notes>::connect(&addr, "", "me").await.unwrap();
    tool.add("milk".into()).unwrap();
    for _ in 0..300 {
        if h.state().items.len() == 1 {
            break;
        }
        aio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(h.state().items, ["milk"]);
});
# }
```

- In an app this is never written by hand: `aio::Source::for_app` opens
  each cell's door at boot (a door that can't open is said and the app
  goes on). `aio::sync::Link::door(app, cell)` finds a running app's door
  by its id; `aio::sync::Live` is what `ax state` reads with.
- **The wire**: the client sends one version byte (`aio::door::VERSION`,
  1), then both sides send frames: a big-endian `u32` length, a kind byte
  and its payload. Kinds are a JSON message (the same messages as `/ws`),
  a ping, its pong, and a close with a code and a reason. A version the
  app doesn't speak gets a close saying so.
- A door speaks only aio-rs's protocol; an aio tool reaches an aio-rs app
  over the network (`/ws` with feature `compat`,
  [wire](../migration/wire.md)).

## The window's sockets

The window's page holds no app code: the app's `VirtualDom` runs in the
Rust process, and its edits reach the page as frames on the wire socket
(at most 256 MB each). The page's events come back the same way.
Electron's main process has a second line for what only it can do
([desktop](desktop.md)).

Both sockets serve one process: the Electron the app just spawned, told
by the kernel's peer pid and that pid's start time, so another program of
the same user (or a later process given the same pid) is refused, said
and dropped. With no window open they serve no one.

A key held down sends repeats faster than a view may need: stale repeats
of a held key are dropped before they reach the view.

## The server

`[server] listen = "host:port"` (feature `server`; `tls` for HTTPS) opens
one TCP port for everything network-facing:

| Path | What |
|---|---|
| `/ws` | sync: a WebSocket, one JSON message per frame ([sync](../state/sync.md)); server functions ride it |
| `/__aio/…` | aio-rs's own: pairing, sign-in and accounts, the UI tabs; never an app's |
| the app's routes | `aio::http::Route`s given to `Http::route` |
| the shell and dirs | `Http::shell(dir)`, `[server.dirs]`, `[server.public_dirs]` |

- **The key**: every path but a `Route::public` one, a public dir,
  `/__aio/pair` (the pairing code proves it), `/__aio/health` and
  `/__aio/metrics` from this machine on an app without accounts or users,
  and (with accounts) the sign-in page needs the app's key (`Authorization: Bearer`, `?token=`,
  or the cookie a `?token=` link sets), or a session. Wrong guesses from one address are slowed and then
  locked out.
- **The gate**, before any route: the `Host` must be an IP, `localhost`,
  a `.local` name or one `[server] hosts` lists (DNS rebinding); a
  browser's `Origin` must be this host or a listed one; a `POST` body must
  be JSON (CSRF); `strict_origin = true` makes an exposed app refuse a
  socket upgrade that says no `Origin`. Behind a reverse proxy,
  `trust_proxy = "x-forwarded-for"` takes the client's address from the
  header's last hop.
- **TLS**: `tls = true` serves HTTPS with this machine's own root
  (`ax trust` explains how browsers trust it), or `cert` and `key` name
  the operator's own PEM files ([auth](../auth/README.md#tls-and-the-network)).
- **Limits** per link (`[server.limits]`): `clients` 256, `max_message`
  4 MB, `calls_per_sec` 200, `bytes_per_sec` 8 MB, bursts up to twice
  that. A full server answers 503 with the reason; a fast sender is read
  more slowly, nothing is dropped.
- **Liveness**: a link pings every 2 s (`aio::sync::PING`) and is dead
  after 5 s without an answer (`DEAD`); a send that takes 10 s
  (`SEND_TIMEOUT`) ends the link. A copy reconnects with backoff.
- **Exposing it**: `listen = "127.0.0.1:8100"` is this machine only;
  `"0.0.0.0:8100"` is the network, and then `tls`, pairing and
  `strict_origin` are worth turning on. A firewall may still block it.
- `open = true` drops the key on purpose: anyone who reaches the port
  reads and calls. Said at boot and by `ax doctor`.

### One socket for every cell

aio's page carries every cell on one socket; so does an aio-rs client.
`Http::cell(handle)` serves more `#[cell(shared)]` cells on the same
`/ws`, and every copy (`Remote`, a page's `Remote::open`, a `Live` tool)
of one server, key and pinned root shares one socket of the process or
page: each copy is a link of its own on it (its hello, user, rules, view
and pace), and one copy dropped closes its link, not the socket.

- **Asked for**: wire 2 plus `x-aio-mux: 1` (a page: the WebSocket
  protocol `aio-mux`, as it sets no header); the server answers with the
  same header (or protocol). A server that doesn't (an older aio-rs) gets
  a socket per copy, as before.
- **Frames**: each text frame is `<link><kind><rest>`, `<link>` a number
  the client gives each new link: `:` a link's message (`7:{"t":"hello",…}`),
  `?` a ping and `.` its pong (`rest` the payload, hex), `!` a link closes
  (`<code> <why>`). A link the server refuses (`too many clients`, another
  cell) gets its `refused` message and its close; the socket stays.
- **Limits**: each link past a socket's first counts as a client of
  `[server.limits] clients`, at most 64 links on one socket; a link that
  lets more than 8 MB (or one `max_message`) wait unread is closed; a
  socket with no link for 15 s is closed.
- **Who**: the socket's key or session is every link's; a session that
  ends closes the socket and all its links. Each cell's own rules
  (`#[client]`, roles, `visible`) still decide per link.
- Measured on loopback (a copy of each of three cells, 300 server
  changes): 3 connections before, 1 now; patch latency p50 44 → 47 µs,
  p99 207 → 188 µs with `sync_interval_ms = 0` (the pacing window, 50 ms
  by default, dominates otherwise:
  `cargo test -p aio-rs --features net,testing --test one_socket -- --ignored --nocapture`).

### Routes or server functions

| You need | Use | It costs |
|---|---|---|
| your own UI asks the server for an answer | a server function (`Http::server_fn`) | nothing more: it rides the sync link, `/ws` or the door, typed at both ends |
| a value a cell should keep | a `#[client]` method | the same link |
| another program calls in (a webhook, an OAuth callback, `curl`) | a route (`Http::route`) | a listening port: routes are served only by `[server] listen`, never on a door |
| events pushed to a page or a tool (SSE, NDJSON) | a route answering `Reply::stream(type, parts)` | the same port; each part goes as it is made |

## Binary data

aio-rs's sync wire is JSON only. A sustained binary stream (screen
frames, audio, telemetry, remote input) goes on a side channel: the cell
carries control (who, which session, start and stop), a WebSocket route
carries the payload, never persisted, diffed or sent to every client
(aio's binary streams).

```rust
use aio::http::side::{Frames, Seen};
use aio::http::{Http, Reply, Route};

fn http() -> Http {
    Http::default().route(Route::socket("/stream", |req| async move {
        // Decided before the upgrade: an unknown session never gets a socket.
        if req.query("sid") != Some("s1") {
            return Err(Reply::text("unknown session").status(403));
        }
        Ok(|mut f: Frames| async move {
            while let Some(frame) = f.recv().await {
                if let Seen::Gap(n) = frame.seen {
                    eprintln!("lost {n} frames of kind {}", frame.kind);
                }
                // Kind 1 (video) is droppable: the next frame supersedes it.
                if f.send(frame.kind, &frame.body, frame.kind == 1).is_err() {
                    return; // backpressure on a frame that can't be dropped: over
                }
            }
        })
    }))
}
# fn main() { let _ = http(); }
```

- Frames are `[kind:u8][seq:u24 LE][payload]` (`side::encode`,
  `side::decode`), one sequence counter per kind (`side::Seqs`), wrapping
  at 2^24, so interleaved kinds never look like a loss.
- `send(kind, body, droppable)`: past `side::HIGH_WATER` (4 MB) waiting
  for a slow receiver, a droppable frame is dropped (`Ok(false)`) and any
  other closes the stream with 1011 `backpressure`, never a silent desync.
- `recv` gives each frame with `Seen::Next`, `Gap(n)` or `Old` against
  the last of its kind: a gap on a droppable kind is information, on
  another a broken stream to tear down.
- The route is behind the key (`?token=` from a browser, or a session),
  its frames up to `max_body`; a plain GET is 426. The side channel and
  the sync link open and fail apart: let the cell say whether a session
  exists.

Other big or binary data travels these ways:

| Need | How |
|---|---|
| upload a file | a `Route::post(..).streamed().max_body(bytes)`: `req.body_reader()` hands the body over as it arrives (a few chunks of memory), into `aio::blobs`; a body said to be bigger is 413 before the handler runs |
| download, play or seek a file | `aio::http::serve_file` from a route, with `Range`; or `[server.dirs]` |
| content-addressed files shared with clients | `aio::blobs` (`Http::blobs`; [sql](../persistence/sql.md)) |
| a stream of results (an LLM's tokens, progress) | `Http::server_fn_streamed`: `parts.send(&x).await` on the server, `remote.run_streamed` on the client, parts in order before the answer |
| live values that change often | a cell field with `coalesce`, read per field by the view ([performance](../performance.md)) |
| compression | every 200 is gzip (and Brotli with feature `br`) when the client asks (`aio::http::encoding`) |

## Gaps

What aio has here and aio-rs doesn't yet:

- Routes are never served over a socket: an app with routes and no
  `[server] listen` doesn't serve them (aio serves a desktop app's routes
  over its local socket through `aio://`).
- No `transport` switch: `--port`, `--host` and `--expose` only move
  where a serving mode listens; they open no port on a `standalone` or
  `client` app.
- A release build's doors serve any process of this user as a client
  would; aio's `allowLocalPeers` opt-out has no counterpart (the window's
  own sockets are always locked to the window).

## Common mistakes

- A webhook as a server function: an outside caller has no sync link.
  Make it a `Route::post(..).public()` with its own signature check.
- A route in a standalone app: nothing listens, so nothing answers. Set
  `[app] mode = "client-server"` and `[server] listen`.
- Binding `0.0.0.0` with plain HTTP and the key in `?token=` links: the
  key crosses the network in the clear. Use `tls` and pairing.
- Expecting `ax call` on a release build to reach a method without
  `#[client]`: a release door gives tools only a client's rights.
- A long socket path on macOS or Linux: the private dir is kept short on
  purpose (and falls back to `/tmp/aio-rt-<uid>`); don't move the data dir's
  sockets yourself. A path still too long is refused at once, naming it, its
  length and the limit: shorten `XDG_RUNTIME_DIR`/`TMPDIR` or the app id.

## API

`cargo doc -p aio-rs --no-deps --all-features --open`: `aio::ipc`
(`addr`, `Listener`, `Only`, `connect`), `aio::door` (`serve`, `at`,
`VERSION`, `OPERATOR`), `aio::sync` (`Link`, `Remote`, `Live`, `PING`,
`DEAD`, `SEND_TIMEOUT`), `aio::http` (`Http`, `Route`, `serve_file`,
`encoding`), `aio::gate`, `aio::blobs`, `aio::paths::sockets`.

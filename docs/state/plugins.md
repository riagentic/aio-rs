# Plugins and other apps

There are three ways to use code or data from outside your app:

| You want | Use | Needs the other app running |
|---|---|---|
| a working piece added to your app (an audit log, a file share) | a [plugin](#plugins): a crate returning `aio::plugin::Plugin` | no: it runs inside yours |
| another aio-rs app's types, methods, rules | [a shared crate](#reuse-the-code) both apps depend on | no |
| another aio-rs app's live data and calls | [a peer](#use-a-running-app-peers): `aio::peer::Peer` | yes, on this machine |
| an aio-rs app on another machine | [a paired server](#an-app-on-another-machine): `aio::sync::Remote` | yes, reachable |
| an aio app (the Deno-based framework) | [`aio::compat::AioLink`](#aio-apps) (feature `compat`) | yes |

## Plugins

A plugin is a crate whose function returns an `aio::plugin::Plugin`: its
cells (with their schedules), routes, server functions and hooks, as one
value the app adds in one line. It is linked by Cargo at build time:
there is no loader, no registry, and nothing is found at run time.

```rust
use aio::Cells;
use aio::http::{Http, Reply, Route};
use aio::plugin::{Plugin, compose};
use aio::prelude::*;
use aio::testing::TestServer;

// ---- the `audit` crate ----

#[cell(persist, id = "audit")]
pub struct AuditLog {
    pub lines: Vec<String>,
}

#[methods]
impl AuditLog {
    pub fn record(&mut self, line: String) {
        self.lines.push(line);
    }
}

/// aio's `setup()`: runs once, when the app calls it. One that can't set
/// itself up returns an `Err` for the app's `?`, so it never half-exists.
pub fn audit(log: Handle<AuditLog>, path: &str) -> std::result::Result<Plugin, aio::Error> {
    if !path.starts_with('/') {
        return Err(aio::Error(format!("audit: {path:?} is not a path")));
    }
    let (csv, rec) = (log.clone(), log.clone());
    Ok(Plugin::new("audit")
        .cell(log, &[])
        .route(Route::get(path, move |_| {
            let body = csv.state().lines.join("\n");
            async move { Reply::text(body) }
        }))
        .on_effect(move |cell, key| {
            _ = rec.record(format!("{cell}: {key}"));
        }))
}

// ---- the app ----

#[cell(persist, shared)]
pub struct Todo {
    pub items: Vec<String>,
}

#[methods]
impl Todo {
    #[client]
    pub fn add(&mut self, cx: &Cx, text: String) {
        cx.run(Effect::new("save", |_: Handle<Todo>| async {}));
        self.items.push(text);
    }
}

async fn until(f: impl Fn() -> bool) {
    for _ in 0..300 {
        if f() {
            return;
        }
        aio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("timed out");
}

# fn main() {
let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
rt.block_on(async {
    let (todo, log) = (Handle::<Todo>::memory(), Handle::<AuditLog>::memory());
    assert!(audit(log.clone(), "audit.csv").is_err(), "a plugin that can't set up refuses");

    let app = Http::default().route(Route::get("/", |_| async { Reply::text("the app") }));
    let plugins = vec![audit(log.clone(), "/audit.csv").unwrap()];
    let (cells, http) = compose(Cells::default().with(todo.clone(), &[]), app, plugins).unwrap();
    let cells = cells.start().unwrap();
    assert_eq!(cells.ids(), ["todo", "audit"], "its cell beside the app's");

    let server = TestServer::start(todo.clone(), http).await.unwrap();
    todo.add("milk".into()).unwrap();
    until(|| log.state().lines == ["todo: save"]).await; // its hook heard the app's effect
    let got = server.fetch("GET", "/audit.csv", "").await.unwrap();
    assert_eq!(got.body, "todo: save");
    assert_eq!(server.fetch("GET", "/", "").await.unwrap().body, "the app");
});
# }
```

### What a plugin can add

| `Plugin` method | Adds | Same as the app's own |
|---|---|---|
| `Plugin::new(name)` | a plugin, named in every collision | |
| `.cell(handle, after)` | a cell, started after the cells `after` names | `Cells::with` |
| `.route(Route)` | an HTTP route, under the app's | `Http::route` |
| `.server_fn(F, access, run)` | a server function | `Http::server_fn` |
| `.on_link(f)` | hears each client link open and close (`aio::http::Presence`) | `Http::on_link` |
| `.on_effect(f)` | hears every effect as it starts (`cell`, `key`) | `aio::on_effect` |
| `.on_action(f)` | hears every committed call (`cell`, the event, the cell's state after it as JSON; aio's plugin `onAction`) | `aio::on_action` |
| `.allowed_origins(&["app.example.com"])` | hosts (or origins) the app may also be reached as, added to `[server] hosts` (aio's plugin `allowedOrigins`) | `[server] hosts` |
| `.on_start(f)` | runs once the cells are live, after the app's own (aio's plugin `onStart`) | `Cells::on_start` |
| `.on_stopping(f)`, `.on_stop(f)` | run at the app's stop: first, and last (aio's plugin `onStop`) | `aio::on_stopping`, `aio::on_stop` |

`aio::plugin::compose(cells, http, plugins)` checks the plugins, then adds
them: their cells beside the app's, their routes and server functions
under the app's, their hooks beside the app's. It says each plugin and its
cells in the log. An app without feature `server` calls `compose(cells,
plugins)`: cells and hooks only.

### The rules

1. **Nothing private.** A plugin uses the same `Handle`, `Route`,
   `ServerFn` and hooks an app writes itself. Whatever a plugin does, the
   app could do without it.
2. **The app wins.** The app's routes match before any plugin's (a
   plugin's route answers only a path none of the app's takes), and the
   app's server function of a name is the one served.
3. **A collision refuses the boot, naming both sides**: two plugins on one
   route (`/u/:id` and `/u/:name` are one route; `GET` and `POST` are
   two; a route for any method collides with each), one cell, or one
   server function; a cell both the app and a plugin hold; a plugin listed
   twice. Nothing is added before the check passes.
4. **Hooks compose.** Every plugin's `on_effect` and `on_link` hears, in
   the order the plugins were listed, beside the app's. Each is guarded:
   one that panics stops none of the others.

```rust
use aio::Cells;
use aio::http::{Http, Reply, Route};
use aio::plugin::{Plugin, compose};
use aio::prelude::*;

#[cell]
pub struct Files {
    pub n: u32,
}

#[methods]
impl Files {}

fn serving(name: &'static str, path: &str) -> Plugin {
    Plugin::new(name).route(Route::get(path, |_| async { Reply::text("x") }))
}

fn boot(plugins: Vec<Plugin>) -> String {
    match compose(Cells::default(), Http::default(), plugins) {
        Ok(_) => "ok".into(),
        Err(e) => e.0,
    }
}

# fn main() {
let e = boot(vec![serving("a", "/u/:id"), serving("b", "/u/:name")]);
assert!(e.contains("claimed by both \"a\" and \"b\""), "{e}");
assert_eq!(boot(vec![serving("a", "/u/:id"), serving("b", "/u/:id/posts")]), "ok");
let cell = |name| Plugin::new(name).cell(Handle::<Files>::memory(), &[]);
assert!(boot(vec![cell("a"), cell("b")]).contains("cell `files`"));
assert!(boot(vec![serving("a", "/x"), serving("a", "/y")]).contains("listed twice"));
# }
```

### When not to write one

- Code only one app uses: write it in the app.
- Shared types or helpers with no cell, route or hook: a plain crate.
- Something that must run on its own, with its own data and updates: a
  separate app, reached as a [peer](#use-a-running-app-peers).

Limits, compared with aio's `definePlugin`: a plugin can't add views,
windows, `aio.toml` settings or capabilities; the app adds the plugin's
views to its own routes and declares what it needs. There is no plugin
discovery at run time, by design.

## Reuse the code

Two apps that share cell types put them in a crate both depend on: the
cell, its methods, its `#[client]` rules and `visible`, its tests. Each
app that runs the cell has its own copy of the data. The other app gets
the same types to call it with, typed, as a peer or a client
(`peer.cell::<Ledger>()`, `Remote::<Ledger>`). The cell's `id` is the
contract: both sides must use the same crate version, or at least the
same id and wire shape.

## Use a running app: peers

`aio::peer::Peer::find(app, user)` reaches the running aio-rs app `app` on
this machine through its local doors: no port, no key, the OS admits
only this user. It calls as a client, never with the operator's rights
`ax` gets in a debug build: what the other app lets a client do (its
`#[client]` methods and their `allow` rules, its `visible`, its
`Access::Open` server functions) is all a peer may.

```rust,no_run
use aio::peer::Peer;
use aio::prelude::*;
use serde_json::json;

// From the `billing` app's crate, shared by both apps.
#[cell(shared)]
pub struct Ledger {
    pub total: i64,
}

#[methods]
impl Ledger {
    #[client]
    pub fn credit(&mut self, n: i64) -> Result<i64> {
        ensure!(n > 0, "nothing to credit");
        self.total += n;
        Ok(self.total)
    }
}

async fn checkout(cents: i64) -> std::io::Result<()> {
    // `None`: billing isn't running. "shop" is who billing's rules see.
    let Some(billing) = Peer::find("billing", "shop")? else { return Ok(()) };
    // By name, no shared crate needed: its answer, or its refusal in its words.
    let total = billing.call("ledger", "credit", json!([cents])).await;
    assert!(total.is_ok() || total.unwrap_err().why == "nothing to credit");
    // What this caller may call, each method's signature and rule.
    let _rules = billing.describe("ledger").await?;
    // Typed, with the shared crate: its state as this caller may see it, live.
    let ledger = billing.cell::<Ledger>().await?;
    ledger.credit(cents).ok();
    let _now = ledger.state().total;
    // Its blob store: read (ranges too) and put bytes to pass by id.
    let receipt = billing.blobs().put(&b"receipt"[..], Some("r.txt"))?;
    let _ = receipt.id;
    Ok(())
}

# fn main() {
aio::rt().block_on(checkout(500)).unwrap();
# }
```

| `Peer` | Does |
|---|---|
| `Peer::find(app, user)` | the running app `app` (its data where its tools find it, `AIORT_DATA` followed), or `None` |
| `Peer::at(app, data, user)` | the same for an app whose data is at `data` (a profile, a ported aio home) |
| `.call(cell, method, args)` | one call by name, `args` a JSON list; refused at once if the app stopped, never queued |
| `.describe(cell)` | what this caller may call: methods, signatures, and whether each rule lets it in |
| `.cell::<T>()` | a typed `aio::sync::Remote<T>`: live state, typed calls, `run` and `run_streamed` for its server functions |
| `.blobs()` | its blob store (`aio::blobs::Blobs`): content-addressed, read and put directly |

- **Who is calling.** `user` is the name the other app's rules see
  (`cx.by()` is `By::User(user)`): by convention this app's id. It is a
  claim, as a key holder's is; two apps of one user trust each other as
  much as the OS does. Rules that need proof belong behind accounts on a
  server.
- **Results in pieces.** A server function served with
  `Http::server_fn_streamed` sends parts as it goes:
  `ledger.run_streamed(F, args, |part| ..)` receives them, then the answer.
- **Bytes.** Put a blob in the other app's store, then pass its id to a
  method: no bytes travel in a call.
- `ax peer <app> --as <user>` shows the same table `describe` returns,
  from the shell: what that caller may call.

### An app on another machine

An app on another machine is a server you pair with (`ax pair`, see
[pairing](sync.md#pairing-and-discovery)): `Remote::<T>::connect_to(
aio::sync::Link { addr, token, root }, user)` is a typed copy of its cell,
over TLS pinned to its root, and `aio::peer::blob(&link, id, start, end)`
fetches its blobs over HTTP with its key (feature `outbound`, held to
`[capabilities] network`).

## aio apps

With feature `compat`, an aio-rs app and an aio app work together:

- A served aio-rs app speaks aio's protocol (v3) to aio pages and tools,
  so `am` and aio's pages can use it ([wire](../migration/wire.md)).
- `aio::compat::AioLink::connect(&Link { .. })` links an aio-rs app to an
  aio server: `cells()` and `cell::<C>(name)` read what it shows,
  `subscribe(&[..])` picks cells, `changed()` waits for the next patch,
  `call(cell, method, args)` makes a `<cell>:<method>` call and waits for
  its answer, and `run(ns, name, args)` runs one of its server functions.
  Nothing is predicted: each call waits for the aio server.

## Who may connect, and AI agents

- Through the local door: only this OS user's processes. A debug build's
  door gives `ax` the operator's rights (any method); a release build's
  gives what a client gets.
- Over the network: whoever has the key, or a signed-in session, as
  `[server]` and accounts say ([access](../auth/README.md)).
- An AI agent uses an app the way a person's tools do: `ax state`, `ax
  call`, `ax peer` from the shell, with the same rules. `ax agent` prints
  the brief an agent reads first.

Limits, compared with aio: there is no ops cell with power access for
another app, and no `resolvePeer` that proves which app is calling; a
peer is a client by design.

## Common mistakes

- Calling a method the other app didn't mark `#[client]`: refused, with
  `access denied`. Ask that app for a client method; peers get no more
  than clients.
- Waiting for a peer that isn't running: `find` says `None` at once, and
  a call to one that stopped is refused at once. Retry with a
  [schedule](scheduling.md#backoff-and-polling), never a loop.
- Two plugins on one route, or a plugin's cell id already the app's: the
  boot refuses naming both; rename the one you control.
- Sending bytes as a call's argument: put a blob, pass its id.
- Two apps sharing a crate at different versions whose cell changed
  shape: keep the shared crate in step, or call by name.

## API

`cargo doc -p aio-rs --no-deps --open`: `aio::plugin` (`Plugin`,
`compose`), `aio::peer` (`Peer`, `blob`), `aio::sync::Remote`,
`aio::sync::Link`, `aio::fns::ServerFn`, `aio::http::Http`
(`server_fn_streamed`, `on_link`), `aio::on_effect`, `aio::blobs::Blobs`,
`aio::compat::AioLink` (feature `compat`).

# FAQ and pitfalls

## Questions

**Why not plain Rust?** A UI crate, SQLite and a sync layer by hand work;
aio-rs gives the log, persistence, sync, targets, tests and shipping as
one: [why aio-rs](why.md).

**What is aio-rs for?** Apps whose state matters: desktop tools, local-first
apps, small servers and the clients that sync with them, command-line
tools on saved state. One codebase, every target, every change replayable.

**How does it relate to aio?** aio (Deno, command `am`) came first;
aio-rs (Rust, command `ax`) is its Rust counterpart with the same app
model. Both run side by side and open the same app data; an aio app is
ported by translating it, construct by construct
([migration](../migration/README.md)). Which to pick, and what differs:
[aio and aio-rs compared](../aio-vs-aio-rs.md).

**Why Rust and `rsx!`?** One language from the cell to the view, checked
by the compiler; small binaries (a server fits in a few MB); views stay
real HTML and CSS, drawn by a browser engine.

**Why Electron for windows?** One shared, pinned runtime draws every app's
window (not one copy per app), so pages render exactly as in Chromium and
every web API works. The app's code stays in its Rust process; the page
gets DOM edits over a private socket.

**Is it production ready?** It is alpha: 0.x versions, and a minor version
may break (the changelog says how to move). 1.0.0 comes when the design is
frozen.

**Do I need a server?** No. `standalone` keeps everything in the app's
`state.db`. Add `shared` and `mode = "client-server"` when several devices
or people should see the same state.

**Can I use my own crates?** Yes: an app is a Cargo workspace. Keep I/O out
of methods (in effects), and keep state types serializable (`#[aio::data]`).

**Where is the API reference?** `cargo doc -p aio-rs --no-deps` builds it
from the code; this guide explains and links it.

**When is aio-rs the wrong tool?** ([why](why.md#when-not-to-use-aio-rs)) When the state is not the point: a
game loop or a 3D scene redrawn every frame (keep that state in the view,
not in cells), a stateless web service behind a load balancer (aio-rs
serves each app from one process; there is no multi-server mode), or a
dataset far bigger than memory as one cell (a call copies the state: use
`aio::imbl`, `#[table]` rows or plain SQL through `ax sql` and the store,
[persistence](../persistence/README.md)). There is no iOS client yet.

**Retries and timeouts?** On the effect: `Effect::new(..).timeout(d)`,
and `cx.backoff(key, attempt, aio::Backoff::from(1000), method, args)` to
call again later with growing delays. Both are in the log, so a replay
sees the same outcome ([effects](../state/effects.md)).

**State machines?** `aio::machine!` declares the states and which method
moves between them; `#[cell(machine = field)]` refuses a method from a
state that lacks it ([cells](../state/cells.md#state-machines)).

**Where does form validation go?** The rule that protects the data goes in
the method (`ensure!`), so every door (a view, a synced client, `ax call`)
gets it. What helps a person type goes in the view: the kit's `use_form`
shows a field's problem as they type ([kit](../ui/kit.md#forms)).

**Can several apps talk to each other?** Yes: over the local door on one
machine, over the network with a client copy, and to aio apps with
`aio::compat::AioLink` ([sync](../state/sync.md#other-apps-other-machines)).

**Do I need Electron installed?** No: `ax setup` fetches one pinned
runtime per machine and every app shares it. A release can bundle it
instead (`[build] runtime = "bundled"`) for offline installs.

**`ax doctor`, `ax health`, `ax migrations`, `ax errors`: which one?**

| Verb | Answers | Changes anything? |
|---|---|---|
| `ax setup --check` | does this machine have what making apps needs? | no (`ax setup` fetches what it can) |
| `ax doctor <app>` | is the running app the code on disk; who decided each setting? | no: it says to restart when stale |
| `ax health <app>` | is it healthy, or why not (exit 1: degraded, stuck, not running)? | no |
| `ax migrations <app>` | does stored data fit the code: each cell's version, its shape drift? | no |
| `ax errors <app>` | the failed build, then the newest warnings and errors | no |
| `ax lint` | what types can't check: impure methods, secrets, hooks, a11y | `--safe-fix` rewrites what it safely can |

aio's `am fix` and `am link` (repairing a checkout's `dep/aio` link) have
no twin: an aio-rs app takes aio-rs from git at a release tag, so a clone
builds as it is; `ax pin <version>` moves it (aio's `am pin`).

**When is aio-rs the wrong tool?**

| If you need… | Use instead |
|---|---|
| server-rendered, SEO-first pages, a static site | a site generator or a server-rendering framework |
| state redrawn every frame (a game, a 3D scene) | keep it in the view, or a game engine |
| one state across many servers or regions | a distributed database; aio-rs serves each app from one process |
| millions of stateless requests per second | a plain HTTP framework behind a load balancer |
| a native iOS app | nothing yet: aio-rs has no iOS client |

aio-rs fits when the state is the product: tools, local-first apps, small
servers and their clients, command-line tools on saved state.

**Why no statecharts?** `aio::machine!` covers flat states and the methods
that move between them. Nested states, history and parallel regions are
not there; keep such logic in a plain Rust type the cell holds, and expose
its result through the cell's fields.

**Streaming and big payloads?** A cell's state is copied on each call and
sent as patches, so bytes and whole datasets never go in a cell. Bytes go
in `aio::blobs::Blobs` (the cell keeps the id; served with Range), rows in
`#[table]` fields or plain SQL, and uploads and downloads through HTTP
routes ([SQL and blobs](../persistence/sql.md), [sync](../state/sync.md#server-functions-and-routes)).
Heavy work runs in an effect and hands back a small result.

**Can the views use something other than `aio::ui`?** Not in the app's
window: `client!`, `part` and `act!` are `aio::ui`'s (Dioxus underneath
today, air in its place later: an app names neither). Another client (a page of
your own, another program) talks to a server over `/ws` or HTTP routes
([wire](../migration/wire.md)); aio-rs ships no adapter for another view
library.

**Why no multi-server mode?** One process per app, calls in order, is
what makes each call atomic and replayable with no consensus. For several
tenants, run one app per tenant (`--instance`, `--home`, `--port`) behind
a proxy that routes by tenant.

**Where does a server run?** Anywhere a process keeps a disk: `ax build`
makes one binary (a `cli` target) and its supervisor file; copy it to a
VPS or a machine at home ([run as a service](setup.md#run-at-startup-or-as-a-service)).
Serverless and edge runtimes, with no disk and no long-lived sockets,
don't fit.

**Are schedule keys checked?** A key (`cx.after("notes:save", …)`) is any
string: a slot the same key replaces, and a label in `ax schedules`. A
method the cell doesn't have refuses the call that schedules it. Prefix
keys with the cell's id by habit.

**UDP, multicast (mDNS, SSDP)?** aio-rs has no UDP API of its own: use
`std::net::UdpSocket` (or tokio's) in an effect, never in a method.
`[capabilities] network` holds only aio's HTTPS client
(`outbound`), not your own sockets. aio-rs's LAN discovery
(`ax discover`, `[server] discover`) is built in.

## Pitfalls

The traps people hit, by area, each with its fix. One missing here is a
bug in this page: `ax feedback` says where to send it.

### State and cells

| Symptom | Cause | Fix |
|---|---|---|
| `ax lint` refuses a method | I/O, a clock, randomness or a thread in a method | `cx.now()`, `cx.id()`, `cx.rand()`; I/O in an effect |
| state differs between runs, replay diverges | a `HashMap` in state (its order differs) | `BTreeMap`, or `aio::imbl::OrdMap` |
| the app starts with no data | `[app] id` or a cell `id` was changed | ids are the data's identity: change them back |
| `h.start()` on a `Handle` doesn't call the cell's `start` method | a `Handle`'s own methods (`start`, `state`, `call`, `listen`, `watch`…) come first | name the method apart (`begin`), or call it by name: `h.call("start", json!([]))` |
| the state is back to its default after a restart | the cell has no `persist` | `#[cell(persist)]` |
| a renamed field comes back empty, the old value is gone | the shape changed without `version` and `migrate` | bump `version`, give `migrate` ([persistence](../persistence/README.md#changing-the-shape)) |
| an older build refuses to boot on the data | the data is from a newer build (a higher `version`) | run the newer build; an older one never overwrites what it can't read |
| a method "does nothing" | it refused (`ensure!`, an invariant, the machine's state), and the caller ignored the `Err` | read the `Err`; `ax events` and `ax logs` show refusals |

### Persistence

| Symptom | Cause | Fix |
|---|---|---|
| a held key writes the disk 30 times a second | the cursor method isn't coalesced | `#[coalesce]` or `#[cell(coalesce)]` |
| a coalesced call is not on disk when it returns | it is written within 100 ms | a call that must be durable at once is a plain method |
| the log keeps a passphrase | a secret passed to a method stays in its event | `#[redact]` on that method |
| deleting `state.db` "fixed" a boot and lost the data | a refused boot was data that didn't fit | read the refusal, `ax migrations`, `ax restore`; the file set aside is the data |

### Effects and schedules

| Symptom | Cause | Fix |
|---|---|---|
| a schedule fires once instead of twice | two schedules with one key: the later replaces | a key per schedule (`notes:save`, `notes:sync`) |
| an effect never runs | it was made but never handed to `cx.run(..)` (nothing warns) | `cx.run(Effect::new(..))` |
| a retry hammers a server | a fixed delay | `cx.backoff(key, attempt, aio::Backoff::from(1000), method, args)` |

### Sync

| Symptom | Cause | Fix |
|---|---|---|
| a client's call shows, then disappears | the server refused it: the prediction rolls back | read the refusal; the server's rule wins |
| a client may not call a method | it has no `#[client]` | mark it, with a rule (`role`, `allow`) |
| a payment shows as approved, then not | `predict = true` on a call the client can't know | `predict = false`, or a server function |
| a client sees a field it shouldn't | it is filtered in the view, after it was sent | `#[server]` or `#[cell(visible = f)]` |
| the build fails on a field named `token` | a credential-named shared field must be `#[server]` | mark it, or rename it if it is no secret |

### UI

| Symptom | Cause | Fix |
|---|---|---|
| a view doesn't update after a call | the method was called without `act!`/`Ui` | `act!(method(args))` |
| a view re-renders on every keypress | it reads the whole cell (`name()`) in its render | `part(\|c\| &c.field)`; the dev build's `[hint]` names it |
| `already borrowed` panic | a signal read in an `if let`/`match` scrutinee stays borrowed in its block | bind it first: `let v = X.get_untracked(); if let Some(v) = v { .. }` |
| a toast after a closed dialog never shows | `spawn` ends with its component | `spawn_forever` (`aio::ui::prelude`) |
| a CSS edit doesn't show under `ax dev` | CSS kept in `once` (made once) | `aio::use_css!` in the render |
| arrows scroll the page as well as move the cursor | `prevent_default()` comes too late in a window | `"data-aio-prevent": "ArrowUp ArrowDown"` |
| a drop handler never sees `set_data` | the view runs after the browser's dragstart | `"data-drag": "<id>"` on the dragged element |
| `key:` has no effect | `key` counts only on a `for` body's first node | put it there; to remount one element, a one-item `for` |

### Testing

| Symptom | Cause | Fix |
|---|---|---|
| a test fails at its end on a refusal | `TestCells`/`TestUi` fail on refusals nobody looked at | take the expected ones with `failures()` |
| a test waits in real time | `sleep` in a cell test | `TestCells::advance(ms)`, `settle()` |
| a test reads a fixture at run time | `CARGO_MANIFEST_DIR` paths break in other builds | `include_str!` / `include_bytes!` |
| a test passes and the app fails, or the reverse | the harnesses are as strict as the app: a refusal unread fails a test | read `failures()`; a harness laxer than the app is a bug: `ax feedback` |
| a UI test breaks when a label is reworded | it names the element by its text (`SaveButton`) | pin `"data-t": "save"` |

### Config

| Symptom | Cause | Fix |
|---|---|---|
| the build fails naming an `aio.toml` key | `aio.toml` is parsed as the app builds: a typo or a refused value is a compile error | fix that key ([reference](../reference/aio-toml.md)) |
| a setting changed in `aio.toml` does nothing in the installed app | `aio.toml` is built in, never read at run time | rebuild; per-launch settings are flags or variables |

### Silencing a lint: `// aio-ok`

`ax lint` takes one marker, and only with a reason:

```rust
// aio-ok(security): a public demo key, printed on the box
pub const DEMO_KEY: &str = "demo-0000";
# fn main() { assert_eq!(DEMO_KEY.len(), 9); }
```

- `// aio-ok: <why>` silences any finding; `// aio-ok(<area>): <why>`
  (several: `aio-ok(cells, a11y)`) only that area's (`cells`, `hooks`,
  `a11y`, `security`, `config`…, the word the finding names).
- It counts on the flagged line, or in the comment block right above it
  (attributes and doc comments between are fine). A blank line or code
  ends the block, so a marker higher up never covers other code.
- A bare `// aio-ok` or an empty reason silences nothing: the reason is
  what makes it a decision. In `aio.toml` and other files with `#`
  comments: `# aio-ok(security): <why>`.

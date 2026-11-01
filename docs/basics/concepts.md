# Concepts

An aio-rs app is state in cells, changed only by their methods, every
change an event in a log. Everything else (saving, syncing, undo, the
views, the tools) follows from that.

```text
            calls (a view, an effect, a schedule, a client, ax call)
                │
                ▼
   ┌──────────────────────────┐   refused: nothing changes, the caller gets why
   │ method(&mut copy, args)  │──────────────────────────────────────────────▶
   │ + invariants             │
   └────────────┬─────────────┘
                │ Ok: the copy becomes the state
                ▼
   commit: the event (method, args, caller, time, ids) and the state, one transaction
                │
     ┌──────────┼──────────────┬──────────────────┬──────────────────┐
     ▼          ▼              ▼                  ▼                  ▼
   views      synced        effects (I/O),     listeners on        the log:
   re-render  clients       hand results       other cells         replay, time
   their      get patches   back as calls                          travel, ax events
   fields
```

## Cells

A cell is a plain struct: `#[aio::cell]`. One app has one or many. A cell
is the unit of saving (`persist`), syncing (`shared`) and history.

```rust
use aio::prelude::*;

#[cell(persist)]
pub struct Timer {
    pub started_at: Option<i64>,
    pub laps: Vec<i64>,
}

#[methods]
impl Timer {
    pub fn begin(&mut self, cx: &Cx) -> Result {
        ensure!(self.started_at.is_none(), "already running");
        self.started_at = Some(cx.now());
        Ok(())
    }

    pub fn lap(&mut self, cx: &Cx) -> Result {
        let Some(t0) = self.started_at else { reject!("not running") };
        self.laps.push(cx.now() - t0);
        Ok(())
    }
}

# fn main() {
let h = Handle::<Timer>::memory();
assert!(h.lap().is_err());
h.begin().unwrap();
h.lap().unwrap();
assert_eq!(h.state().laps.len(), 1);
# }
```

## Methods

- The only writes. Each runs on a copy of the state; `Ok` and every
  `#[invariant]` holding make the copy the state, anything else changes
  nothing.
- Pure: no I/O, clock, randomness or threads. `cx.now()`, `cx.id()` and
  `cx.rand()` are recorded with the event, so replay gives the same state.
- Atomic and ordered: one call at a time per cell, committed before it
  answers.
- Callable by name (`h.call("begin", json!([]))`, `ax call`) and typed
  (`h.begin()`), from any door: a `Handle`, a view's `act!`, a synced
  copy, a test.

### Changing state: a copy, then a commit

A method gets `&mut self`: a copy of the state. It changes the copy in
place (`self.items.push(x)`, `self.user.age = 30`), and returns nothing,
a value, or a refusal. Nothing it did is seen until it returns `Ok` and
every invariant holds; then the copy *is* the state, in one step. There
is no draft library and no new state to return.

### Guard lines and refusals

A guard line says when a method may not run: `ensure!(cond, "why")` or
`reject!("why")`. A refusal is an `Err(aio::Rejected)`: the call changes
nothing (even what the method wrote before the guard), commits no event,
and the caller gets `why` (people) and `code` (code: `AccessDenied`,
`ActionRefused`, `ConnectionLost`). aio's guard lines return early and the
call "succeeds" as a no-op; in aio-rs the caller always learns why.

```rust
use aio::prelude::*;
use aio::serde_json::json;

#[cell]
pub struct Valve {
    pub open: bool,
    pub opened: u32,
}

#[methods]
impl Valve {
    pub fn open(&mut self) -> Result<u32> {
        self.opened += 1; // written to the copy...
        ensure!(!self.open, "already open"); // ...and dropped if this refuses
        self.open = true;
        Ok(self.opened)
    }

    pub fn close(&mut self) -> Result {
        ensure!(self.open, "already closed");
        self.open = false;
        Ok(())
    }

    #[invariant]
    fn at_most_ten_openings(&self) -> Result {
        ensure!(self.opened <= 10, "the valve is worn out");
        Ok(())
    }
}

# fn main() {
let h = Handle::<Valve>::memory();
assert_eq!(h.open(), Ok(1)); // typed
let no = h.open().unwrap_err();
assert_eq!(no.why, "already open");
assert_eq!(h.state().opened, 1, "the refused call changed nothing");
h.close().unwrap();
assert_eq!(h.call("open", json!([])).unwrap(), json!(2)); // by name, as ax call and clients do
# }
```

State machines (which method may run in which phase) are a flag, not a
library: `#[cell(machine = phase)]` ([cells](../state/cells.md#state-machines)).

## Derived values

A value computed from the state (a total, a filtered list) is a
`#[derived]` method: cached per state, or with `#[derived(reads = [a, b])]`
kept while those fields stay the same, so a view that reads it doesn't
compute it again. It may take arguments (cached per argument) and other
cells' states as `&Arc<Other>` (aio's selectors with `deps`). It never
changes state. [Cells: derived values](../state/cells.md#derived-values-and-invariants).

[Cells and methods](../state/cells.md) has every flag.

## Effects

The I/O a method needs (HTTP, files, devices, timers) runs in an effect
it asks for. The effect starts after the call commits and hands its
result back as a call, so the log holds the result and replay does no
I/O. Async methods write the same thing in one piece. Schedules call
methods later or on a cadence, and survive restarts.
[Effects](../state/effects.md).

## Views

Components (`aio::ui`, `rsx!`) read a cell's fields (`part(|c| &c.field)`) and call its
methods (`act!(method(args))`). A view re-renders when a field it reads
changes. [Views](../ui/views.md).

## Where code runs

| Mode (`[app] mode`) | The cell lives | The views are drawn by |
|---|---|---|
| `standalone` | in the app's process, in its `state.db` | the shared Electron runtime (desktop), the system WebView (Android) |
| `client-server` | the server's process; the window holds a synced copy | the same, on each client |
| `server` | a headless process serving `/ws`, HTTP routes, files | browsers, other apps, `ax` |
| `client` | a synced copy of `[client] server` | the same as standalone |
| `cli` | the tool's process | a terminal (`aio::cli`) |
| `ax build --web` | the browser page (its storage), or a synced copy | the browser |

One codebase builds for Linux, macOS, Windows, Android and the web
([targets](../build/targets.md)); the
same method code runs on the server, in a synced copy (predicting) and in
tests. The desktop window's page talks to the Rust process over a private
socket: no port, no network for the page.

## The log

Every committed call is an event: method, arguments, caller, time, the
ids and random numbers it used, and how long it took. That gives, for
free:

- **Persistence and crash safety**: the state and its event commit
  together.
- **Replay and time travel**: `ax events`, `ax timetravel`, `ax replay`,
  `ax record` (a test from what the app just did).
- **Sync**: clients get patches of what changed; offline calls replay on
  the server in order.
- **Debugging**: problem reports carry the newest calls.

## Doors

Everything that reaches a cell goes through a door with the same calls
and refusals: a `Handle` in the process, the `Ui` door views use, a
synced `Remote`, the local door (`ax state`, `ax call`, other apps on this
machine, over a private socket), and the network (`/ws`), where only
`#[client]` methods are callable.

## What crosses a door

A synced copy, a page and `ax` reach a cell over a door, and some of
them over a network. What crosses:

| Thing | Crosses? |
|---|---|
| a method's arguments | yes, as JSON (`serde`): every argument type is `Serialize` + `Deserialize` |
| its return value | yes, as JSON: the caller in the process gets the value, a copy the server's (`call_confirmed`) or its prediction |
| a refusal | yes: `Rejected { why, code }`, the same on every door |
| the state it wrote | yes, as a patch to each copy, cut by `visible` and without `#[server]` fields |
| an effect it asked for | no: it runs where the cell lives; its result comes back as a call |
| a panic in a method | no: the call is refused, the cell stays as it was, and a report is made (a third panic within a minute stops the cell) |

So a method returns plain data (ids, counts, small structs), never a
handle, a file or a closure, and a client that needs a file asks a route
or a server function ([sync](../state/sync.md)).

## Rules

What aio-rs holds every app to, and who enforces it:

1. All state lives in cells; nothing else keeps app data (a view's
   signals hold only what the screen needs). *Convention; `ax lint`
   flags cells no test names.*
2. Only a cell's methods change its state. *The compiler: outside a
   method there is only `&` state (`h.state()` is an `Arc`).*
3. Methods are pure: no I/O, clock, randomness or threads; `cx` gives the
   recorded ones. *`ax lint` flags impure methods.*
4. One cell reaches another only through a handle: a call, `listens_to`,
   or a derived value taking its state. *The compiler: a method sees only
   `self`.*
5. A call answers `Result`: a refusal is a value the caller handles,
   never a silent no-op. *The compiler (`#[must_use]`).*
6. I/O goes in effects, async methods' awaits, routes and server
   functions. *The method's split (`let x: T = io.await;`) and `ax lint`.*
7. One entry point: the launcher's `aio::config!()`, `aio_shell::start`,
   `Source::for_app` ([project](project.md#the-launcher)).

## Writing state many times a second

A cursor, a slider or a stream changes state at a high cadence. One
event per change is right for history but costly on disk:
`#[coalesce]` on the method (or `#[cell(coalesce)]`) shows each change at
once and writes a run of them within 100 ms. A view reads only the
fields it shows (`part`), so a fast field re-renders only its own
views. Decide this before the shape of the cell:
[performance](../performance.md).

## The crates

| Crate | Used as | What it is |
|---|---|---|
| `aio-rs` | `aio` | the core: cells, methods, the log, the store, effects, sync, the kit, `aio::cli`, `aio::testing` |
| `aio-rs-shell` | `aio_shell` | the process around the app: `start` (its home, log, health), `run` (the window on the shared runtime), `serve` (a server), Android, updates |
| `aio-rs-macros` | through `aio` | `#[cell]`, `#[methods]`, `#[data]` |
| `aio-rs-config`, `aio-rs-config-macro` | through `aio` | `aio.toml`'s types, and `aio::config!()`, which parses it as the app builds |
| `aio-rs-cli` | the `ax` binary | makes, runs, inspects and ships apps |

The API reference is built from the code: `cargo doc -p aio-rs -p
aio-rs-shell --no-deps` (in an app: `cargo doc --no-deps -p aio-rs`).

## Next

- [Tutorial](tutorial.md): a todo app, step by step.
- [Cells and methods](../state/cells.md), [effects](../state/effects.md),
  [views](../ui/views.md): each part in depth.
- [Project](project.md): the layout, the launcher and the features.

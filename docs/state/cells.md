# Cells and methods

A cell is a struct with `#[aio::cell]`. Its `#[aio::methods]` impl is the
only way to change it. Each call runs on a copy of the state. The copy
replaces the state only when the method returns `Ok` and every invariant
holds, so a call is atomic, and each committed call is one event in the
log. Every example on this page compiles and runs as a test of aio-rs.

Use a cell for anything the app must remember, show, sync or replay: a
list, a form's draft, a connection's status. Keep out of it what is not
state: bytes (use [blobs](../persistence/sql.md#blobs-files-and-big-data)),
caches of other systems (use [SQL](../persistence/sql.md)) and handles to
the world (sockets, devices: [owned resources](#owned-resources)).

## A cell

```rust
use aio::prelude::*;

#[cell(persist, id = "todo")]
pub struct Todo {
    pub items: Vec<Item>,
    pub next: u32,
}

#[data]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Item {
    pub id: u32,
    pub title: String,
    pub done: bool,
}

#[methods]
impl Todo {
    /// Add one; refused (nothing changes) without a title.
    pub fn add(&mut self, title: String) -> Result<u32> {
        ensure!(!title.trim().is_empty(), "a title, please");
        self.next += 1;
        self.items.push(Item { id: self.next, title, done: false });
        Ok(self.next)
    }

    pub fn toggle(&mut self, id: u32) -> Result {
        let Some(item) = self.items.iter_mut().find(|i| i.id == id) else { reject!("no item {id}") };
        item.done = !item.done;
        Ok(())
    }
}

# fn main() {
let h = Handle::<Todo>::memory();
let id = h.add("milk".into()).unwrap();
h.toggle(id).unwrap();
assert!(h.state().items[0].done);
assert_eq!(h.add(" ".into()).unwrap_err().why, "a title, please");
assert_eq!(h.state().items.len(), 1, "a refusal changes nothing");
# }
```

- `#[cell]` derives `Clone`, `Default`, `PartialEq`, serde and the cell's
  identity. `id` is the cell's name in the store and on the wire; it
  defaults to the struct's name in snake_case (`OrderBook`: `order_book`). Never change it once the
  app has run: the id is how the data is found.
- `#[data]` goes on your own types that a cell holds or a method takes.
  It derives serde through aio, so the app needs no serde dependency.
- `#[methods]` generates a `<Name>Calls` trait: `h.add(..)` is typed on
  every door (a `Handle`, a synced copy, a test client). The same method
  can be called by name: `h.call("add", json!(["milk"]))`, which is how
  the log replays and how `ax call` reaches it.
- A method takes `&mut self` and plain arguments (`&str`, `&[T]` and `&T`
  are decoded for you) and returns nothing, `aio::Result` or
  `aio::Result<T>`. `ensure!(cond, "why")` and `reject!("why")` refuse:
  the state stays as it was and the caller gets an `aio::Rejected` with
  that reason.

## How a call runs

Every call, from a view, a test, a schedule, an effect or the network,
goes through the same steps, on the cell's store, one call at a time:

```text
door (Handle, synced copy, TestUi, ax call, /ws)
  │
  ├─ storm check: refused while [perf.storm] breaker is on and this
  │  method is in a storm (default: 200 calls a second, 5 s in a row)
  ├─ aio::before_call hooks: an Err refuses
  ├─ three panics within a minute stopped the cell: refused
  ├─ a copy of the state
  ├─ the state machine moves (or refuses)
  ├─ the method runs on the copy: Err or panic refuses, the copy is dropped
  ├─ every #[invariant] runs on the copy: one Err refuses
  ├─ commit: one event in the log (or held, for a coalesced method),
  │  with the calls of its listens_to reactions in the same transaction
  │
  └─ after the commit: listeners, watchers, views' holes update,
     synced clients get a patch, effects and schedules start
```

A network call also passes its `#[client]` rule first
([sync](sync.md#who-may-call-what)). A refused call leaves no trace in the
state or the log; the caller gets an `aio::Rejected`, and a refusal of
the cell's own schedule or effect counts as a failure toward its breaker
([lifecycle](lifecycle.md)). Reading the state never waits for a running
call: `h.state()` is the last committed state.

## What the macros make

From `#[cell]` and `#[methods]` on `Todo` you get:

| Made | What it is | Use |
|---|---|---|
| `Todo::ID` | `"todo"`, the cell's id | `Cells::with(h, &[Todo::ID])`, `ax state <app> todo` |
| `TodoCalls` | a trait with `add(..)`, `toggle(..)`, typed | `h.add("milk".into())` on any door |
| `Todo::METHODS`, `SIGNATURES` | the methods' names and Rust signatures | what `ax peer` and the log show |
| `Todo::CLIENT` | the `#[client]` methods | what the network may call |
| `Todo::SCHEDULES` | the `#[every]` and `#[cron]` schedules | armed as the cell goes live |
| `impl aio::Cell` | `apply` (a call by name) and `check` (the invariants) | replay, `h.call("add", json!(["milk"]))` |
| `impl aio::CellData` | the id, version, migrate, save and restore hooks | the store |

```rust
use aio::prelude::*;
use aio::Cell;
use serde_json::json;

#[cell(id = "todo")]
pub struct Todo {
    pub items: Vec<String>,
}

#[methods]
impl Todo {
    pub fn add(&mut self, title: String) -> Result<usize> {
        self.items.push(title);
        Ok(self.items.len())
    }
}

# fn main() {
assert_eq!(Todo::ID, "todo");
assert_eq!(Todo::METHODS, ["add"]);
let h = Handle::<Todo>::memory();
assert_eq!(h.add("milk".into()).unwrap(), 1); // typed, through TodoCalls
assert_eq!(h.call("add", json!(["tea"])).unwrap(), json!(2)); // by name, as replay does
let (seq, state) = h.snapshot();
assert_eq!((seq, state.items.len()), (2, 2), "two calls, two events");
# }
```

A method name is its event's name in the log: `todo.add`. It is exactly
what you wrote, no case change.

### The handle

`aio::Handle<T>` is the door app code holds: cheap to clone, every clone
the same cell.

| Call | What it does |
|---|---|
| `Handle::memory()` | a cell in memory: tests, scratch state |
| `Handle::open(path)` | a cell in that `state.db` (the app's launcher opens yours) |
| `h.state()` | the last committed state, an `Arc<T>`: cheap, never waits |
| `h.snapshot()` | the state and the sequence number of its last event, read together |
| `h.by(by)` | the same cell, its calls recorded as made by `by` |
| `h.listen(f)` | `f(&state)` after each commit |
| `h.watch(f, on)`, `h.on_change(..)` | `on` when `f(state)` changes ([effects](effects.md#reacting-outside-a-cell)) |
| `h.listens_to(&other, "method", f)` | a call here after each call of `other.method` ([composition](composition.md)) |
| `h.until(timeout, f).await` | wait until `f(&state)` holds, refused after `timeout` |
| `h.pending(Some("method"))`, `h.busy(key)` | how many async calls are in flight, whether an effect runs |
| `h.events()`, `h.tells()` | committed events and `cx.tell` messages, from now on |
| `h.travel(ask)`, `h.pause(on)` | time travel ([debugging](../debugging/README.md)) |
| `h.own(key, res)` | keep a resource ([owned resources](#owned-resources)) |

In views, a cell's `client!` gives `part(..)`, `act!(..)` and hooks
instead ([views](../ui/views.md#a-cells-client)).

## Methods do no I/O

A method is a pure function of the state and its arguments. It does not
read files, the clock or randomness, and it starts no threads; `ax lint`
refuses those. What it needs from the outside comes in through `cx: &Cx`,
which records each value with the event so replay is exact:

```rust
use aio::prelude::*;

#[cell]
pub struct Notes {
    pub rows: Vec<(Uuid, i64, String)>,
}

#[methods]
impl Notes {
    pub fn write(&mut self, cx: &Cx, text: String) -> Result<Uuid> {
        let id = cx.id(); // a UUIDv7, kept with the event
        self.rows.push((id, cx.now(), text)); // the call's time, in ms
        Ok(id)
    }
}

# fn main() {
let h = Handle::<Notes>::memory();
let id = h.write("hello".into()).unwrap();
assert_eq!(h.state().rows[0].0, id);
# }
```

`cx.rand()`, `cx.by()` (who called) and `cx.role()` work the same way.
I/O happens around methods, in effects ([effects.md](effects.md)).
Arguments, results, refusals and `cx` in full: [methods](methods.md).

## Flags

| `#[cell(...)]` | What it does |
|---|---|
| `persist` | saved, one event per call; needed for `#[transient]`, `#[table]`, `save`, `restore`, `retire` and a web page's storage ([persistence](../persistence/README.md)) |
| `redact` | every method is `#[redact]`: no call's arguments are kept (aio's `redactActions: ["cell:*"]`) ([secrets](../auth/secrets.md)) |
| `coalesce` | every plain method shows its state at once and is written within `aio::PERSIST_DEBOUNCE` (100 ms) ([performance](../performance.md)) |
| `shared` | synced to clients ([sync.md](sync.md)) |
| `id = "name"` | the cell's name (default: the struct's name in snake_case) |
| `version = N`, `migrate = f` | the data's shape version and how older data comes up |
| `restore = f` | `fn(Self) -> Self`, run on every boot and rewind (clear busy flags) |
| `save = f` | `fn(&Self) -> Self`: what is written |
| `visible = f` | what each viewer is sent: `fn(&Self, &By) -> Self` or `fn(&Self, &Caller) -> Self` ([visibility](visibility.md)) |
| `machine = field` | the field is a state machine that guards methods (below) |
| `diagnostics = false` | keeps its calls out of the live timeline and problem reports |
| `retire = ["old"]` | deletes cells no build reads any more from the file at boot |
| `custom_default` | the struct implements `Default` itself |

| On a field | What it does |
|---|---|
| `#[transient]` | never saved; back to its default on every boot |
| `#[server]` | saved, never sent to a client, kept out of reports |
| `#[table("name")]` | the field is the rows of SQL table `name` ([data](../migration/data.md)) |
| `#[aio_cell] x: Other` | in a ported app's file, the field is a whole aio cell's slice ([data](../migration/data.md)) |

A field of a shared cell named like a credential must be `#[server]`, or
the build fails: one with the word `password`, `passphrase` or `mnemonic`,
or the pairs `api_key`, `private_key`, `secret_key`, `access_token`,
`auth_token`. A name that ends in a word about the secret (`_len`, `_id`,
`_at`, `_count`…) or holds `pub`/`public` is not one. A cell with
`visible = f` is not checked: `f` decides what is sent.

| On a method | What it does |
|---|---|
| `#[derived]` | a cached read (below) |
| `#[derived(reads = [a, b])]` | cached while fields `a` and `b` stay the same |
| `#[invariant]` | `fn(&self) -> Result`, checked after every call; a broken one refuses the call |
| `#[client]`, `#[client(role = "admin", allow = f, predict = false)]` | the network may call it ([sync.md](sync.md)) |
| `#[coalesce]` | this method's calls are written as one run (a cursor) |
| `#[skip_travel]` | time travel steps over its events and its timeline leaves them out (aio's `skipActions`: a tick, a hover); they stay in the log |
| `#[redact]` | its arguments are never kept (a passphrase); its event keeps the state instead |
| `#[every("5m")]`, `#[cron("0 9 * * 1")]` | a schedule ([scheduling](scheduling.md)) |
| `#[ttl(ms)]` | an async method's answer reused for the same arguments |
| `#[long]`, `#[long(ms)]` | lifts the await ceiling of an async method, or gives it its own |
| `#[on_start]`, `#[on_stop]` | called as the cell goes live and as it stops ([lifecycle](#lifecycle)) |
| `#[on_aio]` | in a ported app, run at boot on what aio changed in the file (feature `compat`, [data](../migration/data.md)) |

## Derived values and invariants

```rust
use aio::prelude::*;
use std::sync::Arc;

#[cell]
pub struct Cart {
    pub lines: Vec<(String, u32, u64)>, // name, quantity, cents each
    pub open: bool,
}

#[methods]
impl Cart {
    pub fn put(&mut self, name: String, qty: u32, cents: u64) {
        self.lines.push((name, qty, cents));
    }

    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    /// Computed once per state; any caller of `total()` on that state shares it.
    #[derived]
    pub fn total(&self) -> u64 {
        self.lines.iter().map(|(_, q, c)| *q as u64 * c).sum()
    }

    /// Kept, the same `Arc`, while `lines` stays the same: `toggle` reuses it.
    #[derived(reads = [lines])]
    pub fn names(&self) -> Arc<Vec<String>> {
        Arc::new(self.lines.iter().map(|(n, ..)| n.clone()).collect())
    }

    #[invariant]
    fn at_most_fifty_lines(&self) -> Result {
        ensure!(self.lines.len() <= 50, "a cart holds at most 50 lines");
        Ok(())
    }
}

# fn main() {
let h = Handle::<Cart>::memory();
h.put("tea".into(), 2, 350).unwrap();
assert_eq!(h.state().total(), 700);
let names = h.state().names();
h.toggle().unwrap();
assert!(Arc::ptr_eq(&names, &h.state().names()), "not computed again");
# }
```

A `#[derived]` function may take arguments (cached per argument) and other
cells' states as `&Arc<Other>` (aio's `deps`).

## State machines

```rust
use aio::prelude::*;

aio::machine! {
    pub enum Phase {
        Draft { submit => Review },
        Review { approve => Approved, reopen => Draft },
        Approved {},
    }
}

#[cell(machine = phase)]
pub struct Order {
    pub phase: Phase,
    pub notes: Vec<String>,
}

#[methods]
impl Order {
    pub fn submit(&mut self) {}
    pub fn approve(&mut self) {}
    pub fn reopen(&mut self, why: String) {
        self.notes.push(why);
    }
}

# fn main() {
let h = Handle::<Order>::memory();
assert!(h.approve().is_err(), "only from Review");
h.submit().unwrap();
h.approve().unwrap();
assert_eq!(h.state().phase, Phase::Approved);
# }
```

A method the machine names runs only from a state that has it, and the
field moves first, so the method sees where it is going. A machine that
names a method the cell lacks, or has a state no path reaches, refuses to
open.

## One gate for every call

`aio::before_call(|call| ..)` sees every call of every cell before it runs
(aio's `beforeReduce`): an `Err` refuses it, and one that panics refuses
too. It gets the cell, method, arguments, caller and role
(`aio::Call`); replay never asks it again, since what it let in is in
the log. It runs while the cell is locked: decide from the call alone and
never call a cell from it. `aio::on_effect(|cell, key| ..)` hears each
effect as it starts (aio's `onEffect`); hooks added by plugins and the
app all hear, in the order added.

## Lifecycle

A cell goes live when the app starts it (or a test boots it), and stops
at shutdown or when it is disabled. `#[on_start]` and `#[on_stop]` run
once each way, as ordinary calls (`pub fn (&mut self)`, a `cx: &Cx` may
follow); a refusal or a panic in one is logged and reported, and never
stops the boot or the shutdown.

`aio::Cells` is the app's list of cells: each added with the ids of the
cells it starts after. `start()` refuses a duplicate, an unknown
dependency or a cycle, then starts the cells in that order, firing the
schedules they missed while the app was off. `shutdown(budget)` stops
them in reverse order.

```rust
use aio::prelude::*;
use aio::Cells;
use aio::testing::TestCells;

#[cell]
pub struct Inbox {
    pub opened: u32,
    pub mails: Vec<String>,
}

#[cell]
pub struct Badge {
    pub unread: u32,
}

#[methods]
impl Inbox {
    #[on_start]
    pub fn open(&mut self) {
        self.opened += 1;
    }

    pub fn receive(&mut self, subject: String) {
        self.mails.push(subject);
    }
}

#[methods]
impl Badge {
    pub fn bump(&mut self) {
        self.unread += 1;
    }
}

# fn main() {
let mut t = TestCells::new();
let (inbox, badge) = (t.cell::<Inbox>(), t.cell::<Badge>());
// Badge reacts to Inbox's calls; Inbox never knows who listens.
badge.listens_to(&inbox, "receive", |badge, _event| badge.bump());
let cells = t.start(Cells::default().with(inbox.clone(), &[]).with(badge.clone(), &[Inbox::ID])).unwrap();
assert_eq!(cells.ids(), ["inbox", "badge"], "dependency order");
assert_eq!(inbox.state().opened, 1, "#[on_start] ran as it went live");

inbox.receive("hello".into()).unwrap();
assert_eq!(badge.state().unread, 1);

cells.disable(Inbox::ID).unwrap(); // calls refused, schedules paused, state kept
assert!(inbox.receive("later".into()).is_err());
cells.enable(Inbox::ID).unwrap(); // #[on_start] again
assert_eq!(inbox.state().opened, 2);
# }
```

- **The breaker**: `Cells::trip_after(max, window)` disables a cell after
  `max` failures (a panic, or a refused call of its own schedules and
  effects), its data kept; the trip is said in the log, the app's health
  and a problem report. `health()` lists each cell's state, failures and
  last call; `status(id)` is `disabled`, or the cell's own `status` field
  when it is a string.
- **App hooks**: `aio::on_stopping(f)` runs first when the app stops,
  while methods still commit (stop the app's own timers and pollers
  there); `aio::on_stop(f)` runs last, after every store is closed
  (release what the app holds; a call from there is refused).
  `aio::shutdown(budget)` runs the whole sequence.
- **Shutting down**: new calls are refused with "retry after"
  (`aio::RETRY_AFTER_MS`), schedules stop, running effects finish and
  commit; those still running at the deadline are aborted and named.

Every hook, the breaker and the shutdown order in full: [lifecycle](lifecycle.md).

## Owned resources

A cell may own something that is not state: a socket, a file watcher, a
device. `h.own(key, Some(Box::new(res)))` (any `Door`) keeps it under a
key, dropping the one it replaces; `None` drops it. Owned resources are
dropped when the cell stops or is disabled, so an `#[on_start]` that opens
one in an effect reopens it on enable. For resources that views hold by
key (a camera shown while a page is open), `aio::held::Held` opens one per
key and closes it when its last holder lets go (`aio::kit::use_held` in a
view).

## Composition

An app has as many cells as it likes. Each is its own `Handle`; cells
never call each other from a method (a method does no I/O), but around
methods they meet in four ways:

| Need | Use |
|---|---|
| a call on one cell causes a call on another | `b.listens_to(&a, "method", \|b, event\| ..)` (aio's `listensTo`) |
| read another cell's state in a derived value | a `#[derived]` argument `&Arc<Other>` |
| react to a value outside views | `h.watch(\|s\| s.field.clone(), \|v\| ..)`, `h.on_change(..)` ([effects](effects.md#reacting-outside-a-cell)) |
| an effect waits for another cell | `other.until(timeout, \|s\| s.ready).await` (aio's `until`), then call it |

`listens_to` panics at wiring time when the source has no such method, so
a typo never silently fails; a refused reaction is logged and reported and
never undoes the source's call. Its own calls are made as the same
caller. More, with dependencies between cells: [composition](composition.md).

## Where state lives

Not everything belongs in a cell. Pick by who must see it and whether it
must survive a restart:

| Kind | Lives | Use for |
|---|---|---|
| `#[cell(persist, shared)]` | the app's (or the server's) `state.db`, synced to every client | the app's data: anything two windows or a restart must agree on |
| `#[cell(persist)]` | the app's `state.db`, never sent | data only this process uses |
| a `#[transient]` field | memory, inside a saved cell | live status next to the data: a busy flag, a connection |
| a cell opened with `Handle::memory()` | memory, its default on every start | scratch state of a tool or a test |
| `static X: Global<T>` | one window's views | a panel's layout, a filter shared by views |
| `signal(..)` | one component | open or closed, hover, text being typed |

A view's signal never reaches the log, the store, a test's replay or
another window; when a value must, make it cell state (a draft that must
survive a restart is a field of a persisted cell, `#[coalesce]`d if it
changes on every key). A limit: in a native app the store saves every
cell it opens, `persist` or not (aio's `persist: "none"` has no flag
here); a value that must start fresh each launch is a `#[transient]`
field. Signals in views: [views](../ui/views.md#local-state-and-hooks).

## Naming

- The cell `id` is how its data is found: in the store, on the wire, in
  `ax state <app> <id>`. Never change it once the app has run; to replace
  a cell, give the new one a new id and `retire = ["old"]`.
- Methods are called by name in the log. Renaming a method that has run
  breaks replay of the events that name it: keep the old one, or migrate.
- `<Name>Calls` is the typed trait every door implements: import it
  (`use crate::todo::TodoCalls`) where you call methods outside the
  defining module.
- A method can't be named like one of `Handle`'s or `aio::Door`'s own
  (`start`, `state`, `call`, `read`, `travel`, `listen`, `pause`, `done`…):
  on a handle theirs would win, so `#[methods]` refuses it at build time,
  naming the clash and a new name (`start_todo`). Rust itself refuses two
  methods with one name. A field and a method may share a name
  (`h.state().total` and `h.total()`), but a `#[derived]` value named like
  a field reads badly: name it for what it computes.
- Cells, methods and fields have Rust's names (`snake_case`); the id and
  the method names are what the log, `ax` and aio-rs's wire use.

## Common mistakes

- Reading the clock, a file or randomness in a method: replay would
  differ. Use `cx.now()`, `cx.id()`, `cx.rand()`, and effects for I/O.
- Calling another cell from a method: impossible by design; use
  `listens_to` or an effect.
- Holding a lock or a handle's state across an await in an effect: take
  `h.state()` (an `Arc`, cheap) and drop it.
- A credential-named shared field without `#[server]`: the build fails,
  on purpose.
- Matching on a refusal's words: match on `e.code()` ([debugging](../debugging/README.md#1-read-the-error)).
- "no method named `add` found for struct `Handle<Todo>`": the
  `TodoCalls` trait is not in scope. Import it, or use `aio::prelude::*`
  in the cell's own module.
- "a cell method can't take this borrowed type": a method takes owned
  values, `&str`, `&[T]`, `&T` and `Option<&T>`; anything else borrowed
  (`&mut T`, `Vec<&str>`) can't be decoded from the log.
- "a cell method returns aio::Result<T> (or nothing)": return
  `aio::Result<T>` (the prelude's `Result`), not `std::result::Result` or
  `anyhow::Result`.
- A method that panics: the call is refused like any other, and three
  panics within a minute stop the cell until the app restarts.
- A value in a view's signal that must survive a restart or reach another
  window: move it into a cell ([where state lives](#where-state-lives)).

## API

`cargo doc -p aio-rs --no-deps --open`: `aio::Handle` (calls, state,
listeners, time travel), `aio::Cx`, `aio::Cells`, `aio::Cell` and
`aio::CellData` (what the macros generate), `aio::machine!`,
`aio::before_call`, `aio::on_effect`, `aio::on_stopping`, `aio::on_stop`.

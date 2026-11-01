# Composition

An app has as many cells as it needs, and each is on its own: its state,
its methods, its lock, its log. A method never reaches another cell (it
does no I/O). Cells meet around methods, in three ways:

| Pattern | What | Use for |
|---|---|---|
| [Observe](#observe-react-to-another-cell) | run something when another cell commits | counters, cleanup, cancelling, analytics |
| [Read](#read-derived-values-over-several-cells) | compute from several cells' states | totals, views, checks |
| [Coordinate](#coordinate-call-another-cell-from-an-effect) | an effect calls another cell and uses the answer | workflows: reserve, then charge |

Observe and Read change nothing in the cell they watch. Coordinate is
the only one that calls in. Every example on this page compiles and runs
as a test of aio-rs.

## Observe: react to another cell

`b.listens_to(&a, "method", |b, event| ..)` runs the closure after each
committed call of `a.method` (aio's `listensTo`). `a` never knows who
listens. The closure gets `b`'s handle, made as the caller of `a`'s call,
and the call's `aio::Event` (its `args`, `by`, `at`).

A cancel is a reaction too (aio's `cancelOn`): the reaction calls a
method that aborts the work in flight with `cx.abort(key)`, and marks the
state so that a result that arrives anyway is ignored.

```rust
use aio::prelude::*;
use aio::testing::TestCells;

#[cell]
pub struct Cart {
    pub items: Vec<String>,
}

#[methods]
impl Cart {
    pub fn put(&mut self, item: String) {
        self.items.push(item);
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }
}

#[cell]
pub struct Stats {
    pub added: Vec<String>,
    pub clears: u32,
}

#[methods]
impl Stats {
    pub fn saw_put(&mut self, item: String) {
        self.added.push(item);
    }

    pub fn saw_clear(&mut self) {
        self.clears += 1;
    }
}

#[cell]
pub struct Checkout {
    pub status: String,
}

async fn place_order() -> String {
    "order-1".into()
}

#[methods]
impl Checkout {
    pub async fn place(&mut self) -> Result {
        self.status = "placing".into();
        let id: String = place_order().await;
        if self.status == "cancelled" {
            return Ok(()); // the cart was cleared meanwhile
        }
        self.status = format!("placed {id}");
        Ok(())
    }

    pub fn cancel(&mut self, cx: &Cx) {
        cx.abort("place"); // the running await, if any, is dropped
        self.status = "cancelled".into();
    }
}

# fn main() {
let mut t = TestCells::new();
let (cart, stats, checkout) = (t.cell::<Cart>(), t.cell::<Stats>(), t.cell::<Checkout>());
stats.listens_to(&cart, "put", |s, e| s.saw_put(aio::args::<(String,)>(e.args.clone())?.0));
stats.listens_to(&cart, "clear", |s, _| s.saw_clear());
checkout.listens_to(&cart, "clear", |c, _| c.cancel());

cart.put("tea".into()).unwrap();
assert_eq!(stats.state().added, ["tea"], "the reaction ran before put returned");

checkout.place().unwrap();
cart.clear().unwrap();
t.settle();
assert_eq!(checkout.state().status, "cancelled");
assert_eq!(stats.state().clears, 1);
# }
```

- The reaction runs right after the source commits, on the caller's
  thread, before the source's call returns.
- It runs only for a committed call. A refused call of the source runs no
  reaction, and a refused reaction never undoes the source's call: it is
  said in the log and a problem report (`reaction`).
- Listening to a method the source doesn't have panics as the app wires
  its cells, so a typo can't fail in silence.
- The reaction's arguments are the source call's JSON: decode them with
  `aio::args::<(T, ..)>(e.args.clone())?`.
- Reactions are not replayed: each reaction's own call is an event of its
  own cell, so replay of either log gives the same state.

Other ways to observe, outside views ([effects.md](effects.md#reacting-outside-a-cell)):

| Call | Fires |
|---|---|
| `h.listen(\|state\| ..)` | after every commit, with the new state |
| `h.watch(\|s\| value, \|v\| ..)` | now, then whenever `value` changes |
| `h.on_change(immediate, \|s\| value, \|new, old\| ..)` | whenever `value` changes; what it returns is kept until the next change |
| `h.events()` | a broadcast receiver of every committed event |

## Read: derived values over several cells

A `#[derived]` function may take another cell's state as `&Arc<Other>`
(aio's `deps`). It is cached per state of both cells: it runs again only
when one of them commits.

```rust
use aio::prelude::*;
use std::sync::Arc;

#[cell]
pub struct Budget {
    pub limit: u64,
}

#[cell]
pub struct Spending {
    pub paid: Vec<u64>,
}

#[methods]
impl Budget {
    pub fn set(&mut self, limit: u64) {
        self.limit = limit;
    }

    /// Reads `Spending` too: typed, so a cell that doesn't exist can't compile.
    #[derived]
    pub fn left(&self, spending: &Arc<Spending>) -> i64 {
        self.limit as i64 - spending.paid.iter().sum::<u64>() as i64
    }
}

#[methods]
impl Spending {
    pub fn pay(&mut self, cents: u64) {
        self.paid.push(cents);
    }
}

# fn main() {
let (budget, spending) = (Handle::<Budget>::memory(), Handle::<Spending>::memory());
budget.set(5_000).unwrap();
spending.pay(1_200).unwrap();
assert_eq!(budget.state().left(&spending.state()), 3_800);
spending.pay(4_000).unwrap();
assert_eq!(budget.state().left(&spending.state()), -200);
# }
```

In views, each cell has its own `aio::client!` (one per module: it makes
`part`, `act!`'s door and the hooks for that cell), and a component reads
as many as it needs, each `part(..)` subscribing it to that field only
([views](../ui/views.md)).

## Coordinate: call another cell from an effect

Calling in happens where I/O happens: in an effect, or in the awaited
future of an async method. The effect reaches the other cell through its
handle, kept where the app wires its cells (a `static`, or a clone moved
into the effect), and the answer comes back as the await's result.

```rust
use aio::prelude::*;
use aio::testing::TestCells;
use std::sync::OnceLock;
use std::time::Duration;

#[cell]
pub struct Inventory {
    pub free: u32,
}

#[methods]
impl Inventory {
    pub fn stock(&mut self, n: u32) {
        self.free += n;
    }

    pub fn reserve(&mut self, n: u32) -> Result<u32> {
        ensure!(self.free >= n, "only {} left", self.free);
        self.free -= n;
        Ok(self.free)
    }
}

/// Set once, as the app wires its cells.
static INVENTORY: OnceLock<Handle<Inventory>> = OnceLock::new();

async fn reserve(n: u32) -> std::result::Result<u32, String> {
    let inv = INVENTORY.get().ok_or("no inventory")?;
    // aio's `call({ timeoutMs, retries })`: each try cut off after 5 s, one retry.
    aio::effect::retry(1, Some(Duration::from_secs(5)), || async { inv.reserve(n) }).await.map_err(|e| e.why)
}

#[cell]
pub struct Orders {
    pub placed: Vec<u32>,
    pub error: Option<String>,
}

#[methods]
impl Orders {
    pub async fn place(&mut self, n: u32) -> Result {
        self.error = None;
        let got: std::result::Result<u32, String> = reserve(n).await;
        match got {
            Ok(_) => self.placed.push(n),
            Err(why) => self.error = Some(why),
        }
        Ok(())
    }
}

# fn main() {
let mut t = TestCells::new();
let (inventory, orders) = (t.cell::<Inventory>(), t.cell::<Orders>());
assert!(INVENTORY.set(inventory.clone()).is_ok());
inventory.stock(3).unwrap();

orders.place(2).unwrap();
orders.place(2).unwrap();
t.settle();
assert_eq!(orders.state().placed, [2]);
assert_eq!(orders.state().error.as_deref(), Some("only 1 left"));
assert_eq!(inventory.state().free, 1);
# }
```

- Each side commits on its own: `Inventory.reserve` is an event in its
  log, `Orders.place.1` (with the answer) one in its own. There is no
  transaction over both ([methods.md](methods.md#work-across-cells)): when
  the second step fails, call a compensating method on the first.
- Put the answer in the state (`error` here) rather than refusing the
  step: a refused step of the cell's own effect counts as a failure for
  the breaker ([lifecycle.md](lifecycle.md#the-breaker)).
- To wait for another cell's state instead of a call, await
  `other.until(timeout, |s| s.ready)` (aio's `until`): checked now and
  after each of its commits, refused if it doesn't hold in time.
- To race two waits (aio's `race`), use `tokio::select!` in the awaited
  future.

## Who may call what, where

| Code | May call a cell? | Recorded as |
|---|---|---|
| a method | no: it has no handle, and does no I/O | |
| `aio::before_call`, `aio::on_effect` | never: they run under the cell's lock | |
| a `listens_to` reaction | yes: its own handle, or any other | the source call's caller |
| an effect, an async method's await | yes, through a handle | its own cell's handle: `By::Effect(key)` |
| a view | `act!(..)` on its cell's `Ui` door ([views](../ui/views.md)) | as that door calls |
| `main`, a test, a tool | any handle; `ax call` | `By::System`, `By::Test`, the door's user |

While the app shuts down, only effects' calls still commit
([lifecycle.md](lifecycle.md#the-apps-stop)), so a workflow that is
already running finishes.

## Wiring the cells

- **Order**: `aio::Cells` starts cells after the ones they name and stops
  them in reverse ([lifecycle.md](lifecycle.md#many-cells-aiocells)). A
  cell whose effects call another lists it, so that one is up first and
  down last.
- **Shared types**: a `#[data]` type in a module both cells import (an
  `Address` in contacts and in orders) is one Rust type; each cell saves
  its own copy of the value.
- **Names**: refer to a method by its name (`"clear"`), checked as the app
  wires; refer to a cell by its `ID` (`Cart::ID`), never a string.
- **One cell or two?** Data that must change together, or that one method
  must check against the other, belongs in one cell. Split when the parts
  change for different reasons, sync differently (`shared`, `visible`), or
  are saved differently.

## Differences from aio

| aio | aio-rs |
|---|---|
| `await other.method()` inside an async method | the awaited future calls the other cell's handle |
| `listensTo: { onX: other.x }` | `b.listens_to(&a, "x", \|b, e\| ..)` |
| a listener runs even when the source refused (a call) | a listener runs only for a committed call |
| `cancelOn: { m: [other.x] }` | a `listens_to` reaction that calls `cx.abort("m")` |
| selectors over several cells | `#[derived]` with `&Arc<Other>` |
| `call({ timeoutMs, retries })` | `aio::effect::retry(retries, timeout, f)` |
| `until(() => ..)`, `race({ .. })` | `other.until(timeout, f)`, `tokio::select!` |
| `.type` | the method's name, checked at wiring |

## Common mistakes

- Calling another cell from a method through a `static`: a deadlock if it
  is the same cell, and a second call on every replay. Do it in an effect.
- Reading another cell's state in a method: the method's result would
  depend on state its event doesn't record. Pass the value as an argument,
  or read it in the effect that makes the call.
- Refusing in a reaction to stop the source: too late, the source has
  committed. Check in the source (`ensure!`) or in `before_call`.
- A reaction that calls back the source on every commit: a call storm.
  React to a specific method, not to every change.

## API

`cargo doc -p aio-rs --no-deps --open`: `aio::Handle::listens_to`,
`aio::Handle::listen`, `aio::Handle::watch`, `aio::Handle::on_change`,
`aio::Handle::events`, `aio::Handle::until`, `aio::Event`, `aio::args`,
`aio::effect::retry`, `aio::Cells`.

# Methods in depth

A method is the only way to change a cell. [cells.md](cells.md) shows the
basics; this page covers the rest: what a method may take and return, how
it refuses, what `cx` gives it, how it is called, what "atomic" means for
one call, for an async method and for work that spans cells. Every example
compiles and runs as a test of aio-rs.

## The shape of a method

`#[methods]` looks at each `fn` of the impl:

| The `fn` | What it becomes |
|---|---|
| `pub fn name(&mut self, …)` | a method: callable by name, logged, replayed, typed on every door |
| `pub async fn name(&mut self, …) -> Result` | an async method, split at its awaits ([below](#async-methods-and-atomicity)) |
| `pub fn name(&self, …)` or a private `fn` | a plain Rust function: a helper, never called by name, never logged |
| `#[derived] fn name(&self, …) -> T` | a cached read ([cells.md](cells.md#derived-values-and-invariants)) |
| `#[invariant] fn name(&self) -> Result` | checked after every call ([cells.md](cells.md#derived-values-and-invariants)) |

A method's first parameter after `&mut self` may be `cx: &Cx`
([below](#cx-the-outside-world-recorded)); the rest are its arguments. It
returns nothing, `aio::Result` or `aio::Result<T>`; any other return type
fails the build ("a cell method returns aio::Result<T> (or nothing)"). A
method named `call` or `read` fails the build too: those names are
`aio::Door`'s.

## Arguments

```rust
use aio::prelude::*;
use aio::serde_json::json;

#[data]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Contact {
    pub name: String,
    pub phone: Option<String>,
}

#[cell]
pub struct Contacts {
    pub list: Vec<Contact>,
    pub tags: Vec<String>,
}

#[methods]
impl Contacts {
    /// Borrowed, optional and owned arguments: each decoded from the call's JSON.
    pub fn add(&mut self, name: &str, phone: Option<&str>, tags: &[String]) -> Result<usize> {
        ensure!(!name.trim().is_empty(), "a name, please");
        self.list.push(Contact { name: name.into(), phone: phone.map(Into::into) });
        self.tags.extend(tags.iter().cloned());
        Ok(self.list.len())
    }

    pub fn replace(&mut self, at: usize, with: Contact) -> Result {
        let Some(c) = self.list.get_mut(at) else { reject!("no contact {at}") };
        *c = with;
        Ok(())
    }
}

# fn main() {
let h = Handle::<Contacts>::memory();
// Typed, through the `ContactsCalls` trait `#[methods]` made.
assert_eq!(h.add("Ana", Some("555-0100"), &["work".to_string()]).unwrap(), 1);
h.replace(0, Contact { name: "Ana B".into(), phone: None }).unwrap();

// By name: the arguments as a JSON array, as the log replays them and `ax call` sends them.
assert_eq!(h.call("add", json!(["Bo", null, []])).unwrap(), json!(2));
let bad = h.call("add", json!([1, 2, 3])).unwrap_err();
assert!(bad.why.starts_with("bad arguments"), "{}", bad.why);
let unknown = h.call("remove", json!([0])).unwrap_err();
assert_eq!(unknown.code(), Some(Code::ActionRefused));
# }
```

- An argument is any type serde can decode: a number, a `String`, a
  `#[data]` struct, a `Vec`, an `Option`, a map. It is saved in the log
  with the event, so it must also encode.
- `&str`, `&[T]`, `&T`, `Option<&str>` and `Option<&T>` are accepted and
  decoded into an owned value for you. Any other borrowed type (`&mut T`,
  `Vec<&str>`) fails the build: take it owned.
- A call by name takes the arguments as a JSON array in order. No arguments
  is `[]` or `null`. Arguments that don't decode refuse the call with
  `bad arguments: …`; nothing runs.
- There is no schema to declare (aio's `args`): the Rust types are the
  schema. Check what types can't say (a range, a non-empty string) with
  `ensure!` at the top of the method.

## Results and refusals

A method that returns `Result<T>` hands `T` to its caller: a door's typed
method gives the value, a call by name its JSON. `ensure!(cond, "why")`
and `reject!("why")` refuse with a reason (`format!` syntax), and `?`
works on anything that turns into an `aio::Rejected` (a `String`, a
`&str`, another `Rejected`).

A refusal changes nothing, including what the method wrote before it
refused. The method ran on a copy of the state, and the copy is dropped:

```rust
use aio::prelude::*;

#[cell]
pub struct Roster {
    pub names: Vec<String>,
    pub problems: Vec<String>,
}

#[methods]
impl Roster {
    /// All or nothing: one bad name refuses the whole list.
    pub fn import(&mut self, names: Vec<String>) -> Result<usize> {
        for n in names {
            ensure!(!n.trim().is_empty(), "an empty name in the list");
            self.names.push(n);
        }
        Ok(self.names.len())
    }

    /// Keep what fits, and say why the rest didn't: the reason is state.
    pub fn import_some(&mut self, names: Vec<String>) {
        for n in names {
            if n.trim().is_empty() {
                self.problems.push("skipped an empty name".into());
            } else {
                self.names.push(n);
            }
        }
    }
}

# fn main() {
let h = Handle::<Roster>::memory();
let e = h.import(vec!["ana".into(), "".into()]).unwrap_err();
assert_eq!(e.why, "an empty name in the list");
assert!(h.state().names.is_empty(), "\"ana\" was pushed on the copy only");

h.import_some(vec!["ana".into(), "".into()]).unwrap();
assert_eq!((h.state().names.len(), h.state().problems.len()), (1, 1));
# }
```

So a method can't both record why it refused and refuse. Pick one: refuse
when the caller should handle it, or record the reason and return `Ok`
when the reason belongs on screen.

`aio::Rejected` has `why` (the text) and `code()`: `None` for your own
refusals, otherwise one of aio's kinds, stable across versions and on the
wire:

| `aio::Code` | Wire name | When |
|---|---|---|
| `AccessDenied` | `ACCESS_DENIED` | a network caller its `#[client]` rule refuses ([sync.md](sync.md#who-may-call-what)) |
| `ActionRefused` | `ACTION_REFUSED` | no such method |
| `ConnectionLost` | `CONNECTION_LOST` | the link to the app dropped; the call may have run |

Match on `code()`, never on the words.

### Panics

A method that panics is refused too: the copy is dropped, the caller gets
`<cell>.<method> panicked: <message>`, and the panic is reported as a
failure. After three panics within a minute the cell takes no more calls
until the app restarts, so a bug can't spin.

```rust
use aio::prelude::*;

#[cell]
pub struct Ratio {
    pub n: i64,
}

#[methods]
impl Ratio {
    pub fn divide(&mut self, by: i64) {
        self.n = 100 / by; // panics on 0
    }
}

# fn main() {
let h = Handle::<Ratio>::memory();
h.divide(4).unwrap();
let e = h.divide(0).unwrap_err();
assert!(e.why.starts_with("ratio.divide panicked"), "{}", e.why);
assert_eq!(h.state().n, 25, "the state stays as it was");
_ = h.divide(0);
_ = h.divide(0);
assert!(h.divide(5).unwrap_err().why.contains("stopped after 3 panics"));
# }
```

Refuse with `ensure!` instead of panicking whenever the input can be bad.

## `cx`: the outside world, recorded

A method does no I/O and reads no clock ([cells.md](cells.md#methods-do-no-io)).
What it needs from outside comes through `cx: &Cx`, the first parameter
after `&mut self`. Each value is saved with the event, so replay computes
the same state.

| `cx.` | Gives or does |
|---|---|
| `now()` | the call's time, ms since the Unix epoch: one value per call |
| `id()` | a new UUIDv7 (on a synced call, derived from the call's id, so client and server agree) |
| `rand()` | a random `u64` |
| `by()` | who called: `By::User`, `Agent`, `Schedule`, `Effect`, `System` or `Test` |
| `role()` | the signed-in caller's role, read from the account ([auth](../auth/README.md)) |
| `run(effect)` | start an effect after the call commits ([effects.md](effects.md)) |
| `abort(key)` | stop the running effects with that key after the call commits |
| `tell(t)` | a notification or badge after the call commits ([effects.md](effects.md#telling-people)) |
| `after`, `every`, `at`, `cron`, `backoff`, `cancel` | change the cell's schedules ([scheduling.md](scheduling.md)) |

What `cx` asks for (effects, tells, schedule changes) happens only when
the call commits: a refused call starts nothing.

## Calling a method

| From | How | A refusal |
|---|---|---|
| Rust with a door (`Handle`, synced copy, test client) | `h.add(..)`, through `<Name>Calls` | `Err(Rejected)` |
| Rust, a refusal you only log | `call!(h, add(..))` | logged; `None` |
| a view | `act!(add(..))` ([views](../ui/views.md)) | logged; `None` |
| Rust, by name | `h.call("add", json!([..]))` | `Err(Rejected)` |
| the terminal | `ax call <app> <cell> <method> [arg…]` | printed; exit 1 |
| another app | its sync link or `AioLink` ([plugins.md](plugins.md)) | `Err(Rejected)` |

- The call returns once the call has committed (or was refused): what it
  returns is computed on the state it made. `h.state()` right after shows
  that state.
- `h.by(By::User("ana".into()))` is the same handle, its calls recorded as
  Ana's; `cx.by()` sees it.
- Import the trait where you call: `use crate::contacts::ContactsCalls;`.
- `ax call notes notes add "buy milk"` gives each argument as JSON or as
  text; `--args '["buy milk"]'` gives the array exactly.

## One method calling another

Inside a method, another method is a plain Rust call on `self`. It runs on
the same copy, in the same call: one event, all or nothing.

```rust
use aio::prelude::*;

#[cell]
pub struct Basket {
    pub items: Vec<String>,
}

#[methods]
impl Basket {
    pub fn put(&mut self, item: String) -> Result {
        ensure!(self.items.len() < 3, "the basket is full");
        self.items.push(item);
        Ok(())
    }

    /// Calls `put` for each: one event, and a refusal undoes the earlier puts.
    pub fn put_all(&mut self, items: Vec<String>) -> Result {
        for i in items {
            self.put(i)?;
        }
        Ok(())
    }

    /// A helper: not `pub`, so not a method, not logged.
    fn tidy(&mut self) {
        self.items.sort();
    }

    pub fn put_sorted(&mut self, items: Vec<String>) -> Result {
        self.put_all(items)?;
        self.tidy();
        Ok(())
    }
}

# fn main() {
let h = Handle::<Basket>::memory();
h.put_sorted(vec!["pear".into(), "fig".into()]).unwrap();
assert_eq!(h.state().items, ["fig", "pear"]);
assert!(h.put_all(vec!["kiwi".into(), "plum".into()]).is_err(), "the fourth doesn't fit");
assert_eq!(h.state().items.len(), 2, "kiwi went with the refusal");
let events = h.store(|s| s.events().unwrap());
assert_eq!(events.len(), 1, "put_sorted is one event: its puts are inside it");
assert_eq!(events[0].method, "put_sorted");
# }
```

A method never calls another cell, nor its own handle: it has no handle,
and one reached through a `static` would deadlock on its own cell and
call again on every replay. Cells meet around methods
([composition.md](composition.md)).

## Atomic calls

One call is one transaction:

1. `aio::before_call` and the state machine may refuse it
   ([cells.md](cells.md#one-gate-for-every-call)).
2. The method runs on a copy of the state.
3. Every `#[invariant]` is checked on the copy.
4. The copy becomes the state and the event is written, in one step;
   listeners, views and synced clients are told after.

Calls to one cell run one at a time: a second caller waits for the first
to commit. Readers never wait: `h.state()` is the last committed state,
an `Arc` that stays valid. Calls to different cells run in parallel.

```rust
use aio::prelude::*;

#[cell]
pub struct Hits {
    pub n: u64,
}

#[methods]
impl Hits {
    pub fn hit(&mut self) {
        self.n += 1;
    }
}

# fn main() {
let h = Handle::<Hits>::memory();
let threads: Vec<_> = (0..4)
    .map(|_| {
        let h = h.clone();
        std::thread::spawn(move || (0..100).for_each(|_| h.hit().unwrap()))
    })
    .collect();
threads.into_iter().for_each(|t| t.join().unwrap());
assert_eq!(h.state().n, 400, "no call lost another's write");
# }
```

Since a method holds its cell while it runs, keep it short. `[perf]` in
`aio.toml` sets a budget for a method's own time (16 ms in a debug build,
100 ms in a release build; `[perf.methods] "cell.method" = ms` for one
meant to be slow): a call over it is logged with where the time went
([performance](../performance.md)). A method called 200 times a second for
5 seconds is a call storm (a loop that reacts to its own change): it is
said in the log, and `[perf.storm] breaker = true` refuses its calls while
it lasts.

## Async methods and atomicity

A `pub async fn` is split at each await, wherever it stands
([effects.md](effects.md#async-methods)): the part before the first await
is one call, and each part after an await is another call, made when the
awaited value arrives. Its caller awaits the end: `h.sell(1)?.await` is
the value the last part returned, or the refusal that ended it. Each part is atomic; the method as a whole is not.
Between two parts other calls may commit, and the next part sees the state
they left (aio's live reads; aio-rs has no snapshot mode).

Take what you need before the first await, so nothing else can take it
meanwhile, and give it back if the await fails:

```rust
use aio::prelude::*;
use aio::testing::TestCells;

#[cell(persist, restore = release)]
pub struct Stock {
    pub free: u32,
    pub held: u32,
    pub sold: u32,
}

/// On every boot: a sale cut off by a restart holds nothing.
fn release(mut s: Stock) -> Stock {
    s.free += s.held;
    s.held = 0;
    s
}

/// Stands in for a payment service.
async fn charge(n: u32) -> bool {
    n < 5
}

#[methods]
impl Stock {
    pub fn restock(&mut self, n: u32) {
        self.free += n;
    }

    pub async fn sell(&mut self, n: u32) -> Result {
        ensure!(self.free >= n, "only {} left", self.free);
        self.free -= n; // held from here: no other sale can take it
        self.held += n;
        let paid: bool = charge(n).await;
        self.held -= n;
        if paid { self.sold += n } else { self.free += n }
        Ok(())
    }
}

# fn main() {
let mut t = TestCells::new();
let h = t.cell::<Stock>();
h.restock(6).unwrap();
_ = h.sell(1).unwrap(); // its first part committed; the charge waits
assert_eq!((h.state().free, h.state().held), (5, 1));
assert_eq!(h.sell(6).unwrap_err().why, "only 5 left", "the held one is not for sale");
_ = h.sell(5).unwrap(); // declined below
t.settle();
assert_eq!((h.state().free, h.state().held, h.state().sold), (5, 0, 1));
# }
```

- The call returns once the first part commits, with an `aio::Answer`:
  await it for the end. Count calls in flight with
  `h.pending(Some("sell"))`, and in a view with `use_pending`.
- Overlapping calls of one async method run their awaits in turn, the
  second after the first; `#[mode(newest)]` lets a new call supersede
  the running one, `#[mode(first)]` shares the running one's answer, and
  `#[ttl(ms)]` keys them by the arguments.
- A refusal in a later part refuses that part only. What earlier parts
  committed stays, so a later part must leave the state whole on its
  own (the `free += n` above).
- Each await may take the call ceiling: 30 s, or `[perf] call_ceiling`.
  One that takes longer is cut off, said in the log and a report, its
  caller is told, and its later parts never run: undo what the first part
  held in `restore` or with a schedule. `#[long]` lifts the ceiling for a
  method that waits by nature.
- `cx.abort("sell")` from another method (a cancel button) stops the
  waiting awaits; their later parts never run, so that method gives back
  what they held.
- Network I/O goes in the awaited future, never in a part: a part is a
  method and does no I/O.

## Work across cells

aio-rs has no transaction over two cells: each cell has its own lock and
its own log, and a call commits in one cell only. When values must change
together (a balance and the ledger that explains it), keep them in one
cell; that is the only all-or-nothing aio-rs offers. When the cells must
stay apart, chain the calls and make each step undoable:

- One cell's commit triggers the other's call with `listens_to`
  ([composition.md](composition.md#observe-react-to-another-cell)). The
  reaction runs only after the source committed, and a refused reaction
  never undoes the source: refuse in the source, not the reaction.
- An effect calls the second cell and, if it refuses, calls a
  compensating method on the first (give the reservation back).

## Logging and replay

- Each committed call is one event: `seq`, `method`, `args`, the `cx`
  inputs, `by`, `at` and the cell version. A refused call writes nothing.
  `ax events <app> [cell]` lists them; `--diff` shows what each changed.
- An async method's parts are logged as `sell`, `sell.1`, `sell.2`…, each
  with what its await returned, so replay does no I/O.
- `#[redact]` keeps a method's arguments out of the log (a passphrase):
  its event keeps the resulting state instead.
- `#[coalesce]` (or the cell flag `coalesce`) shows each call at once and
  writes a run of them as one event ([performance](../performance.md)).
- Replay calls each method again with its recorded arguments and inputs.
  Renaming a method that has run breaks replay of its events: keep the
  old name, or migrate ([persistence](../persistence/README.md#changing-the-shape)).

## Differences from aio

| aio | aio-rs |
|---|---|
| sync and async methods on a draft (Immer) | `&mut self` on a copy; async methods split at their awaits (anywhere a statement goes) |
| `await cell.method()`: the async method's value or throw | `h.method(..)?.await`: an `aio::Answer` of its end |
| `throw` in a sync method discards, in an async one keeps | a refusal discards the part it is in; earlier parts of an async method stay |
| `s.$call(other, …)` | `self.other(..)`: a plain call, inside the same event |
| `s.$do(effect)` | `cx.run(Effect::new(..))` |
| `args` schema | the argument types, then `ensure!` |
| `transaction: true` / `{ serialize: true }`, `s.$commit()`, `s.$live`, `TX_CONFLICT` | not available: each part of an async method is atomic and reads the live state; take what you need before the await |
| `cancelOn`, `s.$signal` | `cx.abort(key)`; an aborted await's later parts never run |
| `concurrency: "newest" / "first" / "queue"`, `ttl` | `#[mode(newest)]` / `#[mode(first)]` / the default; `#[ttl(ms)]`; `Mode` on an effect |
| `long: [...]`, `effectTimeoutMs` | `#[long]`; `[perf] call_ceiling` |
| `blocking(id, fn)`, `blocking.cancel(id)` | `aio::effect::blocking(key, f)`, `cancel_blocking(key)` |
| `cell.$pending()` | `h.pending(Some("m"))`, `use_pending` |
| `.type`, generated actions | the method's name; `Cell::METHODS`, `Cell::SIGNATURES` |
| a transaction across cells | not available ([above](#work-across-cells)) |

## Common mistakes

- Writing state, then refusing, and expecting the write to stay: a refusal
  drops the whole copy. Record and return `Ok` instead.
- Checking a condition before an await and trusting it after: another call
  may have changed it. Take the resource first, or check again.
- A long loop or a big clone in a method: every other caller of that cell
  waits. Move heavy work into an effect ([effects.md](effects.md#heavy-work-retries-and-timeouts)).
- `panic!` or `unwrap()` on input: a panic is a refusal plus a failure
  report, and three stop the cell. Use `ensure!`.
- A public `&mut self` helper you didn't mean as a method: it is callable
  by name, by `ax call` and (with `#[client]`) the network. Make it private.
- Calling a cell from a method through a `static`: a deadlock on its own
  cell, a second call on every replay.

## API

`cargo doc -p aio-rs --no-deps --open`: `aio::Cx`, `aio::By`,
`aio::Rejected`, `aio::Code`, `aio::ensure!`, `aio::reject!`, `aio::call!`,
`aio::act!`, `aio::Door`, `aio::Cell` (`METHODS`, `SIGNATURES`,
`REDACTED`, `COALESCED`), `aio::Event`, `aio::effect::CALL_CEILING`,
`aio::Handle::done`, `aio::Handle::pending`.

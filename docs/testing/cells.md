# Testing cells

A cell test calls the real methods of the real cell, in-process, on a
fresh store: the same invariants, effects, schedules, listeners and boot
rules the app runs. No server, no window, no mocks. This page covers
`Handle::<C>::memory()` for one cell's logic and `aio::testing::TestCells`
for effects, schedules, several cells, saved data and access rules.

| You test | Use |
|---|---|
| a method's logic, a refusal, an invariant | `Handle::<C>::memory()` |
| effects, async methods, timers | `TestCells` + `settle()` |
| schedules (`#[every]`, `cx.after`, cron) | `TestCells` + `advance(ms)` |
| several cells wired together, the app's boot order | `TestCells::start(Cells)` |
| data an older build saved, migrations | `TestCells::saved(version, json)` |
| `#[client]` rules and roles as a network caller meets them | `TestCells::as_client(&h, caller)` |
| random calls against the invariants | `TestCells::fuzz` |

Run them with `ax test <app> [filter]` or `cargo test`. `aio::testing`
ships with aio-rs: an app adds no dev-dependency for it. Each test gets a
throwaway world (see [the sandbox](README.md#every-test-runs-in-a-sandbox)).

## One cell: `Handle::memory()`

A memory handle is a live cell with no file. Its typed methods are the
same ones the app calls: each returns `aio::Result`, so a refusal is a
value to assert on.

```rust
use aio::prelude::*;

#[cell]
pub struct Contacts {
    pub names: Vec<String>,
}

#[methods]
impl Contacts {
    pub fn add(&mut self, name: String) -> Result {
        let name = name.trim().to_string();
        ensure!(!name.is_empty(), "a contact needs a name");
        ensure!(!self.names.contains(&name), "{name} is already a contact");
        self.names.push(name);
        Ok(())
    }

    pub fn remove(&mut self, name: String) {
        self.names.retain(|n| *n != name);
    }

    #[invariant]
    fn at_most_three(&self) -> Result {
        ensure!(self.names.len() <= 3, "at most three contacts");
        Ok(())
    }
}

# fn main() {
let h = Handle::<Contacts>::memory();
h.add("Ada".into()).unwrap();
assert_eq!(h.state().names, ["Ada"]);

// A refusal is a typed value: compare it whole, the wrong refusal can't pass.
assert_eq!(h.add("  ".into()), Err(Rejected::new("a contact needs a name")));
assert_eq!(h.add("Ada".into()).unwrap_err().why, "Ada is already a contact");
assert_eq!(h.state().names, ["Ada"], "a refused call changed nothing");

// An invariant refuses the call that would break it.
h.add("Bo".into()).unwrap();
h.add("Cy".into()).unwrap();
let full = h.add("Di".into()).unwrap_err();
assert!(full.why.contains("at most three contacts"), "{}", full.why);
assert_eq!(h.state().names.len(), 3);
# }
```

- `h.state()` is the committed state (an `Arc<C>`): read it after the
  call returns, there is nothing to wait for.
- Assert on the whole `Rejected` or its `why`: a bare `is_err()` passes
  for any refusal, including a typo's. Match `code` (`aio::Code`) for
  aio's own kinds (`AccessDenied`, `ActionRefused`, `ConnectionLost`),
  never the words of one.
- A refused call changed nothing, whatever it wrote before it refused:
  the method ran on a copy. Assert that too when the method writes
  before it checks.
- A memory handle runs effects on aio-rs's background runtime in real
  time. As soon as a test cares about an effect or a schedule, use
  `TestCells`.

## `TestCells`: effects, time, several cells

`TestCells::new()` makes a test world: a sandboxed data folder, a
single-threaded runtime on a paused clock, and a schedule clock that
starts at the wall time and then moves only when the test says. Each
cell booted on it (`t.cell::<C>()`) is a real `Handle<C>` whose calls are
`By::Test`.

| Method | Does |
|---|---|
| `cell::<C>()` | boot `C` at its default state |
| `seeded::<C>(json)` | boot `C` at its default with `json` merged in |
| `saved::<C>(version, json)` | boot `C` from data a build of `version` saved, as the app's boot reads it |
| `start(cells)` | start an `aio::Cells` list as the app does: order, `#[on_start]`, missed schedules |
| `effects()` | the effects asked for and not started yet, `Cell.key`, in order |
| `settle()` | start them and wait until they, and all they lead to, are done (5 s at most) |
| `advance(ms)` | move both clocks on, fire what is due, settle |
| `failures()` | take the failed schedules and effects so far |
| `as_client(&h, caller)` | `h` as a network caller reaches it (feature `server`) |
| `fuzz(&h, seed, n, &calls, holds)` | `n` seeded random calls, checked after each |

Free functions in `aio::testing`: `seeded::<C>(json)` (the state alone,
no handle), `expect(&h, what, |s| ..)`, `dump(&state, max)`,
`fails(|| ..)`, `totp_code(secret, at_ms)`.

### Seeded state

Start a test at the state under test instead of driving the cell there
through its methods:

```rust
use aio::prelude::*;
use aio::testing::{TestCells, expect, fails, seeded};
use serde_json::json;

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Server {
    pub url: String,
    pub retries: u32,
}

#[cell]
pub struct Notes {
    pub server: Server,
    pub list: Vec<String>,
    pub saving: bool,
}

#[methods]
impl Notes {
    pub fn add(&mut self, text: String) -> Result {
        ensure!(!self.saving, "wait for the save");
        self.list.push(text);
        Ok(())
    }
}

# fn main() {
let mut t = TestCells::new();
let h = t.seeded::<Notes>(json!({"saving": true, "server": {"url": "https://example.org"}}));
assert_eq!(h.state().server.retries, 0, "objects merge key by key: the rest stays default");
assert!(h.add("x".into()).is_err());

// A failed expectation prints the state, cut at 2000 chars.
expect(&h, "still saving", |s| s.saving && s.list.is_empty());

// A key the state doesn't have is refused, naming the ones it has.
let why = fails(|| _ = seeded::<Notes>(json!({"server": {"ur1": "x"}})));
assert_eq!(why, "seed `server.ur1` isn't part of Notes's state; it has: retries, url");
# }
```

- Objects merge key by key, all the way down; a list or a number
  replaces the default.
- A seed key the state lacks panics: a typo would otherwise pin nothing
  and the test would pass on the default state.
- `expect(&h, what, f)` fails with `what` and the state as JSON; prefer
  it to a bare `assert!` on a big state.
- `seeded_source::<C>(json)` is the same seed as a `client!` app's
  source, for a UI test ([Testing views](ui.md#seeded-state-and-the-cell-beside-the-page)).

### Effects: see them, then settle them

`TestCells` holds every effect until the test says so. A test first sees
what a call asked for, then lets it run:

```rust
use aio::prelude::*;
use aio::testing::{TestCells, expect};
use std::time::Duration;

#[cell]
pub struct Feed {
    pub url: String,
    pub items: Vec<String>,
    pub loading: bool,
}

#[methods]
impl Feed {
    pub fn refresh(&mut self, cx: &Cx) -> Result {
        ensure!(!self.url.is_empty(), "no url");
        self.loading = true;
        let url = self.url.clone();
        cx.run(Effect::new("fetch", move |h: Handle<Feed>| async move {
            aio::time::sleep(Duration::from_secs(10)).await; // a slow server
            _ = h.fetched(vec![format!("{url}/1")]);
        }));
        Ok(())
    }

    pub fn fetched(&mut self, items: Vec<String>) {
        self.items = items;
        self.loading = false;
    }
}

# fn main() {
let mut t = TestCells::new();
let feed = t.seeded::<Feed>(serde_json::json!({"url": "https://example.org"}));
feed.refresh().unwrap();
assert_eq!(t.effects(), ["Feed.fetch"], "asked for, not started");
expect(&feed, "loading while the fetch is out", |s| s.loading);

let real = std::time::Instant::now();
t.settle(); // the 10 s sleep passes on the virtual clock, at once
assert!(real.elapsed() < Duration::from_secs(5));
expect(&feed, "fetched", |s| s.items == ["https://example.org/1"] && !s.loading);

let empty = t.cell::<Feed>();
assert!(empty.refresh().is_err());
assert!(t.effects().is_empty(), "a refused call asks for nothing");
# }
```

- `effects()` lists every cell's waiting effects as `Cell.key`. It is the
  assertion for "this call asked for a fetch" without running the fetch.
- `settle()` starts them, waits until they and every effect they lead to
  are done, and fails after 5 s naming what still runs
  (`still running after 5s: Feed.hang`). It never guesses: "quiet" and
  "gave up" are different answers.
- Effects' timers run on virtual time: `aio::time::sleep`,
  `tokio::time::timeout`, `interval` and `Effect::timeout` jump to the
  next due timer whenever nothing else can run. A 10 s backoff settles at
  once, timers fire in their order.
- An async method (`pub async fn`) is a method plus effects: call it,
  then `settle()`, then assert ([Effects](../state/effects.md#async-methods)).
- An effect that waits on real I/O (a socket, a process) under a timeout
  belongs in a [`TestServer` test](multi-app.md): on the paused clock its
  timeout would pass before the I/O answers.

### Time and schedules: `advance`

Nothing in a `TestCells` world moves with the wall clock. `advance(ms)`
moves the schedule clock and the effects' clock together, fires every
schedule then due, and settles.

```rust
use aio::prelude::*;
use aio::testing::{TestCells, expect};

#[cell]
pub struct Toast {
    pub text: Option<String>,
    pub at: i64,
    pub polls: u32,
}

#[methods]
impl Toast {
    pub fn show(&mut self, cx: &Cx, text: String) -> Result {
        self.text = Some(text);
        self.at = cx.now();
        cx.after("hide", 3_000, "hide", ())
    }

    pub fn hide(&mut self) {
        self.text = None;
    }

    #[every("1m")]
    pub fn poll(&mut self) {
        self.polls += 1;
    }
}

# fn main() {
let mut t = TestCells::new();
let h = t.cell::<Toast>();
h.show("saved".into()).unwrap();
let shown = h.state().at;

std::thread::sleep(std::time::Duration::from_millis(20));
t.advance(2_999);
expect(&h, "real time moved nothing; not due yet", |s| s.text.is_some());
t.advance(1);
expect(&h, "hidden on time", |s| s.text.is_none());

// The calls' clock (`cx.now()`) is the same clock.
h.show("again".into()).unwrap();
assert_eq!(h.state().at, shown + 3_000);

// A repeat missed more than once fires once, as after a sleep.
t.advance(10 * 60_000);
assert_eq!(h.state().polls, 1);
t.advance(60_000);
assert_eq!(h.state().polls, 2);
# }
```

- `cx.now()` in a method reads the schedule clock, so a stamp written
  after an `advance` is `start + advanced`.
- A schedule's call is `By::Schedule(key)` in the log; a declared
  `#[every]` runs under the key `@<method>`.
- A negative `advance` moves the schedule clock back (to test a clock
  change); tokio's clock never runs backwards.
- Cron schedules (`#[cron("0 9 * * 1")]`, `cx.cron`) are UTC (local
  time with `[app] tz = "local"`): advance from a known seed time, or
  test the next run with `aio::cron` directly.

### Failures nobody looked at

A call the test makes returns its refusal to the test. A call a schedule
or an effect makes has nobody to return to, so `TestCells` keeps it:

```rust
use aio::prelude::*;
use aio::testing::{TestCells, fails};

#[cell]
pub struct Job {
    pub url: String,
    pub runs: u32,
}

#[methods]
impl Job {
    pub fn run(&mut self) -> Result {
        ensure!(!self.url.is_empty(), "no url");
        self.runs += 1;
        Ok(())
    }

    pub fn later(&mut self, cx: &Cx) -> Result {
        cx.after("again", 10, "run", ())
    }

    pub fn retry(&mut self, cx: &Cx) {
        cx.run(Effect::new("retry", |h: Handle<Job>| async move { _ = h.run() }));
    }
}

# fn main() {
let mut t = TestCells::new();
let s = t.cell::<Job>();
s.retry().unwrap();
s.later().unwrap();
t.advance(10);
assert_eq!(t.failures(), ["Job.run: no url (schedule `again`)", "Job.run: no url (effect `retry`)"]);
assert!(t.failures().is_empty(), "taken");

// Not taken, it fails the test when the world drops.
let why = fails(|| {
    let mut t = TestCells::new();
    t.cell::<Job>().retry().unwrap();
    t.settle();
});
assert!(why.starts_with("failed and never looked at (take them with `failures`): Job.run: no url"), "{why}");
# }
```

- A refusal or a panic in a schedule's or an effect's call is a failure;
  `failures()` takes them (`Cell.method: why (schedule `key`)` or
  `(effect `key`)`), so the end of the test doesn't fail on them.
- Nothing fails silently: a fire-and-forget call cannot turn a red test
  green.

### Several cells and the app's boot

Cells booted on the same `TestCells` share its clocks. Wire them as the
app does (`listen`, `listens_to`, an effect that calls the other cell),
and start the app's own `aio::Cells` list to get its boot:

```rust
use aio::prelude::*;
use aio::Cells;
use aio::testing::TestCells;

#[cell]
pub struct Orders {
    pub opened: u32,
    pub list: Vec<String>,
}

#[methods]
impl Orders {
    #[on_start]
    pub fn open(&mut self) {
        self.opened += 1;
    }

    pub fn place(&mut self, item: String) {
        self.list.push(item);
    }
}

#[cell]
pub struct Stock {
    pub left: u32,
}

#[methods]
impl Stock {
    pub fn take(&mut self) -> Result {
        ensure!(self.left > 0, "sold out");
        self.left -= 1;
        Ok(())
    }
}

# fn main() {
let mut t = TestCells::new();
let (orders, stock) = (t.cell::<Orders>(), t.seeded::<Stock>(serde_json::json!({"left": 1})));
stock.listens_to(&orders, "place", |stock, _event| stock.take());
assert_eq!(orders.state().opened, 1, "#[on_start] ran as the cell went live");

let cells = t.start(Cells::default().with(stock.clone(), &[Orders::ID]).with(orders.clone(), &[])).unwrap();
assert_eq!(cells.ids(), ["orders", "stock"], "dependency order, not list order");
assert_eq!(orders.state().opened, 1, "live already: not run again");

orders.place("tea".into()).unwrap();
assert_eq!(stock.state().left, 0);
orders.place("cake".into()).unwrap(); // Stock::take refuses: sold out
assert_eq!((orders.state().list.len(), stock.state().left), (2, 0), "the order stays");

// The app's boot refusals are the test's.
let twice = Cells::default().with(t.cell::<Stock>(), &[]).with(t.cell::<Stock>(), &[]);
let dup = t.start(twice).err().unwrap();
assert!(dup.0.contains("duplicate cell name `stock`"), "{}", dup.0);
let round = Cells::default().with(t.cell::<Orders>(), &[Stock::ID]).with(t.cell::<Stock>(), &[Orders::ID]);
assert_eq!(t.start(round).err().unwrap().0, "dependency cycle: orders → stock → orders");

cells.disable(Orders::ID).unwrap();
assert!(orders.place("x".into()).unwrap_err().why.contains("is disabled"));
# }
```

- `t.cell::<C>()` boots the cell live: its `#[on_start]` runs once and
  its declared schedules are armed, fired by `advance`.
- `start(cells)` refuses a duplicate id, a dependency on an unknown cell
  (`depends on unknown cell `x``) and a cycle, as the app's boot does;
  otherwise each cell, in dependency order, fires the schedules it missed
  and starts its scheduler, on the test's clock. Use it to test the app's
  own list, its order and its refusals.
- A listener's refusal (`Stock::take` when sold out) refuses only the
  listener's call; the call it heard stays.

### Saved data and migrations

`saved::<C>(version, json)` boots from data an older build saved, as the
app's boot reads its file:

```rust
use aio::prelude::*;
use aio::testing::TestCells;
use serde_json::json;

/// Version 1 saved `{"name": …}`.
fn up(old: serde_json::Value, from: u32) -> std::result::Result<Todo, String> {
    assert_eq!(from, 1);
    Ok(Todo { title: old["name"].as_str().ok_or("no name")?.into(), ..Todo::default() })
}

/// A task that was running when the app stopped isn't running after a boot.
fn repair(s: Todo) -> Todo {
    Todo { running: false, ..s }
}

#[cell(persist, version = 2, migrate = up, restore = repair)]
pub struct Todo {
    pub title: String,
    pub running: bool,
    #[transient]
    pub hover: bool,
}

#[methods]
impl Todo {}

# fn main() {
let mut t = TestCells::new();
let now = t.saved::<Todo>(2, json!({"title": "x", "running": true, "hover": true})).unwrap();
assert_eq!(*now.state(), Todo { title: "x".into(), ..Todo::default() }, "restored, transient reset");
assert_eq!(t.saved::<Todo>(1, json!({"name": "old"})).unwrap().state().title, "old", "migrated");

let refused = |t: &mut TestCells, v, data| t.saved::<Todo>(v, data).err().unwrap().0;
assert!(refused(&mut t, 1, json!({})).contains("migrate from v1 failed: no name"));
assert!(refused(&mut t, 3, json!({})).contains("data from a newer build"));
assert!(refused(&mut t, 2, json!({"title": "x", "owner": "ann"})).contains("fields this build doesn't declare (owner)"));
# }
```

Keep a real file an old build wrote as a fixture
(`include_str!("fixtures/todo-version-1.json")`) and boot it: that is the test
that a release can open last release's data. `ax migrations <app>` shows
the drift on a running app ([Data migration](../migration/data.md)).

### Access rules as a network caller

A `Handle`'s calls are the app's own, so `#[client]` rules don't apply to
them. `t.as_client(&h, caller)` (feature `server`) makes each call as a
network caller, asking the rule first, as the app's server does:

```rust
use aio::prelude::*;
use aio::testing::{TestCells, expect};

#[cell]
pub struct Board {
    pub posts: Vec<String>,
}

#[methods]
impl Board {
    /// The author is who calls, never an argument a caller could forge.
    #[client]
    pub fn post(&mut self, cx: &Cx, text: String) {
        let by = match cx.by() {
            By::User(u) => u.clone(),
            _ => "anyone".into(),
        };
        self.posts.push(format!("{by}: {text}"));
    }

    #[client(role = "admin")]
    pub fn clear(&mut self) {
        self.posts.clear();
    }

    pub fn pin(&mut self) {}
}

# fn main() {
let mut t = TestCells::new();
let b = t.cell::<Board>();
let ana = t.as_client(&b, Caller { by: By::User("ana".into()), role: Some("member".into()) });
let root = t.as_client(&b, Caller { by: By::User("root".into()), role: Some("admin".into()) });

ana.post("hi".into()).unwrap();
expect(&b, "the method saw ana", |s| s.posts == ["ana: hi"]);
let no = ana.clear().unwrap_err();
assert!(no.why.contains("needs the role admin"), "{}", no.why);
assert_eq!(no.code, Some(Code::AccessDenied));
assert!(ana.pin().unwrap_err().why.contains("is not a #[client] method"), "the server's own");
root.clear().unwrap();

// The key alone proves no one in particular.
t.as_client(&b, Caller::default()).post("x".into()).unwrap();
expect(&b, "anonymous", |s| s.posts == ["anyone: x"]);
# }
```

`cx.by()` and `cx.role()` in the method see the caller. The rule runs on
the state the call then runs on. For the real wire, keys, cookies and
sign-in, use [`TestServer`](multi-app.md).

## Fuzzing: seeded random calls

`fuzz` makes `n` random calls from a list, each on the state the last one
left, then settles the effects. It fails on a panic, or a state your
check refuses, naming the seed and every call up to it:

```rust
use aio::prelude::*;
use aio::testing::{RandomCall, Rng, TestCells, fails};
use serde_json::{Value, json};

#[cell]
pub struct Account {
    pub balance: u64,
    pub moves: u32,
}

#[methods]
impl Account {
    #[invariant]
    fn capped(&self) -> Result {
        ensure!(self.balance <= 1000, "over the cap");
        Ok(())
    }

    pub fn deposit(&mut self, n: u64) {
        self.balance += n;
        self.moves += 1;
    }

    pub fn withdraw(&mut self, n: u64) -> Result {
        ensure!(n <= self.balance, "not enough");
        self.balance -= n;
        self.moves += 1;
        Ok(())
    }

    /// A bug a fuzzer finds: the 13th move panics.
    pub fn audit(&mut self) {
        assert!(self.moves != 13, "unlucky audit");
    }
}

fn amount(r: &mut Rng) -> Value {
    json!([r.below(300)])
}

# fn main() {
let money: [RandomCall; 2] = [("deposit", &amount), ("withdraw", &amount)];
let mut t = TestCells::new();
let a = t.cell::<Account>();
let run = t.fuzz(&a, 42, 200, &money, |s| s.balance <= 1000);
assert_eq!(run.len(), 200);
assert!(run[0].starts_with("deposit([") || run[0].starts_with("withdraw(["));

// The same seed is the same run, on every machine.
let mut again = TestCells::new();
let b = again.cell::<Account>();
assert_eq!(again.fuzz(&b, 42, 200, &money, |_| true), run);

// A panic names the seed: re-run that one seed to debug it.
let calls: [RandomCall; 2] = [("deposit", &|_| json!([1])), ("audit", &|_| Value::Null)];
let why = fails(|| {
    let mut t = TestCells::new();
    let a = t.cell::<Account>();
    t.fuzz(&a, 7, 500, &calls, |_| true);
});
assert!(why.starts_with("fuzz seed 7: account.audit panicked: unlucky audit"), "{why}");
# }
```

- `RandomCall` is `(method, &dyn Fn(&mut Rng) -> Value)`: the arguments
  as a JSON array (`json!([..])`, `Value::Null` for none).
- `Rng` is SplitMix64: `number()`, `below(n)`. The same seed gives the
  same numbers everywhere, so a failing seed from CI re-runs at home.
- A refusal is an answer, not a failure: the invariants refuse what would
  break them, and the fuzz goes on. A panic in a method or an effect, or
  `holds` returning false (after any call or after the final settle),
  fails.
- The list is the allowlist: leave out the methods a fuzz can't call
  (one that needs a real device). There is no `skip`.
- `fuzz` returns the calls it made (`withdraw([12])`, …): assert on them,
  or print them while you write the test.
- Pick the seed: there is no generated one. Several seeds in a loop cover
  more ground; each failure still names its own.

## Testing a failure on purpose: `fails`

`aio::testing::fails(|| ..)` runs a closure that must panic and returns
the message (it fails if the closure passes). Use it for a harness's own
refusals (`settle`, `seeded`, `fuzz`, a `TestUi` lookup) where
`#[should_panic(expected = ..)]` would end the test. A failure taken by
`fails` leaves no UI trace behind; one under `#[should_panic]` keeps it.

## Logging in with a second factor

`totp_code(secret, at_ms)` is the code an authenticator app shows for a
base32 secret at a time: the test signs in with a second factor the way a
person would ([Auth](../auth/README.md)).

## Tests from a running app

`ax record <app> <cell> <file> [range]` writes a `TestCells` test from
what the running app's cell just did: seeded on the state before the
range, its calls made again, the state they left asserted. `--from file`
reads another store or a backup; `--type path` names the cell's type
when it isn't `<crate>::<Cell>`. The result is an ordinary test to edit.

`ax expect <app> <cell> <path> eq <value> [--wait 5]` asserts on the live
state from a script (exit 1 when it fails; ops `eq ne gt gte lt lte
contains exists absent`), and `ax call <app> <cell> <method> [arg…]`
drives it ([Debugging](../debugging/README.md)).

## What aio has that aio-rs does without

| aio | aio-rs |
|---|---|
| `t.init(seed)` mid-test, `t.destroy()` | boot another cell on the same `TestCells` (`seeded`) |
| `t.expect.effects([..])` per last action | `effects()`: every waiting effect of every cell |
| `t.fuzz` with a generated seed, `skip` | an explicit seed; the call list is the allowlist |
| `t.randomActions(n)` (unseeded) | none: every fuzz is seeded |
| `t.settle(ms)` for real timers | none: timers are virtual; real I/O goes to `TestServer` |
| `perfBudget` call ceilings in `bootCells` | `aio::effect::CALL_CEILING` and `Effect::timeout` apply as in the app |
| `testCell` skips `onInit` (a warning) | every booted cell runs `#[on_start]`, as the app does |

## Common mistakes

- `std::thread::sleep` to wait for an effect or a schedule: nothing moves
  with the wall clock. Use `settle()` or `advance(ms)`.
- `assert!(h.add(..).is_err())`: passes for the wrong refusal. Compare
  the `Rejected` or its `why`.
- Driving the cell through ten calls to reach a state: seed it.
- A `Handle::memory()` test of an effect: it runs on the real clock in
  the background; use `TestCells`.
- Counting on `#[on_start]` not having run: `t.cell()` boots the cell
  live, so its start hook already ran; seed past it if it matters.
- Ignoring `failures()` and then wondering why the test fails at its end:
  take them and assert on them.
- Reading fixtures from the source folder at run time: use
  `include_str!` so the test runs from any folder.
- A real network call inside an effect under `TestCells`: put it behind a
  [cassette](cassettes.md) or test it with `TestServer`.

## API

`cargo doc -p aio-rs --no-deps --all-features --open`:
`aio::testing::TestCells`, `AsClient`, `RandomCall`, `Rng`, `seeded`,
`seeded_source`, `expect`, `dump`, `fails`, `totp_code`; `aio::Handle`
(`memory`, `state`, `listen`, `listens_to`, `done`, `until`, `pending`),
`aio::Cells`, `aio::Rejected`, `aio::Code`, `aio::Caller`.

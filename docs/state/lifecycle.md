# Lifecycle

How a cell comes up, runs, is switched off and on, and goes down with the
app; how the app stops; and the hooks that see every call. Every example
compiles and runs as a test of aio-rs.

## The life of a cell

| Stage | What happens |
|---|---|
| open | `Handle::open(path)` (or `Source::open`, or `aio::client!`'s store) reads the cell from `state.db`: migrated, `#[transient]` fields reset, `restore = f` run ([persistence](../persistence/README.md)) |
| live | `#[on_start]` runs; the cell's `#[every]`/`#[cron]` schedules are declared; schedules missed while the app was off fire once |
| running | calls commit one at a time; effects and schedules run around them |
| disabled | calls refused, schedules paused, effects aborted, owned resources dropped; the state kept |
| stopping | `#[on_stop]` runs; new calls refused with "retry after"; running effects finish and commit, until the deadline |
| closed | the log is flushed and the file closed |

A cell opened by `Source::open` (what `aio::client!` does for the app's
window) starts on its own and is registered for the app's shutdown. A
plain `Handle` goes live when `h.start()` runs (inside a tokio runtime)
or when an `aio::Cells` list starts it.

## `#[on_start]` and `#[on_stop]`

```rust
use aio::prelude::*;
use aio::Cells;
use aio::testing::TestCells;

#[cell(persist, restore = idle)]
pub struct Feed {
    pub starts: u32,
    pub stops: u32,
    pub syncing: bool,
    pub fetched: u32,
}

/// Every boot: a sync cut off by the last stop isn't running any more.
fn idle(mut f: Feed) -> Feed {
    f.syncing = false;
    f
}

#[methods]
impl Feed {
    /// Work at boot goes through a method: it is in the log, replayable.
    #[on_start]
    pub fn up(&mut self, cx: &Cx) -> Result {
        self.starts += 1;
        cx.after("first-sync", 1_000, "sync", ())
    }

    #[on_stop]
    pub fn down(&mut self) {
        self.stops += 1;
    }

    pub fn sync(&mut self) {
        self.fetched += 1;
    }
}

# fn main() {
let mut t = TestCells::new();
let feed = t.cell::<Feed>(); // live, as the app boots it: `up` ran
let cells = t.start(Cells::default().with(feed.clone(), &[])).unwrap();
assert_eq!(feed.state().starts, 1);
t.advance(1_000);
assert_eq!(feed.state().fetched, 1, "the sync up asked for");

cells.disable(Feed::ID).unwrap(); // `down` runs
assert_eq!(feed.state().stops, 1);
assert_eq!(feed.sync().unwrap_err().why, "feed is disabled");
assert_eq!(cells.status(Feed::ID).as_deref(), Some("disabled"));
cells.enable(Feed::ID).unwrap(); // `up` again
assert_eq!(feed.state().starts, 2);
# }
```

- Each hook is an ordinary method: `pub fn (&mut self)`, a `cx: &Cx` may
  follow, it may return `Result`. A cell has at most one of each; a second,
  or one with arguments, or an `async` one, fails the build.
- They run once each way: as the cell goes live (boot, enable) and as it
  stops (shutdown, disable). They are logged like any call.
- A hook that refuses or panics is said in the log and a problem report
  (`hook`); it never stops the boot or the shutdown.
- Start work from `#[on_start]` with `cx.run(..)` (an effect) or
  `cx.after(..)` (a call soon). Repeating work is a schedule
  ([scheduling.md](scheduling.md)). Never spawn a thread or a timer that
  calls the cell from outside: it escapes the log and replay.
- `#[on_stop]` runs while the cell still commits: it may change the state,
  and that change is saved. It must not wait on I/O: stop what you own
  with an effect earlier, or let [owned resources](#owned-resources) drop.
- State that must not survive a restart (a busy flag, a lock) is cleaned
  in `restore`, which runs on every boot, before `#[on_start]`.

## Many cells: `aio::Cells`

`aio::Cells` is the app's list of cells, each added with the ids of the
cells it starts after. `start()` checks the list, then starts each cell in
dependency order (otherwise in the order listed); `shutdown(budget)` stops
them in reverse, so a cell's effects can still reach the cells it depends
on.

```rust
use aio::prelude::*;
use aio::Cells;
use aio::testing::TestCells;

#[cell]
pub struct Accounts {
    pub n: u32,
}

#[cell]
pub struct Orders {
    pub n: u32,
}

#[cell]
pub struct Reports {
    pub n: u32,
}

#[methods]
impl Accounts {}

#[methods]
impl Orders {}

#[methods]
impl Reports {}

# fn main() {
let mut t = TestCells::new();
let (a, o, r) = (t.cell::<Accounts>(), t.cell::<Orders>(), t.cell::<Reports>());

let cells = Cells::default()
    .with(r.clone(), &[Accounts::ID, Orders::ID])
    .with(o.clone(), &[Accounts::ID])
    .with(a.clone(), &[]);
assert_eq!(t.start(cells).unwrap().ids(), ["accounts", "orders", "reports"]);

let unknown = Cells::default().with(o.clone(), &["billing"]);
assert_eq!(t.start(unknown).err().unwrap().0, "`orders` depends on unknown cell `billing`; known: orders");

let cycle = Cells::default().with(o.clone(), &[Reports::ID]).with(r.clone(), &[Orders::ID]);
assert_eq!(t.start(cycle).err().unwrap().0, "dependency cycle: orders → reports → orders");

let twice = Cells::default().with(a.clone(), &[]).with(a.clone(), &[]);
assert!(t.start(twice).err().unwrap().0.starts_with("duplicate cell name `accounts`"));
# }
```

A bad list refuses the boot: a duplicate id, a dependency the list
doesn't hold, a cycle. Dependencies order start and stop only; they give
a cell no access to another's state ([composition.md](composition.md)).

### App-wide hooks

The app's own start code goes on the list (aio's `aio.run({ onStart,
onRestore, fatalOnStart })`):

- `.on_restore(|cells| ..)` runs once at `start()`, after every cell's
  state is restored and before any goes live: repair across cells through
  their methods (what it calls is logged).
- `.on_start(|cells| ..)` runs once after every cell went live: seed data,
  start the app's own timers. A plugin's `on_start` runs after the app's.
- A refused hook, or a cell's refused `#[on_start]`, is said in the log
  and a report and the start goes on; `.fatal_on_start()` makes it fail
  `start()` instead, so the app exits rather than run half-started.
- `cells.get::<Orders>()` is a cell's handle; `cells.view()` every cell's
  state as JSON by id (aio's `getFullState()`), to read across cells.
- `AIORT_ISOLATE=orders,reports` (aio's `isolate`) starts only those
  cells: the others stay disabled, said in the log. A name the app has no
  cell of refuses the start.

```rust
use aio::prelude::*;
use aio::Cells;
use aio::testing::TestCells;

#[cell]
pub struct Plans {
    pub names: Vec<String>,
}

#[methods]
impl Plans {
    pub fn add(&mut self, name: String) {
        self.names.push(name);
    }
}

# fn main() {
let mut t = TestCells::new();
let plans = t.cell::<Plans>();
let cells = Cells::default().with(plans.clone(), &[]).on_start(|cells| {
    let p = cells.get::<Plans>().expect("listed");
    if p.state().names.is_empty() {
        p.add("free".into())?; // seed data, once
    }
    Ok(())
});
let cells = t.start(cells).unwrap();
assert_eq!(plans.state().names, ["free"]);
assert_eq!(cells.view()["plans"]["names"][0], "free");
# }
```

## Disable, enable and health

| `Cells::` | What it does |
|---|---|
| `disable(id)` | `#[on_stop]` runs; calls refused (`<id> is disabled`), schedules paused, running effects aborted, owned resources dropped; the state and the file stay |
| `enable(id)` | calls taken again, the failure count back to zero, `#[on_start]` runs, schedules missed meanwhile fire once |
| `enabled(id)` | is it taking calls? |
| `status(id)` | `disabled`, or the cell's own `status` field when it is a `String` |
| `health()` | a `CellHealth` per cell, in dependency order: `id`, `status`, `enabled`, `failures`, `last_call` (method, time) |
| `ids()` | the ids, in dependency order once started |

Unlike aio, disabling never resets the state: enabling brings the cell
back as it was. An unknown id is refused with the ids the list holds.
aio-rs has no `ax` command to disable a cell of a running app; the app's
own code calls `Cells`.

### The breaker

`Cells::trip_after(max, window)` disables a cell after `max` failures
(within `window`, if given). A failure is a panic in a method, or a
refused call made by the cell's own schedules and effects; a person's call
that is refused is an answer, not a failure. The trip keeps the data, is
said in the log, marks the app degraded in `ax health <app>` and writes a
problem report (`tripped`).

```rust
use aio::prelude::*;
use aio::Cells;
use aio::testing::TestCells;

#[cell]
pub struct Probe {
    pub up: u32,
}

#[methods]
impl Probe {
    pub fn ping(&mut self, cx: &Cx) {
        cx.run(Effect::new("ping", |h: Handle<Probe>| async move { _ = h.answer(false) }));
    }

    pub fn answer(&mut self, up: bool) -> Result {
        ensure!(up, "the service is down");
        self.up += 1;
        Ok(())
    }
}

# fn main() {
let mut t = TestCells::new();
let p = t.cell::<Probe>();
let cells = t.start(Cells::default().with(p.clone(), &[]).trip_after(2, None)).unwrap();
assert!(p.answer(false).is_err(), "a person's refusal: no failure");
p.ping().unwrap();
p.ping().unwrap();
t.settle();
assert_eq!(t.failures().len(), 2, "the effects' refusals, taken so the test may end");
let row = &cells.health()[0];
assert_eq!((row.enabled, row.failures), (false, 2), "tripped, its data kept");
cells.enable(Probe::ID).unwrap();
assert_eq!(cells.health()[0].failures, 0);
# }
```

Without `trip_after` there is no breaker. Two guards apply anyway: three
panics within a minute stop a cell until the app restarts
([methods.md](methods.md#panics)), and `[perf.storm] breaker = true` refuses
a method while it is called in a storm.

## The app's stop

`aio::shutdown(budget)` stops the whole app, in this order:

1. `aio::on_stopping(f)` hooks, in the order added, while methods still
   commit: stop the app's own producers (a poller, a queue) here, so
   nothing they write is refused later. A write made here is saved.
2. Every store the process opened with `Source::open`, all at once: each
   runs `#[on_stop]`, refuses new calls with "retry after
   `aio::RETRY_AFTER_MS`" (effects' own calls still commit), writes a held
   `#[coalesce]` run, stops its schedules and lets running effects finish.
   Those still running at the deadline are aborted and named.
3. `aio::on_stop(f)` hooks, once every store is closed: let go of what the
   app holds (a device, a lock). A cell call from here is refused: there is
   no store left to save it.
4. Every synced copy's last save (`Remote::connect_saved`) written to
   its file: a call made offline just before is kept. One not written by
   the deadline is named.

Stores, `on_stop` and the saves each get at least 500 ms, whatever `on_stopping`
used. A hook that panics is logged and the stop goes on; one still
running at its deadline is left behind and named. `shutdown` returns what
was cut. The app's launcher calls it for you, with a 5 s budget, once the
window closes or a stop is asked for.

```rust
use std::sync::Mutex;
use std::time::Duration;

static SAID: Mutex<Vec<&str>> = Mutex::new(Vec::new());

# fn main() {
aio::on_stopping(|| SAID.lock().unwrap().push("stop polling"));
aio::on_stop(|| SAID.lock().unwrap().push("release the device"));
let cut = aio::shutdown(Duration::from_secs(2));
assert!(cut.is_empty(), "nothing was still running");
assert_eq!(*SAID.lock().unwrap(), ["stop polling", "release the device"]);
# }
```

`Cells::shutdown(budget).await` does step 2 for a list, in reverse
dependency order, and `h.shutdown(budget).await` for one handle (it
returns `Drained`, the aborted effect keys).

### Asking a process to stop

| `aio::lifecycle::` | What it does |
|---|---|
| `stop_asked()` | a future that resolves on SIGTERM or SIGINT (Unix), Ctrl+C, a closed console, logoff or shutdown (Windows), or `ask_stop()`; from the call on, those no longer end the process at once |
| `ask_stop()` | stop this process the graceful way |
| `ask_restart()` | stop so the supervisor starts it again (an installed update); exits 0 |
| `exit_code()` | what a server's `main` returns: `STAYS_DOWN` (143) when its supervisor asked a stop that stays down, else 0 |
| `watched(cmd)`, `watch_parent()` | a child that stops itself, gracefully, when its parent goes, even on `kill -9` (aio's `AIO_PARENT_PID`) |

`ax stop <app>` asks a running app to close and waits until it is gone;
`ax dev` and the test harness start the app watched, so it never outlives
them. `ax status <app>` says `stopping` while the shutdown runs, and
`ax health <app>` says whether a running app is healthy.

## One gate for every call

`aio::before_call(f)` sees every call of every cell before it runs (aio's
`beforeReduce`). An `Err` refuses the call, and nothing runs or is saved;
a hook that panics refuses too. Replay never asks it: what it let in is in
the log.

```rust
use aio::prelude::*;

#[cell]
pub struct Admin {
    pub resets: u32,
}

#[methods]
impl Admin {
    pub fn admin_reset(&mut self) {
        self.resets += 1;
    }
}

# fn main() {
aio::before_call(|call| {
    if call.method.starts_with("admin_") && call.role != Some("admin") {
        reject!("{}.{} needs the admin role", call.cell, call.method);
    }
    Ok(())
});
let h = Handle::<Admin>::memory();
assert_eq!(h.admin_reset().unwrap_err().why, "admin.admin_reset needs the admin role");
assert_eq!(h.state().resets, 0);
# }
```

- `aio::Call` holds `cell`, `method`, `args`, `by`, `role` (the
  network caller's, proven by its session), the state the call runs on
  (`call.state::<T>()`, `None` for another cell's call) and, for a
  network call, `request` (aio's request in `beforeReduce`): the
  client's address (`ip`) and the headers its link opened with
  (`header("user-agent")`), its credentials (`authorization`, `cookie`)
  left out. Neither is recorded: replay never asks the gate.
- There is one gate: a second `before_call` replaces the first. It lets
  a call in or refuses it; `aio::rewrite_call(|call| ..)` may also change
  it (aio's `beforeReduce` returning another action): `Ok(Some(args))`
  runs it with those arguments, logged as it ran, so replay never asks
  again.
- It runs while the cell is locked: decide from the call, its state and
  its request, and never call a cell from it.
- `aio::on_effect(|cell, key| ..)` hears each effect as it starts (aio's
  `onEffect`); those hooks add up (a plugin's and the app's all hear).

## Owned resources

A cell may own what is not state: a socket, a file watcher, a device.
`h.own(key, Some(Box::new(res)))` (on any `Door`) keeps it under `key`,
dropping the one it replaces; `h.own(key, None)` drops it; `h.owns(key)`
asks. Everything a cell owns is dropped when it is disabled or stops, so
open it again from an effect that `#[on_start]` starts. For a resource
views hold by key (a camera while a page shows), `aio::held::Held` opens
one per key and closes it when the last holder lets go (`use_held` in a
view).

## Differences from aio

| aio | aio-rs |
|---|---|
| `onInit`, `onDestroy` | `#[on_start]`, `#[on_stop]` |
| `aio.run({ onStart })`, app-level `onRestore` | `Cells::on_start`, `Cells::on_restore`; `#[on_start]` on a cell |
| `fatalOnStart: true` | `Cells::fatal_on_start()` |
| `dependsOn` | `Cells::with(h, &[ids])` |
| `isolate` (run only some cells) | `AIORT_ISOLATE=a,b` |
| `onCheckpointRestore` | `#[cell(recovered = f)]` returns the state to boot on; `aio::on_recovered(|cell, recovered| ..)` hears it (a store back from a damaged file) |
| `app.cells.disable` resets the state | `Cells::disable` keeps it |
| `circuitBreaker` | `Cells::trip_after` |
| `GET /__aio/health` | `ax health`, `Cells::health()` |
| `onStopping`, `onStop` | `aio::on_stopping`, `aio::on_stop` |
| `AIO_PARENT_PID` | `aio::lifecycle::watched`, `watch_parent` |
| `beforeReduce` may change an action | `before_call` lets in or refuses; `rewrite_call` may change the arguments |
| `app.getFullState()` in a hook | `Cells::view()`, `Cells::get::<T>()` |

## Common mistakes

- Starting a thread or a timer in `main` that calls a cell: use
  `#[on_start]` with `cx.after`, or a schedule.
- Calling a cell from `aio::on_stop`: refused, the stores are closed. Do it
  in `on_stopping` or `#[on_stop]`.
- A busy flag that stays set after a crash: clear it in `restore`.
- Expecting `Cells::disable` to start the cell over: it keeps the state.
- Listing a cell as a dependency to read its state: dependencies only
  order start and stop.

## API

`cargo doc -p aio-rs --no-deps --open`: `aio::Cells`, `aio::CellHealth`,
`aio::Handle::start`, `aio::Handle::shutdown`, `aio::Drained`,
`aio::Source::open`, `aio::shutdown`, `aio::on_stopping`, `aio::on_stop`,
`aio::RETRY_AFTER_MS`, `aio::lifecycle`, `aio::before_call`, `aio::Call`,
`aio::on_effect`, `aio::Door::own`, `aio::held::Held`.

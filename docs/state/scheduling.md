# Schedules, real time and heavy work

A schedule calls one of the cell's own methods later: once, again and
again, at a time, on a cron line, or after a growing wait. It is saved
with the event of the call that set it, so it survives a restart, and the
call it makes is an ordinary call: atomic, logged, replayed. This page
goes deeper than the short tour in [effects](effects.md#schedules): every
kind of schedule, keys, missed runs, testing, state that changes many
times a second, and work too heavy for a method.

| Need | Use |
|---|---|
| always on, every so often | [`#[every("5m")]`](#declared-schedules) on a method |
| always on, at set times | [`#[cron("0 9 * * 1")]`](#declared-schedules) |
| once, later (debounce, timeout, reminder) | [`cx.after`](#schedules-from-a-call) |
| again and again, from a call | [`cx.every`](#schedules-from-a-call) |
| at a given moment | [`cx.at`](#schedules-from-a-call) |
| a retry or poll that waits longer each time | [`cx.backoff`](#backoff-and-polling) |
| stop one | [`cx.cancel`](#schedules-from-a-call) |
| CPU-heavy work | [an effect, off the thread](#heavy-work-off-the-cells-thread) |
| something that moves 60 times a second | [the view, not a cell](#real-time-and-high-frequency-state) |

## Declared schedules

```rust
use aio::prelude::*;
use aio::testing::TestCells;

#[cell(persist)]
pub struct Reports {
    pub hourly: u32,
    pub weekly: u32,
}

#[methods]
impl Reports {
    /// Armed whenever the cell is live, under the key `@hourly`.
    #[every("1h")]
    pub fn hourly(&mut self) {
        self.hourly += 1;
    }

    /// Mondays at 9:00, this machine's local time.
    #[cron("0 9 * * mon")]
    pub fn weekly(&mut self, cx: &Cx) {
        let _ = cx.now(); // a declared method may take `cx`
        self.weekly += 1;
    }
}

# fn main() {
let mut t = TestCells::new();
let h = t.cell::<Reports>();
let mut keys = h.store(|s| s.schedules().unwrap().into_iter().map(|d| d.key).collect::<Vec<_>>());
keys.sort();
assert_eq!(keys, ["@hourly", "@weekly"]);
t.advance(3_600_000);
assert_eq!(h.state().hourly, 1);
t.advance(5 * 3_600_000); // five slots missed (the app was off, the laptop asleep)
assert_eq!(h.state().hourly, 2, "missed slots fire once, not five times");
# }
```

- `#[every(..)]` takes a span, a number and `d`, `h`, `m` or `s`
  (`"30s"`, `"5m"`, `"2h"`, `"1d"`), or plain milliseconds
  (`#[every(300_000)]`). It must be more than zero.
- `#[cron("…")]` takes a [cron line](#cron-lines).
- Either may add `, skip_if_running`: `#[every("1m", skip_if_running)]`.
  A tick is then skipped (and the schedule moved to its next slot) while
  the effect the last tick started, keyed by the method's name, still
  runs. That is what a `pub async fn` scheduled this way starts, so a slow
  sync never overlaps itself.
- The method is a `pub fn(&mut self)`, with `cx: &Cx` allowed, and no
  other arguments; anything else is a build error that says so.
- Declared schedules are armed each time the cell goes live: at start, and
  when it is enabled again. One with the same timing as the saved one
  keeps its next run; a changed timing starts over from now; one a new
  build no longer declares is removed. A cron line that doesn't parse or
  never matches is said in the log and a problem report, and the cell
  starts without it.

## Schedules from a call

A method asks for a schedule through its `cx`. Nothing is armed if the
call is refused, and on replay the request is dropped: the schedule was
saved when the event first ran.

```rust
use aio::prelude::*;
use aio::testing::TestCells;

#[cell(persist)]
pub struct Kitchen {
    pub rang: Vec<String>,
}

#[methods]
impl Kitchen {
    /// One timer per dish: the dish is in the key.
    pub fn timer(&mut self, cx: &Cx, dish: String, minutes: i64) -> Result {
        ensure!(minutes > 0, "how long?");
        cx.after(&format!("timer:{dish}"), minutes * 60_000, "ring", (dish,))
    }

    pub fn stop(&mut self, cx: &Cx, dish: String) {
        cx.cancel(&format!("timer:{dish}"));
    }

    pub fn ring(&mut self, dish: String) {
        self.rang.push(dish);
    }

    /// A misspelled method refuses the call that sets it.
    pub fn broken(&mut self, cx: &Cx) -> Result {
        cx.after("x", 1_000, "rnig", ())
    }
}

# fn main() {
let mut t = TestCells::new();
let h = t.cell::<Kitchen>();
h.timer("rice".into(), 12).unwrap();
h.timer("tea".into(), 4).unwrap();
h.timer("tea".into(), 3).unwrap(); // the same key: replaced, not added
h.stop("rice".into()).unwrap();
t.advance(15 * 60_000);
assert_eq!(h.state().rang, ["tea"]);
assert!(h.broken().is_err());
# }
```

| Call | Fires | Notes |
|---|---|---|
| `cx.after(key, ms, method, args)` | once, `ms` from the call | negative `ms` counts as 0 |
| `cx.every(key, ms, method, args)` | every `ms`, first `ms` from the call | `ms` must be more than 0, or the call is refused |
| `cx.at(key, epoch_ms, method, args)` | once, at that moment | a moment already past fires at once |
| `cx.cron(key, expr, method, args)` | on the cron line | a bad line, or one that never matches, refuses the call |
| `cx.backoff(key, attempt, b, method, args)` | once, after `b.ms(attempt)` | see [backoff](#backoff-and-polling) |
| `cx.cancel(key)` | | removes it; an unknown key is no error |

- `args` is anything serializable, sent as the method's argument list:
  `()` for none, `(dish,)` for one, `(a, b)` for two. It is saved now and
  passed then, so it is the value at the time of the call.
- `method` names a method of this cell: a name it doesn't have refuses
  the call that sets the schedule, naming the methods it does have. A
  scheduled call refused when it fires (or one a new build renamed) is
  said in the log and a report, and the schedule is still moved on (or
  removed), so it never fires in a loop.
- Schedules belong to a cell: they call only that cell. To reach another
  cell, have the scheduled method's cell [listen](composition.md) or call
  it from an effect.

### Keys

- A key names one schedule of one cell. Setting a key that exists
  replaces it: that is the whole of a debounce, an autosave or a session
  timeout (`cx.after("idle", 15 * 60_000, "sign_out", ())` on every
  activity call).
- Put what makes a schedule distinct in its key (`timer:{dish}`,
  `retry:{upload_id}`), and nothing that changes from call to call (a
  time, a counter), or every call adds one more.
- Keys starting with `@` are the declared ones (`@hourly`). Don't use `@`
  in your own keys: `cx.cancel("@hourly")` would stop a declared schedule
  until the cell next goes live.

### Backoff and polling

`aio::Backoff { base, factor, max }` is the wait before try `attempt`:
`base · factor^attempt` ms, at most `max`, at least 1.

```rust
use aio::Backoff;

# fn main() {
let b = Backoff::from(1_000); // doubling from 1 s, at most an hour
assert_eq!((0..5).map(|a| b.ms(a)).collect::<Vec<_>>(), [1_000, 2_000, 4_000, 8_000, 16_000]);
assert_eq!(b.ms(40), 3_600_000);
let poll = Backoff { base: 10_000, factor: 1.0, max: 10_000 }; // a steady poll (aio's `schedule.poll`)
assert_eq!(poll.ms(7), 10_000);
let gentle = Backoff { base: 500, factor: 1.5, max: 30_000 };
assert_eq!(gentle.ms(2), 1_125);
# }
```

`cx.backoff(key, attempt, b, method, args)` is `cx.after(key,
b.ms(attempt), method, args)`. Keep the attempt count in the state, so it
is saved, replayed and reset by the method that succeeds:

```rust
use aio::prelude::*;
use aio::testing::TestCells;

#[cell(persist)]
pub struct Upload {
    pub attempt: u32,
    pub sent: bool,
}

#[methods]
impl Upload {
    pub fn failed(&mut self, cx: &Cx) -> Result {
        let wait = aio::Backoff::from(1_000);
        self.attempt += 1;
        ensure!(self.attempt <= 5, "gave up after 5 tries");
        cx.backoff("send", self.attempt - 1, wait, "send", ())
    }

    pub fn send(&mut self) {
        self.sent = true;
        self.attempt = 0;
    }
}

# fn main() {
let mut t = TestCells::new();
let h = t.cell::<Upload>();
h.failed().unwrap();
h.failed().unwrap(); // the same key: the second wait (2 s) replaces the first
t.advance(1_500);
assert!(!h.state().sent);
t.advance(1_000);
assert!(h.state().sent && h.state().attempt == 0);
# }
```

A poll that slows down while nothing changes (aio's `schedule.poll`
with a growing interval) is the same: count the quiet polls in the state,
and pass that count as the attempt.

### Cron lines

Five fields: minute, hour, day of month, month, day of week, in this
machine's local time.

| Field | Values | Names |
|---|---|---|
| minute | 0-59 | |
| hour | 0-23 | |
| day of month | 1-31 | |
| month | 1-12 | `jan`…`dec` |
| day of week | 0-7 (0 and 7: Sunday) | `sun`…`sat` |

Each field takes `*`, a number, a range `a-b`, a list `a,b,c` and a step
`/n` (`*/15`, `10-50/10`, `5/15` from 5 on). As in cron, when both the
day of month and the day of week are given, either one matching is
enough: `0 9 1 * mon` is the 1st and every Monday.

| Line | Runs |
|---|---|
| `0 9 * * 1-5` | weekdays at 9:00 |
| `*/15 * * * *` | every quarter hour |
| `30 2 1 * *` | 2:30 on the 1st of each month |
| `0 0 * * sun` | Sunday at midnight |
| `0 12 25 dec *` | noon on 25 December |

Clock rules: a time a daylight-saving change skips runs once, at the
first minute that exists after it; a time that happens twice runs once,
the first time. The next run is always found after the last one, so a
clock stepped back never runs a slot twice.

```rust
use aio::cron::Cron;

# fn main() {
let c = Cron::parse("0 9 * * mon").unwrap();
let now = 1_700_000_000_000; // ms since the epoch
let next = c.next(now).unwrap();
assert!(next > now && next - now <= 7 * 86_400_000 + 3_600_000);
assert!(Cron::parse("61 * * * *").unwrap_err().contains("outside 0-59"));
assert!(Cron::parse("0 9 * *").is_err(), "five fields");
# }
```

## When schedules fire

- In the app, `aio::Cells::start` starts each cell's scheduler (and
  `aio::boot` does that for you). It sleeps until the soonest saved
  schedule is due, wakes when a call sets an earlier one, and fires each
  due schedule as a call by `By::Schedule(key)`. The firing runs on
  tokio's blocking pool: a slow scheduled method holds no runtime thread.
- Missed while the app was off, asleep or paused: a repeat fires once at
  start (or wake), then goes back to its rhythm from now; a one-shot fires
  once. Ten missed hourly runs are one run, never ten.
- Disabled or tripped (see [lifecycle](lifecycle.md)): schedules are held,
  and those missed meanwhile fire once when it is enabled again. Time
  travel's pause holds them the same way.
- Shutting down: no schedule fires once the app is closing.
- A failing scheduled call (refused or panicking) counts toward the cell's
  breaker like a failing effect.
- A schedule is a row of the cell's store, so it lasts as long as the
  cell's data does: a `persist` cell's schedules survive restarts; a cell
  without `persist` starts each run with only its declared ones.

`ax schedules <app>` lists the saved schedules, soonest first (key, when,
interval or cron line, method, arguments); `ax metrics <app>` counts them.

## Testing schedules

`aio::testing::TestCells` runs cells on a virtual clock. `t.advance(ms)`
moves it, fires what came due (a repeat missed more than once fires
once, as after a sleep) and settles the effects that led to; tokio's
timers inside effects move as far. `h.store(|s| s.schedules())` lists a
cell's saved schedules (`aio::Due`: `key`, `due`, `every`, `cron`,
`method`, `args`), and `t.failures()` takes the scheduled calls that
failed, so the test can assert on them. `t.start(cells)` starts a list of
cells as the app does, missed schedules first. No test sleeps for real.

## Heavy work off the cell's thread

A cell has no thread of its own. A call runs on the thread that makes it
(a view's handler, an effect, the scheduler, a socket's task), holding
only that cell's lock: calls on different cells run in parallel, and
calls on one cell run one at a time. A method must therefore be quick
(it holds its cell), and pure: no I/O, no sleep, no thread (see
[methods do no I/O](cells.md#methods-do-no-io)).

Work that takes long goes in an effect, and work that is CPU-heavy (a
hash, an image resize, parsing a big file) goes on tokio's blocking pool
inside it. Its result comes back as a call like any other:

```rust
use aio::prelude::*;
use aio::testing::TestCells;

#[cell]
pub struct Checksum {
    pub file: String,
    pub sum: Option<u64>,
}

/// Seconds of CPU in a real app: off every async thread.
async fn digest(text: String) -> u64 {
    tokio::task::spawn_blocking(move || text.bytes().fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(b as u64)))
        .await
        .unwrap_or_default()
}

#[methods]
impl Checksum {
    pub async fn check(&mut self, file: String) -> Result {
        self.file = file.clone();
        self.sum = None;
        let sum: u64 = digest(file).await;
        self.sum = Some(sum);
        Ok(())
    }
}

# fn main() {
let mut t = TestCells::new();
let h = t.cell::<Checksum>();
h.check("notes.txt".into()).unwrap();
assert_eq!(h.state().sum, None, "the call committed at once; the work runs after");
t.settle();
assert!(h.state().sum.is_some());
# }
```

- Effects run on aio-rs's background runtime (`aio::rt()`), never on the
  UI thread; `spawn_blocking` moves the CPU part off its async threads too.
- An await may take at most `aio::effect::CALL_CEILING` (30 s), unless the
  method is `#[long]` ([async methods](effects.md#async-methods)).
- To keep only the newest of many runs (a search as you type), use an
  `Effect` with `Mode::Newest` ([modes](effects.md#modes)).
- A panic in a method refuses that call and leaves the state as it was;
  three within a minute stop the cell until a restart. A panic in an
  effect's task ends that effect only.

Limits, compared with aio's `worker: true` and `blocking()`:

- No cell runs in a thread or process of its own, and there is no
  `worker` flag: a method that never returns holds its cell's lock (and
  its caller) for good, and nothing restarts it. Keep loops and long work
  out of methods.
- There is no `blocking(id, fn)` helper with an id: `spawn_blocking` in
  an effect does the work, and the effect's key and `Mode` give it the
  same "one at a time" or "newest wins" rule.

## Real time and high-frequency state

Ask how often a value changes and who must agree on it. A cell is made
for state worth keeping, sharing or replaying: each call copies the
state, checks the invariants, writes an event and pushes a patch to every
client. State that changes faster than anyone needs to agree on it should
skip all of that.

| What | Where | Why |
|---|---|---|
| a frame-by-frame animation, a canvas, a game loop | the view: `aio::kit::use_frame`, a `use_signal` | frame work is rendering, not state |
| a cadence only this window needs (a clock, a beat) | the view: `aio::kit::use_interval` | a schedule runs in the app's process, for everyone |
| a cursor, a scroll, a selection | a cell method with `#[coalesce]` | shown at once, written once per run ([coalesced writes](../persistence/README.md#coalesced-writes)) |
| shared state that changes often (a price ticker) | a `shared` cell, fed by one effect | each commit that changes what a viewer sees is one patch to them |
| results that matter (a score, settings) | a `persist` cell | saved, synced, tested |

- A cell's calls cost a copy of its state each. A big cell called many
  times a second pays that copy every time: split the hot part into a
  small cell of its own, or use `aio::imbl`'s persistent collections
  (feature `imbl`), whose copy is O(1).
- Views re-render at most once per `aio::ui::BACKGROUND` (150 ms) for
  commits from effects, schedules and other threads; a call from the
  view renders at once ([performance](../performance.md)).
- Pushes to a client follow commits: a client slower than the commits
  gets one patch for several. There is no push-rate setting (aio's
  `syncIntervalMs`); to send less, call less (one effect that commits
  once per batch) or make the viewer's view smaller
  ([visibility](visibility.md)).
- Keep a tick out of the timeline, reports and the debug log with
  `#[cell(diagnostics = false)]`; its events are still kept.
- aio-rs says when it sees trouble. Over 30 pushes a second, 500 KB in
  one push, or 1 MB a second to one client, it writes a `pressure:` line
  in the log (at most once every 2 s each) unless `[budgets]` declares
  its own limits. A method called 200 times a second for 5 s in a row is
  a call storm: said once as it starts (`[perf.storm]` sets `rate` and
  `secs`; `breaker = true` refuses its calls while it lasts). Either one
  on a loop you can't slow down means the state is in the wrong place.
  `ax cost <app>` shows what each cell sent.

## Common mistakes

- A schedule key with a changing part (`format!("save:{}", cx.now())`):
  every call adds one, nothing ever replaces. Key by what the schedule is
  for.
- Expecting a scheduled method to fire on a cell that isn't started: in
  the app the scheduler starts with `Cells::start`; in a test, call
  `t.advance(ms)`.
- `std::thread::sleep` or a loop in a method to "wait": schedule the next
  step instead.
- A schedule on a cell without `persist`, expected after a restart: only
  its declared schedules come back.
- A 60 fps loop calling a cell method: move the moving part into the
  view, and call the cell when it settles.
- Using `@` in a key of your own.

## API

`cargo doc -p aio-rs --no-deps --open`: `aio::Cx` (`after`, `every`,
`at`, `cron`, `backoff`, `cancel`), `aio::Backoff`, `aio::cron::Cron`,
`aio::Declared`, `aio::Cell::SCHEDULES`, `aio::Due`,
`aio::Store::schedules`, `aio::Cells::start`, `aio::effect::CALL_CEILING`,
`aio::testing::TestCells` (`advance`, `start`, `failures`),
`aio::kit::use_frame`, `aio::kit::use_interval`.

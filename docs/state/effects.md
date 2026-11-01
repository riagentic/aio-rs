# Effects, async methods and schedules

Methods never do I/O. What a method needs from the world is fetched by an
effect: work the method asks for, started after its call commits, that
hands its result back as another call. Both calls are events, so replay
needs no network, disk or clock.

| Need | Use |
|---|---|
| fetch, write a file, call a service | an [effect](#an-effect) or an [async method](#async-methods) |
| do it later, again, at a time, with backoff | a [schedule](#schedules) |
| show a notification | [`cx.tell`](#telling-people) |
| react to a value from outside a cell | [listeners and watchers](#reacting-outside-a-cell) |
| check it in a test | [`TestCells`](#testing-effects) |

## An effect

```rust
use aio::prelude::*;
use aio::testing::TestCells;
use std::time::Duration;

#[cell(persist)]
pub struct Weather {
    pub city: String,
    pub loading: bool,
    pub temp: Option<i32>,
}

/// Stands in for an HTTP call.
async fn fetch_temp(city: String) -> i32 {
    aio::time::sleep(Duration::from_millis(300)).await;
    city.len() as i32
}

#[methods]
impl Weather {
    pub fn refresh(&mut self, cx: &Cx, city: String) -> Result {
        ensure!(!city.is_empty(), "which city?");
        self.city = city.clone();
        self.loading = true;
        cx.run(
            Effect::new("fetch", move |h: Handle<Weather>| async move {
                let t = fetch_temp(city).await;
                _ = h.loaded(t); // a call: one more event
            })
            .mode(Mode::Newest) // a newer refresh cancels the one running
            .timeout(Duration::from_secs(10)),
        );
        Ok(())
    }

    pub fn loaded(&mut self, temp: i32) {
        self.loading = false;
        self.temp = Some(temp);
    }
}

# fn main() {
let mut t = TestCells::new(); // in-process cells on a virtual clock
let h = t.cell::<Weather>();
h.refresh("Oslo".into()).unwrap();
assert!(h.state().loading, "the call committed; its effect waits");
assert_eq!(t.effects(), ["Weather.fetch"]);
t.settle(); // runs effects to their end, the 300 ms at once
assert_eq!(h.state().temp, Some(4));
# }
```

- The key (`"fetch"`) names the effect for `Mode`, `cx.abort("fetch")`,
  `h.busy("fetch")`, `h.done("fetch").await` and `h.pending(..)` (what a
  spinner reads).
- An effect is dropped at its next await when its key is aborted, its
  `.timeout(d)` passes or the app closes; one that panics fails its own
  call only.
- A refused call never starts its effects: they start after the commit.

### Modes

Two effects with the same key, the second asked for while the first runs:

| `Mode` | The second | For |
|---|---|---|
| `Queue` (default) | waits for the first; nothing lost, nothing cancelled | saves, uploads in order |
| `Newest` | cancels the first | search as you type |
| `First` | is skipped; its caller may `h.done(key).await` and read what the first left | a refresh already running |

(An async method takes these as `#[mode(newest)]` / `#[mode(first)]`, below.)

`.per(&args)` keys the effect by its arguments too (`aio::effect::key`),
so `scan("/a")` and `scan("/b")` neither cancel, skip nor wait for each
other.

### Heavy work, retries and timeouts

- Effects run on aio-rs's background runtime (`aio::rt()`), never on the
  UI thread. CPU-heavy work (aio's `blocking(id, fn)`) is
  `aio::effect::blocking(key, |cancel| ..).await` inside the effect: a
  worker thread, with a `Cancel` it checks between chunks, set by
  `aio::effect::cancel_blocking(key)` (aio's `blocking.cancel(id)`) and
  when the effect awaiting it ends early (`cx.abort`, a timeout, the app
  stopping). Its result comes back as a call like any other: no cell
  needs its own thread.
- `aio::effect::retry(retries, timeout, || io())` runs `io` again up to
  `retries` more times while it fails, each try cut off after `timeout`
  (aio's `call({ timeoutMs, retries })`).
- Each await of an async method may take at most the call ceiling, 30 s
  unless `[perf] call_ceiling` (ms) says otherwise (aio's
  `effectTimeoutMs`): one that never settles is cut off, logged and
  reported, its later steps never run, and its caller is told. `#[long]`
  lifts the ceiling for a method that legitimately waits (a long download).

## What happens to an effect

From the call that asks for it to its end:

1. The method calls `cx.run(effect)`. Nothing starts yet: the effect is
   kept with the call.
2. The call commits (its event is saved). Only then do its effects start,
   in the order they were asked for. A refused call starts none.
3. The effect runs on aio-rs's background runtime (`aio::rt()`, two
   threads) or, in a test, on `TestCells`'s virtual clock. A process with
   no tokio runtime drops it and says so in the log.
4. It gets a `Handle` of its cell whose caller is `By::Effect(key)`: every
   call it makes is one more event by that caller, so `cx.by()` in the
   method it calls is the effect, not the person who started it. Pass
   what the follow-up needs (a user id) as an argument.
5. It ends by itself, or is dropped at its next await: by `cx.abort(key)`,
   by `Mode::Newest`, by its `.timeout(d)`, or at shutdown.

Replay runs no effect: the log holds the calls the effects made, with
their results, so a replayed store reaches the same state with no
network. On a synced copy a predicted call runs no effect either: the
server runs the call for real and its effects there
([sync](sync.md#what-crosses-the-wire)).

When an effect's call is refused, or the effect panics:

- the refusal is said in the log and in a report (`ax report <app>`),
  never thrown at a caller (there is none);
- it counts toward the cell's breaker: a cell whose schedules and effects
  keep failing is disabled, its state kept
  ([lifecycle](lifecycle.md));
- in a `TestCells` test it fails the test at its end, unless the test
  takes it with `t.failures()`.

At shutdown (`aio::shutdown(budget)`), a cell refuses new calls, stops its
schedules and lets its running effects finish and commit within the
budget; those still running at the deadline are aborted and named in the
log.

| Ask from a method | Does |
|---|---|
| `cx.run(Effect::new(key, f))` | start `f(handle)` after the commit |
| `cx.abort(key)` | drop the running and queued effects with this key, after the commit |
| `cx.tell(t)` | [tell the people using the app](#telling-people) after the commit |
| `cx.after`, `cx.every`, `cx.at`, `cx.cron`, `cx.backoff`, `cx.cancel` | set or drop a [schedule](scheduling.md) |

| `Effect` builder | Default | |
|---|---|---|
| `Effect::new(key, f)` | | `f: FnOnce(Handle<T>) -> impl Future<Output = ()> + Send` |
| `.mode(Mode::…)` | `Queue` | what a second effect with the same key does |
| `.per(&args)` | | adds `args` (as JSON) to the key; returns `Result` |
| `.timeout(d)` | none | drop it after `d`, said in the log and a report |

| Read from outside | |
|---|---|
| `h.busy(key)` | an effect with this key runs or waits |
| `h.done(key).await` | until none does |
| `h.pending(Some(method))`, `h.pending(None)` | how many run or wait (an async method's, or all) |
| `h.on_pending(f)` | `f` when that may have changed: a spinner re-renders |
| `aio::on_effect(\|cell, key\| ..)` | hear every effect as it starts (aio's `onEffect`): metrics, traces |

Each poll of an effect (the time it holds a runtime thread between two
awaits) has a budget, 5 ms by default: `[perf] effect = 10` or
`[perf.effects] "weather.fetch" = 50` in `aio.toml`. A step over it is
written to `logs/perf.log` and the app log with the fix (most often:
`spawn_blocking`). It never stops the effect.

## Async methods

A `pub async fn` is written in one piece, as aio's async methods are, and
its caller awaits its end, as aio's `await cell.method()` does. aio-rs
splits it at each await: the code before the first await is the method,
each await becomes an effect, and the code after it a step method (`m.1`,
`m.2`…, numbered in source order) that the effect calls with the result.
Each part is one atomic call and the log holds every awaited result.

```rust
use aio::prelude::*;
use aio::testing::TestCells;

#[cell(persist)]
pub struct Shop {
    pub status: String,
    pub total: u64,
}

async fn quote(item: String) -> u64 {
    item.len() as u64 * 100
}

#[methods]
impl Shop {
    pub async fn buy(&mut self, item: String, limit: u64) -> Result {
        ensure!(!item.is_empty(), "nothing to buy");
        self.status = "pricing".into();
        let price: u64 = quote(item.clone()).await;
        ensure!(price <= limit, "{item} costs {price}, over {limit}");
        self.total += price;
        self.status = "done".into();
        Ok(())
    }
}

# fn main() {
let mut t = TestCells::new();
let h = t.cell::<Shop>();
let mut end = h.buy("tea".into(), 1000).unwrap(); // its first part committed
t.settle();
assert_eq!((h.state().total, h.state().status.as_str()), (300, "done"));
assert_eq!(end.now(), Some(Ok(())), "and its caller has the end");
# }
```

- The typed call (`h.buy(..)`) returns `Result<aio::Answer<R>>`: `Err`
  when the first part refused; the `Answer`, awaited, is the value the
  method's last part returned, or the refusal that ended it after an await
  (a step's `ensure!`, an awaited `?`, its await cut off at the call
  ceiling, aborted or superseded). Drop it to fire and forget: the call
  runs either way. `Answer::now()` reads it without awaiting (a sync test
  after `t.settle()`). A synced copy's `Answer` is the server's end, as
  is `call_confirmed`'s; an aio page's `action` is acked on the end.
- An await stands where a statement does, at any depth of `if`,
  `if let`, `match`, `loop`, `while` and blocks, with `break` and
  `continue`: `let x: T = io.await;`, `let x: T = io.await?;` (an `Err`
  ends the method there, said to its caller), `x = io.await;`,
  `io.await;`, `let x: T = match .. { .. => io.await, .. };`, or the
  method's last expression (`lookup(id).await`, a `Result<R>`). An await
  inside `for` is refused: keep the position in a typed `let` and use
  `while`.
- Only the arguments (owned, no references) and typed `let`s live across
  an await: `T` is how a value is saved in the log, so it is `Serialize`
  (a `Result<String, String>`, not an error type that isn't). An untyped
  `let` used after an await, or a binding inside a branch that hides one
  used after it, fails the build naming it.
- An async method returns `aio::Result<R>`. A refusal after an await
  refuses that step: what the steps before committed stays.
- Each step after an await is called by the method's effect
  (`By::Effect("buy")`), not by the first caller: keep what the first part
  reads of `cx.by()` or `cx.role()` in a typed `let`. A step's refusal
  reaches its caller's `Answer`, is said in the log and counts as an
  [effect's failure](#what-happens-to-an-effect).
- `#[mode(newest)]`: a new call supersedes the running one (its await is
  aborted, its later steps refused, its caller told; search as you type).
  `#[mode(first)]`: while one runs, a new call runs nothing and shares its
  answer (aio's `concurrency: "first"`). Without one, overlapping calls'
  awaits run in turn.
- `#[ttl(ms)]` answers a repeated call with the same arguments within
  `ms` of a finished one with its kept answer, without running it again
  (aio's `ttl`); its effects are keyed by the arguments.
- `h.pending(Some("buy"))` counts the calls in flight (two overlapping
  calls are two); `h.on_pending(f)` tells when that may have changed.

Awaits in a loop and a branch, and the answer:

```rust
use aio::prelude::*;
use aio::testing::TestCells;

#[cell(persist)]
pub struct Feed {
    pub rows: Vec<String>,
}

/// Stands in for a paged HTTP API: three pages, then none.
async fn page(n: u32) -> std::result::Result<Vec<String>, String> {
    Ok(if n < 3 { vec![format!("row {n}")] } else { vec![] })
}

#[methods]
impl Feed {
    pub async fn sync(&mut self, max: u32) -> Result<u32> {
        let mut n: u32 = 0;
        while n < max {
            let rows: Vec<String> = page(n).await?;
            if rows.is_empty() {
                break;
            }
            self.rows.extend(rows);
            n += 1;
        }
        Ok(n)
    }
}

# fn main() {
let mut t = TestCells::new();
let h = t.cell::<Feed>();
let mut pages = h.sync(10).unwrap();
t.settle();
assert_eq!(pages.now(), Some(Ok(3)));
assert_eq!(h.state().rows, ["row 0", "row 1", "row 2"]);
# }
```

What can't be resumed fails the build:

```rust,compile_fail
use aio::prelude::*;

#[cell]
pub struct Walk {
    pub seen: u32,
}

async fn visit(_: u32) {}

#[methods]
impl Walk {
    pub async fn all(&mut self) -> Result {
        for i in 0..3u32 {
            visit(i).await; // the iterator isn't saved: use `while`
        }
        Ok(())
    }
}
# fn main() {}
```

```rust,compile_fail
use aio::prelude::*;

#[cell]
pub struct Walk {
    pub seen: u32,
}

async fn visit(_: u32) {}

#[methods]
impl Walk {
    pub async fn one(&mut self) -> Result {
        let at = 7u32; // untyped: it couldn't be saved with the step
        visit(at).await;
        self.seen = at;
        Ok(())
    }
}
# fn main() {}
```

Retrying inside the awaited future keeps the log to one result:

```rust
use aio::prelude::*;
use aio::testing::TestCells;
use std::time::Duration;

#[cell]
pub struct Page {
    pub body: Option<String>,
    pub error: Option<String>,
}

/// Stands in for an HTTP GET that fails now and then.
async fn get(url: &str) -> std::result::Result<String, String> {
    if url.is_empty() { Err("no url".into()) } else { Ok(format!("<h1>{url}</h1>")) }
}

async fn get_with_retries(url: String) -> std::result::Result<String, String> {
    aio::effect::retry(2, Some(Duration::from_secs(5)), || get(&url)).await.map_err(|e| e.why)
}

#[methods]
impl Page {
    pub async fn load(&mut self, url: String) -> Result {
        self.error = None;
        let got: std::result::Result<String, String> = get_with_retries(url).await;
        match got {
            Ok(b) => self.body = Some(b),
            Err(e) => self.error = Some(e),
        }
        Ok(())
    }
}

# fn main() {
let mut t = TestCells::new();
let h = t.cell::<Page>();
h.load("example.org".into()).unwrap();
assert!(h.state().body.is_none(), "the call committed; its effect waits");
t.settle();
assert_eq!(h.state().body.as_deref(), Some("<h1>example.org</h1>"));
h.load(String::new()).unwrap();
t.settle();
assert_eq!(h.state().error.as_deref(), Some("no url"), "three tries, the last failure kept");
# }
```

## Schedules

A schedule is a call the cell asks for at a later time: once, again and
again, at a time of day, or after a backoff. It is saved with the call
that sets it, so it survives a restart. [Scheduling](scheduling.md) has
every kind, their rules and the common patterns (debounce, polling,
retries, session timeouts); in short:

```rust
use aio::prelude::*;
use aio::testing::TestCells;

#[cell(persist)]
pub struct Feed {
    pub ticks: u32,
    pub reminders: u32,
}

#[methods]
impl Feed {
    /// Declared: armed whenever the cell is live.
    #[every("1m")]
    pub fn tick(&mut self) {
        self.ticks += 1;
    }

    /// Asked for by a call: saved, so it survives a restart.
    pub fn remind_later(&mut self, cx: &Cx) -> Result {
        cx.after("remind", 5_000, "remind", ())
    }

    pub fn remind(&mut self) {
        self.reminders += 1;
    }
}

# fn main() {
let mut t = TestCells::new();
let h = t.cell::<Feed>();
h.remind_later().unwrap();
t.advance(60_000); // the virtual clock: schedules fire, effects settle
assert_eq!((h.state().ticks, h.state().reminders), (1, 1));
# }
```

- Declared: `#[every("5m")]`, `#[every(300_000)]`, `#[cron("0 9 * * 1")]`,
  each optionally `, skip_if_running`.
- From a call: `cx.after`, `cx.every`, `cx.at`, `cx.cron`, `cx.backoff`
  and `cx.cancel`, each by a key; the same key replaces.
- An effect is for I/O; a schedule only calls a method later. A schedule
  that needs I/O calls a method that starts an effect.

## Telling people

A method that finishes something can say so on the desktop (aio's
`notify()` and `appIcon()`). Like an effect, a tell is kept with the call
and happens after it commits: a refused call tells nothing.

```rust
use aio::prelude::*;
use aio::tell::Tell;

#[cell(persist)]
pub struct Export {
    pub runs: u32,
}

#[methods]
impl Export {
    pub fn finished(&mut self, cx: &Cx, files: u32) -> Result {
        self.runs += 1;
        cx.tell(Tell::Notify {
            title: "Export finished".into(),
            body: format!("{files} files"),
            tag: Some("export".into()), // a second one replaces this card
            silent: false,
            route: Some("/exports".into()), // a click opens the app there
        })
    }

    pub fn untitled(&mut self, cx: &Cx) -> Result {
        self.runs += 1;
        cx.tell(Tell::notify(" ", "no title"))
    }
}

# fn main() {
let h = Handle::<Export>::memory();
let mut heard = h.tells(); // what a window or a synced client would show
h.finished(3).unwrap();
let Ok(Tell::Notify { body, .. }) = heard.try_recv() else { panic!("told nothing") };
assert_eq!(body, "3 files");
assert!(h.untitled().is_err(), "a blank title refuses the call, where it is written");
assert_eq!(h.state().runs, 1);
# }
```

| `Tell::Notify` field | |
|---|---|
| `title` | required: a blank one refuses the call that asks for it |
| `body` | the text under it |
| `tag` | a card with the same tag replaces the last one (a progress count is one card) |
| `silent` | no sound |
| `route` | a click brings the app forward and goes there |

`Tell::notify(title, body)` is the short form. `Tell::Icon { tray,
frames, fps }` sets the window's icon (`tray: true`: the tray's): PNG
frames, base64, looped at `fps` (at most 12 shown a second, 64 frames of
512 KB each at most); several frames need an `fps`, and no frames put the
app's own icon back.

Where it is shown:

- by the cell's own window: the desktop shell shows it through Electron's
  main process, with the app's icon and no permission prompt of its own;
- by every synced client of the cell (`Remote::on_tell(f)` hears them
  too). Every client means every user: a tell made in Ana's call is shown
  on Bo's screen as well, so keep one user's data out of `title` and
  `body`, and show it through their own state instead;
- in-process: `h.tells()` is a broadcast of every tell from now on.

Where it can't be, it says so:

- no window and no client (a headless server, a command): the log says
  `nobody to tell (no window, no client)` with the tell;
- a desktop that shows no notifications, or a Linux session with no
  notification service: the shell's log says it was not shown;
- a client that falls far behind (64 tells unread) misses the ones it
  skipped; the link is kept.

The operating system has the last word: macOS's per-app setting and Focus
modes, Windows's Do not disturb, the Linux desktop's notification daemon.
A notification the OS suppresses is not an error anywhere in aio-rs.

## Reacting outside a cell

- `h.listen(|state| ..)`: after each commit.
- `h.watch(|s| s.temp, |t| ..)`: when one value changes.
- `h.on_change(immediate, |s| s.city.clone(), |city, old| open(city))`:
  what the reaction returns (a socket, a camera) is kept until the next
  change and dropped before it (aio's `onChange`); dropping the returned
  `aio::Watching` stops it.
- `h.until(timeout, |s| s.ready).await`: wait in an effect until another
  cell's state holds (aio's `until`).
- `h.events()`: every committed event from now on, as a tokio broadcast
  (a subscriber that falls behind gets `Lagged` and takes a full state).
- `other.listens_to(&h, "loaded", |other, event| ..)`: a call on one cell
  runs one on another.

## Testing effects

`aio::testing::TestCells` boots cells on a virtual clock: effects are held
until the test lets them run, and their sleeps and timeouts pass at once.

| | |
|---|---|
| `t.effects()` | the effects asked for that haven't started (`Cell.key`), in order |
| `t.settle()` | start them and wait until they, and all they lead to, are done (5 s at most, then it panics naming those still running) |
| `t.advance(ms)` | move the clock: schedules fire, then everything settles |
| `t.failures()` | take the failed effect and schedule calls, so the test's end doesn't fail on them |

```rust
use aio::prelude::*;
use aio::testing::TestCells;

#[cell]
pub struct Inbox {
    pub count: Option<u32>,
    pub by_effect: bool,
}

/// Stands in for a server that answers with how many messages wait.
async fn count_on_server(fail: bool) -> u32 {
    if fail { 0 } else { 3 }
}

#[methods]
impl Inbox {
    pub fn check(&mut self, cx: &Cx, fail: bool) -> Result {
        cx.run(Effect::new("check", move |h: Handle<Inbox>| async move {
            let n = count_on_server(fail).await;
            _ = h.counted(n);
        }));
        Ok(())
    }

    pub fn counted(&mut self, cx: &Cx, n: u32) -> Result {
        ensure!(n > 0, "the server sent nothing");
        self.count = Some(n);
        self.by_effect = matches!(cx.by(), By::Effect(k) if k == "check");
        Ok(())
    }
}

# fn main() {
let mut t = TestCells::new();
let h = t.cell::<Inbox>();
h.check(false).unwrap();
assert_eq!(t.effects(), ["Inbox.check"]);
t.settle();
assert_eq!(h.state().count, Some(3));
assert!(h.state().by_effect, "the follow-up call is the effect's");

h.check(true).unwrap();
t.settle();
let failed = t.failures();
assert!(failed[0].contains("the server sent nothing"), "{failed:?}");
assert!(failed[0].contains("effect `check`"), "{failed:?}");
# }
```

An effect that waits on real I/O under a real timeout belongs in a
`TestServer` test ([testing](../testing/README.md)), and
`aio::cassette::Cassette` records an effect's I/O once and replays it.

## Common mistakes

- `std::thread::sleep` or blocking I/O inside an effect: it holds a
  runtime thread. Use `aio::time::sleep` and `spawn_blocking`.
- Calling the cell's method from inside the method to "continue later":
  ask for a schedule (`cx.after`) or an effect instead.
- `.await` inside an `if` or a loop of an async method: refused at build
  time; put the branching in the awaited future or in the step after it.
- A schedule's `method` misspelled: the call that sets it is refused, so
  check the result of `cx.after(..)` (`?`).
- Expecting a `Mode::First` caller to get its own result: it was skipped;
  wait with `h.done(key).await` and read the state.
- Deciding on `cx.by()` in the method an effect calls: the caller is the
  effect (`By::Effect(key)`). Pass the user as an argument.
- One user's data in a tell's `title` or `body`: every client of the cell
  shows it, whoever they are.
- Ignoring `cx.tell(..)`'s result: a blank title refuses the call, so end
  the method with it or use `?`.

## Limits

What aio has and aio-rs does not, or does differently:

- **Blocking work stops when it looks.** `aio::effect::blocking`'s work
  runs on a worker thread and stops at its next `cancel.cancelled()`
  check, as aio's does at its next message; a thread can't be killed.
- **Tests see effects as descriptions.** aio's `testCell` hands the effect
  objects to the test (`t.getEffects()`, `isScheduleEffect`); `TestCells`
  lists them by key (`t.effects()`) and as `HeldEffect`s (key, mode,
  timeout: `t.held_effects()`), and runs them (`t.settle()`); schedules
  are `ax schedules` and `Store::schedules`.
- **No notifications in a browser page.** A page built with `ax build
  --web` shows no tell unless the app sets `aio::tell::show_with(f)`
  itself; aio asks the browser's permission
  (`requestNotificationPermission()`), aio-rs does not.
- A command-line client prints no tell.

## API

`cargo doc -p aio-rs --no-deps --open`: `aio::Effect`, `aio::Mode`,
`aio::effect` (`retry`, `key`, `blocking`, `cancel_blocking`, `call_ceiling`), `aio::Answer`, `aio::Cx` (schedules,
`run`, `abort`, `tell`), `aio::Backoff`, `aio::cron`, `aio::tell` (`Tell`,
`show_with`), `aio::on_effect`, `aio::held`, `aio::Handle` (`pending`,
`on_pending`, `busy`, `done`, `until`, `watch`, `on_change`, `events`,
`tells`), `aio::sync::Remote::on_tell`, `aio::testing::TestCells`.

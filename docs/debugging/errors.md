# Errors

Every way an aio-rs app can fail, what each looks like, and what to do
about it. For the order to work in (read the error, look at the running
app, reproduce, fix) see [debugging](README.md); for symptoms by OS see
[troubleshooting](troubleshooting.md).

## Where things fail

| Where | What you see | Stops what |
|---|---|---|
| building | a compile error; a wrong `aio.toml` key fails the build in `aio::config!()`, naming the key | the build |
| boot | the app refuses to start, says why and what to do (a port in use, a store it can't read, a second instance) | the start |
| a call | a [refusal](#refusals): `Err(Rejected)`, the state unchanged | that call |
| a method's panic | a refusal plus a [problem report](feedback.md) of kind `panic` | that call |
| an effect, a schedule, a hook, a reaction | a log line and a problem report (`effect:timeout`, `refused`, …) | that step; after many, the cell ([the breaker](#the-breaker)) |
| a best-effort part | `degraded:<part>` once, `ax health` fails ([production](production.md#health)) | nothing: the app goes on |
| a budget or a threshold | a `WARN [perf]` line, the `on_problem` hook (`budget:*`, `storm`), health | nothing (a storm's calls, with `breaker`) |
| the link to a server | `Code::ConnectionLost` on calls made meanwhile | calls to that server |
| the window | `window:crashed` (the page reloads; 4 crashes in a minute close it) or `window:frozen` reported | the window |

## Refusals

A method that can fail returns `aio::Result`, and fails by refusing:
`ensure!(cond, "…")` or `reject!("…")`. A refusal is an
`aio::Rejected`:
`why` (words for people: the cause and the fix) and `code` (an
`aio::Code` for
code, `None` for the app's own). A refused call changes nothing: the
state, the log and the clients stay as they were.

```rust
use aio::prelude::*;

#[cell]
pub struct Contacts {
    pub emails: Vec<String>,
}

#[methods]
impl Contacts {
    pub fn add(&mut self, email: String) -> Result {
        ensure!(email.contains('@'), "{email:?} is not an email address: it needs an @");
        ensure!(!self.emails.contains(&email), "{email} is already in the list");
        self.emails.push(email);
        Ok(())
    }
}

/// What a UI does with a failed call: branch on the code, never the words.
fn say(e: &Rejected) -> String {
    match e.code() {
        Some(Code::AccessDenied) => "sign in to do that".into(),
        Some(Code::ConnectionLost) => "offline: try again when connected".into(),
        Some(Code::ActionRefused) => "this app is older than the page: reload".into(),
        // The app's own refusal: its words are written for the user.
        _ => e.why.clone(),
    }
}

# fn main() {
let h = Handle::<Contacts>::memory();
h.add("ada@example.org".into()).unwrap();
let e = h.add("ada".into()).unwrap_err();
assert_eq!(say(&e), "\"ada\" is not an email address: it needs an @");
assert!(h.add("ada@example.org".into()).is_err(), "a duplicate");
assert_eq!(h.state().emails.len(), 1, "a refused call changed nothing");

let e = h.call("remove", serde_json::json!(["ada@example.org"])).unwrap_err();
assert_eq!((e.code(), e.code().map(Code::as_str)), (Some(Code::ActionRefused), Some("ACTION_REFUSED")));
# }
```

### The codes

| `Code` | Wire name | Made when | What to do |
|---|---|---|---|
| `AccessDenied` | `ACCESS_DENIED` | a network caller may not: the method isn't `#[client]`, its rule said no, a server function's access refused, the session ended | ask the user to sign in, or hide the control for that role |
| `ActionRefused` | `ACTION_REFUSED` | the caller is allowed, but there is no such method (a client newer or older than the app; a typo in `h.call`) | reload the page, update the client; fix the name |
| `ConnectionLost` | `CONNECTION_LOST` | the link to the app dropped or never came up | retry when it is back; **the call may have run**: make it safe to send twice, or read the state first |
| `None` | none | the method's own `ensure!`/`reject!`; a cell disabled or paused by time travel; the app closing (`retry after … ms`); a storm's breaker; a budget's `spend` | show `why`; it was written for people |

- Codes are stable: a code keeps its meaning and its wire name across
  versions, and new ones may come (`Code` is `#[non_exhaustive]`), so a
  `match` has a `_` arm. A client that meets a code it doesn't know reads
  `None` (`Code::of`).
- The words are not stable. Branch on `code()`, never on `why`.
- A refusal crosses the wire as `why` and the wire name, so a synced copy,
  a `Link`, an aio client (`errorCode(err)`) and `ax call` all see the
  same code.
- `Rejected::new(why)` and `Rejected::coded(code, why)` make one by hand
  (an `impl From<&str>` and `From<String>` too, so `Err("…".into())`
  works); `aio::Result<T>` is `Result<T, Rejected>`.

### A refusal is not a bug

A refusal is the expected "no": it is logged at `debug` (with its
`why`), never as an error, and it is not a problem report. A **panic** is
a bug: in a method it refuses that call too (the state stays as it was,
the app goes on), and it is reported as `panic`, with where it happened
and which call it was in (`… at src/lib.rs:12 in notes.add`). In a
debug build the panic's source line is printed under it, with the fix:

```text
thread 'main' panicked at src/lib.rs:12:23: index out of bounds
  12 │         let first = self.items[0].clone();
     │                       ^
  → fix: return Err (aio::reject!) for an expected failure; a panic is a bug
```

`unwrap()` on user input, indexing past the end and `expect` on a file
that may be missing are panics. Use `ensure!`, `.get(i)` and `?`.

### Messages that teach

aio-rs's own messages say what happened and the one thing to do, in one
line: `<what> → fix: <fix>`
(`aio::report::teach`
makes one). Write your `why`s the same way: "a PIN has at least 4
digits", not "invalid input".

## The breaker

A cell whose schedules, effects or hooks keep failing is disabled before
it does harm: calls refused (`<cell> is disabled`), schedules paused,
running effects aborted, its state kept. It is said in the log, the cell
is a degraded part (`cell <id>`), and a `tripped` report is written. Fix
the cause (the log has each failure), then enable it again
(`Cells::enable`). How many failures trip it: `Cells::trip_after(max,
window)` ([lifecycle](../state/lifecycle.md)).

## Problems: every kind

A problem is anything worth a person's look: written as a report (most
kinds), said to the [`on_problem` hook](#hearing-problems-in-code), put on
the diagnostic bus. `aio::report::Code::of(kind)` types the kind and
`tip()` says what to do.

| Kind | `report::Code` | Report? | Made when | Tip |
|---|---|---|---|---|
| `panic` | `Panic` | yes | any panic: a method's (its call refused), an effect's, a thread's | return `Err` (`reject!`) for an expected failure |
| `tripped` | `Tripped` | yes | [the breaker](#the-breaker) disabled a cell | fix the failing calls, then enable the cell |
| `refused` | `Refused` | no | a cell's own schedule or effect called a method that refused | check its arguments and the state |
| `effect:timeout` | `EffectTimeout` | yes | an effect ran past its `.timeout(…)` and was cancelled | raise the timeout, or make the work finish sooner |
| `budget:method`, `budget:effect` | `BudgetMethod`, `BudgetEffect` | no | a call or an effect step slower than its `[perf]` budget | move slow work into an effect; `spawn_blocking` in an effect |
| `budget:state`, `budget:payload`, `budget:broadcast_rate` | `Other` | no | a `[budgets]` size or rate passed ([scaling](../build/scaling.md)) | make the state smaller, push less often |
| `storm` | `Storm` | no | a method called in a runaway loop (`[perf.storm]`) | find what calls it in reply to its own change |
| `degraded:<part>` | `Degraded` | yes | a part failed 5 times in a row (`aio::health::part`, sync, a cell) | the report holds its last error |
| `memory:high`, `memory:growing`, `memory:critical` | `Memory` | yes | the process holds too much of the machine, or keeps growing | look for state or caches that only grow |
| `window:crashed`, `window:frozen` | `Window` | yes | the window's renderer died, or stopped answering | `ax logs <app> --client`; the report has the page's last lines |
| `update:failed` | `UpdateFailed` | yes | an update couldn't be fetched, checked or applied ([release](../build/release.md)) | the report says which step |
| `user` | `User` | yes | the user filed it ([feedback](feedback.md)) | read the body |
| `schedule`, `hook`, `reaction` | `Other` | yes | a schedule couldn't be declared; `#[on_start]`/`#[on_stop]` refused; a `listens_to` reaction refused | the message names the cell and method |
| `client:<kind>` | `Other` | no | a linked client heard `<kind>` on its own bus (at most 20 per 5 s per client) | look at that client |

Two other refusals name themselves in their `why` for scripts:
`MEMORY_UNBOUNDED: <name> passed its ceiling` (an
[`aio::health::budget`](production.md#named-gauges-and-ceilings) spent past
its max) and `refused during a call storm ([perf.storm] breaker)`.

### Hearing problems in code

```rust
use std::sync::mpsc;

# fn main() {
let (tx, rx) = mpsc::channel::<String>();
let tx = std::sync::Mutex::new(tx);
// Runs where the problem happened, maybe while a cell is mid-call: hand
// the work off (a channel, a task), never call a cell from here.
aio::report::on_problem(move |kind, msg| {
    let tip = aio::report::Code::of(kind).tip().unwrap_or("see the report");
    _ = tx.lock().unwrap().send(format!("{kind}: {msg} ({tip})"));
});

let part = aio::health::part("errors-page-thumbs");
for _ in 0..5 {
    part.guard(Err::<(), _>("disk full"));
}
assert!(part.is_degraded());
let heard: Vec<String> = rx.try_iter().collect();
assert!(heard.iter().any(|l| l.starts_with("degraded:errors-page-thumbs: disk full")), "{heard:?}");
# }
```

- `aio::report::on_problem(f)`: one hook, the app's (a second call
  replaces it). It hears every problem, also those not written as a
  report (a repeat, past the cap) and the warnings that never are
  (`budget:*`, `refused`, `storm`). A hook that panics is logged, never
  re-entered, and never takes the caller down.
- `aio::report::subscribe(f)`: as many as like, each held by the
  `Subscription` it returns (drop it to stop). Each sees a
  `Diagnostic`
  (`at`, `kind`, `message`, `held`): the same kind is let through at most
  once per 5 s (`QUIET_MS`), with how many were held back meanwhile.
- `aio::report::recent()`: the newest 200 diagnostics, oldest first
  (what a report carries).
- `aio::report::timeline()`: the newest 100 calls, oldest first: when,
  which, how long (`ms`), and a refusal's reason; never the arguments.

## In tests

A test fails on a refusal nobody looked at, so a failure can't hide:

- `TestCells` panics when dropped with schedule or effect failures not
  taken: `failed and never looked at (take them with failures)`. Take
  the expected ones with `t.failures()`.
- `TestUi` fails on a view's `act!`/`call!` refusals nobody took
  (`ui.failures()`), and on the wire's refusals in a synced test.
- `aio::testing::seeded` panics on a seed key the state doesn't have,
  naming the ones it has: a seed that pins nothing must not pass.

[Testing](../testing/README.md) has the harnesses.

## Logs

The app's stderr (its own lines, aio's, panics, the window's) also lands
in its `logs/` folder (`ax data <app>` shows where):

| File | Holds |
|---|---|
| `app.log` | every line, stamped in UTC |
| `error.log` | the `ERROR` lines |
| `warning.log` | the `WARN` lines |
| `perf.log` | each blown `[perf]` budget, with where the call's time went (`clone`, `apply`, `check`, `commit`) |
| `start.log` | the output of an app `ax start` launched |
| `dev.log` | dx's whole output under `ax dev` |

A line reads `2026-10-28T09:48:01.123Z WARN  [net] lost 3 frames peer=7`:
the stamp, the level padded to 5, the category (`aio` for the framework,
`app` for the app's untagged lines, `page` and `aio-shell` for the
window's), the message, then `key=value` pairs. The level is the
instruction:

| Level | Means |
|---|---|
| `TRACE`, `DEBUG` | detail you asked for (`[log] level`); at `debug` each call is a line, never its arguments |
| `INFO` | the app narrating itself: nothing to do |
| `WARN` | should be fixed; the app keeps working |
| `ERROR` | must be fixed: something did not happen |

- Writing: `aio::trace!`, `debug!`, `info!`, `warn!`, `error!`, with
  `cat: "net"` and `key = value` pairs before the format string. A line
  below `[log] level` costs one load and formats nothing.
- Every line is one line: a message's line breaks are written `\n`,
  control characters `\xNN`, and text past 8192 characters is cut and
  says so, so no text can forge an entry.
- The same line many times in a row is written once, then `… last line
  repeated N more times`.
- Credentials are masked with `…`: `token=` values, `Bearer` tokens and
  pairing codes, in every log line and report.
- Colour: only the level word, only on a terminal; `NO_COLOR` (set to
  anything) turns it off, `FORCE_COLOR` (not `0`, `false` or empty)
  forces it on. Files are never coloured.
- Rotation: a file rotates at `[log] file_mb` (10) to `app.1.log`, …;
  `[log] keep` (5) files are kept per log. The three logs stay within 3 ×
  `keep` × `file_mb`.
- When the log closes, its last line counts the run's errors and
  warnings; every `[log] heartbeat` seconds (3600) a line says uptime,
  calls and errors.

[Debugging: logs](README.md#3-logs) has `[log]` in full and `ax logs`.

### In the window

A debug build's page shows its own errors where the person is looking:
a small corner badge (`2 page errors`) that opens a list, one row per
problem with a count (a per-frame failure is one row, `×2043`), at most
20 rows. It hears uncaught errors, unhandled rejections and `ax dev`'s
failed build (the badge clears once the build is fixed). It is plain DOM,
so it still reports when the renderer is what broke. A release page has
none of it; `ax build --web --dev` gives a web build that has it.

## Forensics

1. `ax errors <app>`: the newest warnings and errors, each under the
   start of the run it came from (and `ax dev`'s failed build first).
2. `ax report <app>`: what it filed by itself; `ax report <app> <id>`
   shows one (a prefix of the id is enough).
3. `ax logs <app> <word> --since 2h`: the lines around it; a cell's or
   method's name is a good word.
4. `ax events <app> <cell> --diff`: what each call changed.
5. `ax logs <app> --tag perf` and `logs/perf.log` when it was slow.
6. `ax record <app> <cell> bug.rs` turns what it just did into a failing
   test ([time travel](time-travel.md#from-the-log-to-a-test)).

## Not in aio-rs

What aio has and aio-rs doesn't, with what to use instead:

- **The error box with the state.** aio prints the cell's state with an
  error. aio-rs keeps state out of log lines (a log is not a data
  channel): a problem report carries the cells as saved.
- **One `onError` hook with an error object** (`code`, `source`,
  `context`, `original`, `stateSnapshot`). aio-rs has `on_problem(kind,
  message)` and the bus, whose `Diagnostic` is the typed object
  (`code()`, `severity`, `tip`; a report has the state); a call's caller
  gets the `Rejected`. A call's lines carry `call=<id>` (aio's
  correlation id: [logs](README.md#3-logs)).
- **aio's many error codes** (`REDUCE_ERROR`, `EFFECT_ERROR`,
  `QUEUE_OVERFLOW`, `DISPATCH_*`, `PERSIST_*`, `TX_CONFLICT`, the test
  harness codes). In aio-rs a method's failure is its own `Rejected`, an
  effect's is logged under its key, and the framework's few typed codes
  are the three above plus the problem kinds; the test harnesses fail
  with words, not codes.
- **A source-mapped `client.log`.** aio-rs has no bundled JavaScript to
  map: the window's lines go into `app.log` as `[page]`, and Rust panics
  carry their file and line.
- **`am logs` budgets** (`logBudget`, `--no-backup-logs`): aio-rs bounds
  the folder with `file_mb` × `keep` instead.

## API

`cargo doc -p aio-rs --no-deps --all-features --open`: `aio::Rejected`,
`aio::Code`, `aio::ensure!`, `aio::reject!`, `aio::report` (`on_problem`,
`subscribe`, `recent`, `timeline`, `Code`, `teach`, `redact`),
`aio::health::part`, `aio::log`.

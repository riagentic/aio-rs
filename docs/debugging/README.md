# Debugging

The order that works: read the error, look at what ran, look at what the
app holds, reproduce it as a red test, fix, green. Symptoms by OS and by
kind (a blank window, an app that won't start, a slow list):
[troubleshooting](troubleshooting.md).

| Page | For |
|---|---|
| [errors](errors.md) | every refusal code and problem kind, the breaker, logs, what to do |
| [time travel](time-travel.md) | stepping a cell through its history, replay, a test from the log |
| [production](production.md) | health, metrics, memory, vitals, budgets, failure scenarios |
| [feedback](feedback.md) | problem reports: what they hold, where they go, a "Report a problem" button |
| [troubleshooting](troubleshooting.md) | symptom, cause and fix, by kind and by OS |

## 1. Read the error

Errors name the cause and the fix: a refusal's `why`, a build error from
`aio::config!()` naming the `aio.toml` key, a boot refused with what to
do. Refusals carry a typed code for code to match on, never their words:

```rust
use aio::prelude::*;

#[cell]
pub struct Vault {
    pub open: bool,
}

#[methods]
impl Vault {
    pub fn unlock(&mut self, pin: String) -> Result {
        ensure!(pin.len() >= 4, "a PIN has at least 4 digits");
        self.open = true;
        Ok(())
    }
}

# fn main() {
let h = Handle::<Vault>::memory();
let e = h.unlock("1".into()).unwrap_err();
assert_eq!((e.why.as_str(), e.code()), ("a PIN has at least 4 digits", None), "the app's own refusal");
let e = h.call("nope", serde_json::json!([])).unwrap_err();
assert_eq!(e.code(), Some(Code::ActionRefused), "one of aio-rs's: unknown method");
# }
```

| `Code` (wire name) | Means |
|---|---|
| `AccessDenied` (`ACCESS_DENIED`) | the caller may not: not a `#[client]` method, its rule said no, or a server function's access |
| `ActionRefused` (`ACTION_REFUSED`) | no such method |
| `ConnectionLost` (`CONNECTION_LOST`) | the link dropped or never came up |
| `None` | any other refusal: the method's own (`ensure!`, `reject!`), a disabled or paused cell, the app closing |

A code keeps its meaning and wire name across versions; new ones may be
added (`#[non_exhaustive]`). A panic in a method refuses its call (the
state stays as it was) and is reported: a panic is a bug, `reject!` is
the expected failure. Framework messages say what happened and the one
thing to do about it.

## 2. Look at the running app

| Ask | Verb |
|---|---|
| is it up, and healthy? | `ax status <app>`, `ax health <app>` (exit 1: degraded, stuck, not running) |
| what did it log? | `ax logs <app> [word] [--level warn] [--since 15m] [--follow]`, `ax errors <app>` |
| what ran, with which arguments, by whom? | `ax events <app> [cell] [--diff]` |
| what does it hold? | `ax state <app> <cell> [path] [--watch]`, `ax snapshot <app>` |
| which migrations, what drifted? | `ax migrations <app>` |
| assert on it | `ax expect <app> <cell> <path> eq <value> [--wait 5]` |
| act as a user | `ax call <app> <cell> <method> [args] [--as user]` |
| step through its history | `ax timetravel <app> <cell> [undo\|redo\|goto N\|pause\|resume]` |
| replay a log into it | `ax replay <app> [range] [--from file]` |
| a test from what it just did | `ax record <app> [cell] <file.rs> [range]` |
| what problems did it report? | `ax report <app> [id]` |
| what may a caller call? | `ax peer <app> [--as user] [--role r]` |
| is it the code on disk? who set each setting? | `ax doctor <app>` |
| memory, sizes, links | `ax heap <app>`, `ax metrics <app>`, `ax cost <app>`, `ax clients <app>` |
| its window | `ax shot <app>`, `ax surface <app>`, `ax components <app> --renders`, `ax eval <app> <js>`, `ax trigger <app> <path> click` |
| schedules, tables | `ax schedules <app>`, `ax sql <app> "select …"` |

`ax ui` opens all of this in a window (the app manager). Every verb takes
`--json`; exit codes are 0 ok, 1 failed, 2 bad usage.

## 3. Logs

Everything an app writes to stderr also lands in `logs/app.log`, stamped
in UTC; errors also in `error.log`, warnings in `warning.log`. Those three
hold at most 3 × `keep` × `file_mb`, whatever the app does; `perf.log`
rotates at 1 MB, 5 kept.
Structured lines:

```rust
# fn main() {
aio::info!("saved");
let n = 3;
aio::warn!(cat: "net", peer = 7, "lost {n} frames"); // WARN  [net] lost 3 frames peer=7
# }
```

A line below `[log] level` costs one load and formats nothing; the same
line repeated is written once with a count; credentials are masked. A
line said during a method call ends with `call=<id>`, a number per call
this run (aio's correlation id): the `debug` line of the call itself, its
`[diff]`, the app's own lines from the method, its timeline entry
(`id`) and a report it causes (`call`) share it, so the lines ending
in `call=42` are what one call did.

```toml
[app]
id = "notes"

[log]
level = "info"              # trace, debug, info (default), warn, error
keep = 5                    # files kept per log
file_mb = 10                # a file rotates at this size
heartbeat = 3600            # seconds between heartbeat lines (uptime, calls, errors); 0: none
suppress = ["notes.tick"]   # methods whose calls are never logged, even at debug
per_run = false             # true: each run starts fresh files (app.1.log is the last run)
```

`ax logs <app> [word] [--level warn] [--tag net] [--since 15m]
[--client] [--follow]` reads them (`--client`: the window's lines);
`ax errors <app>` shows `ax dev`'s failed build, then the newest warnings
and errors, each under the start of the app it came from.

## 4. Time travel

More: [time travel](time-travel.md).

Every cell's log is its history: `h.travel(Some(TimeAsk::Undo))` steps
the state back one event, `Redo` forward, `Goto(seq)` to right after
event `seq`. A step pauses the cell, so it stays where travel put it:
calls are refused (never dropped as if they ran) and schedules held,
until `Resume`.

```rust
use aio::prelude::*;
use aio::TimeAsk;

#[cell(persist)]
pub struct Score {
    pub n: i64,
}

#[methods]
impl Score {
    pub fn add(&mut self, by: i64) {
        self.n += by;
    }
}

# fn main() {
let h = Handle::<Score>::memory();
for by in [1, 2, 3] {
    h.add(by).unwrap();
}
let tl = h.travel(Some(TimeAsk::Undo)).unwrap();
assert_eq!((h.state().n, tl.paused), (3, true));
assert!(h.add(10).is_err(), "paused: calls are refused, not lost");
h.travel(Some(TimeAsk::Redo)).unwrap();
assert_eq!(h.state().n, 6);
h.travel(Some(TimeAsk::Resume)).unwrap();
h.add(4).unwrap();
assert_eq!(h.state().n, 10);
# }
```

- `ax timetravel <app> <cell> [undo|redo|goto N|pause|resume]` does the
  same on the running app, and `ax ui` shows it as a timeline.
- `ax replay <app> [range] [--from file] [--dry]` calls a log's inputs
  again on the running app, in order: what they caused (effects,
  schedules) the app makes again. `--from` replays another store or a
  backup.
- `ax events <app> [cell] --diff` shows each event with the changes it
  made; `#[redact]` methods show `[redacted]`.
- A user-facing "replay the last game" is not this: record the inputs
  in a cell of their own and play them back as calls.
- `#[cell(diagnostics = false)]` keeps a hot cell's calls out of the
  live timeline and reports (they are still in its log).

## 5. Problem reports and health

More: [feedback](feedback.md), [production](production.md), every kind in
[errors](errors.md#problems-every-kind).

- A panic, a crashed or frozen window, or a part that keeps failing writes
  a report to `data/reports/<id>.json` before anything else happens
  (owner-only, credentials masked): the cells as saved, the newest calls
  (never their arguments), what the diagnostic bus heard, the build. The
  same problem is reported once per run, at most 10 automatic reports a
  run, and the newest `[feedback] keep` are kept.
- Nothing leaves the machine unless `[feedback] url` (https) or
  `aio::report::sink(within, f)` says where: then each report is sent on
  after it is on disk, never instead. `[feedback] auto = false` keeps
  only the user's own reports. `aio::feedback` is the "Report a problem"
  button's cell (sending, saved, delivered or why not).
- `aio::report::on_problem(|kind, msg| ..)` hears every problem as it
  happens, also warnings that are not reports; `aio::report::Code::of(kind)`
  types it (`panic`, `tripped`, `refused`, `effect:timeout`,
  `budget:method`, `storm`, `degraded:<part>`, `memory:high`,
  `window:crashed`, `update:failed`, `user`, `degraded-recovered:<part>`,
  `vitals:<layer>`) and `tip()` says what to do. Never call a cell from
  it: hand the work off.
- `aio::report::subscribe(|d| ..)` hears the same as a typed event (aio's
  `onDiagnostic`): `d.kind`, `d.message`, `d.severity` (`Error` for a
  problem, `Warn` for a warning or a vitals step up, `Info` for a
  recovery), `d.tip`, `d.code()`, `d.held` (repeats held back);
  `aio::report::recent()` keeps the newest 200.
- `data/health.json` says each part's state every 10 s; `ax health <app>`
  reads it and fails for a degraded, stuck or stopped app. A served app
  answers `GET /__aio/health`, `GET /__aio/metrics` (Prometheus) and `GET
  /__aio/vitals`: to this machine's tools without the key on an app
  without accounts or users, else to the key, a session or a user's
  token ([production](production.md#health)).
- Best-effort work of the app's own: `aio::health::part("thumbnails")
  .guard(result)` counts successes and failures; after 5 in a row the
  part is degraded (said once, reported, said again when it recovers).
  `aio::health::gauge(name, unit, read)` names what can grow, so a memory
  problem says the biggest.
- Memory (`[memory]`: `high`, `clear`, `critical` in percent of the
  machine, `growth_mb`): a process holding too much, or one that keeps
  growing, degrades.
- Vitals (`[vitals]`, aio's thresholds by default): how late the
  runtime's tick (`loop`), the effect queue (`queue`), a client's
  link (`transport`) and its paint (`render`) are, each `degraded`,
  `warning`, `frozen` from its own thresholds, with a hint naming the
  likely cause (`hints = true`). A frozen `loop` fails the app's health
(its `runtime` part).
- Perf budgets (`[perf]`, in ms): `method` (one frame, 16, in a debug
  build; 100 in a release), `effect` (5 per step), and per name
  `[perf.methods]` `"notes.import" = 2000`, `[perf.effects]`. A call
  slower than its budget is logged with where its time went
  (`logs/perf.log`). `[perf.storm]` (`rate`, `secs`, `breaker`) names a
  method called in a runaway loop, and with `breaker` refuses it while
  the storm lasts.
- Size budgets (`[budgets]`: `state`, `broadcast_rate`, `payload`):
  [big data](../persistence/sql.md#blobs-files-and-big-data).

```toml
[app]
id = "notes"

[feedback]
url = "https://reports.example.org/notes"
keep = 50
auto = true

[capabilities]
network = ["reports.example.org"]

[perf]
method = 50

[perf.methods]
"notes.import" = 2000

[perf.storm]
rate = 200
secs = 3
breaker = true
```

## 6. Reproduce, fix, prove

A bug becomes a failing test first: `ax record` writes one from the
running app, or boot the cell from the state before the bug with
`TestCells::seeded` and make the calls again ([testing](../testing/README.md)).
Then fix it until the test is green. Never widen a type, delete an
assertion, swallow an error or add a sleep-and-retry to make it pass.

## 7. Performance

[Performance](../performance.md): what a keypress costs, and how to
measure renders, writes and wire bytes. On the running app: `ax
components <app> --renders` (which components render, and how long),
`ax cost <app>` (what each cell sent its clients, and its methods' p95),
`ax heap <app> --watch` (memory, each cell's state and events).

## Going to production

- [ ] Every refusal a user can meet says the fix in its `why`.
- [ ] `[feedback] url` set (or `aio::report::sink`), so reports reach you;
  its host in `[capabilities] network`.
- [ ] A served app: `ax health` and `/__aio/metrics` watched;
  `[server] tls`, `hosts`, `strict_origin` set ([security](../auth/README.md#tls-and-the-network)).
- [ ] `[perf]` budgets that hold in a release build; `ax cost` checked
  for a cell that sends too much.
- [ ] Secrets `#[redact]`ed or `#[server]`; `ax lint` clean.
- [ ] A backup before the first release: `ax backup` ([files](../persistence/files.md#backups)).

## API

`cargo doc -p aio-rs --no-deps --all-features --open`: `aio::Rejected`,
`aio::Code`, `aio::TimeAsk`, `aio::Timeline`, `aio::report`,
`aio::health`, `aio::vitals`, `aio::perf`, `aio::feedback`, the logging
macros (`aio::info!`, `aio::warn!`…).

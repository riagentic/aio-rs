# Scaling

How far one aio-rs app goes, what limits it, the budgets it can declare,
and what to change when it is slow or big. For a view's cost per keypress
see [performance](../performance.md).

## How an app is built, at scale

```text
clients ── WebSocket / local door ──▶ the app (one process)
                                       ├─ cells: each its state in memory, behind its own lock
                                       ├─ effects: a tokio runtime
                                       └─ state.db (SQLite, WAL)
                                            ├─ each cell's log and snapshots
                                            └─ #[table] rows, aio::db tables, blobs
```

- One process, one machine. Each cell's calls run one at a time (a
  call copies the state, runs the method, checks the invariants,
  commits); different cells run side by side.
- A commit is an event in the log, written once per call (or once per
  100 ms for `#[coalesce]` runs); a snapshot every 256 events keeps a
  boot short.
- A client gets the whole state once, then patches: only the paths that
  changed, filtered per viewer by `visible` and `#[server]`. A patch
  that touches nothing a viewer sees isn't sent to that viewer.
- Slow work belongs in effects, on the tokio runtime, off the cell's
  lock; their results come back as calls.

## What limits it

| Limit | Why | Do |
|---|---|---|
| the state in memory | all of a cell's state is held, and copied by each call | keep the active set in state, the rest in [`#[table]` or `aio::db`](../persistence/sql.md); `aio::imbl` collections or `Arc` fields make the copy O(1) |
| one cell's calls in a row | a cell is a lock | split a hot cell; move slow work to an effect |
| pushes to many clients | each commit becomes one patch per viewer that sees the change | `visible` per viewer, `#[coalesce]`, fewer fields that change on every call |
| SQLite's one writer | every commit is a write | `#[coalesce]` frequent calls; batch rows in one transaction |
| one process | no shared state between machines | one app per data set; [sync](../state/sync.md) copies are clients, not servers |

## The server's limits

`[server.limits]` bounds each link. Every limit tells the sender: a
client past `clients` gets 503 with the reason, a message over
`max_message` a close (1009) naming the limit, and a fast sender is read
more slowly, so nothing is dropped. Clients learn the limits when they
connect and pace themselves.

| Key | Default | Means |
|---|---|---|
| `clients` | 256 | open links at most |
| `max_message` | 4 MB | the largest message a client may send |
| `calls_per_sec` | 200 | calls per link per second (bursts up to twice that) |
| `bytes_per_sec` | 8 MB | bytes per link per second (bursts up to twice that) |

```toml
[app]
id = "shop"
mode = "server"

[server]
listen = "0.0.0.0:8100"

[server.limits]
clients = 2000
calls_per_sec = 50
```

Other fixed bounds: a route's request body is at most 2 MB unless the
route says (`Route::max_body`); a synced client keeps at most 500 calls
waiting for the server, then refuses new ones (`[client] retention` lets
it drop the oldest past an age instead); a confirmed call waits 15 s for
its answer (`[client] timeouts` per method); an await in an async
method is cut off after 30 s unless the method is `#[long]`.

## Budgets: the times

`[perf]` sets how long a call may take. Over it, the call still commits
(budgets observe, never refuse), and it is said: a `WARN [perf]` line
with the fix (at most once per method per 10 s), a line in
`logs/perf.log` with where the time went (`clone`, `apply`, `check`,
`commit`), and the `on_problem` hook (`budget:method`, `budget:effect`).

| Key | Default | Counts |
|---|---|---|
| `method` | 16 ms in a debug build (a frame), 100 ms in a release | a method's own work: the state's copy, the method, the invariants, not the disk |
| `effect` | 5 ms | one step of an effect (the time it holds a worker thread between awaits), not the calls it makes |
| `[perf.methods]` | none | per method, `"cell.method" = ms`: one meant to be slow, while the rest stay strict |
| `[perf.effects]` | none | per effect, `"cell.name" = ms` |
| `[perf.storm]` `rate`, `secs`, `breaker` | 200, 5, `false` | a method called `rate` times a second for `secs` seconds is a storm (a feedback loop); `breaker` refuses its calls while it lasts |

```toml
[perf]
method = 50

[perf.methods]
"shop.import" = 2000

[perf.storm]
rate = 100
secs = 3
breaker = true
```

A `[perf.methods]` key that names no method of its cell is said at boot:
a typo would budget nothing.

Slow work off the call:

```rust
use aio::prelude::*;

#[cell]
pub struct Report {
    pub rows: Vec<u64>,
    pub total: Option<u64>,
    pub busy: bool,
}

#[methods]
impl Report {
    /// Fast: flags the work and hands it to an effect.
    pub fn sum(&mut self, cx: &Cx) {
        self.busy = true;
        let rows = self.rows.clone();
        cx.run(Effect::new("sum", move |h: Handle<Report>| async move {
            // CPU work off the runtime's threads too.
            let total = tokio::task::spawn_blocking(move || rows.iter().sum()).await.unwrap_or(0);
            _ = h.summed(total);
        }));
    }

    pub fn summed(&mut self, total: u64) {
        (self.total, self.busy) = (Some(total), false);
    }
}

# fn main() {
let mut t = aio::testing::TestCells::new();
let h = t.seeded::<Report>(serde_json::json!({ "rows": [2, 3] }));
h.sum().unwrap();
assert!(h.state().busy, "the call returned at once");
t.settle();
assert_eq!((h.state().total, h.state().busy), (Some(5), false));
# }
```

## Budgets: the sizes

`[budgets]` sets the sizes and rates the app commits to, in the units a
person writes:

| Key | Example | Measures |
|---|---|---|
| `state` | `"1MB"` | one cell's saved state (its JSON) |
| `payload` | `"500KB"` | one push to one client |
| `broadcast_rate` | `"20/s"` | pushes per second to one client |

```toml
[budgets]
state = "1MB"
payload = "500KB"
broadcast_rate = "20/s"
```

- Sizes take `B`, `KB`, `MB`, `GB` (1024-based) or a number of bytes;
  rates `20/s`, `20 per second` or a number. A value that doesn't read
  (`"1 XB"`, `"0"`) fails the build, naming the key: a budget that fell
  back to a default would be a limit nobody declared.
- **A breach fails, not just warns.** It is said once in the log and to
  the `on_problem` hook (`budget:state` …), kept with its worst reading,
  and it turns `/__aio/health` and `ax health` degraded (exit 1), so a
  test or a CI step can fail on it. A later good reading doesn't erase
  it.
- An app that declares none has no verdict (`budgets: null`), never a
  green tick for a promise nobody made. Without them aio-rs still says
  pressure it sees, as `WARN [vitals]` lines naming the fix: a push of
  500 KB or more, more than 30 pushes or 1 MB a second to one client.

What can grow without bound gets a name and a ceiling in code:

```rust
use std::num::NonZeroU64;

# fn main() {
let imports = aio::health::budget("scaling-page.imports", NonZeroU64::new(3).unwrap());
for _ in 0..3 {
    imports.spend(1).unwrap();
}
let e = imports.spend(1).unwrap_err();
assert!(e.why.starts_with("MEMORY_UNBOUNDED: scaling-page.imports passed its ceiling"));
imports.reset(); // a per-session series: the ceiling stays

assert_eq!(aio::budgets::parse_size("1.5 MB", "state"), Ok(1_572_864));
# }
```

`aio::health::gauge(name, unit, read)` names a size that rises and falls
(a queue, a cache), so `health.json` lists it and a memory problem names
the biggest ([production](../debugging/production.md)).

## Tuning by case

### Big state (over 1 MB)

| Do | Why |
|---|---|
| rows in `#[table]` fields or `aio::db`, a page of them in state | state is memory, copied per call, pushed to clients |
| `visible` per viewer, `#[server]` fields | each client gets what it needs |
| `#[transient]` for caches and derived data | not written on every commit |
| `aio::imbl` collections (feature `imbl`) or `Arc` fields | a call's copy shares what didn't change |
| `[budgets] state` and `payload` | a size that is meant, said as such |

[Big data](../persistence/sql.md#blobs-files-and-big-data) has blobs and
files.

### Many clients (over 100)

| Do | Why |
|---|---|
| `visible` per viewer | smaller patches, and none to who sees nothing changed |
| `#[coalesce]` on frequent writers | one write and one push per 100 ms run |
| `[server.limits] clients` raised | 256 by default |
| `ax cost <app>` | what each cell sends, full resends, its methods' p95 |

### Frequent calls (over 10 a second)

| Do | Why |
|---|---|
| `#[coalesce]`, or `cx.after` with one key (debounce) | fewer commits |
| keystrokes in a view's own signal, a call on submit | no call per key |
| bulk rows written by an effect in one transaction, a summary in state | one commit for the batch |
| `[perf] method = 20` | catch slow methods early |
| `[perf.storm] breaker = true` | a runaway loop can't eat the machine |

### Long lists

Keep the list in a `#[table]` field or `aio::db` and the current page in
state; key rows by id in a map rather than a `Vec` when single rows
change; split a heavy collection into a cell of its own. Windowed views:
[performance](../performance.md#4-long-lists).

### Long-running desktop apps

`#[transient]` for what the window alone needs, `[perf]` on, `ax heap
<app> --watch` once in a while; `[memory]` thresholds say growth before
it hurts ([production](../debugging/production.md)).

## Measuring

aio-rs has no runtime throughput numbers published yet: measure your
app, on your machine.

| What | How |
|---|---|
| what each cell sends, and its methods' p95 | `ax cost <app> [cell] [--keys]` |
| memory, each cell's state and events | `ax heap <app> --watch` |
| uptime, links, schedules, sizes | `ax metrics <app>`, `GET /__aio/metrics` (Prometheus) |
| which components render, and how long | `ax components <app> --renders` |
| calls over budget, with where the time went | `logs/perf.log` |
| renders and wire bytes in a test | `TestUi::components()`, `TestUi::wire_bytes()` |
| many clients against one server | `aio::testing::TestServer` and `aio::testing::Sim` ([testing](../testing/README.md)) |

What is measured, in aio-rs's own repository:

- `cargo test -p aio-rs --test list_speed -- --nocapture`: a held key
  over a 1000-row list, renders, writes and wire bytes per key
  ([performance](../performance.md#measured)).
- `scripts/dx-metrics` (in an aio-rs checkout) measures a fresh template
  app: clean and incremental builds, the release build, binary and bundle
  sizes, boot time, the idle window's memory, and save-to-screen times
  under `ax dev`; each run is a row of `scripts/dx-trend.csv`, and
  `--check` fails on a regression past its limit, judged against the same
  machine's rows only (in CI: against the base commit measured on the same
  runner). The latest row (Linux
  x86_64): a 0.2 to 0.6 s boot, about 81 MB private memory for the window
  (166 MB PSS), a 5.4 MB release binary, a 3.6 MB AppImage.
- [Comparison](../comparison.md): one small app in aio-rs, Tauri and
  Dioxus, measured once each.

## Not designed for

- Many machines sharing one app's state: there is no shared-state
  protocol between servers.
- Pages for search engines: views render in the client (no server-side
  HTML of the app's state).
- Stateless APIs: an app is its cells; a server function or route is
  the way to answer without state.

## Not in aio-rs

- aio's render-staleness backpressure (a page whose paint lags is sent
  less): aio-rs paces every link the same (`[server.limits]
  sync_interval_ms`, aio's `syncIntervalMs`) and sends a big patch as the
  view (`full_state_percent`, aio's `fullStateThreshold`).
- aio's `perfCheck: "off"` switch: aio-rs's budgets are always on and
  cost a clock read per call.
- A benchmark gate on the runtime (aio's D12 perf gate): aio-rs gates the
  developer loop (`scripts/dx-metrics --check`) and list costs in tests.

## API

`cargo doc -p aio-rs --no-deps --all-features --open`: `aio::config`
(`Limits`, `Budgets`), `aio::perf`, `aio::budgets`, `aio::health`
(`budget`, `gauge`), `aio::imbl`, `aio::testing::TestServer`.

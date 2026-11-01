# Production

What a released app tells you about itself, and what to watch: health,
metrics, memory, vitals, budgets, and how it behaves when something
around it breaks. The order of work when something is already wrong is
in [debugging](README.md); the errors themselves in [errors](errors.md).

## Going to production

- [ ] Every refusal a user can meet says the fix in its `why`
  ([errors](errors.md#refusals)); no `unwrap` on input.
- [ ] Reports reach you: `[feedback] url` (its host in `[capabilities]
  network`) or `aio::report::sink` ([feedback](feedback.md)), and
  you've told your users a report holds app state.
- [ ] Something watches health: `ax health <app>` in a timer or
  supervisor, or `GET /__aio/health` and `/__aio/metrics` scraped
  ([below](#health)).
- [ ] A served app: `[server] tls`, `hosts` and `strict_origin` set, the
  key or accounts in place ([security](../auth/README.md)).
- [ ] `[perf]` budgets that hold in a release build, `[budgets]` for the
  sizes you promise ([scaling](../build/scaling.md)); `ax cost <app>`
  checked for a cell that sends too much.
- [ ] `[memory]` thresholds fit the machine the app runs on.
- [ ] `[data] durability = "full"` if losing the last second of calls to
  a power cut is not acceptable ([persistence](../persistence/README.md#durability)).
- [ ] Secrets `#[redact]`ed or `#[server]` ([secrets](../auth/secrets.md));
  `ax lint` clean.
- [ ] A backup before the first release, and one before each data
  migration: `ax backup <app>` ([files](../persistence/files.md#backups)).
- [ ] Updates signed and the rollback tried ([release](../build/release.md)).

## What an app says at start

Its first lines say what it is and who decided each setting: `aio.toml`
(built into the app), an `AIORT_*` variable, a file in its data, or a
default.

```text
[aio] notes 1.4.0 starting (pid 41822)
[aio] boot app        notes (aio.toml)
[aio] boot version    1.4.0 (Cargo.toml)
[aio] boot aio-rs     0.1.0 (built in)
[aio] boot mode       server (aio.toml)
[aio] boot data       /srv/notes/data (AIORT_DATA)
[aio] boot durability normal (aio.toml)
[aio] boot listen     0.0.0.0:8100 — every interface: open to the network (aio.toml)
[aio] boot tls        on (aio.toml)
[aio] boot token      data/sync.token (file)
```

`ax doctor <app>` says the same of a running app, and whether it is the
code on disk (version, build, edits since). A setting that was refused
stops the start instead: `AIORT_CONFIG` is refused, never ignored (an app
reads only the `aio.toml` it was built with).

## Health

Every 10 s the app writes `data/health.json` (owner-only):

```json
{
  "status": "degraded",
  "appId": "notes",
  "version": "1.4.0",
  "aio": "0.1.0",
  "pid": 41822,
  "startedAt": "2026-10-28T09:00:00.000Z",
  "updatedAt": 1761645000000,
  "uptimeS": 3000,
  "memory": { "level": "ok", "rssBytes": 73400320, "machinePct": 1 },
  "degraded": [{ "name": "thumbnails", "failures": 5, "since": "2026-10-28T09:40:00.000Z", "lastError": "disk full" }],
  "clientDegraded": [],
  "gauges": [],
  "budgets": null,
  "cpuMs": 5120
}
```

| `status` | Means | `ax health` |
|---|---|---|
| `healthy` | nothing degraded, memory ok, no budget breached | exit 0, `notes is healthy: pid …, up … s, … MB (…% of RAM)` |
| `degraded` | a part degraded, a client says one of its parts is, memory high, growing or critical, or a `[budgets]` breach | exit 1, each reason |
| `stopping` | it has begun to stop | exit 1 |
| `maintenance` | `ax restore` holds it | exit 1, naming the verb |
| (stale) | no update for 30 s | exit 1, `stuck` |
| (gone) | its pid is not running | exit 1, `not running` |

A served app answers `GET /__aio/health` with the same document, made
fresh, plus `cell` (`id`, `enabled`, `failures`), and `GET
/__aio/metrics` in Prometheus' text format; both `Cache-Control:
no-store`. They describe the app (its cells, sizes, clients, errors), so
beyond this machine they need the app's key (`Authorization: Bearer`),
a signed-in session or a user's token; on this machine (loopback, not
relayed by a proxy) an app without accounts or users answers without
one. `[server] open` opens them to anyone.

```sh
ax health notes || systemctl restart notes   # a timer's check
curl -s -H "Authorization: Bearer <key>" https://notes.example.org/__aio/health
```

### Parts of your own

A part is best-effort work that may fail now and then (a cache refill, a
thumbnail, a webhook). One failure is nothing; the same part failing
again and again is a dead feature. `aio::health::part(name)` counts
failures in a row: after 5 (`.after(n)` changes it) the part is
degraded, said once in the log, reported (`degraded:<name>`), and said
again when it recovers. Nothing in between: no line per failure.

```rust
# fn main() {
let thumbs = aio::health::part("production-page-thumbs");
let made: Option<Vec<u8>> = thumbs.guard(std::fs::read("/no/such/thumb.png")); // None on failure
assert!(made.is_none() && thumbs.failures() == 1);

// Or say each outcome yourself:
let hook = aio::health::part("production-page-webhook").after(3);
for _ in 0..3 {
    hook.fail("503 from the receiver");
}
assert!(hook.is_degraded());
hook.ok(); // recovered: said once, the count back to 0
assert!(!hook.is_degraded());
# }
```

`aio::health::degraded()` lists what is degraded now. A client linked to
a server sends its own degraded parts along; the server's health shows
them under `clientDegraded` (how many clients say each), as long as the
link lives.

### Named gauges and ceilings

Name what can grow, so a memory problem names the biggest instead of
"the process is big":

```rust
use std::num::NonZeroU64;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

# fn main() {
let cache = Arc::new(AtomicU64::new(0));
let c = cache.clone();
// Read each time health samples: keep it cheap.
aio::health::gauge("production-page.cache", aio::health::Unit::Bytes, move || c.load(Ordering::Relaxed));
cache.store(3 << 20, Ordering::Relaxed);

// A counter with a ceiling: past it, refused by name.
let imports = aio::health::budget("production-page.imports", NonZeroU64::new(1000).unwrap());
imports.spend(999).unwrap();
let e = imports.spend(2).unwrap_err();
assert!(e.why.starts_with("MEMORY_UNBOUNDED: production-page.imports passed its ceiling"), "{}", e.why);
imports.reset(); // a per-session series: the ceiling stays

let g = aio::health::gauges();
assert!(g.iter().any(|r| r.name == "production-page.cache" && r.value == Some(3 << 20)));
# }
```

- A gauge is a `level` (a size now, up and down) or, from `budget`, a
  `counter` (work done, only up, with its ceiling as `bound`). Names are
  stable and dotted, the owner first (`sync.queue`).
- They are listed in `health.json` and `/__aio/metrics`; a memory problem
  says the biggest byte gauges and the biggest cells.
- One per thing, not one per item: the number of gauges is bounded.

## Metrics

`GET /__aio/metrics`:

| Metric | Type | Labels | What |
|---|---|---|---|
| `aio_up` | gauge | | 1 healthy, 0 degraded |
| `aio_uptime_seconds` | gauge | | since the server started |
| `aio_memory_rss_bytes`, `aio_memory_machine_bytes` | gauge | | the process's resident memory, the machine's |
| `aio_clients_connected` | gauge | | open sync links |
| `aio_calls_total` | counter | | method calls this run |
| `aio_cell_enabled` | gauge | `cell` | 1 enabled, 0 tripped |
| `aio_cell_failures` | gauge | `cell` | failures since it was last enabled |
| `aio_degraded_parts` | gauge | | parts degraded now |
| `aio_degraded_failures` | gauge | `part` | failures in a row of each degraded part |
| `aio_gauge` | gauge | `name`, `unit`, `counter` | each named gauge |
| `aio_gauge_limit` | gauge | `name` | each budget's ceiling |

On the machine, without a server: `ax metrics <app>` (uptime, links,
schedules, each cell's size), `ax heap <app> [--watch]` (memory, each
cell's state and events), `ax cost <app>` (what each cell sent its
clients, its methods' p95), `ax status <app>`.

## Memory

Every 10 s the app samples its resident memory against the machine's:

| `[memory]` key | Default | Means |
|---|---|---|
| `high` | 50 | percent of the machine: from here the app is `high` (degraded) |
| `clear` | 40 | percent: below it, `high` clears (no flapping at the line) |
| `critical` | 80 | percent: the machine is about to swap or kill it |
| `growth_mb` | 256 | MB: growth is said once per step this big (or a tenth of the machine) |

`growing` means 10 samples in a row (100 s) each at least as big as the
last, and past the start by a step (the bigger of `growth_mb` and a
quarter of the start). Each level is said once in the log and reported
(`memory:high`, `memory:growing`, `memory:critical`), with the biggest
cells and gauges named; the next growth step is said again, so a leak is
said once per step, not once per sample.

```toml
[memory]
high = 30
clear = 25
critical = 70
growth_mb = 128
```

Common causes: a `Vec` in state that only grows (keep it bounded, or move
it to a `#[table]`), a cache with no eviction, a `#[ttl]` method's answers
(at most 5000 per cell).

## Vitals

How far behind each layer is, graded against `[vitals]`:

| Layer (`[vitals]` table) | Measures | Probed by | degraded / warning / frozen |
|---|---|---|---|
| `loop` | how late the runtime's tick is: a call or an effect step holding it | the app, every second | 100 / 500 / 2000 ms |
| `queue` | effects waiting their turn | the app, every second | 50 / 200 / 1000 effects |
| `transport` | how long a client has been unheard | each link | 100 / 500 / 2000 ms |
| `render` | how long a client's paint took | the window's page | 50 / 200 / 2000 ms |

A step up to warning or frozen is said in the log, with a hint when
`hints = true` (the default): the likely cause, what shows it, the fix,
and how sure (`likely`: two probes agree; `possible`: one measured it;
`speculative`). The way back is said as recovered. A frozen `loop` fails
the `runtime` part, so health says degraded.

| Hint | Said when |
|---|---|
| a call took N ms | the paint froze right after that call, and it ran past `loop`'s degraded |
| the effect queue backed up | the queue passed `queue`'s frozen before the link stalled: debounce rapid calls (`cx.after` with one key) or `Mode::Newest` |
| the link stalled | the transport froze with the app and the paint healthy; on loopback that means the client's own main thread was busy, not the network |
| the client's main thread was blocked | the paint froze with no aio call before it: third-party scripts, large DOM work |
| a freeze-recover cycle | the paint froze and recovered more than once in 30 s: something heavy runs on each recovery |

A hidden window's frozen paint gets no hint. Each table needs all three
numbers:

```toml
[vitals]
hints = true

[vitals.loop]
degraded = 50
warning = 250
frozen = 1000

[vitals.render]
degraded = 32
warning = 100
frozen = 1000
```

## Budgets

Time budgets (`[perf]`, in ms) and size budgets (`[budgets]`) are
observations: a slow call still commits. Each breach goes to `logs/perf.log`
(with where a method's time went: `clone`, `apply`, `check`, `commit`), a
`WARN [perf]` line with the fix (at most once per name per 10 s), and the
`on_problem` hook (`budget:method`, `budget:effect`, `budget:<size>`).

| Key | Default | Counts |
|---|---|---|
| `[perf] method` | 16 in a debug build, 100 in a release | a method's own work: the state's copy, the method, its invariants (not the disk) |
| `[perf] effect` | 5 | one step of an effect: the time it holds a worker thread (not the calls it makes) |
| `[perf.methods]` `"cell.method" = ms` | | one method meant to be slow |
| `[perf.effects]` `"cell.name" = ms` | | one effect, by the name its key starts with |
| `[perf.storm]` `rate`, `secs`, `breaker` | 200, 5, false | a method called `rate`/s for `secs` s in a row is a storm (a feedback loop); `breaker` refuses its calls while it lasts |
| `[budgets] state` | none | one cell's saved state (`"1MB"`, `"512KB"`, bytes) |
| `[budgets] broadcast_rate` | none | state pushes per second to one client (`"20/s"`) |
| `[budgets] payload` | none | one push to one client (`"500KB"`) |

A `[budgets]` breach keeps its worst reading, is said once, and turns
health degraded (`budgets.ok = false`), so a test or a CI step can fail on
it; an app that declares none has `budgets: null`, never a green tick for
a promise nobody made. A `[perf.methods]` key that names no method of its
cell is said when the cell opens (a typo budgets nothing). More in
[scaling](../build/scaling.md).

## Cells

Each cell is `enabled` or not, with its `failures` since it was last
enabled and its last call: `ax status <app>`, the `cell` of
`/__aio/health`, `aio_cell_enabled`. A cell whose state has a `status`
field shows it. A tripped cell (its [breaker](errors.md#the-breaker)) is a
degraded part named `cell <id>` until `Cells::enable` turns it back on.

## When things around it break

| What happens | What aio-rs does | What you do |
|---|---|---|
| the app is killed, or the machine loses power, mid-call | each call commits in one SQLite transaction: it is in the store whole or not at all; with `durability = "normal"` the last calls before a power cut may be lost, never half-written | `durability = "full"` where that matters |
| killed during a coalesced run | a `#[coalesce]` run is on disk within 100 ms; a kill inside that window loses that run | a plain method for what must be durable when it returns |
| a client's link drops during a call | the call's caller gets `ConnectionLost`: it may have run | make the call safe to send twice, or read the state first |
| a client is offline | its calls wait (at most 500) and are sent once on reconnect; a full queue refuses new ones | [sync: offline](../state/sync.md#offline-and-reconnects) |
| the server restarts with clients connected | clients reconnect by themselves (backing off with jitter) and catch up from where they were | nothing; `ax clients <app>` shows them back |
| an effect hangs | its `.timeout(…)` cancels it (`effect:timeout`); at shutdown, effects still running at the deadline are aborted and named | a timeout on every outbound effect |
| a method is called in a loop | a storm is said; with `[perf.storm] breaker` its calls are refused meanwhile | find what calls it in reply to its own change |
| a cell keeps failing | the breaker disables it, state kept | fix, then `Cells::enable` |
| the window's renderer dies | `window:crashed`, the page reloads; 4 crashes in a minute close it | `ax logs <app> --client`, the report |
| the window stops answering | `window:frozen` reported, `window` degraded | `ax components <app> --renders`, vitals |
| an update fails | `update:failed` reported, the running version kept | [release](../build/release.md) |
| a second launch | it hands over to the running copy and ends | `--instance`, `--takeover` |

## Not in aio-rs

- aio's V8 heap limits and heap snapshots (`v8Flags`): aio-rs has no
  garbage-collected heap; memory is the process's resident set.
- aio's startup linter of cell config at `aio.run()`: aio-rs checks cell
  flags when the app compiles (the macros) and `aio.toml` when it
  builds; `ax lint` adds what types can't check.
- aio's health overlay in the page: a debug page shows its errors
  ([errors: in the window](errors.md#in-the-window)); health is `ax
  health`, `ax ui` and `/__aio/health`.
- aio's "uses reserved key(s)" and "already bound" errors have no
  counterpart: a cell is a Rust type, its fields are its own.

## API

`cargo doc -p aio-rs --no-deps --all-features --open`: `aio::health`
(`part`, `Part`, `gauge`, `budget`, `degraded`, `prometheus`, `Memory`),
`aio::vitals` (`Snapshot`, `Hint`, `hint`), `aio::perf`, `aio::budgets`,
`aio::boot`, `aio::config::Memory`.

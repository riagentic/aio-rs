# Performance

What a keypress costs decides how an app feels: a key held over a long
list fires 30 times a second. aio-rs keeps that cost to a frame, not a
commit, with the same tools aio apps rely on. Use them from the
first line: a view written against the whole state works, and then gets
slower with every field the app adds.

1. **Read fields, not the state**: `part(|c| &c.field)`.
2. **Coalesce frequent writes**: `#[coalesce]`, `#[cell(coalesce)]`.
3. **Memoize what is derived**: `#[derived]`, `#[derived(reads = [..])]`,
   `aio::Cache`.
4. **Draw a window of a long list**, rows by props, the cursor by
   attribute.
5. **Let keys and pointers through the page cheaply**: `data-aio-prevent`,
   `data-aio-reveal`, held-key repeats, throttled pointer moves.
6. **Keep methods short**, heavy work off the cell's thread, and let the
   budgets (`[perf]`, `[budgets]`, `[vitals]`) say when they aren't.

## What a view follows

A view's body runs once. What follows state is its holes: each text,
attribute, child, `if` and `for` of its `rsx!` that isn't a literal is an
effect that runs again when something it read changes, and writes only its
own part of the page. A read in the body itself (a `let` before `rsx!`)
happens once. A hole collects its reads again each run, so a branch not
taken this time follows nothing, and one taken for the first time follows
from then on.

| Where the read happens | Followed? |
|---|---|
| a hole of `rsx!` (`"{n}"`, `{x.get()}`, `disabled: !ok.get()`, an `if`, a `for`), a `memo`, an `effect` | yes |
| `part(\|c\| &c.field)` read in a hole | that one field |
| `name()` (the whole state) in a hole | every commit of the cell |
| the component's body, before `rsx!` | no: read once, as it mounts |
| an event handler (`onclick`, `oninput`…), `act!` | no |
| `spawn(async { .. })`, after an `.await`, a timer | no |
| `.get_untracked()`, `.read_untracked()` of a signal, anywhere | no |
| a cell method or an effect (they write state, they don't render) | no |
| a value kept outside a view (a `static`, `once`) | no: it is made once |

- `part` is per top-level field: reading two fields of a cell follows
  those two, not the cell (in aio one cell is one signal, so aio splits
  state across cells; aio-rs splits it across fields).
- A cache that answers before reading still follows the right thing when
  its key is a part's stamp: `ROWS.get(part(|c| &c.rows).ver(), ..)` in a
  hole or `memo` reads the part first, then asks the cache
  ([memoize](#3-memoize)).
- A stale view: is the read in a hole or a `memo` (not the body, a
  handler, a `once`)? Is it a `part` of the field that changed (not a
  reference into it)? Does the root call `use_live()` (for commits from
  effects, sync and schedules)?

## 1. Read fields

```rust,no_run
use aio::ui::prelude::*;

#[aio::cell(persist, id = "inbox")]
pub struct Inbox {
    pub mails: Vec<String>,
    pub cursor: usize,
    pub unread: u32,
}

#[aio::methods]
impl Inbox {}

aio::client!(Inbox, inbox);

#[component]
fn Badge() -> View {
    // Its hole runs again when `unread` changes, never on a cursor step.
    let n = part(|i| &i.unread);
    rsx! { span { "{n}" } }
}

#[component]
fn Slow() -> View {
    // Its hole runs again on every commit of the cell: avoid.
    rsx! { span { {inbox().unread} } }
}
# fn main() {}
```

`part` makes a reader of one top-level field (aio's signal per cell;
`#[cell]` lays the fields out and says which changed): a hole reading it
follows that field. Pick the field, then index it: `part(|c| &c.rows)[i]`,
not a part inside a row. A dev build names, once, each view body that
reads the whole state (`[hint]` in the log): read there, it is read once.
Handlers may read `inbox()` freely: they don't subscribe.

Split state that changes at different rates into fields (or cells): a
cursor that moves 30 times a second next to rows that change once a
minute.

## 2. Coalesce frequent writes

`#[coalesce]` on a cursor, scroll or selection method, or
`#[cell(persist, coalesce)]` for every plain method (the app template's
default): each call is shown at once, and a run of them is written in one
transaction within `aio::PERSIST_DEBOUNCE` (100 ms), as aio's
`persistDebounceMs` does. Details and the crash window:
[persistence](persistence/README.md#coalesced-writes).

Commits that don't come from the views (effects, schedules, other
threads: prices, sync, file scans) reach the views at most once per
`aio::ui::BACKGROUND` (150 ms), as aio's broadcast coalescer does. A call from the
views shows at once and takes along what came meanwhile (aio's
`flushAllUrgent`).

## 3. Memoize

```rust
use aio::prelude::*;
use std::collections::BTreeMap;
use std::sync::Arc;

#[cell]
pub struct Shelf {
    pub books: Vec<(String, String)>, // author, title
    pub cursor: usize,
}

#[methods]
impl Shelf {
    pub fn add(&mut self, author: String, title: String) {
        self.books.push((author, title));
    }

    pub fn down(&mut self) {
        self.cursor += 1;
    }

    /// aio's `memoLast(() => [s.books], ..)`: the same answer, the same `Arc`,
    /// while `books` stays the same; a cursor step reuses it.
    #[derived(reads = [books])]
    pub fn by_author(&self) -> Arc<BTreeMap<String, Vec<String>>> {
        let mut m: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (a, t) in &self.books {
            m.entry(a.clone()).or_default().push(t.clone());
        }
        Arc::new(m)
    }
}

/// aio's caches on an array's identity (`groupBy`, an owner index, a
/// frame's panel rows): answers per key while their source stays the same.
static TITLES: aio::Cache<aio::Ver<Shelf>, String, Arc<Vec<String>>> = aio::Cache::new();

fn titles_of(state: &Arc<Shelf>, author: &str) -> Arc<Vec<String>> {
    TITLES.get(aio::Ver::of(state), author.to_string(), || {
        Arc::new(state.by_author().get(author).cloned().unwrap_or_default())
    })
}

# fn main() {
let h = Handle::<Shelf>::memory();
h.add("Le Guin".into(), "The Dispossessed".into()).unwrap();
let groups = h.state().by_author();
h.down().unwrap();
assert!(Arc::ptr_eq(&groups, &h.state().by_author()), "kept across the cursor step");
let st = h.state();
assert!(Arc::ptr_eq(&titles_of(&st, "Le Guin"), &titles_of(&st, "Le Guin")), "one answer per key and state");
# }
```

- `#[derived]` alone keeps one answer per state (aio's `createSelector`).
- `#[derived(reads = [a, b])]` keeps it while those fields stay the same
  (aio's `memoLast`).
- `aio::Cache<S, K, R>`: `get(source, key, f)` keeps each key's answer
  while `source` is the last one's. In a view, key it on a part's stamp:
  `ROWS.get(part(|c| &c.rows).ver(), panel, || build(..))`, so the views
  and the key handler share one build per change of the rows.
- Return `Arc<…>` from derived values: equal props then compare by
  pointer, and the rows that didn't change don't render.

## 4. Long lists

```rust,no_run
use aio::act;
use aio::kit::{GlobalKeys, use_global_key};
use aio::ui::prelude::*;

const SHOWN: usize = 30; // rows drawn around the cursor

#[aio::data]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row {
    pub name: String,
    pub amount: u64,
}

#[aio::cell(persist, id = "ledger")]
pub struct Ledger {
    pub rows: Vec<Row>,
    pub cursor: usize,
}

#[aio::methods]
impl Ledger {
    #[coalesce]
    pub fn down(&mut self) {
        self.cursor = (self.cursor + 1).min(self.rows.len().saturating_sub(1));
    }
}

aio::client!(Ledger, ledger);

/// A row by props: unchanged rows are skipped; `selected` follows the
/// cursor (a `Prop`), so a step re-runs the holes of two rows.
#[component]
fn Line(name: String, amount: u64, selected: Prop<bool>, i: usize) -> View {
    rsx! {
        li {
            "aria-selected": selected.get(),
            // Scrolled into view as its frame is applied: no script per step.
            "data-aio-reveal": selected.get().then(|| i.to_string()),
            "{name} {amount}"
        }
    }
}

#[component]
fn Lines() -> View {
    use_global_key("ArrowDown", |_| _ = act!(down()));
    let (rows, cursor) = (part(|l| &l.rows), part(|l| &l.cursor));
    // The rows shown: the list's hole runs again only when the window moves.
    let shown = memo(move || {
        let from = cursor.get().saturating_sub(SHOWN / 2);
        from..(from + SHOWN).min(rows.with(Vec::len))
    });
    rsx! {
        ul { "data-aio-prevent": "ArrowUp ArrowDown PageUp PageDown Home End",
            for i in shown.get() {
                Line {
                    key: "{i}",
                    name: rows.with(|r| r[i].name.clone()),
                    amount: rows.with(|r| r[i].amount),
                    selected: cursor.get() == i,
                    i,
                }
            }
        }
    }
}

#[component]
pub fn App() -> View {
    use_live();
    rsx! { GlobalKeys { Lines {} } }
}
# fn main() {}
```

- Draw a window (aio's `useVirtualList`, or a window by hand):
  `kit::VirtualList` for fixed-height rows, or a window by hand as above
  with stable keys.
- A `for` row takes `key:` on its first node (`rsx!` refuses one
  without): rows are kept by key when the list shifts (a window moving up,
  a row removed), and an item that changed (by `PartialEq`) gets a new row.
  Give a row what changes per step as a `Prop` (`selected: cursor.get() == i`)
  so a step re-runs that row's holes, not the list.
- Show the cursor by attribute (`aria-selected`, `data-cursor`) and CSS,
  with `transition: none` on rows: a step changes two attributes.
- Clamp the cursor in the view (`cursor.min(len - 1)`), not by a write,
  when a list shrinks.
- `data-aio-reveal` scrolls the row into view with `block: nearest`,
  `behavior: auto` (smooth scrolling piles up under key repeat).

## 5. Keys and pointers

- `data-aio-prevent="ArrowDown PageDown Home End Tab"`: the page keeps
  those keys from the browser inside the element; `prevent_default()` in a
  handler comes too late in a window.
- Held keys: a binding fires once while held unless it is movement or
  `Chord::repeating(true)` (aio's `REPEATABLE_KEYS`). When the views fall
  behind, waiting repeats are dropped for the newest, so the cursor stops
  when the key is let go.
- `WindowEvent::MouseMove` comes at most every `kit::MOVE_MS` (100 ms),
  the page throttling it (aio's idle-lock throttle).
- A resize or a divider drag: write CSS variables from the page
  (`aio::kit::Island`) and commit once at the end, as aio apps do.

## 6. Keep methods short; heavy work off the thread

A method runs while its cell waits: every other call of that cell, from
every window and client, waits behind it. Keep a method to what changes
the state; I/O and computing go elsewhere.

| Work | Where |
|---|---|
| I/O: a fetch, a file, a service | an [effect](state/effects.md#an-effect) or an [async method](state/effects.md#async-methods): the runtime waits, not the cell |
| CPU: parsing, hashing, a big sort | `tokio::task::spawn_blocking` inside an effect, the result back as a call |
| a blocking library call (a sync API, FFI) | `spawn_blocking` likewise |
| slow work on a timer | `#[every]` or `#[cron]` whose method starts such an effect |
| big state a client doesn't need | `#[cell(visible = f)]`: don't send it at all ([visibility](state/visibility.md)) |

```rust
use aio::prelude::*;
use aio::testing::TestCells;

#[cell]
pub struct Report {
    pub building: bool,
    pub top: Vec<u64>,
}

#[methods]
impl Report {
    pub fn build(&mut self, cx: &Cx, raw: Vec<u64>) {
        self.building = true; // shows at once
        cx.run(Effect::new("build", move |h: Handle<Report>| async move {
            // A real thread: the cell, the views and the runtime stay free.
            let top = tokio::task::spawn_blocking(move || {
                let mut v = raw;
                v.sort_unstable_by(|a, b| b.cmp(a));
                v.truncate(3);
                v
            })
            .await
            .unwrap_or_default();
            _ = h.built(top);
        }));
    }

    pub fn built(&mut self, top: Vec<u64>) {
        self.building = false;
        self.top = top;
    }
}

# fn main() {
let mut t = TestCells::new();
let h = t.cell::<Report>();
h.build(vec![4, 9, 1, 7]).unwrap();
assert!(h.state().building);
t.settle();
assert_eq!(h.state().top, [9, 7, 4]);
# }
```

`.await` is no escape for CPU work: an effect step that computes for
200 ms holds one of the runtime's threads for 200 ms (and `[perf] effect`
says so). Only `spawn_blocking` moves it to a thread of its own.

## Time budgets: `[perf]`

Every method call and every effect step is timed against a budget. A
breach is observe-only (the call still commits) and goes to three places:
`logs/perf.log`, a `WARN [perf]` line in the app log with the fix (at most
once per method every `aio::perf::QUIET`, 10 s), and the `on_problem` hook
(`aio::report::on_problem`) as `budget:method` or `budget:effect`.

```toml
[perf]
method = 16      # ms; unset: 16 in a debug build, 100 in a release build
effect = 5       # ms per effect step (one poll); unset: 5

[perf.methods]   # one meant to be slow, the rest stay strict
"builds.compile" = 5000

[perf.effects]   # per effect, by the name its key starts with
"hw.poll" = 250

[perf.storm]     # a runaway loop of calls
rate = 200       # calls a second…
secs = 5         # …so many seconds in a row
breaker = false  # true: refuse its calls while it lasts
```

| Key | Default | What it is |
|---|---|---|
| `method` | 16 (debug), 100 (release) | a method's own time: copying its state, the method, the invariants (not the write to disk) |
| `effect` | 5 | one step of an effect: the time it holds a runtime thread between awaits; the calls it makes are budgeted as methods |
| `methods` | none | `"<cell id>.<method>" = ms`; a key naming no method of that cell is warned at start (a typo would budget nothing) |
| `effects` | none | `"<cell id>.<effect name>" = ms` |
| `storm.rate`, `storm.secs` | 200, 5 | a method called `rate` times a second for `secs` seconds is a storm: said when it starts and ends, `on_problem` as `storm` |
| `storm.breaker` | false | refuse the storming method's calls while the storm lasts |

A breach in `perf.log`, with where the method's time went:

```text
method notes.add 43ms > 16ms (clone=0.1ms apply=42.6ms check=0.2ms commit=0.4ms)
```

| Phase | Is | If it dominates |
|---|---|---|
| `clone` | copying the state the method changes (a state slow to copy, 250 µs or more, is copied ahead on a thread of its own after each call, so the next one starts at once: ~0 here) | big fields: hold them as `Arc<…>` or `aio::imbl` collections; they are also compared after each commit (the views' re-render, a `#[coalesce]` run's write) |
| `apply` | the method's body | move the work to an effect (above) |
| `check` | the cell's invariants | an invariant that walks a big list on every call |
| `commit` | writing the event (and now and then a snapshot) | not counted against the budget: it is the disk's and `[data] durability`'s |

The warning names the fix, and the budget that would fit this one method
(the next 10 ms step over twice what it took), so a method that is slow on
purpose is one line in `[perf.methods]`, not a looser budget for all.

A debug build's budget is one frame on purpose: a method runs on its
cell's single path, so its time is time every client's next call waits;
the debug build says so while the app is small enough to fix cheaply.

## Size budgets: `[budgets]`

```toml
[budgets]
state = "1MB"           # one cell's saved state
payload = "500KB"       # one push to one client
broadcast_rate = "20/s" # pushes a second to one client
```

Sizes take `B`, `KB`, `MB`, `GB` (1 KB = 1024) or a number of bytes; a
rate `"20/s"` or a number; a typo or zero is an error at start. A breach
is kept with its worst reading, said once in the log and to `on_problem`
(`budget:state`…), and turns `/__aio/health` degraded, so a test or a CI
step can fail on it. With no `[budgets]` there is no verdict (`null`).

## Vitals: is anything behind?

`[vitals]` grades how late each layer is, in ms (effects for `queue`):

| Layer | Measures | degraded / warning / frozen |
|---|---|---|
| `loop` | how late the runtime's tick is: a call or effect step holding it | 100 / 500 / 2000 |
| `queue` | effects waiting their turn | 50 / 200 / 1000 |
| `transport` | a client not heard from | 100 / 500 / 2000 |
| `render` | a window's paint | 50 / 200 / 2000 |

The server probes `loop` and `queue` every second. A step up to warning or
frozen is logged with a hint (the likely cause: the call made last before
a freeze, a freeze-recover cycle), frozen fails the `runtime` part of
`ax health`, and the way back is logged as recovered. A hidden window is
throttled, never frozen. `hints = false` drops the hints.

## What moves: `ax cost`

```sh
ax cost notes            # every cell, top 3 keys each
ax cost notes inbox      # one cell
ax cost notes --keys     # every key
ax cost notes --json     # for scripts
```

```text
cell             pushes/s     bytes/s      mean p95 call  full  top keys by bytes
inbox                 1.0     7.7 KB/s    7.7 KB   0.4 ms     1  mails 6.1 KB · unread 12 B
watched 60 s, since the app's first send or call (its tools' looks not counted)
```

| Column | Is |
|---|---|
| `pushes/s` | states and patches this cell sent its clients, a second |
| `bytes/s`, `mean` | their bytes on the wire, a second and a push |
| `p95 call` | the 95th percentile of its methods' time (the last 512 calls) |
| `full` | whole states sent (a link's first, a resync), not patches |
| `top keys by bytes` | which top-level fields the patches' bytes came from |

Rates are per second actually watched, from the first thing seen. Read
it so: a big state that moves little is fine (it is there, not moving);
many bytes from one key is what `visible` is for; many `full` resends
mean clients keep reconnecting.

What travels is a patch of what changed: an object key by key, a
same-length list item by item, and a long string that grew as its new
tail (`append`, as aio does), so streamed text costs its growth, not its
length ([wire](migration/wire.md)).

## What renders: `ax components`

```sh
ax components notes             # the mounted tree: each component's renders and updates
ax components notes --renders   # the last 200 renders as they happened, timed
ax components notes --json
```

`--renders` is aio DevTools' render log on air: a line per body run
(`mount`, or `signal` when a wait or a `follow` ran it again) and per
hole rewrite (`signal`), each with its component and key (`root`: the
app's own view), when it ended and how long it took (a body's time holds
the children it placed), then how many in all. A debug build logs them;
a release, and a browser's wasm (no clock there), keep no log. In a
test, `air::renders()` inside the root (`TestPage::within`) gives the
same.

It reads a dev build of a running app through its door. A component's
`renders` are its body's runs (once, as it mounts, unless a `follow` or a
hot reload runs it again); its `updates` are its own holes' writes, what
a change cost it. In a test, `TestUi::components()` gives the same tree
(`name`, `key`, `renders`, `updates`, `children`) and
`TestUi::wire_bytes()` what the views sent the page, so a test can hold a
key press to a cost:

```rust
use aio::act;
use aio::testing::{MountedComponent, TestUi};
use aio::ui::prelude::*;

#[aio::cell(id = "counter")]
pub struct Counter {
    pub n: u32,
    pub label: String,
}

#[aio::methods]
impl Counter {
    pub fn inc(&mut self) {
        self.n += 1;
    }
}

aio::client!(Counter, counter);

#[component]
fn Label() -> View {
    let label = part(|c| &c.label);
    rsx! { h1 { "{label}" } }
}

#[component]
fn Count() -> View {
    let n = part(|c| &c.n);
    rsx! { button { "data-t": "inc", r#type: "button", onclick: move |_| _ = act!(inc()), "{n}" } }
}

#[component]
fn App() -> View {
    rsx! { Label {} Count {} }
}

fn costs(tree: &[MountedComponent], name: &str) -> (u32, u32) {
    let c = tree.iter().find(|c| c.name == name).expect(name);
    (c.renders, c.updates)
}

# fn main() {
let mut ui = TestUi::new(App);
ui.click("inc");
ui.click("inc");
assert_eq!(ui.text("inc"), "2");
let tree = ui.components();
assert_eq!(costs(&tree, "Count"), (1, 2), "its body once, its hole once a click");
assert_eq!(costs(&tree, "Label"), (1, 0), "a field it doesn't read costs it nothing");
# }
```

## Measured

- `cargo test -p aio-rs -F kit --test list_speed -- --nocapture`: a held ↓ over
  1000 rows (30 drawn), views of the whole state against the list
  above (debug build; in an aio-rs checkout):

  | per key | whole state, a commit per step | parts, coalesced, rows by props |
  |---|---|---|
  | views' work (bodies run and holes rewritten) | 15.5 | 1.27 |
  | writes to the file | 1.0 | 0.2 (one per 100 ms window) |
  | wire bytes to the page | 151 | 141 |
  | the store's share (p50 / p95) | 92 µs / 1.9 ms | 69 / 872 µs |

  One run, a loaded machine: the counts are exact, the times noisy.

- `ax heap <app>` shows memory per cell (`--watch`: every 2 s);
  `ax metrics <app>` each cell's size.
- Big state: keep large fields as `Arc<…>` or `aio::imbl` collections
  (feature `imbl`), so a call's copy of the state shares what didn't
  change; [big data](persistence/sql.md) belongs in `#[table]` fields.

## Common mistakes

- Raising `[perf] method` to quiet one slow method: every other method
  loses its warning. Name the one in `[perf.methods]`.
- Computing in an `async fn` method or an effect without
  `spawn_blocking`: `.await` only yields at the await, after the work.
- Reading state in a view's body (`let n = part(..).get();` before
  `rsx!`): read once, it never follows. Read it in a hole (`"{n}"` with
  `let n = part(..)`, `{name().x}`) or a `memo`.
- Reading `name()` in a hole: the hole follows every commit.
- A cursor beside the rows in one field (`Vec<(Row, bool)>`): a step
  rewrites the list. Keep the cursor its own field.
- Building a derived list in each row: build it once
  (`#[derived]`, `aio::Cache`) and pass `Arc`s down.
- Smooth scrolling (`behavior: smooth`) on a list a key moves through:
  the scrolls pile up under key repeat. `data-aio-reveal` uses `auto`.

## Gaps against aio

- aio bundles browser `Worker` scripts written as
  `new Worker(new URL("./w.ts", import.meta.url))`; aio-rs has no
  worker bundling. Heavy work in aio-rs runs in Rust, off the cell's
  thread (`spawn_blocking`); a web build's own worker script is the
  app's to ship as a file.
- aio's renderer yields at a 12 ms frame deadline mid-render; air runs
  the holes a change woke in one batch per tick, so keep what a change
  wakes small (parts, windows, rows by props).
- `ax components` has no time per component in the tree (aio DevTools'
  `lastRenderMs`): the time is per render, in `--renders`.
- aio's `blocking.cancel(id)` stops a running task; `spawn_blocking`
  can't be stopped once it runs: `cx.abort(key)` drops the effect at
  its next await, and the thread's result is thrown away.

## API

`cargo doc -p aio-rs --no-deps --all-features --open`: `aio::perf`
(`Budgets`, `Phases`, `QUIET`), `aio::budgets`, `aio::vitals`,
`aio::cost`, `aio::Cache`, `aio::Ver`, `aio::testing::TestUi`
(`components`, `wire_bytes`), `aio::kit::VirtualList`.

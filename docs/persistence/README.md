# Persistence

`#[cell(persist)]` keeps a cell in the app's `state.db` (SQLite): its
state, and one event per committed call. A call commits before it
answers, so nothing is ever "not saved yet". The exception is a
coalesced run, below, which is written within 100 ms, as aio's debounced
writes are.

| Page | What |
|---|---|
| this one | what is saved, the store and its log, coalesced writes, durability, history, shape changes |
| [Files, backups and restore](files.md) | where an app's data lives, profiles and instances, backups, snapshots, moving it |
| [Tables, SQL and blobs](sql.md) | cell fields as SQL tables, raw SQL with live queries, bytes, big data |
| [Offline and merging](offline.md) | synced copies offline, merging edits, what travels on the wire |

## Saved, reopened, replayed

```rust
use aio::prelude::*;

#[cell(persist, id = "tally")]
pub struct Tally {
    pub n: i64,
}

#[methods]
impl Tally {
    pub fn add(&mut self, by: i64) {
        self.n += by;
    }
}

# fn main() {
let dir = tempfile::tempdir().unwrap();
let file = dir.path().join("state.db");
{
    let h = Handle::<Tally>::open(&file).unwrap();
    h.add(2).unwrap();
    h.add(3).unwrap();
}
let h = Handle::<Tally>::open(&file).unwrap();
assert_eq!(h.state().n, 5, "the state came back");
let events = h.store(|s| s.events()).unwrap();
let calls: Vec<&str> = events.iter().map(|e| e.method.as_str()).collect();
assert_eq!(calls, ["add", "add"]);
assert_eq!(h.store(|s| s.state_at(events[0].seq)).unwrap().n, 2, "any past state, replayed");
# }
```

- In an app nothing is opened by hand: `aio::Source::for_app` opens the
  cell on the app's store (`data/state.db`, or `[data] store`).
  `Handle::<C>::open(path)` is the same store on a file you choose (a
  tool, a test); `Handle::<C>::memory()` keeps it in memory, with a log
  and time travel while it lives.
- `persist` is what lets a cell use `#[transient]`, `save`, `restore`,
  `retire` and `#[table]`, and what saves a page's cell in the browser.
  On this machine the store saves the cell it is opened on whether or
  not it says `persist`, so an app's data stays where it was.
  `#[cell(persist = false)]` (aio's `persist: "none"`) is never saved:
  a draft, a selection, a wizard's step start from their defaults on
  every launch, wherever the cell is opened.
- The log keeps every call with its arguments, its caller (`by`), its
  time and what it read from the world (`cx.now()`, `cx.id()`,
  `cx.rand()`), so a replay makes the same state. `ax events <app>`
  shows it, `ax timetravel <app> <cell>` and
  `h.travel(Some(aio::TimeAsk::Undo))` step through it, `ax replay` runs
  it again on the running app.
- In a page built with `ax build --web`, a persist cell is saved to the
  browser's storage after each call: its state only, no log and no
  history.

## What is saved

Everything in the cell, unless you say otherwise. Five tools shape it:

| Tool | Does | Use it for |
|---|---|---|
| `#[transient]` on a field | never written; back to its default on every boot | hover, focus, a progress value, a cache of a derived value |
| `#[cell(persist(exclude = ["feeds.*.busy"]))]`, `persist(include = ["name", "profile.email"])` | paths into the state left out, or the only ones written (aio's `persist: { include, exclude }`) | a flag inside each row of a map or list, a token inside a profile |
| `#[cell(restore = f)]`, `fn(Self) -> Self` | runs on the state on every boot and every rewind | clearing a "busy" flag a crash left set |
| `#[cell(save = f)]`, `fn(&Self) -> Self` | shapes what is written (the state in memory stays whole) | dropping rows that are only shown, trimming a list to its last 100 |
| `#[redact]` on a method | logs the state the call made, never its arguments | a passphrase, a token: anything the log must not keep |

```rust
use aio::prelude::*;

fn settle(mut j: Jobs) -> Jobs {
    j.busy = false; // a crash mid-job left it set
    j
}

#[cell(persist, id = "jobs", restore = settle)]
pub struct Jobs {
    pub queued: Vec<String>,
    pub busy: bool,
    pub unlocked: bool,
    #[transient]
    pub hover: Option<usize>,
}

#[methods]
impl Jobs {
    pub fn begin(&mut self, job: String) {
        self.queued.push(job);
        self.busy = true;
    }

    pub fn point(&mut self, at: Option<usize>) {
        self.hover = at;
    }

    #[redact]
    pub fn unlock(&mut self, pass: String) -> Result {
        ensure!(pass.len() >= 8, "too short");
        self.unlocked = true;
        Ok(())
    }
}

# fn main() {
let dir = tempfile::tempdir().unwrap();
let file = dir.path().join("state.db");
{
    let h = Handle::<Jobs>::open(&file).unwrap();
    h.begin("backup".into()).unwrap();
    h.point(Some(0)).unwrap();
    h.unlock("correct horse".into()).unwrap();
} // closed while busy, as a crash would leave it

let h = Handle::<Jobs>::open(&file).unwrap();
let s = h.state();
assert_eq!((s.queued.len(), s.busy, s.unlocked, s.hover), (1, false, true, None));
let events = h.store(|s| s.events()).unwrap();
assert_eq!(events.last().unwrap().method, "$restore", "the repair is an event too");
assert!(events.iter().all(|e| !e.args.to_string().contains("correct horse")), "never in the log");
# }
```

- A repair that changes the state is logged as a `$restore` event, so a
  replay and every synced copy follow it.
- `#[transient]` fields come back at their default whether the app
  stopped cleanly or crashed: boot replays the log, then resets them.
- Keep `save` idempotent (`save(save(x)) == save(x)`): a crash and a
  clean stop then come back the same.
- `#[server]` on a field of a shared cell is about the wire, not the
  disk: it is saved, never sent to a client
  ([visibility](../state/visibility.md)).
- `#[cell(diagnostics = false)]` keeps a cell's calls out of the
  timeline and problem reports; it is still saved and logged.
- Persist paths are dot paths from a field (checked as the app builds):
  a `*` segment of an exclude path names every key, and a list is walked
  item by item (`feeds.*.busy` reaches each feed of a map or a list). A
  field a path reaches inside a struct needs a default (`#[serde(default)]`
  on its type): it comes back at it. `include` and `exclude` don't mix.
- `[data] persist = false` (aio's `persist: false`), or the launch flag
  `--no-persist`, keeps the whole app in memory: no store is opened, and
  every launch starts from the defaults.

### Data from a newer build

A build opening data a newer build wrote (a rollback) boots on what it
declares, as aio's downgrade boot does: every snapshot keeps the newer
build's other fields, stamped with the newer version (never lowered, so
rolling forward migrates nothing twice); in an aio file the slice keeps
them too, and a verbatim copy is parked once under `__downgraded:<cell>`.
Fields this build declares may be misread. Data its fields can't hold
refuses the boot, the file kept. Rolled forward again (new, old, new),
the newer build boots on the snapshot the older one wrote and drops the
fields only the older build declares, as aio's restore merge does; its
own fields are as the older build left them.

### State export and import

`h.export_snapshot()` is the whole state as aio's `app.snapshot()` has it: an
object of cells (`{"notes": {...}}`, a `#[aio_cell]` field under its aio
cell's name, `#[server]` fields included). `h.load_snapshot(&snap, force)`
puts one back as a new event, saved before it returns and sent to every
client; a snapshot whose cells aren't this app's is refused unless
`force` (a missing cell starts from its defaults, an unknown one is
dropped). A dev build also serves them as aio does: `GET` and `POST`
`/__aio/snapshot` (`X-AIO: 1` on a `POST`, `?force=1`), to this machine's
tools with the app's key or to an admin; `ax snapshot save` and `load` go
through the local door in any build.

## How the store works

One SQLite file holds every persist cell of the app, in five tables of
its own, plus the rows of any `#[table]` fields ([sql](sql.md)):

| Table | Holds |
|---|---|
| `log` | one row per event: `seq` (one sequence over all cells), `cell`, `method`, `args`, `inputs`, `by`, `at`, `v` (the cell version that wrote it) |
| `snap` | snapshots of each cell's saved state at an event |
| `floor` | per cell, the oldest event history still reaches |
| `sched` | the cell's armed schedules (`cx.after`, `#[every]`, `#[cron]`) |
| `ops` | the ids of calls synced copies made, so a re-sent one applies once |

`ax sql <app> "select cell, count(*) from log group by cell"` reads them
(read-only), `ax tables <app>` lists them, `ax heap <app>` shows each
cell's state size and events.

**A call** runs on a copy of the state. When the method returns `Ok` and
the invariants hold, one SQLite transaction writes the event, the rows of
its `#[table]` fields that changed, its schedule changes and its call id;
only after that commit does the new state replace the old and the caller
get its answer. A storage error refuses the call (`storage: …`) and the
state stays as it was.

**A call and its reactions** (`listens_to`, aio's `listensTo`) commit in
one SQLite transaction: the reactions run inside the call's commit, their
own calls' events and rows go into the same transaction, and only once it
committed are any of them shown or told. A crash (`kill -9`) leaves both
on disk or neither; a refused commit keeps neither and puts every cell in
it back. aio does the same: one reduce runs a call and its listeners, one
transaction saves every cell (aio's `server/persistence.ts`). A joint
commit writes through a connection the cells of its file share; once some
of them react to each other, that file's cells take calls one at a time
(as aio's one reduce does), while a file without reactions keeps taking
them side by side, each cell on its own connection.

**Snapshots.** Every 256 events, at a boot that replayed any, and at a
stop (`ax stop`, a closed window, SIGTERM, logoff or shutdown, `ax dev`'s
restart), the cell's saved state is written to `snap`, so the next start
reads what the calls left and replays none of them with code changed
meanwhile. `ax dev` snapshots every cell before each hot-patch too: a
later start replays only calls the patched code made. A failed snapshot
only costs time: the next boot replays more of the log. A kill (SIGKILL,
power loss) loses at most a `#[coalesce]` run still held (100 ms); every
other acknowledged call is in the log.

**Boot**, for each persist cell, in order:

1. One boot at a time per process; the store's folder is the app's own
   (holding it is the single-instance lock).
2. Leftovers of a crash beside the file (a half-made copy, a half-written
   temp file) are removed.
3. SQLite's `quick_check`. A damaged file is set aside as
   `state.db.damaged-<ms>` and the good copy (`state.db.good`, daily, or
   every `[data] snapshot_every`) takes
   its place, said in the log and as `aio::Recovered`; with no good copy
   the boot is refused and the file kept as is.
4. The file is checked to be aio-rs's (or, for a ported app, aio's); a
   file with other tables is refused, never written.
5. The newest snapshot is read. If it was written by an older cell
   version, the file is first copied to `state.db.v<old>-<seq>.bak`.
6. Retired cells are deleted, `#[table]` fields' tables made or brought
   to their declared shape.
7. The snapshot is migrated (`migrate`) if needed, then the events after
   it are replayed with the current code. An event that no longer applies
   (a method changed its arguments, a version without its `migrate`)
   **quarantines** the cell in a release build (aio's): it runs at its last
   snapshot, refuses every call (`PERSIST_ERROR`, naming the event), serves
   no synced client, and keeps its log and snapshot as they are for a
   fixed build; `Store::quarantined` says why. A dev build (a debug
   build, `ax dev`'s or `ax test`'s) refuses the boot instead, so the fix
   comes first.
   `aio::testing::quarantine(true)` boots a test as a release build does.
8. `restore` runs and `#[transient]` fields reset; after a recovery
   (step 3), `#[cell(recovered = f)]` (`fn(Self, &aio::Recovered) ->
   Self`, aio's `onCheckpointRestore`) returns the state to boot on (say
   from when the data is, ask the server again); a change is a `$restore`
   event.
9. The good copy is refreshed if it is a day old; with `[data]
   snapshot_every = "15m"` also every 15 minutes while the app runs. A
   copy that fails `quick_check` never replaces the last good one.

**One writer.** Each cell's store refuses writes when something else
added to its log (a second process, a tool writing the file) or the file
was deleted or replaced while the app ran: it says so and asks for a
restart, instead of forking the history. Tools (`ax sql`, `ax events`)
open the file read-only.

## Coalesced writes

A method called many times a second (a cursor, a scroll, a selection)
should not cost a transaction per call. Mark it `#[coalesce]`, or the
whole cell `#[cell(persist, coalesce)]`:

```rust
use aio::prelude::*;

#[cell(persist, id = "picker")]
pub struct Picker {
    pub rows: Vec<String>,
    pub at: usize,
}

#[methods]
impl Picker {
    pub fn add(&mut self, row: String) {
        self.rows.push(row);
    }

    #[coalesce]
    pub fn focus(&mut self, at: usize) {
        self.at = at.min(self.rows.len().saturating_sub(1));
    }
}

# fn main() {
let h = Handle::<Picker>::memory();
for i in 0..10 {
    h.add(format!("row {i}")).unwrap();
}
let before = h.store(|s| s.events()).unwrap().len();
for i in 1..=5 {
    h.focus(i).unwrap(); // shown at once, written later
}
assert_eq!(h.state().at, 5);
h.store(|s| s.flush()).unwrap(); // what the debounce, a close or a non-coalesced call does
assert_eq!(h.store(|s| s.events()).unwrap().len(), before + 1, "five steps, one event");
# }
```

Each call shows its state at once. The run is written in one transaction
`[data] coalesce_ms` after its first call (aio's `persistDebounceMs`;
default `aio::PERSIST_DEBOUNCE`, 100 ms; `0` writes each call), or as soon as a call
that can't wait comes (one with an effect, a schedule or an id), a
time-travel step, or the close. When the last call alone ends where the
run did (`focus(5)` above), the run is logged as that one event;
otherwise (a relative `down()`) call by call, still in one transaction. A
run that comes back to where it began logs nothing. A crash loses at most
the window, as in aio. A run that can't be written stays held and is
tried again.

## Durability

```toml
[app]
id = "notes"

[data]
durability = "full"     # "normal" (default): survives a crash of the app
store = "state.db"      # the store's file in data/
pragmas = ["PRAGMA cache_size = -16000"]
retire = ["old.db"]     # old store files to set aside at boot

[history]
keep = "30d"
```

- `normal` (SQLite WAL, `synchronous = NORMAL`): a commit survives the app
  crashing; the last ones may be lost if the machine loses power.
- `full` (`synchronous = FULL`): each commit is on disk before the call
  returns. Phones always use it: the OS may kill the app at any moment.
- `[data] pragmas` are set after aio-rs's own, each read back: one SQLite
  ignored refuses the boot. `journal_mode`, `synchronous` and
  `user_version` belong to the store and can't be set.
- A store is never silently reset: damage boots the good copy (above),
  data that doesn't fit refuses the boot.

| What happens | A plain call | A `#[coalesce]` run |
|---|---|---|
| the call returned | in the file | in memory; in the file within 100 ms |
| the app crashes or is killed | kept | the last ≤100 ms lost |
| the app closes cleanly | kept | written at the close |
| power loss, `normal` | the last commits may be lost | the last commits may be lost |
| power loss, `full` | kept | the last ≤100 ms lost |
| the file is damaged | the good copy, up to a day old (or `[data] snapshot_every`) | the same |

aio needed `journal: true` to make a returned call durable; aio-rs has
no journal because every plain call already is.

## History

Every event stays in the log for `[history] keep`, so time travel,
`ax replay` and `state_at(seq)` reach back that far. Folding drops old events
into a snapshot: the state stays, the steps before it are gone.

```rust
use aio::prelude::*;

#[cell(persist, id = "tally")]
pub struct Tally {
    pub n: i64,
}

#[methods]
impl Tally {
    pub fn add(&mut self, by: i64) {
        self.n += by;
    }
}

# fn main() {
let dir = tempfile::tempdir().unwrap();
let h = Handle::<Tally>::open(dir.path().join("state.db")).unwrap();
h.add(1).unwrap();
h.add(2).unwrap();
let folded = h.store(|s| s.fold_before(i64::MAX)).unwrap(); // every event older than now
assert_eq!(folded, 2);
assert_eq!(h.state().n, 3, "the state stays");
assert!(h.store(|s| s.state_at(1)).is_err(), "the steps before the fold are gone");
# }
```

- `fold_before(ms)` folds the events made before that time (ms since the
  epoch) and returns how many. The ids of synced calls go with them, but
  never sooner than a re-send could still be accepted (30 days).
- `[history] keep` (default `"90d"`) folds for you: the events older
  than `keep` go at boot and then every 256 commits, so the log stays
  about `keep` long. A store opened by hand keeps all until
  `h.store(|s| s.keep_history(keep))` (folds now, then as commits go on)
  or `fold_before`. `ax heap <app>` shows how big each cell's log is.

## Changing the shape

A new field with a default needs nothing: a `#[cell]` reads what is
missing at its default, and a field no longer declared is dropped at the
next save. Anything else bumps the version and gives `migrate`:

```rust
use aio::prelude::*;
use aio::testing::TestCells;
use serde_json::{Value, json};

fn up(old: Value, from: u32) -> std::result::Result<Notes, String> {
    match from {
        1 => {
            let text = old["text"].as_str().ok_or("no text")?;
            Ok(Notes { lines: text.lines().map(String::from).collect() })
        }
        _ => serde_json::from_value(old).map_err(|e| e.to_string()),
    }
}

#[cell(persist, id = "notes", version = 2, migrate = up)]
pub struct Notes {
    pub lines: Vec<String>,
}

#[methods]
impl Notes {}

# fn main() {
let mut t = TestCells::new();
let h = t.saved::<Notes>(1, json!({"text": "a\nb"})).unwrap(); // boots as the app would
assert_eq!(h.state().lines, ["a", "b"]);
assert!(t.saved::<Notes>(1, json!({"other": 1})).is_err(), "data that doesn't fit refuses the boot");
# }
```

- `migrate` gets the stored JSON and the version it was saved at, and
  returns the new state or why it can't. It runs at boot, on every
  rewind across the change, and in `TestCells::saved`. Handle every older
  version in it: a store may skip builds.
- Before the first boot on the new version the file is copied to
  `state.db.v<old>-<seq>.bak`, and an update backs the store up again
  before it installs ([files](files.md#backups)).
- Data that doesn't fit refuses to boot; it is never reset. Data from a
  newer build (a higher version, a rollback) boots on what this build
  declares, keeps the newer build's other fields in every write and
  parks a copy under `__downgraded:<cell>`, as aio does; refused only
  when it doesn't fit. To roll back past a shape change, put the backup
  back (`ax restore`).
- The events after the snapshot are replayed with the new code. An event
  an older version wrote that the new methods can't apply refuses the
  boot, naming it: open the data once with the old build (which writes a
  snapshot), then upgrade.
- `ax migrations <app>` lists each stored cell's version against the
  running app's, and its shape drift: fields stored but no longer
  declared, or of another type.
- A cell no build reads any more: `#[cell(retire = ["old"])]` on a cell
  that does deletes it from the file at boot, history included, its bytes
  zeroed. A whole old store file the app no longer opens: `[data] retire
  = ["old.db"]` sets it aside as `old.db.pre-migration`.

### A field inside a collection

A `#[cell]` struct reads a missing field at its default, but a `#[data]`
type doesn't: serde's rules apply, so a new field in an item of a list
makes old data not fit, and the boot is refused. Give the new field a
default:

```rust
use aio::prelude::*;
use aio::testing::TestCells;
use serde_json::json;

#[data]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Card {
    pub text: String,
    #[serde(default)] // added later: older cards read it as false
    pub pinned: bool,
}

#[cell(persist, id = "wall")]
pub struct Wall {
    pub cards: Vec<Card>,
}

#[methods]
impl Wall {}

# fn main() {
let mut t = TestCells::new();
let h = t.saved::<Wall>(1, json!({"cards": [{"text": "hi"}]})).unwrap();
assert!(!h.state().cards[0].pinned);
# }
```

`#[serde(default)]` on the struct gives every missing field its
`Default`. A rename or a change of type inside an item needs `version`
and `migrate`, as above.

## Merging offline edits

Two synced copies that edit offline merge on the server, which replays
each call in order: what a method expresses merges by itself (`n += 1`
counts twice, a set's `insert` keeps both). Prose needs
`aio::merge::text3`. Both are on [offline and merging](offline.md#merging-edits).

## Limits, compared with aio

- **No store layouts to pick.** aio's `persistMode: "single" | "multi"`
  doesn't apply: aio-rs keeps a log and snapshots per cell. A ported app
  reads and writes aio's layout as aio left it ([data](../migration/data.md)).
- **`cellDefaults`.** `persist: "none"` for every cell is `[data]
  only_persist = true` (then only `#[cell(persist)]` cells are saved);
  one cell's is `#[cell(persist = false)]`. A visibility filter is each
  cell's own `visible = f`, checked against its type (an app-wide field
  list can't be).
- **Strict restore when rolling forward.** Data an older build wrote over
  a newer build's keeps the older build's own fields too; the newer build
  then refuses to drop them silently (declare them, or migrate them away).

## Common mistakes

- Renaming a field without `version` and `migrate`: the old data reads
  the new field as its default, and the old value is gone at the next
  save. Bump the version.
- A new field in a `#[data]` item without `#[serde(default)]`: the old
  data no longer fits and the boot is refused.
- Changing a cell's `id`: the app finds no data and starts empty. Keep
  the id; retire the old cell explicitly if you must move.
- Putting bytes or a whole imported dataset in a cell: every call copies
  and every snapshot writes it. Use [blobs and tables](sql.md).
- Expecting a coalesced call to be on disk when it returns: it is within
  100 ms; a call that must be durable at once is a plain method.
- A secret as a method argument without `#[redact]`: it stays in the log
  and every backup.
- Deleting `state.db` to "fix" a boot: read the refusal, then
  `ax migrations` and `ax restore`; the file set aside is your data.

## API

`cargo doc -p aio-rs --no-deps --open`: `aio::Store` (`events`,
`newest`, `state_at`, `flush`, `fold_before`, `open`, `recovered`,
`durability`), `aio::Handle::open`, `aio::Event`, `aio::Recovered`,
`aio::PERSIST_DEBOUNCE`, `aio::CellData` (`save`, `restore`, `migrate`,
`TRANSIENT`, `PERSIST_INCLUDE`), `aio::filter`, `aio::Handle::export_snapshot`,
`load_snapshot`, `aio::merge`, `aio::testing::TestCells::saved`.

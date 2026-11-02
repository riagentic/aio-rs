# Time travel

Every cell's log is its history: each call that changed the state is an
event, numbered, kept in the store. Time travel moves a cell to the state
after any of those events, on the running app, to answer "how did the
state get like this?". It is an operator's tool: you, on your machine, or
a test. It is not a feature for the app's users (see
[a replay for users](#a-replay-for-users)).

## How it works

- A step is a **new event**, `$rewind`, that puts the state of an earlier
  event back. Nothing is cut from the log, so every step can itself be
  stepped back from, and what happened after the target stays there to
  return to.
- A step **pauses** the cell, so it stays where travel put it: calls
  are refused (`paused by time travel (ax timetravel resume lets calls in
  again)`), never dropped as if they ran; schedules are held and those
  missed fire once on resume; effects already running still commit.
  `Resume` lets calls in again.
- The state put back is what was **saved** of it, as a boot brings it
  back: `#[transient]` fields at their default, `save` applied, `restore`
  run. A rewind is a commit like any other: views update, synced
  copies get it, it is saved.
- Where a cell stands is a `Timeline`: `at` (the event its state is right
  after, a rewind followed to its target), `head` (its newest call, not
  counting rewinds), `floor` (the oldest event still kept), `paused`,
  and `steps`: its newest 20 events, oldest first, each with `seq`,
  `method` (`$rewind` for a step), `at` (ms since the epoch) and `took`
  (for calls of this run).

| Step | `TimeAsk` | Goes to |
|---|---|---|
| undo | `Undo` | the state the current one was made on; refused at the cell's start (`nothing to undo`) or before `floor` |
| redo | `Redo` | the next state toward where the run of undos began; refused when a new call came after them (`nothing to redo`) |
| goto | `Goto(seq)` | the state right after event `seq` (between `floor` and the newest) |
| pause | `Pause` | nowhere: calls refused until `Resume` |
| resume | `Resume` | nowhere: calls taken again |
| where | `None` | nowhere: only the `Timeline` |

```rust
use aio::prelude::*;
use aio::TimeAsk;

#[cell(persist)]
pub struct Score {
    pub n: i64,
    #[transient]
    pub hovered: bool,
}

#[methods]
impl Score {
    pub fn add(&mut self, by: i64) {
        self.n += by;
    }

    pub fn hover(&mut self, on: bool) {
        self.hovered = on;
    }
}

# fn main() {
let h = Handle::<Score>::memory();
for by in [1, 2, 3] {
    h.add(by).unwrap(); // events 1, 2, 3
}
h.hover(true).unwrap(); // event 4

let tl = h.travel(Some(TimeAsk::Goto(3))).unwrap();
assert_eq!((h.state().n, h.state().hovered), (6, false), "what was saved: #[transient] is not");
assert_eq!((tl.at, tl.head, tl.paused), (3, 4, true));
assert_eq!(tl.steps.last().unwrap().method, "$rewind", "a step is a new event");

h.travel(Some(TimeAsk::Undo)).unwrap();
h.travel(Some(TimeAsk::Undo)).unwrap();
assert_eq!(h.state().n, 1);
h.travel(Some(TimeAsk::Redo)).unwrap();
assert_eq!(h.state().n, 3, "redo walks back toward where the undos began");

let e = h.add(10).unwrap_err();
assert!(e.why.contains("paused by time travel"), "{}", e.why);
h.travel(Some(TimeAsk::Resume)).unwrap();
h.add(10).unwrap();
assert_eq!(h.state().n, 13, "a new call goes on from where travel put it");
assert!(h.travel(Some(TimeAsk::Redo)).is_err(), "a call after the undos leaves nothing to redo");
assert!(h.travel(Some(TimeAsk::Goto(4))).is_ok(), "and every state is still there");
# }
```

`Handle::travel(ask)` is the API; `Handle::pause(on)` and
`Handle::paused()` hold a cell without moving it.

## On the running app

```sh
ax timetravel notes tasks             # where it is, and its newest 20 events
ax timetravel notes tasks undo
ax timetravel notes tasks goto 41     # the numbers ax events lists
ax timetravel notes tasks resume      # take calls again
```

`ax timetravel` prints where the cell is, then its events, the current one
marked:

```text
tasks at event 41 of 44, paused (resume: take calls again); kept from event 0
      40  2026-10-28T09:48:01.123Z  add  0.21 ms
▶     41  2026-10-28T09:48:02.410Z  toggle  0.08 ms
      42  2026-10-28T09:48:03.002Z  remove  0.11 ms
```

It goes through the app's local door, which only the OS user who runs the
app can reach. `ax ui` shows the same as a timeline. Around it:

| Ask | Verb |
|---|---|
| the events, oldest first, with what each changed | `ax events notes tasks --last 50 --diff` |
| of another store or a backup | `ax events notes tasks --from backup.aio-backup` |
| the state at this moment, to a file | `ax snapshot save notes before.json` |
| put a snapshot's cells back (each a new event: `goto` steps back from it) | `ax snapshot load notes before.json` |
| call a log's inputs again, in order | `ax replay notes 40..44`, `ax replay notes --from backup.aio-backup` |
| what a replay would send, without sending | `ax replay notes 40.. --dry` |
| a test from what a cell just did | `ax record notes tasks bug.rs 40..44` |

Ranges are `N`, `N..M`, `N..` and `..M`. A replay sends the inputs only:
what they caused (effects, schedules) the app makes again, so a replay
against a changed build shows what the change does. `#[redact]` methods
show `[redacted]` in `ax events`. A snapshot taken `--as` a user holds only
what that user saw.

### From the log to a test

```sh
ax events notes tasks --last 10       # find the calls that went wrong: 40 to 44
ax record notes tasks tests/bug.rs 40..44
ax test notes bug
```

`ax record` writes a Rust test that boots the cell on its state before
event 40, makes the same calls, and asserts the state they left. Change
the assertion to what the state should have been, see it fail, then fix
the code until it passes ([debugging](README.md#6-reproduce-fix-prove)).
`--type` names the cell's Rust type when it isn't `<crate>::<Cell>`.

## In the window

`aio::kit::TimeTravelPanel` is aio's dev panel: hidden until Ctrl+.
(needs `aio::kit::GlobalKeys` around the app), it shows where the cell is,
Undo, Redo, Pause or Resume, and the newest steps (a click goes there).
`client!` gives it its data, `use_time_travel()`, and takes its asks,
`time_travel(ask)`:

```rust
use aio::act;
use aio::kit::{GlobalKeys, TimeTravelPanel};
use aio::ui::prelude::*;

#[aio::cell(persist, id = "counter")]
pub struct Counter {
    pub n: i64,
}

#[aio::methods]
impl Counter {
    pub fn add(&mut self) {
        self.n += 1;
    }
}

aio::client!(Counter, counter);

#[component]
pub fn App() -> View {
    use_live();
    let n = part(|c| &c.n);
    let tl = use_time_travel();
    rsx! {
        GlobalKeys {
            main {
                p { "data-t": "count", "{n}" }
                button { onclick: move |_| _ = act!(add()), "Add" }
            }
            // Shown open here; in an app leave `open` out: Ctrl+. shows it.
            TimeTravelPanel { timeline: tl.get(), open: true, onask: move |ask| _ = time_travel(ask) }
        }
    }
}

# fn main() {
let mut ui = aio::testing::TestUi::new(App);
ui.click("AddButton");
ui.click("AddButton");
assert_eq!(ui.text("count"), "2");
ui.click("ttUndo");
assert_eq!(ui.text("count"), "1");
assert!(ui.text("ttWhere").contains("paused"));
# }
```

A synced copy's history is its server's: there `use_time_travel()` is
`None` and travel is refused (`a synced copy travels on its server`); run
`ax timetravel` against the server. An aio page's time-travel panel works
against an aio-rs server too (feature `compat`), for the `admin` role
only: a step moves the state of every user.

## How far back

The log keeps every event for `[history] keep`, so a cell travels back
that far. `floor` moves up only when old events are folded into a snapshot:
`h.store(|s| s.fold_before(ms))` folds the events older than `ms` (ms
since the epoch) and returns how many; travel before `floor` is refused
(`nothing earlier is kept`). An app folds by itself what is older than
`[history] keep` (default 90 days): at boot and every 256 commits.

What it costs:

- A call's event holds its arguments; a step's holds the whole saved
  state, so stepping through a big cell writes that much each time.
- `#[cell(diagnostics = false)]` keeps a hot cell's calls out of the live
  call timeline, reports and the debug log; they are still events, so
  travel still steps through them.
- `[log] suppress = ["game.tick"]` keeps one method's calls out of the
  debug log only.

## A replay for users

"Replay the last game" is not time travel: it would replay every user's
history, and a step pauses the cell. Record the inputs the game depends
on in a cell of their own, and play them back as calls from a known start:

```rust
use aio::prelude::*;

#[cell(persist, id = "game")]
pub struct Game {
    pub seed: u64,
    pub x: i64,
    /// Every input the game depends on, in order.
    pub tape: Vec<(u32, i64)>,
}

#[methods]
impl Game {
    pub fn begin(&mut self, seed: u64) {
        (self.seed, self.x, self.tape) = (seed, 0, vec![]);
    }

    pub fn press(&mut self, frame: u32, dx: i64) {
        self.tape.push((frame, dx));
        self.x += dx;
    }
}

# fn main() {
let live = Handle::<Game>::memory();
live.begin(7).unwrap();
for (f, dx) in [(1, 2), (5, -1), (9, 3)] {
    live.press(f, dx).unwrap();
}

let replay = Handle::<Game>::memory();
replay.begin(live.state().seed).unwrap();
for (f, dx) in live.state().tape.clone() {
    replay.press(f, dx).unwrap();
}
assert_eq!(replay.state().x, live.state().x);
# }
```

It survives restarts and release builds, and it is exact as long as
methods are pure (randomness from the seed in the state, time from
`cx.now()`, never the clock).

## Not as in aio

- aio's time travel is dev-only and a ring of 2000 entries (128 MB) in
  memory. aio-rs's reads the store, so it works in a release build, on
  the whole history, after a restart.
- aio's `diagnostics.dev.timeTravel: false` turns it off; aio-rs has no
  switch: only the OS user (the local door) or an `admin` on the aio panel
  can travel.
- aio's `skipActions` keeps a method out of the history. aio-rs keeps
  every event (it is the store); `diagnostics = false` and `[log]
  suppress` only quiet what is shown.

## API

`cargo doc -p aio-rs --no-deps --all-features --open`: `aio::Handle`
(`travel`, `pause`, `paused`, `store`), `aio::TimeAsk`, `aio::Timeline`,
`aio::Moment`, `aio::Store::fold_before`, `aio::kit::TimeTravelPanel`,
`aio::client!`.

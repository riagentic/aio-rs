# Tutorial: a todo app, step by step

The [quickstart](quickstart.md) made an app from the template; this page
builds one of your own: a todo list, saved, tested, with a window. Each
step adds one idea. Every Rust block here compiles and runs as a test of
aio-rs (the lines a step repeats from the one before are hidden), so what
you read is what works.

Start from an app: `ax create todo`, then replace `src/lib.rs` step by step.

## 1. The cell

The state, and the only ways to change it:

```rust
use aio::prelude::*;

#[data]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Task {
    pub id: Uuid,
    pub title: String,
    pub done: bool,
}

#[cell(persist, id = "todo")]
pub struct Todo {
    pub tasks: Vec<Task>,
}

#[methods]
impl Todo {
    /// Add a task; refused (nothing changes) without a title.
    pub fn add(&mut self, cx: &Cx, title: String) -> Result<Uuid> {
        let title = title.trim().to_string();
        ensure!(!title.is_empty(), "a task needs a title");
        let id = cx.id();
        self.tasks.push(Task { id, title, done: false });
        Ok(id)
    }

    pub fn toggle(&mut self, id: Uuid) -> Result {
        let Some(t) = self.tasks.iter_mut().find(|t| t.id == id) else { reject!("no task {id}") };
        t.done = !t.done;
        Ok(())
    }

    pub fn remove(&mut self, id: Uuid) -> Result {
        let before = self.tasks.len();
        self.tasks.retain(|t| t.id != id);
        ensure!(self.tasks.len() < before, "no task {id}");
        Ok(())
    }
}

# fn main() {
let h = Handle::<Todo>::memory();
let milk = h.add("buy milk".into()).unwrap();
h.add("call Ada".into()).unwrap();
h.toggle(milk).unwrap();
assert!(h.state().tasks[0].done);
assert_eq!(h.add("  ".into()).unwrap_err().why, "a task needs a title");
assert_eq!(h.state().tasks.len(), 2, "a refusal changes nothing");
# }
```

- `#[cell(persist, id = "todo")]`: the state is saved in the app's
  `state.db`, one event per call. `id` is its name in the store: never
  change it once the app has run.
- `#[data]` goes on your own types a cell holds; it brings serde, so the
  app adds no dependency.
- A method runs on a copy of the state: `Ok` makes the copy the state,
  `ensure!` / `reject!` refuse and leave it as it was.
- `cx.id()` is a fresh UUIDv7 kept with the event, so a replay of the log
  makes the same ids. A method never reads the clock, files or randomness
  itself: `cx.now()`, `cx.id()` and `cx.rand()` give them, recorded.
- `Handle::<Todo>::memory()` is the cell with no file: what tests use.

## 2. Derived values and an invariant

A count the window shows, computed once per state, and a rule no call
may break:

```rust
# use aio::prelude::*;
# #[data]
# #[derive(Clone, Debug, Default, PartialEq)]
# pub struct Task {
#     pub id: Uuid,
#     pub title: String,
#     pub done: bool,
# }
# #[cell(persist, id = "todo")]
# pub struct Todo {
#     pub tasks: Vec<Task>,
# }
#[methods]
impl Todo {
#     pub fn add(&mut self, cx: &Cx, title: String) -> Result<Uuid> {
#         let title = title.trim().to_string();
#         ensure!(!title.is_empty(), "a task needs a title");
#         let id = cx.id();
#         self.tasks.push(Task { id, title, done: false });
#         Ok(id)
#     }
#     pub fn toggle(&mut self, id: Uuid) -> Result {
#         let Some(t) = self.tasks.iter_mut().find(|t| t.id == id) else { reject!("no task {id}") };
#         t.done = !t.done;
#         Ok(())
#     }
    /// Remove every finished task.
    pub fn clear_done(&mut self) {
        self.tasks.retain(|t| !t.done);
    }

    /// How many are still to do: cached per state.
    #[derived]
    pub fn left(&self) -> usize {
        self.tasks.iter().filter(|t| !t.done).count()
    }

    /// Checked after every call: a call that breaks it is refused.
    #[invariant]
    fn at_most_500_tasks(&self) -> Result {
        ensure!(self.tasks.len() <= 500, "a list holds at most 500 tasks");
        Ok(())
    }
}

# fn main() {
let h = Handle::<Todo>::memory();
let a = h.add("one".into()).unwrap();
h.add("two".into()).unwrap();
h.toggle(a).unwrap();
assert_eq!(h.state().left(), 1);
h.clear_done().unwrap();
assert_eq!(h.state().tasks.len(), 1);
# }
```

## 3. Tests

Tests run against the real cell, in process. `Handle::memory()` for one
cell's calls; `aio::testing::TestCells` for cells with effects and
schedules on a virtual clock, and for seeded random calls against the
invariants:

```rust
# use aio::prelude::*;
# #[data]
# #[derive(Clone, Debug, Default, PartialEq)]
# pub struct Task {
#     pub id: Uuid,
#     pub title: String,
#     pub done: bool,
# }
# #[cell(persist, id = "todo")]
# pub struct Todo {
#     pub tasks: Vec<Task>,
# }
# #[methods]
# impl Todo {
#     pub fn add(&mut self, cx: &Cx, title: String) -> Result<Uuid> {
#         let title = title.trim().to_string();
#         ensure!(!title.is_empty(), "a task needs a title");
#         let id = cx.id();
#         self.tasks.push(Task { id, title, done: false });
#         Ok(id)
#     }
#     pub fn clear_done(&mut self) {
#         self.tasks.retain(|t| !t.done);
#     }
#     #[invariant]
#     fn at_most_500_tasks(&self) -> Result {
#         ensure!(self.tasks.len() <= 500, "a list holds at most 500 tasks");
#         Ok(())
#     }
# }
use aio::testing::{RandomCall, Rng, TestCells};
use serde_json::{Value, json};

fn title(r: &mut Rng) -> Value {
    let pick = ["", "milk", "tea", "  "][r.below(4) as usize];
    json!([pick])
}

fn nothing(_: &mut Rng) -> Value {
    json!([])
}

# fn main() {
// A state of your choosing, as a saved one would boot.
let mut t = TestCells::new();
let h = t.seeded::<Todo>(json!({ "tasks": [] }));
h.add("milk".into()).unwrap();
assert_eq!(h.state().tasks.len(), 1);

// 300 seeded random calls: a panic or a broken invariant names the seed.
let calls: [RandomCall; 2] = [("add", &title), ("clear_done", &nothing)];
let fuzzed = t.cell::<Todo>();
t.fuzz(&fuzzed, 7, 300, &calls, |s| s.tasks.iter().all(|t| !t.title.is_empty()));
# }
```

In the app these live in `#[cfg(test)] mod tests` beside the code, and
`ax test todo` runs them. More: [testing](../testing/README.md).

## 4. A schedule

A method can run on a cadence. Declared schedules are armed whenever the
cell is live and survive restarts; a missed run fires once:

```rust
# use aio::prelude::*;
# #[data]
# #[derive(Clone, Debug, Default, PartialEq)]
# pub struct Task {
#     pub id: Uuid,
#     pub title: String,
#     pub done: bool,
# }
# #[cell(persist, id = "todo")]
# pub struct Todo {
#     pub tasks: Vec<Task>,
# }
#[methods]
impl Todo {
#     pub fn add(&mut self, cx: &Cx, title: String) -> Result<Uuid> {
#         let title = title.trim().to_string();
#         ensure!(!title.is_empty(), "a task needs a title");
#         let id = cx.id();
#         self.tasks.push(Task { id, title, done: false });
#         Ok(id)
#     }
#     pub fn toggle(&mut self, id: Uuid) -> Result {
#         let Some(t) = self.tasks.iter_mut().find(|t| t.id == id) else { reject!("no task {id}") };
#         t.done = !t.done;
#         Ok(())
#     }
    /// Every day, finished tasks go.
    #[every("1d")]
    pub fn sweep(&mut self) {
        self.tasks.retain(|t| !t.done);
    }
}

# fn main() {
use aio::testing::TestCells;
let mut t = TestCells::new();
let h = t.cell::<Todo>();
let a = h.add("done soon".into()).unwrap();
h.toggle(a).unwrap();
t.advance(86_400_000); // a day on the virtual clock
assert!(h.state().tasks.is_empty());
# }
```

I/O (a fetch, a file) never goes in a method: it goes in an effect the
method asks for, whose result comes back as a call
([effects](../state/effects.md)).

## 5. A second cell

A list's statistics are not the list: they go in a cell of their own,
with its own id, saved and replayed on their own. One cell reacts to
another's committed calls with `listens_to`; the source never knows who
listens.

```rust
# use aio::prelude::*;
# #[data]
# #[derive(Clone, Debug, Default, PartialEq)]
# pub struct Task {
#     pub id: Uuid,
#     pub title: String,
#     pub done: bool,
# }
# #[cell(persist, id = "todo")]
# pub struct Todo {
#     pub tasks: Vec<Task>,
# }
# #[methods]
# impl Todo {
#     pub fn add(&mut self, cx: &Cx, title: String) -> Result<Uuid> {
#         let title = title.trim().to_string();
#         ensure!(!title.is_empty(), "a task needs a title");
#         let id = cx.id();
#         self.tasks.push(Task { id, title, done: false });
#         Ok(id)
#     }
# }
#[cell(persist, id = "stats")]
pub struct Stats {
    pub added: u32,
}

#[methods]
impl Stats {
    pub fn count_add(&mut self) {
        self.added += 1;
    }
}

# fn main() {
let (todo, stats) = (Handle::<Todo>::memory(), Handle::<Stats>::memory());
stats.listens_to(&todo, "add", |s, _event| s.count_add());

todo.add("tea".into()).unwrap();
assert!(todo.add("  ".into()).is_err()); // refused: nothing to react to
assert_eq!(stats.state().added, 1);
# }
```

- The reaction runs inside the source's commit, as a call on `stats` by
  the same caller: both commit in one transaction. A refused reaction is
  logged and never undoes the source's call.
- `_event` is the source's event (`aio::Event`): its method, arguments,
  caller and time.
- A misspelled method name panics as the cells are wired, so it can't
  silently never fire.
- Keep each cell's state its own: a cell never reads another's state in
  a method (that would break replay). Pass what it needs as arguments, or
  react with `listens_to`. More: [cells](../state/cells.md).

## 6. An async method

Adding tasks from elsewhere (a link, a file) needs I/O. An async method
is written in one piece; aio-rs splits it at each `let x: T = io.await;`,
runs the await as an effect, and records its result in the log, so
replay does no I/O.

```rust
# use aio::prelude::*;
# #[data]
# #[derive(Clone, Debug, Default, PartialEq)]
# pub struct Task {
#     pub id: Uuid,
#     pub title: String,
#     pub done: bool,
# }
#[cell(persist, id = "todo")]
pub struct Todo {
    pub tasks: Vec<Task>,
    pub importing: bool,
}

/// Stands in for an HTTP fetch: any future works.
async fn fetch_list(url: String) -> Result<Vec<String>, String> {
    if url.starts_with("https://") { Ok(vec!["milk".into(), "bread".into()]) } else { Err(format!("{url}: not https")) }
}

#[methods]
impl Todo {
    pub async fn import(&mut self, url: String) -> Result {
        ensure!(!self.importing, "an import is running");
        self.importing = true;
        let got: Result<Vec<String>, String> = fetch_list(url).await;
        self.importing = false;
        let titles = got.map_err(Rejected::new)?;
        for title in titles {
            let id = Uuid::from_u128(self.tasks.len() as u128 + 1);
            self.tasks.push(Task { id, title, done: false });
        }
        Ok(())
    }
}

# fn main() {
use aio::testing::TestCells;
let mut t = TestCells::new();
let h = t.cell::<Todo>();
h.import("https://example.org/list".into()).unwrap();
assert!(h.state().importing, "the first part committed at once");
t.settle(); // the effect ran, its result came back as a call
assert_eq!(h.state().tasks.len(), 2);
assert!(!h.state().importing);
# }
```

- The part before the await is one call (it sets `importing` at once, so
  a view shows a spinner); the part after it is another, called with
  the awaited value.
- `T` in `let x: T = ..` is how the result is saved in the log, so it is
  `Serialize`: a `Result<_, String>`, not an error type that isn't.
- A refusal after the await refuses that step only; what the first part
  committed stays (here `importing` is set back before the `?`).
- Only the arguments and typed `let`s live across the await. An `.await`
  inside an `if` or a loop is refused at build time.
- For a step-by-step process (a checkout, an onboarding), a state machine
  says which method may run in which state: [cells](../state/cells.md).
  More on effects, modes and retries: [effects](../state/effects.md).

## 7. The window

Views are `aio::ui` components (`use aio::ui::prelude::*`). `aio::client!`
connects them to the cell; `part(|c| &c.field)` is a handle on one field,
and a hole of `rsx!` reading it (`"{draft}"`, `disabled: ..`) runs again
only when it changes (a view's body runs once);
`act!(method(args))` calls a method from a handler:

```rust,no_run
use aio::act;
use aio::ui::prelude::*;

#[aio::data]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Task {
    pub id: aio::Uuid,
    pub title: String,
    pub done: bool,
}

#[aio::cell(persist, coalesce, id = "todo")]
pub struct Todo {
    pub tasks: Vec<Task>,
    pub draft: String,
}

#[aio::methods]
impl Todo {
    /// Every keystroke: shown at once, written within 100 ms (`coalesce`).
    pub fn type_draft(&mut self, text: String) {
        self.draft = text;
    }

    pub fn add(&mut self, cx: &aio::Cx) -> aio::Result {
        let title = self.draft.trim().to_string();
        aio::ensure!(!title.is_empty(), "a task needs a title");
        self.tasks.push(Task { id: cx.id(), title, done: false });
        self.draft.clear();
        Ok(())
    }

    pub fn toggle(&mut self, id: aio::Uuid) {
        if let Some(t) = self.tasks.iter_mut().find(|t| t.id == id) {
            t.done = !t.done;
        }
    }
}

aio::client!(Todo, todo);

#[component]
pub fn App() -> View {
    use_live(); // once, in the root
    rsx! {
        main { NewTask {} Tasks {} }
    }
}

#[component]
fn NewTask() -> View {
    let draft = part(|t| &t.draft);
    rsx! {
        form { onsubmit: move |e| { e.prevent_default(); _ = act!(add()); },
            input { "data-t": "title", placeholder: "What needs doing?", value: "{draft}",
                oninput: move |e| _ = act!(type_draft(e.value())) }
            button { r#type: "submit", class: aio::ui::btn("primary"), disabled: draft.with(|d| d.trim().is_empty()), "Add" }
        }
    }
}

#[component]
fn Tasks() -> View {
    let tasks = part(|t| &t.tasks);
    let left = memo(move || tasks.with(|ts| ts.iter().filter(|t| !t.done).count()));
    rsx! {
        ul { "data-t": "tasks",
            for t in tasks.get() {
                li { key: "{t.id}",
                    label {
                        input { r#type: "checkbox", checked: t.done,
                            onchange: { let id = t.id; move |_| _ = act!(toggle(id)) } }
                        "{t.title}"
                    }
                }
            }
        }
        p { "data-t": "left", "{left} left" }
    }
}

# fn main() {
// The window's test: the real page, headless.
let mut ui = aio::testing::TestUi::new(App);
ui.type_text("title", "buy milk");
ui.click("AddButton");
assert_eq!(ui.text("tasks"), "buy milk");
assert_eq!(ui.text("left"), "1 left");
# }
```

- Plain `cargo test` or `ax test todo` runs the window's test in the real
  page, headless: no screen needed.
- The draft lives in the cell, so it survives a restart; view-only state
  (an open menu) goes in a view's `signal`.
- State read at the top of a view's body is read once, as it mounts; the
  dev build names a body that reads the whole state (`todo()`). Read
  fields with `part`, in the holes that show them: a handle (`.get()`,
  `.with(|v| ..)`, `"{p}"`) that follows its field only.

More: [views](../ui/views.md), [the kit](../ui/kit.md) (inputs, lists,
dialogs, keys).

## 8. Run, inspect, ship

```sh
ax dev todo                     # the window; edits reach it live
ax state todo todo              # from another terminal: the live state
ax call todo todo type_draft '"tea"'
ax call todo todo add
ax events todo --last 5 --diff  # the log, each event with what it changed
ax test todo                    # the tests, the window's included
ax build todo                   # an installable app for this OS in dist/
```

The data lives in the app's home (`ax data todo` says where): quit and
start again, and the list is there. `ax timetravel todo todo undo` steps
the running cell back; `ax backup todo` copies its data while it runs.

## Common mistakes

- Reading the clock or a random number in a method: use `cx.now()`,
  `cx.id()`, `cx.rand()`, which the log records, so replay agrees.
- I/O in a plain method: it would run again on every replay and on every
  synced copy. Put it behind an await or in an effect.
- Changing `id = "todo"` after the app ran: the app starts on an empty
  list, the old data still under the old id. Change it back.
- Reading the whole state (`todo()`) in a view's body: read once, it never
  follows; in a hole it follows every commit. Read the fields it shows
  with `part`.
- One cell for everything: a cell is the unit of saving, syncing and
  history. Split by what changes together (the list, its statistics, the
  settings).

## Next

- Several devices on one list: [sync](../state/sync.md).
- Migrating the saved shape when `Task` changes:
  [persistence](../persistence/README.md).
- Every `aio.toml` key: [reference](../reference/aio-toml.md).
- What a running app did: [debugging](../debugging/README.md).

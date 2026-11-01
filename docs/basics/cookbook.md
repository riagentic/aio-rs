# Cookbook

Twenty-two things an app needs early, each as a complete example you can
copy. Every block on this page is a doctest of aio-rs: it compiles, and
the ones without `no_run` run and check what they claim. View recipes are
`no_run` like the other UI pages (they need the Electron runtime `ax
setup` fetches); `TestUi` runs the same code in an app's own tests.

The guides explain each piece; this page is the paste buffer. Where aio
has a feature aio-rs lacks, the recipe says so and shows the nearest way.

| # | Recipe | # | Recipe |
|---|---|---|---|
| 1 | [A list that persists itself](#1-a-list-that-persists-itself) | 12 | [Each user sees only their own rows](#12-each-user-sees-only-their-own-rows) |
| 2 | [View state the server never sees](#2-view-state-the-server-never-sees) | 13 | [Render for the signed-in user](#13-render-for-the-signed-in-user) |
| 3 | [A form that refuses to submit until valid](#3-a-form-that-refuses-to-submit-until-valid) | 14 | [A JSON endpoint](#14-a-json-endpoint) |
| 4 | [Show the change before the server confirms it](#4-show-the-change-before-the-server-confirms-it) | 15 | [An upload that keeps only metadata in state](#15-an-upload-that-keeps-only-metadata-in-state) |
| 5 | [One method using another](#5-one-method-using-another) | 16 | [A page title that follows the page](#16-a-page-title-that-follows-the-page) |
| 6 | [Answer an identical call from cache](#6-answer-an-identical-call-from-cache) | 17 | [A keyboard shortcut that belongs to no element](#17-a-keyboard-shortcut-that-belongs-to-no-element) |
| 7 | [Only the newest search wins](#7-only-the-newest-search-wins) | 18 | [A resource you must close](#18-a-resource-you-must-close) |
| 8 | [Cancel the load when the reader leaves](#8-cancel-the-load-when-the-reader-leaves) | 19 | [Keep one broken panel from taking the page](#19-keep-one-broken-panel-from-taking-the-page) |
| 9 | [A job every five minutes](#9-a-job-every-five-minutes) | 20 | [Heavy work off the cell's thread](#20-heavy-work-off-the-cells-thread) |
| 10 | [Migrate when the state's shape changes](#10-migrate-when-the-states-shape-changes) | 21 | [An external data source](#21-an-external-data-source) |
| 11 | [Keep a secret off the wire and out of reach](#11-keep-a-secret-off-the-wire-and-out-of-reach) | 22 | [A subprocess with a timeout and a stop button](#22-a-subprocess-with-a-timeout-and-a-stop-button) |

## 1. A list that persists itself

Add, toggle and remove, surviving a restart. All of it is methods on a
`persist` cell: each call commits to `state.db` before it answers.

```rust
use aio::prelude::*;

#[cell(persist, id = "tasks")]
pub struct Tasks {
    pub items: Vec<Task>,
    pub next: u32,
}

#[data]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Task {
    pub id: u32,
    pub text: String,
    pub done: bool,
}

#[methods]
impl Tasks {
    pub fn add(&mut self, text: String) -> Result<u32> {
        ensure!(!text.trim().is_empty(), "a task needs words");
        self.next += 1;
        self.items.push(Task { id: self.next, text, done: false });
        Ok(self.next)
    }

    pub fn toggle(&mut self, id: u32) -> Result {
        let Some(t) = self.items.iter_mut().find(|t| t.id == id) else { reject!("no task {id}") };
        t.done = !t.done;
        Ok(())
    }

    pub fn remove(&mut self, id: u32) {
        self.items.retain(|t| t.id != id);
    }
}

# fn main() {
let dir = tempfile::tempdir().unwrap();
let file = dir.path().join("state.db");
{
    let h = Handle::<Tasks>::open(&file).unwrap();
    let milk = h.add("milk".into()).unwrap();
    h.add("tea".into()).unwrap();
    h.toggle(milk).unwrap();
    h.remove(2).unwrap();
} // the app closes
let h = Handle::<Tasks>::open(&file).unwrap(); // and starts again
assert_eq!(h.state().items, [Task { id: 1, text: "milk".into(), done: true }]);
# }
```

- In an app the launcher opens the cell (`aio::Source::for_app`); every
  view showing it re-renders on each commit.
- A shared cell (`#[cell(persist, shared)]`) is the same list live on
  every synced window ([sync](../state/sync.md)).

[Cells](../state/cells.md), [persistence](../persistence/README.md).

## 2. View state the server never sees

A filter, a sort order, an open tab: per window, never saved, never
synced. That is view state, not a cell: `signal` in a component, or
a `Global` shared by the views of one window.

```rust,no_run
use aio::act;
use aio::ui::prelude::*;

#[aio::cell(persist, id = "tasks")]
pub struct Tasks {
    pub items: Vec<(String, bool)>,
}

#[aio::methods]
impl Tasks {
    pub fn add(&mut self, text: String) {
        self.items.push((text, false));
    }

    pub fn finish(&mut self, i: usize) {
        if let Some(t) = self.items.get_mut(i) {
            t.1 = true;
        }
    }
}

aio::client!(Tasks, tasks);

/// This window's filter: not in the cell, so not saved or sent anywhere.
static OPEN_ONLY: Global<bool> = Global::new(|| false);

#[component]
fn App() -> View {
    use_live();
    let items = part(|t| &t.items);
    let shown = move || items.get().into_iter().filter(|(_, done)| !OPEN_ONLY.get() || !done);
    rsx! {
        button { "data-t": "add", onclick: move |_| _ = act!(add("tea".into())), "Add" }
        button { "data-t": "finish", onclick: move |_| _ = act!(finish(0)), "Finish first" }
        label { input { r#type: "checkbox", "data-t": "open", checked: OPEN_ONLY.get(),
            onchange: move |e| *OPEN_ONLY.write() = e.checked() } "Open only" }
        ul { "data-t": "list", for (i, (text, _)) in shown().enumerate() { li { key: "{i}", "{text}" } } }
    }
}

# fn main() {
let mut ui = aio::testing::TestUi::new(App);
ui.click("add");
ui.click("add");
ui.click("finish");
ui.check("open");
assert_eq!(ui.text("list"), "tea");
assert_eq!(HANDLE.read_untracked().state().items.len(), 2, "the cell has both; the filter is the view's");
# }
```

- Gap: aio has `scope: "client"` cells (a cell with methods that never
  reaches the server). aio-rs has no such scope; a cell without `shared`
  is never on the wire, and one without `persist` is never on disk, but
  in a `client` window every cell is the server's. Keep window-only state
  in signals.

[Views](../ui/views.md#local-state-and-hooks).

## 3. A form that refuses to submit until valid

`use_form` holds the draft and one check function; `submit()` gives the
values only when the check finds nothing, and shows every error.

```rust,no_run
use aio::act;
use aio::kit::{Field, Input, Problems, use_form};
use aio::ui::prelude::*;

#[aio::cell(persist, id = "members")]
pub struct Members {
    pub names: Vec<String>,
}

#[aio::methods]
impl Members {
    pub fn join(&mut self, name: String) -> aio::Result {
        aio::ensure!(name.trim().len() >= 2, "a name, please");
        self.names.push(name);
        Ok(())
    }
}

aio::client!(Members, members);

#[derive(Clone, Default, PartialEq)]
struct Signup {
    name: String,
}

fn check(s: &Signup) -> Problems {
    let mut p = Problems::new();
    if s.name.trim().len() < 2 {
        p.push(("name", "Two letters at least".into()));
    }
    p
}

#[component]
fn App() -> View {
    use_live();
    let f = use_form(Signup::default, check);
    rsx! {
        form { "data-t": "form",
            onsubmit: move |e| {
                e.prevent_default();
                if let Some(ok) = f.submit() {
                    _ = act!(join(ok.name));
                }
            },
            Field { label: "Name", error: f.error("name"),
                Input { "data-t": "name", value: f.values().name, oninput: move |n| f.edit("name", |s| s.name = n) }
            }
            button { r#type: "submit", "data-t": "join", "Join" }
        }
    }
}

# fn main() {
let mut ui = aio::testing::TestUi::new(App);
ui.click("join");
assert!(ui.text("form").contains("Two letters at least"), "the error shows on a submit");
assert!(HANDLE.read_untracked().state().names.is_empty(), "nothing was called");
ui.type_text("name", "Ada");
ui.click("join");
assert_eq!(HANDLE.read_untracked().state().names, ["Ada"]);
# }
```

- The view checks so people see why; the method checks again
  (`ensure!`), because any door can call it.
- A rule that waits (a name already taken): `use_async_rule`.

[Kit: forms](../ui/kit.md#forms).

## 4. Show the change before the server confirms it

A synced copy predicts every call it makes: the state changes at once,
the server confirms it, and a refusal rolls it back. Nothing to write.

```rust
use aio::prelude::*;
use aio::testing::TestServer;

#[cell(persist, shared)]
pub struct Likes {
    pub n: u32,
}

#[methods]
impl Likes {
    #[client]
    pub fn like(&mut self) {
        self.n += 1;
    }

    /// Not `#[client]`: the server refuses it from the network.
    pub fn reset(&mut self) {
        self.n = 0;
    }
}

# fn main() {
let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
rt.block_on(async {
    let server = TestServer::start(Handle::<Likes>::memory(), Default::default()).await.unwrap();
    let ana = server.client("ana").await.unwrap();
    ana.like().unwrap();
    assert_eq!(ana.state().n, 1, "shown before the server answered");
    server.converged(&[&ana]).await;
    _ = ana.reset(); // predicted, then refused by the server
    server.converged(&[&ana]).await;
    assert_eq!(ana.state().n, 1, "rolled back");
});
# }
```

- `#[client(predict = false)]` waits for the server instead (payments).
  `remote.call_confirmed(..)` waits for one call's answer.
- A standalone app commits in-process before the call returns, so there
  is nothing to predict.
- Gap: aio's `useOptimistic` (a pending value layered over any state in
  a view) has no aio-rs hook; prediction lives in the synced copy.

[Sync: offline and reconnects](../state/sync.md#offline-and-reconnects).

## 5. One method using another

Inside a method, another method of the same cell is a plain Rust call:
it runs on the same copy, in the same commit, and sees your writes. A
refusal anywhere refuses the whole call.

```rust
use aio::prelude::*;

#[cell]
pub struct Bench {
    pub status: String,
    pub samples: Vec<(String, u32)>,
}

#[methods]
impl Bench {
    pub fn record(&mut self, kind: String, n: u32) -> Result {
        ensure!(n > 0, "an empty sample");
        self.samples.push((kind, n));
        Ok(())
    }

    /// Two records and a status: one event, all or nothing.
    pub fn run(&mut self, n: u32) -> Result {
        self.status = "running".into();
        self.record("warm".into(), n)?;
        self.record("hot".into(), n * 2)?;
        self.status = "done".into();
        Ok(())
    }
}

# fn main() {
let h = Handle::<Bench>::memory();
h.run(3).unwrap();
assert_eq!(h.state().samples.len(), 2);
assert!(h.run(0).is_err());
assert_eq!((h.state().samples.len(), h.state().status.as_str()), (2, "done"), "the refused run changed nothing");
# }
```

- A helper that is not a method (no `pub`, or outside `#[methods]`)
  works the same way and is not callable from outside.
- Another cell is never called from a method: use
  `listens_to` or an effect ([composition](../state/cells.md#composition)).

## 6. Answer an identical call from cache

`#[ttl(ms)]` on an async method: a call with the same arguments within
`ms` of a finished one answers `Ok(())` without running again.

```rust
use aio::prelude::*;
use aio::testing::TestCells;

#[cell]
pub struct Weather {
    pub temp: Option<i32>,
    pub fetches: u32,
}

/// Stands in for an HTTP call.
async fn fetch_temp(city: String) -> i32 {
    city.len() as i32
}

#[methods]
impl Weather {
    #[ttl(60_000)]
    pub async fn forecast(&mut self, city: String) -> Result {
        let t: i32 = fetch_temp(city).await;
        self.temp = Some(t);
        self.fetches += 1;
        Ok(())
    }
}

# fn main() {
let mut t = TestCells::new();
let h = t.cell::<Weather>();
h.forecast("Oslo".into()).unwrap();
t.settle();
h.forecast("Oslo".into()).unwrap(); // within a minute: from cache
t.settle();
assert_eq!(h.state().fetches, 1);
t.advance(61_000);
h.forecast("Oslo".into()).unwrap(); // expired: fetched again
t.settle();
assert_eq!(h.state().fetches, 2);
# }
```

- Only successful runs are cached; a refusal is not.
- `#[ttl]` needs an `.await` in the method (the build says so).

[Async methods](../state/effects.md#async-methods).

## 7. Only the newest search wins

Every keystroke starts a search; the last one's answer must win, not the
last to arrive. `Mode::Newest` cancels the running effect with the same
key.

```rust
use aio::prelude::*;
use aio::testing::TestCells;
use std::time::Duration;

#[cell]
pub struct Search {
    pub query: String,
    pub hits: Vec<String>,
    pub answers: u32,
}

async fn lookup(q: String) -> Vec<String> {
    aio::time::sleep(Duration::from_millis(200)).await;
    vec![format!("{q}-1"), format!("{q}-2")]
}

#[methods]
impl Search {
    pub fn typed(&mut self, cx: &Cx, q: String) {
        self.query = q.clone();
        cx.run(
            Effect::new("search", move |h: Handle<Search>| async move {
                let hits = lookup(q).await;
                _ = h.found(hits);
            })
            .mode(Mode::Newest),
        );
    }

    pub fn found(&mut self, hits: Vec<String>) {
        self.hits = hits;
        self.answers += 1;
    }
}

# fn main() {
let mut t = TestCells::new();
let h = t.cell::<Search>();
h.typed("t".into()).unwrap();
h.typed("te".into()).unwrap();
h.typed("tea".into()).unwrap();
t.settle();
assert_eq!(h.state().hits, ["tea-1", "tea-2"]);
assert_eq!(h.state().answers, 1, "the older searches were cancelled");
# }
```

- `Mode::First` is the opposite: a refresh already running makes the new
  one a no-op.
- Debounce instead of cancel: `cx.after("search", 300, "run", (q,))`
  ([schedules](../state/scheduling.md#schedules-from-a-call)).

[Effects: modes](../state/effects.md#modes).

## 8. Cancel the load when the reader leaves

The method that leaves aborts the effect by its key; the effect is
dropped at its next await and its result never arrives.

```rust
use aio::prelude::*;
use std::time::Duration;

#[cell]
pub struct Article {
    pub open: Option<u32>,
    pub body: Option<String>,
    pub loading: bool,
}

#[methods]
impl Article {
    pub fn show(&mut self, cx: &Cx, id: u32) {
        (self.open, self.body, self.loading) = (Some(id), None, true);
        cx.run(Effect::new("load", move |h: Handle<Article>| async move {
            aio::time::sleep(Duration::from_millis(200)).await; // a slow download
            _ = h.loaded(format!("article {id}"));
        }));
    }

    pub fn loaded(&mut self, body: String) {
        (self.body, self.loading) = (Some(body), false);
    }

    pub fn close(&mut self, cx: &Cx) {
        cx.abort("load");
        (self.open, self.loading) = (None, false);
    }
}

# fn main() {
let mut t = aio::testing::TestCells::new();
let h = t.cell::<Article>();
h.show(7).unwrap();
assert_eq!(t.effects(), ["Article.load"]);
h.close().unwrap(); // aborted before it started: it never runs
assert!(t.effects().is_empty());
t.settle();
assert_eq!((h.state().body.clone(), h.state().loading), (None, false));
# }
```

- Gap: aio's `cancelOn` lists, per async method, the calls that abort it.
  aio-rs has no such list: the leaving method calls `cx.abort(key)`.
- An async method's effects are keyed by the method's name, so
  `cx.abort("show")` stops one written as `pub async fn show`.

[Effects](../state/effects.md#an-effect).

## 9. A job every five minutes

`#[every("5m")]` arms a method whenever the cell is live; runs missed
while the app was off fire once at start. `skip_if_running` skips a tick
while the last one's effect still runs.

```rust
use aio::prelude::*;
use aio::testing::TestCells;

#[cell(persist)]
pub struct Digest {
    pub sent: u32,
    pub last: i64,
}

#[methods]
impl Digest {
    #[every("5m", skip_if_running)]
    pub fn send(&mut self, cx: &Cx) {
        self.sent += 1;
        self.last = cx.now();
    }
}

# fn main() {
let mut t = TestCells::new();
let h = t.cell::<Digest>();
for _ in 0..3 {
    t.advance(5 * 60_000);
}
assert_eq!(h.state().sent, 3);
# }
```

- A schedule a call sets (and so can stop): `cx.every("digest", 300_000,
  "send", ())`, `cx.cancel("digest")`. At a wall-clock time:
  `#[cron("0 9 * * 1")]` (UTC, as aio; `[app] tz = "local"` for local time).
- `ax schedules <app>` lists what is armed.
- Never a thread with a sleep loop: replay and tests can't see it.

[Schedules](../state/effects.md#schedules).

## 10. Migrate when the state's shape changes

Adding a field needs nothing (it comes up at its default). A rename or a
type change bumps `version` and gives `migrate`, which gets the stored
JSON and the version it was saved at.

```rust
use aio::prelude::*;
use aio::testing::TestCells;
use serde_json::{Value, json};

fn up(old: Value, from: u32) -> std::result::Result<Prefs, String> {
    match from {
        // version 1 had `dark: bool`; version 2 has `theme: String`
        1 => Ok(Prefs { theme: if old["dark"] == json!(true) { "dark" } else { "light" }.into() }),
        _ => serde_json::from_value(old).map_err(|e| e.to_string()),
    }
}

#[cell(persist, id = "prefs", version = 2, migrate = up)]
pub struct Prefs {
    pub theme: String,
}

#[methods]
impl Prefs {}

# fn main() {
let mut t = TestCells::new();
let h = t.saved::<Prefs>(1, json!({"dark": true})).unwrap(); // boots as the app would
assert_eq!(h.state().theme, "dark");
# }
```

- Data that doesn't fit refuses the boot, after a backup; it is never
  reset. `ax migrations <app>` shows stored versions and shape drift.
- Renaming a field without a version bump loses its value at the next
  save.

[Persistence: shape changes](../persistence/README.md).

## 11. Keep a secret off the wire and out of reach

Reads and calls are guarded separately. `#[server]` keeps a field off
every client; only `#[client]` methods are callable from the network;
`#[redact]` keeps a method's arguments out of the log.

```rust
use aio::prelude::*;
use aio::testing::TestCells;
use aio::CellData;

#[cell(persist, shared)]
pub struct Vault {
    pub label: String,
    #[server]
    pub api_key: Hidden<String>, // saved, never sent
}

#[methods]
impl Vault {
    #[client]
    pub fn rename(&mut self, label: String) {
        self.label = label;
    }

    /// Not `#[client]`: only the app's own code may set it.
    #[redact]
    pub fn set_key(&mut self, key: String) {
        *self.api_key = key;
    }
}

# fn main() {
let mut t = TestCells::new();
let h = t.cell::<Vault>();
h.set_key("sk-test".into()).unwrap();
let ana = t.as_client(&h, Caller { by: By::User("ana".into()), role: None });
ana.rename("billing".into()).unwrap();
assert!(ana.set_key("mine".into()).is_err(), "not callable from the network");
let seen = h.state().project(&Caller { by: By::User("ana".into()), role: None });
assert!(seen.get("api_key").is_none(), "never sent");
assert_eq!(*h.state().api_key, "sk-test");
# }
```

- A shared field named like a credential (`api_key`, `token`,
  `password`…) without `#[server]` fails the build.
- A `#[server]` field of a shared cell is an `aio::Hidden<T>`: it reads
  as its value in the app; on a synced copy, which never has it, a read
  panics naming the field (aio throws), so a method that touches it is
  `#[client(predict = false)]`.
- A `#[client]` method's answer goes back to the caller: never return a
  secret from one.

[Sync: who may call what](../state/sync.md#who-may-call-what),
[security](../auth/README.md).

## 12. Each user sees only their own rows

`visible = f` runs for every viewer on every change and returns what that
viewer is sent. The server keeps every row; each client gets its slice,
as patches of its own view.

```rust
use aio::prelude::*;
use aio::CellData;

#[cell(persist, shared, visible = own)]
pub struct Notes {
    pub rows: Vec<Note>,
}

#[data]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Note {
    pub owner: String,
    pub text: String,
}

fn own(n: &Notes, who: &By) -> Notes {
    let mine = |r: &&Note| matches!(who, By::User(u) if *u == r.owner);
    Notes { rows: n.rows.iter().filter(mine).cloned().collect() }
}

#[methods]
impl Notes {
    #[client]
    pub fn write(&mut self, cx: &Cx, text: String) -> Result {
        let By::User(owner) = cx.by() else { reject!("sign in first") };
        self.rows.push(Note { owner: owner.clone(), text });
        Ok(())
    }
}

# fn main() {
let n = Notes {
    rows: vec![
        Note { owner: "ana".into(), text: "a".into() },
        Note { owner: "bo".into(), text: "b".into() },
    ],
};
let ana = n.project(&Caller { by: By::User("ana".into()), role: None });
assert_eq!(ana["rows"].as_array().unwrap().len(), 1);
assert_eq!(ana["rows"][0]["text"], "a");
# }
```

- `visible` may take `&Caller` to decide by role too.
- Without `[accounts]` a key holder can say any name: turn accounts on
  before trusting `By::User` ([security](../auth/README.md)).

[Sync: what each viewer sees](../state/sync.md#what-each-viewer-sees).

## 13. Render for the signed-in user

Who is signed in has three answers: not known yet, nobody, someone.
Render all three, so a signed-in app never flashes its sign-in form while
the answer is on its way.

```rust,no_run
use aio::ui::prelude::*;

/// `None`: still asking; `Some(None)`: signed out; `Some(Some((id, role)))`.
#[component]
fn Account(who: Option<Option<(String, String)>>) -> View {
    rsx! {
        div { "data-t": "account",
            match who {
                None => rsx! { aio::kit::Spinner {} },
                Some(None) => rsx! { a { href: "/signin", "Sign in" } },
                Some(Some((id, _role))) => rsx! { "Signed in as {id}" },
            }
        }
    }
}

# fn main() {
let ui = aio::testing::TestUi::with(Account, AccountProps { who: Some(Some(("ana".into(), "user".into()))) });
assert_eq!(ui.text("account"), "Signed in as ana");
# }
```

- In the browser build `aio::web::use_user()` gives exactly that value
  and `aio::web::sign_out()` ends the session. `aio::kit::SignIn {
  client, on_signed }` is the form itself, on an `aio::auth::client::Client`.
- Gap: aio's `useUser` works in every window. In an aio-rs desktop or
  Android window there is no such hook; the user is the server's
  `Caller` (`cx.by()`, `cx.role()`), and a view shows what `visible`
  sent it for that user.

[Security: accounts](../auth/README.md).

## 14. A JSON endpoint

State travels on the sync wire; everything else (a webhook, an API for
somebody's script) is a route. `Route::get` and `Route::post` take a
path with `:params`; `Req::json` reads the body, `Reply::json` answers.

```rust
use aio::http::{Http, Reply, Route};
use aio::prelude::*;
use aio::testing::TestServer;

#[cell(persist, shared)]
pub struct Notes {
    pub rows: Vec<String>,
}

#[methods]
impl Notes {
    pub fn add(&mut self, text: String) -> Result<usize> {
        ensure!(!text.is_empty(), "empty note");
        self.rows.push(text);
        Ok(self.rows.len() - 1)
    }
}

#[data]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NewNote {
    pub text: String,
}

fn http(h: Handle<Notes>) -> Http {
    let read = h.clone();
    Http::default()
        .route(Route::get("/api/notes/:id", move |req| {
            let id: Option<usize> = req.param("id").and_then(|s| s.parse().ok());
            let row = id.and_then(|i| read.state().rows.get(i).cloned());
            async move {
                match row {
                    Some(text) => Reply::json(&serde_json::json!({ "text": text })),
                    None => Reply::text("no such note").status(404),
                }
            }
        }))
        .route(Route::post("/api/notes", move |req| {
            let made = req.json::<NewNote>().and_then(|n| h.add(n.text));
            async move {
                match made {
                    Ok(id) => Reply::json(&serde_json::json!({ "id": id })).status(201),
                    Err(e) => Reply::text(e.why).status(400),
                }
            }
        }))
}

# fn main() {
let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
rt.block_on(async {
    let h = Handle::<Notes>::memory();
    let server = TestServer::start(h.clone(), http(h.clone())).await.unwrap();
    let made = server.fetch("POST", "/api/notes", r#"{"text":"hello"}"#).await.unwrap();
    assert_eq!((made.status, made.body.as_str()), (201, r#"{"id":0}"#));
    let got = server.fetch("GET", "/api/notes/0", "").await.unwrap();
    assert_eq!(got.body, r#"{"text":"hello"}"#);
    assert_eq!(server.fetch("GET", "/api/notes/9", "").await.unwrap().status, 404);
    assert_eq!(server.fetch("POST", "/api/notes", r#"{"text":""}"#).await.unwrap().status, 400);
});
# }
```

- Routes sit on the served cell's server. A cell without `shared` serves
  its routes (and files, health, sign-in) but never its state: no `/ws`
  sync, no remote UI; add `shared` for those.
- Routes need the app's key unless `.public()` (then check a signature
  yourself, as a webhook does). `req.user()` and `req.role()` say who
  called with accounts on.
- Typed calls from an aio-rs client are better as a server function
  (`aio::fns::ServerFn`): one declaration types both ends.

[Sync: server functions and routes](../state/sync.md#server-functions-and-routes).

## 15. An upload that keeps only metadata in state

Bytes never go in a cell (it would save, copy and sync them). The route
puts them in the blob store; the cell keeps the blob's id, name and size,
which sync to every client for free.

```rust
use aio::blobs::{Blob, Blobs};
use aio::http::{Http, Reply, Route};
use aio::prelude::*;
use aio::testing::TestServer;

#[cell(persist, shared)]
pub struct Files {
    pub list: Vec<Blob>,
}

#[methods]
impl Files {
    pub fn uploaded(&mut self, blob: Blob) {
        if !self.list.iter().any(|b| b.id == blob.id) {
            self.list.push(blob);
        }
    }
}

fn http(h: Handle<Files>, blobs: Blobs) -> Http {
    let store = blobs.clone();
    Http::default().blobs(blobs).route(
        Route::post("/upload/:name", move |req| {
            let put = store.put(req.body(), req.param("name"));
            let answer = match put {
                Ok(blob) => {
                    let id = blob.id.clone();
                    _ = h.uploaded(blob);
                    Reply::json(&serde_json::json!({ "id": id, "url": aio::blobs::url(&id) }))
                }
                Err(e) => Reply::text(e.to_string()).status(500),
            };
            async move { answer }
        })
        .max_body(20 << 20), // 20 MB
    )
}

# fn main() {
let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
rt.block_on(async {
    let dir = tempfile::tempdir().unwrap();
    let h = Handle::<Files>::memory();
    let server = TestServer::start(h.clone(), http(h.clone(), Blobs::at(dir.path().join("blobs")))).await.unwrap();
    let up = server.fetch("POST", "/upload/notes.txt", "hello").await.unwrap();
    assert_eq!(up.status, 200);
    let f = &h.state().list[0];
    assert_eq!((f.name.as_deref(), f.size), (Some("notes.txt"), 5));
    let back = server.fetch("GET", &aio::blobs::url(&f.id), "").await.unwrap();
    assert_eq!(back.body, "hello", "served from the blob store, Range and all");
});
# }
```

- Big files: `.streamed()` on the route and `req.body_reader()` read in
  `spawn_blocking` straight into `put`, a few chunks of memory at a time.
- In an app the store is `Blobs::of(&dirs)`, in `data/files/blobs`.

[Blobs, files and big data](../persistence/sql.md#blobs-files-and-big-data).

## 16. A page title that follows the page

`use_head(Head::title(..))` owns the window's title and `<meta>` tags
while its component lives and gives them back at unmount, so two pages
never fight. The innermost owner wins; the rules are `aio::head::Heads`:

```rust
use aio::head::{Head, Heads};

# fn main() {
let mut heads = Heads::default();
let (layout, page) = (heads.owner(), heads.owner());
heads.set(layout, Head::title("Shop"));
heads.set(page, Head::title("Green tea · Shop").meta(&[("name", "description"), ("content", "Loose leaf")]));
assert_eq!(heads.merged().title.as_deref(), Some("Green tea · Shop"));
heads.remove(page); // the page unmounts
assert_eq!(heads.merged().title.as_deref(), Some("Shop"));
# }
```

- In a window: `aio_shell::page::use_head(Head::title(format!("{} · Shop",
  post.title)))` at the top of the page component; in the browser build
  `aio::head::use_head`.
- With `routes!`, a page variant gives its own head (`page.head()`), so
  the title follows the route without a hook
  ([routing](../ui/routing.md)).

[Views: the page](../ui/views.md#the-page).

## 17. A keyboard shortcut that belongs to no element

`use_global_key` binds a chord for as long as its component is mounted,
under the root's `GlobalKeys`. It stays quiet while typing in a field.

```rust,no_run
use aio::kit::{GlobalKeys, use_global_key};
use aio::ui::prelude::*;

static PALETTE: Global<bool> = Global::new(|| false);

#[component]
fn App() -> View {
    rsx! { GlobalKeys { Palette {} } }
}

#[component]
fn Palette() -> View {
    use_global_key("mod+k", |_| *PALETTE.write() = true); // Ctrl+K, ⌘K on macOS
    use_global_key("esc", |_| *PALETTE.write() = false);
    rsx! {
        p { "data-t": "palette", if PALETTE.get() { "open" } else { "closed" } }
        input { "data-t": "search" }
    }
}

# fn main() {
let mut ui = aio::testing::TestUi::new(App);
ui.press("palette", "ctrl+k");
assert_eq!(ui.text("palette"), "open");
ui.press("palette", "esc");
assert_eq!(ui.text("palette"), "closed");
# }
```

- A key the browser would act on first (Tab, arrows scrolling): put
  `data-aio-prevent` on the element; `e.prevent_default()` comes too late.
- `ax trigger <app> window press ctrl+k` presses it in the running app.

[Kit: keys](../ui/kit.md#keys).

## 18. A resource you must close

A camera, a socket, a file handle. `aio::held::Held` opens one per key,
shares it between holders and closes it when the last one lets go;
`use_held` holds it for a view, re-opening when the key changes. Never
open it in the render or in `on_cleanup`'s pair by hand.

```rust,no_run
use aio::held::{Held, State};
use aio::kit::use_held;
use aio::ui::prelude::*;
use std::sync::OnceLock;

/// Stands in for a device: opening takes a moment, closing is said.
fn cameras() -> &'static Held<String, String> {
    static CAMS: OnceLock<Held<String, String>> = OnceLock::new();
    CAMS.get_or_init(|| {
        Held::new(
            |name: String| async move { Ok::<_, String>(format!("camera {name}")) },
            |_cam, _name| async {},
        )
    })
}

#[component]
fn App() -> View {
    let on = signal(|| true);
    rsx! {
        button { "data-t": "toggle", onclick: move |_| on.toggle(), "Toggle" }
        if on.get() { Preview { name: "front" } }
    }
}

#[component]
fn Preview(name: String) -> View {
    let text = match use_held(cameras(), Some(name)) {
        Some(State::Open(cam)) => cam.to_string(),
        Some(State::Failed(why)) => format!("failed: {why}"),
        _ => "opening".into(),
    };
    rsx! { p { "data-t": "preview", "{text}" } }
}

# fn main() {
let mut ui = aio::testing::TestUi::new(App);
ui.until_text("preview", "camera front");
ui.click("toggle"); // the view unmounts
ui.wait(std::time::Duration::from_millis(20));
assert_eq!(cameras().len(), 0, "closed with its last holder");
# }
```

- A resource a cell owns (a socket an `#[on_start]` opened): `h.own(key,
  Some(Box::new(res)))`, dropped when the cell stops or is disabled
  ([owned resources](../state/cells.md#owned-resources)).
- A resource that follows a value outside views: `h.on_change(..)` keeps
  what its reaction returns until the next change
  ([effects](../state/effects.md#reacting-outside-a-cell)).

## 19. Keep one broken panel from taking the page

A component that fails with no boundary above it takes its whole tree.
`ErrorBoundary` catches a child's render error and shows its `fallback`
in its place; the rest of the page goes on.

```rust,no_run
use aio::ui::prelude::*;

#[component]
fn Dashboard() -> View {
    rsx! {
        ErrorBoundary {
            fallback: |_: String| rsx! { p { "data-t": "fallback", "This panel failed." } },
            Chart { data: "1,2,x".to_string() }
        }
        p { "data-t": "rest", "The rest works" }
    }
}

#[component]
fn Chart(data: String) -> View {
    let points: Vec<i32> = data.split(',').map(str::parse).collect::<Result<_, _>>()?; // a render error
    rsx! { p { "{points.len()} points" } }
}

# fn main() {
let ui = aio::testing::TestUi::new(Dashboard);
assert_eq!(ui.text("fallback"), "This panel failed.");
assert_eq!(ui.text("rest"), "The rest works");
# }
```

- Event handlers are not caught (as in React and aio): a refused call
  from `act!` is logged and gives `None`.
- A panic in a method or an effect fails that call only, never the view.

## 20. Heavy work off the cell's thread

A parse, a crunch, a blocking library call. Effects already run off the
UI thread; CPU-heavy work goes in `spawn_blocking` inside one, and the
result comes back as a call.

```rust
use aio::prelude::*;
use aio::testing::TestCells;

#[cell]
pub struct Thumbs {
    pub busy: bool,
    pub sums: Vec<u64>,
}

/// Stands in for image work that blocks.
fn crunch(n: u64) -> u64 {
    (1..=n).map(|i| i * i).sum()
}

#[methods]
impl Thumbs {
    pub fn make(&mut self, cx: &Cx, sizes: Vec<u64>) {
        self.busy = true;
        cx.run(Effect::new("make", move |h: Handle<Thumbs>| async move {
            let out = tokio::task::spawn_blocking(move || sizes.into_iter().map(crunch).collect()).await;
            _ = h.made(out.unwrap_or_default());
        }));
    }

    pub fn made(&mut self, sums: Vec<u64>) {
        (self.busy, self.sums) = (false, sums);
    }
}

# fn main() {
let mut t = TestCells::new();
let h = t.cell::<Thumbs>();
h.make(vec![3, 10]).unwrap();
assert!(h.state().busy);
t.settle();
assert_eq!(h.state().sums, [14, 385]);
# }
```

- Other cells keep committing while it runs: only this effect waits.
- Gap: aio's `worker: true` runs a whole cell's methods in their own
  isolate. aio-rs has no per-cell thread: methods are fast by rule, and
  the slow part is an effect.

[Effects: heavy work](../state/effects.md#heavy-work-retries-and-timeouts).

## 21. An external data source

The shape every dashboard tile repeats: fetch, normalize, record a
status, keep the last good data when the source fails. The I/O is an
effect, so a slow source never holds a call open.

```rust
use aio::prelude::*;
use aio::testing::TestCells;

#[cell(persist)]
pub struct Feed {
    pub url: String,
    pub items: Vec<String>,
    pub status: Status,
}

#[data]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    pub at: i64,
    pub ok: bool,
    pub err: Option<String>,
    pub busy: bool,
}

/// Stands in for an HTTP GET and its parsing.
async fn fetch(url: String) -> std::result::Result<Vec<String>, String> {
    if url.contains("down") { Err("503 from the source".into()) } else { Ok(vec![format!("{url}/1")]) }
}

#[methods]
impl Feed {
    pub fn point(&mut self, cx: &Cx, url: String) {
        self.url = url;
        self.refresh(cx);
    }

    #[every("5m", skip_if_running)]
    pub fn refresh(&mut self, cx: &Cx) {
        self.status.busy = true;
        let url = self.url.clone();
        cx.run(
            Effect::new("fetch", move |h: Handle<Feed>| async move {
                _ = h.got(fetch(url).await);
            })
            .mode(Mode::First),
        );
    }

    pub fn got(&mut self, cx: &Cx, r: std::result::Result<Vec<String>, String>) {
        self.status = Status { at: cx.now(), ok: r.is_ok(), err: r.as_ref().err().cloned(), busy: false };
        if let Ok(items) = r {
            self.items = items; // a failure keeps the last good data
        }
    }
}

# fn main() {
let mut t = TestCells::new();
let h = t.cell::<Feed>();
h.point("https://example.org/feed".into()).unwrap();
t.settle();
assert_eq!(h.state().items, ["https://example.org/feed/1"]);
h.point("https://example.org/down".into()).unwrap();
t.settle();
let s = h.state();
assert_eq!((s.status.ok, s.status.err.as_deref()), (false, Some("503 from the source")));
assert_eq!(s.items, ["https://example.org/feed/1"], "the last good data stays");
# }
```

- A real fetch goes through aio's HTTPS client (feature `outbound`),
  held to `[capabilities] network`; `aio::effect::retry` retries it.
- `aio::cassette::Cassette` records the source once and replays it in
  tests ([testing](../testing/README.md)).

## 22. A subprocess with a timeout and a stop button

Run a tool, keep what it wrote even when it is cut short. The effect
watches the child off the async threads, kills it at the deadline or when
the cell says stop, and hands back the output so far.

```rust
use aio::prelude::*;
use aio::testing::TestCells;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[cell]
pub struct Tool {
    pub running: bool,
    pub stop: bool,
    pub output: String,
    pub ended: String,
}

/// Runs `cmd`, killed after `limit` or once `stop()` says so; the output so far either way.
fn run(cmd: &str, args: &[&str], limit: Duration, stop: impl Fn() -> bool) -> (String, String) {
    let mut child = match Command::new(cmd).args(args).stdout(Stdio::piped()).spawn() {
        Ok(c) => c,
        Err(e) => return (String::new(), format!("could not start: {e}")),
    };
    let t0 = Instant::now();
    let ended = loop {
        match child.try_wait() {
            Ok(Some(code)) => break format!("exited: {code}"),
            Err(e) => break format!("lost: {e}"),
            Ok(None) if stop() => break "stopped".into(),
            Ok(None) if t0.elapsed() > limit => break "timed out".into(),
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    _ = child.kill();
    _ = child.wait();
    let mut out = String::new();
    _ = child.stdout.take().map(|mut s| s.read_to_string(&mut out));
    (out, ended)
}

#[methods]
impl Tool {
    pub fn launch(&mut self, cx: &Cx, limit_ms: u64) -> Result {
        ensure!(!self.running, "already running");
        (self.running, self.stop) = (true, false);
        cx.run(Effect::new("tool", move |h: Handle<Tool>| async move {
            let watch = h.clone();
            let (out, ended) = tokio::task::spawn_blocking(move || {
                // `exec`: the shell becomes the long command, so a kill ends it.
                run("sh", &["-c", "echo first; exec sleep 5"], Duration::from_millis(limit_ms), || watch.state().stop)
            })
            .await
            .unwrap_or_default();
            _ = h.finished(out, ended);
        }));
        Ok(())
    }

    pub fn stop(&mut self) {
        self.stop = true;
    }

    pub fn finished(&mut self, output: String, ended: String) {
        (self.running, self.output, self.ended) = (false, output, ended);
    }
}

# fn main() {
# if !cfg!(unix) { return; }
let mut t = TestCells::new();
let h = t.cell::<Tool>();
h.launch(300).unwrap();
t.settle();
assert_eq!((h.state().output.as_str(), h.state().ended.as_str()), ("first\n", "timed out"));
# }
```

- The stop button calls `stop()`: the watcher sees it within 20 ms.
- A kill ends the child, not processes it started: `exec`, or a process
  group, keeps them one.
- Gap: aio passes an abort signal to `Deno.Command`; aio-rs has no
  subprocess helper (and no async process API in its runtime), so the
  wait loop is yours, as above.

## Where to go next

- [Cells and methods](../state/cells.md), [effects](../state/effects.md),
  [sync](../state/sync.md): each part in depth.
- [aio.toml reference](../reference/aio-toml.md): every key.
- [Testing](../testing/README.md): how the checks above are written.
- [FAQ](faq.md): questions and mistakes.

# What each viewer sees, and where code runs

A cell's state lives in one place, the app (or its server). Everyone
else, a synced window, a phone, a page, a tool, gets a *view* of it: the
state with the `#[server]` fields taken out, filtered by the cell's
`visible` function for that viewer. This page covers views in depth, how
they relate to saving, and the contexts a cell's code runs in. Access to
*calls* is a separate question, answered by `#[client]`
([who may call what](sync.md#who-may-call-what)).

| Need | Use |
|---|---|
| a field the app keeps and saves, never sent | `#[server]` on the field |
| a field kept in memory only, never saved | `#[transient]` on the field |
| a field neither saved nor sent (scratch) | both |
| each user sees their own rows | `#[cell(visible = f)]`, `f(&Self, &By) -> Self` |
| what a user sees depends on their role | `visible = f` with `f(&Self, &Caller) -> Self` |
| what a user may *call* | `#[client]`, `#[client(role = .., allow = ..)]` |
| reshape what is saved | `#[cell(save = f)]` ([persistence](../persistence/README.md)) |

## Reads and calls are two questions

`visible` and `#[server]` decide what a viewer *reads*. `#[client]`
decides what the network may *call*. Neither implies the other:

- A method without `#[client]` can't be called from the network at all:
  a client, a peer and a release build's local door are refused. That is
  the default, so a cell's methods are closed until you open them.
- A `#[client]` method's return value goes back to its caller whatever
  `visible` says. A method that returns a hidden field (`fn token_of(&mut
  self, id: u32) -> Result<String>`) hands it out as surely as sending
  it. Return facts (`has_token: bool`), never the secret.
- `visible` hides nothing from the app's own code: methods, effects,
  routes and server functions read the whole state.

## Server fields, transient fields

```rust
use aio::prelude::*;

#[cell(persist, shared)]
pub struct Mailbox {
    pub subjects: Vec<String>,
    /// Kept and saved; never sent to a client, a tool or a report.
    #[server]
    pub imap_password: Hidden<String>,
    /// The fact clients read, published beside the secret.
    pub can_sync: bool,
    /// Sent; never saved: back to the default on every start.
    #[transient]
    pub syncing: bool,
    /// Neither: scratch the methods use (the last count seen).
    #[server]
    #[transient]
    pub last_count: Hidden<u32>,
}

#[methods]
impl Mailbox {
    /// The app's own code sets it; its argument stays out of the log.
    #[redact]
    pub fn set_password(&mut self, pw: String) {
        self.can_sync = !pw.is_empty();
        *self.imap_password = pw;
    }
}

# fn main() {
use aio::CellData;
let m = Mailbox {
    subjects: vec!["hi".into()],
    imap_password: Hidden::new("pw".into()),
    can_sync: true,
    syncing: true,
    last_count: Hidden::new(3),
};
let view = m.project(&Caller::default());
assert!(view.get("imap_password").is_none() && view.get("last_count").is_none());
assert_eq!(view["syncing"], true);
assert_eq!(Mailbox::SERVER, ["imap_password", "last_count"]);
assert_eq!(Mailbox::TRANSIENT, ["syncing", "last_count"]);
# }
```

- `#[server]` fields are left out of every view, whatever `visible`
  returns, and out of problem reports and the debug log (a report may be
  sent to someone). They are saved, so the app has them after a restart.
  On a shared cell each is an `aio::Hidden<T>` (below).
- `#[transient]` fields are never saved and start at their default on
  every boot. They are sent, so a view can show them.
- Neither may carry `#[serde(..)]` (a build error says so).

### Credentials in a shared cell

A `shared` cell without `visible` sends every field but its `#[server]`
ones to every client. A field whose name is an unambiguous credential is
refused at build time, before it can be sent:

```rust,compile_fail
#[aio::cell(shared)]
pub struct Keys {
    pub api_key: String, // error: looks like a credential, and this cell is shared
}
```

The names refused contain `password`, `passphrase` or `mnemonic`, or the
pairs `private key`, `api key`, `secret key`, `access token`, `auth
token` (`apiKey`, `api_key`, `AuthToken` alike). A name with `pub` or
`public` in it, or ending in a word about the value rather than the
value (`api_key_id`, `password_len`, `secret_key_name`... `id`, `len`,
`name`, `count`, `kind`, `version` and the like), is not. The fix is
`#[server]`, or a rename if it truly isn't a credential.

Limits, compared with aio:

- aio also warns on looser names (`secret`, `seed`, `key`...); aio-rs
  checks only the unambiguous ones, and only on a `shared` cell without
  `visible`. Once a cell has `visible`, keeping secrets out of it is up
  to the function (and `#[server]`).
- aio's `publicFields` opt-out has no counterpart: a public field that
  trips the check is renamed (`public_key`, `pub_key`).

## Per-user and per-role views

```rust
use aio::prelude::*;
use aio::CellData;

#[data]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Note {
    pub owner: String,
    pub text: String,
}

/// Admins see every note, others their own; the count stays for all.
#[cell(persist, shared, visible = mine)]
pub struct Notes {
    pub notes: Vec<Note>,
    pub count: usize,
}

fn mine(n: &Notes, who: &Caller) -> Notes {
    if who.role.as_deref() == Some("admin") {
        return n.clone();
    }
    let me = who.user().unwrap_or_default();
    Notes { notes: n.notes.iter().filter(|x| x.owner == me).cloned().collect(), ..n.clone() }
}

#[methods]
impl Notes {
    #[client]
    pub fn add(&mut self, cx: &Cx, text: String) -> Result {
        let By::User(owner) = cx.by().clone() else { reject!("sign in to write notes") };
        self.notes.push(Note { owner, text });
        self.count = self.notes.len();
        Ok(())
    }
}

# fn main() {
let n = Notes {
    notes: vec![Note { owner: "ana".into(), text: "a".into() }, Note { owner: "bo".into(), text: "b".into() }],
    count: 2,
};
let ana = Caller { by: By::User("ana".into()), role: None };
let admin = Caller { by: By::User("cy".into()), role: Some("admin".into()) };
let key_only = Caller::default(); // a key holder who names no user
assert_eq!(n.project(&ana)["notes"].as_array().unwrap().len(), 1);
assert_eq!(n.project(&admin)["notes"].as_array().unwrap().len(), 2);
assert_eq!(n.project(&key_only)["notes"], serde_json::json!([]));
assert_eq!(n.project(&key_only)["count"], 2);
# }
```

- `visible = f` names a function, `fn(&Self, &By) -> Self` (by who views)
  or `fn(&Self, &Caller) -> Self` (who, and their role now). It returns a
  whole state of the cell's type, so the compiler checks it: there are no
  field lists to misspell. Leave a field out of the view by giving it its
  default (`Vec::new()`, `..Default::default()`), filter rows with
  `filter`, and blank the whole cell with `Self::default()`.
- `Caller { by, role }`: `by` is `By::User(name)` for a signed-in user or
  the name a key holder gives (a claim: a shared key proves no one in
  particular); `role` is the signed-in account's role, read again on
  every change of the accounts, so a demotion counts at once. A
  key-only viewer is `Caller::default()` (`By::System`, no role): write
  `f` so that this caller sees the least.
- Think "fail closed": `f` should start from nothing and add, not start
  from everything and remove, so a new field stays hidden until you
  decide.
- `f` runs again on every commit, for every viewer, on the server. A
  `visible` that panics sends nothing more to that client; it never falls
  back to a fuller view.
- Each link gets its viewer's view: a synced client, a peer, and the
  local door, so `ax state <app> <cell> --as <user>` shows exactly what
  that user's client gets.

Limits, compared with aio: aio's `include` / `exclude` lists, dot paths
and `"none"` are all written as the function's body; there is no
`cellDefaults` for a default filter across cells.

### Test a view

`CellData::project(&state, &caller)` is the view a caller gets, as JSON:
assert on it, as above, without a server. `aio::testing::TestCells`
`t.as_client(&h, caller)` calls methods as that network caller (its
`#[client]` rules apply); its reads are the raw state.

## What a synced copy holds

A copy (`aio::sync::Remote`, a window in `client` mode, a page) holds the
viewer's view, deserialized into the cell's own type. A `#[server]`
field of a shared cell is an `aio::Hidden<T>`: in the app it reads and
writes as its value (`self.seed.len()`, `*self.seed = 7`); on a copy,
which never has it, any read or write panics, naming the field, as aio
throws when client code reads a field its view hides. `Hidden::get`
(`None` on a copy) is for code that runs on both sides. A `#[server]`
field of a shared cell of another type fails the build.

```rust
use aio::prelude::*;
use aio::sync::{Remote, serve};
use std::time::Duration;

#[cell(shared, visible = by_user)]
pub struct Scores {
    pub mine: Vec<(String, u32)>,
    #[server]
    pub seed: Hidden<u64>,
}

fn by_user(s: &Scores, who: &By) -> Scores {
    let me = match who {
        By::User(u) => u.as_str(),
        _ => "",
    };
    Scores { mine: s.mine.iter().filter(|(u, _)| u == me).cloned().collect(), ..s.clone() }
}

#[methods]
impl Scores {
    #[client]
    pub fn score(&mut self, cx: &Cx, points: u32) -> Result {
        let By::User(me) = cx.by().clone() else { reject!("who are you?") };
        self.mine.push((me, points));
        Ok(())
    }
}

async fn until(f: impl Fn() -> bool) {
    for _ in 0..300 {
        if f() {
            return;
        }
        aio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out");
}

# fn main() {
let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
rt.block_on(async {
    let h = Handle::<Scores>::memory();
    h.store(|s| *s.state().seed); // the app reads every field
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(serve(h.clone(), l, "secret".into()));

    let ana = Remote::<Scores>::connect(&addr, "secret", "ana").await.unwrap();
    let bo = Remote::<Scores>::connect(&addr, "secret", "bo").await.unwrap();
    ana.score(10).unwrap();
    bo.score(7).unwrap();
    until(|| h.state().mine.len() == 2 && ana.pending() == 0 && bo.pending() == 0).await;
    until(|| ana.state().mine == [("ana".to_string(), 10)]).await;
    until(|| bo.state().mine == [("bo".to_string(), 7)]).await;
    assert_eq!(ana.state().seed.get(), None, "a copy never has a #[server] field");
    let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| *ana.state().seed));
    assert!(read.is_err(), "and reading it there fails loudly");
});
# }
```

- Patches follow the view: after each commit the server diffs each
  viewer's new view against the one it last sent and sends only that. A
  change no viewer can see sends them nothing; a client never learns that
  rows it can't see changed.
- Calls are predicted on the copy, so a predicted method runs on the
  view. One that touches a `#[server]` field panics there, and the call
  is refused at once with the field's name: mark it
  `#[client(predict = false)]` so the copy waits for the server instead
  ([sync](sync.md)). A field `visible` leaves out (a row, a column of a
  row) is not there to read: that view is all the copy knows.
- Effects and schedules a predicted call asks for are dropped on the copy:
  the server runs them once.

As in aio, reading a `#[server]` field on a copy throws (panics); publish a
fact field next to a secret (`can_sync`) and read that. In return,
aio-rs needs no rule against `visible` on a synced cell (aio refuses
`sync` with a filter): copies sync by replaying calls on the server, and
each client is sent only its own view.

## Cost

Each commit runs `visible` once per viewer, then a diff of their view, so
the cost grows with viewers times the size of the view. Keep views small
(rows, not whole tables; summaries, not raw events), and keep fast-moving
state in a cell of its own. `ax cost <app> [cell]` shows what each cell
sent its clients (pushes, bytes a second, full resends) and its top keys
by bytes.

## Saving and showing are separate

| | Saved | Sent |
|---|---|---|
| a plain field | yes | yes (as `visible` allows) |
| `#[server]` | yes | never |
| `#[transient]` | never | yes |
| `#[server]` `#[transient]` | never | never |
| a cell without `persist` | in a web page, never; in a native app, yes: the store saves every cell it opens | yes, if `shared` |
| a cell without `shared` | as any cell | its state never on the network (served, only its HTTP routes answer); this user's tools and peers reach it through the local door |

A cell's state goes on the network only if it says `shared`: serving
another (`aio::sync::serve`, `TestServer`) answers its HTTP routes alone,
with no `/ws` sync, so a cell for the app's own bookkeeping needs no
`visible` for remote clients.
`#[cell(save = f)]` reshapes what is saved, and `restore = f` repairs it
on boot ([persistence](../persistence/README.md)).

## Contexts: where a cell's code runs

One `#[cell]` runs in several places. In each, the state it sees and what
happens to its effects differ:

| Context | What runs | State it sees | Effects, schedules |
|---|---|---|---|
| the app (or its server) | every method, effects, schedules, routes, server functions | the whole state | run, once |
| a synced copy (`Remote`, `client` mode, a page) | `#[client]` methods, predicted | the viewer's view | dropped; the server runs them |
| replay (boot, `ax replay`, time travel) | the logged calls again | the whole state | dropped: their results are in the log |
| `TestCells`, `Handle::memory()` | every method, in-process | the whole state | held until `t.settle()` or `t.advance(ms)` |
| `t.as_client(&h, who)` | calls as network caller `who` | the whole state | as in the app |
| the local door (`ax call`, `ax state`) | debug: any method; release: `#[client]` ones | the door's view | as in the app |
| a peer (`aio::peer::Peer`) | `#[client]` methods | the peer's view | as in the app |

Rules that follow, and what refuses them:

- A method is the same code everywhere, so it does no I/O and reads time,
  ids and randomness only through `cx` ([methods do no
  I/O](cells.md#methods-do-no-io)): then a prediction, a replay and the
  server's run agree. `ax lint` names I/O in a method.
- `cx.by()` and `cx.role()` are saved with the event, so replay sees the
  same caller.
- Refused at build time: `#[every]` / `#[cron]` on a method with
  arguments; `#[ttl]` or `#[long]` on a method that isn't `async`;
  `#[client]`, `#[redact]` or `#[coalesce]` on one that isn't a `pub fn
  (&mut self, ..)`; `.await` anywhere but a top-level `let x: T =
  io.await;`; a credential in a shared cell without `visible`; a
  `visible` function of the wrong shape; an unknown cell flag.
- Refused at boot: a cell id twice, a dependency that doesn't exist, or a
  cycle (`Cells::start`); serving a cell that isn't `shared`.

## Common mistakes

- Starting `visible` from `s.clone()` and blanking fields one by one: a
  field added later is sent to everyone. Build the view up instead.
- Trusting `visible` to keep a secret a `#[client]` method returns.
- Reading a `#[server]` field in client code: it panics there (a copy
  never has it); publish the fact as its own field.
- Expecting `Caller::default()` (a key with no user name) to see what a
  user sees: write `visible` for it on purpose.
- A big `visible` on a cell with many viewers, called many times a
  second: split the cell.

## API

`cargo doc -p aio-rs --no-deps --open`: `aio::Caller`, `aio::By`,
`aio::CellData` (`project`, `SERVER`, `TRANSIENT`, `save`, `restore`),
`aio::cell` (the `visible`, `save`, `restore` flags), `aio::sync::Remote`,
`aio::sync::serve`, `aio::testing::TestCells::as_client`, `aio::Cx`
(`by`, `role`).

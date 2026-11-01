# Offline and merging

A synced copy keeps working while its server is out of reach: reads and
calls stay local, and the calls go to the server when the link comes
back. This page is how that works, how two people's offline edits come
together, and what travels on the wire. Setting up a server and its
copies is in [sync](../state/sync.md).

| Topic | Where |
|---|---|
| a copy that starts and works offline | [Working offline](#working-offline) |
| what a waiting call means, and its limits | [Calls that wait](#calls-that-wait) |
| two offline edits of the same data | [Merging edits](#merging-edits) |
| what a change costs on the wire | [What travels: patches](#what-travels-patches) |
| how a link is kept healthy | [The link](#the-link) |

## The model in one paragraph

The server is the truth. Each copy (a window in `client` mode, a page,
a tool) holds the server's view of one cell, and every call it makes is
**predicted**: run at once on the copy, then sent with its id and the
ids and random numbers it used (`cx.id()`, `cx.rand()`), so the server's
event is the same call with the same outcome. What a copy shows is the
confirmed view plus its calls still waiting. The server answers each
call; a refusal drops it and the view rolls back. There is no merge
step and no CRDT: the server replays each call, in the order it gets
them, on the state it really has.

## Working offline

```rust
use aio::prelude::*;
use aio::sync::{Link, Remote, serve};
use aio::Conn;
use std::collections::BTreeMap;
use std::time::Duration;

#[cell(persist, shared)]
pub struct List {
    pub items: BTreeMap<Uuid, String>,
}

#[methods]
impl List {
    #[client]
    pub fn add(&mut self, cx: &Cx, text: String) -> Result<Uuid> {
        ensure!(!text.trim().is_empty(), "an empty item");
        let id = cx.id();
        self.items.insert(id, text);
        Ok(id)
    }
}

async fn until(f: impl Fn() -> bool) {
    for _ in 0..1000 {
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
    let dir = tempfile::tempdir().unwrap();
    let copy = dir.path().join("copy.json");
    let h = Handle::<List>::memory();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let server = tokio::spawn(serve(h.clone(), l, "secret".into()));
    let link = Link { addr: addr.clone(), token: "secret".into(), root: None };

    // Online once: the copy is saved beside the app.
    let first = Remote::<List>::connect_saved(link.clone(), "ana", &copy).await.unwrap();
    first.add("milk".into()).unwrap();
    until(|| first.pending() == 0).await;
    // The copy is written in the background; wait for the confirmed one.
    let saved = || std::fs::read_to_string(&copy).unwrap_or_default();
    until(|| saved().contains("milk") && saved().contains(r#""pending":[]"#)).await;
    drop(first);

    // The server goes away; the app starts anyway, from the saved copy.
    server.abort();
    let ana = Remote::<List>::connect_saved(link, "ana", &copy).await.unwrap();
    assert!(!ana.online());
    assert_eq!(ana.status().conn, Conn::Offline);
    assert_eq!(ana.state().items.len(), 1, "what it had");

    let id = ana.add("eggs".into()).unwrap(); // shown at once, waits for the server
    assert_eq!(ana.state().items[&id], "eggs");
    assert_eq!((ana.pending(), ana.confirmed().items.len()), (1, 1));

    // The server is back on the same address: the call goes, once.
    let l = loop {
        match tokio::net::TcpListener::bind(&addr).await {
            Ok(l) => break l,
            Err(_) => aio::time::sleep(Duration::from_millis(20)).await,
        }
    };
    tokio::spawn(serve(h.clone(), l, "secret".into()));
    until(|| ana.pending() == 0).await;
    assert_eq!(h.state().items[&id], "eggs", "the server has it, with the copy's id");
    assert_eq!(*ana.state(), *h.state());
});
# }
```

- `Remote::connect_saved(link, user, file)` keeps the copy in `file`
  (mode 0600): the server's last view and the calls still waiting. With
  the server unreachable it starts from that file, and the waiting calls
  survive a restart. An app in `client` mode does this on its own, in
  `data/copy.json` ([files](files.md#whats-in-data)). A saved copy of
  another cell or another version is ignored.
- `Remote::connect(addr, token, user)` keeps nothing on disk: it needs
  the server to start, then works offline the same way while it runs.
- A refused link (a wrong key: `PermissionDenied`) is not "offline": the
  copy stops and says why instead of retrying.
- A page built with `ax build --web` keeps the same thing in the
  browser's storage, so calls made offline survive a reload. Two tabs of
  one app share it: neither drops the other's waiting calls, and a call
  sent twice is applied once.

### What the UI reads

| Read | Says |
|---|---|
| `remote.state()` | confirmed view + calls still waiting (what to draw) |
| `remote.confirmed()` | what the server has said, nothing predicted |
| `remote.pending()`, `pending_of("add")` | calls waiting, all or of one method |
| `remote.online()` | the link is up now |
| `remote.status()` | `aio::Status`: `online`, `pending`, `degraded` (over 80% of the queue), `conn`, `last_sync`, `rtt` |
| `status().conn` | `Conn::Online`, `Offline`, `Syncing` (linked, calls on their way), `Blocked` (the queue is full) |
| `on_sync(f)` | each catch-up: `Synced { events, elapsed, status, conflicts }` |
| `on_refused(f)` | `f(method, why)` for each call the server refused or the queue dropped |
| `on_conflict(f)` | where a server change met a waiting call ([conflicts](#conflicts-and-fields-shown-merged)) |
| `merge(field, how)` | show `field` merged while a call waits |

`aio::Source` (what views read) passes the same `status()` through, so
an "offline" badge or a "3 changes not sent" line reads one value.

## Calls that wait

| | aio-rs |
|---|---|
| queue size | `aio::sync::PENDING_MAX`, 500 calls |
| kept across a restart | yes, with `connect_saved` (a window in `client` mode always) |
| full queue | new calls are refused (`Conn::Blocked`), or with `[client] retention = "7d"` the oldest call waiting longer than that is dropped to make room, told to `on_refused` |
| a call bigger than the server's `max_message` | refused at once, never queued |
| oldest call the server takes | 30 days old; an older one is refused as stale |
| a client clock ahead of the server | up to 5 minutes is accepted |

- **Each call applies once.** The server keeps every call's id with its
  event (the store's `ops` table). A call sent again (the link dropped
  before the answer came, a restart re-sends the queue) is answered and
  not applied a second time. The ids are kept at least as long as a
  re-send could still be accepted.
- **Order.** A copy's calls reach the server in the order they were
  made. Calls from different copies interleave in the order the server
  gets them.
- **Waiting for the answer.** A normal call returns the predicted result
  at once. `remote.call_confirmed(method, args)` waits for the server's
  answer instead (15 s by default, `[client.timeouts]` per method, `"off"`
  for no limit). Past its wait it returns an error, but the call stays
  queued and still goes when the link is back. `#[client(predict = false)]` on a method makes it never shown
  before the server ran it: it returns nothing and the view changes when
  the server's answer comes.
- **Server functions don't wait.** A server fn call
  ([sync](../state/sync.md#server-functions-and-routes)) fails at once
  offline, and a link that drops fails the ones running: they are not
  idempotent calls on a cell, so they are never queued or re-sent.
- **A refused call.** When the server refuses a waiting call (an
  `ensure!` that no longer holds on the real state, a role that changed),
  the call leaves the queue, the view shows the server's state, and
  `on_refused(method, why)` says what was lost. Show it: the person made
  that change and it did not happen.

```toml
[client]
server = "notes.example.org:8100"
retention = "7d"          # a full queue drops calls older than this (default: never)

[client.timeouts]
"notes.publish" = "60s"   # call_confirmed's wait for this method
"notes.sync" = "off"
```

## Merging edits

Two copies edit the same data offline, then both reconnect. The server
runs one copy's calls, then the other's, each on the state it has then.
So the merge is whatever the methods say, which is usually right without
any work:

| aio's strategy | aio-rs: write the method so |
|---|---|
| `counter` | `self.n += by`: both increments count |
| `set-add`, `set-remove` | `insert(id)`, `remove(&id)` on a set or map: both sides' changes stay |
| `lww` (last write wins) | `self.title = title`: the later call wins |
| `lww-per-key` | a map keyed by id, one key per call: edits to different keys both stay |
| `text` | `aio::merge::text3` in the method (below) |
| a rule the result must keep | `ensure!` in the method: it decides on the real state, and refuses if the other edit broke it |

Write the person's intent as the method's arguments, not the result they
saw. `toggle(id)` from two copies flips twice; `set_done(id, true)` from
two copies ends done. `add(text)` with an id made in the method
(`cx.id()`) never collides with another copy's add.

### Prose: three-way merge

A text two people edit is the one value a method can't merge by itself.
Send the text the edit started from with the new one, and merge in the
method with `aio::merge::text3(base, mine, theirs)`:

```rust
use aio::merge::text3;
use aio::prelude::*;

#[cell(persist, shared)]
pub struct Pad {
    pub text: String,
}

#[methods]
impl Pad {
    /// `base`: the text this edit started from. Returns whether two edits
    /// touched the same part (this one is kept there).
    #[client]
    pub fn edit(&mut self, base: String, new: String) -> Result<bool> {
        let m = text3(&base, &new, &self.text);
        self.text = m.text;
        Ok(m.conflict)
    }
}

# fn main() {
let h = Handle::<Pad>::memory();
h.edit(String::new(), "milk\neggs\nbread\n".into()).unwrap();
let base = h.state().text.clone();

// Two offline edits of the same text, run by the server one after the other.
assert!(!h.edit(base.clone(), "oat milk\neggs\nbread\n".into()).unwrap());
assert!(!h.edit(base.clone(), "milk\neggs\nbread\ntea\n".into()).unwrap());
assert_eq!(h.state().text, "oat milk\neggs\nbread\ntea\n", "both edits survive");

// The same line changed twice: the later edit wins there, and says so.
let base = h.state().text.clone();
h.edit(base.clone(), "soy milk\neggs\nbread\ntea\n".into()).unwrap();
assert!(h.edit(base, "rice milk\neggs\nbread\ntea\n".into()).unwrap());
assert!(h.state().text.starts_with("rice milk\n"));

// Without lines the unit is a character: a title.
let m = text3("Shoping list", "Shopping list", "Shoping lists");
assert_eq!((m.text.as_str(), m.conflict), ("Shopping lists", false));
# }
```

- It is a diff3: edits to different parts both survive, two edits of the
  same part are a conflict, and there `mine` (the incoming edit) is kept
  and `conflict` is true. It never mixes two edits of one part into a
  garbled line.
- Lines are the unit when the text has lines, characters otherwise.
- A side longer than `aio::merge::MAX_TOKENS` (4000 lines or characters)
  is not diffed: the incoming edit wins, as a conflict.
- The method is pure, so the merge replays the same from the log and on
  every copy's prediction.

### Conflicts and fields shown merged

A copy shows its waiting calls replayed on the server's state. When a
server change (another user's) meets one of them and the two differ,
`remote.on_conflict(f)` hears where (aio's `onConflict`): an
`aio::merge::Conflict { field, local, remote }` per value, live and in a
reconnect's catch-up (`Synced::conflicts` counts those). The copy's own
call landing is never one. The server keeps what its methods make.

#### Shown merged while a call waits

`remote.merge("body", Merge::Text)` (aio's `sync.merge`) shows a
top-level field both sides changed merged, not the call replayed alone:
`Text` (both edits by `text3`; a part both changed shows the copy's, a
conflict), `Counter` (both changes added, never a conflict), `SetAdd(id)`
(either side's additions stay, what the copy took out goes; items the
same by the key `id`, or whole). It is what the copy shows until the
server answers: a method that assigns (`self.body = text`) still
replaces the value there, so write methods that edit.

```rust
use aio::merge::{Conflict, Merge, conflicts, merged};
use serde_json::json;

# fn main() {
let base = json!({ "title": "a", "n": 10 });
let local = json!({ "title": "mine", "n": 15 });
let remote = json!({ "title": "theirs", "n": 18 });
let merges = [("n".to_string(), Merge::Counter)];
assert_eq!(
    conflicts(&base, &remote, &local, &merges),
    [Conflict { field: "title".into(), local: json!("mine"), remote: json!("theirs") }],
    "a counter never conflicts"
);
assert_eq!(merged(&Merge::Counter, &base["n"], &local["n"], &remote["n"]), Some(json!(23)));
# }
```

## What travels: patches

On connect a copy gets its whole view (`state`). After each event that
changes what this viewer may see, it gets a **patch**: the change from
the view it was sent to the new one, a list of `set`, `del`, `insert`
and `append` ops by path. `aio::patch` is that code, pure:

```rust
use aio::patch::{Op, Step, apply, diff, digest};
use serde_json::json;

# fn main() {
let old = json!({ "items": { "a": "milk" }, "list": [1, 2], "n": 1 });
let new = json!({ "items": { "a": "milk", "b": "eggs" }, "list": [1, 2, 3], "n": 1 });
let ops = diff(&old, &new);
// A map: only the new key. A list that grew: only the item put in.
let list = |i| vec![Step::Key("list".into()), Step::Index(i)];
assert!(ops.contains(&Op::Set { path: vec![Step::Key("items".into()), Step::Key("b".into())], value: json!("eggs") }));
assert!(ops.contains(&Op::Insert { path: list(2), value: json!(3) }));
assert_eq!(ops.len(), 2, "`n` didn't change: nothing about it is sent");
// A filter: one `del` per item taken out (from the back).
let rows: Vec<String> = (0..50).map(|i| format!("row {i}")).collect();
let kept: Vec<&String> = rows.iter().filter(|r| r.as_str() != "row 7").collect();
assert_eq!(diff(&json!({ "list": rows }), &json!({ "list": kept })), [Op::Del { path: list(7) }]);

let mut copy = old.clone();
apply(&mut copy, &ops).unwrap();
assert_eq!(copy, new);
assert_eq!(digest(&copy), digest(&new), "both sides compare the digest");

// A long text that grew is sent as its new end only (streamed output).
let a = json!({ "reply": "x".repeat(300) });
let b = json!({ "reply": format!("{}more", "x".repeat(300)) });
assert_eq!(diff(&a, &b), [Op::Append { path: vec![Step::Key("reply".into())], value: "more".into() }]);
# }
```

| A change | Patch |
|---|---|
| a field set | one `set` at its path |
| a key added to or removed from a map (`BTreeMap`, `HashMap`, a struct) | one `set` or `del` for that key |
| an item of a `Vec` changed in place, same length | ops inside that item only |
| a `Vec` that grew or shrank (`push`, `insert`, `remove`, `retain`, a spread) | one `insert` per item put in or `del` per item taken out (aio's Immer `add` and `remove`); items compared from the start with the rest put in or cut off at the end, when that is smaller; the whole `Vec` (one `set`) when every way costs as much as the array (a sort, a rebuilt list) |
| a string of 256+ characters that grew at its end | `append` of the new part (`aio::patch::APPEND_MIN`) |
| nothing this viewer sees | nothing at all |

- Each patch names the event it starts from and the one it makes
  (`prev`, `seq`) and the digest of the view it makes. A copy that missed
  one, or whose patched view doesn't match the digest, asks for the whole
  view again (`resync`) instead of guessing.
- What a viewer sees is the cell without its `#[server]` fields,
  filtered by `visible` ([visibility](../state/visibility.md)). A patch is
  the diff of *that* view, so a change another user can't see sends this
  viewer nothing, and a change of role sends the new view whole.
- One link carries one cell: a copy gets only the cell it follows. A
  server serves every shared cell on one `/ws` (`Http::cell`), each link
  naming its cell in its hello.
- **Pacing** (aio's `syncIntervalMs`, `[server.limits] sync_interval_ms`,
  50 ms): a link gets the first change of a burst at once, then at most
  one patch per window, the changes between folded into one. A person's
  call (a user's or an agent's) skips the window, so typing stays
  instant; schedules, effects and other background changes are paced.
- **Whole view over a big patch** (aio's `fullStateThreshold`,
  `full_state_percent`, 50): a patch over that share of the viewer's view
  is sent as the view (`state`) instead.
- A client that falls behind (a slow reader) is not sent every patch it
  missed: it gets one patch to the newest view. One that stops reading
  for 10 s (`aio::sync::SEND_TIMEOUT`) is dropped and gets the whole view
  when it comes back.
- `ax cost <app>` shows what each cell sent its clients: pushes, bytes a
  second, full resends, and its biggest keys (`--keys`).

### Shaping state for the wire

- **Add and remove, don't rebuild.** A push, a spread with new items or
  a filter sends only those items; a sort or a list rebuilt with every
  item changed is sent whole. A `BTreeMap<Uuid, Item>` sends one key
  whatever moves, and an edit in the middle of a `Vec` sends that item
  only.
- **Split cells by who reads them.** A ticker that changes every second
  and a catalog read once should be two cells: a copy of one gets no
  patches of the other.
- **Refer by id.** An order holding a `user_id` doesn't change when the
  user is renamed; one holding a copy of the user does.
- **One call, one patch.** A method that sets three fields sends one
  patch; three calls send three. Batch work that belongs together into
  one method.

## The link

- Transport: a WebSocket at `/ws` (aio's path), one JSON text message per
  frame, the key in `Authorization: Bearer`. On this machine a tool or
  peer app uses the app's local door instead: the same frames on a
  private socket, no network, no key.
- A ping every 2 s (`aio::sync::PING`); no answer for 5 s
  (`aio::sync::DEAD`) closes the link. `remote.rtt()` is the last round
  trip.
- Reconnects back off from 250 ms, doubling up to 10 s, with jitter so
  many clients don't come back at once. On each reconnect the copy takes
  the whole view and re-sends its waiting calls.
- The server's limits (`[server.limits]`: `clients`, `max_message`,
  `calls_per_sec`, `bytes_per_sec`) reach the copy in its first message;
  a fast caller is read more slowly, never dropped
  ([aio.toml](../reference/aio-toml.md#serverlimits)).
- Without `[budgets]`, a push to one client over 500 KB, more than 30
  pushes a second, or over 1 MB a second is said in the log as pressure;
  `[budgets] payload` and `broadcast_rate` make them commitments
  ([sql: big data](sql.md#how-big-a-cell-may-be)).

## aio pages on an aio-rs server

With feature `compat`, an aio page whose cells are `sync: true` keeps
working offline against an aio-rs app: its CRDT ops are taken as calls
with an id made from the op's, so a re-sent op is acked again and never
applied twice. Every catch-up it asks for is answered with a snapshot of
its view (what aio itself answers after a compaction), never the ops
since its cursor ([wire](../migration/wire.md)).

## Limits, compared with aio

- **Merging is methods replayed in order on the server**
  ([merging](#merging-edits)), as aio's own CRDT ops go through its
  methods. A copy may show a field merged while its call waits
  ([shown merged](#shown-merged-while-a-call-waits): `Text`, `Counter`,
  `SetAdd`); aio's `lww-per-key` and `set-remove` view merges and its
  hybrid logical clock have no counterpart. Every merge goes through a
  server, as in aio.
- **A full queue refuses.** 500 waiting calls, as aio's `pendingCap`; a
  full queue refuses the new call unless `retention` lets an old one go,
  where aio drops the new op (`prune-failed`). Both keep the queue across
  a reload (aio in the page's storage, aio-rs on disk or in the browser's).
- **No render-staleness backpressure.** aio slows a page whose paint
  lags (its staleness multipliers); aio-rs paces every link
  (`sync_interval_ms`), and a slow reader is sent the newest view once it
  reads again.
- **No path subscriptions.** One link is one cell, so a copy gets every
  patch of that cell, not only the paths it reads.
- The sync wire is JSON; binary payloads go on a side channel
  ([transports](../clients/transports.md#binary-data)).

## Common mistakes

- Calling `connect` in an app that must start offline: without a saved
  copy the first connect needs the server. Use `connect_saved` (a window
  in `client` mode already does).
- A method that reads what the person saw instead of taking it as an
  argument (`toggle` where `set_done(true)` was meant): two offline
  copies then undo each other.
- Making ids or times outside the method and passing different ones on
  the copy and the server: use `cx.id()` and `cx.now()`, which the call
  carries, so the prediction and the event agree.
- Ignoring `on_refused`: a call refused after reconnecting was a change
  the person saw and lost. Tell them.
- Growing a `Vec` by one item many times a second in a shared cell:
  each push sends the whole list. Use a map, or page the rows
  ([sql](sql.md)).

## API

`cargo doc -p aio-rs --no-deps --open`: `aio::sync` (`Remote`,
`connect_saved`, `Link`, `Synced`, `PENDING_MAX`, `CALL_TIMEOUT`, `PING`,
`DEAD`, `SEND_TIMEOUT`), `aio::Status`, `aio::Conn`, `aio::merge`
(`text3`, `Merged`, `MAX_TOKENS`), `aio::patch` (`diff`, `apply`,
`digest`, `Op`, `Step`, `APPEND_MIN`).

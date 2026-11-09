# Problem reports and feedback

When an aio-rs app breaks, it writes down what a maintainer would ask
anyway: which build, what it held, what had just happened, what it
logged. A user can file the same report from a "Report a problem"
button. Reports stay on the machine unless the app says where to send
them.

There is nothing to switch on: `aio::boot` (which every app's launcher
runs) sets reports up from `[feedback]` in `aio.toml`.

```toml
[app]
id = "notes"

[feedback]
url = "https://reports.example.org/notes"   # POST each report here as JSON (https only)
timeout = "15s"                             # a POST slower than this is not delivered
keep = 50                                   # reports kept on disk, newest first
auto = true                                 # report problems by itself; false: only the user's

[capabilities]
network = ["reports.example.org"]           # the url's host, or the POST is refused
```

| Key | Default | Means |
|---|---|---|
| `url` | none | each report, once on disk, is also POSTed here as JSON; `https://` only (`http://127.0.0.1` for tests) |
| `timeout` | `15s` | a collector that hasn't answered 2xx by then has not got it |
| `keep` | 50 | reports kept in `data/reports/`; older ones are deleted |
| `auto` | `true` | write a report by itself when the app breaks; `false`: only the reports users file |

## When one is written

By itself (`auto = true`) for a panic, a tripped cell, a part degraded,
memory high, growing or critical, an effect's timeout, a crashed or
frozen window, a failed update, and a refused schedule, hook or
reaction ([every kind](errors.md#problems-every-kind)). Two rules keep it
from becoming a problem of its own:

- the same kind and message is reported once per run;
- at most 10 automatic reports per run; past that, problems are only
  logged (said once: `10 problem reports this run: further problems are
  logged, not reported`).

A user's report is not capped by these, but by its own budget: 5 a
minute (`aio::feedback::MAX`), which a person never meets and a script
does at once.

## What a report holds

Each is one JSON file, `data/reports/<id>.json`, written atomically and
owner-only (0600). The id sorts by time
(`2026-10-28T09-48-01-123Z-00001-3f9a0c`).

| Field | Holds |
|---|---|
| `id`, `createdAt`, `kind` | which report, when, and its kind (`panic`, `user`, `degraded:sync` …) |
| `tip` | the one thing to do, for kinds that have one |
| `title`, `body`, `contact` | the problem in a line (300 characters), the details (20 000), how to reach the user (200) |
| `app` | `id`, `version`, `channel`, `build` (debug or release), `aio` (aio-rs's version), `os`, `arch`, `pid` |
| `state` | each of the app's cells as it would be saved (`save` applied, `#[transient]` left out) and as a client with the key sees it (`visible`, no `#[server]` fields), at most 256 KB in all |
| `withheld` | cells left out of `state`, and why: `#[redact]` methods, too big, unreadable |
| `timeline` | the newest 100 calls: when, which (`notes.add`), how long (`ms`), a refusal's reason; never the arguments |
| `diagnostics` | the newest 200 problems and warnings off the [diagnostic bus](errors.md#hearing-problems-in-code) |
| `logs` | the last 200 lines of `app.log` |
| `truncated` | what was cut to keep it attachable (`title`, `body`) |

Plain JSON: a maintainer, a script, an issue tracker and an agent read it
with nothing installed.

### What it never holds

- **Credentials.** Every string in it is masked as log lines are:
  `token=` values, `Bearer` tokens and pairing codes become `…`.
- **A cell with `#[redact]` methods.** Its state is withheld whole and
  named in `withheld`, so the gap reads as a decision. The rule is the
  same one `ax events` and the log follow.
- **Arguments.** The timeline names calls, never what was passed: they
  may be anything the user typed.
- **Half a state.** A cell that doesn't fit the 256 KB left is left out
  whole and named: half a state misleads, none does not.

- **What a client may not see.** `#[server]` fields, and what `visible`
  hides from a client with the key (aio's screening). A report taken
  mid-panic runs none of the app's code, so a cell with `visible` is
  withheld from it.

A secret is still in the saved state, on disk. Keep it out of saved
state (`#[transient]`, `save`) or give its cell a `#[redact]` method
([secrets](../auth/secrets.md)).

## Reading them

```sh
ax report notes                 # the reports, newest first: id and title
ax report notes 2026-10-28T09   # one in full (a prefix of its id is enough)
ax report notes --json          # the list as JSON
ax data notes                   # where data/reports/ is
```

They live in the app's data folder, so a backup carries them and an
uninstall removes them.

## Sending them somewhere

aio-rs runs no service of its own to receive reports. With `[feedback]
url` set, each report is POSTed there after it is on disk, never instead:
a collector that is down, refuses, or doesn't answer in `timeout` leaves
the report saved and says it was not delivered. For anything else (an
issue tracker's API, a message queue), set a sink in code after boot
(the launcher sets the `url`'s at start; the last one set is used):

```rust
use std::sync::{Arc, Mutex};
use std::time::Duration;

# fn main() {
let dir = std::env::temp_dir().join(format!("feedback-page-{}", std::process::id()));
aio::report::init(aio::report::About { id: "notes".into(), dir: dir.clone(), ..Default::default() });

let got = Arc::new(Mutex::new(Vec::new()));
let g = got.clone();
// Called after the report is on disk; an Err, or no answer within 5 s, leaves it there only.
aio::report::sink(Duration::from_secs(5), move |report| {
    g.lock().unwrap().push(report["title"].as_str().unwrap_or_default().to_string());
    Ok(())
});

// What a "Report a problem" button's cell does, here by hand:
let path = aio::report::file_from("Sync stopped", "after waking the laptop", Some("ana@example.org")).unwrap();
assert_eq!(aio::report::send(&path), Ok(true), "delivered");
assert_eq!(*got.lock().unwrap(), ["Sync stopped"]);

let (id, report) = aio::report::list(&dir).remove(0);
assert_eq!((report["kind"].as_str(), report["contact"].as_str()), (Some("user"), Some("ana@example.org")));
assert!(path.ends_with(format!("{id}.json")));
std::fs::remove_dir_all(&dir).unwrap();
# }
```

An automatic report is sent from a thread of its own, so a slow sink
holds nothing up. If you send reports off the machine, tell your users:
a report holds the app's state, which is their data.

## A "Report a problem" button

`aio::feedback` is the cell such a button uses (aio's `feedback` cell),
open from boot: `aio::feedback::cell()` is its handle.

| Field | Means |
|---|---|
| `enabled` | reports are set up (`aio::boot` ran) |
| `status` | `Idle`, `Capturing` (being written and sent), `Saved`, `Error` |
| `last` | the newest report this run: `id`, `path`, `created_at`, `delivered` |
| `pending` | reports on disk, the app's own included |
| `error` | the last failure, in words |

| Method | Does |
|---|---|
| `report(title, body, contact)` | write a report (the facts are collected), then send it on; refused for an empty title, while one is being sent, or past 5 a minute |
| `refresh()` | count the reports on disk again |
| `dismiss()` | clear the last report's banner |

All three are `#[client]`: anyone using the app may report, because
refusing them would silence the people who hit something.

```rust,no_run
use aio::feedback::{FeedbackCalls, Status};

# fn main() {
let fb = aio::feedback::cell().expect("aio::boot ran");
fb.report("The list is empty after an update".into(), Some("It had 40 notes".into()), None).unwrap();
while fb.state().status == Status::Capturing {
    std::thread::sleep(std::time::Duration::from_millis(20));
}
match (&fb.state().status, &fb.state().last) {
    (Status::Saved, Some(last)) if last.delivered => println!("thanks: sent as {}", last.id),
    (Status::Saved, Some(last)) => println!("thanks: saved at {}", last.path),
    _ => println!("not saved: {}", fb.state().error.clone().unwrap_or_default()),
}
# }
```

In a view, call it from an event handler and read its state with
`fb.state()` (or `fb.listen(…)` to follow its changes); the
`status` and `last` fields are what a "thanks, saved" line shows.

## aio-rs's own feedback

`ax feedback` is about aio-rs itself, not the app's users: it says where
findings about aio-rs go, and `ax feedback notes --create` makes the
app's own file for them (what an agent working on the app noticed about
the framework).

## Not as in aio

- aio turns reports on with `feedback: true`; aio-rs always has them
  (`auto = false` keeps only the user's).
- A report's state is screened as in aio: as saved (`#[transient]` and
  `save` apply), as a client with the key sees it (`visible`, `#[server]`
  fields out), and any cell with `#[redact]` methods withheld whole. A
  report taken mid-panic runs none of the app's code, so a cell with
  `visible` is withheld from it.
- aio's timeline entries carry their payloads and diffs (screened);
  aio-rs's carry each call's arguments with `[feedback] args = true`
  (a `#[redact]` method's as `[redacted]`, keys masked, 1 KB each), never
  by default: a report may leave the machine. The state's diff is the
  report's state.
- aio's `am report path` prints the folder: `ax data <app>` does.

## API

`cargo doc -p aio-rs --no-deps --all-features --open`: `aio::report`
(`file`, `file_from`, `sink`, `send`, `list`, `problem`, `About`,
`build`), `aio::feedback` (`Feedback`, `Status`, `Saved`, `cell`, `MAX`).

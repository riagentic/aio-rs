# A CLI and a service

A job queue as two small apps: a headless **service** that keeps the
queue and works through it, and a **command-line tool** that adds jobs,
lists them and follows them live, from the same machine or another one.
No window, no browser.

You will use:

- a shared cell served by a headless app (`[app] mode = "server"`);
- a declared schedule that drives the work, and an effect that does it;
- `restore` to recover from a crash in the middle of a job;
- a CLI in `client` mode: a synced copy of the service's cell, with
  calls that wait for the service's answer;
- `aio::cli` for the command line, and `ax` to inspect and run both.

Every Rust block on this page compiles and runs as a test of aio-rs.
Prerequisites: [sync](../state/sync.md) and
[command-line apps](../clients/cli-apps.md).

## The layout

```sh
ax create queue --template service      # the service: its cell, served
ax create queue-cli --template cli      # the tool
```

The tool needs the service's cell type, so its library depends on the
service's: in `queue-cli/Cargo.toml` add
`queue = { path = "../queue" }` and give `aio` the feature `net` (the
sync client). Both apps then share one definition of `Queue`: the
service runs it, the tool holds a synced copy of it.

## 1. The service's cell

```rust
use aio::prelude::*;
use std::time::Duration;

#[data]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Status {
    #[default]
    Waiting,
    Running,
    Done,
    Failed,
}

#[data]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Job {
    pub id: u32,
    pub task: String,
    pub by: By,
    pub status: Status,
    pub said: Option<String>,
}

/// The queue: served to clients, saved on every change.
#[cell(persist, shared, restore = requeue, id = "queue")]
pub struct Queue {
    pub jobs: Vec<Job>,
    pub next: u32,
    pub done: u32,
    pub failed: u32,
}

/// On every boot: a job that was running when the service stopped runs again.
fn requeue(mut q: Queue) -> Queue {
    for j in q.jobs.iter_mut().filter(|j| j.status == Status::Running) {
        j.status = Status::Waiting;
    }
    q
}

/// The work itself (a build, an export): stands in for a real command.
async fn work(task: String) -> std::result::Result<String, String> {
    aio::time::sleep(Duration::from_secs(2)).await;
    match task.contains("fail") {
        true => Err(format!("{task}: exit 1")),
        false => Ok(format!("{task}: ok")),
    }
}

#[methods]
impl Queue {
    /// Queue `task` as the caller; its number.
    #[client]
    pub fn enqueue(&mut self, cx: &Cx, task: String) -> Result<u32> {
        let task = task.trim().to_string();
        ensure!(!task.is_empty(), "a job needs a task");
        ensure!(task.chars().count() <= 200, "a task is 200 characters at most");
        ensure!(self.waiting() < 1000, "the queue is full (1000 waiting)");
        self.next += 1;
        self.jobs.push(Job { id: self.next, task, by: cx.by().clone(), ..Job::default() });
        Ok(self.next)
    }

    /// A failed job, queued again.
    #[client]
    pub fn retry(&mut self, id: u32) -> Result {
        let Some(j) = self.jobs.iter_mut().find(|j| j.id == id) else { reject!("there is no job #{id}") };
        ensure!(j.status == Status::Failed, "job #{id} hasn't failed");
        (j.status, j.said) = (Status::Waiting, None);
        self.failed -= 1;
        Ok(())
    }

    /// Every second: start the next waiting job, one at a time.
    #[every("1s", skip_if_running)]
    pub fn tick(&mut self, cx: &Cx) {
        if self.jobs.iter().any(|j| j.status == Status::Running) {
            return;
        }
        let Some(j) = self.jobs.iter_mut().find(|j| j.status == Status::Waiting) else { return };
        j.status = Status::Running;
        let (id, task) = (j.id, j.task.clone());
        cx.run(
            Effect::new("work", move |h: Handle<Queue>| async move {
                _ = h.finish(id, work(task).await);
            })
            .timeout(Duration::from_secs(60)),
        );
    }

    /// The work's answer. Not `#[client]`: only the service's own effect calls it.
    pub fn finish(&mut self, id: u32, out: std::result::Result<String, String>) -> Result {
        let Some(j) = self.jobs.iter_mut().find(|j| j.id == id && j.status == Status::Running) else {
            reject!("job #{id} isn't running")
        };
        match out {
            Ok(said) => (j.status, j.said, self.done) = (Status::Done, Some(said), self.done + 1),
            Err(why) => (j.status, j.said, self.failed) = (Status::Failed, Some(why), self.failed + 1),
        }
        Ok(())
    }

    /// Forget finished jobs. Not `#[client]`: the operator's (`ax call`).
    pub fn clear_finished(&mut self) {
        self.jobs.retain(|j| matches!(j.status, Status::Waiting | Status::Running));
    }

    #[derived]
    pub fn waiting(&self) -> usize {
        self.jobs.iter().filter(|j| j.status == Status::Waiting).count()
    }
}

# fn main() {
use aio::testing::TestCells;
let mut t = TestCells::new(); // the cell on a virtual clock
let q = t.cell::<Queue>();
let one = q.enqueue("build docs".into()).unwrap();
q.enqueue("deploy, then fail".into()).unwrap();
assert_eq!((one, q.state().waiting()), (1, 2));
assert!(q.enqueue("  ".into()).is_err());

t.advance(1_000); // the first tick starts job 1; its 2 s of work run at once
assert_eq!(q.state().jobs[0].status, Status::Done);
assert_eq!(q.state().jobs[1].status, Status::Waiting, "one job at a time");
t.advance(1_000);
assert_eq!((q.state().done, q.state().failed), (1, 1));
assert_eq!(q.state().jobs[1].said.as_deref(), Some("deploy, then fail: exit 1"));

// A client may queue and retry, never report a job's outcome itself.
let ana = t.as_client(&q, Caller { by: By::User("ana".into()), role: None });
ana.retry(2).unwrap();
assert!(ana.finish(2, Ok("forged".into())).is_err(), "not a #[client] method");
assert!(ana.retry(1).is_err(), "job #1 didn't fail");

// After a crash halfway through a job, the boot puts it back in line.
let crashed = Queue { jobs: vec![Job { id: 7, status: Status::Running, ..Job::default() }], ..Queue::default() };
assert_eq!(requeue(crashed).jobs[0].status, Status::Waiting);
# }
```

- **The schedule drives, the effect works.** `tick` runs every second in
  the service (declared schedules are armed whenever the cell is live),
  marks one job `Running` and asks for the effect. `skip_if_running`
  skips a tick while the last tick's effect still runs.
- **The outcome is a call.** `finish` is an event in the log, with the
  work's answer as its argument: `ax events queue` shows every job's
  life, and a replay rebuilds the queue without running any work again.
- **A crash halfway** leaves a job `Running` in the saved state.
  `restore = requeue` runs on every boot (and every rewind) and puts it
  back in line. Work that must not run twice needs its own check
  (an idempotency key the work's target honours).
- **Who may do what**: `enqueue` and `retry` are `#[client]`, so the
  network may call them; `finish` and `clear_finished` are not, so only
  the service's own code and the operator's `ax call` may.

## 2. The service's config and launcher

```toml
# queue/aio.toml
[app]
id = "queue"
name = "queue"
mode = "server"            # headless: the cell is served, no window

[server]
listen = "127.0.0.1:7420"  # --port, --host and --expose change it at launch
tls = true                 # needed beyond this machine, and for pairing
pairing = true             # `ax pin` + `ax pair` hand the key to the tool

[history]
keep = "90d"

[capabilities]
network = []
```

The launcher is the service template's with `app::Board` changed to
`app::Queue`. Its frame:

```rust,ignore
// ax create's service launcher (app/src/main.rs), compiled by ax's own tests:
let cfg = aio::config!();
let Start::Run(dirs) = aio_shell::start(&cfg, aio::app_version!())? else {
    return Ok(std::process::ExitCode::SUCCESS);
};
aio_shell::serve(&dirs)?;
Ok(aio::lifecycle::exit_code())
```

Between `start` and `serve` it opens the queue with
`aio::Source::<app::Queue>::for_server(&cfg, &dirs)?`: the store, served
on `[server] listen`, its schedules armed. `aio_shell::serve` then waits
until the service is asked to stop (Ctrl-C, SIGTERM, `ax stop`), lets
running effects finish and closes the store.

## 3. The tool

The tool's library parses its command line with `aio::cli` and works on
any copy of the queue: a synced copy of the running service in the app,
the served cell itself in a test.

```rust
# use aio::prelude::*;
# #[data]
# #[derive(Clone, Copy, Debug, Default, PartialEq)]
# pub enum Status { #[default] Waiting, Running, Done, Failed }
# #[data]
# #[derive(Clone, Debug, Default, PartialEq)]
# pub struct Job { pub id: u32, pub task: String, pub by: By, pub status: Status, pub said: Option<String> }
# #[cell(persist, shared, id = "queue")]
# pub struct Queue { pub jobs: Vec<Job>, pub next: u32, pub done: u32, pub failed: u32 }
# #[methods]
# impl Queue {
#     #[client]
#     pub fn enqueue(&mut self, cx: &Cx, task: String) -> Result<u32> {
#         let task = task.trim().to_string();
#         ensure!(!task.is_empty(), "a job needs a task");
#         self.next += 1;
#         self.jobs.push(Job { id: self.next, task, by: cx.by().clone(), ..Job::default() });
#         Ok(self.next)
#     }
#     #[client]
#     pub fn retry(&mut self, id: u32) -> Result {
#         let Some(j) = self.jobs.iter_mut().find(|j| j.id == id) else { reject!("there is no job #{id}") };
#         ensure!(j.status == Status::Failed, "job #{id} hasn't failed");
#         j.status = Status::Waiting;
#         Ok(())
#     }
#     pub fn clear_finished(&mut self) {}
# }
use aio::cli::{Column, Flag, Spec, Stop, Style, parse, table};
use aio::{Door, Source};
use serde_json::{Value, json};

pub fn spec() -> Spec {
    Spec::new("queue-cli")
        .about("the job queue, from a terminal")
        .version("0.1.0")
        .command("add", "queue a job: add <task…>")
        .command("list", "the jobs, newest last")
        .command("retry", "queue a failed job again: retry <id>")
        .command("watch", "follow the queue live (Ctrl-C ends)")
        .rest("words")
        .flag(Flag::switch("json").help("answer as JSON"))
        .flag(Flag::number("last").default("20").help("list at most this many"))
}

/// A call that waits for the service's answer: a tool prints the truth,
/// never a prediction, and exits only once the service has the call.
fn confirmed(q: &Source<Queue>, method: &str, args: Value) -> Result<Value, (u8, String)> {
    let out = match q {
        Source::Remote(r) => r.call_confirmed(method, args),
        Source::Local(h) => h.call(method, args),
    };
    out.map_err(|e| (1, e.why))
}

/// The jobs as a table, or JSON for a script.
pub fn list(q: &Queue, last: usize, as_json: bool) -> String {
    let jobs = &q.jobs[q.jobs.len().saturating_sub(last)..];
    if as_json {
        return json!(jobs).to_string();
    }
    let rows: Vec<Vec<String>> = jobs
        .iter()
        .map(|j| vec![j.id.to_string(), format!("{:?}", j.status).to_lowercase(), j.task.clone()])
        .collect();
    let cols = [Column::right("id"), Column::left("status"), Column::left("task")];
    table(&cols, &rows, Style::stdout())
}

/// One command: what to print, or (exit code, why): 1 refused, 2 bad usage.
pub fn run(q: &Source<Queue>, argv: &[String]) -> Result<String, (u8, String)> {
    let a = match parse(&spec(), argv) {
        Ok(a) => a,
        Err(Stop::Say(text)) => return Ok(text), // --help, --version
        Err(Stop::Refused { why, .. }) => return Err((2, why)),
    };
    match a.command.as_deref() {
        Some("add") => {
            let id = confirmed(q, "enqueue", json!([a.rest.join(" ")]))?;
            Ok(if a.json { json!({ "id": id }).to_string() } else { format!("queued #{id}") })
        }
        Some("retry") => {
            let id: u32 = a.rest.first().and_then(|w| w.parse().ok()).ok_or((2, "retry <id>: a job's number".into()))?;
            confirmed(q, "retry", json!([id]))?;
            Ok(format!("#{id} waits again"))
        }
        Some("watch") => Err((2, "watch runs in the launcher (it never returns)".into())),
        _ => Ok(list(&q.state(), a.number("last").unwrap_or(20.0) as usize, a.json)),
    }
}

# fn main() {
use aio::testing::TestServer;
let words = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
// The real service on loopback, and the tool's synced copy of it.
let server = rt.block_on(TestServer::start(Handle::<Queue>::memory(), Default::default())).unwrap();
let q = Source::Remote(rt.block_on(server.client("ops")).unwrap());

assert_eq!(run(&q, &words("add build docs")), Ok("queued #1".into()));
assert_eq!(run(&q, &words("add --json nightly report")), Ok(r#"{"id":2}"#.into()));
assert_eq!(server.state().jobs[1].by, By::User("ops".into()), "the service knows who queued it");
assert_eq!(run(&q, &words("add  ")), Err((1, "a job needs a task".into())), "refused by the service");
assert_eq!(run(&q, &words("retry 1")), Err((1, "job #1 hasn't failed".into())));
assert_eq!(run(&q, &words("retry x")).unwrap_err().0, 2);
assert!(run(&q, &words("ad x")).unwrap_err().1.contains("add"), "did you mean");

let out = run(&q, &words("list --json --last 1")).unwrap();
let jobs: Vec<Job> = serde_json::from_str(&out).unwrap();
assert_eq!(jobs[0].task, "nightly report");
assert!(run(&q, &words("list")).unwrap().contains("build docs"));

// What a client may not call, the service refuses, whoever asks.
let Source::Remote(r) = &q else { unreachable!() };
assert!(r.call_confirmed("clear_finished", json!([])).unwrap_err().why.contains("not a #[client] method"));
# }
```

- `aio::cli::parse` refuses a wrong invocation with a reason (exit 2) and
  answers `--help` and `--version`; a switch named `json` turns the
  answers into JSON for scripts.
- `call_confirmed` blocks until the service answers (15 s by default,
  `[client] timeouts` per method). A plain call (`q.enqueue(..)`) would
  be predicted and queued, which is right for a window and wrong for a
  tool that exits at once: the call would wait in the copy's file for the
  next run.
- The tool reads the state it was sent (`q.state()`): the service's view
  for this caller, current as of the last patch.

The launcher is the cli template's, with the queue's source in place of
the notes cell:

```rust,ignore
// ax create's launcher (app/src/main.rs), compiled by ax's own tests:
let cfg = aio::config!();
let args: Vec<String> = std::env::args().skip(1).collect();
```

It opens `aio::Source::<queue::Queue>::for_app(&cfg, &dirs)?`, which in
`client` mode connects to the service and keeps a copy (with the service
down, it starts from the copy's file). For `watch`, before `run`, it
draws the queue on every change and never returns:
`aio::cli::watch(&*q.state(), |f| q.listen(f), |s| app::list(s, 10, false))`
followed by `loop { std::thread::park() }`.

```toml
# queue-cli/aio.toml
[app]
id = "queue-cli"
name = "queue-cli"
mode = "client"            # a synced copy of [client] server's cell

[client]
server = "127.0.0.1:7420"  # where the service listens (ax pair saves its own link)

[capabilities]
network = []               # this machine is always allowed; add the service's host when it is elsewhere
```

## 4. Run both

```sh
ax start queue                              # the service, in the background
ax pin queue                                # an 8-digit code, valid 3 minutes
ax pair queue-cli 127.0.0.1:7420 12345678   # the tool gets the key and the server's root
queue-cli add build docs                    # queued #1
queue-cli add --json nightly report         # {"id":2}
queue-cli list
queue-cli watch                             # redrawn on every change; Ctrl-C ends
```

`ax dev queue` runs the service in the foreground instead, with its log
in the terminal. A tool on another machine pairs the same way with the
service's LAN address (`ax discover` lists the apps that answer, with
`[server] discover = true`), and its host goes in the tool's
`[capabilities] network`.

## 5. Inspect and operate

`ax` reaches the running service through its local door (this user
only, no key needed):

```sh
ax state queue queue jobs                 # the jobs array
ax state queue queue --watch              # the state, again on every change
ax call queue queue enqueue '"run migrations"'
ax call queue queue clear_finished        # the operator's, not a client's
ax events queue --last 20 --diff          # each call, with what it changed
ax schedules queue                        # @tick, and when it fires next
ax clients queue                          # the tools and copies linked now
ax peer queue --as ops                    # what a client called ops may call
ax health queue                           # exit 0 when healthy
ax logs queue --follow                    # the log as it is written
ax stop queue
```

`ax call` in a debug build reaches any method; a release build's local
door gets what a client gets, so `clear_finished` there needs a client
method with a rule, or the operator's own tooling.

## 6. Ship and run at boot

```sh
ax build queue --targets cli       # the service: a headless binary and its supervisor file
ax build queue-cli --targets cli   # the tool: a binary for PATH
```

Both are `cli` targets (or `[build] targets = ["cli"]` in their
`aio.toml`): a service built as `desktop` gets a window bundle and no
supervisor. Beside the service's binary comes its supervisor file, with
the line that installs it: a systemd user unit (Linux; `loginctl
enable-linger` runs it without a login), a launchd agent (macOS) or a
Task Scheduler script (Windows). Each starts the service again 5 s after
it exits, except after a stop: `ax stop` keeps it down
([targets](../build/targets.md#service)). The service updates itself
from signed releases like any app ([release](../build/release.md)): it
exits for the supervisor to start the new version.

## What maps to what

| aio's task queue | Here |
|---|---|
| `client: "server-only"` | `[app] mode = "server"`, `aio_shell::serve` |
| `schedules: [{ every: 1000, action: queue.process.action() }]` | `#[every("1s", skip_if_running)]` on `tick` |
| `connectCli(url)` + `cli.bind(queue)` | `client` mode: `Source::for_app` gives a typed synced copy |
| `cli.send({ type: "queue:enqueue", .. })` | `call_confirmed("enqueue", ..)`, or `q.enqueue(..)` typed |
| `instances("task-queue")` to find the port | `ax pair` saves the link; `ax` finds a local app by its id |
| `am state`, `am dispatch`, `am schedules` | `ax state`, `ax call`, `ax schedules` |
| `--compile --service --remote` | `ax build --targets cli`; `[server] tls = true` and `listen` on the LAN address |

## Limits

- More shared cells of the service are served beside the first on the
  same port and `/ws` (`Http::cell(handle)`); each copy opens its own
  socket, where aio's page carries every cell on one.
- `aio::cli::watch` draws the whole view again on each change; there is
  no diffing screen library (aio has none either).

## Common mistakes

- A plain predicted call in a tool that exits right away: it may not
  reach the service before the process ends. Use `call_confirmed`.
- Doing the work in `tick` itself: a method does no I/O. Mark the job,
  ask for an effect, and let the effect call back.
- Trusting a job's `by` without accounts: anyone with the key can say any
  name. Turn `[accounts]` on when it matters ([accounts](../auth/README.md)).
- Serving on a LAN address without `tls`: the key crosses the network in
  the clear.
- Forgetting `restore`: a job that was running at a crash stays
  `Running`, and `tick` never starts another.

## API

`cargo doc -p aio-rs --no-deps --all-features --open`: `aio::cli`
(`Spec`, `Flag`, `parse`, `table`, `watch`), `aio::sync::Remote`
(`call_confirmed`, `listen`), `aio::Source`, `aio::Effect`,
`aio::testing::TestCells`, `aio::testing::TestServer`.

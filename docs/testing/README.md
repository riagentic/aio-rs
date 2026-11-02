# Testing

Tests run in-process against the real cell: the same store, methods,
invariants, effects and views the app runs. `ax test <app> [filter]` runs
an app's tests (`cargo test` works too). `aio::testing` is aio-rs's
`testing` feature, which the tests get as a dev-dependency, so the app's
own builds never compile the test kit; `ax create` writes the line, and
makes an app whose template already has a cell test and a UI test:

```toml
[dev-dependencies]
aio = { package = "aio-rs", git = "https://github.com/riagentic/aio-rs", tag = "v0.1.0", features = ["testing"] }
```

Tests that use `aio::testing` without it don't build; `ax lint` names the
first such line and the fix.

| Page | Covers |
|---|---|
| [Testing cells](cells.md) | `Handle::memory`, `TestCells`: refusals, seeds, effects, virtual time, schedules, boot, migrations, access rules, fuzzing |
| [Testing views](ui.md) | `TestUi`: the real views in a headless page, names, clicks, keys, waits, shots, video, canvas |
| [Cassettes](cassettes.md) | an effect's I/O recorded once and replayed |
| [Servers, sync and many apps](multi-app.md) | `TestServer`, `Sim`: routes, sign-in, synced copies, cuts, several apps |
| [Lint and proving the tests](lint.md) | `ax lint`, and tests that can go red |
| [Proving the tests](proving.md) | the mutation ledger: breaks planted in the source and in a built `.deb`, each caught by a named test |
| [Real machines](labs.md) | `ax build --smoke`, `ax lab`: the built app on Windows, macOS, Linux |

Which harness:

- a method's logic, a refusal, an invariant: a `Handle::<C>::memory()`;
- effects, schedules, several cells, access rules, migrations:
  `TestCells` (a virtual clock: a 10-minute schedule runs at once);
- what a person sees and clicks: `TestUi`;
- the wire, HTTP routes, server functions, sync under bad networks:
  `TestServer` and `Sim`.

| aio | aio-rs | For |
|---|---|---|
| `testCell` | `Handle::<C>::memory()` | one cell's methods |
| `bootCells` + `advance` | `aio::testing::TestCells` | cells, effects, schedules on a virtual clock |
| `t.as(user)` | `TestCells::as_client(&h, caller)` | access rules as a network caller meets them |
| `t.fuzz` | `TestCells::fuzz`, `fuzz_methods` | seeded random calls against invariants |
| `t.init`, `expect.effects`, `perfBudget` | `TestCells::reset`, `last_effects`, `budget` | a cell reset mid-test, the last call's effects, a per-test budget |
| `testIO` | `aio::testing::TestIo` | a command-line run: stdin, a terminal or not, colour, what was written |
| `testApps` | `aio::testing::TestApps` | several apps, each its own dirs and server |
| `smoke()` | `aio::testing::smoke(bundle)` | the built bundle started and stopped, in a test |
| `testUI` | `aio::testing::TestUi` | views in the real page, headless |
| `testServer` | `aio::testing::TestServer` | real HTTP and WebSocket on loopback |
| `testMultiClient` | `aio::testing::Sim` | a server and synced copies under seeded cuts |
| `cassette` | `aio::cassette::Cassette` | an effect's I/O recorded once, replayed after |
| `testBrowser` | `aio::testing::TestBrowser` | a headless Chromium a test owns (the web build) |
| `aiol` | `ax lint` | what types can't check: impure methods, credentials, hooks, a11y |

## Running tests

```sh
ax test notes                      # every test of the app (cargo test on each of its packages)
ax test notes adds                 # the tests whose name contains "adds"
ax test notes --video videos/      # and a video of each UI test
cargo test -p notes --lib          # plain cargo works too
```

- `ax test <app> [filter]` runs `cargo test` on every package of the app
  (its library holds the tests, its launcher the binary) and fails when
  no test ran: a green run tested something.
- `--video out` records each UI test without changing it (a folder: one
  video per test; a `.mp4` or `.webm` file: one video), `--pace ms` per
  step on screen (800). See [a video of a test](ui.md#a-video-of-a-test).
- UI tests need the pinned Electron runtime: `ax setup` fetches it once.
  They run headless, in parallel like any test (one browser per test
  binary, a hidden page per `TestUi`).
- Where tests go: a cell's tests in a `#[cfg(test)] mod tests` beside it
  (the template's shape), cross-cell and server tests in `tests/*.rs`.
  `cargo test --lib` is the fast loop; the whole run is the gate.

## Cells

```rust
use aio::prelude::*;
use aio::testing::{RandomCall, Rng, TestCells};
use serde_json::{Value, json};

#[cell(persist)]
pub struct Account {
    pub balance: u64,
}

#[methods]
impl Account {
    pub fn deposit(&mut self, n: u64) {
        self.balance += n;
    }

    pub fn withdraw(&mut self, n: u64) -> Result {
        ensure!(n <= self.balance, "not enough");
        self.balance -= n;
        Ok(())
    }
}

fn amount(r: &mut Rng) -> Value {
    json!([r.below(300)])
}

# fn main() {
// One cell, its typed calls; a refusal is a value to assert on.
let h = Handle::<Account>::memory();
h.deposit(100).unwrap();
assert_eq!(h.withdraw(500).unwrap_err().why, "not enough");

// Seeded random calls: a panic, or a state the check refuses, names the seed.
let money: [RandomCall; 2] = [("deposit", &amount), ("withdraw", &amount)];
let mut t = TestCells::new();
let a = t.cell::<Account>();
let calls = t.fuzz(&a, 42, 200, &money, |s| s.balance < 60_000);
assert_eq!(calls.len(), 200);

// A cell booted from a state of your choosing.
let rich = t.seeded::<Account>(json!({"balance": 5}));
assert_eq!(rich.state().balance, 5);
# }
```

- `TestCells` holds effects until `settle()`, so a test first sees what a
  call asked for (`t.effects()`), and moves schedules only on
  `advance(ms)`. Effects' timers (`aio::time::sleep`, timeouts) run on the
  same virtual clock: a 10 s backoff settles at once.
- A schedule or effect whose call fails fails the test at its end unless
  the test took it with `t.failures()`: nothing fails silently.
- `t.saved::<C>(version, json)` boots from data an older version saved,
  as the app's boot does, migration included.
- `aio::testing::fails(|| ..)` returns the message of a failure a test
  expects.
- Fixtures come in with `include_str!` / `include_bytes!`, never read from
  the source folder at run time.

## Views

```rust,no_run
use aio::act;
use aio::testing::TestUi;
use aio::ui::prelude::*;

#[aio::cell(persist, id = "counter")]
pub struct Counter {
    pub count: i64,
}

#[aio::methods]
impl Counter {
    pub fn add(&mut self, by: i64) {
        self.count += by;
    }
}

aio::client!(Counter, counter);

#[component]
fn App() -> View {
    use_live();
    let n = part(|c| &c.count).get();
    rsx! {
        h1 { "data-t": "count", "{n}" }
        button { onclick: move |_| _ = act!(add(1)), "Add" }
    }
}

# fn main() {
let mut ui = TestUi::new(App);
ui.click("AddButton"); // named from its label and role
assert_eq!(ui.text("count"), "1");
assert_eq!(HANDLE.read_untracked().state().count, 1, "the cell, not only the page");
# }
```

`TestUi::new(App)` mounts the app in a hidden page of the pinned Electron
runtime, headless, with real layout and CSS (`ax setup` fetches the
runtime once). It drives the page as a person does:

- **Find**: `t` is a `data-t` handle, an automatic name made from the
  label and role (`SaveButton`, `EmailInput`), `outer/t`,
  `text=Skip this version` or `css=.rows li:nth-child(2)`; it must match
  exactly one element. `names()`, `handles()`, `present(t)`, `count(css)`.
- **Act**: `click`, `dblclick`, `hover`, `drag_to`, `wheel`, `scroll`,
  `type_text` (literal, `\n` is no Enter), `paste`, `set_value`, `clear`,
  `select`, `check`, `focus`, `press(t, "ctrl+s")`, `press_key`.
- **Read**: `text`, `value`, `attr`, `style`, `rect`, `checked`,
  `disabled`, `expanded`, `focused`, `html`, `contrast`, `surface()`.
- **Wait**: `until_text(t, s)`, `until_contains`, `until(..)`, `settle()`
  (a pending background render), `wait(duration)`.
- **Measure**: `components()` (each component's renders), `wire_bytes()`,
  `shot()` (a PNG once transitions ended, `Mount::no_scrollbars()` for
  pixel-stable shots).
- A refusal nobody looked at fails the test at its end; take the expected
  ones with `failures()`.
- `ax test <app> --video <dir>` records each UI test (`--pace ms` per
  step on screen).
- **Mount**: `aio::testing::Mount::default().size(390, 844).dark()
  .no_scrollbars().before(|| seed()).network(&["api.example.org"])
  .app(App)`; `TestUi::with(Component, props)` mounts one component
  alone. By default a UI test refuses the real network: requests
  elsewhere are cancelled and fail the test.
- **The cell beside the page**: `expect_cell(&HANDLE.read_untracked(), "saved", |c|
  ..)` waits until the cell holds; `read(&GLOBAL)` reads a global signal.
- **When it fails**: the message shows the screen as text (`surface()`)
  and writes a trace (the steps, the page's warnings and errors) under
  the temp folder's `aio-traces/<binary>/<test>`.
- **A served cell under the page**: `TestServer::ui_source(user,
  latency)` gives the view a copy over the real wire, with latency each
  way; the server decides who may call what, and its refusals are the
  test's.

## Servers and sync

```rust
use aio::prelude::*;
use aio::testing::{RandomCall, Rng, Sim, TestServer};
use serde_json::{Value, json};

#[cell(persist, shared)]
pub struct Tally {
    pub n: i64,
}

#[methods]
impl Tally {
    #[client]
    pub fn add(&mut self, by: i64) {
        self.n += by;
    }
}

fn small(r: &mut Rng) -> Value {
    json!([r.below(10)])
}

# fn main() {
let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
rt.block_on(async {
    // A real server on loopback, a fresh key, two real synced copies.
    let server = TestServer::start(Handle::<Tally>::memory(), Default::default()).await.unwrap();
    let (ana, bo) = (server.client("ana").await.unwrap(), server.client("bo").await.unwrap());
    ana.add(2).unwrap();
    bo.add(3).unwrap();
    server.converged(&[&ana, &bo]).await; // online, nothing waiting, the server's state
    assert_eq!(server.state().n, 5);
    let stranger = server.fetch_with("GET", "/ws", &[], "").await.unwrap();
    assert_ne!(stranger.status, 200, "no key, no socket");

    // Three copies over lines a seeded script cuts and heals.
    let calls: [RandomCall; 1] = [("add", &small)];
    let mut sim = Sim::start(Handle::<Tally>::memory(), 3, 7).await.unwrap();
    let script = sim.run(60, &calls, |t| t.n >= 0).await;
    assert!(!script.is_empty(), "a failure would name seed 7 and this script");
});
# }
```

- `TestServer::start(handle, http)` serves a cell as the app does, in a
  sandbox; `start_with(handle, opts)` takes `aio::sync::Options`
  (accounts, limits). `fetch` sends a request with the key, `fetch_with`
  with exactly the headers given (none: a stranger), `page()` is a
  browser page that keeps the cookies the server set (sign-in flows).
- `Sim::start(handle, clients, seed)` then `.run(steps, &calls, holds)`:
  each step a client calls (predicted at once, sent when its line is up),
  the server's own code calls, a line is cut or healed, or time passes;
  then every line heals and every copy must settle on the server's state,
  which `holds`. The same seed is the same script, whatever the timing.

## Recorded I/O

```rust
use aio::cassette::Cassette;

async fn price(item: &str) -> Result<u64, String> {
    Ok(item.len() as u64 * 100) // a real HTTP call in the app
}

# fn main() {
let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
rt.block_on(async {
    let rec = Cassette::record(None);
    let p: u64 = rec.call("price", &"tea", || price("tea")).await.unwrap();
    let play = Cassette::replay(&rec.text()).unwrap(); // include_str! of a saved tape
    let again: u64 = play.call("price", &"tea", || async { Err::<u64, _>("not called") }).await.unwrap();
    assert_eq!(p, again);
});
# }
```

`Cassette::open(path)` records on the first run and replays from then on
(`save()` writes it); `Cassette::passthrough()` calls through and records
nothing (the app's own build). A call is matched by its id and its
arguments: a call the tape lacks fails loudly, never reaches the network,
and errors are recorded and replayed like answers.

## Tests from a running app

- `ax record <app> <cell> <file> [range]` writes a Rust test from what
  the running app's cell just did: booted on the state before the range,
  its calls made again, the state they left asserted.
- `ax expect <app> <cell> <path> eq <value> [--wait 5]` asserts on the
  live state from a script (exit 1 when it fails); `ax call` drives it.
- `ax surface <app>`, `ax trigger <app> <path> click`, `ax shot <app>`
  drive and look at the running window as `TestUi` does a test's.
- `ax lint [dir]` checks what types can't: credentials in shared fields,
  impure methods (clock, files, threads), hooks, accessibility, cell ids,
  plus clippy; `--safe-fix` applies the safe fixes.

## Common mistakes

- `std::thread::sleep` in a test to "wait for" an effect: use
  `t.settle()`, `t.advance(ms)`, `ui.until_text(..)` or `converged`.
- Ignoring refusals: a refusal nobody took fails the test at its end; take
  the expected ones with `failures()` and assert on them.
- Reading fixtures from the source tree at run time: use `include_str!`
  so the test runs from any folder (and in `ax test` on another machine).
- One `TestUi` app instance per test: globals are fresh each mount, so
  don't share state between tests through them.
- Testing a canvas or 3D view by its pixels: keep its state in a cell and
  test the cell; check the drawing with `shot()` against a saved image.

## What each tier proves

| Tier | Proves | Doesn't prove |
|---|---|---|
| `Handle::memory`, `TestCells` | the methods, invariants, effects, schedules, boot order and migrations, on a virtual clock | the wire, the page |
| `TestUi` | the views in the window's engine: layout, focus, forms, what a person can click and read | the real wire (unless `ui_source`), the OS window |
| `TestServer`, `Sim` | the key, `#[client]` rules, sign-in, the wire encoding, sync under cuts | a second process, the installed app |
| `ax build --smoke` | the bundle starts, opens its window and stops cleanly on that OS | that it looks right |
| `ax lab` | nothing on its own: a person looks | |

Use the cheapest tier that can fail for the bug at hand, and one more
above it for the path the app takes: a refusal in a cell test, the
button that shows it in a view test.

## Every test runs in a sandbox

`TestCells`, `TestUi` and `TestServer` each hold an
`aio::paths::Sandbox`: while one lives, the process runs in a fresh temp
world (`<temp>/aio-test-<pid>-<id>`), its home and every OS data dir
aio-rs would use under it. Opening app data under the person's real home
is refused (`a test may not touch real app data: …`), and nothing is
opened in the person's browser or apps. The last sandbox dropped removes
the world.

- `aio::paths::sandbox()` enters (or shares) the same world in a test of
  your own; `aio::paths::in_sandbox()` says which world runs, for code
  that must pick a simulated device over real hardware.
- A test that needs hardware puts the transport behind a trait and gives
  the test world a simulated device, or replays a [cassette](cassettes.md).
- A UI test's page and aio-rs's own outbound requests are held to this
  machine and the hosts its mount allows
  ([the network](ui.md#the-network)).

## Where temporary files go

| What | Where | Removed |
|---|---|---|
| a test's world (data, home) | `<temp>/aio-test-<pid>-<id>` | when its last harness drops |
| a failed UI test's trace | `<temp>/aio-traces/<test binary>/<test>/ui-<time>.json` (20 kept) | when the test passes again |
| a UI test video | where `--video` says | never: it is yours |
| a `TestBrowser` profile | a temp folder | when it drops |

`<temp>` is the OS's temp folder (`TMPDIR` on Linux and macOS). Nothing
a test makes lands in the person's home, so a run leaves nothing to clean
up but what it was asked to write.

## API

`cargo doc -p aio-rs --no-deps --all-features --open`: `aio::testing`
(`TestCells`, `TestUi`, `Mount`, `TestServer`, `TestApps`, `Sim`,
`TestBrowser`, `TestIo`, `patches`, `call_all`, `smoke`, `fails`,
`totp_code`), `aio::cassette`.

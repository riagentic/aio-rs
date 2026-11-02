# Cassettes

Some calls can't be made in a test: a device that isn't plugged into the
build machine, a price feed that answers differently every minute, a
vendor sandbox that is down. A hand-written mock drifts from the real
thing and keeps passing. A cassette records the real call once and
replays what came back from then on: no device, no network, the same
answer every run, and the fixture came from the real thing.

`aio::cassette::Cassette` wraps any async call (an effect's HTTP request,
a device transport, a process). It comes with aio-rs, in every build.

## Record once, replay forever

```rust
use aio::cassette::Cassette;

/// The call a test can't make: an HTTP GET in the app.
async fn rate(pair: &str) -> Result<f64, String> {
    Ok(if pair == "EUR-USD" { 1.08 } else { 0.0 })
}

# fn main() {
let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
rt.block_on(async {
    // The first run records: the real call runs, its answer is kept.
    let rec = Cassette::record(None);
    let live: f64 = rec.call("rates.get", &"EUR-USD", || rate("EUR-USD")).await.unwrap();

    // Every run after replays: nothing runs, the kept answer comes back.
    let tape = rec.text(); // in a test: include_str!("fixtures/rates.json")
    let play = Cassette::replay(&tape).unwrap();
    let again: f64 = play.call("rates.get", &"EUR-USD", || async { Err::<f64, _>("never called") }).await.unwrap();
    assert_eq!(live, again);
});
# }
```

In a real test the tape is a file next to the test:
`Cassette::open(path)` records there when the file doesn't exist and
replays it when it does. Run the test once with the device or network
there, `save()` it, commit the file; every run after needs neither.

## The API

| Item | Does |
|---|---|
| `Cassette::open(path)` | the tape at `path` replayed; none there yet: recorded, to `path` on `save()` |
| `Cassette::record(Some(path))` / `record(None)` | always record (to `path`, or in memory for `text()`) |
| `Cassette::replay(&text)` | answer from a tape's text (`include_str!` of a saved one) |
| `Cassette::passthrough()` | every call runs, nothing is kept: the app's own build |
| `c.call(id, &args, \|\| io())` | `io` as the mode says; `Result<R, String>` |
| `c.wrap(id, f)` | `f` (an async fn of one argument) with each call through the tape: `let key = c.wrap("trezor.key", get_key); key((path,)).await` |
| `c.save()` | write the tape to its path when recording (else nothing) |
| `c.text()` | the recorded tape as JSON |
| `c.frames()` | the calls recorded so far (`Frame`) |
| `c.mode()` | `Mode::Record`, `Replay` or `Passthrough` |

- `call` takes the call's id (a name you pick, `rates.get`), its
  arguments (anything `Serialize`) and a closure making the future. The
  answer type `R` is `Serialize + DeserializeOwned`; the error type is
  anything `Display`, kept as its text.
- Clones share one tape: hand a clone to each effect that needs it.
- Arguments that don't serialize are refused (`cassette <id>: its
  arguments don't serialize, and they key the tape: …`): a key that
  can't match on replay never gets recorded.
- `open` of a file that is there but isn't a tape fails
  (`cassette: not a tape: … record it again with AIO_CASSETTE=rerecord`):
  it never records over a broken fixture unasked.
- `AIO_CASSETTE=rerecord cargo test` makes every `open` record afresh
  over the tape that is there (the calls changed, or it is damaged);
  `save()` writes the new one. Any other value of `AIO_CASSETTE` fails.

### Modes

| Mode | A call |
|---|---|
| `Record` | runs, and its answer (or error) is kept |
| `Replay` | never runs: the tape answers |
| `Passthrough` | runs, nothing kept: the tape is out of the way |

Pin a CI job to replay with `Cassette::replay(include_str!(..))`: a
missing tape is then a compile error, never a silent recording on the
build machine.

## How a call is matched

A frame is keyed by its id and its arguments (as JSON text), and counted:
the same call again is the next frame of that id and arguments. Repeats
replay in the order they were recorded, so a counter, a nonce or a page
of a feed replays exactly:

```rust
use aio::cassette::Cassette;
use std::sync::atomic::{AtomicU32, Ordering};

# fn main() {
let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
rt.block_on(async {
    let n = AtomicU32::new(0);
    let rec = Cassette::record(None);
    for _ in 0..3 {
        let _: u32 = rec.call("counter.next", &(), || async { Ok::<_, String>(n.fetch_add(1, Ordering::SeqCst) + 1) }).await.unwrap();
    }
    let page: Vec<String> = rec.call("feed.page", &2, || async { Ok::<_, String>(vec!["c".into(), "d".into()]) }).await.unwrap();

    let play = Cassette::replay(&rec.text()).unwrap();
    let mut got = vec![];
    for _ in 0..3 {
        got.push(play.call::<_, u32, String, _>("counter.next", &(), || async { Ok(0) }).await.unwrap());
    }
    assert_eq!(got, [1, 2, 3], "in the order recorded");
    assert_eq!(play.call::<_, Vec<String>, String, _>("feed.page", &2, || async { Ok(vec![]) }).await.unwrap(), page);

    // A call the tape lacks fails loudly: it never reaches the real thing.
    let why = play.call::<_, Vec<String>, String, _>("feed.page", &3, || async { Ok(vec![]) }).await.unwrap_err();
    assert_eq!(why, "cassette: no recorded call feed.page(3): the calls changed; record the tape again (AIO_CASSETTE=rerecord)");
});
# }
```

A new argument, an extra call or a call in another order replays nothing
and says so. That is the point: a cassette that quietly called through
would turn the build machine into one that needs the device at the
moment the tape stopped describing the code. Delete the file and run once
with the real thing to record it again.

## Errors record too

A failed call is kept as its error's text and replays as that error, so
the unhappy path is as repeatable as the happy one:

```rust
use aio::cassette::Cassette;

# fn main() {
let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
rt.block_on(async {
    let rec = Cassette::record(None);
    let busy = rec.call::<_, String, _, _>("device.read", &1, || async { Err("device busy") }).await;
    assert_eq!(busy.unwrap_err(), "device busy");

    let play = Cassette::replay(&rec.text()).unwrap();
    let again = play.call::<_, String, String, _>("device.read", &1, || async { Ok("never".into()) }).await;
    assert_eq!(again.unwrap_err(), "device busy");
});
# }
```

Only the text survives: assert on the message, not on an error type.

## In an app

The app builds with `Cassette::passthrough()`, a test with a tape. Put it
where the effect finds it, for example a `OnceLock` the test sets before
the call:

```rust
use aio::cassette::Cassette;
use aio::prelude::*;
use aio::testing::TestCells;
use std::sync::OnceLock;

static TAPE: OnceLock<Cassette> = OnceLock::new();

/// The app's tape: the test's, else none (every call runs).
fn tape() -> Cassette {
    TAPE.get().cloned().unwrap_or_else(Cassette::passthrough)
}

/// The real request.
async fn get_temp(city: String) -> Result<i32, String> {
    Err(format!("no network in this example ({city})"))
}

/// The request through the tape: what the effect awaits.
async fn temp(city: String) -> Result<i32, String> {
    tape().call("weather.get", &city, || get_temp(city.clone())).await
}

#[cell]
pub struct Weather {
    pub temp: Option<i32>,
    pub error: Option<String>,
}

#[methods]
impl Weather {
    pub async fn load(&mut self, city: String) -> Result {
        let got: std::result::Result<i32, String> = temp(city).await;
        match got {
            Ok(t) => self.temp = Some(t),
            Err(e) => self.error = Some(e),
        }
        Ok(())
    }
}

# fn main() {
// A tape as a test keeps it (include_str! of the saved file).
let saved = r#"[{"id": "weather.get", "key": "\"Oslo\"", "seq": 0, "answer": {"Ok": 4}}]"#;
TAPE.set(Cassette::replay(saved).unwrap()).ok().unwrap();
let mut t = TestCells::new();
let w = t.cell::<Weather>();
w.load("Oslo".into()).unwrap();
t.settle();
assert_eq!(w.state().temp, Some(4));
# }
```

- Wrap the lowest call that speaks plain data: the transport
  (`device.exchange(bytes)`, the HTTP GET), not a decoded domain object.
  Its arguments are the match key and its answer goes through JSON.
- One tape per test file keeps a re-record small. Name ids after the
  call site (`weather.get`, `printer.status`).
- The cassette sits under the effect, so the cell, its log and its replay
  are unchanged: the log keeps the answer the effect handed back, as in
  the app.

## A tape

A tape is a JSON array of frames, readable in a diff:

```json
[
  {
    "id": "weather.get",
    "key": "\"Oslo\"",
    "seq": 0,
    "answer": { "Ok": 4 }
  },
  {
    "id": "weather.get",
    "key": "\"Nowhere\"",
    "seq": 0,
    "answer": { "Err": "404: no such city" }
  }
]
```

| Field | Holds |
|---|---|
| `id` | the id given to `call` |
| `key` | the arguments, as JSON text |
| `seq` | which call of this id and key it was (0, 1, …) |
| `answer` | `{"Ok": value}`, or `{"Err": "text"}` |

A tape holds whatever the real call returned. Record against a test
account, and read the tape before you commit it: a token in an answer
is a token in the repository.

## What aio has that aio-rs does without

| aio | aio-rs |
|---|---|
| `openCassette(path, { mode })` | `open(path)`; `record`, `replay`, `passthrough` pick a mode |
| a corrupt file is re-recorded | a corrupt file fails, naming `AIO_CASSETTE=rerecord`, which records afresh |
| `createCassette(mode, { initial })` | `Cassette::replay(&text)` |
| `tape.wrap(id, fn)` | `tape.wrap(id, f)`: an async fn whose calls go through the tape |
| an unserializable argument falls back to `String(args)` (never replays) | refused: the call is named, and why its arguments don't serialize |
| frames `ok`, `value`, `error` | `answer: {"Ok": ..}` or `{"Err": ..}` |

## Common mistakes

- Recording in CI: a job that may never reach the device should replay
  from `include_str!`, so a missing tape fails the build.
- Wrapping a high-level call whose answer is a rich type: wrap the
  transport underneath, so the tape is plain data.
- A changing argument (a timestamp, a random id) in the key: every run is
  a new call the tape lacks. Pass it outside the arguments, or leave it
  out of what you record.
- Forgetting `save()` after the recording run: the tape stays in memory.
- Committing a tape with secrets in it.

## API

`cargo doc -p aio-rs --no-deps --open`: `aio::cassette` (`Cassette`,
`Mode`, `Frame`).

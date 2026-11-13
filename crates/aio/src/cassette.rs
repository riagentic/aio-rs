//! Record once, replay forever (aio's `cassette`): a device transport, a
//! network call, any async I/O an effect does. Wrap it in [`Cassette::call`]:
//! recording, the real call runs and its answer is kept; replaying, the kept
//! answer comes back and nothing runs, so a test needs no device or server.
//! A call is keyed by its id and its arguments (JSON); repeats replay in the
//! order they were recorded. [`Cassette::wrap`] makes an async fn one that
//! goes through the tape (aio's `tape.wrap(id, fn)`). `AIO_CASSETTE=rerecord`
//! makes [`Cassette::open`] record afresh over a tape that is there, even an
//! unreadable one (aio re-recorded a corrupt tape silently; here a tape that
//! doesn't read is refused unless asked). Arguments that don't serialize are
//! refused too, unless [`Cassette::lenient`] keys them as aio does.
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// One recorded call.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    /// The call site: `trezor.get_public_key`.
    pub id: String,
    /// Its arguments, as JSON text.
    pub key: String,
    /// Which call of this id and key it was (0, 1, …).
    pub seq: u32,
    /// The answer, or the error it gave.
    pub answer: Result<Value, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Record,
    Replay,
    /// Every call runs; nothing is kept.
    Passthrough,
}

#[derive(Default)]
struct Tape {
    recorded: Vec<Frame>,
    queued: HashMap<(String, String), VecDeque<Frame>>,
    seqs: HashMap<(String, String), u32>,
}

/// A tape of calls; clones share it.
#[derive(Clone)]
pub struct Cassette {
    mode: Mode,
    path: Option<PathBuf>,
    tape: Arc<Mutex<Tape>>,
    /// Arguments that don't serialize key the tape by their error ([`Self::lenient`]).
    lenient: bool,
}

impl Cassette {
    /// Keep every call's answer, for [`Self::save`] (to `path`) or [`Self::text`].
    pub fn record(path: Option<PathBuf>) -> Self {
        Self { mode: Mode::Record, path, tape: Default::default(), lenient: false }
    }

    /// Answer from a tape (`include_str!` of a saved one, in a test).
    pub fn replay(text: &str) -> Result<Self, String> {
        let mut frames: Vec<Frame> = serde_json::from_str(text).map_err(|e| format!("cassette: not a tape: {e}"))?;
        frames.sort_by_key(|f| f.seq);
        let mut tape = Tape::default();
        for mut f in frames {
            // Keyed as [`key_of`] keys now (a tape from before kept a map's
            // or a struct's entries in their own order).
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&f.key) {
                f.key = v.to_string();
            }
            tape.queued.entry((f.id.clone(), f.key.clone())).or_default().push_back(f);
        }
        Ok(Self { mode: Mode::Replay, path: None, tape: Arc::new(Mutex::new(tape)), lenient: false })
    }

    pub fn passthrough() -> Self {
        Self { mode: Mode::Passthrough, path: None, tape: Default::default(), lenient: false }
    }

    /// The tape at `path` replayed, or recorded there when there is none
    /// yet: record once, replay from then on. With `AIO_CASSETTE=rerecord`,
    /// recorded afresh whatever is there (the calls changed, or the tape is
    /// damaged); a tape that doesn't read is refused otherwise.
    pub fn open(path: PathBuf) -> Result<Self, String> {
        Self::open_as(path, std::env::var("AIO_CASSETTE").ok().as_deref())
    }

    /// [`Self::open`] with `AIO_CASSETTE` as given.
    fn open_as(path: PathBuf, env: Option<&str>) -> Result<Self, String> {
        match env {
            Some("rerecord") => return Ok(Self::record(Some(path))),
            None | Some("") => {}
            Some(v) => return Err(format!("AIO_CASSETTE={v}: the one value is `rerecord`")),
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::replay(&text).map_err(|e| {
                format!("{e} ({}): record it again with AIO_CASSETTE=rerecord", path.display())
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::record(Some(path))),
            Err(e) => Err(format!("cassette {}: {e}", path.display())),
        }
    }

    /// Arguments that don't serialize are kept, not refused (aio's
    /// `String(args)` fallback): such a call is keyed by
    /// `(unserializable: <why>)`, so every such call of an id shares one
    /// key and replays in its recorded order, whatever its arguments were.
    pub fn lenient(self) -> Self {
        Self { lenient: true, ..self }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    fn tape(&self) -> std::sync::MutexGuard<'_, Tape> {
        self.tape.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `run`, as this cassette says: called and kept, answered from the
    /// tape, or just called. An error (the real one, or a recorded one) is
    /// its text. Arguments that don't serialize are refused (they key the
    /// tape), naming the call and why, unless the tape is [`Self::lenient`].
    pub async fn call<A, R, E, F>(&self, id: &str, args: &A, run: impl FnOnce() -> F) -> Result<R, String>
    where
        A: Serialize,
        R: Serialize + DeserializeOwned,
        E: std::fmt::Display,
        F: std::future::Future<Output = Result<R, E>>,
    {
        self.keyed(id, key_of(id, args, self.lenient)?, run).await
    }

    /// `f` with every call of it going through this tape as `id` (aio's
    /// `tape.wrap(id, fn)`): `let key = tape.wrap("trezor.key", get_key);
    /// key((path,)).await`. Its argument is what keys the call.
    pub fn wrap<A, R, E>(&self, id: &str, f: impl AsyncFn(A) -> Result<R, E>) -> impl AsyncFn(A) -> Result<R, String>
    where
        A: Serialize,
        R: Serialize + DeserializeOwned,
        E: std::fmt::Display,
    {
        let (tape, id) = (self.clone(), id.to_string());
        async move |args: A| {
            let key = key_of(&id, &args, tape.lenient)?;
            tape.keyed(&id, key, || f(args)).await
        }
    }

    async fn keyed<R, E, F>(&self, id: &str, args: String, run: impl FnOnce() -> F) -> Result<R, String>
    where
        R: Serialize + DeserializeOwned,
        E: std::fmt::Display,
        F: std::future::Future<Output = Result<R, E>>,
    {
        let key = (id.to_string(), args);
        match self.mode {
            Mode::Passthrough => run().await.map_err(|e| e.to_string()),
            Mode::Replay => {
                let frame = self.tape().queued.get_mut(&key).and_then(VecDeque::pop_front).ok_or_else(|| {
                    format!(
                        "cassette: no recorded call {id}({}): the calls changed; record the tape again (AIO_CASSETTE=rerecord)",
                        key.1
                    )
                })?;
                let v = frame.answer?;
                serde_json::from_value(v).map_err(|e| format!("cassette {id}: the recorded answer doesn't fit: {e}"))
            }
            Mode::Record => {
                // Numbered as it starts, as a replay takes them: calls that
                // overlap may finish in another order.
                let seq = {
                    let mut t = self.tape();
                    let n = t.seqs.entry(key.clone()).or_default();
                    *n += 1;
                    *n - 1
                };
                let out = run().await.map_err(|e| e.to_string());
                let answer = match &out {
                    Ok(v) => serde_json::to_value(v).map_err(|e| format!("cassette {id}: {e}")),
                    Err(e) => Err(e.clone()),
                };
                self.tape().recorded.push(Frame { id: key.0, key: key.1, seq, answer });
                out
            }
        }
    }

    /// The calls recorded so far.
    pub fn frames(&self) -> Vec<Frame> {
        self.tape().recorded.clone()
    }

    /// The tape as JSON text.
    pub fn text(&self) -> String {
        serde_json::to_string_pretty(&self.tape().recorded).unwrap_or_else(|_| "[]".into())
    }

    /// Write the tape to its path (recording; else nothing to do).
    pub fn save(&self) -> std::io::Result<()> {
        match (&self.path, self.mode) {
            (Some(p), Mode::Record) => crate::paths::write_atomic(p, self.text().as_bytes(), false),
            _ => Ok(()),
        }
    }
}

/// A call's arguments as the tape keys them (JSON); unserializable ones
/// refused, or `lenient`, keyed by why.
fn key_of(id: &str, args: &impl Serialize, lenient: bool) -> Result<String, String> {
    // Through a Value: a map's entries sorted (serde_json's Map), never in a
    // hash map's order, which a new process's hasher changes.
    serde_json::to_value(args).map(|v| v.to_string()).or_else(|e| match lenient {
        true => Ok(format!("(unserializable: {e})")),
        false => Err(format!(
            "cassette {id}: its arguments don't serialize, and they key the tape: {e} (Cassette::lenient keeps such calls)"
        )),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering::SeqCst};

    fn block<R>(f: impl std::future::Future<Output = R>) -> R {
        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(f)
    }

    async fn device(c: &Cassette, path: &str, calls: &AtomicU32) -> Result<String, String> {
        c.call("trezor.key", &(path,), || async {
            calls.fetch_add(1, SeqCst);
            if path == "bad" { Err("device unplugged") } else { Ok(format!("xpub-{path}-{}", calls.load(SeqCst))) }
        })
        .await
    }

    #[test]
    fn a_recorded_session_replays_without_the_device() {
        let calls = AtomicU32::new(0);
        let rec = Cassette::record(None);
        let got = block(async {
            [device(&rec, "m/0", &calls).await, device(&rec, "m/0", &calls).await, device(&rec, "bad", &calls).await]
        });
        assert_eq!(got, [Ok("xpub-m/0-1".into()), Ok("xpub-m/0-2".into()), Err("device unplugged".into())]);
        assert_eq!(rec.frames().iter().map(|f| f.seq).collect::<Vec<_>>(), [0, 1, 0]);
        let play = Cassette::replay(&rec.text()).unwrap();
        let calls = AtomicU32::new(0);
        let again = block(async {
            [device(&play, "m/0", &calls).await, device(&play, "m/0", &calls).await, device(&play, "bad", &calls).await]
        });
        assert_eq!(again, got, "repeats in recorded order, the error as an error");
        assert_eq!(calls.load(SeqCst), 0, "nothing ran");
        let e = block(device(&play, "m/0", &calls)).unwrap_err();
        assert!(e.starts_with("cassette: no recorded call trezor.key([\"m/0\"])"), "{e}");
        assert!(Cassette::replay("{not json").is_err());
        let through = Cassette::passthrough();
        assert_eq!(block(device(&through, "x", &calls)), Ok("xpub-x-1".into()));
        assert!(through.frames().is_empty() && through.save().is_ok());
    }

    /// Calls with equal arguments that overlap: each replays the answer of
    /// the one that started in its place, whichever finished first.
    #[test]
    fn overlapping_repeats_replay_in_the_order_they_started() {
        async fn rate(c: &Cassette, waits: u32, answer: &str) -> Result<String, String> {
            c.call("rates.get", &1, || async move {
                for _ in 0..waits {
                    tokio::task::yield_now().await;
                }
                Ok::<_, String>(answer.to_string())
            })
            .await
        }
        let rec = Cassette::record(None);
        let got = block(async { tokio::join!(rate(&rec, 3, "slow"), rate(&rec, 0, "fast")) });
        assert_eq!(got, (Ok("slow".into()), Ok("fast".into())));
        let play = Cassette::replay(&rec.text()).unwrap();
        let again = block(async { tokio::join!(rate(&play, 0, "-"), rate(&play, 0, "-")) });
        assert_eq!(again, got, "the first call started gets the first answer");
    }

    /// A map's entries key the call in one order, whatever order a hash
    /// map holds them in (a new process, a new hasher seed).
    #[test]
    fn a_maps_arguments_key_the_same_whatever_its_order() {
        let pairs = [("q", "x"), ("page", "1"), ("sort", "new"), ("lang", "cs"), ("tz", "utc"), ("n", "20"), ("f", "a")];
        let hashed: HashMap<&str, &str> = pairs.into_iter().collect();
        let sorted: std::collections::BTreeMap<&str, &str> = pairs.into_iter().collect();
        assert_eq!(key_of("api.search", &hashed, false), key_of("api.search", &sorted, false));
        // A tape keyed in another order (recorded before) still replays.
        let old = r#"[{"id":"api.search","key":"{\"q\":\"x\",\"page\":\"1\"}","seq":0,"answer":{"Ok":3}}]"#;
        let play = Cassette::replay(old).unwrap();
        let args: HashMap<&str, &str> = [("page", "1"), ("q", "x")].into_iter().collect();
        assert_eq!(block(play.call("api.search", &args, || async { Ok::<u32, String>(0) })), Ok(3));
    }

    #[test]
    fn a_tape_is_recorded_once_then_replayed() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("session.json");
        let rec = Cassette::open(p.clone()).unwrap();
        assert_eq!(rec.mode(), Mode::Record, "no tape yet");
        let calls = AtomicU32::new(0);
        block(device(&rec, "m/1", &calls)).unwrap();
        rec.save().unwrap();
        let play = Cassette::open(p.clone()).unwrap();
        assert_eq!(play.mode(), Mode::Replay);
        assert_eq!(block(device(&play, "m/1", &AtomicU32::new(0))), Ok("xpub-m/1-1".into()));
        assert_eq!(Cassette::open_as(p.clone(), Some("rerecord")).unwrap().mode(), Mode::Record, "asked: afresh");
        assert!(Cassette::open_as(p, Some("yes")).is_err_and(|e| e.contains("`rerecord`")));
    }

    #[test]
    fn a_damaged_tape_is_refused_unless_rerecorded() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("session.json");
        std::fs::write(&p, "[{\"id\": trunc").unwrap();
        let Err(e) = Cassette::open_as(p.clone(), None) else { panic!("a damaged tape replayed") };
        assert!(e.contains("not a tape") && e.contains("AIO_CASSETTE=rerecord"), "{e}");
        let rec = Cassette::open_as(p.clone(), Some("rerecord")).unwrap();
        block(device(&rec, "m/2", &AtomicU32::new(0))).unwrap();
        rec.save().unwrap();
        assert_eq!(Cassette::open_as(p, None).unwrap().mode(), Mode::Replay, "the new tape reads");
    }

    #[test]
    fn a_wrapped_fn_records_and_replays() {
        let calls = AtomicU32::new(0);
        let real = async |(n,): (u32,)| -> Result<u32, String> {
            calls.fetch_add(1, SeqCst);
            Ok(n * 2)
        };
        let rec = Cassette::record(None);
        let twice = rec.wrap("math.double", real);
        assert_eq!(block(twice((21,))), Ok(42));
        let play = Cassette::replay(&rec.text()).unwrap();
        let replayed = play.wrap("math.double", async |_: (u32,)| -> Result<u32, String> { panic!("ran on replay") });
        assert_eq!(block(replayed((21,))), Ok(42));
        assert_eq!(calls.load(SeqCst), 1, "only the recording ran it");
        assert!(block(replayed((21,))).unwrap_err().contains("AIO_CASSETTE=rerecord"), "one recorded call, one replay");
    }

    #[test]
    fn unserializable_arguments_are_refused_naming_the_call() {
        let bad: HashMap<(u8, u8), u8> = [((1, 2), 3)].into();
        let e = block(Cassette::record(None).call("grid.cell", &bad, || async { Ok::<u8, String>(0) })).unwrap_err();
        assert!(e.starts_with("cassette grid.cell: its arguments don't serialize"), "{e}");
    }

    #[test]
    fn a_lenient_tape_keeps_unserializable_arguments_and_replays_them_in_order() {
        let bad: HashMap<(u8, u8), u8> = [((1, 2), 3)].into();
        let rec = Cassette::record(None).lenient();
        let got = block(async {
            let one = rec.call("grid.cell", &bad, || async { Ok::<u8, String>(1) }).await;
            [one, rec.call("grid.cell", &bad, || async { Ok::<u8, String>(2) }).await]
        });
        assert_eq!(got, [Ok(1), Ok(2)]);
        assert!(rec.frames()[0].key.starts_with("(unserializable: "), "{:?}", rec.frames());
        let play = Cassette::replay(&rec.text()).unwrap().lenient();
        let again = block(async {
            let one = play.call("grid.cell", &bad, || async { Ok::<u8, String>(9) }).await;
            [one, play.call("grid.cell", &bad, || async { Ok::<u8, String>(9) }).await]
        });
        assert_eq!(again, got, "replayed in recorded order, nothing ran");
    }
}

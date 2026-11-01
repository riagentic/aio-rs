//! Record once, replay forever (aio's `cassette`): a device transport, a
//! network call, any async I/O an effect does. Wrap it in [`Cassette::call`]:
//! recording, the real call runs and its answer is kept; replaying, the kept
//! answer comes back and nothing runs, so a test needs no device or server.
//! A call is keyed by its id and its arguments (JSON); repeats replay in the
//! order they were recorded.
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
}

impl Cassette {
    /// Keep every call's answer, for [`Self::save`] (to `path`) or [`Self::text`].
    pub fn record(path: Option<PathBuf>) -> Self {
        Self { mode: Mode::Record, path, tape: Default::default() }
    }

    /// Answer from a tape (`include_str!` of a saved one, in a test).
    pub fn replay(text: &str) -> Result<Self, String> {
        let mut frames: Vec<Frame> = serde_json::from_str(text).map_err(|e| format!("cassette: not a tape: {e}"))?;
        frames.sort_by_key(|f| f.seq);
        let mut tape = Tape::default();
        for f in frames {
            tape.queued.entry((f.id.clone(), f.key.clone())).or_default().push_back(f);
        }
        Ok(Self { mode: Mode::Replay, path: None, tape: Arc::new(Mutex::new(tape)) })
    }

    pub fn passthrough() -> Self {
        Self { mode: Mode::Passthrough, path: None, tape: Default::default() }
    }

    /// The tape at `path` replayed, or recorded there when there is none
    /// yet: record once, replay from then on.
    pub fn open(path: PathBuf) -> Result<Self, String> {
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::replay(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::record(Some(path))),
            Err(e) => Err(format!("cassette {}: {e}", path.display())),
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    fn tape(&self) -> std::sync::MutexGuard<'_, Tape> {
        self.tape.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `run`, as this cassette says: called and kept, answered from the
    /// tape, or just called. An error (the real one, or a recorded one) is
    /// its text.
    pub async fn call<A, R, E, F>(&self, id: &str, args: &A, run: impl FnOnce() -> F) -> Result<R, String>
    where
        A: Serialize,
        R: Serialize + DeserializeOwned,
        E: std::fmt::Display,
        F: std::future::Future<Output = Result<R, E>>,
    {
        let key = (id.to_string(), serde_json::to_string(args).map_err(|e| format!("cassette {id}: {e}"))?);
        match self.mode {
            Mode::Passthrough => run().await.map_err(|e| e.to_string()),
            Mode::Replay => {
                let frame = self.tape().queued.get_mut(&key).and_then(VecDeque::pop_front).ok_or_else(|| {
                    format!("cassette: no recorded call {id}({}): record the tape again (the calls changed)", key.1)
                })?;
                let v = frame.answer?;
                serde_json::from_value(v).map_err(|e| format!("cassette {id}: the recorded answer doesn't fit: {e}"))
            }
            Mode::Record => {
                let out = run().await.map_err(|e| e.to_string());
                let answer = match &out {
                    Ok(v) => serde_json::to_value(v).map_err(|e| format!("cassette {id}: {e}")),
                    Err(e) => Err(e.clone()),
                };
                let mut t = self.tape();
                let seq = t.seqs.entry(key.clone()).or_default();
                let frame = Frame { id: key.0, key: key.1, seq: *seq, answer };
                *seq += 1;
                t.recorded.push(frame);
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

    #[test]
    fn a_tape_is_recorded_once_then_replayed() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("session.json");
        let rec = Cassette::open(p.clone()).unwrap();
        assert_eq!(rec.mode(), Mode::Record, "no tape yet");
        let calls = AtomicU32::new(0);
        block(device(&rec, "m/1", &calls)).unwrap();
        rec.save().unwrap();
        let play = Cassette::open(p).unwrap();
        assert_eq!(play.mode(), Mode::Replay);
        assert_eq!(block(device(&play, "m/1", &AtomicU32::new(0))), Ok("xpub-m/1-1".into()));
    }
}

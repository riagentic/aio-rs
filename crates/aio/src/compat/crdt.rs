//! aio's CRDT sync on `/ws` (`src/sync/server-handler.ts`): the `op` and
//! `sync-req` frames of an aio page whose cells are `sync: true`, so it keeps
//! working offline against an aio-rs app.
//!
//! The page sends each local call as `op {id, cell, action, payload, hlc}`
//! and is answered `sync-ack {cell, opId, serverHlc, serverTs, serverNow}`
//! or `op-rejected {opId, cell, reason, serverNow?}`; every other aio link
//! that sees the whole cell gets the op as `op {…, serverTs}`. On
//! (re)connect it sends `sync-req {clientId, reqId?, cells: {<cell>:
//! {lastHlc, lastServerTs?}}, pendingOps}`: each pending op is taken as an
//! `op` is, then `sync-res` says where each asked cell is. While the cell
//! is not taking calls (paused by time travel, closing) an op is held, not
//! refused: `sync-err {reason}`, which the page answers by asking again.
//!
//! The same contract on a smaller design than aio's ~3,700 lines, because
//! aio-rs already has what aio built for it:
//! - aio's op log (`sync_ops`) and its id dedup are aio-rs's event log and its
//!   synced-call ids (`ops`): an op is the call `<cell>:<action>` with the
//!   id [`op_id`] made of it, so a re-send is acked again, never applied
//!   twice, for as long as aio-rs keeps call ids (`Op::MAX_AGE_MS`), and an op
//!   older than that is refused as aio's stale resend (the page makes it
//!   again). An op's `serverTs` is its event's seq.
//! - aio answers a catch-up with the ops since the page's cursor, or with a
//!   snapshot when the cursor is behind its compaction. Here every answer
//!   is a snapshot (the page's view of each asked cell at the cell's seq):
//!   the answer aio itself gives after any compaction, which its client
//!   folds the same way (pending ops re-applied, acks at or below the
//!   snapshot's seq known to be in it). It costs a whole cell per catch-up
//!   instead of the ops since; nothing a page shows differs.
//! - aio's `push` frames (server writes folded into the client's confirmed
//!   state) are an opt-in of its client (`pushPatch`); without them the
//!   client compares the server's state frames with its own and asks for a
//!   catch-up when they differ, which is the path taken here.
//! - aio refuses at boot a sync cell with a view filter, since an op goes to
//!   every peer as is. Here an op goes only to a link that sees its whole
//!   cell; one that sees less follows by its state frames and snapshots.
use super::data::method_of;
use super::wire::cells;
use crate::link::View;
use crate::{Cell, Handle};
use serde_json::{Value, json};
use std::sync::Mutex;
use tokio::sync::broadcast;

/// aio's `SYNC_DEFAULTS.maxDrift`: an op stamped further ahead is refused.
const MAX_DRIFT_MS: i64 = 60_000;
/// aio's `STALE_OP_REASON`: its client makes an op refused so again.
const STALE: &str = "stale-beyond-retention";
/// aio's `MAX_HLC_COUNTER`.
const MAX_COUNTER: i64 = 1 << 31;

/// A cell's aio sync, shared by its links: the server's clock and the ops
/// accepted, for the other links.
pub(crate) struct Shared {
    clock: Mutex<Hlc>,
    pub(crate) ops: broadcast::Sender<Passed>,
}

impl Default for Shared {
    fn default() -> Self {
        Self { clock: Mutex::new(Hlc::default()), ops: broadcast::channel(256).0 }
    }
}

/// An accepted op, for every aio link but `from`.
#[derive(Clone, Debug)]
pub(crate) struct Passed {
    pub(crate) from: u64,
    pub(crate) cell: String,
    pub(crate) frame: Value,
}

/// aio's hybrid logical clock (`hlc.ts`), node `server`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Hlc {
    physical: i64,
    counter: i64,
}

impl Hlc {
    fn tick(&mut self, now: i64) -> Value {
        if now > self.physical {
            (self.physical, self.counter) = (now, 0);
        } else {
            self.counter += 1;
        }
        json!([self.physical, self.counter, "server"])
    }

    /// A page's stamp moves the clock on, unless it is too far off or not one.
    fn receive(&mut self, (physical, counter): (i64, i64), now: i64) {
        if (physical - now).abs() > MAX_DRIFT_MS || !(0..=MAX_COUNTER).contains(&counter) {
            return;
        }
        if now > self.physical && now > physical {
            (self.physical, self.counter) = (now, 0);
        } else if physical > self.physical {
            (self.physical, self.counter) = (physical, counter + 1);
        } else if physical == self.physical {
            self.counter = self.counter.max(counter) + 1;
        } else {
            self.counter += 1;
        }
    }
}

/// An aio sync op, checked as aio's `isValidSyncOp` checks it.
#[derive(Debug, PartialEq)]
struct SyncOp<'a> {
    id: &'a str,
    cell: &'a str,
    action: &'a str,
    /// Its stamp's wall time and counter.
    hlc: (i64, i64),
}

const FORBIDDEN: [&str; 3] = ["__proto__", "constructor", "prototype"];

/// `None` when it isn't one: no string id, cell or action, no `[number,
/// number, string]` stamp, or an internal action (`__…`).
fn parse(op: &Value) -> Option<SyncOp<'_>> {
    let (id, cell, action) = (op["id"].as_str()?, op["cell"].as_str()?, op["action"].as_str()?);
    let h = op["hlc"].as_array().filter(|h| h.len() == 3 && h[2].is_string())?;
    let (physical, counter) = (h[0].as_f64().filter(|p| p.is_finite())?, h[1].as_f64()?);
    let fine =
        !id.is_empty() && !FORBIDDEN.contains(&cell) && !FORBIDDEN.contains(&action) && !action.starts_with("__");
    fine.then_some(SyncOp { id, cell, action, hlc: (physical as i64, counter as i64) })
}

/// The id of aio-rs's synced call an aio op is (pure): a UUIDv7 at the op's
/// stamp, the rest of it the op's id hashed, so a re-send is the same call.
fn op_id(id: &str, ms: i64) -> String {
    let h = crate::sha256::Sha256::digest(format!("aio-v1-op\0{id}").as_bytes());
    let rest: [u8; 10] = h[..10].try_into().expect("ten of 32 bytes");
    uuid::Builder::from_unix_timestamp_millis(ms.max(0) as u64, &rest).into_uuid().to_string()
}

fn enc(t: &str, d: Value) -> Value {
    json!({ "v": 2, "t": t, "d": d })
}

fn rejected(id: &str, cell: &str, reason: String, now: Option<i64>) -> Value {
    let mut d = json!({ "opId": id, "cell": cell, "reason": reason });
    if let Some(n) = now {
        d["serverNow"] = n.into();
    }
    enc("op-rejected", d)
}

/// Why the cell takes no calls right now (an op waits), if so.
fn held<T: Cell>(h: &Handle<T>) -> Option<String> {
    if h.paused() {
        return Some(format!("{}: paused by time travel; the change is held, not refused", T::ID));
    }
    h.closing().then(|| format!("{}: the app is closing; the change is held, not refused", T::ID))
}

/// What a taken op says back.
pub(crate) enum Taken {
    /// Its `sync-ack` or `op-rejected` (or nothing, as aio, for an op
    /// without an id and cell to name).
    Said(Option<Value>),
    /// `sync-err`: the cell isn't taking calls; the page asks again.
    Held(Value),
}

/// Take one op from link `link` of `who` (`gate`: what it may call).
pub(crate) async fn take<T: Cell>(
    h: &Handle<T>,
    gate: &crate::link::Gate,
    who: &crate::By,
    link: u64,
    raw: &Value,
) -> Result<Taken, String> {
    let Some(op) = parse(raw) else {
        let named = raw["id"].as_str().filter(|i| !i.is_empty()).zip(raw["cell"].as_str());
        let why = "malformed sync op — this server cannot accept it (an op needs string id/cell/action, an [number, number, string] hlc, and an action that is not framework-internal)";
        return Ok(Taken::Said(named.map(|(id, cell)| rejected(id, cell, why.into(), None))));
    };
    let Some(method) = method_of::<T>(op.cell, op.action) else {
        let why = format!("unknown cell \"{}\" — this server does not sync it", op.cell);
        return Ok(Taken::Said(Some(rejected(op.id, op.cell, why, None))));
    };
    if let Some(why) = held(h) {
        return Ok(Taken::Held(enc("sync-err", json!({ "reason": why }))));
    }
    let now = crate::store::now_ms();
    let ahead = op.hlc.0 - now;
    if ahead > MAX_DRIFT_MS {
        let why = format!(
            "clock drift: this change is stamped {}s ahead of the server (limit {}s). Correct this device's system clock (turn on automatic time sync) and the change can be made again.",
            (ahead + 500) / 1000,
            MAX_DRIFT_MS / 1000
        );
        return Ok(Taken::Said(Some(rejected(op.id, op.cell, why, Some(now)))));
    }
    let id = op_id(op.id, op.hlc.0);
    let (hh, gate, by, args) = (h.clone(), gate.clone(), who.clone(), raw["payload"]["args"].clone());
    let args = if args.is_array() { args } else { json!([]) };
    let stale = now - op.hlc.0 > crate::cx::Op::MAX_AGE_MS - MAX_DRIFT_MS;
    let ran = tokio::task::spawn_blocking(move || {
        let known = hh.op_seq(&id);
        if known.is_none() && stale {
            return None;
        }
        let r = match known {
            Some(_) => Ok(None),
            None => gate.call(&hh, &by, &method, args, Some(&id)),
        };
        Some(r.map(|v| (v.is_some(), hh.op_seq(&id))))
    });
    let (new, seq) = match ran.await.map_err(|e| e.to_string())? {
        None => {
            let why = format!(
                "{STALE}: this change is stamped {} days ago, older than this server keeps the record that tells a resend from a new change; it is refused rather than maybe applied twice, and the change can be made again.",
                (now - op.hlc.0) / 86_400_000
            );
            return Ok(Taken::Said(Some(rejected(op.id, op.cell, why, Some(now)))));
        }
        Some(Err(e)) => {
            if let Some(why) = held(h) {
                return Ok(Taken::Held(enc("sync-err", json!({ "reason": why }))));
            }
            return Ok(Taken::Said(Some(rejected(op.id, op.cell, e.why, None))));
        }
        Some(Ok(r)) => r,
    };
    let server = {
        let mut c = h.aio_sync().clock.lock().unwrap_or_else(|e| e.into_inner());
        c.receive(op.hlc, now);
        c.tick(crate::store::now_ms())
    };
    let mut ack = json!({ "cell": op.cell, "opId": op.id, "serverHlc": server, "serverNow": crate::store::now_ms() });
    if let Some(s) = seq {
        ack["serverTs"] = s.into();
    }
    if new {
        let mut frame =
            json!({ "id": op.id, "hlc": raw["hlc"], "cell": op.cell, "action": op.action, "payload": raw["payload"] });
        if let Some(s) = seq {
            frame["serverTs"] = s.into();
        }
        _ = h.aio_sync().ops.send(Passed { from: link, cell: op.cell.into(), frame: enc("op", frame) });
    }
    Ok(Taken::Said(Some(enc("sync-ack", ack))))
}

/// A `sync-req`'s cursor of one cell, as aio checks it: `lastHlc` a stamp or
/// null, `lastServerTs` a number if there; its `lastServerTs`.
fn cursor(v: &Value) -> Result<Option<f64>, ()> {
    let o = v.as_object().ok_or(())?;
    let hlc_ok = match o.get("lastHlc") {
        None | Some(Value::Null) => true,
        Some(Value::Array(h)) => h.len() == 3 && h[0].is_number() && h[1].is_number() && h[2].is_string(),
        _ => false,
    };
    match o.get("lastServerTs") {
        _ if !hlc_ok => Err(()),
        None => Ok(None),
        Some(t) => t.as_f64().filter(|t| t.is_finite()).map(Some).ok_or(()),
    }
}

/// aio's `sync-req` (pure): its cells and their cursors, or why it is refused.
fn request(d: &Value) -> Result<Vec<(String, Option<f64>)>, String> {
    let shape = "invalid sync request envelope — `clientId` must be a non-empty string, `cells` an object and `pendingOps` an array";
    if d["clientId"].as_str().is_none_or(str::is_empty)
        || !(d["cells"].is_null() || d["cells"].is_object())
        || !(d["pendingOps"].is_null() || d["pendingOps"].is_array())
    {
        return Err(shape.into());
    }
    let empty = serde_json::Map::new();
    d["cells"].as_object().unwrap_or(&empty).iter().map(|(c, v)| {
        cursor(v).map(|t| (c.clone(), t)).map_err(|()| {
            format!("invalid cursor for cell \"{c}\" — a cell entry must be {{ lastHlc: [number, number, string] | null, lastServerTs?: number }}")
        })
    }).collect()
}

/// Answer a `sync-req` of link `link` (`seer`: who it is, with the role it
/// has now): each frame to send, in order.
pub(crate) async fn answer<T: Cell>(
    h: &Handle<T>,
    gate: &crate::link::Gate,
    seer: &crate::Caller,
    link: u64,
    d: &Value,
) -> Result<Vec<Value>, String> {
    let asked = match request(d) {
        Ok(a) => a,
        Err(why) => return Ok(vec![enc("sync-err", json!({ "reason": why }))]),
    };
    let pending = d["pendingOps"].as_array().map_or(&[][..], Vec::as_slice);
    if !pending.is_empty()
        && let Some(why) = held(h)
    {
        return Ok(vec![enc("sync-err", json!({ "reason": why }))]);
    }
    let mut out = Vec::new();
    for op in pending {
        match take(h, gate, &seer.by, link, op).await? {
            Taken::Said(f) => out.extend(f),
            // The rest waits for the page's next ask.
            Taken::Held(f) => {
                out.push(f);
                return Ok(out);
            }
        }
    }
    let view = View::of(h, seer);
    let shown = cells::<T>(&view.shown);
    let (mut snapshot, mut at, mut reset) = (serde_json::Map::new(), serde_json::Map::new(), Vec::new());
    for (cell, last) in asked {
        let Some(s) = shown.get(&cell) else { continue };
        // A cursor this cell never gave: the page synced with another history.
        if last.is_some_and(|t| t > view.seq as f64) {
            reset.push(cell.clone());
        }
        snapshot.insert(cell.clone(), s.clone());
        at.insert(cell, view.seq.into());
    }
    let mut res = json!({ "ops": [], "lowWater": {}, "lastServerTs": at, "serverNow": crate::store::now_ms() });
    if !snapshot.is_empty() {
        res["mode"] = "snapshot".into();
        res["snapshot"] = snapshot.into();
    } else {
        res["mode"] = "incremental".into();
    }
    if !reset.is_empty() {
        res["reset"] = reset.into();
    }
    if d["reqId"].as_f64().is_some_and(f64::is_finite) {
        res["reqId"] = d["reqId"].clone();
    }
    out.push(enc("sync-res", res));
    Ok(out)
}

/// Whether `seer` sees all of `cell` (an op of it may go to them).
pub(crate) fn sees_whole<T: Cell>(h: &Handle<T>, seer: &crate::Caller, cell: &str) -> bool {
    let (_, st) = h.snapshot();
    let all = cells::<T>(&serde_json::to_value(&*st).unwrap_or(Value::Null));
    let theirs = cells::<T>(&st.project(seer));
    all.get(cell).is_some() && all.get(cell) == theirs.get(cell)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_runs_as_aios() {
        let mut c = Hlc::default();
        assert_eq!(c.tick(100), json!([100, 0, "server"]));
        assert_eq!(c.tick(100), json!([100, 1, "server"]));
        assert_eq!(c.tick(90), json!([100, 2, "server"]), "a clock that went back doesn't move it back");
        c.receive((150, 4), 120);
        assert_eq!(c, Hlc { physical: 150, counter: 5 }, "a page ahead moves it on");
        c.receive((150, 9), 120);
        assert_eq!(c.counter, 10);
        c.receive((150 + MAX_DRIFT_MS + 200, 0), 120);
        assert_eq!(c.physical, 150, "too far off is ignored");
        c.receive((150, -1), 120);
        assert_eq!(c.counter, 10, "not a stamp");
        c.receive((10, 0), 200);
        assert_eq!(c, Hlc { physical: 200, counter: 0 });
    }

    #[test]
    fn ops_are_checked_as_aio_checks_them() {
        let ok = json!({ "id": "c-s-1.ab", "cell": "notes", "action": "add", "payload": { "args": [1] }, "hlc": [5.0, 2, "c"] });
        assert_eq!(parse(&ok), Some(SyncOp { id: "c-s-1.ab", cell: "notes", action: "add", hlc: (5, 2) }));
        for (k, v) in [
            ("id", json!("")),
            ("id", json!(1)),
            ("cell", json!("__proto__")),
            ("action", json!("constructor")),
            ("action", json!("__set")),
            ("hlc", json!([1, 2])),
            ("hlc", json!([1, 2, 3])),
            ("hlc", json!(["1", 2, "c"])),
        ] {
            let mut bad = ok.clone();
            bad[k] = v;
            assert_eq!(parse(&bad), None, "{bad}");
        }
    }

    #[test]
    fn an_op_is_one_call_id_however_often_it_is_sent() {
        let a = op_id("c-s-1.ab", 1_700_000_000_000);
        assert_eq!(a, op_id("c-s-1.ab", 1_700_000_000_000));
        assert_ne!(a, op_id("c-s-2.ab", 1_700_000_000_000));
        let parsed = crate::cx::Op::parse(&a).expect("a UUIDv7");
        assert_eq!(parsed.ms(), 1_700_000_000_000, "at the op's stamp");
    }

    #[test]
    fn requests_are_checked_as_aio_checks_them() {
        let r = request(
            &json!({ "clientId": "c", "cells": { "a": { "lastHlc": null }, "b": { "lastHlc": [1, 0, "c"], "lastServerTs": 7 } }, "pendingOps": [] }),
        );
        assert_eq!(r, Ok(vec![("a".into(), None), ("b".into(), Some(7.0))]));
        assert_eq!(request(&json!({ "clientId": "c" })), Ok(vec![]));
        for bad in [
            json!({ "clientId": "" }),
            json!({ "cells": {} }),
            json!({ "clientId": "c", "cells": [] }),
            json!({ "clientId": "c", "pendingOps": {} }),
        ] {
            assert!(request(&bad).unwrap_err().contains("envelope"), "{bad}");
        }
        for bad in [json!(null), json!({ "lastHlc": [1] }), json!({ "lastServerTs": "7" })] {
            let e = request(&json!({ "clientId": "c", "cells": { "a": bad } })).unwrap_err();
            assert!(e.contains("cursor for cell \"a\""), "{e}");
        }
    }
}

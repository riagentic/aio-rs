//! aio's protocol v3 on `/ws` (`protocol/envelope.ts`, `server/server-ws.ts`),
//! so aio pages and tools can use an aio-rs app. Each frame is `{"v":2,"t","d"?}`.
//! The server speaks first: `proto`, `state {<cell>: view}`, `boot`; then
//! `patches` (Immer ops, `path[0]` = the cell) after each change. A client
//! calls with `action {type:"<cell>:<method>", payload:{args}, cid}` and
//! gets `ack {cid, ok, value | error}`. A server fn ([`crate::fns`]) is
//! `sfn {cid, ns, name, args: [..]}`, answered by `sfnr {cid, ok, value |
//! error, code?}` when it is done (calls run side by side); meanwhile `sfnh
//! {cid, hold}` says a person is at a picker (`deciding::start`). `vitals-ping
//! {t1}` is answered `vitals-pong {t1, t2}` (the page's watchdog); `cdiag
//! {name, kind: down | up, …}` goes to the app's health while the page is
//! linked. A dev build writes a page's forwarded console (`log {level,
//! msg}`) into the app's log as `[page]` lines.
//!
//! Time travel (aio's panel): `tt-cmd {cmd}` moves the cell (`undo`, `redo`,
//! `pause`, `resume`, `goto:<seq>`) and `tt-state {entries: [{id, type, ts,
//! perf?}], index, paused}` says where it is, to a link that drives it or
//! said `sub` (`unsub`: no more), after each change, and once to a UI at
//! its `type {kind: browser | electron}`. Only the app's own tools (the
//! local door) and an `admin` see or move it, as aio's admin bar.
//!
//! The cells a page sees are the app's cell, and with `#[aio_cell]` fields each
//! of them as the aio cell it is (`#[aio_cell] settings: Settings` is aio cell
//! `settings`), the app's cell keeping its other fields ([`cells`]). Each
//! is subscribed, patched, snapshotted and called on its own: `settings:m`
//! calls the app cell's method `settings_m`, or `m` when it has none
//! ([`super::data::method_of`]). A path inside a cell subscribes the whole cell.
//!
//! aio's offline CRDT sync (`op`, `sync-req`) is [`super::crdt`].
use crate::link::{Sock, View, next_ax, send_ax};
use crate::patch::{Op, Step};
use crate::{By, Cell, Handle};
use serde_json::{Value, json};
use std::sync::OnceLock;
use tokio::sync::broadcast::error::RecvError;

pub const VERSION: u64 = 3;
pub const MIN: u64 = 3;
const MISMATCH: u16 = 4505;
const MAX_SUBS: usize = 1024;

fn enc(t: &str, d: Value) -> Value {
    json!({ "v": 2, "t": t, "d": d })
}

/// This process's boot id: a client that sees it change reloads.
fn boot_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| crate::uuid_v4().to_string())
}

/// aio's `negotiateProtocol`: the smaller `v`, which both `min`s must allow.
pub fn negotiate(theirs: &Value) -> Result<u64, String> {
    let (Some(v), Some(min)) = (theirs["v"].as_u64(), theirs["min"].as_u64()) else {
        return Err("malformed proto hello".into());
    };
    if min < 1 || min > v {
        return Err("malformed proto hello".into());
    }
    let effective = v.min(VERSION);
    if effective < MIN {
        return Err(format!("peer speaks protocol v{v} but this side requires ≥ v{MIN}"));
    }
    if effective < min {
        return Err(format!("this side speaks protocol v{VERSION} but the peer requires ≥ v{min}"));
    }
    Ok(effective)
}

/// The cells a `subs` list covers: `None` = all (`"*"`); refused when not a
/// list of 1..=1024 strings.
fn parse_subs(raw: &Value) -> Result<Option<Vec<String>>, ()> {
    let list = raw.as_array().ok_or(())?;
    if list.is_empty() || list.len() > MAX_SUBS {
        return Err(());
    }
    let paths: Vec<&str> = list.iter().map(Value::as_str).collect::<Option<_>>().ok_or(())?;
    if paths.contains(&"*") {
        return Ok(None);
    }
    let mut cells: Vec<String> = paths.iter().map(|p| p.split('.').next().unwrap_or_default().to_string()).collect();
    cells.dedup();
    Ok(Some(cells))
}

/// An `sfn` frame's `cid` and the server fn it names (`ns.name`); `None`
/// when malformed (aio drops it).
fn sfn(d: &Value) -> Option<(String, String)> {
    let (cid, ns, name) = (d["cid"].as_str()?, d["ns"].as_str()?, d["name"].as_str()?);
    d["args"].is_array().then(|| (cid.into(), format!("{ns}.{name}")))
}

/// aio's `parseTTCommand`: a `tt-cmd` as the step it asks; `None` when it
/// isn't one (aio ignores it).
fn tt_ask(cmd: &str) -> Option<crate::TimeAsk> {
    use crate::TimeAsk;
    match cmd {
        "undo" => Some(TimeAsk::Undo),
        "redo" => Some(TimeAsk::Redo),
        "pause" => Some(TimeAsk::Pause),
        "resume" => Some(TimeAsk::Resume),
        _ => {
            let n = cmd.strip_prefix("goto:")?;
            n.bytes()
                .all(|b| b.is_ascii_digit())
                .then(|| n.parse().ok())
                .flatten()
                .filter(|n| *n < 1_000_000)
                .map(TimeAsk::Goto)
        }
    }
}

/// aio's `tt-state` of `cell`'s timeline (pure): each event as an entry
/// (`id` its seq, the `goto:` it takes), `index` the one the cell is at
/// (-1: none shown).
fn tt_state(cell: &str, tl: &crate::Timeline) -> Value {
    let entries: Vec<Value> = tl
        .steps
        .iter()
        .map(|m| {
            let mut e = json!({ "id": m.seq, "type": format!("{cell}:{}", m.method), "ts": m.at });
            if let Some(d) = m.took {
                e["perf"] = (d.as_secs_f64() * 1000.0).into();
            }
            e
        })
        .collect();
    let index = tl.steps.iter().position(|m| m.seq == tl.at).map_or(-1, |i| i as i64);
    enc("tt-state", json!({ "entries": entries, "index": index, "paused": tl.paused }))
}

/// A page's forwarded console entry (`log {level, msg}`) as a log line of
/// `who`'s (pure): its level, its text (cut and redacted); `None` without one.
fn page_line(who: &str, d: &Value) -> Option<(crate::log::Level, String)> {
    use crate::log::Level;
    let msg = d["msg"].as_str().filter(|m| !m.is_empty())?;
    let level = match d["level"].as_str() {
        Some("error") => Level::Error,
        Some("warn") => Level::Warn,
        Some("debug") => Level::Debug,
        _ => Level::Info,
    };
    let msg: String = msg.chars().take(2000).collect();
    Some((level, format!("{who}: {}", crate::report::redact(&msg))))
}

/// The cells an aio page sees in `shown`, a view of `T` (pure): `T` under
/// its id, and each `#[aio_cell]` field instead as the aio cell it is.
pub(super) fn cells<T: Cell>(shown: &Value) -> Value {
    if T::AIO_CELLS.is_empty() {
        return json!({ T::ID: shown });
    }
    let mut own = shown.as_object().cloned().unwrap_or_default();
    let mut out: serde_json::Map<String, Value> =
        T::AIO_CELLS.iter().filter_map(|s| Some((s.cell.to_string(), own.remove(s.field)?))).collect();
    out.insert(T::ID.into(), own.into());
    out.into()
}

/// The cells `subs` shows of `all` (`None`: all of them).
fn shown(all: Value, subs: &Option<Vec<String>>) -> Value {
    match (all, subs) {
        (Value::Object(m), Some(s)) => {
            m.into_iter().filter(|(c, _)| s.contains(c)).collect::<serde_json::Map<_, _>>().into()
        }
        (all, _) => all,
    }
}

/// The cell an op of [`cells`]'s diff is in.
fn cell_of(op: &Op) -> Option<&str> {
    let (Op::Set { path, .. } | Op::Del { path } | Op::Insert { path, .. } | Op::Append { path, .. }) = op;
    match path.first() {
        Some(Step::Key(k)) => Some(k),
        _ => None,
    }
}

/// aio-rs patch ops on [`cells`] as aio's wire ops (`path[0]`: the cell).
fn wire_ops(ops: &[Op]) -> Value {
    let path = |p: &[Step]| -> Vec<Value> {
        p.iter()
            .map(|s| match s {
                Step::Key(k) => json!(k),
                Step::Index(i) => json!(i),
            })
            .collect()
    };
    ops.iter()
        .map(|op| match op {
            Op::Set { path: p, value } => {
                json!({ "op": "replace", "path": path(p), "value": value })
            }
            // At an index, both splice (Immer's array ops).
            Op::Del { path: p } => json!({ "op": "remove", "path": path(p) }),
            Op::Insert { path: p, value } => json!({ "op": "add", "path": path(p), "value": value }),
            // Protocol v3's `append` (why aio needs v3).
            Op::Append { path: p, value } => json!({ "op": "append", "path": path(p), "value": value }),
        })
        .collect()
}

/// Send what changed since `view` in the cells `subs` shows; `view` is now.
async fn catch_up<T: Cell>(
    w: &mut Sock,
    view: &mut View,
    h: &Handle<T>,
    viewer: &crate::Caller,
    subs: &Option<Vec<String>>,
    pushes: &mut crate::budgets::Pushes,
) -> Result<(), String> {
    let now = View::of(h, viewer);
    let mut ops = crate::patch::diff(&cells::<T>(&view.shown), &cells::<T>(&now.shown));
    *view = now;
    ops.retain(|op| subs.as_ref().is_none_or(|s| cell_of(op).is_some_and(|c| s.iter().any(|x| x == c))));
    if !ops.is_empty() {
        let msg = enc("patches", wire_ops(&ops));
        pushes.push(&msg, crate::store::now_ms());
        send_ax(w, &msg).await?;
    }
    Ok(())
}

/// The next op another link passed on (`None`: one it missed); pending
/// until the link syncs.
async fn next_passed(
    rx: &mut Option<tokio::sync::broadcast::Receiver<super::crdt::Passed>>,
) -> Option<super::crdt::Passed> {
    match rx {
        Some(r) => match r.recv().await {
            Ok(p) => Some(p),
            // A page that missed one catches up by its state frames.
            Err(RecvError::Lagged(_)) => None,
            Err(RecvError::Closed) => std::future::pending().await,
        },
        None => std::future::pending().await,
    }
}

/// `ask` time travel of `h` (`None`: only where it is), as `tt-state`.
async fn travel<T: Cell>(h: &Handle<T>, ask: Option<crate::TimeAsk>) -> Result<Value, String> {
    let h = h.clone();
    let tl = tokio::task::spawn_blocking(move || h.travel(ask)).await.map_err(|e| e.to_string())?;
    tl.map(|tl| tt_state(T::ID, &tl)).map_err(|e| e.why)
}

/// One aio client, until it leaves.
/// `gate`: what it may call. `signed`: the user its session proved (else
/// everyone is `peer`). `link`: its id and who it is, for health.
pub async fn session<T: Cell>(
    h: Handle<T>,
    mut w: Sock,
    limits: crate::sync::Limits,
    gate: crate::link::Gate,
    signed: Option<By>,
    calls: crate::fns::Served,
    (link, who): (u64, String),
) -> Result<(), String> {
    // What the page says is degraded on its side (`cdiag`), by name.
    let mut down = std::collections::BTreeMap::<String, Value>::new();
    let viewer = signed.unwrap_or_else(|| By::User("peer".into()));
    // What it sees goes by its role too, read again on every account change
    // (heard before the read: no change between them is missed).
    let mut accounts = match &gate {
        crate::link::Gate::Signed(a, _) => Some(a.changes()),
        _ => None,
    };
    let role = |g: &crate::link::Gate| {
        let g = g.clone();
        tokio::task::spawn_blocking(move || g.role())
    };
    let mut seer = crate::Caller { by: viewer.clone(), role: role(&gate).await.map_err(|e| e.to_string())? };
    let mut events = h.events();
    let mut view = View::of(&h, &seer);
    let mut subs: Option<Vec<String>> = None;
    let state = |view: &View, subs: &Option<Vec<String>>| json!({ "v": 2, "t": "state", "d": shown(cells::<T>(&view.shown), subs), "s": subs });
    let hello = json!({ "v": VERSION, "min": MIN, "ver": crate::BUILT_BY, "maxMessageBytes": limits.max_message, "rate": limits.calls_per_sec, "cfg": false, "appId": T::ID, "user": null });
    send_ax(&mut w, &enc("proto", hello)).await?;
    let mut pushes = crate::budgets::Pushes::default();
    send_ax(&mut w, &state(&view, &subs)).await?;
    send_ax(&mut w, &enc("boot", json!({ "id": boot_id() }))).await?;
    let h = h.by(viewer.clone());
    let mut pulse = crate::link::Pulse::new();
    pulse.link = Some((link, who.clone()));
    let mut pace = crate::link::Pace::new(limits.calls_per_sec);
    let mut bytes = crate::link::Pace::new(limits.bytes_per_sec);
    let mut running = tokio::task::JoinSet::new();
    let (held_tx, mut held) = tokio::sync::mpsc::unbounded_channel();
    // An async method's ack, once its end comes (aio acks on completion).
    let (ends_tx, mut ends) = tokio::sync::mpsc::unbounded_channel::<Value>();
    // Time travel: shown after each change (`sub`; unsaid until it says or
    // drives it), to whom it is the operator's.
    let (mut tt_sub, mut greeted) = (None::<bool>, false);
    // Other links' ops, once it syncs (its first `sync-req`).
    let mut passed: Option<tokio::sync::broadcast::Receiver<super::crdt::Passed>> = None;
    let travels = |role: &Option<String>| matches!(gate, crate::link::Gate::Door) || role.as_deref() == Some("admin");
    loop {
        tokio::select! {
            Some(Ok(answer)) = running.join_next(), if !running.is_empty() => send_ax(&mut w, &answer).await?,
            Some(m) = held.recv() => send_ax(&mut w, &m).await?,
            Some(ack) = ends.recv() => {
                while events.try_recv().is_ok() {}
                catch_up(&mut w, &mut view, &h, &seer, &subs, &mut pushes).await?;
                send_ax(&mut w, &enc("ack", ack)).await?;
            }
            // Another link's op, to a link that syncs and sees its whole cell.
            p = next_passed(&mut passed) => if let Some(p) = p
                && p.from != link
                && super::crdt::sees_whole(&h, &seer, &p.cell)
            {
                send_ax(&mut w, &p.frame).await?;
            },
            more = crate::link::changed(&mut accounts) => {
                if !more {
                    accounts = None;
                }
                let now = role(&gate).await.map_err(|e| e.to_string())?;
                // A new role sees another view: sent whole.
                if now != seer.role {
                    seer.role = now;
                    view = View::of(&h, &seer);
                    send_ax(&mut w, &state(&view, &subs)).await?;
                }
            }
            ev = events.recv() => match ev {
                Ok(_) | Err(RecvError::Lagged(_)) => {
                    catch_up(&mut w, &mut view, &h, &seer, &subs, &mut pushes).await?;
                    if tt_sub == Some(true) && travels(&seer.role) && let Ok(tt) = travel(&h, None).await {
                        send_ax(&mut w, &tt).await?;
                    }
                }
                Err(RecvError::Closed) => return Ok(()),
            },
            msg = next_ax(&mut w, &mut pulse, limits.max_message, &mut bytes) => {
                let Some(msg) = msg else { return Ok(()) };
                if matches!(msg["t"].as_str(), Some("action" | "sfn" | "op" | "sync-req")) {
                    pace.take().await;
                }
                if msg["v"] != 2 {
                    continue;
                }
                let d = &msg["d"];
                match msg["t"].as_str().unwrap_or_default() {
                    "proto" => if let Err(reason) = negotiate(d) {
                        _ = send_ax(&mut w, &enc("proto-err", json!({ "reason": reason }))).await;
                        _ = w.send(crate::link::Msg::Close(MISMATCH, format!("__proto-err:{reason}").chars().take(120).collect())).await;
                        return Err(format!("aio client refused: {reason}"));
                    },
                    "subs" => match parse_subs(&d["subs"]) {
                        Ok(s) => {
                            subs = s;
                            send_ax(&mut w, &state(&view, &subs)).await?;
                        }
                        Err(()) => {
                            let diag = json!({ "type": "subs-refused", "severity": "warning", "source": "server-ws", "message": "this page's subscription set was refused by the server", "hint": "send 1 to 1024 cell names or paths, or [\"*\"]" });
                            send_ax(&mut w, &enc("diag", diag)).await?;
                        }
                    },
                    "resync" => send_ax(&mut w, &state(&view, &subs)).await?,
                    // A UI is greeted with where time travel is (its panel binds on it).
                    "type" if matches!(d["kind"].as_str(), Some("browser" | "electron")) => {
                        if !std::mem::replace(&mut greeted, true) && travels(&seer.role) && let Ok(tt) = travel(&h, None).await {
                            send_ax(&mut w, &tt).await?;
                        }
                    }
                    "tt-cmd" => match d["cmd"].as_str().unwrap_or_default() {
                        sub @ ("sub" | "unsub") => {
                            tt_sub = Some(sub == "sub");
                            if sub == "sub" && travels(&seer.role) && let Ok(tt) = travel(&h, None).await {
                                send_ax(&mut w, &tt).await?;
                            }
                        }
                        cmd if !travels(&seer.role) => {
                            eprintln!("[aio] aio page: time travel {cmd:?} refused for {who}: it moves the state for everyone (the local door's tools and admins only)");
                        }
                        cmd => match tt_ask(cmd) {
                            None => eprintln!("[aio] aio page: unknown time travel {cmd:?} dropped"),
                            Some(ask) => {
                                // A link that drives it is shown what it did, unless it said otherwise.
                                tt_sub.get_or_insert(true);
                                let r = travel(&h, Some(ask)).await;
                                // Its change first, then where the cell is (as an ack).
                                while events.try_recv().is_ok() {}
                                catch_up(&mut w, &mut view, &h, &seer, &subs, &mut pushes).await?;
                                match r {
                                    Ok(tt) => send_ax(&mut w, &tt).await?,
                                    Err(e) => eprintln!("[aio] aio page: time travel {cmd:?}: {e}"),
                                }
                            }
                        },
                    },
                    "log" if cfg!(debug_assertions) => if let Some((level, line)) = page_line(&who, d) {
                        crate::log::emit(level, module_path!(), Some("page"), format_args!("{line}"), &[]);
                    },
                    // The page's watchdog judges the link by this answer.
                    "vitals-ping" if d["t1"].is_number() => {
                        send_ax(&mut w, &enc("vitals-pong", json!({ "t1": d["t1"], "t2": crate::store::now_ms() }))).await?;
                    }
                    "cdiag" => if let Some(name) = d["name"].as_str().filter(|n| !n.is_empty()) {
                        match d["kind"].as_str() {
                            Some("down") => _ = down.insert(name.into(), d.clone()),
                            Some("up") => _ = down.remove(name),
                            _ => continue,
                        }
                        crate::health::client_says(link, &who, &Value::Array(down.values().cloned().collect()));
                    },
                    "sfn" => match sfn(d) {
                        None => eprintln!("[aio] aio page: an invalid sfn frame was dropped"),
                        Some((cid, name)) => {
                            let (held, c) = (held_tx.clone(), cid.clone());
                            let tell = move |hold| _ = held.send(enc("sfnh", json!({ "cid": c, "hold": hold })));
                            calls.start(&mut running, name, d["args"].clone(), Default::default(), tell, move |r| {
                            let d = match r {
                                Ok(v) => json!({ "cid": cid, "ok": true, "value": v }),
                                Err(f) => json!({ "cid": cid, "ok": false, "error": f.why, "code": f.code }),
                            };
                            enc("sfnr", d)
                        })
                        }
                    },
                    // aio's CRDT sync: its change first, then what the op says.
                    "op" => {
                        let said = match super::crdt::take(&h, &gate, &viewer, link, d).await? {
                            super::crdt::Taken::Said(f) => f,
                            super::crdt::Taken::Held(f) => Some(f),
                        };
                        while events.try_recv().is_ok() {}
                        catch_up(&mut w, &mut view, &h, &seer, &subs, &mut pushes).await?;
                        if let Some(f) = said {
                            send_ax(&mut w, &f).await?;
                        }
                    }
                    "sync-req" => {
                        passed.get_or_insert_with(|| h.aio_sync().ops.subscribe());
                        let frames = super::crdt::answer(&h, &gate, &seer, link, d).await?;
                        while events.try_recv().is_ok() {}
                        catch_up(&mut w, &mut view, &h, &seer, &subs, &mut pushes).await?;
                        for f in frames {
                            send_ax(&mut w, &f).await?;
                        }
                    }
                    "action" => {
                        let cid = d["cid"].clone();
                        let kind = d["type"].as_str().unwrap_or_default();
                        let method = kind.split_once(':').and_then(|(c, m)| super::data::method_of::<T>(c, m));
                        let is_async = method.as_deref().is_some_and(|m| T::async_mode(m).is_some());
                        let r = match method {
                            None => Err(crate::Rejected::coded(crate::Code::ActionRefused, format!("unknown action {kind}"))),
                            Some(m) => {
                                let (hh, gate, by) = (h.clone(), gate.clone(), viewer.clone());
                                let args = d["payload"]["args"].clone();
                                let args = if args.is_null() { json!([]) } else { args };
                                let r = tokio::task::spawn_blocking(move || gate.ask(&hh, &by, &m, args, None));
                                r.await.map_err(|e| e.to_string())?
                            }
                        };
                        if cid.is_null() {
                            continue;
                        }
                        // An async method is acked on its end, with its answer.
                        let r = match r {
                            Ok((_, answer)) if is_async => {
                                let (tx, closing) = (ends_tx.clone(), h.clone());
                                tokio::spawn(async move {
                                    let end = answer.await;
                                    _ = tx.send(ack_of(cid, end.map(Some), closing.closing()));
                                });
                                continue;
                            }
                            Ok((v, _)) => Ok(v),
                            Err(e) => Err(e),
                        };
                        // Its change first, then the ack (as aio).
                        while events.try_recv().is_ok() {}
                        catch_up(&mut w, &mut view, &h, &seer, &subs, &mut pushes).await?;
                        send_ax(&mut w, &enc("ack", ack_of(cid, r, h.closing()))).await?;
                    }
                    // "x" is reserved and ignored; other kinds aren't ours to answer.
                    _ => {}
                }
            }
        }
    }
}

/// An `action`'s ack: its answer, or its refusal (aio's `code` only when
/// aio made it; while closing, a `retryAfterMs` so the page sends it again).
fn ack_of(cid: Value, r: crate::Result<Option<Value>>, closing: bool) -> Value {
    match r {
        Ok(v) => json!({ "cid": cid, "ok": true, "value": v.unwrap_or(Value::Null) }),
        Err(e) if closing => json!({ "cid": cid, "ok": false, "error": e.why, "retryAfterMs": crate::RETRY_AFTER_MS }),
        Err(e) => match e.code {
            Some(c) => json!({ "cid": cid, "ok": false, "error": e.why, "code": c }),
            None => json!({ "cid": cid, "ok": false, "error": e.why }),
        },
    }
}

/// aio's patch ops (Immer's `add`, `replace`, `remove`, and v3's string
/// `append`; `path[0]` is the cell) applied to `cells` (pure). An op that
/// doesn't fit (a missing parent, an index past the end, `__proto__`) is
/// an error: the caller takes the full state instead of guessing.
pub(crate) fn immer(cells: &mut Value, ops: &Value) -> Result<(), String> {
    for op in ops.as_array().ok_or("patches are not a list")? {
        let path = op["path"].as_array().filter(|p| !p.is_empty()).ok_or("an op without a path")?;
        if path.iter().any(|s| s == "__proto__") {
            return Err("a path through __proto__".into());
        }
        let (last, parents) = path.split_last().ok_or("an op without a path")?;
        let mut at = &mut *cells;
        for s in parents {
            at = match (s, at) {
                (Value::String(k), Value::Object(m)) => m.get_mut(k),
                (s, Value::Array(a)) => index(s).and_then(|i| a.get_mut(i)),
                _ => None,
            }
            .ok_or_else(|| format!("no {s} to patch"))?;
        }
        let value = op.get("value").cloned().unwrap_or(Value::Null);
        match (op["op"].as_str().unwrap_or_default(), at, last) {
            ("add" | "replace", Value::Object(m), Value::String(k)) => _ = m.insert(k.clone(), value),
            ("remove", Value::Object(m), Value::String(k)) => {
                _ = m.remove(k).ok_or_else(|| format!("no {k} to remove"))?
            }
            ("append", Value::Object(m), Value::String(k)) => match (m.get_mut(k), value) {
                (Some(Value::String(s)), Value::String(more)) => s.push_str(&more),
                _ => return Err(format!("append to {k}, not a string")),
            },
            ("replace", Value::Array(a), Value::String(k)) if k == "length" => {
                a.resize(value.as_u64().ok_or("a length that is not a number")? as usize, Value::Null)
            }
            (kind, Value::Array(a), s) => {
                let i = index(s).filter(|i| *i <= a.len()).ok_or_else(|| format!("no index {s}"))?;
                match kind {
                    "add" => a.insert(i, value),
                    "replace" if i == a.len() => a.push(value),
                    "replace" => a[i] = value,
                    "remove" if i < a.len() => _ = a.remove(i),
                    "append" => match (a.get_mut(i), value) {
                        (Some(Value::String(s)), Value::String(more)) => s.push_str(&more),
                        _ => return Err(format!("append to [{i}], not a string")),
                    },
                    _ => return Err(format!("{kind} at [{i}] doesn't fit")),
                }
            }
            (kind, _, s) => return Err(format!("{kind} at {s} doesn't fit")),
        }
    }
    Ok(())
}

/// An array index as aio writes one: a number, or a string of digits.
fn index(s: &Value) -> Option<usize> {
    s.as_u64().map(|i| i as usize).or_else(|| s.as_str()?.parse().ok())
}

/// An aio-rs app's link to an aio app (protocol v3, as aio's own pages and tools
/// speak it): the cells it shows as JSON, kept current from aio's patches,
/// and calls (`<cell>:<method>` actions) answered by their acks. Nothing
/// is predicted: a call waits for the aio server.
pub struct AioLink {
    w: crate::sync::Wire,
    cells: Value,
    boot: String,
    cid: u64,
}

impl AioLink {
    /// Connect with `link`'s key, agree on the protocol (aio's hello), and
    /// take the first state.
    pub async fn connect(link: &crate::sync::Link) -> std::io::Result<Self> {
        let w = crate::sync::dial(link, false).await?;
        let mut l = Self { w, cells: json!({}), boot: String::new(), cid: 0 };
        let first = tokio::time::timeout(crate::sync::CALL_TIMEOUT, l.next())
            .await
            .map_err(|_| std::io::Error::other("no answer"))??;
        if first["t"] != "proto" {
            return Err(std::io::Error::other(format!("not an aio app: it said {}", first["t"])));
        }
        negotiate(&first["d"]).map_err(|e| std::io::Error::new(std::io::ErrorKind::Unsupported, e))?;
        l.send(&enc("proto", json!({ "v": VERSION, "min": MIN, "ver": crate::BUILT_BY }))).await?;
        while l.boot.is_empty() {
            let m = l.next().await?;
            l.take(&m).await?;
        }
        Ok(l)
    }

    /// The cells it shows, by name, as the aio server last sent them.
    pub fn cells(&self) -> &Value {
        &self.cells
    }

    /// Cell `cell` as a `C` (refused when it isn't shown or doesn't fit).
    pub fn cell<C: serde::de::DeserializeOwned>(&self, cell: &str) -> crate::Result<C> {
        let v =
            self.cells.get(cell).ok_or_else(|| crate::Rejected::new(format!("the aio app shows no cell {cell}")))?;
        serde_json::from_value(v.clone()).map_err(|e| crate::Rejected::new(format!("{cell}: {e}")))
    }

    /// The aio server's boot id: a new one means it restarted.
    pub fn boot(&self) -> &str {
        &self.boot
    }

    /// Show only `cells` (aio's `subs`; `["*"]`: all); the state then.
    pub async fn subscribe(&mut self, cells: &[&str]) -> crate::Result<()> {
        self.send(&enc("subs", json!({ "subs": cells }))).await.map_err(|e| crate::Rejected::new(e.to_string()))?;
        loop {
            let m = self.next().await.map_err(|e| crate::Rejected::new(e.to_string()))?;
            if m["t"] == "diag" && m["d"]["type"] == "subs-refused" {
                crate::reject!("the aio app refused the subscription: {}", m["d"]["message"].as_str().unwrap_or("?"));
            }
            if m["t"] == "state" {
                return self.take(&m).await.map(drop).map_err(|e| crate::Rejected::new(e.to_string()));
            }
            self.take(&m).await.map_err(|e| crate::Rejected::new(e.to_string()))?;
        }
    }

    /// Wait until a cell it shows changes.
    pub async fn changed(&mut self) -> std::io::Result<()> {
        loop {
            let m = self.next().await?;
            if self.take(&m).await? {
                return Ok(());
            }
        }
    }

    /// Call `cell`'s `method` with `args` (a JSON list) and wait for its
    /// ack (up to `CALL_TIMEOUT`); its change is in `cells` by then (aio
    /// sends the change before the ack).
    pub async fn call(&mut self, cell: &str, method: &str, args: Value) -> crate::Result<Value> {
        let gone = |e: std::io::Error| {
            crate::Rejected::lost(format!("{cell}.{method}: the link dropped ({e}); it may have run"))
        };
        self.cid += 1;
        let cid = format!("v2-{}", self.cid);
        let d = json!({ "type": format!("{cell}:{method}"), "payload": { "args": args }, "cid": cid });
        self.send(&enc("action", d)).await.map_err(gone)?;
        let ack = async {
            loop {
                let m = self.next().await?;
                if m["t"] == "ack" && m["d"]["cid"] == cid.as_str() {
                    return Ok(m["d"].clone());
                }
                self.take(&m).await?;
            }
        };
        let d = tokio::time::timeout(crate::sync::CALL_TIMEOUT, ack)
            .await
            .map_err(|_| {
                crate::Rejected::new(format!("{cell}.{method}: no answer in {:?}", crate::sync::CALL_TIMEOUT))
            })?
            .map_err(gone)?;
        match d["ok"].as_bool() {
            Some(true) => Ok(d["value"].clone()),
            _ => Err(crate::Rejected {
                why: d["error"].as_str().map_or_else(|| d["error"].to_string(), String::from),
                code: crate::Code::of(&d["code"]),
            }),
        }
    }

    /// Run the aio app's server fn `ns.name` with `args` (a JSON list): its
    /// value, or its refusal (with aio's `code` when it gave one). It may wait on a person (a picker) as long as it takes.
    pub async fn run(&mut self, ns: &str, name: &str, args: Value) -> crate::Result<Value> {
        let gone =
            |e: std::io::Error| crate::Rejected::lost(format!("{ns}.{name}: the link dropped ({e}); it may have run"));
        self.cid += 1;
        let cid = format!("v2-{}", self.cid);
        self.send(&enc("sfn", json!({ "cid": cid, "ns": ns, "name": name, "args": args }))).await.map_err(gone)?;
        let d = loop {
            let m = self.next().await.map_err(gone)?;
            if m["t"] == "sfnr" && m["d"]["cid"] == cid.as_str() {
                break m["d"].clone();
            }
            self.take(&m).await.map_err(gone)?;
        };
        match d["ok"].as_bool() {
            Some(true) => Ok(d["value"].clone()),
            _ => Err(crate::Rejected {
                why: d["error"].as_str().unwrap_or("?").into(),
                code: crate::Code::of(&d["code"]),
            }),
        }
    }

    async fn send(&mut self, v: &Value) -> std::io::Result<()> {
        use futures_util::SinkExt;
        self.w.send(crate::sync::frame(v)).await.map_err(std::io::Error::other)
    }

    async fn next(&mut self) -> std::io::Result<Value> {
        crate::sync::next(&mut self.w)
            .await
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "the aio app closed the link"))
    }

    /// Fold a frame into what it knows; did a cell change? A patch that
    /// doesn't fit asks for the full state (aio's `resync`), never a guess.
    async fn take(&mut self, m: &Value) -> std::io::Result<bool> {
        let d = &m["d"];
        match m["t"].as_str() {
            Some("state") if d.is_object() => {
                let changed = self.cells != *d;
                self.cells = d.clone();
                Ok(changed)
            }
            Some("patches") => {
                let mut next = self.cells.clone();
                match immer(&mut next, d) {
                    Ok(()) => {
                        self.cells = next;
                        Ok(true)
                    }
                    Err(_) => self.send(&enc("resync", Value::Null)).await.map(|_| false),
                }
            }
            Some("boot") => {
                self.boot = d["id"].as_str().unwrap_or("?").to_string();
                Ok(false)
            }
            Some("proto-err") => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                format!("the aio app refused: {}", d["reason"].as_str().unwrap_or("?")),
            )),
            _ => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_negotiate_like_aio() {
        assert_eq!(negotiate(&json!({ "v": 3, "min": 3 })), Ok(3));
        assert_eq!(negotiate(&json!({ "v": 9, "min": 2 })), Ok(3));
        assert!(negotiate(&json!({ "v": 2, "min": 1 })).unwrap_err().contains("requires ≥ v3"));
        assert!(negotiate(&json!({ "v": 9, "min": 5 })).unwrap_err().contains("peer requires ≥ v5"));
        assert!(negotiate(&json!({ "v": 3 })).is_err());
        assert!(negotiate(&json!({ "v": 3, "min": 4 })).is_err());
    }

    #[test]
    fn subs_are_checked_like_aio() {
        assert_eq!(parse_subs(&json!(["*"])), Ok(None));
        assert_eq!(parse_subs(&json!(["board", "board.notes"])), Ok(Some(vec!["board".into()])));
        assert!(parse_subs(&json!([])).is_err());
        assert!(parse_subs(&json!([1])).is_err());
        assert!(parse_subs(&json!(vec!["a"; 1025])).is_err());
    }

    #[test]
    fn sfn_frames_are_checked_like_aio() {
        let f = |d: Value| sfn(&d);
        assert_eq!(
            f(json!({ "cid": "1", "ns": "api", "name": "charge", "args": [5] })),
            Some(("1".into(), "api.charge".into()))
        );
        assert_eq!(f(json!({ "cid": "1", "ns": "api", "name": "charge", "args": 5 })), None);
        assert_eq!(f(json!({ "cid": 1, "ns": "api", "name": "charge", "args": [] })), None);
        assert_eq!(f(json!({ "cid": "1", "name": "charge", "args": [] })), None);
    }

    #[test]
    fn tt_commands_parse_like_aio() {
        use crate::TimeAsk;
        assert_eq!(tt_ask("undo"), Some(TimeAsk::Undo));
        assert_eq!(tt_ask("redo"), Some(TimeAsk::Redo));
        assert_eq!(tt_ask("pause"), Some(TimeAsk::Pause));
        assert_eq!(tt_ask("resume"), Some(TimeAsk::Resume));
        assert_eq!(tt_ask("goto:0"), Some(TimeAsk::Goto(0)));
        assert_eq!(tt_ask("goto:999999"), Some(TimeAsk::Goto(999_999)));
        for bad in ["goto:", "goto:1000000", "goto:-1", "goto:1.5", "goto:+3", "goto: 2", "jump", ""] {
            assert_eq!(tt_ask(bad), None, "{bad}");
        }
    }

    #[test]
    fn tt_state_is_aios_panel_summary() {
        use std::time::Duration;
        let m = |seq, method: &str, took| crate::Moment { seq, method: method.into(), at: 100 + seq as i64, took };
        let tl = crate::Timeline {
            at: 2,
            head: 3,
            floor: 0,
            paused: true,
            steps: vec![m(1, "add", None), m(2, "add", Some(Duration::from_micros(1500))), m(3, "$rewind", None)],
        };
        let s = tt_state("board", &tl);
        assert_eq!(s["t"], "tt-state");
        assert_eq!(
            s["d"],
            json!({ "entries": [
                { "id": 1, "type": "board:add", "ts": 101 },
                { "id": 2, "type": "board:add", "ts": 102, "perf": 1.5 },
                { "id": 3, "type": "board:$rewind", "ts": 103 },
            ], "index": 1, "paused": true })
        );
        let none = crate::Timeline { steps: vec![], ..tl };
        assert_eq!(tt_state("board", &none)["d"]["index"], -1);
    }

    #[test]
    fn a_pages_console_is_a_log_line_of_its_own() {
        use crate::log::Level;
        let l = |d: Value| page_line("ann from 127.0.0.1 (aio)", &d);
        assert_eq!(
            l(json!({ "level": "error", "msg": "boom", "ts": 1 })),
            Some((Level::Error, "ann from 127.0.0.1 (aio): boom".into()))
        );
        assert_eq!(l(json!({ "level": "warn", "msg": "w" })).unwrap().0, Level::Warn);
        assert_eq!(l(json!({ "level": "log", "msg": "x" })).unwrap().0, Level::Info);
        assert_eq!(l(json!({ "level": "error" })), None, "nothing said");
        assert_eq!(l(json!({ "msg": "y".repeat(5000) })).unwrap().1.len(), "ann from 127.0.0.1 (aio): ".len() + 2000);
    }

    #[test]
    fn ops_are_immer_ops_under_the_cell() {
        let ops = crate::patch::diff(&json!({ "c": { "a": [1, 2], "b": 1 } }), &json!({ "c": { "a": [1, 3] } }));
        assert_eq!(
            wire_ops(&ops),
            json!([{ "op": "remove", "path": ["c", "b"] }, { "op": "replace", "path": ["c", "a", 1], "value": 3 }])
        );
        let long: Vec<i32> = (0..40).collect();
        let mut more = long.clone();
        more.push(40);
        let ops = crate::patch::diff(&json!({ "c": { "a": long } }), &json!({ "c": { "a": more } }));
        assert_eq!(wire_ops(&ops), json!([{ "op": "add", "path": ["c", "a", 40], "value": 40 }]));
    }

    #[test]
    fn aios_patches_apply_as_immer_applies_them() {
        let mut s = json!({ "c": { "a": [1, 2, 3], "t": "x".repeat(3), "o": { "k": 1 } } });
        let ops = json!([
            { "op": "add", "path": ["c", "a", 1], "value": 9 },
            { "op": "remove", "path": ["c", "a", 0] },
            { "op": "replace", "path": ["c", "a", "1"], "value": 7 },
            { "op": "add", "path": ["c", "a", 3], "value": 4 },
            { "op": "append", "path": ["c", "t"], "value": "yz" },
            { "op": "add", "path": ["c", "o", "n"], "value": null },
            { "op": "remove", "path": ["c", "o", "k"] },
            { "op": "add", "path": ["d"], "value": { "new": true } },
        ]);
        immer(&mut s, &ops).unwrap();
        assert_eq!(s, json!({ "c": { "a": [9, 7, 3, 4], "t": "xxxyz", "o": { "n": null } }, "d": { "new": true } }));
        // Immer shrinks an array by its length.
        immer(&mut s, &json!([{ "op": "replace", "path": ["c", "a", "length"], "value": 1 }])).unwrap();
        assert_eq!(s["c"]["a"], json!([9]));
        // What doesn't fit is refused whole, never half-applied by a caller.
        for bad in [
            json!([{ "op": "replace", "path": ["c", "nope", "x"], "value": 1 }]),
            json!([{ "op": "add", "path": ["c", "a", 5], "value": 1 }]),
            json!([{ "op": "append", "path": ["c", "o", "n"], "value": "x" }]),
            json!([{ "op": "replace", "path": ["c", "__proto__", "x"], "value": 1 }]),
            json!([{ "op": "replace", "path": [], "value": 1 }]),
            json!({ "op": "replace" }),
        ] {
            assert!(immer(&mut s.clone(), &bad).is_err(), "{bad}");
        }
    }
}

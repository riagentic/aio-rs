//! Moving a running app through its log, over its local door (the
//! operator's: only this OS user reaches it): `ax snapshot load`, `aio
//! events --diff`, `ax replay`, `ax record` and `ax migrations` (`aio
//! timetravel` is `live::travel`). Every move is a new event (a rewind),
//! so the history is never cut and each step can be stepped back from
//! (deterministic core).
use aio::serde_json::{self, Value, json};
use aio::sync::{Link, Live};
use rusqlite::Connection;
use std::collections::BTreeMap;

type Out = Result<String, String>;

/// What the store marks a rewind (and a loaded state) with.
const REWIND: &str = "$rewind";

/// A cell's log as time travel reads it: each event's seq, and for a
/// rewind the event it went back to.
pub fn log(db: &Connection, cell: &str) -> Result<Vec<(u64, Option<u64>)>, String> {
    let mut q =
        db.prepare("SELECT seq, method, args FROM log WHERE cell = ?1 ORDER BY seq").map_err(|e| e.to_string())?;
    let rows = q
        .query_map([cell], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))
        .map_err(|e| e.to_string())?;
    rows.map(|r| {
        let (seq, method, args) = r.map_err(|e| e.to_string())?;
        let to = (method == REWIND).then(|| serde_json::from_str::<Value>(&args).ok()?["to"].as_u64()).flatten();
        Ok((seq as u64, to))
    })
    .collect()
}

pub fn ask(link: &Link, cell: &str, user: &str, msg: Value) -> Result<(Value, Value), String> {
    aio::rt().block_on(async {
        let mut l = Live::connect(link, cell, user).await.map_err(|e| e.to_string())?;
        let ok = l.ask(msg).await.map_err(|e| format!("{cell}: {e}"))?;
        Ok((ok, l.view().clone()))
    })
}

/// The whole state right after each of `seqs` and the event before each,
/// over one link to the door (`ax events --diff`).
pub fn states(link: &Link, cell: &str, user: &str, seqs: &[u64]) -> Result<BTreeMap<u64, Value>, String> {
    aio::rt().block_on(async {
        let mut l = Live::connect(link, cell, user).await.map_err(|e| e.to_string())?;
        let wanted: std::collections::BTreeSet<u64> = seqs.iter().flat_map(|s| [s.saturating_sub(1), *s]).collect();
        let mut out = BTreeMap::new();
        for s in wanted {
            let st = l.ask(json!({ "t": "at", "to": s })).await.map_err(|e| format!("{cell}: {e}"))?;
            out.insert(s, st);
        }
        Ok(out)
    })
}

/// Each leaf where `after` differs from `before`: (path, before, after),
/// `null` where there was or is none; a list of another length whole (pure).
pub fn diff(before: &Value, after: &Value, at: &str) -> Vec<(String, Value, Value)> {
    let path = |k: &str| if at.is_empty() { k.to_string() } else { format!("{at}.{k}") };
    match (before, after) {
        _ if before == after => vec![],
        (Value::Object(b), Value::Object(a)) => {
            let keys: std::collections::BTreeSet<&String> = b.keys().chain(a.keys()).collect();
            let none = Value::Null;
            keys.into_iter()
                .flat_map(|k| diff(b.get(k).unwrap_or(&none), a.get(k).unwrap_or(&none), &path(k)))
                .collect()
        }
        (Value::Array(b), Value::Array(a)) if b.len() == a.len() => {
            b.iter().zip(a).enumerate().flat_map(|(i, (b, a))| diff(b, a, &path(&i.to_string()))).collect()
        }
        _ => vec![(at.to_string(), before.clone(), after.clone())],
    }
}

/// One diff line: `path: before → after`, each side cut to 60 characters.
pub fn said((path, before, after): &(String, Value, Value)) -> String {
    let brief = |v: &Value| {
        let s = v.to_string();
        match s.char_indices().nth(57) {
            Some((i, _)) if s.chars().count() > 60 => format!("{}…", &s[..i]),
            _ => s,
        }
    };
    format!("{path}: {} → {}", brief(before), brief(after))
}

/// `ax snapshot load <app> <file>`: each cell the file holds (`aio
/// snapshot`'s one object) put in place over its door, a new event each.
pub fn load(links: &dyn Fn(&str) -> Result<Link, String>, text: &str, user: &str) -> Out {
    let all: serde_json::Map<String, Value> =
        serde_json::from_str(text).map_err(|e| format!("not a snapshot (one JSON object of cells): {e}"))?;
    let mut said = vec![];
    for (cell, state) in all.iter().filter(|(_, v)| !v.is_null()) {
        let (seq, _) = ask(&links(cell)?, cell, user, json!({ "t": "load", "state": state }))?;
        let back = seq.as_u64().unwrap_or(1).saturating_sub(1);
        said.push(format!(
            "{cell}: loaded as event {seq} (the state before it: `ax timetravel <app> {cell} goto {back}`)"
        ));
    }
    if said.is_empty() {
        return Err("the snapshot holds no cell's state".into());
    }
    Ok(said.join("\n"))
}

/// One event of a log, as `ax replay` sends it again.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub seq: u64,
    pub cell: String,
    pub method: String,
    pub args: Value,
    /// Who caused it, as the log has it (`{"User":"ana"}`, `"System"`).
    pub by: Value,
    pub v: u64,
    /// It drew ids or random numbers (its recorded inputs).
    pub drew: bool,
}

/// `N`, `N..M`, `N..`, `..M` or nothing (all), as an inclusive range (pure).
pub fn range(spec: Option<&str>) -> Result<(u64, u64), String> {
    let bad = || format!("bad range `{}`: N, N..M, N.., ..M, or none for all", spec.unwrap_or_default());
    let n = |s: &str, or: u64| if s.is_empty() { Ok(or) } else { s.parse::<u64>().map_err(|_| bad()) };
    match spec.map(|s| s.split_once("..").unwrap_or((s, s))) {
        None => Ok((0, u64::MAX)),
        Some((a, b)) => {
            let (lo, hi) = (n(a, 0)?, n(b, u64::MAX)?);
            if lo > hi { Err(bad()) } else { Ok((lo, hi)) }
        }
    }
}

/// The events of `db` in `lo..=hi`, oldest first.
pub fn rows(db: &Connection, (lo, hi): (u64, u64)) -> Result<Vec<Row>, String> {
    let mut q = db
        .prepare("SELECT seq, cell, method, args, by, v, inputs FROM log WHERE seq >= ?1 AND seq <= ?2 ORDER BY seq")
        .map_err(|e| e.to_string())?;
    let hi = i64::try_from(hi).unwrap_or(i64::MAX);
    let rows = q
        .query_map(rusqlite::params![lo as i64, hi], |r| {
            let (args, by): (String, String) = (r.get(3)?, r.get(4)?);
            Ok(Row {
                seq: r.get::<_, i64>(0)? as u64,
                cell: r.get(1)?,
                method: r.get(2)?,
                args: serde_json::from_str(&args).unwrap_or(Value::String(args)),
                by: serde_json::from_str(&by).unwrap_or(Value::String(by)),
                v: r.get::<_, i64>(5)? as u64,
                drew: serde_json::from_str::<Value>(&r.get::<_, String>(6)?)
                    .is_ok_and(|i| i.as_object().is_some_and(|o| !o.is_empty())),
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
}

/// Why `row` isn't sent again, if it isn't (pure): replay sends a run's
/// inputs, never what they caused (aio's `am-replay-sends-inputs-only`), and
/// only to the version that wrote it. `declared`: the running cell's.
pub fn skip(row: &Row, declared: Option<u64>) -> Option<String> {
    let caused = ["System", "Schedule", "Effect"].iter().any(|k| row.by == *k || row.by.get(k).is_some());
    if row.method.starts_with('$') {
        Some("time travel or a restore: a state put back, not a call".into())
    } else if caused {
        Some("caused by a schedule, an effect or the boot: the running app makes it again".into())
    } else if declared.is_some_and(|d| d != row.v) {
        Some(format!("written by v{}, the app runs v{}", row.v, declared.unwrap_or_default()))
    } else {
        None
    }
}

/// `ax replay <app> [range] [--from file] [--dry]`: each input of `rows`
/// called again on the running app (`call`), in order; `--dry` says what
/// would be. Fails when any call is refused (the rest still go: each says).
pub fn replay(
    rows: &[Row],
    declared: &dyn Fn(&str) -> Option<u64>,
    call: &dyn Fn(&Row) -> Result<String, String>,
    dry: bool,
) -> Out {
    if rows.is_empty() {
        return Err("no events in that range (ax events lists them)".into());
    }
    let mut out = vec![];
    let mut refused = 0;
    for r in rows {
        let what = format!("{:>6}  {}.{}({})", r.seq, r.cell, r.method, r.args);
        match (skip(r, declared(&r.cell)), dry) {
            (Some(why), _) => out.push(format!("{what}  not sent: {why}")),
            (None, true) => out.push(format!("{what}  would be sent")),
            (None, false) => match call(r) {
                Ok(answer) => out.push(format!("{what}  → {answer}")),
                Err(e) => {
                    refused += 1;
                    out.push(format!("{what}  {e}"));
                }
            },
        }
    }
    if refused > 0 {
        out.push(format!("{refused} refused: the calls after one ran on another state than they did"));
        return Err(out.join("\n"));
    }
    Ok(out.join("\n"))
}

/// How a stored state's shape differs from the one this version saves
/// (pure): (path, what). A field stored but no longer declared is not
/// read by this build; one whose type changed doesn't load. `null` and
/// `[]` fit anything (no value, no item to tell by).
pub fn drift(stored: &Value, declared: &Value, at: &str) -> Vec<(String, String)> {
    let path = |k: &str| if at.is_empty() { k.to_string() } else { format!("{at}.{k}") };
    match (stored, declared) {
        (Value::Object(s), Value::Object(d)) => s
            .iter()
            .flat_map(|(k, v)| match d.get(k) {
                None => vec![(path(k), format!("{}, stored but not declared: this build does not read it", kind(v)))],
                Some(dv) => drift(v, dv, &path(k)),
            })
            .collect(),
        (Value::Array(s), Value::Array(d)) => match (s.first(), d.first()) {
            (Some(s), Some(d)) => drift(s, d, &format!("{at}[]")),
            _ => vec![],
        },
        (s, d) if s == "null" || d == "null" || kind(s) == kind(d) => vec![],
        (s, d) => vec![(at.to_string(), format!("{} stored, {} declared", kind(s), kind(d)))],
    }
}

fn kind(shape: &Value) -> String {
    match shape {
        Value::Object(_) => "object".into(),
        Value::Array(_) => "list".into(),
        v => v.as_str().unwrap_or("?").into(),
    }
}

/// `ax migrations <app>`: each stored cell's version against the one the
/// running app declares, and its shape drift. `shape(cell)` asks the
/// running app (`None`: it isn't running or serves no such cell).
pub fn migrations(db: &Connection, shape: &dyn Fn(&str) -> Option<Value>, json_out: bool) -> Out {
    let mut rows = vec![];
    for cell in crate::metrics::cells(db)? {
        let snap: Option<(u32, String)> = db
            .query_row("SELECT v, state FROM snap WHERE cell = ?1 ORDER BY seq DESC LIMIT 1", [&cell], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .ok();
        let declared = shape(&cell);
        // Each upgrade its snapshots record: the version, and the event it came at.
        let mut q = db.prepare("SELECT seq, v FROM snap WHERE cell = ?1 ORDER BY seq").map_err(|e| e.to_string())?;
        let snaps: Vec<(i64, u32)> = q
            .query_map([&cell], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        let upgrades: Vec<Value> = snaps
            .windows(2)
            .filter(|w| w[0].1 != w[1].1)
            .map(|w| json!({ "from": w[0].1, "to": w[1].1, "at": w[1].0 }))
            .collect();
        let drift: Vec<(String, String)> = match (&snap, &declared) {
            (Some((v, text)), Some(d)) if d["v"].as_u64() == Some(u64::from(*v)) => {
                let st: Value = serde_json::from_str(text).unwrap_or_default();
                drift(&aio::shape_of(&st), &d["shape"], "")
            }
            _ => vec![],
        };
        rows.push(json!({
            "cell": cell,
            "stored": snap.as_ref().map(|s| s.0),
            "declared": declared.as_ref().map(|d| d["v"].clone()),
            "upgrades": upgrades,
            "drift": drift.iter().map(|(p, w)| json!({ "path": p, "issue": w })).collect::<Vec<_>>(),
        }));
    }
    if json_out {
        return Ok(Value::Array(rows).to_string());
    }
    let mut out = vec!["versions (declared, stored):".to_string()];
    let mut drifted = vec![];
    for r in &rows {
        let v = |x: &Value| x.as_u64().map_or("-".to_string(), |v| format!("v{v}"));
        let (d, s) = (v(&r["declared"]), v(&r["stored"]));
        let note = match (r["declared"].as_u64(), r["stored"].as_u64()) {
            (None, _) => "  (not declared by a running app: start it to compare)",
            (Some(d), Some(s)) if s < d => "  (the next start migrates it)",
            (Some(d), Some(s)) if s > d => "  (stored by a newer build: this one refuses it)",
            _ => "",
        };
        let ups: Vec<String> = r["upgrades"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|u| format!("v{}→v{} at event {}", u["from"], u["to"], u["at"]))
            .collect();
        let ups = if ups.is_empty() { String::new() } else { format!("; upgraded {}", ups.join(", ")) };
        out.push(format!("  {}: {d}, stored {s}{ups}{note}", r["cell"].as_str().unwrap_or("?")));
        for x in r["drift"].as_array().into_iter().flatten() {
            drifted.push(format!(
                "  {}.{}: {}",
                r["cell"].as_str().unwrap_or("?"),
                x["path"].as_str().unwrap_or(""),
                x["issue"].as_str().unwrap_or("")
            ));
        }
    }
    out.push(format!("shape drift: {}", drifted.len()));
    out.extend(drifted);
    Ok(out.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aio::prelude::*;

    #[test]
    fn drift_names_fields_stored_but_not_declared_and_changed_types() {
        let stored = json!({ "n": "number", "gone": "string", "items": [{ "id": "string", "old": "bool" }], "at": "null", "t": "string" });
        let declared = json!({ "n": "number", "items": [{ "id": "number" }], "at": "number", "t": "number" });
        let d = drift(&stored, &declared, "");
        assert_eq!(
            d,
            [
                ("gone".to_string(), "string, stored but not declared: this build does not read it".to_string()),
                ("items[].id".into(), "string stored, number declared".into()),
                ("items[].old".into(), "bool, stored but not declared: this build does not read it".into()),
                ("t".into(), "string stored, number declared".into()),
            ]
        );
        assert!(drift(&json!({ "l": [] }), &json!({ "l": ["string"] }), "").is_empty(), "an empty list fits");
    }

    #[test]
    fn a_range_reads_like_aios() {
        assert_eq!(range(None), Ok((0, u64::MAX)));
        assert_eq!(range(Some("4")), Ok((4, 4)));
        assert_eq!(range(Some("2..9")), Ok((2, 9)));
        assert_eq!(range(Some("3..")), Ok((3, u64::MAX)));
        assert_eq!(range(Some("..5")), Ok((0, 5)));
        for bad in ["x", "9..2", "1..x", "-1"] {
            assert!(range(Some(bad)).unwrap_err().contains("bad range"), "{bad}");
        }
    }

    fn row(seq: u64, method: &str, by: Value, v: u64) -> Row {
        Row { seq, cell: "steps".into(), method: method.into(), args: json!([]), by, v, drew: false }
    }

    #[test]
    fn replay_sends_inputs_only_and_to_the_version_that_wrote_them() {
        let rows = [
            row(1, "inc", json!({ "User": "ana" }), 1),
            row(2, "inc", json!({ "Schedule": "tick" }), 1),
            row(3, "inc", json!({ "Effect": "inc:1" }), 1),
            row(4, "$rewind", json!({ "User": "ops" }), 1),
            row(5, "inc", json!("System"), 1),
            row(6, "inc", json!({ "Agent": "bot" }), 2),
            row(7, "inc", json!("Test"), 1),
        ];
        let sent = std::cell::RefCell::new(vec![]);
        let call = |r: &Row| {
            sent.borrow_mut().push(r.seq);
            Ok("1".to_string())
        };
        let out = replay(&rows, &|_| Some(1), &call, true).unwrap();
        assert!(sent.borrow().is_empty(), "--dry sends nothing");
        assert_eq!(out.lines().filter(|l| l.ends_with("would be sent")).count(), 2, "{out}");
        assert!(out.contains("written by v2, the app runs v1") && out.contains("a state put back"), "{out}");
        replay(&rows, &|_| Some(1), &call, false).unwrap();
        assert_eq!(*sent.borrow(), [1, 7]);
        // Not running: no version to hold it to.
        assert!(skip(&rows[5], None).is_none());
        let refuse = |r: &Row| if r.seq == 1 { Err("refused: no".to_string()) } else { Ok("2".into()) };
        let e = replay(&rows, &|_| Some(1), &refuse, false).unwrap_err();
        assert!(e.contains("refused: no") && e.contains("→ 2") && e.contains("1 refused"), "{e}");
        assert!(replay(&[], &|_| None, &call, false).unwrap_err().contains("no events"));
    }

    #[test]
    fn a_diff_names_each_leaf_that_changed() {
        let before = json!({ "n": 1, "notes": { "a": "x" }, "tags": ["t"], "list": [1, 2] });
        let after = json!({ "n": 2, "notes": { "a": "x", "b": "y" }, "tags": ["u"], "list": [1] });
        let d: Vec<String> = diff(&before, &after, "").iter().map(said).collect();
        assert_eq!(d, ["list: [1,2] → [1]", "n: 1 → 2", "notes.b: null → \"y\"", "tags.0: \"t\" → \"u\""]);
        assert!(diff(&before, &before, "").is_empty());
        let long = said(&("s".into(), json!("x".repeat(100)), Value::Null));
        assert!(long.starts_with("s: \"xxx") && long.contains("… → null") && long.chars().count() < 80, "{long}");
    }

    #[cell(persist, shared)]
    pub struct Steps {
        pub n: u32,
    }

    #[methods]
    impl Steps {
        pub fn inc(&mut self) -> Result<u32> {
            self.n += 1;
            Ok(self.n)
        }
    }

    #[test]
    fn the_verbs_move_load_pause_and_compare_a_running_app() {
        let d = tempfile::tempdir().unwrap();
        let store = d.path().join("state.db");
        let h = Handle::<Steps>::open(&store).unwrap();
        for _ in 0..3 {
            h.inc().unwrap();
        }
        // Opened again, as a start does: a snapshot is stored.
        drop(h);
        let h = Handle::<Steps>::open(&store).unwrap();
        let addr = aio::ipc::addr(d.path(), "cell-steps");
        let l = {
            let _in = aio::rt().enter();
            aio::ipc::Listener::bind(&addr).unwrap()
        };
        aio::rt().spawn(aio::sync::serve_door(h.clone(), l, Default::default()));
        let link = Link { addr, token: String::new(), root: None };
        let db = || Connection::open_with_flags(&store, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        // The states around each event, for its diff.
        let at = states(&link, "steps", "ops", &[1, 3]).unwrap();
        assert_eq!(at.keys().copied().collect::<Vec<_>>(), [0, 1, 2, 3]);
        assert_eq!(diff(&at[&2], &at[&3], ""), [("n".to_string(), json!(2), json!(3))]);
        let travel = |step: &str, seq: Option<&str>| {
            crate::live::travel(&link, "steps", crate::live::travel_args(Some(step), seq)?, "ops", false)
        };
        let said = travel("undo", None).unwrap();
        assert!(said.starts_with("steps at event 2 of 3, paused"), "{said}");
        assert_eq!(h.state().n, 2);
        travel("undo", None).unwrap();
        assert_eq!(h.state().n, 1);
        travel("redo", None).unwrap();
        assert_eq!(h.state().n, 2);
        travel("goto", Some("3")).unwrap();
        assert_eq!(h.state().n, 3);
        assert!(travel("goto", Some("x")).unwrap_err().contains("an event number"));
        assert!(travel("sideways", None).unwrap_err().contains("undo, redo"));
        // Paused: calls refused, not swallowed.
        assert!(travel("pause", None).unwrap().contains("paused"));
        assert!(h.inc().unwrap_err().why.contains("paused"));
        assert!(!travel("resume", None).unwrap().contains("paused"));
        h.inc().unwrap();
        // A snapshot file loaded back: each cell a new event.
        let links = |c: &str| if c == "steps" { Ok(link.clone()) } else { Err(format!("no door for {c}")) };
        let said = load(&links, r#"{ "steps": { "n": 40 } }"#, "ops").unwrap();
        assert!(said.starts_with("steps: loaded as event"), "{said}");
        assert_eq!(h.state().n, 40);
        assert!(load(&links, r#"{ "steps": { "n": "x" } }"#, "ops").unwrap_err().contains("doesn't fit"));
        assert!(load(&links, "[1]", "ops").unwrap_err().contains("not a snapshot"));
        assert!(load(&links, r#"{ "other": {} }"#, "ops").unwrap_err().contains("no door for other"));
        // Replay: an input sent again, by the running app's rules.
        crate::live::call(&link, "steps", "inc", json!([]), "ana").unwrap();
        let seq = h.snapshot().0;
        let n = h.state().n;
        let call = |r: &Row| crate::live::call(&link, &r.cell, &r.method, r.args.clone(), "ops");
        let out = replay(&rows(&db(), range(Some(&seq.to_string())).unwrap()).unwrap(), &|_| Some(1), &call, false);
        assert!(out.unwrap().contains(&format!("steps.inc([])  → {}", n + 1)));
        assert_eq!(h.state().n, n + 1);
        // Migrations: declared against stored, and the shape drift.
        let shape = |c: &str| ask(&link, c, "ops", json!({ "t": "shape" })).ok().map(|(v, _)| v);
        let out = migrations(&db(), &shape, false).unwrap();
        assert!(out.contains("steps: v1, stored v1") && out.contains("shape drift: 0"), "{out}");
        let rows: Value = serde_json::from_str(&migrations(&db(), &|_| None, true).unwrap()).unwrap();
        assert_eq!((rows[0]["stored"].as_u64(), rows[0]["declared"].is_null()), (Some(1), true));
        assert!(migrations(&db(), &|_| None, false).unwrap().contains("start it to compare"));
        let drifted = |_: &str| Some(json!({ "v": 1, "shape": { "count": "number" } }));
        let out = migrations(&db(), &drifted, false).unwrap();
        assert!(out.contains("shape drift: 1") && out.contains("steps.n: number, stored but not declared"), "{out}");
        // An upgrade its snapshots record, and a store a newer build wrote.
        let w = Connection::open(&store).unwrap();
        w.execute("INSERT INTO snap (cell, seq, v, state) VALUES ('steps', 9000, 2, '{\"n\":1}')", []).unwrap();
        let out = migrations(&db(), &shape, false).unwrap();
        assert!(out.contains("steps: v1, stored v2; upgraded v1→v2 at event 9000  (stored by a newer build"), "{out}");
    }
}

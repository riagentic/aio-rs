//! `ax metrics <app>`: how long the app has been up, its open links, its
//! schedules and each cell's size. Up from `health.json`; links and sizes
//! live over each cell's door (a cell without a door: its stored size);
//! schedules from the store. The look itself is one link per cell: not
//! counted.
use aio::serde_json::{Value, json};
use rusqlite::Connection;

type Out = Result<String, String>;

/// One cell: its state's size in bytes (UTF-8), and its links (`None`: not
/// reached live, the size is the stored one).
#[derive(Debug, PartialEq)]
pub struct CellUse {
    pub name: String,
    pub bytes: usize,
    pub links: Option<usize>,
}

/// The cells of the store, by name.
pub fn cells(db: &Connection) -> Result<Vec<String>, String> {
    let mut q =
        db.prepare("SELECT cell FROM snap UNION SELECT cell FROM log ORDER BY cell").map_err(|e| e.to_string())?;
    q.query_map([], |r| r.get(0)).map_err(|e| e.to_string())?.collect::<Result<_, _>>().map_err(|e| e.to_string())
}

/// The newest stored state of `cell`, as text.
fn stored_bytes(db: &Connection, cell: &str) -> usize {
    db.query_row(
        "SELECT length(CAST(state AS BLOB)) FROM snap WHERE cell = ?1 ORDER BY seq DESC LIMIT 1",
        [cell],
        |r| r.get::<_, i64>(0),
    )
    .map_or(0, |n| n as usize)
}

/// `cell` live over `link`: its size and the links open to it but this one.
pub fn live(link: &aio::sync::Link, cell: &str, user: &str) -> std::io::Result<(usize, usize)> {
    aio::rt().block_on(async {
        let mut t = aio::sync::Live::connect(link, cell, user).await?;
        let bytes = t.view().to_string().len();
        Ok((bytes, t.clients().await?.len().saturating_sub(1)))
    })
}

/// Each cell of the store: live through `door` when it answers, else stored.
pub fn uses(
    db: &Connection,
    door: &dyn Fn(&str) -> Option<aio::sync::Link>,
    user: &str,
) -> Result<Vec<CellUse>, String> {
    Ok(cells(db)?
        .into_iter()
        .map(|name| match door(&name).and_then(|l| live(&l, &name, user).ok()) {
            Some((bytes, links)) => CellUse { name, bytes, links: Some(links) },
            None => CellUse { bytes: stored_bytes(db, &name), name, links: None },
        })
        .collect())
}

/// Saved schedules.
pub fn schedules(db: &Connection) -> Result<usize, String> {
    db.query_row("SELECT COUNT(*) FROM sched", [], |r| r.get::<_, i64>(0))
        .map(|n| n as usize)
        .map_err(|e| e.to_string())
}

/// The report (pure): `health` is the app's `health.json` when it runs.
pub fn show(health: Option<&Value>, cells: &[CellUse], schedules: usize, as_json: bool) -> Out {
    let up = health.and_then(|h| h["uptimeS"].as_i64());
    let links: usize = cells.iter().filter_map(|c| c.links).sum();
    if as_json {
        let c: aio::serde_json::Map<String, Value> =
            cells.iter().map(|c| (c.name.clone(), json!({ "bytes": c.bytes, "links": c.links }))).collect();
        return Ok(json!({ "uptimeS": up, "connections": links, "schedules": schedules, "cells": c }).to_string());
    }
    let mut out = vec![
        match up {
            Some(s) => format!("up {s} s (pid {})", health.map_or(Value::Null, |h| h["pid"].clone())),
            None => "not running (sizes are the stored ones)".into(),
        },
        format!("{links} links open, {schedules} schedules"),
    ];
    out.extend(cells.iter().map(|c| {
        let how = c.links.map_or("stored".into(), |n| format!("{n} links"));
        format!("  {:<20} {:>10} bytes  {how}", c.name, c.bytes)
    }));
    Ok(out.join("\n"))
}

/// `ax cost <app> [cell] [--keys]`: what each cell sent its clients and
/// how long its methods took, from the running app's meters (`aio::cost`),
/// a row a cell; the top three keys by bytes (`all`: every one) (pure).
pub fn cost(rows: &[(String, Value)], all: bool, as_json: bool) -> String {
    if as_json {
        return Value::Object(rows.iter().cloned().collect()).to_string();
    }
    let b = |v: &Value| match v.as_f64().unwrap_or(0.0) {
        n if n <= 0.0 => "-".to_string(),
        n => aio::fmt::bytes(n as u64),
    };
    let mut out = vec![format!(
        "{:<16} {:>8} {:>11} {:>9} {:>8} {:>5}  {}",
        "cell", "pushes/s", "bytes/s", "mean", "p95 call", "full", "top keys by bytes"
    )];
    for (cell, c) in rows {
        let keys = c["keys"].as_array().cloned().unwrap_or_default();
        let shown: Vec<String> = keys
            .iter()
            .take(if all { usize::MAX } else { 3 })
            .map(|k| format!("{} {}", k["key"].as_str().unwrap_or("?"), b(&k["bytes"])))
            .collect();
        let p95 = c["p95Ms"].as_f64().filter(|p| *p > 0.0).map_or("-".into(), |p| format!("{p:.1} ms"));
        out.push(format!(
            "{cell:<16} {:>8.1} {:>11} {:>9} {:>8} {:>5}  {}",
            c["pushesPerS"].as_f64().unwrap_or(0.0),
            format!("{}/s", b(&c["bytesPerS"])),
            b(&c["meanBytes"]),
            p95,
            c["fullResends"],
            if shown.is_empty() { "-".into() } else { shown.join(" · ") }
        ));
    }
    let watched = rows.iter().filter_map(|(_, c)| c["windowS"].as_f64()).fold(0.0, f64::max);
    out.push(format!("watched {watched:.0} s, the newest window asked for or since the app's first send or call (its tools' looks not counted)"));
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use aio::prelude::*;

    #[cell(shared, persist)]
    pub struct Board {
        pub cards: Vec<String>,
    }

    #[methods]
    impl Board {
        pub fn add(&mut self, cx: &Cx, c: String) -> Result {
            self.cards.push(c);
            cx.every("sweep", 60_000, "add", ("swept",))
        }
    }

    #[test]
    fn uptime_links_schedules_and_sizes_of_a_running_app() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("state.db");
        let h = Handle::<Board>::open(&p).unwrap();
        h.call("add", json!(["日本"])).unwrap();
        let addr = aio::ipc::addr(d.path(), "cell-board");
        let l = {
            let _in = aio::rt().enter();
            aio::ipc::Listener::bind(&addr).unwrap()
        };
        aio::rt().spawn(aio::sync::serve_door(h.clone(), l, Default::default()));
        let link = aio::sync::Link { addr, token: String::new(), root: None };
        let db = rusqlite::Connection::open_with_flags(&p, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let live = uses(&db, &|_| Some(link.clone()), "ops").unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].bytes, json!({ "cards": ["日本"] }).to_string().len(), "bytes, not chars");
        assert_eq!(live[0].links, Some(0), "the look's own link is not counted");
        assert_eq!(schedules(&db).unwrap(), 1);
        let health = json!({ "uptimeS": 42, "pid": 7 });
        let text = show(Some(&health), &live, 1, false).unwrap();
        assert!(text.starts_with("up 42 s (pid 7)\n0 links open, 1 schedules"), "{text}");
        let j: Value = aio::serde_json::from_str(&show(Some(&health), &live, 1, true).unwrap()).unwrap();
        assert_eq!((j["uptimeS"].as_i64(), j["cells"]["board"]["links"].as_u64()), (Some(42), Some(0)));
        // Not running: the stored size, said so.
        let stored = uses(&db, &|_| None, "ops").unwrap();
        assert_eq!(stored[0].links, None);
        assert!(show(None, &stored, 1, false).unwrap().contains("not running"));
        // The cost of each cell, from its meter over the door.
        let c = aio::rt()
            .block_on(async {
                aio::sync::Live::connect(&link, "board", "ops").await.unwrap().ask(json!({ "t": "cost" })).await
            })
            .unwrap();
        assert!(c["calls"].as_u64().unwrap() >= 1, "its method was timed: {c}");
        let meter = json!({ "windowS": 4.0, "pushesPerS": 0.5, "bytesPerS": 2048.0, "meanBytes": 4096, "p95Ms": 1.25,
            "fullResends": 1, "keys": [{ "key": "cards", "bytes": 6000 }, { "key": "a", "bytes": 10 }, { "key": "b", "bytes": 9 }, { "key": "c", "bytes": 8 }] });
        let text = cost(&[("board".into(), meter.clone())], false, false);
        let row = text.lines().nth(1).unwrap();
        assert!(
            row.starts_with("board") && row.contains("2.0 KB/s") && row.contains("4.0 KB") && row.contains("1.2 ms"),
            "{text}"
        );
        assert!(row.ends_with("cards 5.9 KB · a 10 B · b 9 B"), "the top three: {row}");
        assert!(cost(&[("board".into(), meter.clone())], true, false).contains("· c 8 B"));
        assert!(text.ends_with("watched 4 s, the newest window asked for or since the app's first send or call (its tools' looks not counted)"));
        let j: Value = aio::serde_json::from_str(&cost(&[("board".into(), meter)], false, true)).unwrap();
        assert_eq!(j["board"]["meanBytes"], 4096);
    }
}

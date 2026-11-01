//! `ax status <app>…`: where each app is in its life, from its lock and
//! `data/health.json`: `started` (exit 0), `starting`, `stopping` or
//! `maintenance` (held by `ax restore`; exit 2: wait and ask again), or
//! `stopped` (exit 1). A stopped app whose health file is still there ended
//! without a clean stop: it crashed or was killed, and says so.
use aio::serde_json::{Value, json};
use std::path::Path;

/// One app's state: its word, what it says, and its exit code.
#[derive(Debug, PartialEq)]
pub struct Said {
    pub status: &'static str,
    pub line: String,
    pub code: u8,
}

/// The state of app `id` from its health file (if any) and lock, at `now` (pure).
pub fn of(id: &str, doc: Option<&Value>, alive: bool, now: i64) -> Said {
    let said = |status, line: String, code| Said { status, line: format!("{id}: {line}"), code };
    let Some(doc) = doc else {
        return if alive {
            said("starting", "starting (locked, no health yet)".into(), 2)
        } else {
            said("stopped", "stopped".into(), 1)
        };
    };
    let seen = aio::log::stamp(doc["updatedAt"].as_i64().unwrap_or(0));
    match (alive, doc["status"].as_str()) {
        (false, Some("maintenance")) => {
            said("stopped", format!("stopped: `ax {}` was cut short ({seen})", doc["op"].as_str().unwrap_or("?")), 1)
        }
        (false, Some("stopping")) => said("stopped", format!("stopped: it ended while stopping ({seen})"), 1),
        (false, _) => said("stopped", format!("stopped: crashed or killed, last seen {seen}"), 1),
        (true, Some("stopping")) => said("stopping", format!("stopping (pid {})", doc["pid"]), 2),
        (true, Some("maintenance")) => said(
            "maintenance",
            format!("maintenance: `ax {}` holds it (pid {})", doc["op"].as_str().unwrap_or("?"), doc["pid"]),
            2,
        ),
        (true, _) => match aio::health::judge(doc, now, true) {
            Ok(h) => said("started", format!("started: {}", h.trim_start_matches(&format!("{id} is "))), 0),
            Err(e) if e.contains(" is stuck") => {
                said("started", format!("started but {}", e.trim_start_matches(&format!("{id} is "))), 1)
            }
            Err(e) => said("started", format!("started: {}", e.trim_start_matches(&format!("{id} is "))), 0),
        },
    }
}

/// Run `f` (`op`, holding app `id`'s lock) with the app in maintenance:
/// `ax status` says so until `f` is done, however it ends.
pub fn held<R>(data: &Path, id: &str, op: &str, f: impl FnOnce() -> R) -> std::io::Result<R> {
    let file = data.join("health.json");
    aio::paths::write_atomic(&file, aio::health::maintenance(id, op).to_string().as_bytes(), true)?;
    let r = f();
    _ = std::fs::remove_file(&file);
    Ok(r)
}

/// `ax status <app>…`: each app's line (or `--json`), and the exit code.
pub fn main(names: &[&str], ask: &aio::paths::Ask, json: bool) -> std::process::ExitCode {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
    let mut all = Vec::new();
    for name in names {
        let data = match crate::target::of(name, ask) {
            Ok(t) => t.plan.data,
            Err(e) => {
                eprintln!("ax: {name}: {e}");
                return std::process::ExitCode::FAILURE;
            }
        };
        let doc = std::fs::read(data.join("health.json")).ok().and_then(|b| aio::serde_json::from_slice(&b).ok());
        all.push(of(name, doc.as_ref(), aio::paths::running(&data), now));
    }
    println!("{}", show(&all, json));
    std::process::ExitCode::from(code(&all))
}

/// One exit code for many apps: 0 when all are started, else 1 when any is
/// stopped (or stuck), else 2.
pub fn code(all: &[Said]) -> u8 {
    if all.iter().all(|s| s.code == 0) {
        0
    } else if all.iter().any(|s| s.code == 1) {
        1
    } else {
        2
    }
}

/// The lines (or a JSON list) to print.
pub fn show(all: &[Said], json: bool) -> String {
    if json {
        let v: Vec<Value> = all.iter().map(|s| json!({ "status": s.status, "said": s.line })).collect();
        return Value::Array(v).to_string();
    }
    all.iter().map(|s| s.line.as_str()).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(status: &str, at: i64) -> Value {
        json!({ "status": status, "appId": "notes", "pid": 42, "updatedAt": at, "uptimeS": 3,
                "memory": { "level": "ok", "rssBytes": 1 << 20, "machinePct": 0 }, "degraded": [], "op": "restore" })
    }

    #[test]
    fn each_state_says_itself_with_its_exit_code() {
        let now = 100_000;
        let s = |d: Option<Value>, alive| of("notes", d.as_ref(), alive, now);
        assert_eq!(s(None, false), Said { status: "stopped", line: "notes: stopped".into(), code: 1 });
        assert_eq!((s(None, true).status, s(None, true).code), ("starting", 2));
        let up = s(Some(doc("healthy", now)), true);
        assert_eq!((up.status, up.code), ("started", 0));
        assert!(up.line.starts_with("notes: started: healthy: pid 42"), "{}", up.line);
        let sick = s(Some(doc("degraded", now)), true);
        assert!(sick.line.starts_with("notes: started: degraded") && sick.code == 0, "{}", sick.line);
        let stuck = s(Some(doc("healthy", 0)), true);
        assert!(stuck.line.contains("started but stuck") && stuck.code == 1, "{}", stuck.line);
        assert_eq!(s(Some(doc("stopping", 0)), true).line, "notes: stopping (pid 42)");
        assert_eq!(s(Some(doc("stopping", 0)), true).code, 2);
        let held = s(Some(doc("maintenance", 0)), true);
        assert_eq!((held.status, held.code), ("maintenance", 2));
        assert!(held.line.contains("`ax restore` holds it"), "{}", held.line);
    }

    #[test]
    fn a_stopped_app_whose_last_run_crashed_says_so() {
        let crashed = of("notes", Some(&doc("healthy", 0)), false, 9);
        assert_eq!((crashed.status, crashed.code), ("stopped", 1));
        assert!(crashed.line.contains("crashed or killed, last seen 1970-01-01T00:00:00.000Z"), "{}", crashed.line);
        assert!(of("notes", Some(&doc("maintenance", 0)), false, 9).line.contains("`ax restore` was cut short"));
        // A clean stop leaves no file: plain `stopped`.
        assert_eq!(of("notes", None, false, 9).line, "notes: stopped");
    }

    #[test]
    fn a_restore_holds_the_app_in_maintenance_until_it_is_done() {
        let d = tempfile::tempdir().unwrap();
        let read =
            || aio::serde_json::from_slice::<Value>(&std::fs::read(d.path().join("health.json")).unwrap()).unwrap();
        let during = held(d.path(), "notes", "restore", || of("notes", Some(&read()), true, 0)).unwrap();
        assert_eq!((during.status, during.code), ("maintenance", 2));
        assert!(!d.path().join("health.json").exists(), "after it, a plain stopped app");
        let failed: Result<(), &str> = held(d.path(), "notes", "restore", || Err("bad backup")).unwrap();
        assert!(failed.is_err() && !d.path().join("health.json").exists());
    }

    #[test]
    fn many_apps_one_code() {
        let s = |code| Said { status: "x", line: String::new(), code };
        assert_eq!(code(&[s(0), s(0)]), 0);
        assert_eq!(code(&[s(0), s(2)]), 2);
        assert_eq!(code(&[s(2), s(1), s(0)]), 1);
        let j: Value = aio::serde_json::from_str(&show(&[of("notes", None, false, 0)], true)).unwrap();
        assert_eq!(j, json!([{ "status": "stopped", "said": "notes: stopped" }]));
    }
}

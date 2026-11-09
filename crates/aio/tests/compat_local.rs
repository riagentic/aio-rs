//! aio's tools reach an aio-rs app (feature `compat`): its lock record in
//! aio's lock folder (`am instances`) and aio's local socket, NDJSON lines
//! of aio's envelope, as `am state` and `am dispatch` send them.
use aio::prelude::*;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};

#[cell(shared)]
pub struct Board {
    pub notes: Vec<String>,
}

#[methods]
impl Board {
    pub fn add(&mut self, text: String) -> Result<usize> {
        ensure!(!text.is_empty(), "empty note");
        self.notes.push(text);
        Ok(self.notes.len())
    }
}

/// One `am` request: the three lines aio's `localRequest` writes, the
/// `ctlr` answering it.
fn am(socket: &str, path: &str, method: &str, headers: Value, body: Option<&str>) -> Value {
    let s = aio::ipc::connect_blocking(socket).unwrap();
    let mut w = s.try_clone().unwrap();
    let mut ctl = json!({ "id": "r1", "path": path, "method": method, "headers": headers });
    if let Some(b) = body {
        ctl["body"] = b.into();
    }
    let lines = [
        json!({ "v": 2, "t": "type", "d": { "kind": "control" } }),
        json!({ "v": 2, "t": "proto", "d": { "v": 3, "min": 3, "ver": "test" } }),
        json!({ "v": 2, "t": "ctl", "d": ctl }),
    ];
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    w.write_all(text.as_bytes()).unwrap();
    for line in BufReader::new(s).lines() {
        let m: Value = serde_json::from_str(&line.unwrap()).unwrap();
        if m["t"] == "ctlr" && m["d"]["id"] == "r1" {
            let d = &m["d"];
            return json!({ "status": d["status"], "body": serde_json::from_str::<Value>(d["body"].as_str().unwrap()).unwrap() });
        }
    }
    panic!("no ctlr");
}

#[test]
fn am_lists_reads_and_calls_an_aio_rs_app() {
    let d = tempfile::tempdir_in(aio::paths::short_temp()).unwrap();
    // SAFETY: before any thread of this test binary is started (one test).
    unsafe {
        // aio's lock folder's base: XDG_RUNTIME_DIR, on Windows %TEMP%.
        std::env::set_var("XDG_RUNTIME_DIR", d.path().join("run"));
        std::env::set_var("TEMP", d.path().join("run"));
        std::env::set_var("HOME", d.path().join("home"));
        std::env::set_var("USERPROFILE", d.path().join("home"));
        std::env::remove_var("AIO_APPS_DIR");
    }
    std::fs::create_dir_all(d.path().join("run")).unwrap();
    // aio's default home: `~/.<id>`, so the record's key is the id.
    let data = d.path().join("home/.notes/data");
    std::fs::create_dir_all(&data).unwrap();
    let h = Handle::<Board>::memory();
    let _in = aio::rt().enter();
    let addr = aio::ipc::addr(d.path(), "cell-board");
    let l = aio::ipc::Listener::bind(&addr).unwrap();
    aio::rt().spawn(aio::door::serve(h.clone(), l, Default::default()));
    aio::compat::local::serve("notes", &data, None, "board", addr);

    let lock = d.path().join("run/aio/notes.lock");
    let rec: Value = serde_json::from_slice(&std::fs::read(&lock).unwrap()).unwrap();
    assert_eq!((rec["appId"].as_str(), rec["pid"].as_u64()), (Some("notes"), Some(u64::from(std::process::id()))));
    assert_eq!(rec["home"].as_str(), d.path().join("home").join(".notes").to_str());
    let socket = rec["socketPath"].as_str().unwrap().to_string();
    // aio's socket: a Unix socket in the lock folder, on Windows a named pipe.
    let at = if cfg!(windows) { r"\\.\pipe\aio-notes".into() } else { d.path().join("run/aio/notes.sock") };
    assert_eq!(socket, at.to_str().unwrap());

    let health = am(&socket, "/__aio/health", "GET", json!({}), None);
    assert_eq!(health["status"], 200);
    let key = std::fs::read_to_string(data.join("control.key")).unwrap();
    assert_eq!(key.trim().len(), 64, "aio's control key: 32 bytes as hex");
    let keyed = json!({ "X-Aio-Control": key.trim() });
    let state = am(&socket, "/__aio/trojan/state", "GET", keyed.clone(), None);
    if !aio::door::OPERATOR {
        assert_eq!(
            (state["status"].as_u64(), state["body"]["error"].as_str()),
            (Some(404), Some("trojan is disabled in production"))
        );
        return;
    }
    assert_eq!(state, json!({ "status": 200, "body": { "board": { "notes": [] } } }));
    assert_eq!(
        am(&socket, "/__aio/trojan/state", "GET", json!({ "X-Aio-Control": "0".repeat(64) }), None)["status"],
        401
    );
    let post = |ty: &str, payload: Value| {
        let mut headers = keyed.clone();
        headers["X-AIO"] = "1".into();
        let body = json!({ "type": ty, "payload": payload }).to_string();
        am(&socket, "/__aio/trojan/dispatch", "POST", headers, Some(&body))
    };
    let done = post("board:add", json!({ "args": ["from am"] }));
    assert_eq!(done, json!({ "status": 200, "body": { "ok": true, "result": 1, "unsaved": null } }));
    assert_eq!(h.state().notes, ["from am"]);
    assert_eq!(post("board:add", json!({ "args": [""] }))["body"]["error"], "empty note");
    assert!(post("Increment", json!({ "by": 1 }))["body"]["error"].as_str().unwrap().contains("no plain actions"));
    let unmarked = am(&socket, "/__aio/trojan/dispatch", "POST", keyed, Some("{}"));
    assert_eq!(unmarked["status"], 403, "aio's X-AIO header");
    assert_eq!(
        h.store(|s| s.events().unwrap()).last().unwrap().by,
        By::User(std::env::var("USER").unwrap_or_else(|_| "am".into()))
    );
}

//! `ax control <app>` (aio's control REST API, `/__aio/trojan/*`): the
//! running app's verbs over loopback HTTP, for a tool that can't run `ax`
//! (a browser test runner, another language's harness). It is a mirror of
//! the command line, not a second API: `GET /__aio/trojan/<verb>?…` runs
//! `ax <verb> <app> … --json` here and answers its one JSON object, with
//! the status of its exit code (200 ok, 422 failed, 400 bad usage). Query
//! parameters `arg` are the verb's words in order, any other `k=v` is
//! `--k v` and a bare `k` is `--k`. Only on 127.0.0.1, only with the key
//! made for the run (`Authorization: Bearer <key>`), never from a page (a
//! request with an `Origin` is refused, a foreign `Host` too).
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};

/// The verbs it mirrors: those about the running app that answer once.
pub const VERBS: &[&str] = &[
    "state",
    "snapshot",
    "snapshot save",
    "snapshot load",
    "call",
    "expect",
    "events",
    "timeline",
    "timetravel",
    "clients",
    "shot",
    "eval",
    "surface",
    "trigger",
    "components",
    "logs",
    "errors",
    "health",
    "metrics",
    "heap",
    "cost",
    "peer",
    "schedules",
    "sql",
    "tables",
    "migrations",
    "persist",
    "replay",
    "doctor",
    "data",
    "config",
    "report",
    "testgen",
    "stop",
    "restart",
];

/// Flags that never end: not over a request.
const ENDLESS: [&str; 2] = ["follow", "watch"];

/// `%xx` and `+` decoded (pure); a bad escape stays as it is.
fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                match std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()).ok_or(())
                {
                    Ok(v) => {
                        out.push(v);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The command line a request target asks for (pure): `Err` (status, why).
pub fn args_of(app: &str, target: &str) -> Result<Vec<String>, (u16, String)> {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let verb = path
        .strip_prefix("/__aio/trojan/")
        .map(|v| decode(v).replace('/', " "))
        .ok_or((404, format!("{path}: the verbs are at /__aio/trojan/<verb>")))?;
    if !VERBS.contains(&verb.as_str()) {
        return Err((404, format!("{verb}: not mirrored (these are: {})", VERBS.join(", "))));
    }
    let mut words = vec![app.to_string()];
    let mut flags = vec!["--json".to_string()];
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').map_or((decode(pair), None), |(k, v)| (decode(k), Some(decode(v))));
        if ENDLESS.contains(&k.as_str()) || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') || k.is_empty() {
            return Err((400, format!("{k}: no such parameter here (never --follow or --watch: a request ends)")));
        }
        match (k.as_str(), v) {
            ("arg", Some(v)) => words.push(v),
            (_, None) => flags.push(format!("--{k}")),
            (_, Some(v)) => flags.extend([format!("--{k}"), v]),
        }
    }
    Ok(verb.split(' ').map(String::from).chain(flags).chain(std::iter::once("--".into())).chain(words).collect())
}

/// The status of an exit code.
fn status(exit: u8) -> (u16, &'static str) {
    match exit {
        0 => (200, "OK"),
        2 => (400, "Bad Request"),
        _ => (422, "Unprocessable Entity"),
    }
}

fn answer(s: &mut TcpStream, code: u16, reason: &str, body: &str) {
    let head = format!(
        "HTTP/1.1 {code} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\ncache-control: no-store\r\nconnection: close\r\n\r\n",
        body.len()
    );
    _ = s.write_all(head.as_bytes()).and_then(|_| s.write_all(body.as_bytes()));
}

fn refuse(s: &mut TcpStream, code: u16, why: &str) {
    let reason = match code {
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Bad Request",
    };
    answer(s, code, reason, &aio::serde_json::json!({ "ok": false, "error": why, "exit": 2 }).to_string());
}

/// Equal, in time that doesn't say where they differ.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0, |d, (x, y)| d | (x ^ y)) == 0
}

/// One request on `s`, run by `run` (a command line → exit code, its JSON).
pub fn serve_one(s: &mut TcpStream, app: &str, key: &str, port: u16, run: &dyn Fn(&[String]) -> (u8, String)) {
    let mut r = BufReader::new(match s.try_clone() {
        Ok(c) => c,
        Err(_) => return,
    });
    let mut line = String::new();
    if r.read_line(&mut line).is_err() {
        return;
    }
    let (mut auth, mut host, mut origin) = (None, None, false);
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).map_or(true, |n| n == 0) || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            match k.trim().to_ascii_lowercase().as_str() {
                "authorization" => auth = v.trim().strip_prefix("Bearer ").map(String::from),
                "host" => host = Some(v.trim().to_string()),
                "origin" => origin = true,
                _ => {}
            }
        }
    }
    let mut parts = line.split_whitespace();
    let (method, target) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
    let ours = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    if origin || !host.as_ref().is_some_and(|h| ours.contains(h)) {
        return refuse(s, 403, "only a local tool: no page (Origin), no other Host");
    }
    if !auth.as_deref().is_some_and(|a| same(a, key)) {
        return refuse(s, 401, "Authorization: Bearer <the key ax control printed>");
    }
    if method != "GET" {
        return refuse(s, 405, "GET /__aio/trojan/<verb>?arg=…&flag=…");
    }
    match args_of(app, target) {
        Err((code, why)) => refuse(s, code, &why),
        Ok(args) => {
            let (exit, body) = run(&args);
            let (code, reason) = status(exit);
            answer(s, code, reason, &body);
        }
    }
}

/// Serve until Ctrl-C on `127.0.0.1:port` (0: a free one), the key printed first.
pub fn serve(app: &str, port: u16, run: &dyn Fn(&[String]) -> (u8, String)) -> Result<String, String> {
    let l = TcpListener::bind(("127.0.0.1", port)).map_err(|e| format!("127.0.0.1:{port}: {e}"))?;
    let port = l.local_addr().map_err(|e| e.to_string())?.port();
    let key: String = aio::random_bytes::<16>().iter().map(|b| format!("{b:02x}")).collect();
    println!("{key}");
    eprintln!(
        "ax control: {app}'s verbs on http://127.0.0.1:{port}/__aio/trojan/<verb> (Authorization: Bearer <the line above>); Ctrl-C ends it"
    );
    for s in l.incoming().flatten() {
        let mut s = s;
        _ = s.set_read_timeout(Some(std::time::Duration::from_secs(10)));
        serve_one(&mut s, app, &key, port, run);
    }
    Ok(String::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_its_command_line() {
        assert_eq!(
            args_of("notes", "/__aio/trojan/state?arg=todo&arg=items.0&as=ana").unwrap(),
            ["state", "--json", "--as", "ana", "--", "notes", "todo", "items.0"]
        );
        assert_eq!(
            args_of("notes", "/__aio/trojan/snapshot/load?arg=s.json&force").unwrap()[..3],
            ["snapshot", "load", "--json"]
        );
        assert_eq!(
            args_of("n", "/__aio/trojan/call?arg=todo&arg=add&arg=buy+milk%21").unwrap().last().unwrap(),
            "buy milk!"
        );
        assert_eq!(args_of("n", "/__aio/trojan/dev").unwrap_err().0, 404);
        assert_eq!(args_of("n", "/other").unwrap_err().0, 404);
        assert_eq!(args_of("n", "/__aio/trojan/logs?follow").unwrap_err().0, 400);
        assert_eq!(args_of("n", "/__aio/trojan/logs?x%20y=1").unwrap_err().0, 400);
        assert_eq!(decode("a%2Fb%zz"), "a/b%zz");
    }

    fn ask(port: u16, req: &str) -> String {
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        c.write_all(req.as_bytes()).unwrap();
        let mut out = String::new();
        std::io::Read::read_to_string(&mut c, &mut out).unwrap();
        out
    }

    #[test]
    fn only_a_local_tool_with_the_key_is_answered() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let run = |a: &[String]| (u8::from(a[0] != "health"), format!("{{\"ok\":true,\"args\":{}}}", a.len()));
        std::thread::spawn(move || {
            for s in l.incoming().flatten().take(5) {
                let mut s = s;
                serve_one(&mut s, "notes", "k3y", port, &run);
            }
        });
        let get = |path: &str, extra: &str| {
            ask(port, &format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{extra}\r\n"))
        };
        let ok = get("/__aio/trojan/health", "Authorization: Bearer k3y\r\n");
        assert!(ok.starts_with("HTTP/1.1 200 OK") && ok.ends_with("{\"ok\":true,\"args\":4}"), "{ok}");
        assert!(get("/__aio/trojan/state?arg=c", "Authorization: Bearer k3y\r\n").starts_with("HTTP/1.1 422"));
        assert!(get("/__aio/trojan/health", "Authorization: Bearer nope\r\n").starts_with("HTTP/1.1 401"));
        assert!(
            get("/__aio/trojan/health", "Authorization: Bearer k3y\r\nOrigin: https://evil.example\r\n")
                .starts_with("HTTP/1.1 403")
        );
        let other =
            ask(port, "GET /__aio/trojan/health HTTP/1.1\r\nHost: evil.example\r\nAuthorization: Bearer k3y\r\n\r\n");
        assert!(other.starts_with("HTTP/1.1 403"), "{other}");
    }
}

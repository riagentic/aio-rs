//! The Chrome DevTools Protocol, as `ax shot` reaches an app's window:
//! over the running app's own `ui` door (no flag, no port), or over a
//! port a debug build opened on purpose (`--cdp`, `AIORT_CDP=<port>`, loopback only:
//! the target list over HTTP, then one target's WebSocket). Every protocol
//! error is an `Err`, never swallowed.
use aio::serde_json::{Value, json};
use futures_util::{SinkExt, StreamExt};
use std::io::{Read, Write};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

/// How long a CDP endpoint may take to answer anything.
pub const WAIT: Duration = Duration::from_secs(10);

/// A promise of the page's next paint: two frames, as a person sees what
/// an action did. A hidden window (minimized, covered, on another desktop)
/// draws no frames, so its DOM, already changed, is as far as it goes: it
/// answers at once, also when the window hides while it waits.
pub const PAINTED: &str = "new Promise((r) => { const done = () => r(true); if (document.hidden) return done(); \
     document.addEventListener('visibilitychange', done, { once: true }); \
     requestAnimationFrame(() => requestAnimationFrame(done)) })";

/// Where a launch with `--cdp` (`ax start`, `ax dev`) keeps the port it
/// opened, in the app's logs dir; a launch without it removes the file.
pub const PORT_FILE: &str = "cdp.port";

/// `--cdp` (any free port) or `--cdp=<port>` taken out of a launch's args,
/// before a bare `--` (pure): the rest, and the port asked (0: any).
pub fn take_flag(args: &[String]) -> Result<(Vec<String>, Option<u16>), String> {
    let end = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let mut port = None;
    let mut rest = vec![];
    for (i, a) in args.iter().enumerate() {
        match a.strip_prefix("--cdp") {
            Some("") if i < end => port = Some(0),
            Some(p) if i < end && p.starts_with('=') => {
                port = Some(p[1..].parse().ok().filter(|n| *n > 0).ok_or(format!("{a}: not a port"))?)
            }
            _ => rest.push(a.clone()),
        }
    }
    Ok((rest, port))
}

/// The port a launch `asked` for (`take_flag`; 0 picks a free one), kept in
/// `logs`; asked for none, an earlier one is forgotten. The launch hands it
/// to the app as `AIORT_CDP`.
pub fn open(logs: &std::path::Path, asked: Option<u16>) -> Result<Option<u16>, String> {
    let file = logs.join(PORT_FILE);
    let Some(port) = asked.map(|p| if p == 0 { crate::dev::free_port() } else { p }) else {
        _ = std::fs::remove_file(&file);
        return Ok(None);
    };
    aio::paths::private_dir(logs).map_err(|e| format!("{}: {e}", logs.display()))?;
    aio::paths::write_atomic(&file, port.to_string().as_bytes(), false)
        .map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(Some(port))
}

/// The port the running app at `plan` was launched with `--cdp` on.
pub fn recorded(plan: &aio::paths::Plan) -> Option<u16> {
    if !aio::paths::running(&plan.data) {
        return None;
    }
    std::fs::read_to_string(plan.logs.join(PORT_FILE)).ok()?.trim().parse().ok()
}

/// Where a window's DevTools protocol is reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reach {
    /// A port the app opened on purpose (`--cdp`, `AIORT_CDP`).
    Port(u16),
    /// The running app's own `ui` door (`aio_shell::inspect`): no flag.
    Door(String),
}

/// The `ui` door of app `id` running on `plan`'s data, if it runs.
pub fn door(id: &str, plan: &aio::paths::Plan) -> Option<String> {
    aio::paths::running(&plan.data)
        .then(|| aio::ipc::addr(&aio::paths::sockets_at(id, &plan.data), aio_shell::inspect::DOOR))
}

/// The window's way in: `flag` (`--cdp`), else `AIORT_CDP`, else the port
/// the running app was launched with (`recorded`), else its `door`.
pub fn reach(flag: Option<&str>, recorded: Option<u16>, door: Option<String>) -> Result<Reach, String> {
    let p = flag.map(String::from).or_else(|| std::env::var("AIORT_CDP").ok()).filter(|p| !p.is_empty());
    let Some(p) = p else {
        return recorded
            .map(Reach::Port)
            .or(door.map(Reach::Door))
            .ok_or("the app is not running: start it with `ax start <app>` (or `ax dev <app>`)".into());
    };
    p.parse().ok().filter(|n| *n > 0).map(Reach::Port).ok_or(format!("--cdp {p}: not a port"))
}

/// The targets the endpoint on `127.0.0.1:<port>` lists (`GET /json`).
pub fn targets(port: u16) -> Result<Vec<Value>, String> {
    let at = format!("127.0.0.1:{port}");
    let no = |e: std::io::Error| format!("no DevTools protocol on {at} ({e}): start the app with AIORT_CDP={port}");
    let mut s =
        std::net::TcpStream::connect_timeout(&at.parse().map_err(|e| format!("{at}: {e}"))?, WAIT).map_err(no)?;
    s.set_read_timeout(Some(WAIT)).map_err(no)?;
    // HTTP/1.1: Chromium answers an HTTP/1.0 request with nothing.
    write!(s, "GET /json HTTP/1.1\r\nHost: {at}\r\nConnection: close\r\n\r\n").map_err(no)?;
    // Read to its length: Chromium keeps the connection open.
    let (mut got, mut buf) = (Vec::new(), [0; 8192]);
    let text = loop {
        let n = s.read(&mut buf).map_err(no)?;
        got.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&got).into_owned();
        let whole = text.split_once("\r\n\r\n").is_some_and(|(head, body)| {
            let len = head.lines().find_map(|l| {
                let (k, v) = l.split_once(':')?;
                k.eq_ignore_ascii_case("content-length").then(|| v.trim().parse::<usize>().ok())?
            });
            len.is_some_and(|len| body.len() >= len)
        });
        if n == 0 || whole {
            break text;
        }
    };
    let (head, body) = text.split_once("\r\n\r\n").ok_or("CDP /json: no answer")?;
    if !head.starts_with("HTTP/1.1 200") && !head.starts_with("HTTP/1.0 200") {
        return Err(format!("CDP /json answered {}", head.lines().next().unwrap_or("")));
    }
    aio::serde_json::from_str(body).map_err(|e| format!("CDP /json: {e}"))
}

/// The app's own windows among `targets` (pure): its `aio://` pages, not
/// DevTools' or a blank one.
pub fn app_pages(targets: &[Value]) -> Vec<&Value> {
    targets
        .iter()
        .filter(|t| t["type"] == "page" && t["url"].as_str().is_some_and(|u| u.starts_with("aio://")))
        .collect()
}

/// A session with the app's window number `n` (0: its first).
pub async fn window(at: &Reach, n: usize) -> Result<Session, String> {
    let port = match at {
        Reach::Port(p) => *p,
        Reach::Door(addr) if n == 0 => return Session::door(addr).await,
        Reach::Door(_) => return Err(format!("no window {n}: the app has 1 (0)")),
    };
    let list = tokio::task::spawn_blocking(move || targets(port)).await.map_err(|e| e.to_string())??;
    let pages = app_pages(&list);
    let Some(page) = pages.get(n) else {
        return Err(match pages.len() {
            0 => format!("no app window on 127.0.0.1:{port} (is the app's window open?)"),
            k => format!(
                "no window {n}: the app has {k} ({})",
                (0..k).map(|i| i.to_string()).collect::<Vec<_>>().join(", ")
            ),
        });
    };
    Session::connect(page["webSocketDebuggerUrl"].as_str().unwrap_or_default()).await
}

/// [`window`], waited for up to `wait` (aio's `--wait-client`): right after a
/// start or a reload the app may briefly have no window, or no endpoint yet,
/// or its door answers before its page does (asked once: a page that runs).
pub async fn window_within(at: &Reach, n: usize, wait: Duration) -> Result<Session, String> {
    let until = tokio::time::Instant::now() + wait;
    loop {
        let r = match window(at, n).await {
            Ok(mut s) if !wait.is_zero() => s.eval("1").await.map(|_| s),
            r => r,
        };
        match r {
            Err(_) if tokio::time::Instant::now() < until => tokio::time::sleep(Duration::from_millis(100)).await,
            r => return r,
        }
    }
}

/// The app's windows (aio's `am clients`), one line each: its number (as
/// `--window` takes it), title and page.
pub async fn windows(at: &Reach) -> Result<String, String> {
    let line = |n: usize, t: &Value| {
        format!("{n}  {}  {}", t["title"].as_str().unwrap_or(""), t["url"].as_str().unwrap_or(""))
    };
    match at {
        Reach::Port(port) => {
            let port = *port;
            let list = tokio::task::spawn_blocking(move || targets(port)).await.map_err(|e| e.to_string())??;
            Ok(app_pages(&list).into_iter().enumerate().map(|(n, t)| line(n, t)).collect::<Vec<_>>().join("\n"))
        }
        Reach::Door(_) => {
            let t = window(at, 0).await?.eval("({ title: document.title, url: location.href })").await?;
            Ok(line(0, &t))
        }
    }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type Lines = tokio::io::Lines<tokio::io::BufReader<tokio::io::ReadHalf<aio::ipc::Stream>>>;

/// A target's messages: over its WebSocket, or as lines over the door.
enum Link {
    Ws(Box<Ws>),
    Door(Lines, tokio::io::WriteHalf<aio::ipc::Stream>),
}

/// One target's session: commands answered by id, events in between kept
/// for [`Session::event`].
pub struct Session {
    link: Link,
    id: u64,
    events: Vec<Value>,
}

impl Session {
    /// Connect to a target's `webSocketDebuggerUrl`.
    pub async fn connect(url: &str) -> Result<Self, String> {
        let (ws, _) = tokio::time::timeout(WAIT, tokio_tungstenite::connect_async(url))
            .await
            .map_err(|_| format!("CDP connect timed out after {WAIT:?}"))?
            .map_err(|e| format!("CDP connect failed: {url}: {e}"))?;
        Ok(Self { link: Link::Ws(Box::new(ws)), id: 0, events: vec![] })
    }

    /// Connect to a running app's `ui` door at `addr`.
    pub async fn door(addr: &str) -> Result<Self, String> {
        use tokio::io::AsyncBufReadExt;
        let s = aio::ipc::connect(addr)
            .await
            .map_err(|e| format!("the running app has no window to reach ({addr}: {e}); a server, a CLI or a release build opens none: start it with --cdp"))?;
        let (r, w) = tokio::io::split(s);
        Ok(Self { link: Link::Door(tokio::io::BufReader::new(r).lines(), w), id: 0, events: vec![] })
    }

    async fn send(&mut self, m: Value) -> Result<(), String> {
        use tokio::io::AsyncWriteExt;
        match &mut self.link {
            Link::Ws(ws) => ws.send(Message::text(m.to_string())).await.map_err(|e| format!("CDP: {e}")),
            Link::Door(_, w) => w.write_all(format!("{m}\n").as_bytes()).await.map_err(|e| format!("CDP: {e}")),
        }
    }

    async fn next(&mut self) -> Result<Value, String> {
        let text = match &mut self.link {
            Link::Ws(ws) => loop {
                match ws.next().await {
                    Some(Ok(Message::Text(t))) => break t.to_string(),
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => return Err(format!("CDP: {e}")),
                    None => return Err("CDP socket closed".into()),
                }
            },
            Link::Door(lines, _) => match lines.next_line().await {
                Ok(Some(l)) => l,
                Ok(None) => return Err("the app's window door closed".into()),
                Err(e) => return Err(format!("CDP: {e}")),
            },
        };
        aio::serde_json::from_str(&text).map_err(|e| format!("CDP: {e}"))
    }

    /// Run `method` with `params`: its result, or the protocol's error.
    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.id += 1;
        let id = self.id;
        let m = json!({ "id": id, "method": method, "params": params });
        self.send(m).await?;
        let answer = async {
            loop {
                let m = self.next().await?;
                if m["id"] == id {
                    return match m["error"]["message"].as_str() {
                        Some(e) => Err(format!("CDP {method}: {e}")),
                        None => Ok(m["result"].clone()),
                    };
                }
                if m.get("method").is_some() {
                    self.events.push(m);
                }
            }
        };
        tokio::time::timeout(WAIT, answer).await.map_err(|_| format!("CDP {method}: no answer in {WAIT:?}"))?
    }

    /// The next `event`'s params (one heard during a call first), if one
    /// comes within `within`.
    pub async fn event(&mut self, event: &str, within: Duration) -> Result<Option<Value>, String> {
        if let Some(i) = self.events.iter().position(|e| e["method"] == event) {
            return Ok(Some(self.events.remove(i)["params"].take()));
        }
        let heard = async {
            loop {
                let m = self.next().await?;
                if m["method"] == event {
                    return Ok(m["params"].clone());
                }
            }
        };
        tokio::time::timeout(within, heard).await.ok().transpose()
    }

    /// `expr`'s value in the page (awaited when it is a promise).
    pub async fn eval(&mut self, expr: &str) -> Result<Value, String> {
        let r = self
            .call("Runtime.evaluate", json!({ "expression": expr, "awaitPromise": true, "returnByValue": true }))
            .await?;
        if let Some(e) = r["exceptionDetails"].as_object() {
            let why =
                e.get("exception").and_then(|x| x["description"].as_str()).or(e.get("text").and_then(Value::as_str));
            return Err(format!("the page threw: {}", why.unwrap_or("?")));
        }
        Ok(r["result"]["value"].clone())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashMap;

    /// What a fake endpoint answers, by method.
    pub(crate) type Answers = HashMap<String, Value>;

    /// A fake CDP endpoint on loopback: `GET /json` lists one app page and a
    /// DevTools page, and its socket answers each method from `answers`
    /// (an `{"error": …}` one as the protocol's error), after an event and
    /// the answer's own `$events`.
    pub(crate) async fn endpoint(answers: Answers) -> u16 {
        endpoint_on(tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(), answers)
    }

    /// [`endpoint`] on `l`.
    fn endpoint_on(l: tokio::net::TcpListener, answers: Answers) -> u16 {
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (s, _) = l.accept().await.unwrap();
                let mut peek = [0; 16];
                let n = s.peek(&mut peek).await.unwrap();
                if peek[..n].starts_with(b"GET /json ") {
                    let ws = format!("ws://127.0.0.1:{port}/devtools/page/A");
                    let list = json!([
                        { "type": "page", "url": "devtools://devtools/x", "webSocketDebuggerUrl": "ws://x" },
                        { "type": "page", "url": "aio://app/", "webSocketDebuggerUrl": ws },
                    ]);
                    let mut s = s.into_std().unwrap();
                    s.set_nonblocking(false).unwrap();
                    // The whole request, then a clean close: unread bytes make
                    // Windows reset the socket, and the answer would be lost.
                    let (mut got, mut buf) = (Vec::new(), [0; 1024]);
                    while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                        match s.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => got.extend_from_slice(&buf[..n]),
                        }
                    }
                    let body = list.to_string();
                    _ = write!(s, "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{body}", body.len());
                    _ = s.shutdown(std::net::Shutdown::Write);
                    continue;
                }
                let answers = answers.clone();
                tokio::spawn(async move {
                    let mut w = tokio_tungstenite::accept_async(s).await.unwrap();
                    while let Some(Ok(Message::Text(t))) = w.next().await {
                        let m: Value = aio::serde_json::from_str(&t).unwrap();
                        let ev = json!({ "method": "Page.frameNavigated", "params": { "n": m["id"] } });
                        w.send(Message::text(ev.to_string())).await.unwrap();
                        let mut a = answers.get(m["method"].as_str().unwrap()).cloned().unwrap_or(json!({}));
                        for e in a
                            .as_object_mut()
                            .and_then(|a| a.remove("$events"))
                            .into_iter()
                            .flat_map(|e| e.as_array().cloned().unwrap_or_default())
                        {
                            w.send(Message::text(e.to_string())).await.unwrap();
                        }
                        let reply = match a.get("error") {
                            Some(e) => json!({ "id": m["id"], "error": { "message": e } }),
                            None => json!({ "id": m["id"], "result": a }),
                        };
                        w.send(Message::text(reply.to_string())).await.unwrap();
                    }
                });
            }
        });
        port
    }

    /// A headless Chromium showing one page (a real engine for the verbs
    /// that run in a window), ended when dropped; `None` without one.
    #[cfg(unix)]
    pub(crate) struct Chromium {
        browser: aio::testing::TestBrowser,
        _dir: tempfile::TempDir,
    }

    #[cfg(unix)]
    impl Chromium {
        pub(crate) fn open(html: &str) -> Option<Self> {
            let bin = aio::testing::TestBrowser::find().ok()?;
            let dir = tempfile::tempdir().unwrap();
            let page = dir.path().join("page.html");
            std::fs::write(&page, format!("<!doctype html><html><body>{html}</body></html>")).unwrap();
            let browser = aio::testing::TestBrowser::launch(&bin, &[&format!("file://{}", page.display())]).ok()?;
            Some(Self { browser, _dir: dir })
        }

        /// A session with its page, once the page loaded.
        pub(crate) async fn session(&self) -> Session {
            let port = self.browser.port();
            for _ in 0..100 {
                let list = tokio::task::spawn_blocking(move || targets(port)).await.unwrap().unwrap_or_default();
                let page = list
                    .iter()
                    .find(|t| t["type"] == "page" && t["url"].as_str().is_some_and(|u| u.starts_with("file:")));
                if let Some(p) = page {
                    let mut s = Session::connect(p["webSocketDebuggerUrl"].as_str().unwrap()).await.unwrap();
                    // The target can say file: while the blank page before it is
                    // still the one shown: wait for the file's own document.
                    for _ in 0..200 {
                        let at = s.eval("location.protocol + document.readyState").await.unwrap_or_default();
                        if at == "file:complete" {
                            return s;
                        }
                        tokio::time::sleep(Duration::from_millis(25)).await;
                    }
                    panic!("Chromium never loaded the page");
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            panic!("Chromium shows no page: {:?}", tokio::task::spawn_blocking(move || targets(port)).await.unwrap())
        }
    }

    #[test]
    fn a_launch_with_cdp_keeps_its_port_for_the_verbs_that_reach_the_window() {
        let s = |v: &[&str]| v.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        assert_eq!(take_flag(&s(&["--x", "--cdp"])), Ok((s(&["--x"]), Some(0))), "any free port");
        assert_eq!(take_flag(&s(&["--cdp=9222", "a.md"])), Ok((s(&["a.md"]), Some(9222))));
        assert_eq!(take_flag(&s(&["--", "--cdp"])), Ok((s(&["--", "--cdp"]), None)), "the app's own word");
        assert_eq!(take_flag(&s(&["--cdpx"])), Ok((s(&["--cdpx"]), None)));
        assert!(take_flag(&s(&["--cdp=0"])).unwrap_err().contains("not a port"));
        let d = tempfile::tempdir().unwrap();
        let plan = aio::paths::Plan {
            data: d.path().join("data"),
            logs: d.path().join("logs"),
            store: d.path().join("data").join("state.db"),
            cache: d.path().join("cache"),
            profile: None,
            port_of: None,
            from: "default",
            asked: false,
        };
        let p = open(&plan.logs, Some(0)).unwrap().unwrap();
        assert!(p > 0);
        assert_eq!(recorded(&plan), None, "a stopped app's port is no answer");
        assert_eq!(door("notes", &plan), None, "a stopped app has no door");
        {
            let _running = aio::paths::AppDirs::at(d.path(), "notes").unwrap();
            let ui = door("notes", &plan).unwrap();
            assert!(ui.contains("notes") && ui.ends_with(if cfg!(windows) { "-ui" } else { "ui.sock" }), "{ui}");
            assert_eq!(recorded(&plan), Some(p));
            assert_eq!(reach(None, recorded(&plan), door("notes", &plan)), Ok(Reach::Port(p)), "found with no flag");
            assert_eq!(reach(Some("9333"), recorded(&plan), None), Ok(Reach::Port(9333)), "the flag wins");
            assert_eq!(open(&plan.logs, None), Ok(None));
            assert_eq!(recorded(&plan), None, "a launch without --cdp forgets it");
            assert_eq!(
                reach(None, recorded(&plan), door("notes", &plan)),
                Ok(Reach::Door(ui)),
                "no flag: its own door"
            );
        }
        assert!(reach(None, None, None).unwrap_err().contains("not running: start it with `ax start <app>`"));
        assert!(reach(Some("x"), None, None).unwrap_err().contains("not a port"));
    }

    /// A fake `ui` door (as `aio_shell::inspect` serves it): each line is
    /// answered from `answers` as [`endpoint`] does, after an event.
    pub(crate) async fn door_endpoint(answers: Answers) -> (Reach, tempfile::TempDir) {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let d = tempfile::tempdir().unwrap();
        let addr = aio::ipc::addr(d.path(), aio_shell::inspect::DOOR);
        let mut l = aio::ipc::Listener::bind(&addr).unwrap();
        tokio::spawn(async move {
            while let Ok(s) = l.accept().await {
                let answers = answers.clone();
                tokio::spawn(async move {
                    let (r, mut w) = tokio::io::split(s);
                    let mut lines = tokio::io::BufReader::new(r).lines();
                    while let Ok(Some(t)) = lines.next_line().await {
                        let m: Value = aio::serde_json::from_str(&t).unwrap();
                        let ev = json!({ "method": "Page.frameNavigated", "params": { "n": m["id"] } });
                        let a = answers.get(m["method"].as_str().unwrap()).cloned().unwrap_or(json!({}));
                        let reply = match a.get("error") {
                            Some(e) => json!({ "id": m["id"], "error": { "message": e } }),
                            None => json!({ "id": m["id"], "result": a }),
                        };
                        w.write_all(format!("{ev}\n{reply}\n").as_bytes()).await.unwrap();
                    }
                });
            }
        });
        (Reach::Door(addr), d)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_running_apps_own_door_reaches_its_window_with_no_flag() {
        let answers = Answers::from([
            ("Page.navigate".into(), json!({ "error": "Cannot navigate" })),
            ("Runtime.evaluate".into(), json!({ "result": { "value": { "title": "Notes", "url": "aio://app/" } } })),
        ]);
        let (at, _d) = door_endpoint(answers).await;
        let mut s = window(&at, 0).await.unwrap();
        assert_eq!(s.call("Page.navigate", json!({})).await.unwrap_err(), "CDP Page.navigate: Cannot navigate");
        assert_eq!(s.eval("document.title").await.unwrap()["title"], "Notes");
        assert_eq!(s.event("Page.frameNavigated", WAIT).await.unwrap().unwrap()["n"], 1, "events kept as over a port");
        assert_eq!(windows(&at).await.unwrap(), "0  Notes  aio://app/", "ax clients <app>");
        assert_eq!(window(&at, 1).await.err().unwrap(), "no window 1: the app has 1 (0)");
        let gone = Reach::Door(aio::ipc::addr(_d.path(), "gone"));
        assert!(window(&gone, 0).await.err().unwrap().contains("has no window to reach"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_session_matches_answers_to_calls_and_keeps_the_events_between() {
        let answers = Answers::from([
            ("Browser.getVersion".into(), json!({ "product": "Chrome/1" })),
            ("Page.navigate".into(), json!({ "error": "Cannot navigate" })),
            ("Runtime.evaluate".into(), json!({ "result": { "value": 2 } })),
        ]);
        let port = endpoint(answers).await;
        let list = tokio::task::spawn_blocking(move || targets(port)).await.unwrap().unwrap();
        let pages = app_pages(&list);
        assert_eq!(pages.len(), 1, "only the app's own window: {list:?}");
        let mut s = Session::connect(pages[0]["webSocketDebuggerUrl"].as_str().unwrap()).await.unwrap();
        assert_eq!(s.call("Browser.getVersion", json!({})).await.unwrap()["product"], "Chrome/1");
        assert_eq!(s.call("Page.navigate", json!({})).await.unwrap_err(), "CDP Page.navigate: Cannot navigate");
        assert_eq!(s.eval("1+1").await.unwrap(), 2);
        let first = s.event("Page.frameNavigated", WAIT).await.unwrap().unwrap();
        assert_eq!(first["n"], 1, "heard during a call, kept in order");
        assert_eq!(s.event("Page.loadEventFired", Duration::from_millis(50)).await, Ok(None), "none came");
        assert!(tokio::task::spawn_blocking(|| targets(1)).await.unwrap().unwrap_err().contains("AIORT_CDP=1"));
        assert_eq!(windows(&Reach::Port(port)).await.unwrap(), "0    aio://app/", "only the app's own");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_window_is_waited_for_until_it_appears_or_the_time_is_up() {
        // aio --wait-client: an app mid-start has no endpoint for a moment.
        let port = crate::dev::free_port();
        let at = Reach::Port(port);
        // Before the window comes (Windows takes ~2 s to refuse a connect).
        assert!(window(&at, 0).await.is_err(), "not there yet");
        let late = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            endpoint_on(tokio::net::TcpListener::bind(("127.0.0.1", port)).await.unwrap(), Answers::new())
        });
        assert!(window_within(&at, 0, WAIT).await.is_ok(), "waited for");
        late.await.unwrap();
        let t = tokio::time::Instant::now();
        let e = window_within(&at, 1, Duration::from_millis(300)).await.err().unwrap();
        assert!(t.elapsed() >= Duration::from_millis(300) && e.contains("no window 1: the app has 1"), "{e}");
    }
}

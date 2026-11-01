//! The shell's guests in the real runtime: the shared Electron, headless
//! (no display, and no session bus to reach the desktop through), running
//! this crate's own main.js, preload and page, with a view of `Browser`s
//! served over the wire as `run` serves it, and the guests' pages from a
//! local HTTP server. The page is read and driven over the shell's own
//! DevTools line (`{"t":"cdp"}`). Skipped where the runtime is not installed.
use crate::{Window, guest, install, runtime, wire};
use aio::kit::{Browser, Go, GuestEvent, go};
use aio::serde_json::{Value, json};
use aio::ui::prelude::*;
use dioxus_core::VirtualDom;
use dioxus_liveview::LiveViewPool;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

/// The guests' server, `http://127.0.0.1:<port>`.
static BASE: OnceLock<String> = OnceLock::new();

/// A declared preload: says it loaded, answers the app's messages.
const PROBE: &str = "const { ipcRenderer } = require('electron');
window.addEventListener('DOMContentLoaded', () => ipcRenderer.sendToHost('hello', location.pathname));
ipcRenderer.on('ping', (_e, x) => ipcRenderer.sendToHost('pong', x));
ipcRenderer.on('go', (_e, u) => { location.href = u; });
ipcRenderer.on('open', () => window.open('https://example.invalid/'));
ipcRenderer.on('perm', () => navigator.permissions.query({ name: 'clipboard-read' })
  .then((s) => ipcRenderer.sendToHost('perm', s.state), (e) => ipcRenderer.sendToHost('perm', String(e))));";

static PRELOADS: [(&str, &str); 1] = [("probe.js", PROBE)];

fn app() -> View {
    let base = BASE.get().cloned().unwrap_or_default();
    let src = signal(|| format!("{base}/a"));
    let at = signal(String::new);
    let on = signal(|| true);
    let log = signal(Vec::<String>::new);
    let send = |channel: &str, args: Vec<Value>| go("b", Go::Send { channel: channel.into(), args });
    let c = format!("{base}/c");
    let b = format!("{base}/b");
    rsx! {
        button { id: "hide", onclick: move |_| on.toggle(), "hide" }
        button { id: "follow", onclick: move |_| src.set(at.get()), "follow" }
        button { id: "to-b", onclick: move |_| src.set(b.clone()), "b" }
        button { id: "ping", onclick: move |_| send("ping", vec![json!(7)]), "ping" }
        button { id: "go-c", onclick: move |_| send("go", vec![json!(c.clone())]), "c" }
        button { id: "perm", onclick: move |_| send("perm", vec![]), "perm" }
        button { id: "back", onclick: move |_| go("b", Go::Back), "back" }
        button { id: "open", onclick: move |_| send("open", vec![]), "open" }
        if on.get() {
            Browser {
                id: "b",
                src: src.get(),
                keep_alive: "k",
                partition: "persist:t",
                preload: "probe.js",
                host_keys: vec!["Escape".into()],
                popups: true,
                style: "width:400px;height:300px",
                onevent: move |e: GuestEvent| {
                    if let GuestEvent::Navigated { url, .. } = &e {
                        at.set(url.clone());
                    }
                    log.write().push(format!("{e:?}"));
                },
            }
        }
        // No partition, an undeclared preload: the shell's own session and
        // no preload, both said. Its first load sends the given Referer.
        Browser {
            id: "d",
            src: format!("{base}/d"),
            preload: "nope.js",
            referrer: "http://ref.example/",
            style: "width:100px;height:100px",
        }
        pre { id: "log", {log.get().join("\n")} }
    }
}

/// Serve `/<path>` as a page titled `path`; every request's path is told.
async fn serve(seen: Arc<Mutex<Vec<String>>>) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            let seen = seen.clone();
            tokio::spawn(async move {
                let mut r = tokio::io::BufReader::new(&mut s);
                let mut line = String::new();
                _ = r.read_line(&mut line).await;
                let path = line.split(' ').nth(1).unwrap_or("/").to_string();
                seen.lock().unwrap().push(path.clone());
                // Its Referer, as `<path> from <referer>`.
                loop {
                    line.clear();
                    if r.read_line(&mut line).await.unwrap_or(0) == 0 || line.trim().is_empty() {
                        break;
                    }
                    if let Some(from) = line.to_ascii_lowercase().strip_prefix("referer:") {
                        seen.lock().unwrap().push(format!("{path} from {}", from.trim()));
                    }
                }
                let body = format!("<!doctype html><title>{path}</title><p>{path}</p>");
                let head = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                _ = s.write_all(format!("{head}{body}").as_bytes()).await;
            });
        }
    });
    base
}

/// The runtime under test: its control line, its stderr.
struct Shell {
    child: tokio::process::Child,
    ctl: tokio::io::WriteHalf<aio::ipc::Stream>,
    heard: tokio::sync::mpsc::UnboundedReceiver<Value>,
    said: Arc<Mutex<Vec<String>>>,
    next: u64,
}

impl Shell {
    /// Send `m` (given the next id) and wait for its answer of type `t`.
    async fn ask(&mut self, mut m: Value, t: &str) -> Value {
        self.next += 1;
        m["id"] = json!(self.next);
        self.ctl.write_all(format!("{m}\n").as_bytes()).await.unwrap();
        loop {
            let v = tokio::time::timeout(Duration::from_secs(10), self.heard.recv()).await.unwrap().unwrap();
            if v["t"] == t && v["id"] == self.next {
                return v;
            }
        }
    }

    /// `expr`'s value in the app's page, as a person's gesture.
    async fn eval(&mut self, expr: &str) -> Value {
        let p = json!({ "expression": expr, "returnByValue": true, "awaitPromise": true, "userGesture": true });
        let v = self.ask(json!({ "t": "cdp", "method": "Runtime.evaluate", "params": p }), "cdp").await;
        assert!(v["error"].is_null() && v["result"]["exceptionDetails"].is_null(), "{expr}: {v}");
        v["result"]["result"]["value"].clone()
    }

    /// Wait until `expr` is true in the page (one that throws is not yet).
    async fn until(&mut self, expr: &str) {
        let t = format!("(() => {{ try {{ return {expr}; }} catch (e) {{ return false; }} }})()");
        for _ in 0..200 {
            if self.eval(&t).await == json!(true) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let log = self.eval("document.getElementById('log') && document.getElementById('log').textContent").await;
        panic!("never true: {expr}\nlog: {log}\nstderr: {}", self.said.lock().unwrap().join("\n"));
    }

    fn said(&self, what: &str) -> bool {
        self.said.lock().unwrap().iter().any(|l| l.contains(what))
    }
}

/// How many `Message { channel: "<c>"` lines the view's log has.
fn count(channel: &str) -> String {
    format!("document.getElementById('log').textContent.split('channel: \"{channel}\"').length - 1")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_browser_shows_another_site_in_its_own_session_and_the_shell_guards_it() {
    let rt = runtime::dir();
    let exe = runtime::exe(&rt);
    if !exe.exists() {
        return eprintln!("skipped: no Electron runtime in {}", rt.display());
    }
    let d = tempfile::tempdir_in(aio::paths::short_temp()).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    _ = BASE.set(serve(seen.clone()).await);
    let base = BASE.get().unwrap().clone();
    let win = Window {
        title: "guests",
        web: guest::Web { browser: true, windows: true, preloads: &PRELOADS, ..Default::default() },
        permissions: &["clipboard-read"],
        ..Default::default()
    };
    let dir = d.path().join("shell");
    install(&dir, "guests", &win).unwrap();
    let (sock, ctl_sock) = (aio::ipc::addr(d.path(), "wire"), aio::ipc::addr(d.path(), "ctl"));
    let mut wires = aio::ipc::Listener::bind(&sock).unwrap();
    let mut ctls = aio::ipc::Listener::bind(&ctl_sock).unwrap();
    let mut cmd = tokio::process::Command::new(&exe);
    cmd.arg(&dir)
        .args(runtime::sandbox_args(&rt))
        .args(["--ozone-platform=headless", "--aio-title=guests", "--aio-width=800", "--aio-height=600"])
        // A link a guest opens goes nowhere (never the person's browser).
        .arg("--aio-no-open=1")
        .arg(format!("--user-data-dir={}", d.path().join("profile").display()))
        .arg(format!("--aio-sock={sock}"))
        .arg(format!("--aio-ctl={ctl_sock}"))
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        // Chromium's own sockets (SingletonSocket) beside ours, as short.
        .env("TMPDIR", d.path())
        // No session bus: nothing (a portal, a11y) reaches the desktop.
        .env("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={}", d.path().join("no-bus").display()))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn().unwrap();
    let said = Arc::new(Mutex::new(Vec::new()));
    {
        let (said, err) = (said.clone(), child.stderr.take().unwrap());
        tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(err).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                said.lock().unwrap().push(l);
            }
        });
    }
    let s = tokio::time::timeout(Duration::from_secs(20), ctls.accept()).await.unwrap().unwrap();
    let (r, mut ctl) = tokio::io::split(s);
    let (tx, heard) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(r).lines();
        while let Ok(Some(l)) = lines.next_line().await {
            if let Ok(v) = aio::serde_json::from_str::<Value>(&l) {
                _ = tx.send(v);
            }
        }
    });
    ctl.write_all(b"{\"t\":\"place\",\"width\":800,\"height\":600}\n").await.unwrap();
    let s = tokio::time::timeout(Duration::from_secs(20), wires.accept()).await.unwrap().unwrap();
    let pool = Arc::new(LiveViewPool::new());
    tokio::spawn(async move { _ = pool.launch_virtualdom(wire(s), || VirtualDom::new(app)).await });
    let mut sh = Shell { child, ctl, heard, said, next: 0 };

    // The guest loads its page with the declared preload, and says where it landed.
    sh.until(&format!("{} === 1", count("hello"))).await;
    sh.until(&format!("document.getElementById('log').textContent.includes('Navigated {{ url: \"{base}/a\"')")).await;
    // Undeclared: refused and said; no partition: the guests' own session.
    sh.until("document.querySelectorAll('webview').length === 2").await;
    for _ in 0..100 {
        if sh.said("preload REFUSED") && sh.said("persist:aio-webview") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        sh.said("<Browser> preload REFUSED: \"file:///aio-guest-preload/nope.js\" is not a declared guest preload")
    );
    assert!(sh.said("gets the session \"persist:aio-webview\""));

    // Messages both ways: the app's to its preload, the preload's back.
    sh.eval("document.getElementById('ping').click()").await;
    sh.until("document.getElementById('log').textContent.includes('channel: \"pong\", args: [Number(7)]')").await;

    // Host keys: a declared keydown comes back with its modifiers; an
    // undeclared one never leaves the shell.
    let key = |k: &str| {
        format!(
            "document.querySelector('#b webview').sendInputEvent({{ type: 'keyDown', keyCode: '{k}', modifiers: ['shift'] }})"
        )
    };
    for _ in 0..100 {
        sh.eval(&key("a")).await;
        sh.eval(&key("Escape")).await;
        if sh.eval("document.getElementById('log').textContent.includes('Key(HostKey')").await == json!(true) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    sh.until("document.getElementById('log').textContent.includes('Key(HostKey { key: \"Escape\", code: \"Escape\", ctrl: false, shift: true')").await;
    assert_eq!(sh.eval("document.getElementById('log').textContent.includes('key: \"a\"')").await, json!(false));

    // Its own site's permissions: none for the guest, clipboard-read for
    // the app's page, which declared it.
    sh.eval("document.getElementById('perm').click()").await;
    sh.until("document.getElementById('log').textContent.includes('channel: \"perm\", args: [String(\"denied\")]')")
        .await;
    assert_eq!(
        sh.eval("navigator.permissions.query({ name: 'clipboard-read' }).then((s) => s.state)").await,
        "granted"
    );

    // The guest moves itself: told.
    sh.eval("document.getElementById('go-c').click()").await;
    sh.until(&format!("{} === 2", count("hello"))).await;
    sh.until(&format!("document.getElementById('log').textContent.includes('Navigated {{ url: \"{base}/c\"')")).await;
    // Unmounted and back (keep_alive): the new guest opens where the last
    // one was, and a re-render with the same src never sends it back.
    sh.eval("document.getElementById('hide').click()").await;
    sh.until("!document.querySelector('#b')").await;
    sh.eval("document.getElementById('hide').click()").await;
    sh.until(&format!("{} === 3", count("hello"))).await;
    let loads = |p: &str| seen.lock().unwrap().iter().filter(|s| *s == p).count();
    assert_eq!((loads("/a"), loads("/c")), (1, 2), "reopened at /c, not /a");
    sh.eval("document.getElementById('ping').click()").await;
    sh.until(&format!("{} === 2", count("pong"))).await;
    // The address bar writes its landing back into src: no load.
    sh.eval("document.getElementById('follow').click()").await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(sh.eval(&count("hello")).await, json!(3), "a src where it already is loads nothing");
    // A changed src navigates.
    sh.eval("document.getElementById('to-b').click()").await;
    sh.until(&format!("{} === 4", count("hello"))).await;
    assert_eq!(loads("/b"), 1);
    assert_eq!(loads("/d from http://ref.example/"), 1, "the given Referer");
    // With a way back, which Go::Back takes.
    sh.until(&format!(
        "document.getElementById('log').textContent.includes('Navigated {{ url: \"{base}/b\", back: true')"
    ))
    .await;
    let at_c = format!("document.getElementById('log').textContent.split('Navigated {{ url: \"{base}/c\"').length - 1");
    let before = sh.eval(&at_c).await.as_u64().unwrap();
    sh.eval("document.getElementById('back').click()").await;
    sh.until(&format!("{at_c} === {}", before + 1)).await;

    // A child window: opened on an http page with a declared preload, in a
    // session of its own; refused, with the reason, otherwise.
    let ok = guest::child_line(
        0,
        &guest::Child { url: format!("{base}/child"), preload: Some("probe.js".into()), ..Default::default() },
    );
    let ok: Value = aio::serde_json::from_str(&ok.unwrap()).unwrap();
    assert!(sh.ask(ok.clone(), "opened").await["error"].is_null());
    for _ in 0..100 {
        if loads("/child") == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(loads("/child"), 1);
    let no = sh.ask(json!({ "t": "child", "url": format!("{base}/x"), "preload": "nope.js" }), "opened").await;
    assert!(no["error"].as_str().unwrap().contains("preload refused"), "{no}");
    let own = sh.ask(json!({ "t": "child", "url": format!("{base}/x"), "partition": "persist:" }), "opened").await;
    assert!(own["error"].as_str().unwrap().contains("the app's own session"), "{own}");

    // A guest's pop-up: blocked without real input in it, said; after a
    // click, an http(s) link for the system browser (none in this run).
    sh.eval("document.getElementById('open').click()").await;
    for _ in 0..100 {
        if sh.said("pop-up BLOCKED") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(sh.said("pop-up BLOCKED from http://127.0.0.1"), "{}", sh.said.lock().unwrap().join("\n"));
    assert!(sh.said("no click or key in that page in the last 5 s"));
    sh.eval("const w = document.querySelector('#b webview'); w.sendInputEvent({ type: 'mouseDown', x: 5, y: 5, button: 'left', clickCount: 1 }); w.sendInputEvent({ type: 'mouseUp', x: 5, y: 5, button: 'left', clickCount: 1 }); true").await;
    sh.eval("document.getElementById('open').click()").await;
    for _ in 0..100 {
        if sh.said("not opened (AIORT_NO_OPEN") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(sh.said("not opened (AIORT_NO_OPEN, a contained run): https://example.invalid/"));

    // A page's beforeunload veto keeps the window on the person's close
    // (said once); the app's own quit leaves past it.
    sh.eval("window.addEventListener('beforeunload', (e) => { e.preventDefault(); e.returnValue = ''; }); true").await;
    sh.eval("window.close(); true").await;
    for _ in 0..100 {
        if sh.said("the page vetoed leaving") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(sh.said("the page vetoed leaving"), "{}", sh.said.lock().unwrap().join("\n"));
    assert!(sh.child.try_wait().unwrap().is_none(), "the window stayed");
    // The page's policy is strict (no eval, no inline script but its own),
    // so the runtime warns of none, and the page refused nothing of its own.
    assert!(
        !sh.said("Insecure Content-Security-Policy") && !sh.said("Refused to"),
        "{}",
        sh.said.lock().unwrap().join("\n")
    );
    sh.ctl.write_all(b"quit\n").await.unwrap();
    let st = tokio::time::timeout(Duration::from_secs(10), sh.child.wait()).await.expect("quit past the veto").unwrap();
    assert!(st.success(), "{st}");
}

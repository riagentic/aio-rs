// The browser `TestUi` draws in: the pinned Electron runtime (the engine
// the app's window runs), headless, one per test binary, a hidden page per
// TestUi. It runs `electron.js` from a folder of its own (removed when it
// quits) and is driven over its stdin and stdout, so no port is opened and
// nothing but the standard library carries it. The test process ending
// closes its stdin, which quits it.
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::Duration;

/// How long the browser may take to start, or to answer one call (a cold
/// start on a loaded machine).
const WAIT: Duration = Duration::from_secs(30);

/// Lines of the browser's own stderr kept, for a failure to show.
const TAIL: usize = 20;

type Answer = Result<Value, String>;

/// What a page asked of a host a test answers ([`super::ui::Mount::fetch`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asked {
    pub method: String,
    pub url: String,
    /// A POST's or PUT's body; empty for none.
    pub body: String,
}

/// A test's answer to one of those requests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answered {
    pub status: u16,
    /// Its `Content-Type`.
    pub kind: String,
    pub body: Vec<u8>,
}

impl Answered {
    pub fn new(status: u16, kind: &str, body: impl Into<Vec<u8>>) -> Self {
        Self { status, kind: kind.into(), body: body.into() }
    }
}

/// A test's answers to the page's requests to one host.
pub(crate) type Stub = Arc<dyn Fn(&Asked) -> Answered + Send + Sync>;
/// Shared by the caller and the reader thread, per call or page id.
type ById<T> = Arc<Mutex<HashMap<u64, T>>>;

pub(crate) struct Browser {
    stdin: Mutex<std::process::ChildStdin>,
    asks: ById<mpsc::Sender<Answer>>,
    /// Each page's warnings and errors, newest last.
    console: ById<Vec<String>>,
    /// Each page's requests to the network, refused, not taken yet.
    refused: ById<Vec<String>>,
    /// Each page's answered hosts, with their answers.
    stubs: ById<Vec<(String, Stub)>>,
    stderr: Arc<Mutex<VecDeque<String>>>,
    next: AtomicU64,
    _child: Arc<Mutex<std::process::Child>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The runtime to drive: `AIORT_RUNTIME` (a runtime folder, as for an
/// app), else the pinned version in the shared store.
fn runtime() -> PathBuf {
    let dir = match std::env::var_os("AIORT_RUNTIME") {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => crate::paths::electron_pinned(),
    };
    crate::paths::electron_exe_for(std::env::consts::OS, &dir)
}

/// The flags the browser starts with (pure): no sandbox (it shows only
/// the test page), WebGL drawn on the CPU by SwiftShader (as `ax dev`'s
/// nested display: a canvas app's 3D is tested and shot as it draws) but
/// the page composited in software (through SwiftShader every test ran
/// about 10 times slower: an app's suite 6 s → 73 s), frames as fast as
/// they are drawn, not at a screen's pace (a hidden
/// window otherwise draws about once a second; measured with 30 pages at
/// once: 2.6 ms to a frame), and on Linux no display at all.
fn flags(os: &str) -> Vec<&'static str> {
    let mut f = vec!["--no-sandbox", "--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--disable-gpu-compositing", "--disable-gpu-vsync", "--disable-frame-rate-limit"];
    if os == "linux" {
        f.push("--ozone-platform=headless");
    }
    f
}

/// This test binary's browser, started on first use; a machine without
/// the runtime fails the test, saying how to get it.
pub(crate) fn browser() -> Arc<Browser> {
    static ONE: OnceLock<Result<Arc<Browser>, String>> = OnceLock::new();
    ONE.get_or_init(|| start(page()).map(Arc::new)).clone().unwrap_or_else(|e| panic!("{e}"))
}

/// The test page: the window's own body (`aio::page`) with the test's bridge.
fn page() -> String {
    let page = format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>test</title></head><body>{}</body></html>"#,
        crate::page::body(&format!("{}\nwindow.__aioTest = {};", super::ui::BRIDGE, crate::ui::SURFACE_JS))
    );
    crate::page::sealed(&page, "'none'")
}

/// The start of each browser's folder name; the test process's pid follows.
const LEFT: &str = "aio-test-browser-";

/// The folder of a browser whose test process `name` (a folder's name) is
/// gone (pure, given `alive`): a browser quits when its test does.
fn stale(name: &str, alive: &dyn Fn(u32) -> bool) -> bool {
    let pid = name.strip_prefix(LEFT).and_then(|r| r.split('-').next()).and_then(|p| p.parse().ok());
    pid.is_some_and(|p| !alive(p))
}

/// Take away the folders of browsers that are gone: on Windows a browser
/// can't remove its own (its runtime still holds files in it as it exits).
fn sweep(tmp: &std::path::Path) {
    let alive = |pid| crate::lifecycle::started(pid).is_some();
    for e in std::fs::read_dir(tmp).into_iter().flatten().flatten() {
        if e.file_name().to_str().is_some_and(|n| stale(n, &alive)) {
            _ = std::fs::remove_dir_all(e.path());
        }
    }
}

fn start(page: String) -> Result<Browser, String> {
    let exe = runtime();
    if !exe.is_file() {
        return Err(format!(
            "[TestUi] UI tests draw in the Electron {} runtime the app's window runs, and there is none at {}: \
             run the app once (it fetches the runtime) or set AIORT_RUNTIME to a runtime folder",
            crate::paths::ELECTRON,
            exe.display()
        ));
    }
    let tmp = std::env::temp_dir();
    sweep(&tmp);
    let dir = tmp.join(format!("{LEFT}{}-{}", std::process::id(), crate::uuid_v4().simple()));
    let files = [
        ("main.js", include_str!("electron.js")),
        ("package.json", r#"{"name":"aio-test-browser","main":"main.js"}"#),
        ("page.html", page.as_str()),
    ];
    std::fs::create_dir_all(&dir)
        .and_then(|_| files.iter().try_for_each(|(f, s)| std::fs::write(dir.join(f), s)))
        .map_err(|e| format!("[TestUi] {}: {e}", dir.display()))?;
    // A HOME of its own in its folder: Chromium's NSS store, caches and
    // crash settings never reach the person's home (`TestBrowser`'s rule).
    let home = super::browser::own_home(&dir, std::env::consts::OS, &|k| std::env::var(k).ok())
        .inspect_err(|_| _ = std::fs::remove_dir_all(&dir))?;
    let spawned = std::process::Command::new(&exe)
        .arg(&dir)
        .args(flags(std::env::consts::OS))
        .envs(home)
        .env_remove("ELECTRON_RUN_AS_NODE")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => {
            _ = std::fs::remove_dir_all(&dir);
            return Err(format!("[TestUi] {} did not start: {e}", exe.display()));
        }
    };
    let (stdin, stdout, stderr) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
    let (Some(stdin), Some(stdout), Some(stderr)) = (stdin, stdout, stderr) else {
        return Err("[TestUi] the browser's pipes".into());
    };
    let child = Arc::new(Mutex::new(child));
    let tail: Arc<Mutex<VecDeque<String>>> = Arc::default();
    let t = tail.clone();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stderr).lines().map_while(Result::ok) {
            let mut t = lock(&t);
            if t.len() == TAIL {
                t.pop_front();
            }
            t.push_back(line);
        }
    });
    let asks: ById<mpsc::Sender<Answer>> = Default::default();
    let (console, refused): (ById<Vec<String>>, ById<Vec<String>>) = Default::default();
    let stubs: ById<Vec<(String, Stub)>> = Default::default();
    let (ready, up) = mpsc::channel();
    let (a, c, t, gone, r) = (asks.clone(), console.clone(), tail.clone(), child.clone(), refused.clone());
    let st = stubs.clone();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines().map_while(Result::ok) {
            let Ok(m) = serde_json::from_str::<Value>(&line) else { continue };
            if m["ready"] == true {
                _ = ready.send(());
            } else if let Some(id) = m["id"].as_u64() {
                let answer = match m["error"].as_str() {
                    Some(e) => Err(e.to_string()),
                    None => Ok(m["result"].clone()),
                };
                if let Some(to) = lock(&a).remove(&id) {
                    _ = to.send(answer);
                }
            } else if let (Some(p), Some(s)) = (m["page"].as_u64(), m["console"].as_str()) {
                lock(&c).entry(p).or_default().push(s.to_string());
            } else if let (Some(p), Some(u)) = (m["page"].as_u64(), m["refused"].as_str()) {
                lock(&r).entry(p).or_default().push(u.to_string());
            } else if let Some(p) = m["page"].as_u64().filter(|_| m["paused"].is_object()) {
                let stub = lock(&st).get(&p).and_then(|ss| {
                    let host = host_of(m["paused"]["request"]["url"].as_str().unwrap_or_default());
                    ss.iter().find(|(h, _)| *h == host).map(|(_, s)| s.clone())
                });
                // Off this thread: the answer is a call, and this thread reads its reply.
                let paused = m["paused"].clone();
                std::thread::spawn(move || answer(p, &paused, stub));
            }
        }
        // It went: every call waiting fails, saying how and what it said last.
        let code = (0..20)
            .find_map(|_| {
                let s = lock(&gone).try_wait().ok().flatten();
                s.is_none().then(|| std::thread::sleep(Duration::from_millis(50)));
                s
            })
            .map_or("still running, its output closed".into(), |s| s.to_string());
        let why = format!("the test browser exited ({code}): {}", Vec::from(lock(&t).clone()).join("\n"));
        lock(&a).drain().for_each(|(_, to)| _ = to.send(Err(why.clone())));
    });
    if up.recv_timeout(WAIT).is_err() {
        _ = lock(&child).kill();
        _ = lock(&child).wait();
        _ = std::fs::remove_dir_all(&dir);
        let said = Vec::from(lock(&tail).clone()).join("\n");
        return Err(format!("[TestUi] {} did not come up in {WAIT:?}: {said}", exe.display()));
    }
    Ok(Browser {
        stdin: Mutex::new(stdin),
        asks,
        console,
        refused,
        stubs,
        stderr: tail,
        next: AtomicU64::new(0),
        _child: child,
    })
}

/// The host of `url` (`https://api.example.org:8443/x`: `api.example.org`; pure).
pub(crate) fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let auth = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let auth = auth.rsplit_once('@').map_or(auth, |(_, h)| h);
    if auth.starts_with('[') { auth.split_inclusive(']').next().unwrap_or(auth) } else { auth.split(':').next().unwrap_or(auth) }
}

/// The `Fetch.fulfillRequest` of a paused request with the stub's answer
/// (pure), readable from any origin (the test page is `aio://app`).
fn fulfil(request: &str, a: &Answered) -> Value {
    use base64::Engine;
    json!({
        "requestId": request,
        "responseCode": a.status,
        "responseHeaders": [
            { "name": "content-type", "value": a.kind },
            { "name": "access-control-allow-origin", "value": "*" },
        ],
        "body": base64::engine::general_purpose::STANDARD.encode(&a.body),
    })
}

/// Answer page `page`'s paused request with `stub`; with none, let it go on.
fn answer(page: u64, paused: &Value, stub: Option<Stub>) {
    let request = paused["requestId"].as_str().unwrap_or_default();
    let (method, params) = match stub {
        Some(stub) => {
            let r = &paused["request"];
            let asked = Asked {
                method: r["method"].as_str().unwrap_or("GET").into(),
                url: r["url"].as_str().unwrap_or_default().into(),
                body: r["postData"].as_str().unwrap_or_default().into(),
            };
            ("Fetch.fulfillRequest", fulfil(request, &stub(&asked)))
        }
        None => ("Fetch.continueRequest", json!({ "requestId": request })),
    };
    _ = browser().call(json!({ "page": page, "method": method, "params": params }));
}

impl Browser {
    /// Send `m` (an id added) and wait for its answer.
    fn call(&self, mut m: Value) -> Answer {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        m["id"] = id.into();
        let (to, from) = mpsc::channel();
        lock(&self.asks).insert(id, to);
        let sent = writeln!(lock(&self.stdin), "{m}");
        if let Err(e) = sent {
            lock(&self.asks).remove(&id);
            return Err(format!("the test browser is gone ({e}): {}", self.said()));
        }
        from.recv_timeout(WAIT).unwrap_or_else(|_| {
            lock(&self.asks).remove(&id);
            Err(format!("the test browser did not answer in {WAIT:?}: {m}"))
        })
    }

    fn said(&self) -> String {
        Vec::from(lock(&self.stderr).clone()).join("\n")
    }
}

/// One test's page in the browser; dropped, it closes.
pub(crate) struct Page {
    browser: Arc<Browser>,
    id: u64,
}

impl Page {
    /// A page `width` × `height` CSS px showing the test page, reaching
    /// this machine and `network`'s hosts (`*`: any) only.
    pub(crate) fn open(width: u32, height: u32, network: &[String]) -> Self {
        let browser = browser();
        let r = browser.call(json!({ "open": { "width": width, "height": height, "network": network } }));
        let id = r.and_then(|v| v["page"].as_u64().ok_or(format!("no page in {v}"))).unwrap_or_else(|e| panic!("{e}"));
        Self { browser, id }
    }

    /// A DevTools protocol call on the page; a refusal panics, naming it.
    pub(crate) fn cmd(&self, method: &str, params: Value) -> Value {
        self.browser
            .call(json!({ "page": self.id, "method": method, "params": params }))
            .unwrap_or_else(|e| panic!("{method}: {e}"))
    }

    /// `expr`'s value in the page (a promise awaited); a throw panics with it.
    pub(crate) fn eval(&self, expr: &str) -> Value {
        let r = self.cmd("Runtime.evaluate", json!({ "expression": expr, "returnByValue": true, "awaitPromise": true }));
        if let Some(e) = r.get("exceptionDetails") {
            let text = e["exception"]["description"].as_str().or(e["text"].as_str()).unwrap_or("an exception");
            panic!("the test page threw: {text}");
        }
        r["result"]["value"].clone()
    }

    /// The page as a person would see it now, a PNG.
    pub(crate) fn shot(&self) -> Vec<u8> {
        use base64::Engine;
        let r = self.browser.call(json!({ "shot": self.id })).unwrap_or_else(|e| panic!("a screenshot: {e}"));
        base64::engine::general_purpose::STANDARD.decode(r["png"].as_str().unwrap_or_default()).unwrap_or_default()
    }

    /// The page's warnings and errors so far.
    pub(crate) fn console(&self) -> Vec<String> {
        lock(&self.browser.console).get(&self.id).cloned().unwrap_or_default()
    }

    /// Answer the page's requests to each of `stubs`' hosts with its stub,
    /// from now on.
    pub(crate) fn answer(&self, stubs: Vec<(String, Stub)>) {
        if stubs.is_empty() {
            return;
        }
        let patterns: Vec<Value> = stubs.iter().map(|(h, _)| json!({ "urlPattern": format!("*://{h}/*") })).collect();
        lock(&self.browser.stubs).insert(self.id, stubs);
        self.cmd("Fetch.enable", json!({ "patterns": patterns }));
    }

    /// Take the page's requests the network rule refused so far.
    pub(crate) fn refused(&self) -> Vec<String> {
        lock(&self.browser.refused).remove(&self.id).unwrap_or_default()
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        _ = self.browser.call(json!({ "close": self.id }));
        lock(&self.browser.console).remove(&self.id);
        lock(&self.browser.refused).remove(&self.id);
        lock(&self.browser.stubs).remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_folder_of_a_browser_whose_test_is_gone_is_stale() {
        let alive = |p| p == 7;
        assert!(stale("aio-test-browser-9-abc", &alive));
        assert!(!stale("aio-test-browser-7-abc", &alive), "its test still runs");
        assert!(!stale("aio-test-browser-abc", &alive), "no pid: not ours to take");
        assert!(!stale("other-9-abc", &alive));
    }

    /// Its HOME is its own folder's: what Chromium writes there (its NSS
    /// store, caches) never reaches the person's home.
    #[test]
    fn the_browser_writes_in_a_home_of_its_own() {
        let _page = Page::open(10, 10, &[]);
        let mine = format!("{LEFT}{}-", std::process::id());
        let homes: Vec<_> = std::fs::read_dir(std::env::temp_dir())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(&mine))
            .map(|e| e.path().join("aio-home"))
            .collect();
        // On Linux it writes its cache there at once: HOME was taken.
        let wrote = |h: &std::path::PathBuf| if cfg!(target_os = "linux") { h.join(".cache").is_dir() } else { h.is_dir() };
        assert!(cfg!(target_os = "macos") || homes.iter().any(wrote), "{homes:?}");
    }

    #[test]
    fn a_request_s_host_is_read_from_its_url() {
        assert_eq!(host_of("https://api.example.org:8443/v1?x=1"), "api.example.org");
        assert_eq!(host_of("http://user:pw@cdn.example.org/a"), "cdn.example.org");
        assert_eq!(host_of("https://[::1]:9/x"), "[::1]");
        assert_eq!(host_of("wss://example.org#a"), "example.org");
    }

    #[test]
    fn a_stub_s_answer_is_sent_as_given() {
        use base64::Engine;
        let f = fulfil("r1", &Answered::new(201, "image/png", [0x89, b'P']));
        assert_eq!((f["requestId"].as_str(), f["responseCode"].as_u64()), (Some("r1"), Some(201)));
        assert_eq!(f["responseHeaders"][0]["value"], "image/png");
        assert_eq!(f["responseHeaders"][1]["value"], "*", "readable from the aio:// test page");
        let body = base64::engine::general_purpose::STANDARD.decode(f["body"].as_str().unwrap()).unwrap();
        assert_eq!(body, [0x89, b'P']);
    }

    #[test]
    fn linux_runs_with_no_display_and_none_in_a_sandbox() {
        assert_eq!(flags("linux").last(), Some(&"--ozone-platform=headless"));
        assert!(!flags("macos").contains(&"--ozone-platform=headless"));
        assert!(flags("windows").contains(&"--disable-frame-rate-limit"));
        let f = flags("macos");
        assert!(f.contains(&"--use-angle=swiftshader") && f.contains(&"--disable-gpu-compositing") && !f.contains(&"--disable-gpu"));
    }
}

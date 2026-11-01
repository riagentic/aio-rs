//! The first-run window's last resort on a Linux desktop with neither zenity
//! nor yad (aio's `browserBackend`): a page in the default browser, served on
//! loopback, in the app's colours. It polls `state` and answers on
//! `answer?retry|quit|previous`; its tab
//! closing says `closed` (a cancel, or Quit when asked). A page that never
//! loads is a window lost: the download goes on, and a question nobody can
//! see answers Quit.
//!
//! Only the browser it opened may talk to it: every path is under a random
//! token (another local user or a page can't guess it), the `Host` must be
//! the address it serves (a DNS-rebound name is refused) and an `Origin`,
//! when sent, its own; a request's head is read up to 8 KiB.
use crate::firstrun::Answer;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long the page has to ask for its state before it counts as never
/// opened.
pub(crate) const OPEN_WITHIN: Duration = Duration::from_secs(30);

/// What the page shows.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub(crate) struct State {
    pub(crate) heading: String,
    pub(crate) detail: String,
    /// 0 to 1; `None`: unknown.
    pub(crate) fraction: Option<f64>,
    /// The Retry / Quit question is up.
    pub(crate) asking: bool,
    /// Its third answer's words, when it has one.
    pub(crate) previous: Option<String>,
    pub(crate) done: bool,
}

/// A page being served, until dropped.
pub(crate) struct Page {
    shared: Arc<Shared>,
    opened: Instant,
    /// Why the browser could not be asked to open it.
    refused: Option<String>,
}

#[derive(Default)]
struct Shared {
    title: String,
    /// The app's colours (`firstrun::Look`).
    colors: [[String; 4]; 2],
    /// The splash's image over the page (a first start with `[build.splash]`), a PNG.
    image: Option<Vec<u8>>,
    /// `127.0.0.1:<port>`: the only `Host` answered.
    host: String,
    /// The first path segment every request must have.
    token: String,
    state: Mutex<State>,
    /// The page asked for its state at least once.
    seen: AtomicBool,
    closed: AtomicBool,
    /// What was clicked.
    answer: Mutex<Option<Answer>>,
    stop: AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The page (aio's `browserPage`) in the app's colours (`[light, dark]`, each
/// background, text, bar, track): the title as text, never markup.
pub(crate) fn html(title: &str, [l, d]: &[[String; 4]; 2], image: Option<&[u8]>) -> String {
    let title = title.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    // Colours reach CSS only as colours (`#hex`), never as markup.
    let c = |v: &String| if aio::config::hex_color(v) { v.clone() } else { "inherit".into() };
    format!(
        r#"<!doctype html><meta charset="utf-8"><title>{title}</title>
<style>body{{font:15px system-ui,sans-serif;max-width:480px;margin:15vh auto;padding:0 16px;color:{lt};background:{lb};accent-color:{la}}}
@media (prefers-color-scheme:dark){{body{{color:{dt};background:{db};accent-color:{da}}}}}
progress{{width:100%;height:22px}}button{{font:inherit;padding:6px 16px;margin-right:8px}}</style>
{img}<h2 id="h"></h2><p id="d"></p><progress id="p"></progress><p id="q" hidden><button id="r">Retry</button><button id="v" hidden></button><button id="x">Quit</button></p>
<script>
const $=(i)=>document.getElementById(i);
async function tick(){{let s;try{{s=await (await fetch('state')).json()}}catch{{$('d').textContent='The app has closed: you can close this tab.';return}}
$('h').textContent=s.heading;$('d').textContent=s.detail;
if(s.fraction===null)$('p').removeAttribute('value');else $('p').value=s.fraction;
$('q').hidden=!s.asking;$('v').hidden=!s.previous;$('v').textContent=s.previous||'';if(s.done){{$('p').value=1;$('d').textContent='Done: you can close this tab.';return}}
setTimeout(tick,300)}}
$('r').onclick=()=>fetch('answer?retry',{{method:'POST'}});$('x').onclick=()=>fetch('answer?quit',{{method:'POST'}});
$('v').onclick=()=>fetch('answer?previous',{{method:'POST'}});
window.addEventListener('beforeunload',()=>navigator.sendBeacon('closed'));tick();
</script>"#,
        img = image.map_or(String::new(), |png| {
            use base64::Engine;
            let b64 = base64::engine::general_purpose::STANDARD.encode(png);
            format!(r#"<img src="data:image/png;base64,{b64}" alt="" style="display:block;max-width:100%;margin:0 auto 8px">"#)
        }),
        lb = c(&l[0]),
        lt = c(&l[1]),
        la = c(&l[2]),
        db = c(&d[0]),
        dt = c(&d[1]),
        da = c(&d[2]),
    )
}

/// The answer to one request line (`GET /<token>/state HTTP/1.1`) with its
/// `Host` and `Origin`: status, type, body (pure but for the state).
fn answer(s: &Shared, line: &str, host: Option<&str>, origin: Option<&str>) -> (&'static str, &'static str, String) {
    let mut parts = line.split(' ');
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    if host != Some(s.host.as_str()) || origin.is_some_and(|o| o != format!("http://{}", s.host)) {
        return ("403 Forbidden", "text/plain", String::new());
    }
    let Some(target) = target.strip_prefix(&format!("/{}", s.token)) else {
        return ("404 Not Found", "text/plain", String::new());
    };
    match (method, target) {
        ("GET", "/state") => {
            s.seen.store(true, Ordering::SeqCst);
            let json = aio::serde_json::to_string(&*lock(&s.state)).unwrap_or_default();
            ("200 OK", "application/json", json)
        }
        ("POST", "/answer?retry") => {
            *lock(&s.answer) = Some(Answer::Retry);
            ("204 No Content", "text/plain", String::new())
        }
        ("POST", "/answer?quit") => {
            *lock(&s.answer) = Some(Answer::Quit);
            ("204 No Content", "text/plain", String::new())
        }
        ("POST", "/answer?previous") if lock(&s.state).previous.is_some() => {
            *lock(&s.answer) = Some(Answer::Previous);
            ("204 No Content", "text/plain", String::new())
        }
        ("POST", "/closed") => {
            s.closed.store(true, Ordering::SeqCst);
            ("204 No Content", "text/plain", String::new())
        }
        ("GET", "/") => ("200 OK", "text/html; charset=utf-8", html(&s.title, &s.colors, s.image.as_deref())),
        _ => ("404 Not Found", "text/plain", String::new()),
    }
}

/// The most of a request's head read.
const HEAD: u64 = 8 << 10;

fn reply(s: &Shared, c: TcpStream) -> io::Result<()> {
    use std::io::Read;
    c.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut r = BufReader::new((&c).take(HEAD));
    let mut line = String::new();
    r.read_line(&mut line)?;
    // The rest of the head; no request here carries a body worth reading.
    let (mut h, mut host, mut origin) = (String::new(), None, None);
    while r.read_line(&mut h)? > 2 {
        if let Some((k, v)) = h.split_once(':') {
            let v = Some(v.trim().to_string());
            match k.trim().to_ascii_lowercase().as_str() {
                "host" => host = v,
                "origin" => origin = v,
                _ => {}
            }
        }
        h.clear();
    }
    let (status, kind, body) = answer(s, line.trim_end(), host.as_deref(), origin.as_deref());
    let mut c = &c;
    write!(
        c,
        "HTTP/1.1 {status}\r\ncontent-type: {kind}\r\ncontent-length: {}\r\ncache-control: no-store\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

impl Page {
    /// Serve the page, in `colors`, and ask `opener` (`xdg-open`) to show it.
    pub(crate) fn open(
        title: &str,
        heading: &str,
        opener: &Path,
        colors: &[[String; 4]; 2],
        image: Option<&[u8]>,
    ) -> io::Result<Self> {
        let l = TcpListener::bind("127.0.0.1:0")?;
        let host = l.local_addr()?.to_string();
        let token = aio::uuid_v4().simple().to_string();
        let url = format!("http://{host}/{token}/");
        let state = State { heading: heading.into(), ..State::default() };
        let shared = Arc::new(Shared {
            title: title.into(),
            colors: colors.clone(),
            image: image.map(<[u8]>::to_vec),
            host,
            token,
            state: Mutex::new(state),
            ..Shared::default()
        });
        let s = shared.clone();
        std::thread::spawn(move || {
            for c in l.incoming() {
                if s.stop.load(Ordering::SeqCst) {
                    break;
                }
                // One at a time: a page polls; nothing here is slow.
                _ = c.and_then(|c| reply(&s, c));
            }
        });
        let refused = match crate::firstrun::quiet(Command::new(opener)).arg(&url).status() {
            Ok(st) if st.success() => None,
            Ok(st) => Some(format!("{} {st}", opener.display())),
            Err(e) => Some(format!("{}: {e}", opener.display())),
        };
        Ok(Self { shared, opened: Instant::now(), refused })
    }

    #[cfg(all(test, unix))]
    fn url(&self) -> String {
        format!("http://{}/{}/", self.shared.host, self.shared.token)
    }

    /// Why nobody will see the page, if so: the browser refused, or it
    /// never loaded within `within`.
    pub(crate) fn lost(&self, within: Duration) -> Option<String> {
        if self.shared.seen.load(Ordering::SeqCst) {
            return None;
        }
        if let Some(why) = &self.refused {
            return Some(format!("the browser could not be opened ({why})"));
        }
        (self.opened.elapsed() > within).then(|| format!("the page did not load within {} s", within.as_secs()))
    }

    /// Its tab was closed.
    pub(crate) fn closed(&self) -> bool {
        self.shared.closed.load(Ordering::SeqCst)
    }

    pub(crate) fn show(&self, f: impl FnOnce(&mut State)) {
        f(&mut lock(&self.shared.state));
    }

    /// Ask Retry or Quit (with `previous`, [`crate::firstrun::PREVIOUS`]
    /// too), with `message`: a closed tab, or a page nobody sees within
    /// `within`, is a Quit.
    pub(crate) fn ask(&self, title: &str, message: &str, previous: bool, within: Duration) -> Answer {
        *lock(&self.shared.answer) = None;
        self.shared.closed.store(false, Ordering::SeqCst);
        let previous = previous.then(|| crate::firstrun::PREVIOUS.to_string());
        self.show(|s| {
            *s = State { heading: title.into(), detail: message.into(), asking: true, previous, ..State::default() }
        });
        let answer = loop {
            if let Some(a) = lock(&self.shared.answer).take() {
                break a;
            }
            if self.closed() || self.lost(within).is_some() {
                break Answer::Quit;
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        self.show(|s| (s.asking, s.previous) = (false, None));
        answer
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        self.show(|s| s.done = true);
        // One poll to see "done" before the server goes.
        if self.shared.seen.load(Ordering::SeqCst) && !self.closed() {
            std::thread::sleep(Duration::from_millis(700));
        }
        self.shared.stop.store(true, Ordering::SeqCst);
        _ = TcpStream::connect(&self.shared.host);
    }
}

// The opener in them is `true` or `false` (macOS has no /bin/true).
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::Read;

    /// `method path` (under the page's token) against the page, as its
    /// browser asks: its status line and body.
    fn call(p: &Page, method: &str, path: &str) -> (String, String) {
        let host = &p.shared.host;
        raw(p, &format!("{method} /{}{path} HTTP/1.1\r\nhost: {host}\r\ncontent-length: 0\r\n\r\n", p.shared.token))
    }

    /// `request` as sent: the answer's status line and body.
    fn raw(p: &Page, request: &str) -> (String, String) {
        let mut c = TcpStream::connect(&p.shared.host).unwrap();
        c.write_all(request.as_bytes()).unwrap();
        let mut out = String::new();
        c.read_to_string(&mut out).unwrap();
        let (head, body) = out.split_once("\r\n\r\n").unwrap();
        (head.lines().next().unwrap().to_string(), body.to_string())
    }

    fn colors() -> [[String; 4]; 2] {
        [
            ["#fafbff", "#1b1e24", "#2f6fe0", "#d9dde6"].map(String::from),
            ["#0e1015", "#eceef3", "#2f6fe0", "</style>"].map(String::from),
        ]
    }

    #[test]
    fn the_page_shows_the_progress_and_its_tab_closing_is_a_cancel() {
        let p = Page::open("Notes <beta>", "Getting Notes ready", Path::new("true"), &colors(), None).unwrap();
        // Not asked for yet: lost only once its time is up.
        assert_eq!((p.lost(OPEN_WITHIN), p.lost(Duration::ZERO).is_some()), (None, true));
        let (status, page) = call(&p, "GET", "/");
        assert!(status.contains("200") && page.contains("<title>Notes &lt;beta&gt;</title>"), "{page}");
        assert!(page.contains("color:#1b1e24;background:#fafbff") && page.contains("color:#eceef3;background:#0e1015"));
        assert_eq!(page.matches("</style>").count(), 1, "a colour is never markup");
        p.show(|s| (s.fraction, s.detail) = (Some(0.5), "45 / 90 MB".into()));
        let (_, state) = call(&p, "GET", "/state");
        let v: aio::serde_json::Value = aio::serde_json::from_str(&state).unwrap();
        assert_eq!(
            (v["heading"].as_str(), v["fraction"].as_f64(), v["asking"].as_bool()),
            (Some("Getting Notes ready"), Some(0.5), Some(false))
        );
        // Seen once: never lost after.
        assert_eq!(p.lost(Duration::ZERO), None);
        assert!(call(&p, "GET", "/etc/passwd").0.contains("404"));
        assert!(p.url().ends_with(&format!("/{}/", p.shared.token)), "{}", p.url());
        // Only its own browser: the token, the address it serves, its origin.
        let host = &p.shared.host;
        assert!(raw(&p, &format!("GET /state HTTP/1.1\r\nhost: {host}\r\n\r\n")).0.contains("404"), "no token");
        let rebound = format!("POST /{}/answer?quit HTTP/1.1\r\nhost: evil.example:80\r\n\r\n", p.shared.token);
        assert!(raw(&p, &rebound).0.contains("403"), "a rebound name");
        let foreign =
            format!("GET /{}/state HTTP/1.1\r\nhost: {host}\r\norigin: https://evil.example\r\n\r\n", p.shared.token);
        assert!(raw(&p, &foreign).0.contains("403"), "another site's page");

        assert!(!p.closed());
        call(&p, "POST", "/closed");
        assert!(p.closed());
    }

    #[test]
    fn the_question_waits_for_a_click_and_nobody_seeing_it_is_a_quit() {
        let p = Arc::new(Page::open("Notes", "Getting Notes ready", Path::new("true"), &colors(), None).unwrap());
        call(&p, "GET", "/state");
        let ask = |previous: bool| {
            let p = p.clone();
            std::thread::spawn(move || p.ask("Notes", "It could not be downloaded.", previous, OPEN_WITHIN))
        };
        let state = || aio::serde_json::from_str::<aio::serde_json::Value>(&call(&p, "GET", "/state").1).unwrap();
        let asked = ask(false);
        while !state()["asking"].as_bool().unwrap() {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(state()["previous"].is_null());
        assert!(call(&p, "POST", "/answer?previous").0.contains("404"), "not offered: not taken");
        call(&p, "POST", "/answer?retry");
        assert_eq!(asked.join().unwrap(), Answer::Retry);
        let asked = ask(true);
        while state()["previous"].as_str() != Some(crate::firstrun::PREVIOUS) {
            std::thread::sleep(Duration::from_millis(20));
        }
        call(&p, "POST", "/answer?previous");
        assert_eq!(asked.join().unwrap(), Answer::Previous);
        // A browser that would not open: nobody sees the question, so Quit.
        let refused = Page::open("Notes", "h", Path::new("false"), &colors(), None).unwrap();
        assert!(refused.lost(OPEN_WITHIN).unwrap().starts_with("the browser could not be opened"));
        assert_eq!(refused.ask("Notes", "?", true, OPEN_WITHIN), Answer::Quit);
    }
}

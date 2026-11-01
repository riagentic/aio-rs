//! Pages that are not the app's (aio's `<webview>`, `openWindow`,
//! `build.guestPreloads`, `electron.permissions`, `confirmUnload`,
//! `shortcuts`): what the window allows beyond its own page, as the runtime
//! reads it (`shell.json`, and `guest.js`, its policy), and child windows.
//! Everything here is off until the app's [`Window`] says so.
use crate::Window;
use aio::serde_json::{Value, json};
use std::io;
use std::path::Path;

/// Embedded web content.
#[derive(Clone, Copy, Debug, Default)]
pub struct Web {
    /// `aio::kit::Browser` shows pages (aio's `electron.webviewTag`).
    pub browser: bool,
    /// [`open_window`] opens child windows (aio's `childWindows`).
    pub windows: bool,
    /// The preloads a `Browser` or a child window may name (aio
    /// `build.guestPreloads`): `(name, script)`, the script carried in the
    /// binary (`include_str!`), so it is there in every build. A name is
    /// `/`-separated plain segments: `dapp/preload.js`.
    pub preloads: &'static [(&'static str, &'static str)],
    /// A guest or child window may go fullscreen (a video's button); unset,
    /// it holds no permission at all.
    pub fullscreen: bool,
    /// A guest or child window may save files (aio's `guestDownloads`).
    pub downloads: bool,
}

/// The window's own keys the page gets instead (aio's `electron.shortcuts`
/// set to false): Ctrl+R / Ctrl+F5 (reload) and Ctrl+P (print).
#[derive(Clone, Copy, Debug, Default)]
pub struct Keys {
    pub reload: bool,
    pub print: bool,
}

/// The permissions an app's page may also hold (`Window::permissions`),
/// beyond copying text and fullscreen: Electron's names. Notifications are
/// not one: they come from the app (`desktop::notify`), one door.
pub const PERMISSIONS: [&str; 13] = [
    "clipboard-read",
    "clipboard-sanitized-write",
    "display-capture",
    "fullscreen",
    "geolocation",
    "idle-detection",
    "keyboardLock",
    "media",
    "midi",
    "midiSysex",
    "pointerLock",
    "speaker-selection",
    "window-management",
];

/// Why `name` cannot be a guest preload's name, if it cannot (aio
/// `guestPreloadRefusal`): it reads the same on every OS and in a URL, and
/// never climbs out of the folder it is in.
pub fn preload_refusal(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        return Some("it is empty");
    }
    for seg in name.split('/') {
        match seg {
            "" => return Some("it needs single `/` between segments, none first or last"),
            "." | ".." => return Some("`.` and `..` segments are not allowed"),
            s if !s.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)) => {
                return Some("only letters, digits, `.`, `_` and `-` are allowed in a segment");
            }
            _ => {}
        }
    }
    None
}

/// `shell.json`: what the runtime allows, checked. A preload name, or a
/// permission, it would not take is an error here, never a guest that
/// silently has none.
pub(crate) fn shell_json(win: &Window) -> io::Result<String> {
    let bad = |m: String| io::Error::new(io::ErrorKind::InvalidInput, m);
    for (i, (name, _)) in win.web.preloads.iter().enumerate() {
        if let Some(why) = preload_refusal(name) {
            return Err(bad(format!("guest preload {name:?}: {why}")));
        }
        if win.web.preloads[..i].iter().any(|(n, _)| n == name) {
            return Err(bad(format!("guest preload {name:?} is declared twice")));
        }
    }
    if let Some(p) = win.permissions.iter().find(|p| !PERMISSIONS.contains(p)) {
        return Err(bad(format!("permission {p:?} is not one a page may hold: {}", PERMISSIONS.join(", "))));
    }
    let names: Vec<&str> = win.web.preloads.iter().map(|(n, _)| *n).collect();
    Ok(json!({
        "browser": win.web.browser,
        "windows": win.web.windows,
        "preloads": names,
        "guestFullscreen": win.web.fullscreen,
        "downloads": win.web.downloads,
        "allow": win.permissions,
        "confirmUnload": win.confirm_unload,
        "keys": { "reload": win.keys.reload, "print": win.keys.print },
    })
    .to_string())
}

/// Write the runtime's part into the shell's `dir`: its policy, what the
/// app allows, and the declared preloads (`guest/<name>`, only those).
pub(crate) fn install(dir: &Path, win: &Window) -> io::Result<()> {
    let shell = shell_json(win)?;
    std::fs::write(dir.join("guest.js"), include_str!("guest.js"))?;
    std::fs::write(dir.join("shell.json"), shell)?;
    let guests = dir.join("guest");
    match std::fs::remove_dir_all(&guests) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    for (name, script) in win.web.preloads {
        let p = guests.join(name);
        std::fs::create_dir_all(p.parent().unwrap_or(&guests))?;
        std::fs::write(p, script)?;
    }
    Ok(())
}

/// A child window (aio's `openWindow`): someone else's http(s) page in a
/// window of its own, sandboxed, in a session of its own.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Child {
    pub url: String,
    /// One of [`Web::preloads`], by name; unset: none.
    pub preload: Option<String>,
    /// Its session: `persist:name` is kept across starts, a name without
    /// `persist:` lives in memory. Unset: `persist:aio-child`, which child
    /// windows share. Never the app's own.
    pub partition: Option<String>,
    /// Where it may go: unset, any site (never the app's scheme or a file);
    /// set, http(s) on these origins and the one it opened on.
    pub origins: Option<Vec<String>>,
    /// Its size in px; unset: 1100 × 800.
    pub size: Option<(u32, u32)>,
}

/// The control line opening `c`, or why it cannot be opened.
pub(crate) fn child_line(id: u64, c: &Child) -> Result<String, String> {
    let http = crate::urls::Web::parse;
    let url = http(&c.url).ok_or_else(|| format!("only an http(s) page opens in a child window, not {:?}", c.url))?;
    if let Some(p) = &c.partition
        && matches!(p.trim(), "" | "persist:")
    {
        return Err(format!("partition {p:?} names the app's own session: name one, like \"persist:name\""));
    }
    if let Some(p) = c.preload.as_deref().filter(|p| preload_refusal(p).is_some()) {
        return Err(format!("preload {p:?}: {}", preload_refusal(p).unwrap_or_default()));
    }
    let mut origins = None;
    if let Some(os) = &c.origins {
        let all: Option<Vec<String>> = os.iter().map(|o| http(o).map(|u| u.origin())).collect();
        origins =
            Some(all.ok_or_else(|| format!("origins must be http(s) origins, like \"https://id.example\": {os:?}"))?);
    }
    let (width, height) = c.size.unzip();
    Ok(json!({
        "t": "child", "id": id, "url": url.to_string(), "preload": c.preload, "partition": c.partition,
        "origins": origins, "width": width, "height": height,
    })
    .to_string())
}

/// Open `c` (needs [`Web::windows`]). Every refusal says its reason: one
/// here (the URL, the partition), or the runtime's (off, a preload it does
/// not have, the app's session by another spelling).
pub async fn open_window(c: &Child) -> io::Result<()> {
    let id = {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        NEXT.fetch_add(1, Ordering::Relaxed)
    };
    let line = child_line(id, c).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    lock().push((id, tx));
    if !crate::send_runtime(line).await {
        lock().retain(|(k, _)| *k != id);
        return Err(io::Error::new(io::ErrorKind::NotConnected, "no window to open a child window from"));
    }
    rx.await.map_err(io::Error::other)?.map_err(io::Error::other)
}

/// The child windows asked for and not answered yet, by id.
type Opening = Vec<(u64, tokio::sync::oneshot::Sender<Result<(), String>>)>;
static OPENING: std::sync::Mutex<Opening> = std::sync::Mutex::new(Vec::new());

fn lock() -> std::sync::MutexGuard<'static, Opening> {
    OPENING.lock().unwrap_or_else(|e| e.into_inner())
}

/// The runtime's answer to [`open_window`]: `{"t":"opened","id":1}`, or
/// with `"error":"…"`.
pub(crate) fn opened(m: &Value) {
    let mut all = lock();
    if let Some(i) = all.iter().position(|(k, _)| Some(*k) == m["id"].as_u64()) {
        let tx = all.swap_remove(i).1;
        _ = tx.send(m["error"].as_str().map_or(Ok(()), |e| Err(format!("the child window was refused: {e}"))));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_preload_name_is_plain_segments_that_never_climb_out() {
        // aio tests/guest-preloads.test.ts guestPreloadRefusal.
        for ok in ["preload.js", "dapp/reader-preload.cjs", "a_b-c.d/e"] {
            assert_eq!(preload_refusal(ok), None, "{ok}");
        }
        for bad in ["", "/abs.js", "a//b.js", "a/", "../x.js", "a/./b.js", "a\\b.js", "a b.js", "C:x.js", "é.js"] {
            assert!(preload_refusal(bad).is_some(), "{bad}");
        }
    }

    #[test]
    fn the_runtime_is_told_only_what_the_app_allows_and_a_bad_declaration_is_an_error() {
        let v: Value = aio::serde_json::from_str(&shell_json(&Window::default()).unwrap()).unwrap();
        // Off by default: no tag, no windows, no preloads, nothing more for the page.
        assert_eq!(v["browser"], false);
        assert_eq!(v["windows"], false);
        assert_eq!(v["preloads"], json!([]));
        assert_eq!((v["allow"].clone(), v["confirmUnload"].clone()), (json!([]), json!(false)));
        assert_eq!(v["keys"], json!({ "reload": false, "print": false }), "the shell keeps its keys");
        static PRE: [(&str, &str); 2] = [("dapp/preload.js", "1"), ("reader.js", "2")];
        let win = Window {
            web: Web { browser: true, windows: true, preloads: &PRE, fullscreen: true, downloads: false },
            permissions: &["clipboard-read"],
            confirm_unload: true,
            keys: Keys { reload: true, print: false },
            ..Default::default()
        };
        let v: Value = aio::serde_json::from_str(&shell_json(&win).unwrap()).unwrap();
        assert_eq!(v["preloads"], json!(["dapp/preload.js", "reader.js"]));
        assert_eq!((v["allow"].clone(), v["guestFullscreen"].clone()), (json!(["clipboard-read"]), json!(true)));
        assert_eq!(v["keys"]["reload"], true);
        let err = |w: &Window| shell_json(w).unwrap_err().to_string();
        static TWICE: [(&str, &str); 2] = [("a.js", ""), ("a.js", "")];
        static CLIMB: [(&str, &str); 1] = [("../a.js", "")];
        assert!(err(&Window { web: Web { preloads: &TWICE, ..win.web }, ..win }).contains("declared twice"));
        assert!(err(&Window { web: Web { preloads: &CLIMB, ..win.web }, ..win }).contains("`..`"));
        assert!(err(&Window { permissions: &["notifications"], ..win }).contains("not one a page may hold"));
    }

    #[test]
    fn only_the_declared_preloads_are_written() {
        let d = tempfile::tempdir().unwrap();
        static A: [(&str, &str); 2] = [("dapp/preload.js", "a"), ("b.js", "b")];
        static B: [(&str, &str); 1] = [("b.js", "b2")];
        install(d.path(), &Window { web: Web { preloads: &A, ..Default::default() }, ..Default::default() }).unwrap();
        assert_eq!(std::fs::read_to_string(d.path().join("guest/dapp/preload.js")).unwrap(), "a");
        install(d.path(), &Window { web: Web { preloads: &B, ..Default::default() }, ..Default::default() }).unwrap();
        assert!(!d.path().join("guest/dapp").exists(), "a preload no longer declared is gone");
        assert_eq!(std::fs::read_to_string(d.path().join("guest/b.js")).unwrap(), "b2");
        assert_eq!(std::fs::read_to_string(d.path().join("guest.js")).unwrap(), include_str!("guest.js"));
    }

    #[test]
    fn a_child_window_is_an_http_page_in_a_session_of_its_own() {
        // aio tests/electron-openwindow.test.ts, electron-preload-openwindow-answers.test.ts.
        let c = Child { url: "https://id.example/login".into(), ..Default::default() };
        let v: Value = aio::serde_json::from_str(&child_line(7, &c).unwrap()).unwrap();
        assert_eq!(
            (v["t"].as_str(), v["id"].as_u64(), v["url"].as_str()),
            (Some("child"), Some(7), Some("https://id.example/login"))
        );
        assert_eq!(
            (v["partition"].clone(), v["origins"].clone(), v["width"].clone()),
            (Value::Null, Value::Null, Value::Null)
        );
        let o = Child {
            origins: Some(vec!["https://a.example/path".into(), "http://b.example:8080".into()]),
            size: Some((640, 480)),
            partition: Some("persist:login".into()),
            ..c.clone()
        };
        let v: Value = aio::serde_json::from_str(&child_line(1, &o).unwrap()).unwrap();
        assert_eq!(v["origins"], json!(["https://a.example", "http://b.example:8080"]));
        assert_eq!((v["width"].as_u64(), v["height"].as_u64()), (Some(640), Some(480)));
        let no = |c: Child| child_line(1, &c).unwrap_err();
        assert!(no(Child { url: "aio://app/".into(), ..c.clone() }).contains("only an http(s) page"));
        assert!(no(Child { url: "file:///etc/passwd".into(), ..c.clone() }).contains("only an http(s) page"));
        assert!(no(Child { partition: Some("persist:".into()), ..c.clone() }).contains("app's own session"));
        assert!(no(Child { partition: Some(" ".into()), ..c.clone() }).contains("app's own session"));
        assert!(no(Child { preload: Some("../x.js".into()), ..c.clone() }).contains("`..`"));
        assert!(no(Child { origins: Some(vec!["*".into()]), ..c }).contains("origins must be"));
    }

    #[tokio::test]
    async fn the_runtimes_answer_reaches_the_caller_with_its_reason() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        lock().push((u64::MAX, tx));
        opened(&json!({ "t": "opened", "id": u64::MAX, "error": "child windows are off" }));
        assert_eq!(rx.await.unwrap(), Err("the child window was refused: child windows are off".into()));
        // Without a window there is nothing to open it from: said, never a hang.
        let e = open_window(&Child { url: "https://x.example".into(), ..Default::default() }).await.unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::NotConnected);
    }

    /// guest.js, in Node: aio's main-process policy tests on its pure parts.
    #[test]
    fn the_guest_policy_in_node() {
        let js = format!(
            "const module = {{ exports: {{}} }}; const exports = module.exports;
             {}
             const G = module.exports, out = {{}};
             // aio guest-preloads.test.ts: only a declared name, exactly.
             out.pre = [G.preload('', ['a.js']), G.preload(G.PRELOAD + 'a.js', ['a.js']), G.preload(G.PRELOAD + '../a.js', ['a.js']).why.includes('not a declared'),
               G.preload('/home/x/a.js', ['a.js']).why.includes('not a declared'), G.preload(G.PRELOAD + 'b.js', []).why.includes('declared: none')];
             // aio electron-webview-host-keys.test.ts: malformed is ignored whole.
             out.keys = [[...G.hostKeys(['Escape', 'F6'])], G.hostKeys([]), G.hostKeys('Escape'), G.hostKeys([1]), G.hostKeys(Array(17).fill('a')), G.hostKeys(['x'.repeat(33)])];
             // aio electron-web-isolation.test.ts pop-ups.
             const st = {{ input: 1000, opened: 0 }};
             out.pop = [G.popup('https://a.example/x', 3000, st), G.popup('https://a.example/x', 7000, st).why.slice(0, 8), G.popup('mailto:x', 3000, st).why.slice(0, 9),
               G.popup('nope', 3000, st).why, G.popup('https://a.example', 3000, {{ input: 2900, opened: 2000 }}).why];
             // aio child-window navigation: the app's scheme and files never; origins listed: those only.
             const ok = new Set(['https://a.example']);
             out.nav = [G.childNav('https://z.example', null, ok, true), G.childNav('aio://app/', null, ok, true), G.childNav('file:///etc', null, ok, true),
               G.childNav('https://a.example/2', ['https://a.example'], ok, true), G.childNav('https://z.example', ['https://a.example'], ok, true),
               G.childNav('https://z.example', ['https://a.example'], ok, false), G.childNav('mailto:x', ['https://a.example'], ok, false)];
             // aio electron-shortcuts.test.ts.
             const k = (key, m) => Object.assign({{ type: 'keyDown', key, control: false, meta: false, shift: false }}, m);
             const keep = {{ reload: false, print: false }}, give = {{ reload: true, print: true }};
             out.sc = [G.shortcut(k('r', {{ control: true }}), keep, false), G.shortcut(k('R', {{ control: true, shift: true }}), keep, false),
               G.shortcut(k('F5', {{ control: true }}), keep, false), G.shortcut(k('F5'), keep, false), G.shortcut(k('p', {{ meta: true }}), keep, false),
               G.shortcut(k('r', {{ control: true }}), give, false), G.shortcut(k('p', {{ control: true }}), give, false),
               G.shortcut(k('F12'), keep, false), G.shortcut(k('F12'), keep, true), G.shortcut(k('I', {{ control: true, shift: true }}), keep, true),
               G.shortcut(k('Delete', {{ control: true, shift: true }}), keep, true), G.shortcut(Object.assign(k('r', {{ control: true }}), {{ type: 'keyUp' }}), keep, false)];
             // aio electron-unload-veto.test.ts.
             out.unload = [G.unload(true, false), G.unload(false, true), G.unload(false, false)];
             console.log(JSON.stringify(out));",
            include_str!("guest.js")
        );
        let Ok(out) = std::process::Command::new("node").args(["-e", &js]).output() else {
            return eprintln!("skipped: no node");
        };
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let v: Value = aio::serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["pre"], json!([null, { "name": "a.js" }, true, true, true]));
        assert_eq!(v["keys"], json!([["Escape", "F6"], null, null, null, null, null]));
        let why = "more than one link in 2 s";
        assert_eq!(v["pop"], json!([{ "open": "https://a.example/x" }, "no click", "only http", "not a URL", why]));
        assert_eq!(v["nav"][0], Value::Null, "no origins: any site");
        assert!(v["nav"][1].as_str().unwrap().contains("aio:") && v["nav"][2].as_str().unwrap().contains("file:"));
        assert_eq!(
            (v["nav"][3].clone(), v["nav"][5].clone()),
            (Value::Null, Value::Null),
            "listed, and its first load's redirects"
        );
        assert!(v["nav"][4].as_str().unwrap().contains("not one of its origins"));
        assert!(v["nav"][6].as_str().unwrap().contains("http(s) only"));
        let sc = json!([
            "reload", "reload", "reload", null, "print", null, null, null, "devtools", "devtools", "clear", null
        ]);
        assert_eq!(v["sc"], sc, "plain F5 is the app's; DevTools only in a dev build");
        assert_eq!(v["unload"], json!(["leave", "ask", "stay"]));
    }
}

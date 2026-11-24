//! An embedded web page (aio's `<Browser>`): someone else's site in a box of
//! the view, in Electron's `<webview>` (on when the app's window says so:
//! `aio_shell::Window { web: Web { browser: true, .. } }`). The guest runs
//! in a session of its own, sandboxed, with only a preload the app
//! declared, and holds no permission (the shell's `guest.js`).
//!
//! aio's two traps are closed by shape. `src` navigates only when the prop
//! changes, so an address bar that writes [`GuestEvent::Navigated`] back
//! into `src` never loops, and a re-render never sends the guest back. A
//! guest dies with its element (Electron's rule); `keep_alive` remembers
//! where it was, and the next `Browser` under that id opens there (its
//! scroll and forms cannot be kept; its session's cookies always are).
use super::{Island, WebIsland};
use crate::ui::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// What a guest did (`onevent`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum GuestEvent {
    /// It landed on `url`, and whether it can now go back or forward
    /// ([`Go`]). `in_page`: the same document moved (a hash, a single-page
    /// site's `pushState`); otherwise a new document took its place, a
    /// reload of the same address too: what the last one was given (an
    /// approval it asked for) dies with it.
    Navigated {
        url: String,
        back: bool,
        forward: bool,
        in_page: bool,
    },
    /// It could not open `url` (Chromium's net error `code`, negative, and
    /// its name: `-102`, `ERR_CONNECTION_REFUSED`): nothing answered there,
    /// or the answer broke off. An empty error page took the last
    /// document's place (a step of its history), so what that one was
    /// given dies as on a new one. A load given up for another (`-3`, a new
    /// address before the last one came) is none.
    LoadFailed {
        url: String,
        code: i32,
        description: String,
    },
    /// Its page's title.
    Title {
        title: String,
    },
    /// One of `host_keys`, pressed in it (in any of its frames).
    Key(HostKey),
    /// Its preload's `ipcRenderer.sendToHost(channel, ...args)`.
    Message {
        channel: String,
        args: Vec<Value>,
    },
    /// It took the keyboard.
    Focus,
    /// Its page crashed or was killed (`reason`: Electron's word).
    Gone {
        reason: String,
    },
    /// Its page stopped answering, and came back.
    Unresponsive,
    Responsive,
    /// The shell refused its `preload` (not one the window declared): it
    /// loads with none (aio's `aio:guest-preload-refused`, also sent to
    /// the page's `window` as that event).
    PreloadRefused {
        preload: String,
        reason: String,
    },
}

/// A key a guest handed back (aio's `HostKey`), with the modifiers held.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostKey {
    pub key: String,
    pub code: String,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
    pub repeat: bool,
}

/// What [`go`] asks a guest.
#[derive(Clone, Debug, PartialEq)]
pub enum Go {
    Back,
    Forward,
    Reload,
    Stop,
    /// To its preload's `ipcRenderer.on(channel, (e, ...args))`.
    Send {
        channel: String,
        args: Vec<Value>,
    },
    /// A stylesheet added to its page (`insertCSS`), until it navigates.
    Css(String),
    /// Its page drawn at this zoom (`setZoomFactor`; 1 is 100 %).
    Zoom(f64),
    /// Its DevTools opened, in a window of their own.
    DevTools,
}

/// Ask the guest of the `Browser` with element id `id`; one that is not
/// there (yet) is not asked.
pub fn go(id: &str, what: Go) {
    let call = match what {
        Go::Back => "goBack()".to_string(),
        Go::Forward => "goForward()".into(),
        Go::Reload => "reload()".into(),
        Go::Stop => "stop()".into(),
        Go::Send { channel, args } => format!("send({}, ...{})", Value::from(channel), Value::from(args)),
        Go::Css(css) => format!("insertCSS({})", Value::from(css)),
        Go::Zoom(z) => format!("setZoomFactor({})", Value::from(z)),
        Go::DevTools => "openDevTools()".into(),
    };
    _ = eval(&guest_js(id, &format!("try {{ w.{call} }} catch (e) {{}}")));
}

/// The guest of the `Browser` with element id `id` (`Guest::new("web")`),
/// steered: a call on one that is not there (yet) does nothing, a question
/// gets `None`. [`go`] and [`webview`] reach the rest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Guest(String);

impl Guest {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Open `url`, its `Browser`'s `referrer` sent; `src` stays as it was.
    /// One that fails is a [`GuestEvent::LoadFailed`].
    pub fn load_url(&self, url: &str) {
        let load = format!(
            "const r = w.getAttribute('httpreferrer'); w.loadURL({}, r ? {{ httpReferrer: r }} : undefined).catch(() => {{}});",
            Value::from(url)
        );
        _ = eval(&guest_js(&self.0, &load));
    }

    /// The address it shows (`getURL`).
    pub async fn url(&self) -> Option<String> {
        webview(&self.0, "getURL", vec![]).await.and_then(|v| v.as_str().map(Into::into))
    }

    pub fn reload(&self) {
        go(&self.0, Go::Reload)
    }

    pub fn stop(&self) {
        go(&self.0, Go::Stop)
    }

    pub fn go_back(&self) {
        go(&self.0, Go::Back)
    }

    pub fn go_forward(&self) {
        go(&self.0, Go::Forward)
    }

    /// `input` in its page as a person's (`sendInputEvent`): a key to what
    /// has its focus, a click at its point.
    pub fn send_input(&self, input: &GuestInput) {
        let send: String = input_events(input).iter().map(|e| format!("w.sendInputEvent({e});")).collect();
        _ = eval(&guest_js(&self.0, &format!("try {{ {send} }} catch (e) {{}}")));
    }
}

/// A person's input for [`Guest::send_input`].
#[derive(Clone, Debug, PartialEq)]
pub enum GuestInput {
    /// A key pressed and let go, with these modifiers held; a character
    /// key types its character (unless Ctrl or Meta is held).
    Key(Key, Modifiers),
    /// A left click at `(x, y)` of its page, in CSS pixels.
    Click(f64, f64, Modifiers),
}

/// `input` as Electron's input events, in order.
fn input_events(input: &GuestInput) -> Vec<Value> {
    let held = |m: &Modifiers| {
        [
            (Modifiers::SHIFT, "shift"),
            (Modifiers::CONTROL, "control"),
            (Modifiers::ALT, "alt"),
            (Modifiers::META, "meta"),
        ]
        .into_iter()
        .filter(|(b, _)| m.contains(*b))
        .map(|(_, n)| n)
        .collect::<Vec<_>>()
    };
    match input {
        GuestInput::Key(key, m) => {
            // Electron's key names (its accelerators'): `Up`, not `ArrowUp`.
            let code = match key {
                Key::Character(c) if c == " " => "Space".to_string(),
                Key::Character(c) => c.clone(),
                Key::ArrowUp => "Up".into(),
                Key::ArrowDown => "Down".into(),
                Key::ArrowLeft => "Left".into(),
                Key::ArrowRight => "Right".into(),
                k => k.to_string(),
            };
            let types = matches!(key, Key::Character(_)) && !m.intersects(Modifiers::CONTROL | Modifiers::META);
            let e = |t: &str| json!({ "type": t, "keyCode": code, "modifiers": held(m) });
            [Some(e("keyDown")), types.then(|| e("char")), Some(e("keyUp"))].into_iter().flatten().collect()
        }
        GuestInput::Click(x, y, m) => ["mouseDown", "mouseUp"]
            .map(|t| json!({ "type": t, "x": x, "y": y, "button": "left", "clickCount": 1, "modifiers": held(m) }))
            .into(),
    }
}

/// `js` run in the page of the guest of the `Browser` with element id `id`
/// (`executeJavaScript`, a promise awaited): its value as JSON; `None` when
/// that guest is not there, or the script threw.
pub async fn run_js(id: &str, js: &str) -> Option<Value> {
    let then = format!("try {{ return await w.executeJavaScript({}) }} catch (e) {{ return null }}", Value::from(js));
    eval(&guest_js(id, &then)).join::<Option<Value>>().await.ok().flatten()
}

/// Every method of Electron's `<webview>`, by name: what [`webview`] takes
/// (aio handed out the element itself).
pub const WEBVIEW_METHODS: &[&str] = &[
    "loadURL",
    "downloadURL",
    "getURL",
    "getTitle",
    "isLoading",
    "isLoadingMainFrame",
    "isWaitingForResponse",
    "stop",
    "reload",
    "reloadIgnoringCache",
    "canGoBack",
    "canGoForward",
    "canGoToOffset",
    "clearHistory",
    "goBack",
    "goForward",
    "goToIndex",
    "goToOffset",
    "isCrashed",
    "setUserAgent",
    "getUserAgent",
    "insertCSS",
    "removeInsertedCSS",
    "executeJavaScript",
    "openDevTools",
    "closeDevTools",
    "isDevToolsOpened",
    "isDevToolsFocused",
    "inspectElement",
    "inspectSharedWorker",
    "inspectServiceWorker",
    "setAudioMuted",
    "isAudioMuted",
    "isCurrentlyAudible",
    "undo",
    "redo",
    "cut",
    "copy",
    "centerSelection",
    "paste",
    "pasteAndMatchStyle",
    "delete",
    "selectAll",
    "unselect",
    "scrollToTop",
    "scrollToBottom",
    "adjustSelection",
    "replace",
    "replaceMisspelling",
    "insertText",
    "findInPage",
    "stopFindInPage",
    "print",
    "printToPDF",
    "capturePage",
    "send",
    "sendToFrame",
    "sendInputEvent",
    "setZoomFactor",
    "setZoomLevel",
    "getZoomFactor",
    "getZoomLevel",
    "setVisualZoomLevelLimits",
    "showDefinitionForSelection",
    "getWebContentsId",
];

/// Call `method` ([`WEBVIEW_METHODS`]) of the guest of the `Browser` with element
/// id `id` with `args`: its value as JSON, a promise awaited
/// (`capturePage`'s image as a PNG data URL, `printToPDF`'s bytes in
/// base64). `None` when that guest is not there, the method is none of a
/// `<webview>`'s (said), or it threw.
pub async fn webview(id: &str, method: &str, args: Vec<Value>) -> Option<Value> {
    if !WEBVIEW_METHODS.contains(&method) {
        crate::warn!(cat: "ui", "<Browser> has no method {method:?}: aio::kit::WEBVIEW_METHODS lists them");
        return None;
    }
    let then = format!(
        "try {{
          const v = await w[{}](...{});
          if (v && typeof v.toDataURL === 'function') return v.toDataURL();
          if (v instanceof Uint8Array) {{
            let s = '';
            for (let i = 0; i < v.length; i += 32768) s += String.fromCharCode.apply(null, v.subarray(i, i + 32768));
            return btoa(s);
          }}
          return v === undefined ? null : v;
        }} catch (e) {{ return null }}",
        Value::from(method),
        Value::from(args)
    );
    eval(&guest_js(id, &then)).join::<Option<Value>>().await.ok().flatten()
}

/// `then` with `w` the guest's `<webview>`, once it is attached.
fn guest_js(id: &str, then: &str) -> String {
    format!(
        "const b = document.getElementById({}), w = b && b.querySelector('webview'); \
         if (w && typeof w.getWebContentsId === 'function') {{ {then} }}",
        Value::from(id)
    )
}

/// Why `keys` cannot be host keys, if they cannot (aio's `isHostKeyList`):
/// 1 to 16 `KeyboardEvent.key` names of at most 32 characters. A list that
/// is a handful of ways back, never a keylogger for the page.
fn host_keys_refusal(keys: &[String]) -> Option<String> {
    let ok = (1..=16).contains(&keys.len()) && keys.iter().all(|k| (1..=32).contains(&k.chars().count()));
    (!ok).then(|| format!("host keys must be 1 to 16 KeyboardEvent.key names (like \"Escape\"), got {keys:?}"))
}

/// Where a `keep_alive` guest was when it went: the page it showed, and
/// the `src` it had then.
#[derive(Clone, Debug, Default, PartialEq)]
struct Kept {
    at: String,
    src: String,
}

/// Where a new guest under a kept id opens: where the last one was, unless
/// `src` changed meanwhile (then `src` wins) or it never left `src`.
fn reopen(kept: Option<&Kept>, src: &str) -> Option<String> {
    kept.filter(|k| k.src == src && !k.at.is_empty() && k.at != src).map(|k| k.at.clone())
}

/// This page's kept guests, by id.
#[derive(Clone, Default)]
struct Keep(Rc<RefCell<HashMap<String, Kept>>>);

/// What the page's script gets.
#[derive(Clone, Debug, PartialEq, Serialize)]
struct Shown {
    src: String,
    /// The first load's page, a kept one's.
    start: Option<String>,
    partition: Option<String>,
    preload: Option<String>,
    referrer: Option<String>,
    keys: Vec<String>,
    popups: bool,
}

/// The guest, drawn by the page: a `<webview>` made once per box, its
/// events out, a changed `src` loaded (`loadURL` once it is up). The host
/// keys go to the shell (`__aioGuest`, the preload), which relays them.
const GUEST: Island = Island::js(
    r#"const live = new Map();
    const G = window.__aioGuest;
    if (G) G.onKey((id, k) => { const f = live.get(id); if (f) f(k); });
    // A refused preload: the page's event (aio's), and each guest that asked for it.
    const refusals = new Set();
    if (G && G.onRefused) G.onRefused((d) => {
      window.dispatchEvent(new CustomEvent('aio:guest-preload-refused', { detail: d }));
      refusals.forEach((f) => f(d));
    });
    let said = false;
    // Its own look, whether or not the page has the kit's sheet: the guest
    // fills its box. In the kit's layer, so any rule of the app's wins.
    const look = new CSSStyleSheet();
    look.replaceSync('@layer aio { .aio-browser { display: flex; min-block-size: 0 } .aio-browser > webview { flex: 1; inline-size: 100%; block-size: 100% } }');
    document.adoptedStyleSheets = [...document.adoptedStyleSheets, look];
    return (el, p, emit) => {
      const refused = (d) => { if (d.preload === p.preload) emit({ t: 'preloadrefused', preload: d.preload, reason: d.reason }); };
      if (p.preload) refusals.add(refused);
      const wv = document.createElement('webview');
      const keys = p.keys;
      for (const [k, v] of [['partition', p.partition], ['httpreferrer', p.referrer],
        ['preload', p.preload && 'file:///aio-guest-preload/' + p.preload], ['allowpopups', p.popups && 'on']]) if (v) wv.setAttribute(k, v);
      let src = p.src, at = p.start || p.src, ready = false, gid = null;
      wv.addEventListener('dom-ready', () => {
        ready = true;
        const id = wv.getWebContentsId();
        if (id === gid) return;
        live.delete(gid);
        gid = id;
        if (keys.length && G) { live.set(id, (k) => emit(Object.assign({ t: 'key' }, k))); G.keys(id, keys); }
      });
      // Its history has the entry a task later; before dom-ready the guest
      // cannot be asked: no way back yet.
      const can = (f) => { try { return wv[f](); } catch (e) { return false; } };
      const nav = (url, in_page) => {
        at = url;
        setTimeout(() => emit({ t: 'navigated', url, back: can('canGoBack'), forward: can('canGoForward'), in_page }), 0);
      };
      wv.addEventListener('did-navigate', (e) => nav(e.url, false));
      wv.addEventListener('did-navigate-in-page', (e) => { if (e.isMainFrame) nav(e.url, true); });
      // -3 (ERR_ABORTED): a load given up for another, no failure.
      wv.addEventListener('did-fail-load', (e) => {
        if (e.isMainFrame && e.errorCode !== -3) emit({ t: 'loadfailed', url: e.validatedURL, code: e.errorCode, description: e.errorDescription });
      });
      wv.addEventListener('page-title-updated', (e) => emit({ t: 'title', title: e.title }));
      wv.addEventListener('ipc-message', (e) => emit({ t: 'message', channel: e.channel, args: e.args }));
      wv.addEventListener('focus', () => emit({ t: 'focus' }));
      wv.addEventListener('render-process-gone', (e) => emit({ t: 'gone', reason: (e.details && e.details.reason) || '' }));
      wv.addEventListener('unresponsive', () => emit({ t: 'unresponsive' }));
      wv.addEventListener('responsive', () => emit({ t: 'responsive' }));
      if (p.start || p.src) wv.setAttribute('src', p.start || p.src);
      el.appendChild(wv);
      setTimeout(() => {
        if (said || typeof wv.getWebContentsId === 'function') return;
        said = true;
        console.warn('<Browser> shows nothing: this window has no <webview>. Turn it on with aio_shell::Window { web: Web { browser: true, .. } } (a browser tab has none at all).');
      }, 0);
      return {
        update(q) {
          if (q.src === src) return;
          src = q.src;
          // Where it already is: an address bar writing its landing back.
          if (!src || src === at) return;
          at = src;
          if (ready) wv.loadURL(src, q.referrer ? { httpReferrer: q.referrer } : undefined).catch(() => {});
          else wv.setAttribute('src', src);
        },
        unmount() { live.delete(gid); refusals.delete(refused); },
      };
    };"#,
);

/// An embedded web page at `src`: http(s) or `about:blank`, another scheme
/// only one the window lists (`aio_shell::Web { schemes }`). `partition`
/// names its session (two `Browser`s with one share a login; `persist:`
/// keeps it across starts; unset: the guests' shared `persist:aio-webview`,
/// never the app's own).
/// `preload` is one of the window's declared preloads, by name, read once
/// when the guest starts, as `host_keys` are. `referrer` is the `Referer`
/// its loads send (an embed that checks it refuses to play without one).
/// With `popups`, a link that opens a window goes to the system browser,
/// after a real click in the page; without, it does nothing. Give it an
/// `id` to steer its guest ([`Guest`]). It brings its own look (it fills
/// its box), with or without the kit's sheet.
#[component]
pub fn Browser(
    src: Prop<String>,
    keep_alive: Option<String>,
    partition: Option<String>,
    preload: Option<String>,
    referrer: Option<String>,
    #[props(default)] host_keys: Vec<String>,
    #[props(default)] popups: bool,
    #[props(default)] onevent: EventHandler<GuestEvent>,
    #[props(default, into)] class: String,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> View {
    let keep = once(|| try_context::<Keep>().unwrap_or_else(|| provide_root(Keep::default())));
    let start = once(|| {
        reopen(keep_alive.as_ref().and_then(|k| keep.0.borrow().get(k).cloned()).as_ref(), &src.get_untracked())
    });
    // Where it is (its last landing, else what it was given), and under
    // which id and `src` it is kept when it goes.
    let now =
        once(|| Rc::new(RefCell::new((Kept { at: start.clone().unwrap_or_default(), src: String::new() }, None))));
    now.borrow_mut().1.clone_from(&keep_alive);
    {
        let (now, src) = (now.clone(), src);
        effect(move || src.with(|s| now.borrow_mut().0.src.clone_from(s)));
    }
    {
        let (keep, now) = (keep.clone(), now.clone());
        on_cleanup(move || {
            if let (k, Some(id)) = &*now.borrow() {
                keep.0.borrow_mut().insert(id.clone(), k.clone());
            }
        });
    }
    // Read once, as the guest does.
    let keys = once(|| match host_keys_refusal(&host_keys) {
        _ if host_keys.is_empty() => vec![],
        Some(why) => {
            crate::warn!(cat: "ui", "<Browser> {why}: none is relayed");
            vec![]
        }
        None => host_keys.clone(),
    });
    // A changed `src` is an update of the guest's props: it navigates there.
    let props = memo(move || Shown {
        src: src.get(),
        start: start.clone(),
        partition: partition.clone(),
        preload: preload.clone(),
        referrer: referrer.clone(),
        keys: keys.clone(),
        popups,
    });
    rsx! {
        WebIsland::<Shown, GuestEvent> {
            island: GUEST,
            props: props.get(),
            onevent: move |e: GuestEvent| {
                if let GuestEvent::Navigated { url, .. } = &e {
                    now.borrow_mut().0.at.clone_from(url);
                }
                onevent.call(e);
            },
            class: super::join("aio-browser", &class),
            attributes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kept_guest_reopens_where_it_was_unless_its_src_changed() {
        // aio tests/ui-browser.test.ts keepAlive.
        let k = Kept { at: "https://a.example/3".into(), src: "https://a.example/".into() };
        assert_eq!(reopen(Some(&k), "https://a.example/").as_deref(), Some("https://a.example/3"));
        assert_eq!(reopen(Some(&k), "https://b.example/"), None, "a src changed meanwhile wins");
        assert_eq!(reopen(None, "https://a.example/"), None, "nothing kept: start over at src");
        assert_eq!(reopen(Some(&Kept { at: k.src.clone(), ..k.clone() }), &k.src), None);
        assert_eq!(reopen(Some(&Kept { at: String::new(), ..k }), "https://a.example/"), None);
    }

    #[test]
    fn host_keys_are_a_handful_of_short_names() {
        // aio tests/electron-webview-host-keys.test.ts: validated loudly.
        let s = |v: &[&str]| v.iter().map(|k| k.to_string()).collect::<Vec<_>>();
        assert_eq!(host_keys_refusal(&s(&["Escape", "F6"])), None);
        assert!(host_keys_refusal(&[]).is_some());
        assert!(host_keys_refusal(&s(&[""])).is_some());
        assert!(host_keys_refusal(&s(&["x"; 17])).is_some());
        assert!(host_keys_refusal(&["x".repeat(33)]).is_some());
    }

    #[test]
    fn a_guest_event_reads_as_the_page_sends_it() {
        let ev = |v: Value| serde_json::from_value::<GuestEvent>(v).unwrap();
        use serde_json::json;
        assert_eq!(
            ev(
                json!({ "t": "navigated", "url": "https://a.example/#x", "back": true, "forward": false, "in_page": true })
            ),
            GuestEvent::Navigated { url: "https://a.example/#x".into(), back: true, forward: false, in_page: true }
        );
        let failed = json!({ "t": "loadfailed", "url": "http://127.0.0.1:9/", "code": -102, "description": "ERR_CONNECTION_REFUSED" });
        assert_eq!(
            ev(failed.clone()),
            GuestEvent::LoadFailed {
                url: "http://127.0.0.1:9/".into(),
                code: -102,
                description: "ERR_CONNECTION_REFUSED".into()
            }
        );
        // As the page sends it (a test's stand-in guest, TestUi::emit).
        assert_eq!(serde_json::to_value(ev(failed.clone())).unwrap(), failed);
        let key = json!({ "t": "key", "key": "Escape", "code": "Escape", "ctrl": false, "shift": true, "alt": false, "meta": false, "repeat": false });
        assert_eq!(
            ev(key),
            GuestEvent::Key(HostKey { key: "Escape".into(), code: "Escape".into(), shift: true, ..Default::default() })
        );
        assert_eq!(
            ev(json!({ "t": "message", "channel": "sign", "args": [1, "x"] })),
            GuestEvent::Message { channel: "sign".into(), args: vec![json!(1), json!("x")] }
        );
        assert_eq!(ev(json!({ "t": "gone", "reason": "crashed" })), GuestEvent::Gone { reason: "crashed".into() });
        assert_eq!(ev(json!({ "t": "focus" })), GuestEvent::Focus);
        assert_eq!(
            ev(json!({ "t": "preloadrefused", "preload": "nope.js", "reason": "not declared" })),
            GuestEvent::PreloadRefused { preload: "nope.js".into(), reason: "not declared".into() }
        );
        let k = GuestEvent::Key(HostKey { key: "F6".into(), code: "F6".into(), ..Default::default() });
        assert_eq!(ev(serde_json::to_value(&k).unwrap()), k);
    }

    #[test]
    fn an_input_is_electron_s_events_in_order() {
        let types = |i: GuestInput| {
            input_events(&i).iter().map(|e| e["type"].as_str().unwrap().to_string()).collect::<Vec<_>>()
        };
        let a = input_events(&GuestInput::Key(Key::Character("a".into()), Modifiers::SHIFT));
        assert_eq!(a[1], json!({ "type": "char", "keyCode": "a", "modifiers": ["shift"] }));
        assert_eq!(
            types(GuestInput::Key(Key::Character("a".into()), Modifiers::empty())),
            ["keyDown", "char", "keyUp"]
        );
        assert_eq!(
            types(GuestInput::Key(Key::Character("a".into()), Modifiers::CONTROL)),
            ["keyDown", "keyUp"],
            "Ctrl+A types nothing"
        );
        assert_eq!(types(GuestInput::Key(Key::Escape, Modifiers::empty())), ["keyDown", "keyUp"]);
        let code = |k: Key| input_events(&GuestInput::Key(k, Modifiers::empty()))[0]["keyCode"].clone();
        assert_eq!(
            (code(Key::ArrowUp), code(Key::Enter), code(Key::Character(" ".into()))),
            (json!("Up"), json!("Enter"), json!("Space"))
        );
        let c = input_events(&GuestInput::Click(10.0, 20.0, Modifiers::CONTROL | Modifiers::META));
        assert_eq!(c.len(), 2);
        assert_eq!(
            c[0],
            json!({ "type": "mouseDown", "x": 10.0, "y": 20.0, "button": "left", "clickCount": 1, "modifiers": ["control", "meta"] })
        );
        assert_eq!(c[1]["type"], "mouseUp");
    }
}

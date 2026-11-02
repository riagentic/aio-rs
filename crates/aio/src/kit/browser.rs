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
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// What a guest did (`onevent`).
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum GuestEvent {
    /// It landed on `url` (a load, or a single-page site's own move), and
    /// whether it can now go back or forward ([`Go`]).
    Navigated {
        url: String,
        back: bool,
        forward: bool,
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
}

/// A key a guest handed back (aio's `HostKey`), with the modifiers held.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
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
    };
    let js = format!(
        "const b = document.getElementById({}), w = b && b.querySelector('webview'); \
         if (w && typeof w.getWebContentsId === 'function') try {{ w.{call} }} catch (e) {{}}",
        Value::from(id)
    );
    _ = eval(&js);
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
    let said = false;
    return (el, p, emit) => {
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
      const nav = (url) => {
        at = url;
        setTimeout(() => emit({ t: 'navigated', url, back: can('canGoBack'), forward: can('canGoForward') }), 0);
      };
      wv.addEventListener('did-navigate', (e) => nav(e.url));
      wv.addEventListener('did-navigate-in-page', (e) => { if (e.isMainFrame) nav(e.url); });
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
        unmount() { live.delete(gid); },
      };
    };"#,
);

/// An embedded web page at `src`. `partition` names its session (two
/// `Browser`s with one share a login; `persist:` keeps it across starts;
/// unset: the guests' shared `persist:aio-webview`, never the app's own).
/// `preload` is one of the window's declared preloads, by name, read once
/// when the guest starts, as `host_keys` are. `referrer` is the `Referer`
/// its loads send (an embed that checks it refuses to play without one).
/// With `popups`, a link that opens a window goes to the system browser,
/// after a real click in the page; without, it does nothing. Give it an
/// `id` to [`go`] its guest.
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
        let (now, src) = (now.clone(), src.clone());
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
            ev(json!({ "t": "navigated", "url": "https://a.example/", "back": true, "forward": false })),
            GuestEvent::Navigated { url: "https://a.example/".into(), back: true, forward: false }
        );
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
    }
}

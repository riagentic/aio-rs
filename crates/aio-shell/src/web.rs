//! aio's browser host: the app's own UI in any browser, served by the app's
//! HTTP server (aio served its app to browsers). A browser runs the app's
//! views itself, as wasm (`ax build --web`, `aio::web::launch`), over the
//! page's own copies of the server's cells (offline too): [`painted`]
//! serves that page, its first paint rendered here for each viewer. The
//! views and the kit are the window's, their DOM and styles the same.
//!
//! [`tabs`] is aio's own tools' (`ax ui --browser`): a view that runs
//! native verbs stays in this process, each tab drawn over its socket.
use crate::{Window, wire};
use aio::Cell;
use aio::ui::View;
use dioxus_core::VirtualDom;
use dioxus_liveview::LiveViewPool;
use std::sync::{Arc, OnceLock};

/// The page's side of the socket: `window.__aioWire`, as the desktop
/// preload gives it, over `/__aio/ui`. A dropped link is retried with
/// backoff (up to 10 s), `data-aio-link` on `<html>` saying `offline`
/// meanwhile; once it is back the page starts again from the server.
/// A browser's own ways stay: a link navigates as links do (the desktop
/// interpreter keeps them for the app), and a form never leaves the page
/// (its submit is the app's, as on the desktop, where navigation is off).
const BRIDGE: &str = "window.__aioWire = (() => {
  const WS = window.WebSocket;
  const url = (location.protocol === 'https:' ? 'wss://' : 'ws://') + location.host + '/__aio/ui';
  document.addEventListener('submit', (e) => e.preventDefault(), true);
  const out = [];
  let ws = null, onFrame = null, linked = false, wait = 250;
  const say = (s) => document.documentElement.setAttribute('data-aio-link', s);
  const open = () => {
    ws = new WS(url);
    ws.binaryType = 'arraybuffer';
    ws.onopen = () => {
      if (linked) return location.reload();
      linked = true;
      say('online');
      out.splice(0).forEach((m) => ws.send(m));
    };
    ws.onmessage = (m) => onFrame && onFrame(new Uint8Array(m.data));
    ws.onclose = () => {
      say('offline');
      setTimeout(open, wait);
      wait = Math.min(wait * 2, 10000);
    };
  };
  open();
  return {
    send: (u) => (ws.readyState === 1 && linked ? ws.send(u) : out.push(u)),
    listen: (f) => {
      onFrame = f;
      if (window.interpreter) window.interpreter.intercept_link_redirects = false;
    },
  };
})();";

/// The tab page `win` describes (pure): the window's, connecting only to
/// its own server.
pub fn page(win: &Window) -> String {
    crate::page_of(win, BRIDGE, "'self'")
}

/// `http`, serving view `app` to browser tabs that show the key, each its
/// own view in this process (aio's tools: `ax ui --browser`). An app's
/// page is its wasm build: [`painted`].
pub fn tabs(http: aio::http::Http, app: fn() -> View, win: Window) -> aio::http::Http {
    static POOL: OnceLock<Arc<LiveViewPool>> = OnceLock::new();
    let pool = POOL.get_or_init(|| Arc::new(LiveViewPool::new())).clone();
    http.ui(page(&win), move |t| {
        let pool = pool.clone();
        async move {
            if let Err(e) = pool.launch_virtualdom(wire(Box::new(t.wire)), move || VirtualDom::new(app)).await {
                eprintln!("[aio-shell] browser tab: {e}");
            }
        }
    })
}

/// `http` serving the web build in `dist` (`ax build --web`) as the app's
/// shell, each page painted on the server for its viewer (aio's SSR):
/// `app` over `h`'s state as that viewer may see it (no `#[server]` field,
/// its `visible` rules and role), so the page shows before its script
/// runs; then the page's own view takes its place (`aio::web::launch`).
pub fn painted<T: Cell>(
    http: aio::http::Http,
    h: aio::Handle<T>,
    app: fn() -> View,
    dist: impl Into<std::path::PathBuf>,
) -> aio::http::Http {
    http.shell(dist).paint(move |v| paint(&h, app, v))
}

/// `app`'s first paint over what viewer `v` may see of `h`'s state.
fn paint<T: Cell>(h: &aio::Handle<T>, app: fn() -> View, v: &aio::http::Viewer) -> aio::http::Paint {
    let who = aio::Caller { by: aio::By::User(v.user.clone().unwrap_or_else(|| "peer".into())), role: v.role.clone() };
    let seen: T = aio::serde_json::from_value(h.state().project(&who)).unwrap_or_default();
    let copy = aio::Source::Local(aio::Handle::new(aio::Store::with(seen)));
    let mut dom = VirtualDom::new(app).with_root_context(aio::ui::Tab::new(v.user.clone()).with(copy));
    dom.rebuild_in_place();
    let head = aio::head::of(&dom);
    aio::http::Paint { title: head.title.clone(), head: aio::head::html(&head), body: dioxus_ssr::render(&dom) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_links_its_own_socket_and_nothing_else() {
        let p = page(&Window { title: "Board <1>", ..Default::default() });
        assert!(p.contains("<title>Board &lt;1&gt;</title>"), "{p}");
        assert!(p.contains("connect-src 'self'") && !p.contains("connect-src *"), "{p}");
        assert!(!p.contains("'unsafe-eval'") && !p.contains("script-src 'unsafe-inline'"), "{p}");
        assert!(p.contains("'/__aio/ui'") && p.contains("window.__aioWire"), "{p}");
        // The bridge comes before the interpreter that opens it.
        assert!(p.find("__aioWire = ").unwrap() < p.find("WS_ADDR").unwrap());
    }
}

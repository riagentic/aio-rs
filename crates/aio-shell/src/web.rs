//! aio's browser host: the app's own UI in any browser, served by the app's
//! HTTP server (aio served its app to browsers). A browser runs the app's
//! views itself, as wasm (`ax build --web`, `aio::web::launch`), over the
//! page's own copies of the server's cells (offline too): the server serves
//! that build (`aio::http::Http::shell`). The views and the kit are the
//! window's, their DOM and styles the same.
//!
//! [`tabs`] is aio's own tools' (`ax ui --browser`): a view that runs
//! native verbs stays in this process, each tab drawn over its socket.
use crate::Window;
use aio::ui::View;

/// The page's side of the socket: `window.__aioWire`, as the desktop
/// preload gives it, over `/__aio/ui`. A dropped link is retried with
/// backoff (up to 10 s), `data-aio-link` on `<html>` saying `offline`
/// meanwhile; once it is back the page starts again from the server.
/// A form never leaves the page (its submit is the app's, as on the
/// desktop, where navigation is off).
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
    listen: (f) => (onFrame = f),
  };
})();";

/// The tab page `win` describes (pure): the window's, connecting only to
/// its own server.
pub fn page(win: &Window) -> String {
    crate::page_of(win, BRIDGE, "'self'")
}

/// `http`, serving view `app` to browser tabs that show the key, each its
/// own view in this process (aio's tools: `ax ui --browser`), drawn by air
/// as a window is. An app's page is its wasm build (`Http::shell`).
pub fn tabs(http: aio::http::Http, app: fn() -> View, win: Window) -> aio::http::Http {
    http.ui(page(&win), move |t| async move {
        if let Err(e) = crate::views::Views::new().launch(Box::new(t.wire), app).await {
            eprintln!("[aio-shell] browser tab: {e}");
        }
    })
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
        // The bridge comes before air's interpreter, which talks through it.
        assert!(p.find("__aioWire = ").unwrap() < p.find(air::PAGE_JS).unwrap());
    }
}

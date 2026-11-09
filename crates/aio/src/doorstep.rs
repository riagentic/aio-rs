//! The page a browser with no key is shown (aio's connect page, served by
//! the server itself): a phone or a tablet opens the server's address, pairs
//! with a 6-digit code (`ax pair <app> --plain`), and lands in the app, its
//! key in the HttpOnly cookie a `?token=` link sets. It is an installable
//! web app: Add to Home Screen gives an icon that opens the app (iOS keeps a
//! Home Screen app's cookies apart, so it pairs once more there), and an
//! offline shell that says the server can't be reached instead of a blank
//! error. Same origin as the app throughout: no CORS, no key in the page's
//! storage.
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};

/// The paths it is served at, all public (`/__aio` is the framework's).
pub const PAGE: &str = "/__aio/connect";
const SCRIPT: &str = "/__aio/connect.js";
const MANIFEST: &str = "/__aio/connect.webmanifest";
const WORKER: &str = "/__aio/connect-sw.js";
const ICON: &str = "/__aio/connect.png";

/// What the page's own requests may do: its own script and icon, its
/// inline style, and talk to this server only; framed by nobody.
const CSP: &str =
    "default-src 'none'; script-src 'self'; style-src 'unsafe-inline'; img-src 'self'; connect-src 'self'; \
     manifest-src 'self'; worker-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'";

/// Is this request a person's browser opening a page (not a script's fetch)?
pub fn navigated(headers: &HeaderMap) -> bool {
    let h = |k: &str| headers.get(k).and_then(|v| v.to_str().ok());
    match h("sec-fetch-mode") {
        Some(m) => m == "navigate",
        None => h("accept").is_some_and(|a| a.contains("text/html")),
    }
}

/// The page, for app `title` (pure).
pub fn page(title: &str) -> String {
    let t = title.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
<title>{t}: connect</title>
<link rel="manifest" href="{MANIFEST}">
<link rel="apple-touch-icon" href="{ICON}">
<meta name="apple-mobile-web-app-capable" content="yes">
<meta name="mobile-web-app-capable" content="yes">
<meta name="apple-mobile-web-app-title" content="{t}">
<meta name="color-scheme" content="light dark">
<style>
body {{ font: 17px/1.4 -apple-system, system-ui, sans-serif; margin: 0; min-height: 100vh; display: grid; place-items: center; }}
main {{ width: min(22rem, 100% - 2rem); padding: env(safe-area-inset-top) 0 env(safe-area-inset-bottom); }}
img {{ width: 64px; height: 64px; border-radius: 14px; }}
input, button {{ font: inherit; width: 100%; box-sizing: border-box; padding: .7rem; margin-top: .5rem; border-radius: 10px; }}
input {{ border: 1px solid #8888; letter-spacing: .3em; text-align: center; }}
button {{ border: 0; background: #0a64d8; color: #fff; }}
p.why {{ color: #c4321f; min-height: 1.4em; }}
small {{ color: #888; }}
</style>
</head>
<body>
<main>
<img src="{ICON}" alt="">
<h1>{t}</h1>
<form id="pair">
<label for="code">Pairing code</label>
<input id="code" name="code" inputmode="numeric" autocomplete="one-time-code" pattern="[0-9]{{6}}" maxlength="8" required>
<button type="submit">Connect</button>
</form>
<p class="why" id="why" role="alert"></p>
<small>On the app's computer: <code>ax pair &lt;app&gt; --plain</code> shows a code for 3 minutes.</small>
</main>
<script src="{SCRIPT}"></script>
</body>
</html>
"#
    )
}

/// The page's script: pair, then open the app; offline, say so.
const JS: &str = r#""use strict";
const why = document.getElementById("why");
const say = (t) => { why.textContent = t; };
const reach = () => fetch("/__aio/connect.webmanifest", { cache: "no-store" }).then(() => true, () => false);
reach().then((up) => { if (!up) say("Can't reach the server: is it running, and this device on its network? Pull to retry."); });
if ("serviceWorker" in navigator) {
  navigator.serviceWorker.getRegistration("/").then((r) => {
    if (!r) navigator.serviceWorker.register("/__aio/connect-sw.js", { scope: "/" }).catch(() => {});
  });
}
document.getElementById("pair").addEventListener("submit", async (e) => {
  e.preventDefault();
  const pin = document.getElementById("code").value.trim();
  if (pin.length === 8) return say("An 8-digit code pairs aio-rs's own clients and never leaves them: ask for a 6-digit one (ax pair <app> --plain).");
  if (!/^[0-9]{6}$/.test(pin)) return say("A pairing code has 6 digits.");
  say("");
  let r;
  try {
    r = await fetch("/__aio/pair", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ pin }) });
  } catch {
    return say("Can't reach the server.");
  }
  if (r.status === 404) return say("This server pairs no devices: open the link ax profile <app> --link prints.");
  const v = await r.json().catch(() => ({}));
  if (!r.ok || !v.key) return say([v.error, v.hint].filter(Boolean).join(": ") || "Pairing failed (" + r.status + ").");
  location.replace("/?token=" + encodeURIComponent(v.key));
});
"#;

/// The offline shell: a page opened with no network gets this one, which
/// says so. Installed only where no other worker serves the origin (the
/// app's web build's own replaces it).
const SW: &str = r#""use strict";
const C = "aio-connect-1";
self.addEventListener("install", (e) => e.waitUntil(caches.open(C).then((c) => c.addAll(["/__aio/connect", "/__aio/connect.js", "/__aio/connect.png"])).then(() => self.skipWaiting())));
self.addEventListener("activate", (e) => e.waitUntil(self.clients.claim()));
self.addEventListener("fetch", (e) => {
  const u = new URL(e.request.url);
  if (e.request.mode === "navigate") e.respondWith(fetch(e.request).catch(() => caches.match("/__aio/connect")));
  else if (u.pathname === "/__aio/connect.js" || u.pathname === "/__aio/connect.png") e.respondWith(fetch(e.request).catch(() => caches.match(u.pathname)));
});
"#;

/// The web manifest: installed, it opens the app's root (pure).
pub fn manifest(title: &str) -> String {
    serde_json::json!({
        "name": title,
        "short_name": title,
        "start_url": "/",
        "scope": "/",
        "display": "standalone",
        "icons": [{ "src": ICON, "sizes": "512x512", "type": "image/png", "purpose": "any" }],
    })
    .to_string()
}

fn reply(ty: &'static str, body: impl Into<axum::body::Body>, extra: &[(&'static str, &'static str)]) -> Response {
    let mut r = (StatusCode::OK, [(header::CONTENT_TYPE, ty), (header::CACHE_CONTROL, "no-cache")], body.into())
        .into_response();
    for (k, v) in extra {
        r.headers_mut().insert(*k, header::HeaderValue::from_static(v));
    }
    r
}

/// The page as the answer to a keyless navigation: 401, its body the page.
pub fn unpaired(title: &str) -> Response {
    let mut r = reply("text/html; charset=utf-8", page(title), &[("content-security-policy", CSP)]);
    *r.status_mut() = StatusCode::UNAUTHORIZED;
    r
}

/// `app` with the page's files, for app `title` in `hue`.
pub fn routes(app: axum::Router, title: &str, hue: u16) -> axum::Router {
    use axum::routing::get;
    let (html, man) = (page(title), manifest(title));
    // Drawn on the first ask, never at a start.
    let (name, png) = (title.to_string(), std::sync::Arc::new(std::sync::OnceLock::<Vec<u8>>::new()));
    app.route(PAGE, get(move || async move { reply("text/html; charset=utf-8", html, &[("content-security-policy", CSP)]) }))
        .route(SCRIPT, get(|| async { reply("text/javascript; charset=utf-8", JS, &[]) }))
        .route(WORKER, get(|| async { reply("text/javascript; charset=utf-8", SW, &[("service-worker-allowed", "/")]) }))
        .route(MANIFEST, get(move || async move { reply("application/manifest+json", man, &[]) }))
        .route(
            ICON,
            get(move || async move {
                reply("image/png", png.get_or_init(|| aio_config::icon::icon_png(&name, hue, 512)).clone(), &[])
            }),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_navigation_is_told_from_a_fetch() {
        let h = |kv: &[(&'static str, &'static str)]| {
            let mut m = HeaderMap::new();
            kv.iter().for_each(|(k, v)| _ = m.insert(*k, header::HeaderValue::from_static(v)));
            m
        };
        assert!(navigated(&h(&[("sec-fetch-mode", "navigate")])));
        assert!(!navigated(&h(&[("sec-fetch-mode", "cors"), ("accept", "text/html")])));
        assert!(navigated(&h(&[("accept", "text/html,application/xhtml+xml")])), "an older browser");
        assert!(!navigated(&h(&[])), "a tool");
    }

    #[test]
    fn the_page_names_the_app_escaped_and_is_installable() {
        let p = page("Notes <&> \"Co\"");
        assert!(p.contains("<title>Notes &lt;&amp;&gt; &quot;Co&quot;: connect</title>"), "{p}");
        assert!(p.contains(r#"<link rel="manifest" href="/__aio/connect.webmanifest">"#));
        assert!(p.contains(r#"<link rel="apple-touch-icon" href="/__aio/connect.png">"#));
        assert!(!p.contains("<script>"), "no inline script: the policy allows none");
        let m: serde_json::Value = serde_json::from_str(&manifest("Notes")).unwrap();
        assert_eq!((m["start_url"].as_str(), m["display"].as_str()), (Some("/"), Some("standalone")));
        assert!(SW.contains("caches.match(\"/__aio/connect\")") && JS.contains("location.replace(\"/?token=\""));
    }
}

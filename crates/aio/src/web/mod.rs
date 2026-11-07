//! The app in the browser's own page (`wasm32`, `ax build --web`): its
//! cells run there. A `Handle` is a cell of the page itself (the
//! standalone web app), kept in the browser's storage when it persists; a
//! `sync::Remote` is a synced copy of a server's cell over its `/ws`, as
//! a native copy is, whose calls made offline wait in the browser's storage
//! and reach the server once the page is linked again, twin tabs mirrored.
//! What a native build adds (the store's history, files, the server) is
//! left out; the browser keeps the rest.
pub mod storage;

#[cfg(target_arch = "wasm32")]
mod handle;
#[cfg(target_arch = "wasm32")]
pub mod notify;
#[cfg(target_arch = "wasm32")]
mod source;
#[cfg(target_arch = "wasm32")]
pub mod sync;
#[cfg(target_arch = "wasm32")]
pub use handle::Handle;
#[cfg(target_arch = "wasm32")]
pub use source::Source;

/// The `/ws` of the server that served the page (its `location`), as an aio-rs
/// copy, with the page's key if its address has one (`?token=`, a key
/// link); else the browser shows its cookie, the key's or a session's (pure).
pub fn ws_url(protocol: &str, host: &str, search: &str) -> String {
    let scheme = if protocol == "https:" { "wss" } else { "ws" };
    let token = search.trim_start_matches('?').split('&').find_map(|p| p.strip_prefix("token="));
    let token = token.map_or(String::new(), |t| format!("&token={t}"));
    format!("{scheme}://{host}/ws?wire=2{token}")
}

/// The page's clock (ms since the epoch): a browser has no system clock.
#[cfg(target_arch = "wasm32")]
pub(crate) fn now_ms() -> i64 {
    js_sys::Date::now() as i64
}

/// Wait `ms` on the page's timer.
#[cfg(target_arch = "wasm32")]
pub async fn sleep(ms: u32) {
    let p = js_sys::Promise::new(&mut |done, _| {
        if let Some(w) = web_sys::window() {
            _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(&done, ms as i32);
        }
    });
    _ = wasm_bindgen_futures::JsFuture::from(p).await;
}

/// Say `msg` on the page's console (a browser drops stderr).
#[cfg(target_arch = "wasm32")]
pub(crate) fn say(msg: &str) {
    web_sys::console::warn_1(&format!("[aio] {msg}").into());
}

/// The page's request to door `path` of the server that served it: same
/// origin, its cookies sent, `GET` without a body. The session stays in
/// the door's HttpOnly cookie ([`crate::auth::client::SESSION_HEADER`]),
/// never in the page's storage or address. The answer's status and JSON
/// (`null` when it has none).
#[cfg(target_arch = "wasm32")]
pub(crate) async fn fetch(
    path: &str,
    token: Option<&str>,
    body: Option<&serde_json::Value>,
) -> std::io::Result<(u16, serde_json::Value)> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    let err = |e: wasm_bindgen::JsValue| std::io::Error::other(format!("{e:?}"));
    let w = web_sys::window().ok_or_else(|| std::io::Error::other("no window"))?;
    let headers = web_sys::Headers::new().map_err(err)?;
    headers.set("content-type", "application/json").map_err(err)?;
    headers.set(crate::auth::client::SESSION_HEADER, "cookie").map_err(err)?;
    if let Some(t) = token.filter(|t| !t.is_empty()) {
        headers.set("authorization", &format!("Bearer {t}")).map_err(err)?;
    }
    let init = web_sys::RequestInit::new();
    init.set_method(if body.is_some() { "POST" } else { "GET" });
    init.set_credentials(web_sys::RequestCredentials::SameOrigin);
    init.set_headers(&headers);
    if let Some(b) = body {
        init.set_body(&b.to_string().into());
    }
    let r: web_sys::Response =
        JsFuture::from(w.fetch_with_str_and_init(path, &init)).await.map_err(err)?.dyn_into().map_err(err)?;
    let text = JsFuture::from(r.text().map_err(err)?).await.map_err(err)?.as_string().unwrap_or_default();
    Ok((r.status(), serde_json::from_str(&text).unwrap_or_default()))
}

/// The login doors of the server that served this page (aio's browser
/// `authClient`), for `aio::kit::SignIn`: a sign-in sets the session's
/// HttpOnly cookie, which links this page's copies as that user once it
/// [`restart`]s.
#[cfg(target_arch = "wasm32")]
pub fn client() -> crate::auth::client::Client {
    crate::auth::client::Client::new("", None)
}

/// The page starts again (a reload): what a sign-in or sign-out changed
/// (its cookie, its user) is then what its copies link as.
#[cfg(target_arch = "wasm32")]
pub fn restart() {
    if let Some(w) = web_sys::window() {
        _ = w.location().reload();
    }
}

/// Sign this browser out (aio's `signOut`): its session ends, its cookie is
/// cleared, and the page starts again signed out.
#[cfg(target_arch = "wasm32")]
pub async fn sign_out() {
    _ = client().logout("").await;
    restart();
}

/// Who this page is signed in as (aio's `useUser`), as `(id, role)`: its
/// `get()` is `None` while the server is asked, then `Some(None)` signed out
/// (or no accounts). A hook; read it in a hole.
#[cfg(all(target_arch = "wasm32", feature = "kit"))]
pub fn use_user() -> crate::ui::Resource<Option<(String, String)>> {
    crate::ui::resource(|| async { client().me(None).await.ok().and_then(|m| m.user) })
}

/// The page's `localStorage` as a [`storage::Kv`]: none in a context that
/// has it switched off (then copies live in memory only).
#[cfg(target_arch = "wasm32")]
pub(crate) struct Local;

#[cfg(target_arch = "wasm32")]
impl Local {
    fn get_store() -> Option<web_sys::Storage> {
        web_sys::window()?.local_storage().ok().flatten()
    }
}

#[cfg(target_arch = "wasm32")]
impl storage::Kv for Local {
    fn get(&self, key: &str) -> Option<String> {
        Self::get_store()?.get_item(key).ok().flatten()
    }

    fn set(&self, key: &str, value: &str) -> Result<(), String> {
        let s = Self::get_store().ok_or("this page has no localStorage")?;
        s.set_item(key, value).map_err(|e| format!("{e:?}"))
    }

    fn remove(&self, key: &str) {
        if let Some(s) = Self::get_store() {
            _ = s.remove_item(key);
        }
    }

    fn keys(&self) -> Vec<String> {
        let Some(s) = Self::get_store() else { return Vec::new() };
        (0..s.length().unwrap_or(0)).filter_map(|i| s.key(i).ok().flatten()).collect()
    }
}

/// Mount the app's root view into the page's `<div id="main">`, on air:
/// its interpreter (`air::PAGE_JS`) runs in the page as a script of the
/// page's own making (its policy allows `blob:`), and talks to the views
/// in this module with no socket: each flush's edits handed to it as a
/// view of the module's memory (no copy), its events straight to the
/// root. Set the cells' sources (`aio::client!`'s `SOURCE`) first. The
/// page's splash goes once the view is drawn (`aio:mounted`). No server
/// paint on air: the page's view is the first.
#[cfg(all(target_arch = "wasm32", feature = "web", feature = "kit"))]
pub fn launch(root: fn() -> crate::ui::View) {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::*;
    // A `notify` tell is the page's notification, unless the app shows its own.
    crate::tell::show_unless_set(notify::show);
    let open = SYNCING.with_borrow(Clone::clone);
    for (app, cells) in &open {
        warn_orphans(app, &cells.iter().map(String::as_str).collect::<Vec<_>>());
    }
    let Some(w) = web_sys::window() else { return };
    // The wire the interpreter talks to: its frames to the root, here.
    let wire = js_sys::Object::new();
    let send = Closure::<dyn Fn(js_sys::Uint8Array)>::new(|f: js_sys::Uint8Array| {
        let f = f.to_vec();
        on_air::with_root(|r| _ = r.event(&f));
        on_air::flush();
    });
    let listen = Closure::<dyn Fn(JsValue)>::new(|_: JsValue| {});
    _ = js_sys::Reflect::set(&wire, &"send".into(), send.as_ref());
    _ = js_sys::Reflect::set(&wire, &"listen".into(), listen.as_ref());
    send.forget();
    listen.forget();
    _ = js_sys::Reflect::set(&w, &"__aioWire".into(), &wire);
    // The interpreter, then the views once it has run.
    let started = Closure::once_into_js(move || {
        air::on_wake(on_air::woken);
        on_air::mount(root);
        mounted();
    });
    // Fetched with the page: without its indentation and comment lines.
    let parts = js_sys::Array::of1(&crate::shrunk!(js, air::PAGE_JS).into());
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type("text/javascript");
    let (Some(doc), Ok(blob)) = (w.document(), web_sys::Blob::new_with_str_sequence_and_options(&parts, &opts)) else {
        return say("the page can't take air's interpreter");
    };
    let (Ok(url), Ok(el)) = (web_sys::Url::create_object_url_with_blob(&blob), doc.create_element("script")) else {
        return say("the page can't take air's interpreter");
    };
    _ = el.set_attribute("src", &url);
    _ = el.add_event_listener_with_callback("load", started.unchecked_ref());
    match doc.head() {
        Some(h) => _ = h.append_child(&el),
        None => say("the page has no <head> for air's interpreter"),
    }
}

/// The views of the page, on air: one root, flushed after each event, and
/// whenever a task wakes (once per turn of the page's event loop).
#[cfg(all(target_arch = "wasm32", feature = "web", feature = "kit"))]
mod on_air {
    use std::cell::{Cell, RefCell};

    thread_local! {
        static ROOT: RefCell<Option<air::Root>> = const { RefCell::new(None) };
        static QUEUED: Cell<bool> = const { Cell::new(false) };
    }

    pub(super) fn mount(app: fn() -> crate::ui::View) {
        let root = air::Root::new();
        root.show(app);
        #[cfg(feature = "dev")]
        root.within(crate::ui::audit::watch_page);
        ROOT.set(Some(root));
        flush();
    }

    pub(super) fn with_root(f: impl FnOnce(&air::Root)) {
        ROOT.with_borrow(|r| r.as_ref().map(f));
    }

    /// The views' work done and their edits applied by the interpreter.
    pub(super) fn flush() {
        air::run_tasks();
        ROOT.with_borrow(|r| {
            let Some(r) = r else { return };
            let edits = r.take_edits();
            if !edits.is_empty() {
                // SAFETY: a view of this module's memory, read by the
                // interpreter at once (it allocates nothing of ours meanwhile).
                let view = unsafe { js_sys::Uint8Array::view(&edits) };
                let apply = web_sys::window().and_then(|w| js_sys::Reflect::get(&w, &"__airApply".into()).ok());
                let apply = apply.and_then(|f| wasm_bindgen::JsCast::dyn_into::<js_sys::Function>(f).ok());
                match apply.map(|f| f.call1(&wasm_bindgen::JsValue::NULL, &view)) {
                    Some(Ok(_)) => {}
                    Some(Err(e)) => super::say(&format!("air's interpreter failed: {e:?}")),
                    None => super::say("air's interpreter is not in the page"),
                }
                // The frame is applied: what rows that left held goes now,
                // not at the next event (a page with none would keep it).
                air::drop_left();
            }
            for e in r.uncaught() {
                super::say(&format!("a view failed and no ErrorBoundary is above it: {e}"));
            }
        });
    }

    /// A task woke: flush once the page's current work is done.
    pub(super) fn woken() {
        if QUEUED.replace(true) {
            return;
        }
        let run = wasm_bindgen::closure::Closure::once_into_js(|| {
            QUEUED.set(false);
            flush();
        });
        if let Some(w) = web_sys::window() {
            _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(wasm_bindgen::JsCast::unchecked_ref(&run), 0);
        }
    }
}

/// Say the app's view is drawn: `aio:mounted` on the document.
#[cfg(all(target_arch = "wasm32", feature = "web"))]
fn mounted() {
    let d = web_sys::window().and_then(|w| w.document());
    if let (Some(d), Ok(e)) = (d, web_sys::Event::new("aio:mounted")) {
        _ = d.dispatch_event(&e);
    }
}

#[cfg(target_arch = "wasm32")]
thread_local! {
    /// The cells this page's copies sync, by app.
    static SYNCING: std::cell::RefCell<std::collections::BTreeMap<String, Vec<String>>> = Default::default();
}

/// A copy of app `app`'s cell `cell` opened in this page.
#[cfg(target_arch = "wasm32")]
pub(crate) fn syncing(app: &str, cell: &str) {
    SYNCING.with_borrow_mut(|s| s.entry(app.into()).or_default().push(cell.into()));
}

/// Warn about the calls stored for app `app`'s cells that no copy in this
/// page syncs (`syncing`): nothing will ever send them (aio's orphan-queue
/// warning). The documents are left as they are.
#[cfg(target_arch = "wasm32")]
pub fn warn_orphans(app: &str, syncing: &[&str]) {
    for (cell, n) in storage::orphans(&Local, app, syncing) {
        say(&format!(
            "{n} unsent calls of cell \"{cell}\" are stored in this browser, but no copy here syncs it: \
             they are never sent (\"{}\")",
            storage::key(app, &cell)
        ));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_page_links_its_own_server_as_an_aio_rs_copy_with_its_key_if_its_address_has_one() {
        assert_eq!(super::ws_url("http:", "127.0.0.1:8080", ""), "ws://127.0.0.1:8080/ws?wire=2");
        assert_eq!(super::ws_url("https:", "app.example", "?a=1&token=K&b=2"), "wss://app.example/ws?wire=2&token=K");
    }
}

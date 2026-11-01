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
#[cfg(all(target_arch = "wasm32", feature = "web"))]
mod page;
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

/// Who this page is signed in as (aio's `useUser`), as `(id, role)`: `None`
/// while the server is asked, then `Some(None)` signed out (or no
/// accounts). A hook.
#[cfg(all(target_arch = "wasm32", feature = "kit"))]
pub fn use_user() -> Option<Option<(String, String)>> {
    let me = dioxus::prelude::use_resource(|| async { client().me(None).await.ok().and_then(|m| m.user) });
    me()
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

/// Mount the app's root view into the page's `<div id="main">`: the real
/// DOM, edited as the view changes, its events the view's. Set the cells'
/// sources (`aio::client!`'s `SOURCE`) first. A page painted on the server
/// (`aio_shell::web::painted`) shows its paint until the view is drawn
/// beside it, unseen, from the page's own copies; then the view takes its
/// place in one step, with what a person did to the paint meanwhile (its
/// scroll, focus, typed text: [`crate::page::ADOPT`]). Not adopted in place (hydration): dioxus-web 0.7's
/// `rehydrate` binds the view's node ids to the paint's
/// `data-node-hydration` marks by position and never checks they match,
/// and the page's first view may differ from the paint (its copy is the
/// one this browser kept, or newer). A mismatch then leaves ids on the
/// wrong nodes or on none, and the next edit throws in the interpreter;
/// it also needs dioxus-fullstack's serialized hydration data in the page.
#[cfg(all(target_arch = "wasm32", feature = "web"))]
pub fn launch(root: fn() -> dioxus_core::Element) {
    use dioxus_core::VirtualDom;
    // The copies are open now: what no copy syncs is said.
    let open = SYNCING.with_borrow(Clone::clone);
    for (app, cells) in &open {
        warn_orphans(app, &cells.iter().map(String::as_str).collect::<Vec<_>>());
    }
    let doc = web_sys::window().and_then(|w| w.document());
    let main = doc.as_ref().and_then(|d| d.get_element_by_id("main"));
    let next = match (&doc, &main) {
        (Some(d), Some(m)) if m.has_child_nodes() => d.create_element("div").ok().filter(|n| {
            _ = n.set_attribute("hidden", "");
            m.after_with_node_1(n).is_ok()
        }),
        _ => None,
    };
    // The page's own document: its DOM, never `eval`.
    let doc = || std::rc::Rc::new(page::Page) as std::rc::Rc<dyn dioxus_document::Document>;
    /// The view, and once it is drawn, the page told (`aio:mounted`: its
    /// splash goes, `page.js`).
    fn drawn(app: fn() -> dioxus_core::Element) -> dioxus_core::Element {
        dioxus_core::use_hook(|| dioxus_core::queue_effect(mounted));
        app()
    }
    let Some(next) = next else {
        let dom = VirtualDom::new_with_props(drawn, root).with_root_context(doc());
        return dioxus_web::launch::launch_virtual_dom(dom, dioxus_web::Config::new().rootname("main"));
    };
    /// The view, and once it is drawn, its place taken from the paint.
    fn take_place(app: fn() -> dioxus_core::Element) -> dioxus_core::Element {
        dioxus_core::use_hook(|| {
            dioxus_core::queue_effect(|| {
                let d = web_sys::window().and_then(|w| w.document());
                let Some(main) = d.and_then(|d| d.get_element_by_id("main")) else { return };
                if let Some(next) = main.next_element_sibling() {
                    // With what was done to the paint (`page::ADOPT`, in page.js).
                    let adopt = web_sys::window()
                        .and_then(|w| js_sys::Reflect::get(&w, &"__aioAdopt".into()).ok())
                        .and_then(|f| wasm_bindgen::JsCast::dyn_into::<js_sys::Function>(f).ok());
                    if adopt.is_none_or(|f| f.call2(&wasm_bindgen::JsValue::NULL, &main, &next).is_err()) {
                        main.remove();
                        next.set_id("main");
                        _ = next.remove_attribute("hidden");
                    }
                }
                mounted();
            })
        });
        app()
    }
    let dom = VirtualDom::new_with_props(take_place, root).with_root_context(doc());
    dioxus_web::launch::launch_virtual_dom(dom, dioxus_web::Config::new().rootelement(next));
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

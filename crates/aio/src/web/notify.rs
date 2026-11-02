//! A `notify` tell in the browser (aio's browser notifications): the
//! page's own `Notification`, the app's icon on it, a click bringing the
//! page forward and going to the tell's `route` ([`on_click`]). A browser
//! grants notifications from a user gesture only: the first tell asks, and
//! one asked outside a gesture (or blocked) is said once on the console
//! with the fix, [`request_permission`] in a click handler. Where a page
//! may not make one itself (Chrome on Android) the app's service worker
//! shows it.
use crate::tell::Tell;
use std::cell::Cell;
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use web_sys::{Notification, NotificationOptions, NotificationPermission};

/// The app's router's `go`.
type Go = fn(&str) -> bool;

thread_local! {
    static GO: Cell<Option<Go>> = const { Cell::new(None) };
    static SAID: Cell<bool> = const { Cell::new(false) };
}

/// Where a click on a notification goes: `go` takes its `route` (the app's
/// `router!` `go`).
pub fn on_click(go: Go) {
    GO.set(Some(go));
}

/// Ask for the permission now: call it in a click handler (a browser asks
/// only from a gesture), e.g. an "Enable alerts" button.
pub fn request_permission() {
    if supported() {
        _ = Notification::request_permission();
    }
}

fn supported() -> bool {
    web_sys::window().is_some_and(|w| js_sys::Reflect::has(&w, &"Notification".into()).unwrap_or(false))
}

fn say_once(msg: &str) {
    if !SAID.replace(true) {
        super::say(msg);
    }
}

/// Show tell `t` in this page, as the desktop shows it.
pub(crate) fn show(t: &Tell) {
    let Tell::Notify { title, body, tag, silent, route } = t else { return };
    if !supported() {
        return say_once(&format!("notify: this browser has no notifications: \"{title}\" was not shown"));
    }
    let opts = NotificationOptions::new();
    opts.set_body(body);
    opts.set_icon("./icon-192.png");
    opts.set_silent(Some(*silent));
    if let Some(tag) = tag {
        opts.set_tag(tag);
    }
    let (title, route) = (title.clone(), route.clone());
    match Notification::permission() {
        NotificationPermission::Granted => post(&title, &opts, route),
        NotificationPermission::Denied => say_once(&format!(
            "notify: notifications are blocked for this site (the browser's site settings): \"{title}\" was not shown"
        )),
        _ => {
            let Ok(asked) = Notification::request_permission() else { return };
            let then = Closure::once(move |_: JsValue| match Notification::permission() {
                NotificationPermission::Granted => post(&title, &opts, route),
                _ => say_once(&format!(
                    "notify: no permission for notifications yet (a browser asks only from a click): call \
                     aio::web::notify::request_permission() in a click handler. \"{title}\" was not shown"
                )),
            });
            _ = asked.then(&then);
            then.forget();
        }
    }
}

fn post(title: &str, opts: &NotificationOptions, route: Option<String>) {
    let n = match Notification::new_with_options(title, opts) {
        Ok(n) => n,
        // A page that may not construct one (Chrome on Android): its worker's.
        Err(_) => return by_worker(title, opts),
    };
    let click = Closure::<dyn FnMut()>::new(move || {
        if let Some(w) = web_sys::window() {
            _ = w.focus();
        }
        if let (Some(r), Some(go)) = (route.as_deref(), GO.get()) {
            go(r);
        } else if route.is_some() {
            super::say("notify: a click asked for a route, but no view takes it: call aio::web::notify::on_click(go)");
        }
    });
    n.set_onclick(Some(click.as_ref().unchecked_ref()));
    click.forget();
}

/// `navigator.serviceWorker.ready.then((r) => r.showNotification(title, opts))`.
fn by_worker(title: &str, opts: &NotificationOptions) {
    let Some(w) = web_sys::window() else { return };
    let get = |o: &JsValue, k: &str| js_sys::Reflect::get(o, &k.into()).ok().filter(|v| !v.is_undefined());
    let ready = get(&w, "navigator").and_then(|n| get(&n, "serviceWorker")).and_then(|sw| get(&sw, "ready"));
    let Some(ready) = ready.and_then(|r| r.dyn_into::<js_sys::Promise>().ok()) else {
        return say_once(&format!("notify: this page can't show notifications: \"{title}\" was not shown"));
    };
    let (title, opts) = (JsValue::from(title), JsValue::from(opts.clone()));
    let show = Closure::once(move |reg: JsValue| {
        if let Some(f) = get(&reg, "showNotification").and_then(|f| f.dyn_into::<js_sys::Function>().ok()) {
            _ = f.call2(&reg, &title, &opts);
        }
    });
    _ = ready.then(&show);
    show.forget();
}

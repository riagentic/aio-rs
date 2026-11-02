// The page of the web build's test: a synced copy of the server's board,
// opened when the page starts, its view mounted, driven from the test
// through its DOM and its exports. At /signin, a server with accounts:
// who the page is, its sign-in form, and its copy once signed in.

use wasm_bindgen::prelude::*;
include!("board.rs");
include!("tally.rs");
include!("view.rs");

thread_local! {
    static COPY: std::cell::RefCell<Option<aio::sync::Remote<Board>>> = Default::default();
    /// The votes' copy, when the server serves them.
    static VOTES: std::cell::RefCell<Option<aio::sync::Remote<Votes>>> = Default::default();
}

fn copy() -> aio::sync::Remote<Board> {
    COPY.with_borrow(|c| c.clone().expect("the copy is open"))
}

#[wasm_bindgen(start)]
fn start() {
    if pathname() == "/signin" {
        return signin();
    }
    wasm_bindgen_futures::spawn_local(async {
        let opened = aio::sync::Remote::<Board>::open("web-test").await;
        let ready = match opened {
            Ok(r) => {
                COPY.with_borrow_mut(|c| *c = Some(r.clone()));
                _ = SOURCE.set(aio::Source::Remote(r));
                aio::web::launch(view);
                if let Ok(v) = aio::sync::Remote::<Votes>::open("web-test").await {
                    VOTES.with_borrow_mut(|c| *c = Some(v));
                }
                "yes".to_string()
            }
            Err(e) => e.to_string(),
        };
        _ = js_sys::Reflect::set(&js_sys::global(), &"ready".into(), &ready.into());
    });
}

fn pathname() -> String {
    web_sys::window().and_then(|w| w.location().pathname().ok()).unwrap_or_default()
}

/// Linked when its cookie holds a session, else not (signed out, perhaps
/// with the copy this browser kept); the view either way.
fn signin() {
    wasm_bindgen_futures::spawn_local(async {
        let ready = match aio::sync::Remote::<Board>::open("web-test").await {
            Ok(r) if r.status().online => {
                COPY.with_borrow_mut(|c| *c = Some(r));
                "linked"
            }
            _ => "unlinked",
        };
        aio::web::launch(who);
        _ = js_sys::Reflect::set(&js_sys::global(), &"ready".into(), &ready.into());
    });
}

fn who() -> View {
    match aio::web::use_user() {
        None => rsx! { p { id: "who", "asking" } },
        Some(None) => rsx! {
            p { id: "who", "nobody" }
            aio::kit::SignIn { client: aio::web::client(), on_signed: |_| aio::web::restart() }
        },
        Some(Some((id, role))) => rsx! {
            p { id: "who", "{id} {role}" }
            button { id: "out", onclick: |_| aio::web::sign_out(), "Sign out" }
        },
    }
}

#[wasm_bindgen]
pub fn add(text: String) -> bool {
    copy().add(text).is_ok()
}

thread_local! {
    static TALLY: aio::Handle<Tally> = aio::Handle::saved("web-test");
}

#[wasm_bindgen]
pub fn bump() -> bool {
    TALLY.with(|t| t.bump().is_ok())
}

#[wasm_bindgen]
pub fn finish() -> bool {
    TALLY.with(|t| t.finish().is_ok())
}

#[wasm_bindgen]
pub fn later() -> bool {
    TALLY.with(|t| t.later().is_ok())
}

#[wasm_bindgen]
pub fn tally() -> String {
    TALLY.with(|t| {
        let s = t.state();
        aio::serde_json::json!({ "n": s.n, "busy": s.busy, "running": t.pending(None) }).to_string()
    })
}

/// What the page shows, as JSON.
#[wasm_bindgen]
pub fn shown() -> String {
    let r = copy();
    let s = r.status();
    aio::serde_json::json!({ "items": r.state().items, "pending": s.pending, "online": s.online }).to_string()
}

/// The votes as the page's copy shows them (`null`: not served).
#[wasm_bindgen]
pub fn votes() -> String {
    VOTES.with_borrow(|v| match v {
        Some(v) => aio::serde_json::json!({ "n": v.state().n, "online": v.status().online }).to_string(),
        None => "null".into(),
    })
}

#[wasm_bindgen]
pub fn vote() -> bool {
    VOTES.with_borrow(|v| v.as_ref().is_some_and(|v| v.vote().is_ok()))
}

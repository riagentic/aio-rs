//! A client's connect page (aio's thin client): an address, the app's
//! servers on the LAN, a pairing code, the recent servers. `aio_shell`
//! shows it in a client's window in place of the app while the client has
//! no server it can reach ([`crate::connect`]); it goes the moment one
//! answers. Parts: `connectWhy`, `connectAddr`, `connectCode`,
//! `connectGo`, `connectError`, `connectLook`, `near{n}`, `recent{n}`,
//! `forget{n}`.
use super::{Alert, Button, Field, Input, Tone, Variant, join};
use crate::connect::{Near, Server};
use crate::ui::prelude::*;
use std::path::PathBuf;

/// How long the LAN is asked.
const LOOK: std::time::Duration = std::time::Duration::from_millis(1200);

/// Why the connect page shows (`None`: the app does), following
/// [`crate::connect::page`].
pub fn use_connect_page() -> Signal<Option<String>> {
    let mut rx = crate::connect::page();
    let why = signal(|| rx.borrow_and_update().clone());
    spawn(async move {
        while rx.changed().await.is_ok() {
            why.set(rx.borrow_and_update().clone());
        }
    });
    why
}

/// `f` on aio's runtime (sockets, files); a page asks on its own tasks.
async fn asked<R: Send + 'static>(f: impl Future<Output = std::io::Result<R>> + Send + 'static) -> Result<R, String> {
    match crate::rt().spawn(f).await {
        Ok(r) => r.map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// Where a connect goes: a recent server again, or an address with the
/// code it shows (blank: no pairing).
#[derive(Clone)]
enum To {
    Again(Server),
    At(String, String),
}

/// The page for the client of app `app` keeping its data in `data`.
#[component]
pub fn ConnectPage(
    data: PathBuf,
    app: String,
    #[props(default, into)] class: String,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> View {
    let why = use_connect_page();
    let (addr, code) = (signal(String::new), signal(String::new));
    let error = signal(|| None::<String>);
    let busy = signal(|| false);
    let recent = signal({
        let data = data.clone();
        move || crate::connect::recent(&data)
    });
    let near = {
        let app = app.clone();
        resource(move || {
            let app = app.clone();
            async move { asked(async move { crate::connect::lan(&app, LOOK).await }).await }
        })
    };
    // Connect: by code (pairing), or plain to the address; a recent one with its link.
    let go = {
        let (data, app) = (data.clone(), app.clone());
        move |to: To| {
            if busy.get() {
                return;
            }
            busy.set(true);
            error.set(None);
            let (data, app, shown) = (data.clone(), app.clone(), data.clone());
            spawn(async move {
                let r = asked(async move {
                    match to {
                        To::Again(s) => crate::connect::again(&data, &s),
                        To::At(a, c) if c.trim().is_empty() => crate::connect::open_at(&data, &a),
                        To::At(a, c) => crate::connect::pair(&data, &app, &a, &c).await.map(|_| ()),
                    }
                })
                .await;
                busy.set(false);
                match r {
                    Ok(()) => recent.set(crate::connect::recent(&shown)),
                    Err(e) => error.set(Some(e)),
                }
            });
        }
    };
    let typed = go.clone();
    let submit = move |e: FormEvent| {
        e.prevent_default();
        typed(To::At(addr.get(), code.get()));
    };
    let forget = {
        let data = data.clone();
        move |a: String| {
            _ = crate::connect::forget(&data, &a);
            recent.set(crate::connect::recent(&data));
        }
    };
    let rows = memo(move || match near.get() {
        Some(Ok(n)) => n,
        _ => vec![],
    });
    let pick = {
        let go = go.clone();
        move |n: Near| {
            addr.set(n.addr.clone());
            if !n.auth {
                go(To::At(n.addr, String::new()));
            }
        }
    };
    rsx! {
        main { class: join("aio-connect", &class), ..attributes,
            h1 { "Connect to a server" }
            Alert { tone: Tone::Warn, "data-t": "connectWhy", {why.get().unwrap_or_default()} }
            form { onsubmit: submit,
                Field { label: "Address",
                    Input { value: addr.get(), "name": "addr", placeholder: "192.168.1.20:8100", "data-t": "connectAddr", oninput: move |v| addr.set(v) }
                }
                Field { label: "Pairing code", hint: "what ax pair shows on the server's machine; none for a server without a key",
                    Input { value: code.get(), "name": "code", "inputmode": "numeric", "autocomplete": "one-time-code", "data-t": "connectCode", oninput: move |v| code.set(v) }
                }
                Button { variant: Variant::Primary, r#type: "submit", disabled: busy.get(), "data-t": "connectGo", "Connect" }
            }
            if error.with(Option::is_some) {
                Alert { tone: Tone::Error, "data-t": "connectError", {error.get().unwrap_or_default()} }
            }
            h2 { "On this network" }
            if let Some(Err(e)) = near.get() {
                p { "{e}" }
            }
            if near.get().is_some() && rows.with(Vec::is_empty) {
                p { "No server of this app answered." }
            }
            ul {
                for (i, n) in rows.get().into_iter().enumerate() {
                    li { key: "{n.addr}",
                        button { r#type: "button", "data-t": format!("near{i}"), onclick: { let (pick, n) = (pick.clone(), n.clone()); move |_| pick(n.clone()) },
                            {n.title.clone().unwrap_or_else(|| n.addr.clone())} " " {n.addr.clone()}
                            if n.auth { " (needs its code)" }
                        }
                    }
                }
            }
            Button { variant: Variant::Ghost, "data-t": "connectLook", onclick: move |_| near.restart(), "Look again" }
            if recent.with(|r| !r.is_empty()) {
                h2 { "Recent" }
                ul {
                    for (i, s) in recent.get().into_iter().enumerate() {
                        li { key: "{s.addr}",
                            button { r#type: "button", "data-t": format!("recent{i}"), onclick: { let (go, s) = (go.clone(), s.clone()); move |_| go(To::Again(s.clone())) },
                                {s.title.clone().unwrap_or_else(|| s.addr.clone())} " " {s.addr.clone()}
                            }
                            button { r#type: "button", "aria-label": "Forget", "data-t": format!("forget{i}"), onclick: { let (forget, a) = (forget.clone(), s.addr.clone()); move |_| forget(a.clone()) }, "×" }
                        }
                    }
                }
            }
        }
    }
}

//! Islands (aio's `island`, `reactIsland`): a box of the page drawn by the
//! app's own JS (an npm library, a React component, WebGL), with typed props
//! in and typed events out. The view owns where the box is; the JS owns what
//! is inside it, and the view never diffs there.
//!
//! An [`Island`]'s script is the body of a function that returns
//! `mount(el, props, emit)`, which draws into `el` and returns
//! `{ update(props), unmount() }` (both optional), or just its unmount
//! function. It runs once per page, shared by every box of that island;
//! `emit(v)` sends `v` to the box's `onevent`. A prop change calls `update`
//! in place (no remount, the JS keeps its state; with no `update`: unmount,
//! then mount anew); the box going away calls `unmount`. A script or `mount` that
//! throws leaves the box as it was (its `loading`) and says which half
//! failed in the app's log.
//!
//! **Trust.** An island's script runs in the page itself: it shares the
//! page's DOM and its wire to the app, so it acts with the app's rights
//! (the page's policy still holds: no `eval`, no connection, no script from
//! elsewhere). That is right for the app's own code. A library the app
//! doesn't vouch for, or one fed untrusted input, goes in a
//! [`sandboxed`](Island::sandboxed) island: an iframe of an origin of its
//! own, with no page DOM, no wire, no storage and no cookies, reached only
//! through its props and `emit`. It draws into its own document (`el` is
//! its `<body>`, the box's size; the app's CSS is not there).
//!
//! The page loads nothing from the network: bundle the library into one
//! script (`esbuild x.js --bundle --format=iife --global-name=m
//! --footer:js='return m.default'`) and give it with `include_str!`.
use super::join;
use crate::ui::prelude::*;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

/// An island's script, and how it mounts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Island {
    src: &'static str,
    react: bool,
    sandbox: bool,
}

impl Island {
    /// A script returning `mount(el, props, emit)`.
    pub const fn js(src: &'static str) -> Self {
        Self { src, react: false, sandbox: false }
    }

    /// A React component (aio's `reactIsland`): a script returning
    /// `{ react, reactDomClient, component }` (React, `react-dom/client`, and
    /// the component or its module), all from the app's own bundle, so aio
    /// depends on no React. The component gets the props, and `emit`.
    pub const fn react(src: &'static str) -> Self {
        Self { src, react: true, sandbox: false }
    }

    /// Run it in a sandboxed iframe of its own (see the module's Trust):
    /// for a library the app doesn't vouch for. Each box gets its own
    /// frame; props, `update`, `unmount` and `emit` work as in the page.
    pub const fn sandboxed(self) -> Self {
        Self { sandbox: true, ..self }
    }

    /// The script's key in the page: one per script and kind.
    fn key(&self) -> String {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (self.src, self.react, self.sandbox).hash(&mut h);
        format!("{:016x}", h.finish())
    }

    /// The expression for its `mount`.
    fn module(&self) -> String {
        let run = format!("(function(){{\n{}\n}})()", self.src);
        if !self.react {
            return run;
        }
        format!(
            "((m) => {{ const C = m.component && m.component.default || m.component;\
             return (el, props, emit) => {{ const root = m.reactDomClient.createRoot(el);\
             const draw = (p) => root.render(m.react.createElement(C, Object.assign({{}}, p, {{ emit }})));\
             draw(props); return {{ update: draw, unmount: () => root.unmount() }}; }}; }})({run})"
        )
    }
}

/// The islands' scripts already sent to this page (one view per page).
#[derive(Clone, Default)]
struct Sent(Rc<RefCell<HashSet<String>>>);

/// What the page has of a box: the script and props it mounted with, and
/// the task hearing it.
struct Shown {
    key: String,
    props: Value,
    task: Task,
}

/// A box drawn by `island`'s JS with `props` (sent as JSON; a change calls
/// its `update`, a new script remounts it). `emit(v)` from the JS reaches
/// `onevent` as an `E`; one that doesn't parse as `E` is said in the log.
/// `loading` shows until it mounted. `children` are the view's, inside the
/// box: a script that only acts on its box (aio's `use=` actions: a tooltip,
/// a drag handle) leaves them be.
#[component]
pub fn WebIsland<P: Serialize + Clone + PartialEq + 'static, E: DeserializeOwned + 'static>(
    island: Island,
    props: P,
    #[props(default)] onevent: EventHandler<E>,
    loading: Option<View>,
    #[props(default, into)] class: String,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: View,
) -> View {
    let id = super::use_id("island");
    let sent = once(|| try_context::<Sent>().unwrap_or_else(|| provide_root(Sent::default())));
    // The window's document, kept: at unmount there is no scope to ask.
    let doc = once(crate::ui::Page::here);
    // `None` until the box is in the page.
    let shown = once(|| Rc::new(RefCell::new(None::<Shown>)));
    let ready = signal(|| false);
    let now = serde_json::to_value(&props).unwrap_or(Value::Null);
    let start = {
        let (doc, id, now) = (doc.clone(), id.clone(), now.clone());
        move || mount(&doc, &sent, island, &id, now.clone(), ready, onevent)
    };
    match shown.borrow_mut().as_mut() {
        Some(s) if s.key != island.key() => {
            s.task.cancel();
            _ = doc.eval(unmount_js(&id));
            *s = start();
        }
        Some(s) if s.props != now => {
            _ = doc.eval(update_js(&id, &now));
            s.props = now;
        }
        _ => {}
    }
    {
        let (doc, id) = (doc.clone(), id.clone());
        on_cleanup(move || _ = doc.eval(unmount_js(&id)));
    }
    rsx! {
        div {
            class: join("aio-island", &class),
            "data-aio-island": "{id}",
            onmounted: move |_| {
                let mut s = shown.borrow_mut();
                if s.is_none() {
                    *s = Some(start());
                }
            },
            ..attributes,
            if !ready.get() {
                {loading}
            }
            {children}
        }
    }
}

/// Mount box `id` in the page, sending `island`'s script first if this page
/// has none of it yet, and hear it: mounted, its events, its failures.
fn mount<E: DeserializeOwned + 'static>(
    doc: &crate::ui::Page,
    sent: &Sent,
    island: Island,
    id: &str,
    props: Value,
    ready: Signal<bool>,
    onevent: EventHandler<E>,
) -> Shown {
    let key = island.key();
    let js = match island.sandbox {
        true => sandbox_js(&island.module(), id, &props),
        false => {
            mount_js(&key, sent.0.borrow_mut().insert(key.clone()).then(|| island.module()).as_deref(), id, &props)
        }
    };
    let mut ev = doc.eval(js);
    let task = spawn(async move {
        while let Ok(m) = ev.recv::<Value>().await {
            match m["t"].as_str() {
                Some("mounted") => ready.set(true),
                Some("event") => match serde_json::from_value::<E>(m["v"].clone()) {
                    Ok(e) => onevent.call(e),
                    Err(e) => crate::warn!(
                        cat: "ui",
                        "[island] an event is not a {}: {e}: {}",
                        std::any::type_name::<E>(),
                        m["v"]
                    ),
                },
                _ => crate::warn!(cat: "ui", "[island] {}", m["why"].as_str().unwrap_or("failed")),
            }
        }
    });
    Shown { key, props, task }
}

/// The page's islands: `mods` by key; by box id, `live` handles and `out`,
/// the channel back to the box's view.
const REG: &str = "const I = window.__aioIslands || (window.__aioIslands = { mods: {}, live: {}, out: {} });";

/// A script's error as text.
const WHY: &str = "(e && e.stack || e)";

/// Mount box `id` with `key`'s `mount` (loading it from `src` first when
/// given), then say so, or which half failed. Its events go out on this
/// script's channel, which stays open.
fn mount_js(key: &str, src: Option<&str>, id: &str, props: &Value) -> String {
    let (key, id) = (json!(key), json!(id));
    let load = match src {
        Some(m) => format!(
            "if (!(K in I.mods)) {{ try {{ const f = {m}; if (typeof f !== 'function') throw new Error('its script \
             returned no mount(el, props, emit) function'); I.mods[K] = f; }} catch (e) {{ return \
             aio.send({{ t: 'error', why: 'the script failed to load: ' + {WHY} }}); }} }}"
        ),
        None => String::new(),
    };
    format!(
        "{REG} const K = {key}, ID = {id};{load}
         if (!(K in I.mods)) return aio.send({{ t: 'error', why: 'its script failed to load before' }});
         const el = document.querySelector('[data-aio-island=\"' + ID + '\"]');
         if (!el || I.live[ID]) return;
         I.out[ID] = (m) => aio.send(m);
         const go = (p) => {{ const h = I.mods[K](el, p, (v) => aio.send({{ t: 'event', v }}));
           return Object.assign(typeof h === 'function' ? {{ unmount: h }} : h || {{}}, {{ go }}); }};
         try {{ I.live[ID] = go({props}); }}
         catch (e) {{ return aio.send({{ t: 'error', why: 'its script loaded but mount() threw: ' + {WHY} }}); }}
         aio.send({{ t: 'mounted' }});
         await new Promise(() => {{}});"
    )
}

/// Mount box `id` in a sandboxed iframe of its own (`crate::ISLAND_SANDBOX`
/// inside), handing it `src` (the island's `mount` expression) and `props`
/// once it loaded; only that frame's messages are heard, and passed on as
/// a page island's are.
fn sandbox_js(src: &str, id: &str, props: &Value) -> String {
    let (id, doc, src) = (json!(id), json!(sandbox_doc()), json!(src));
    format!(
        "{REG} const ID = {id};
         const el = document.querySelector('[data-aio-island=\"' + ID + '\"]');
         if (!el || I.live[ID]) return;
         I.out[ID] = (m) => aio.send(m);
         const f = document.createElement('iframe');
         f.setAttribute('sandbox', 'allow-scripts');
         f.className = 'aio-island-frame';
         f.style.cssText = 'border:0;width:100%;height:100%;display:block';
         f.srcdoc = {doc};
         const to = (m) => f.contentWindow && f.contentWindow.postMessage(m, '*');
         const hear = (e) => {{
           if (e.source !== f.contentWindow || !e.data) return;
           const m = e.data;
           if (m.t === 'event') aio.send({{ t: 'event', v: m.v }});
           else if (m.t === 'mounted') aio.send({{ t: 'mounted' }});
           else aio.send({{ t: 'error', why: String(m.why).slice(0, 4000) }});
         }};
         window.addEventListener('message', hear);
         f.addEventListener('load', () => to({{ t: 'mount', src: {src}, props: {props} }}), {{ once: true }});
         I.live[ID] = {{ update: (p) => to({{ t: 'update', props: p }}),
           unmount: () => {{ to({{ t: 'unmount' }}); window.removeEventListener('message', hear); f.remove(); }} }};
         el.appendChild(f);
         await new Promise(() => {{}});"
    )
}

/// The `srcdoc` of a sandboxed island's iframe.
fn sandbox_doc() -> String {
    format!(
        "<!doctype html><meta charset=\"utf-8\"><style>html,body{{margin:0;height:100%}}</style><script>{}</script>",
        crate::ISLAND_SANDBOX
    )
}

/// Box `id`'s new props, to its `update`; with none, it is mounted again
/// with them (aio actions: the cleanup, then the action anew).
fn update_js(id: &str, props: &Value) -> String {
    format!(
        "{REG} const ID = {}, h = I.live[ID]; if (h) try {{ if (h.update) h.update({props}); \
         else {{ if (h.unmount) h.unmount(); I.live[ID] = h.go({props}); }} }} \
         catch (e) {{ I.out[ID]({{ t: 'error', why: 'update() threw: ' + {WHY} }}); }}",
        json!(id)
    )
}

/// Box `id` gone: its `unmount`. Its view is gone too: a throw is the page's.
fn unmount_js(id: &str) -> String {
    format!(
        "{REG} const ID = {}, h = I.live[ID]; delete I.live[ID]; delete I.out[ID]; \
         if (h && h.unmount) h.unmount();",
        json!(id)
    )
}

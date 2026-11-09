//! A served page's first paint (aio's `renderToString` with `collectHead`):
//! the app's view rendered on the server for one viewer, at the address
//! asked, with its cells as that viewer's socket would show them. The page
//! shows it before its script runs; the web build then hydrates it in
//! place (`air::ssr`), its copies starting from the state the paint used
//! (the page's `aio::sync::Remote::open` takes it from the page).
use crate::http::{Paint, Painted, Viewer};
use serde_json::json;
use std::sync::{Arc, LazyLock, Mutex};

/// The `<script>` id the page's copies start from.
pub const SEED_ID: &str = "aio-seed";

/// A cell's copy for one viewer: the cell and its state's seq it was made
/// from, the copy (each a `Handle<T>`) and its seed (JSON, safe in a script).
type Copy = (Arc<dyn std::any::Any + Send + Sync>, u64, Arc<dyn std::any::Any + Send + Sync>, Arc<str>);

/// The last copy of each cell made for each viewer (cell, user, role): a
/// page asked again while the state is the same takes it again, with no
/// projection and no store made.
static COPIES: LazyLock<Mutex<std::collections::HashMap<Viewed, Copy>>> = LazyLock::new(Default::default);

/// A cell (its id) as one viewer (user, role) sees it.
type Viewed = (&'static str, String, Option<String>);

/// How many copies [`COPIES`] keeps at most (then it starts again).
const COPIES_MAX: usize = 256;

/// The cells of a first paint as its viewer sees them (`#[server]` fields
/// out, `visible` applied, by its role): the copy each view reads, and what
/// the page's copy starts from. A viewer who may not link the cells (no
/// key, no session: a public sign-in page) gets none: its views show empty
/// cells.
pub struct Seed {
    tab: super::Tab,
    seer: Option<crate::Caller>,
    cells: Vec<(&'static str, Arc<str>)>,
}

impl Seed {
    /// For `viewer`: who its socket links as (a key holder: `peer`).
    pub fn new(viewer: &Viewer) -> Self {
        let by = crate::By::User(viewer.user.clone().unwrap_or_else(|| "peer".into()));
        let seer = viewer.linked.then(|| crate::Caller { by, role: viewer.role.clone() });
        Self { tab: super::Tab::new(viewer.user.clone()), seer, cells: Vec::new() }
    }

    /// With cell `h`, as the viewer sees it now.
    pub fn cell<T: crate::Cell>(mut self, h: &crate::Handle<T>) -> Self {
        // A cell not `shared` never has its state on the network.
        let Some(seer) = self.seer.as_ref().filter(|_| T::SHARED) else { return self };
        let crate::By::User(user) = &seer.by else { return self };
        let key = (T::ID, user.clone(), seer.role.clone());
        let (seq, st) = h.snapshot();
        let lock = || COPIES.lock().unwrap_or_else(|e| e.into_inner());
        let kept = lock().get(&key).filter(|c| c.1 == seq).cloned();
        let of = |a: Arc<dyn std::any::Any + Send + Sync>| a.downcast::<crate::Handle<T>>().ok();
        let kept = kept.and_then(|(from, _, c, json)| Some((of(from)?, of(c)?, json)));
        let kept = kept.filter(|(from, ..)| from.same(h)).map(|(_, c, json)| (c, json));
        // A copy a view called a method on is not the state any more.
        let (copy, json) = match kept.filter(|(c, _)| c.snapshot().0 == 0) {
            Some(k) => k,
            None => {
                let shown = crate::shown(&*st, seer);
                let Ok(t) = crate::copy_of::<T>(shown.clone()) else { return self };
                let copy = Arc::new(crate::Handle::new(crate::Store::with(t)));
                // Inert data: `<` written as an escape, so no text in it ends the script.
                let json: Arc<str> =
                    json!({ "seq": seq, "state": shown, "user": user }).to_string().replace('<', "\\u003c").into();
                let mut all = lock();
                if all.len() >= COPIES_MAX {
                    all.clear();
                }
                all.insert(key, (Arc::new(h.clone()), seq, copy.clone(), json.clone()));
                (copy, json)
            }
        };
        self.cells.push((T::ID, json));
        self.tab = self.tab.with(crate::Source::Local((*copy).clone()));
        self
    }
}

/// `app` painted for `viewer` at the address it asked for (the `router!`
/// starts there): its view's markup, the head its views ask for (title,
/// tags) and the seed of its cells, a JSON script. A view that failed with
/// no `ErrorBoundary` above it is the error (the page is then served
/// unpainted).
pub fn paint(app: fn() -> super::View, viewer: &Viewer, seed: Seed) -> Result<Paint, String> {
    let (app, heads, cells) = prepared(app, viewer, seed);
    let (body, m) = air::ssr::render(app, || heads.borrow().merged())?;
    Ok(Paint { title: m.title.clone(), head: crate::head::html(&m) + &cells, body })
}

/// [`paint`] streamed (`Http::painted`): the page to `send` at once, each
/// `Suspense` waiting for a [`seeded_resource`](super::seeded_resource)
/// showing its fallback, then each one's children as they load, with the
/// values they loaded, until none waits or `until`.
pub(crate) fn paint_streaming(
    app: fn() -> super::View,
    viewer: &Viewer,
    seed: Seed,
    until: std::time::Instant,
    send: &mut dyn FnMut(Painted),
) -> Result<(), String> {
    let (app, heads, cells) = prepared(app, viewer, seed);
    let (body, m, mut s) = air::ssr::stream(app, || heads.borrow().merged())?;
    // The values loaded already: the client's first frame starts from them.
    let loaded = s.ready();
    let more = s.waiting();
    send(Painted::Page(Paint { title: m.title.clone(), head: crate::head::html(&m) + &cells + &loaded, body }, more));
    if more {
        s.finish(until, |part| send(Painted::More(part)));
    }
    Ok(())
}

/// `app` with the viewer's tab, address and head given it, the head it
/// asks for, and the cells' seed (a JSON script).
fn prepared(
    app: fn() -> super::View,
    viewer: &Viewer,
    seed: Seed,
) -> (impl Fn() -> super::View + 'static, crate::head::Shared, String) {
    let heads = crate::head::Shared::default();
    let (tab, at, h) = (seed.tab, viewer.path.clone(), heads.clone());
    let app = move || {
        air::provide_root(tab.clone());
        air::provide_root(crate::route::StartAt(at.clone()));
        air::provide_root(h.clone());
        app()
    };
    let cells: Vec<String> = seed.cells.iter().map(|(id, json)| format!("{}:{json}", json!(id))).collect();
    let script = format!("<script type=\"application/json\" id=\"{SEED_ID}\">{{{}}}</script>", cells.join(","));
    (app, heads, script)
}

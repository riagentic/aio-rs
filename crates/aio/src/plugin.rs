//! Plugins (aio's `definePlugin`): a crate hands an app a working piece of it,
//! its cells (with their schedules), routes, server functions and hooks, as
//! one value the app adds in one line instead of edits spread through its
//! code. A plugin is a crate whose function returns a [`Plugin`], linked at
//! build time by Cargo: no loader, no registry, nothing found at run time.
//!
//! ```ignore
//! // the `audit` crate
//! pub fn plugin(log: Handle<AuditLog>) -> Plugin {
//!     let csv = log.clone();
//!     Plugin::new("audit")
//!         .cell(log, &[])
//!         .route(Route::get("/audit.csv", move |_| {
//!             let csv = csv.clone();
//!             async move { Reply::text(csv.state().csv()) }
//!         }))
//!         .on_effect(|cell, key| eprintln!("audit: {cell} {key}"))
//! }
//! // the app
//! let (cells, http) = aio::plugin::compose(Cells::default().with(todo, &[]), http, vec![audit::plugin(log)])?;
//! let cells = cells.start()?;
//! ```
//!
//! The rules, aio's:
//!
//! 1. A plugin does only what the app could write itself: the same
//!    `Handle`, `Route`, `ServerFn` and hooks, no private API.
//! 2. The app wins: its routes match before any plugin's (a plugin's answers
//!    only a path none of the app's takes), and its server function of a
//!    name is the one served.
//! 3. A collision is an error at boot naming both sides: two plugins on one
//!    route (`/u/:id` and `/u/:name` are one; `GET` and `POST` are two), one
//!    cell or one server function; a cell both the app and a plugin hold; a
//!    plugin listed twice.
//! 4. Hooks compose: every plugin's `on_effect` and `on_link` hears, in
//!    plugin order, beside the app's; each is guarded, so one that panics
//!    stops none of the others.
//!
//! aio's `setup()` is the plugin's own function: it runs once, when the app
//! calls it, and one that can't set itself up returns an `Err` for the app's
//! `?`, so it never half-exists.
use crate::cells::Cells;
use crate::{Cell, Handle};

/// What a plugin adds to an app, under its name ([`compose`]).
pub struct Plugin {
    name: &'static str,
    cells: Cells,
    on_start: Vec<crate::cells::Hook>,
    #[cfg(not(target_arch = "wasm32"))]
    stops: Vec<(bool, Box<dyn FnOnce() + Send>)>,
    #[cfg(feature = "server")]
    http: crate::http::Http,
    on_effect: Vec<crate::Started>,
    on_action: Vec<crate::Acted>,
}

impl Plugin {
    /// `name` says which plugin in every collision.
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            cells: Cells::default(),
            on_start: vec![],
            #[cfg(not(target_arch = "wasm32"))]
            stops: vec![],
            #[cfg(feature = "server")]
            http: Default::default(),
            on_effect: vec![],
            on_action: vec![],
        }
    }

    /// A cell it owns, started after the cells `after` names.
    pub fn cell<T: Cell>(mut self, h: Handle<T>, after: &[&'static str]) -> Self {
        self.cells = self.cells.with(h, after);
        self
    }

    /// A route it serves (under the app's: rule 2).
    #[cfg(feature = "server")]
    pub fn route(mut self, r: crate::http::Route) -> Self {
        self.http = self.http.route(r);
        self
    }

    /// A server function it serves ([`crate::http::Http::server_fn`]).
    #[cfg(feature = "server")]
    pub fn server_fn<A, R, F, Fut>(mut self, f: crate::fns::ServerFn<A, R>, access: crate::fns::Access, run: F) -> Self
    where
        A: serde::de::DeserializeOwned + Send + 'static,
        R: serde::Serialize + 'static,
        F: Fn(crate::fns::Req, A) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = crate::Result<R>> + Send + 'static,
    {
        self.http = self.http.server_fn(f, access, run);
        self
    }

    /// Hear each client link open and close ([`crate::http::Http::on_link`]).
    #[cfg(feature = "server")]
    pub fn on_link(mut self, f: impl Fn(&crate::http::Presence) + Send + Sync + 'static) -> Self {
        self.http = self.http.on_link(f);
        self
    }

    /// Hear every effect as it starts ([`crate::on_effect`]).
    pub fn on_effect(mut self, f: impl Fn(&str, &str) + Send + Sync + 'static) -> Self {
        self.on_effect.push(std::sync::Arc::new(f));
        self
    }

    /// Hear every committed call ([`crate::on_action`], aio's plugin `onAction`).
    pub fn on_action(mut self, f: impl Fn(&str, &crate::Event, &serde_json::Value) + Send + Sync + 'static) -> Self {
        self.on_action.push(std::sync::Arc::new(f));
        self
    }

    /// Hosts the app may also be reached as (aio's plugin `allowedOrigins`):
    /// names (`app.example.com`) or origins (`https://app.example.com`),
    /// added to `[server] hosts`, never replacing them, for the Host gate,
    /// the socket's Origin check and who may frame the page.
    #[cfg(feature = "server")]
    pub fn allowed_origins(mut self, hosts: &[&str]) -> Self {
        let name = |h: &str| {
            let h = h.split_once("://").map_or(h, |(_, rest)| rest);
            crate::gate::name_of(h.split('/').next().unwrap_or(h)).to_ascii_lowercase()
        };
        self.http.hosts.extend(hosts.iter().map(|h| name(h)).filter(|h| !h.is_empty()));
        self
    }

    /// Run `f` once the app's cells are live (aio's plugin `onStart`), after
    /// the app's own [`Cells::on_start`].
    pub fn on_start(mut self, f: impl FnOnce(&Cells) -> crate::Result + Send + 'static) -> Self {
        self.on_start.push(Box::new(f));
        self
    }

    /// [`crate::on_stopping`] for this plugin: stop its own producers first.
    pub fn on_stopping(mut self, f: impl FnOnce() + Send + 'static) -> Self {
        self.stops.push((true, Box::new(f)));
        self
    }

    /// [`crate::on_stop`] for this plugin (aio's plugin `onStop`): let go of what it holds.
    pub fn on_stop(mut self, f: impl FnOnce() + Send + 'static) -> Self {
        self.stops.push((false, Box::new(f)));
        self
    }

    /// Its hooks into the app: effects heard, stops queued, its cells with its starts.
    fn join(self, cells: Cells) -> Cells {
        self.on_effect.into_iter().for_each(|f| crate::on_effect(move |c, k| f(c, k)));
        self.on_action.into_iter().for_each(|f| crate::on_action(move |c, e, s| f(c, e, s)));
        for (stopping, f) in self.stops {
            match stopping {
                true => crate::on_stopping(f),
                false => crate::on_stop(f),
            }
        }
        cells.and(self.cells).hooks(self.name, self.on_start)
    }
}

/// A thing two plugins claim, naming both.
fn collision(kind: &str, what: &str, first: &str, second: &str) -> crate::Error {
    crate::Error(format!(
        "plugin collision: {kind} {what} is claimed by both \"{first}\" and \"{second}\": whichever came second \
         would shadow the other. Drop one, or change the one you control"
    ))
}

/// Check `plugins` against each other and the app (rule 3), before any of
/// them is added. Pure.
fn check(app: &[&'static str], plugins: &[Plugin]) -> crate::Result<(), crate::Error> {
    let mut cells: Vec<(&str, &str)> = vec![];
    for (i, p) in plugins.iter().enumerate() {
        if plugins[..i].iter().any(|q| q.name == p.name) {
            return Err(crate::Error(format!(
                "plugins: \"{}\" is listed twice: a plugin is added once (twice would run each hook twice)",
                p.name
            )));
        }
        for id in p.cells.ids() {
            if app.contains(&id) {
                return Err(crate::Error(format!(
                    "plugin collision: cell `{id}` is both the app's and \"{}\"'s: one of them must go",
                    p.name
                )));
            }
            match cells.iter().find(|(c, _)| *c == id) {
                Some((_, by)) if *by != p.name => return Err(collision("cell", &format!("`{id}`"), by, p.name)),
                _ => cells.push((id, p.name)),
            }
        }
    }
    #[cfg(feature = "server")]
    {
        let mut routes: Vec<(&crate::http::Route, &str)> = vec![];
        let mut fns: Vec<(&str, &str)> = vec![];
        for p in plugins {
            for r in p.http.routes() {
                if let Some((first, by)) = routes.iter().find(|(o, by)| *by != p.name && o.overlaps(r)) {
                    let also = match first.pattern() == r.pattern() {
                        true => String::new(),
                        false => format!(" (the same route as {:?})", first.pattern()),
                    };
                    return Err(collision("route", &format!("{:?}{also}", r.pattern()), by, p.name));
                }
                routes.push((r, p.name));
            }
            for name in p.http.fns.names() {
                if let Some((_, by)) = fns.iter().find(|(n, _)| *n == name) {
                    return Err(collision("server function", &format!("`{name}`"), by, p.name));
                }
                fns.push((name, p.name));
            }
        }
    }
    Ok(())
}

/// The app's cells and HTTP with `plugins` added under them (the rules
/// above): their cells beside the app's, their routes and server functions
/// under the app's, their hooks beside it. A collision refuses the boot
/// before anything is added.
#[cfg(feature = "server")]
pub fn compose(
    cells: Cells,
    http: crate::http::Http,
    plugins: Vec<Plugin>,
) -> crate::Result<(Cells, crate::http::Http), crate::Error> {
    check(&cells.ids(), &plugins)?;
    let (mut cells, mut http) = (cells, http);
    for mut p in plugins {
        eprintln!("[aio] plugin {}: cells {}", p.name, p.cells.ids().join(", "));
        http = http.under(std::mem::take(&mut p.http));
        cells = p.join(cells);
    }
    Ok((cells, http))
}

/// [`compose`] for an app without a server: cells and hooks.
#[cfg(not(feature = "server"))]
pub fn compose(cells: Cells, plugins: Vec<Plugin>) -> crate::Result<Cells, crate::Error> {
    check(&cells.ids(), &plugins)?;
    let mut cells = cells;
    for p in plugins {
        cells = p.join(cells);
    }
    Ok(cells)
}

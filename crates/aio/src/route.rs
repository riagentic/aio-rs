//! In-app navigation (aio's `air/router-core.ts`, `air/router.ts`,
//! `air/back-button.ts`): routes, params, a history the app owns, links and
//! Back. Plain functions; the view glue is a macro over the app's own
//! views ([`router!`](crate::router)), like `client!`.
//!
//! The history lives in Rust, not in the page's `location`: the desktop page
//! is `aio://app/index.html` and Android's an asset, so aio's route base (the
//! app's `/` under `/assets`) has nothing to undo here. One spelling on
//! every target: a route is the path the app wrote.
use std::collections::BTreeMap;

/// A route's params by name: `:id` as `id`, a bare `*` as `*`, `*rest` as
/// `rest`. Values are decoded.
pub type Params = BTreeMap<String, String>;

/// Param `name` parsed (`param::<u32>(&p, "id")`); `None` if absent or bad.
pub fn param<T: std::str::FromStr>(p: &Params, name: &str) -> Option<T> {
    p.get(name)?.parse().ok()
}

/// A place in the app: path, query and fragment, percent-encoded as a URL
/// spells them (`/café` is `/caf%C3%A9`), so two spellings of one place are
/// equal.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Loc {
    pub path: String,
    /// Without the `?`.
    pub query: String,
    /// Without the `#`.
    pub hash: String,
}

impl Loc {
    /// `s` as a place in the app (resolved from `/`); `None` if it leaves the app.
    pub fn parse(s: &str) -> Option<Loc> {
        resolve(s, &Loc { path: "/".into(), ..Default::default() })
    }

    /// The first value of query key `key`, decoded (`+` is a space, as a form sends it).
    pub fn query(&self, key: &str) -> Option<String> {
        let dec = |s: &str| decode(&s.replace('+', " "));
        self.query
            .split('&')
            .map(|kv| kv.split_once('=').unwrap_or((kv, "")))
            .find(|(k, _)| dec(k) == key)
            .map(|(_, v)| dec(v))
    }
}

impl std::fmt::Display for Loc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.path)?;
        if !self.query.is_empty() {
            write!(f, "?{}", self.query)?;
        }
        if !self.hash.is_empty() {
            write!(f, "#{}", self.hash)?;
        }
        Ok(())
    }
}

/// Where `to` leads from `from`, by URL rules: `/x` from the app's root,
/// `api` from `/docs/x` is `/docs/api`, `?q=1` keeps the path, `#top` the
/// query too, `..` goes up. `None` when `to` leaves the app: a scheme
/// (`https:`, `mailto:`) or another host (`//host`, `/\host`).
pub fn resolve(to: &str, from: &Loc) -> Option<Loc> {
    let to = to.trim().replace('\\', "/");
    let scheme = to.split_once(':').is_some_and(|(s, _)| {
        s.starts_with(|c: char| c.is_ascii_alphabetic())
            && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
    });
    if scheme || to.starts_with("//") {
        return None;
    }
    let (rest, hash) = to.split_once('#').unwrap_or((&to, ""));
    let (path, query) = rest.split_once('?').map_or((rest, None), |(p, q)| (p, Some(q)));
    let (path, query) = match path {
        "" => (from.path.clone(), query.map_or(from.query.clone(), String::from)),
        p if p.starts_with('/') => (p.to_string(), query.unwrap_or("").to_string()),
        p => (format!("{}{p}", &from.path[..from.path.rfind('/').map_or(0, |i| i + 1)]), query.unwrap_or("").into()),
    };
    Some(Loc { path: encode(&dots(&path), ""), query: encode(&query, ""), hash: encode(hash, "") })
}

/// `path` without `.` and `..` segments (RFC 3986 5.2.4); always absolute.
fn dots(path: &str) -> String {
    let segs: Vec<&str> = path.split('/').skip(1).collect();
    let mut out: Vec<&str> = vec![];
    for (i, s) in segs.iter().enumerate() {
        let last = i + 1 == segs.len();
        match *s {
            "." if last => out.push(""),
            "." => {}
            ".." => {
                out.pop();
                if last {
                    out.push("");
                }
            }
            s => out.push(s),
        }
    }
    format!("/{}", out.join("/"))
}

/// `s` percent-encoded where a URL must be (controls, space, `"<>` and the
/// like, non-ASCII) and in `also`; existing escapes are kept.
fn encode(s: &str, also: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        let bad = b <= b' ' || b >= 0x7F || b"\"<>`{}".contains(&b) || also.as_bytes().contains(&b);
        if bad { out.push_str(&format!("%{b:02X}")) } else { out.push(b as char) }
    }
    out
}

/// The escaped octets of a `%XX` at `s[i..]`.
fn escape_at(s: &[u8], i: usize) -> Option<u8> {
    let h = s.get(i + 1..i + 3).filter(|h| s[i] == b'%' && h.iter().all(u8::is_ascii_hexdigit))?;
    u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok()
}

/// `s` fully decoded; as written when it holds a malformed escape or isn't
/// UTF-8 once decoded (a param is then its raw text, as in aio).
pub fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let (o, n) = match (b[i], escape_at(b, i)) {
            (b'%', None) => return s.into(),
            (_, Some(o)) => (o, 3),
            (c, None) => (c, 1),
        };
        out.push(o);
        i += n;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.into())
}

/// A path decoded for comparison, except `%2F` (a slash inside a segment)
/// and `%25` (a literal percent), kept escaped in one spelling: so
/// `/caf%C3%A9` is `/café`, and `/a%2Fb` stays one segment. A run of
/// escapes that isn't UTF-8 is left as written.
fn for_match(s: &str) -> String {
    let b = s.as_bytes();
    let (mut out, mut run, mut i) = (String::with_capacity(b.len()), Vec::new(), 0);
    let mut raw = String::new();
    let flush = |out: &mut String, run: &mut Vec<u8>, raw: &mut String| {
        match String::from_utf8(std::mem::take(run)) {
            Ok(t) => out.push_str(&t),
            Err(_) => out.push_str(raw),
        }
        raw.clear();
    };
    while i < b.len() {
        match escape_at(b, i) {
            Some(o @ (0x2F | 0x25)) => {
                flush(&mut out, &mut run, &mut raw);
                out.push_str(&format!("%{o:02X}"));
                i += 3;
            }
            Some(o) => {
                run.push(o);
                raw.push_str(&s[i..i + 3]);
                i += 3;
            }
            None => {
                flush(&mut out, &mut run, &mut raw);
                let n = s[i..].chars().next().map_or(1, char::len_utf8);
                out.push_str(&s[i..i + n]);
                i += n;
            }
        }
    }
    flush(&mut out, &mut run, &mut raw);
    out
}

/// A path as matching compares it: no query or fragment, decoded (but `%2F`
/// and `%25`), no trailing slash except the root's.
pub fn normalize(path: &str) -> String {
    let bare = path.split(['?', '#']).next().unwrap_or("");
    let p = for_match(bare);
    match p.trim_end_matches('/') {
        "" if !p.is_empty() => "/".into(),
        t => t.into(),
    }
}

/// `path`'s params if it matches `pattern`, else `None`. Segments are
/// literal, `:name` (one non-empty segment) or a whole `*` / `*name` (the
/// rest, any number of segments; last, it may be empty: `/files/*` matches
/// `/files`). `exact`: the whole path, one trailing slash allowed; else a
/// prefix ending at a segment (a layout's match). `/a*b` is a literal.
pub fn match_path(pattern: &str, path: &str, exact: bool) -> Option<Params> {
    let pattern = pattern.trim_end_matches('/');
    let path = for_match(path.split(['?', '#']).next().unwrap_or(""));
    // One trailing slash is allowed (`/` itself is then the empty path).
    let path = if exact { path.strip_suffix('/').unwrap_or(&path) } else { &path };
    let pat: Vec<&str> = pattern.split('/').collect();
    let segs: Vec<&str> = path.split('/').collect();
    let mut out = Params::new();
    segments(&pat, &segs, exact, &mut out).then_some(out)
}

/// The name a `*` segment captures under, if `seg` is one.
fn star(seg: &str) -> Option<&str> {
    let name = seg.strip_prefix('*')?;
    match name {
        "" => Some("*"),
        n if n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') => Some(n),
        _ => None,
    }
}

fn segments(pat: &[&str], path: &[&str], exact: bool, out: &mut Params) -> bool {
    let Some((&p, pat_rest)) = pat.split_first() else { return !exact || path.is_empty() };
    if let Some(name) = star(p) {
        // Last: everything left (maybe nothing); else at least one segment, greedy.
        let least = if pat_rest.is_empty() { path.len() } else { 1 };
        return (least..=path.len()).rev().any(|k| {
            let ok = segments(pat_rest, &path[k..], exact, out);
            if ok {
                let v = decode(&path[..k].join("/"));
                out.insert(name.into(), v.strip_suffix('/').unwrap_or(&v).into());
            }
            ok
        });
    }
    let Some((&s, path_rest)) = path.split_first() else { return false };
    let hit = match p.strip_prefix(':') {
        Some(name) if !s.is_empty() => {
            out.insert(name.into(), decode(s));
            true
        }
        Some(_) => false,
        None => s == for_match(p),
    };
    hit && segments(pat_rest, path_rest, exact, out)
}

/// `pattern` with its params filled in, for a link to it: `:id` takes the
/// value encoded as one segment; a `*` takes it segment by segment.
pub fn fill(pattern: &str, params: &[(&str, String)]) -> String {
    let get = |n: &str| params.iter().find(|(k, _)| *k == n).map(|(_, v)| v.as_str());
    let segs = pattern.split('/').map(|seg| match (seg.strip_prefix(':'), star(seg)) {
        (Some(n), _) => get(n).map_or(seg.into(), |v| encode(v, "/?#%")),
        (_, Some(n)) => {
            get(n).map_or(seg.into(), |v| v.split('/').map(|p| encode(p, "?#%")).collect::<Vec<_>>().join("/"))
        }
        _ => seg.into(),
    });
    let out = segs.collect::<Vec<String>>().join("/");
    if out.len() > 1 { out.trim_end_matches('/').into() } else { out }
}

/// Is a link to `to` the current place? Compared as paths, `to` resolved
/// from `here` (so a relative link is active on the page it leads to).
/// `exact` (and a link to `/`): only that path; else also any path under it.
/// A link that leaves the app never is.
pub fn is_active(to: &str, here: &Loc, exact: bool) -> bool {
    let Some(t) = resolve(to, here) else { return false };
    let (t, p) = (normalize(&t.path), normalize(&here.path));
    p == t || (!exact && t != "/" && p.starts_with(&format!("{t}/")))
}

/// `path` under `base`, as a nested route's pattern: `settings` (or
/// `/settings`) under `/dash` is `/dash/settings`; no doubled or trailing
/// slash, `/` at the root.
pub fn join(base: &str, path: &str) -> String {
    let segs: Vec<&str> = base.split('/').chain(path.split('/')).filter(|s| !s.is_empty()).collect();
    format!("/{}", segs.join("/"))
}

/// Where a route sits in a tree of them (`router!`'s `Route`): the pattern
/// so far and every enclosing route's params. The root is empty.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Nest {
    pub pattern: String,
    pub params: Params,
}

impl Nest {
    /// The child route `path` here, if it matches `at`: `layout` (it has
    /// routes inside) matches a prefix, a page the whole path; `index` is
    /// this route's own place (`/dash` and `/dash/`, never `/dash/x`). Its
    /// params join the enclosing ones.
    pub fn child(&self, path: &str, index: bool, layout: bool, at: &str) -> Option<Nest> {
        if !index && path.is_empty() {
            return None;
        }
        let pattern = join(&self.pattern, if index { "" } else { path });
        let mut params = self.params.clone();
        params.extend(match_path(&pattern, at, index || !layout)?);
        Some(Nest { pattern, params })
    }
}

/// Where the app starts: `--route <place>` (or `--route=<place>`) among the
/// launch's `args` (a deep link: `myapp --route /users/7`), else `/`.
pub fn start(args: impl IntoIterator<Item = String>) -> String {
    let mut args = args.into_iter();
    while let Some(a) = args.next() {
        match a.strip_prefix("--route") {
            Some("") => return args.next().unwrap_or_else(|| "/".into()),
            Some(v) if v.starts_with('=') => return v[1..].into(),
            _ => {}
        }
    }
    "/".into()
}

/// The place a link the app is opened with leads to: what follows its
/// scheme (`myapp://users/7?tab=a` → `/users/7?tab=a`, `myapp:/a` → `/a`).
pub fn of_link(url: &str) -> String {
    let rest = url.split_once(':').map_or(url, |(_, r)| r);
    let rest = rest.strip_prefix("//").unwrap_or(rest);
    if rest.starts_with('/') { rest.to_string() } else { format!("/{rest}") }
}

/// Where the app starts: in a browser the page's address (a reload or a
/// shared link lands where it was; the server answers every route with the
/// app), elsewhere the launch's `--route` ([`start`]).
pub fn start_place() -> String {
    #[cfg(target_arch = "wasm32")]
    if let Some(l) = web_sys::window().map(|w| w.location()) {
        return [l.pathname(), l.search(), l.hash()].map(Result::unwrap_or_default).concat();
    }
    start(std::env::args())
}

/// In a browser, the address bar shows `here` (a new entry, or `replace`
/// the current one; none when it already shows it), so the browser's Back
/// and a reload follow the app. Nothing elsewhere: the desktop's and
/// Android's pages have no address bar.
pub fn mirror(here: &Loc, replace: bool) {
    #[cfg(target_arch = "wasm32")]
    if let Some(w) = web_sys::window() {
        let (url, h) = (here.to_string(), w.history());
        let shown = w.location().href().ok().and_then(|u| Some(u.split_once("://")?.1.split_once('/')?.1.to_string()));
        let Ok(h) = h else { return };
        let _ = if replace || shown.is_some_and(|s| format!("/{s}") == url) {
            h.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&url))
        } else {
            h.push_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&url))
        };
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (here, replace);
}

/// History one step back, if it can: in a browser through the page's own
/// history (its `popstate` moves `h`, so the browser's Forward still
/// leads where the app was), elsewhere `h.go(-1)`.
pub fn step_back(h: &mut History) -> bool {
    #[cfg(target_arch = "wasm32")]
    if let Some(Ok(page)) = web_sys::window().map(|w| w.history()) {
        return h.can_go(-1) && page.back().is_ok();
    }
    h.go(-1)
}

/// The places the browser's own Back and Forward lead to (`popstate`):
/// none but in a browser.
pub fn pops() -> tokio::sync::mpsc::UnboundedReceiver<String> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    #[cfg(target_arch = "wasm32")]
    if let Some(w) = web_sys::window() {
        use wasm_bindgen::JsCast;
        let on = wasm_bindgen::closure::Closure::<dyn Fn()>::new(move || _ = tx.send(start_place()));
        _ = w.add_event_listener_with_callback("popstate", on.as_ref().unchecked_ref());
        // The page's for its life (one router per page).
        on.forget();
    }
    #[cfg(not(target_arch = "wasm32"))]
    drop(tx);
    rx
}

/// How many places Back can go through (Chromium keeps 50 too).
pub const HISTORY_MAX: usize = 50;

/// The app's history: where it is, where Back and Forward lead. One per
/// window, owned by the app (the page's `location` is never a route).
#[derive(Clone, Debug, PartialEq)]
pub struct History {
    entries: Vec<Loc>,
    at: usize,
}

impl History {
    /// Starting at `start` (a deep link, the last place); the root if it
    /// leaves the app.
    pub fn new(start: &str) -> Self {
        Self { entries: vec![Loc::parse(start).unwrap_or_else(|| Loc::parse("/").expect("root"))], at: 0 }
    }

    pub fn here(&self) -> &Loc {
        &self.entries[self.at]
    }

    /// Go to `to` (resolved from here), dropping what Forward led to. Going
    /// where the app already is replaces that entry, as a browser does (a
    /// nav bar's own item clicked twice adds no Back step). False if `to`
    /// leaves the app: nothing changes.
    pub fn push(&mut self, to: &str) -> bool {
        let Some(loc) = resolve(to, self.here()) else { return false };
        if loc != *self.here() {
            self.entries.truncate(self.at + 1);
            self.entries.push(loc);
            let over = self.entries.len().saturating_sub(HISTORY_MAX);
            self.entries.drain(..over);
            self.at = self.entries.len() - 1;
        }
        true
    }

    /// Go to `to` in place of here (a redirect: no Back step).
    pub fn replace(&mut self, to: &str) -> bool {
        let Some(loc) = resolve(to, self.here()) else { return false };
        self.entries[self.at] = loc;
        true
    }

    /// `delta` steps back (negative) or forward; false, with nothing
    /// changed, past either end.
    pub fn go(&mut self, delta: isize) -> bool {
        match self.at.checked_add_signed(delta).filter(|&i| i < self.entries.len()) {
            Some(i) => {
                self.at = i;
                true
            }
            None => false,
        }
    }

    /// The page's own history moved to `place` (the browser's Back or
    /// Forward, a `popstate`): the step it took when it is the place one
    /// step back or on, else `place` in place of here. False if it leaves
    /// the app or is where the app already is: nothing changes.
    pub fn sync(&mut self, place: &str) -> bool {
        let Some(loc) = Loc::parse(place) else { return false };
        if loc == *self.here() {
            return false;
        }
        match [-1, 1]
            .into_iter()
            .find(|&d| self.at.checked_add_signed(d).and_then(|i| self.entries.get(i)) == Some(&loc))
        {
            Some(d) => self.go(d),
            None => {
                self.entries[self.at] = loc;
                true
            }
        }
    }

    pub fn can_go(&self, delta: isize) -> bool {
        self.at.checked_add_signed(delta).is_some_and(|i| i < self.entries.len())
    }

    /// The number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        false
    }
}

type Handler = std::rc::Rc<std::cell::RefCell<dyn FnMut() -> bool>>;

/// `router!`'s mark that the host's Back is wired (once per app instance).
#[doc(hidden)]
#[derive(Clone)]
pub struct HostBack;

/// The Back handlers of a window, newest last (aio's `onBackButton`).
#[derive(Clone, Default)]
pub struct Backs {
    next: u64,
    list: Vec<(u64, Handler)>,
}

impl Backs {
    /// Handle Back (Android's button, a mouse's back key): `f` returns true
    /// when it took it (closed a dialog, went up a screen), false to let the
    /// next one try. The newest runs first, so a dialog over a screen gets
    /// Back before the screen does. The id removes exactly this one.
    pub fn add(&mut self, f: impl FnMut() -> bool + 'static) -> u64 {
        self.next += 1;
        self.list.push((self.next, std::rc::Rc::new(std::cell::RefCell::new(f))));
        self.next
    }

    pub fn remove(&mut self, id: u64) {
        self.list.retain(|(k, _)| *k != id);
    }

    /// Back was pressed: the handlers, newest first, until one takes it.
    /// True if one did; false lets the caller do its default (history, then
    /// leave). A handler that panics is loud and counts as taken: a broken
    /// handler must not also close the app.
    pub fn press(&self) -> bool {
        self.list.iter().rev().any(|(_, f)| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f.borrow_mut()())).unwrap_or_else(|_| {
                eprintln!("[aio] a Back handler panicked; Back is taken as handled");
                true
            })
        })
    }
}

/// Typed routes: an enum whose variants are the app's pages, each with its
/// pattern; fields are the pattern's params, parsed (`FromStr`) and written
/// back (`Display`).
///
/// ```ignore
/// aio::routes! {
///     pub enum Page {
///         Home = "/",
///         User { id: u32 } = "/users/:id",
///         File { path: String } = "/files/*path",
///     }
/// }
/// assert_eq!(Page::parse("/users/7"), Some(Page::User { id: 7 }));
/// assert_eq!(Page::User { id: 7 }.href(), "/users/7");
/// ```
///
/// A page may name its title, formatted with its fields (`head()`):
/// `User { id: u32 } = "/users/:id" => "User {id}"`.
///
/// `parse` takes the first variant, in written order, whose pattern matches
/// and whose params parse (one page shows, where aio rendered every
/// matching `<Route>`).
#[macro_export]
macro_rules! routes {
    ($(#[$m:meta])* $vis:vis enum $name:ident {
        $($(#[$vm:meta])* $v:ident $({ $($f:ident : $t:ty),* $(,)? })? = $pat:literal $(=> $title:literal)?),* $(,)?
    }) => {
        $(#[$m])*
        #[derive(Clone, Debug, PartialEq)]
        $vis enum $name { $($(#[$vm])* $v $({ $($f: $t),* })?),* }

        impl $name {
            /// The page at `path` (a query or fragment is ignored), if any.
            pub fn parse(path: &str) -> Option<Self> {
                $(
                    if let Some(_p) = $crate::route::match_path($pat, path, true) {
                        #[allow(clippy::redundant_closure_call)]
                        let page = (|| Some($name::$v $({ $($f: $crate::route::param(&_p, stringify!($f))?),* })?))();
                        if page.is_some() {
                            return page;
                        }
                    }
                )*
                None
            }

            /// The path a link to this page uses.
            pub fn href(&self) -> String {
                match self {
                    $($name::$v $({ $($f),* })? => {
                        $crate::route::fill($pat, &[$($((stringify!($f), $f.to_string())),*)?])
                    })*
                }
            }

            /// What this page puts in `<head>` (for `aio_shell::page::use_head`):
            /// the title its `=> "…"` formats with its fields, if it has one.
            pub fn head(&self) -> $crate::head::Head {
                match self {
                    $(#[allow(unused_variables)] $name::$v $({ $($f),* })? => $crate::head::Head {
                        title: [$(format!($title))?].into_iter().next(),
                        ..Default::default()
                    },)*
                }
            }
        }
    };
}

/// The router for the views: `router!()` makes `NAV` (the window's
/// [`History`], from the launch's `--route` ([`start`]) or, in a browser,
/// the page's address ([`start_place`]), else `/`),
/// `here()` (the current place; reading it subscribes the view), `go(to)`,
/// `redirect(to)` (no Back step), `back()` (the [`Backs`] handlers, then
/// history), `use_back(f)` (a handler for the component's life), `Link`,
/// and for a tree of routes (aio's `<Route>`, `<Outlet>`) `Route`, `Outlet`,
/// `params()` and `Redirect`.
/// Global like `client!`'s: one per app instance (so per test).
#[macro_export]
macro_rules! router {
    () => {
        pub static NAV: $crate::ui::Global<$crate::route::History> = $crate::ui::Global::named(
            || $crate::route::History::new(&$crate::route::start_place()),
            concat!(module_path!(), "::NAV"),
        );

        /// The window's Back handlers (`use_back`).
        pub static BACKS: $crate::ui::Global<$crate::route::Backs> =
            $crate::ui::Global::named(Default::default, concat!(module_path!(), "::BACKS"));

        /// Where the app is. The first read also wires the host's Back
        /// (Android's button, a mouse's back key) to [`back`], and the links
        /// the app is opened with (`aio::ui::links`) to [`go`].
        pub fn here() -> $crate::route::Loc {
            if $crate::ui::try_context::<$crate::route::HostBack>().is_none() {
                $crate::ui::provide_root($crate::route::HostBack);
                $crate::ui::spawn_forever(async {
                    let mut asks = $crate::ui::back_presses();
                    while let Some(ask) = asks.recv().await {
                        _ = ask.send(back());
                    }
                });
                // A link the app is opened with (Android): it goes there.
                $crate::ui::spawn_forever(async {
                    let mut links = $crate::ui::links();
                    while let Some(url) = links.recv().await {
                        go(&$crate::route::of_link(&url));
                    }
                });
                // The browser's Back and Forward (a web build): history follows.
                $crate::ui::spawn_forever(async {
                    let mut pops = $crate::route::pops();
                    while let Some(place) = pops.recv().await {
                        NAV.update(|n| n.sync(&place));
                    }
                });
            }
            NAV.with(|n| n.here().clone())
        }

        /// Go to `to` (resolved from here); false if it leaves the app.
        pub fn go(to: &str) -> bool {
            NAV.update(|n| n.push(to) && { $crate::route::mirror(n.here(), false); true })
        }

        /// Go to `to` without a Back step.
        pub fn redirect(to: &str) -> bool {
            NAV.update(|n| n.replace(to) && { $crate::route::mirror(n.here(), true); true })
        }

        /// Back: a handler takes it, or history goes one step; false at the
        /// first place (the shell's default then: leave the app).
        pub fn back() -> bool {
            // A copy: a handler may add or remove handlers.
            let backs = BACKS.get_untracked();
            backs.press() || NAV.update($crate::route::step_back)
        }

        /// `f` handles Back while this component lives (see `Backs::add`).
        pub fn use_back(f: impl FnMut() -> bool + 'static) {
            let id = $crate::ui::once(move || BACKS.update(|b| b.add(f)));
            $crate::ui::on_cleanup(move || BACKS.update(|b| b.remove(id)));
        }

        pub use __router::*;

        /// The router's components: `rsx!`'s names in scope whatever the
        /// module that says `router!()` imports.
        #[doc(hidden)]
        pub mod __router {
            use super::{go, here, redirect};
            use $crate::ui::prelude::*;

            /// Where a `Route` is in the tree, and what its `Outlet` shows.
            #[doc(hidden)]
            #[derive(Clone)]
            pub struct RouteAt($crate::route::Nest, Option<View>);

            /// A route in a tree of them: `element` where the place matches
            /// `path`, joined to the enclosing routes' (`settings` inside `/dash`
            /// is `/dash/settings`). With routes inside, it is a layout: it
            /// matches a prefix, and its element shows the matching inner one at
            /// its `Outlet` (no element: they show in its place). `index`: the
            /// enclosing route's own place. Every matching route shows (as in aio);
            /// a typed `routes!` enum picks one page.
            #[component]
            pub fn Route(
                #[props(default)] path: String,
                #[props(default)] index: bool,
                element: Option<View>,
                children: View,
            ) -> View {
                here();
                // From above: this view holds its own since its first render.
                let up = $crate::ui::parent_context::<RouteAt>().map(|r| r.0).unwrap_or_default();
                let layout = !$crate::ui::is_empty(&children);
                // Its match follows the place, and its view only a new match
                // (on air its body runs again then; a query alone runs nothing).
                let nest = $crate::ui::follow(move || {
                    super::NAV.with(|n| up.child(&path, index, layout, &n.here().path))
                });
                let Some(nest) = nest else { return rsx! {} };
                $crate::ui::provide_now(RouteAt(nest, layout.then(|| children.clone())));
                match element {
                    Some(e) => e,
                    None if layout => children,
                    None => rsx! {},
                }
            }

            /// Where a layout's element shows its matching inner route (nothing
            /// outside a layout).
            #[component]
            pub fn Outlet() -> View {
                here();
                try_context::<RouteAt>().and_then(|r| r.1).unwrap_or_else(|| rsx! {})
            }

            /// The params of the enclosing routes (`:id` of `/users/:id` in its
            /// inner ones too); empty outside a `Route`.
            pub fn params() -> $crate::route::Params {
                try_context::<RouteAt>().map(|r| r.0.params).unwrap_or_default()
            }

            /// Goes to `to` once shown, in place of here (aio's default; a pushed
            /// one would send Back straight here again).
            #[component]
            pub fn Redirect(to: String) -> View {
                once(move || spawn(async move { _ = redirect(&to) }));
                rsx! {}
            }

            /// An `<a>` that goes to `to` in the app, with `active_class` (and
            /// `aria-current="page"`) on the current place's link. A link out of
            /// the app opens in the system browser. The author's `onclick` runs
            /// first; `prevent_default()` there keeps it from navigating.
            #[component]
            pub fn Link(
                to: String,
                #[props(default)] exact: bool,
                #[props(default)] replace: bool,
                #[props(default)] class: String,
                #[props(default = "active".to_string())] active_class: String,
                onclick: Option<EventHandler<MouseEvent>>,
                #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
                children: View,
            ) -> View {
                let own = onclick;
                if $crate::route::resolve(&to, &here()).is_none() {
                    return rsx! {
                        a { href: "{to}", target: "_blank", rel: "noopener noreferrer", class, ..attributes, {children} }
                    };
                }
                // Read where they are drawn: on air these holes alone follow the place.
                let on = move |to: &str| super::NAV.with(|n| $crate::route::is_active(to, n.here(), exact));
                let go_to = to.clone();
                rsx! {
                    a {
                        href: "{to}",
                        class: $crate::ui::cx(&[(&class, !class.is_empty()), (&active_class, on(&to))]),
                        "aria-current": on(&to).then_some("page"),
                        onclick: move |e: MouseEvent| {
                            if let Some(f) = &own {
                                f.call(e.clone());
                            }
                            let routed = e.default_action_enabled();
                            e.prevent_default();
                            if !routed {
                                return;
                            }
                            if replace { redirect(&go_to) } else { go(&go_to) };
                        },
                        ..attributes,
                        {children}
                    }
                }
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(kv: &[(&str, &str)]) -> Option<Params> {
        Some(kv.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect())
    }

    #[test]
    fn matching_agrees_with_aios_table() {
        // aio tests/route-matching-parity.test.ts, row by row.
        let table: &[(&str, &str, Option<Params>)] = &[
            ("/users", "/users", p(&[])),
            ("/users", "/users/", p(&[])),
            ("/users/", "/users", p(&[])),
            ("/users", "/users//", None),
            ("/users", "/user", None),
            ("/users", "/users/42", None),
            ("/users/:id", "/users/42", p(&[("id", "42")])),
            ("/users/:id", "/users/42/", p(&[("id", "42")])),
            ("/users/:id", "/users/", None),
            ("/users/:id", "/users", None),
            ("/users/:id", "/users/1/2", None),
            ("/users/:id", "/users/a%20b", p(&[("id", "a b")])),
            ("/a/:x/b", "/a/1/b/", p(&[("x", "1")])),
            ("/files/*", "/files", p(&[("*", "")])),
            ("/files/*", "/files/", p(&[("*", "")])),
            ("/files/*", "/files/a/b", p(&[("*", "a/b")])),
            ("/files/*", "/files/a/b/", p(&[("*", "a/b")])),
            ("/files/*", "/filesx", None),
            ("/files/*", "/other/a", None),
            ("/files/:dir/*", "/files/x", p(&[("dir", "x"), ("*", "")])),
            ("/files/:dir/*", "/files/x/y/z", p(&[("dir", "x"), ("*", "y/z")])),
            ("/*", "/", p(&[("*", "")])),
            ("/*", "/a/b", p(&[("*", "a/b")])),
            ("/", "/", p(&[])),
            ("/", "/x", None),
        ];
        for (pattern, path, want) in table {
            assert_eq!(&match_path(pattern, path, true), want, "{pattern} ← {path}");
        }
    }

    #[test]
    fn escapes_compare_decoded_but_a_slash_or_percent_stays_one_segment() {
        assert_eq!(match_path("/café", "/caf%C3%A9", true), p(&[]));
        assert_eq!(match_path("/caf%C3%A9", "/caf%c3%a9", true), p(&[]));
        assert_eq!(match_path("/about us", "/about%20us", true), p(&[]));
        assert_eq!(match_path("/files/a%2Fb", "/files/a%2fb", true), p(&[]));
        assert_eq!(match_path("/files/a/b", "/files/a%2Fb", true), None);
        assert_eq!(match_path("/u/:id", "/u/a%2Fb", true), p(&[("id", "a/b")]));
        assert_eq!(match_path("/u/:id/x", "/u/a%2Fb/x", true), p(&[("id", "a/b")]));
        assert_eq!(match_path("/u/:id", "/u/100%2541", true), p(&[("id", "100%41")]));
        // Malformed: the raw text, never a panic.
        assert_eq!(match_path("/u/:id", "/u/%E0%A4%A", true), p(&[("id", "%E0%A4%A")]));
        assert_eq!(match_path("/u/:id", "/u/%FF", true), p(&[("id", "%FF")]));
    }

    #[test]
    fn only_a_whole_star_segment_is_a_wildcard() {
        assert_eq!(match_path("/a*b", "/b", true), None);
        assert_eq!(match_path("/a*b", "/aaab", true), None);
        assert_eq!(match_path("/a*b", "/a*b", true), p(&[]));
        assert_eq!(match_path("/files/*", "/files/a/b", true), p(&[("*", "a/b")]));
        // aio-rs: a wildcard may be named, and may sit mid-pattern.
        assert_eq!(match_path("/f/*path", "/f/a/b", true), p(&[("path", "a/b")]));
        assert_eq!(match_path("/a/*/z", "/a/b/c/z", true), p(&[("*", "b/c")]));
        assert_eq!(match_path("/a/*/z", "/a/z", true), None);
    }

    #[test]
    fn a_layout_matches_a_prefix_at_a_segment() {
        assert_eq!(match_path("/dash", "/dash/stats", false), p(&[]));
        assert_eq!(match_path("/dash", "/dash/", false), p(&[]));
        assert_eq!(match_path("/dash", "/dashboard", false), None);
        assert_eq!(match_path("/u/:id", "/u/7/posts", false), p(&[("id", "7")]));
        assert_eq!(match_path("/dash", "/dash/stats", true), None);
        // A query or fragment never takes part.
        assert_eq!(match_path("/dash", "/dash?tab=1#x", true), p(&[]));
    }

    #[test]
    fn links_resolve_by_url_rules_and_outside_ones_leave() {
        let at = Loc::parse("/docs/x?q=1#h").unwrap();
        let r = |to| resolve(to, &at).map(|l| l.to_string());
        assert_eq!(r("api").as_deref(), Some("/docs/api"));
        assert_eq!(r("../api").as_deref(), Some("/api"));
        assert_eq!(r("./").as_deref(), Some("/docs/"));
        assert_eq!(r("/about us").as_deref(), Some("/about%20us"));
        assert_eq!(r("/café").as_deref(), Some("/caf%C3%A9"));
        assert_eq!(r("?p=2").as_deref(), Some("/docs/x?p=2"));
        assert_eq!(r("#top").as_deref(), Some("/docs/x?q=1#top"));
        assert_eq!(r("/a/../../b").as_deref(), Some("/b"));
        for out in
            ["https://example.com/x", "mailto:a@b.c", "//evil.com/x", "/\\evil.com", "\\\\evil.com", " javascript:x"]
        {
            assert_eq!(r(out), None, "{out} leaves the app");
        }
        assert_eq!(Loc::parse("/s?q=a+b&x=%C3%A9&f").unwrap().query("q").as_deref(), Some("a b"));
        assert_eq!(Loc::parse("/s?x=%C3%A9&f").unwrap().query("x").as_deref(), Some("é"));
        assert_eq!(Loc::parse("/s?f").unwrap().query("f").as_deref(), Some(""));
        assert_eq!(Loc::parse("/s?f").unwrap().query("g"), None);
    }

    #[test]
    fn a_link_is_active_on_the_page_it_leads_to() {
        let at = |s| Loc::parse(s).unwrap();
        assert!(is_active("/users", &at("/users/7"), false));
        assert!(!is_active("/users", &at("/users/7"), true));
        assert!(!is_active("/user", &at("/users"), false), "a prefix only at a segment");
        assert!(is_active("/about us", &at("/about%20us"), true));
        assert!(is_active("/users/", &at("/users"), true));
        assert!(is_active("/users?tab=1", &at("/users"), true));
        assert!(is_active("/users#top", &at("/users"), true));
        // Relative: from where it is rendered.
        assert!(is_active("api", &at("/docs/api"), true));
        assert!(is_active("../api", &at("/docs/api/v2"), false));
        assert!(!is_active("../api", &at("/docs/api/v2"), true));
        // `/` is active only at the root; a link out of the app never is.
        assert!(!is_active("/", &at("/x"), false));
        assert!(is_active("/", &at("/"), false));
        assert!(!is_active("https://example.com/", &at("/"), false));
    }

    #[test]
    fn the_same_place_replaces_and_a_new_one_pushes() {
        // aio tests/router-same-url-replaces.test.ts.
        let mut h = History::new("/users/7");
        assert!(h.push("/users/7"));
        assert!(h.push("7"), "the same place, spelled relative");
        assert_eq!(h.len(), 1);
        h.push("/users/8");
        h.push("/users/8?tab=1");
        h.push("/users/8?tab=1#top");
        h.push("/users/8?tab=1#top");
        assert_eq!(h.len(), 4);
        assert!(!h.push("https://example.com"), "out of the app: nothing changes");
        assert_eq!(h.here().to_string(), "/users/8?tab=1#top");
        // Back and Forward; a new place drops what Forward led to.
        assert!(h.go(-3) && h.here().path == "/users/7" && !h.go(-1) && h.can_go(3) && !h.can_go(4));
        assert!(h.go(1));
        h.push("/x");
        assert!(!h.go(1));
        assert_eq!(h.len(), 3);
        // A redirect leaves no Back step.
        assert!(h.replace("/y"));
        assert!(h.go(-1) && h.here().path == "/users/8");
        // A deep link that leaves the app starts at the root.
        assert_eq!(History::new("https://x.org/").here().path, "/");
    }

    #[test]
    fn the_pages_own_back_and_forward_step_the_history() {
        let mut h = History::new("/a");
        h.push("/b");
        h.push("/c");
        assert!(h.sync("/b") && h.here().path == "/b" && h.can_go(1), "the browser's Back: one step back");
        assert!(h.sync("/c") && h.here().path == "/c", "its Forward");
        assert!(!h.sync("/c"), "where it is: nothing");
        assert!(!h.sync("https://example.com/"), "out of the app: nothing");
        assert!(h.sync("/z?q=1") && h.here().to_string() == "/z?q=1" && h.len() == 3, "elsewhere: in place of here");
        assert!(step_back(&mut h) && h.here().path == "/b", "Back off a browser: history's");
    }

    #[test]
    fn history_is_bounded() {
        let mut h = History::new("/");
        for i in 0..HISTORY_MAX + 10 {
            h.push(&format!("/{i}"));
        }
        assert_eq!(h.len(), HISTORY_MAX);
        assert_eq!(h.here().path, format!("/{}", HISTORY_MAX + 9));
        assert!(h.go(1 - HISTORY_MAX as isize) && !h.go(-1));
    }

    #[test]
    fn a_link_to_a_page_fills_its_params() {
        assert_eq!(fill("/users/:id", &[("id", "a/b c".into())]), "/users/a%2Fb%20c");
        assert_eq!(fill("/f/*path", &[("path", "a b/c".into())]), "/f/a%20b/c");
        assert_eq!(fill("/files/*", &[("*", String::new())]), "/files");
        assert_eq!(fill("/", &[]), "/");
        let href = fill("/u/:id", &[("id", "100%41/é".into())]);
        assert_eq!(match_path("/u/:id", &href, true), p(&[("id", "100%41/é")]), "round trip");
    }

    #[test]
    fn a_nested_route_joins_its_parents_pattern_and_params() {
        assert_eq!(
            (join("", "/dash"), join("/dash", "settings"), join("/dash/", "/x/")),
            ("/dash".into(), "/dash/settings".into(), "/dash/x".into())
        );
        assert_eq!((join("", ""), join("/", "/")), ("/".into(), "/".into()));
        let root = Nest::default();
        let user = root.child("/users/:id", false, true, "/users/4/posts/9").unwrap();
        assert_eq!(user.pattern, "/users/:id");
        let post = user.child("posts/:post", false, false, "/users/4/posts/9").unwrap();
        assert_eq!(post.params, p(&[("id", "4"), ("post", "9")]).unwrap());
        assert_eq!(user.child("posts/:post", false, false, "/users/4/posts/9/x"), None, "a page: the whole path");
        // aio: an index under `/users/:id` matched only a literal "/users/:id".
        assert!(user.child("", true, false, "/users/4/").is_some());
        assert!(user.child("", true, false, "/users/4/posts").is_none());
        assert!(root.child("", true, false, "/").is_some() && root.child("", true, false, "/x").is_none());
        assert_eq!(root.child("", false, false, "/"), None, "no path, no index: never");
    }

    #[test]
    fn the_start_place_is_the_launchs_route() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(start(a(&["app", "--route", "/users/7"])), "/users/7");
        assert_eq!(start(a(&["app", "x.md", "--route=/f?q=1"])), "/f?q=1");
        assert_eq!(start(a(&["app", "--router"])), "/");
        assert_eq!(start(a(&["app", "--route"])), "/");
        assert_eq!(start(a(&["app"])), "/");
        assert_eq!(
            History::new(&start(a(&["app", "--route", "https://x.org/"]))).here().path,
            "/",
            "never out of the app"
        );
    }

    #[test]
    fn a_links_place_is_what_follows_its_scheme() {
        assert_eq!(of_link("myapp://users/7?tab=a#x"), "/users/7?tab=a#x");
        assert_eq!(of_link("myapp:/a"), "/a");
        assert_eq!(of_link("myapp:users"), "/users");
        assert_eq!(of_link("myapp://"), "/");
        assert_eq!(History::new(&of_link("myapp://users/7")).here().path, "/users/7");
    }

    #[test]
    fn back_goes_to_the_newest_handler_first_and_a_panic_is_handled() {
        // aio tests/on-back-button.test.ts.
        use std::cell::RefCell;
        use std::rc::Rc;
        let calls = Rc::new(RefCell::new(Vec::<&str>::new()));
        let log = |name: &'static str, took: bool| {
            let c = calls.clone();
            move || {
                c.borrow_mut().push(name);
                took
            }
        };
        let mut b = Backs::default();
        let screen = b.add(log("screen", true));
        b.add(log("dialog", false));
        assert!(b.press());
        assert_eq!(*calls.borrow(), ["dialog", "screen"]);
        // The id removes exactly its own, even for the same function twice.
        let f = log("twice", false);
        let one = b.add(f.clone());
        b.add(f);
        b.remove(one);
        b.remove(screen);
        calls.borrow_mut().clear();
        assert!(!b.press(), "nobody took it: the shell's default");
        assert_eq!(*calls.borrow(), ["twice", "dialog"]);
        b.add(|| panic!("broken handler"));
        assert!(b.press(), "a panicking handler counts as taken");
    }
}

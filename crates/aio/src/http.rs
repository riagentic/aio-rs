//! App HTTP (aio's `routes`, `route()` and static serving), on the sync
//! server, behind [`crate::gate`] like every path there.
//!
//! - **Routes**: `/users/:id` (a param is one non-empty, decoded segment) and
//!   a trailing `*` (the rest, kept encoded: a path, not a value). A trailing
//!   slash is the same path, except the root's; a route written with it wins
//!   for that spelling. Matched segment by segment after decoding, so `/café`
//!   answers `/caf%C3%A9` and `/a%2Fb` never reaches `/a/b`.
//! - **Method guard**: a path some route takes, asked with a method none of
//!   them allows, is 405 with `Allow`. HEAD rides with GET.
//! - **The key**: a route needs the app's key (`Authorization: Bearer`,
//!   `?token=`, or the key cookie a `?token=` link sets), unless it is
//!   [`Route::public`] (a webhook with its own signature check). Fail closed.
//!   With accounts, a live session opens it too, and [`Req::user`] is its
//!   user. A key in the URL is said once: it leaks into history and logs.
//! - **Bodies**: read whole up to [`Route::max_body`]; a [`Route::streamed`]
//!   one is handed over as it arrives (an upload into [`crate::blobs`]).
//! - **Files**: the shell ([`Http::shell`]: unknown route-shaped paths get its
//!   `index.html`, so deep links work) and extra read-only dirs
//!   ([`Http::dir`]), GET/HEAD only, behind the key, but a
//!   [`Http::public_dir`] (a sign-in page's logo) and, with accounts, the
//!   shell (the sign-in page itself). A mapped dir is not a weaker root: no
//!   dotfiles (but `.well-known`), no traversal, no symlink out, no file
//!   under a trailing slash.
//! - `/ws` and `/__aio…` are the framework's: no route or dir may claim them,
//!   and a `/*` catch-all never swallows them.
//!
//! - **Encoding**: every 200 is tagged and compressed as the client asks
//!   ([`encoding`]); a route serves a file with Range by [`serve_file`].
//!
//! A handler is app code: one that panics fails its request (500, logged
//! with route, method and path), never the server.
use axum::http::HeaderMap;
use futures_util::FutureExt;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use serde::Serialize;
use std::borrow::Cow;
use std::collections::HashMap;
use std::future::Future;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

pub mod encoding;
mod file;
pub use file::serve_file;

/// Largest request body a route is handed unless it says
/// ([`Route::max_body`]); a bigger one is 413, counted as it arrives.
pub const MAX_BODY: usize = 2 << 20;
/// A file this small is read whole to be served (tagged by its content and
/// compressed); a bigger one is streamed as it is read.
const BUFFER: u64 = 8 << 20;

pub type Params = HashMap<String, String>;

type Run = Arc<dyn Fn(Req) -> Pin<Box<dyn Future<Output = Reply> + Send>> + Send + Sync>;

pub use crate::config::reserved;

/// A query string's pairs, decoded as a form sends them (`+` is a space,
/// bad UTF-8 replaced): what axum's `Query` gives, without serde_urlencoded.
pub(crate) fn query_pairs(uri: &axum::http::Uri) -> Vec<(String, String)> {
    let dec = |s: &str| percent_decode_str(&s.replace('+', " ")).decode_utf8_lossy().into_owned();
    let pairs = uri.query().unwrap_or("").split('&').filter(|kv| !kv.is_empty());
    pairs.map(|kv| kv.split_once('=').unwrap_or((kv, ""))).map(|(k, v)| (dec(k), dec(v))).collect()
}

/// The request's query as a map (a repeated key: its last value).
pub(crate) struct Query(pub Params);

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for Query {
    type Rejection = std::convert::Infallible;
    async fn from_request_parts(parts: &mut axum::http::request::Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(Query(query_pairs(&parts.uri).into_iter().collect()))
    }
}

/// `v` as a JSON answer: what axum's `Json` gives, without its extractor's crates.
pub(crate) fn json(v: impl Serialize) -> axum::response::Response {
    use axum::response::IntoResponse;
    match serde_json::to_vec(&v) {
        Ok(b) => ([(axum::http::header::CONTENT_TYPE, "application/json")], b).into_response(),
        Err(e) => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// A path segment decoded; a malformed escape stays as written.
fn decode(s: &str) -> Cow<'_, str> {
    percent_decode_str(s).decode_utf8().unwrap_or(Cow::Borrowed(s))
}

/// `pattern`'s params for `path`, or `None` when it doesn't match.
pub fn match_route(pattern: &str, path: &str) -> Option<Params> {
    let pattern = match pattern.trim_end_matches('/') {
        "" => "/",
        p => p,
    };
    let path = path.strip_suffix('/').filter(|p| !p.is_empty()).unwrap_or(path);
    let segs: Vec<&str> = path.split('/').collect();
    let mut params = Params::new();
    let mut n = 0;
    for (i, p) in pattern.split('/').enumerate() {
        if p == "*" {
            params.insert("*".into(), segs.get(i..).unwrap_or_default().join("/"));
            return Some(params);
        }
        let u = segs.get(i)?;
        match p.strip_prefix(':') {
            Some(_) if u.is_empty() => return None,
            Some(name) => _ = params.insert(name.into(), decode(u).into_owned()),
            None if p != decode(u) => return None,
            None => {}
        }
        n = i + 1;
    }
    (n == segs.len()).then_some(params)
}

/// A literal route exactly as written, trailing slash and all.
fn exact(pattern: &str, path: &str) -> bool {
    pattern.split('/').map(Cow::Borrowed).eq(path.split('/').map(decode))
}

fn literal(pattern: &str) -> bool {
    !pattern.contains([':', '*'])
}

/// The request's cookies (every `Cookie` header), values decoded.
pub fn parse_cookies(headers: &HeaderMap) -> HashMap<String, String> {
    let all = headers.get_all(axum::http::header::COOKIE).iter().filter_map(|v| v.to_str().ok());
    all.flat_map(|h| h.split(';'))
        .filter_map(|part| part.split_once('='))
        .filter(|(k, _)| !k.trim().is_empty())
        .map(|(k, v)| (k.trim().to_string(), decode(v.trim()).into_owned()))
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SameSite {
    Strict,
    Lax,
    None,
}

/// A `Set-Cookie`'s attributes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CookieOpts {
    pub path: Option<String>,
    pub domain: Option<String>,
    /// Seconds.
    pub max_age: Option<i64>,
    pub expires: Option<chrono::DateTime<chrono::Utc>>,
    pub http_only: bool,
    pub secure: bool,
    pub same_site: Option<SameSite>,
}

/// What `encodeURIComponent` leaves as is.
const COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

/// One `Set-Cookie` value, the value encoded; `None` for a name that isn't
/// an RFC 6265 token (it goes in verbatim, so it could add attributes or a
/// second cookie).
pub fn set_cookie(name: &str, value: &str, o: &CookieOpts) -> Option<String> {
    let token = |c: char| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c);
    if name.is_empty() || !name.chars().all(token) {
        return None;
    }
    let mut s = format!("{name}={}", utf8_percent_encode(value, COMPONENT));
    // An attribute value with `;` or a line break would add attributes too.
    let clean = |v: &str| v.chars().filter(|c| *c != ';' && !c.is_control()).collect::<String>();
    if let Some(p) = &o.path {
        s += &format!("; Path={}", clean(p));
    }
    if let Some(d) = &o.domain {
        s += &format!("; Domain={}", clean(d));
    }
    if let Some(m) = o.max_age {
        s += &format!("; Max-Age={m}");
    }
    if let Some(e) = o.expires {
        s += &format!("; Expires={}", e.format("%a, %d %b %Y %H:%M:%S GMT"));
    }
    if o.http_only {
        s += "; HttpOnly";
    }
    if o.secure {
        s += "; Secure";
    }
    if let Some(ss) = o.same_site {
        s += &format!("; SameSite={ss:?}");
    }
    Some(s)
}

/// What a route is handed.
pub struct Req {
    method: String,
    path: String,
    params: Params,
    query: Vec<(String, String)>,
    cookies: HashMap<String, String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
    /// A streamed route's body, not yet read, and its limit (in a lock:
    /// a body isn't `Sync`).
    stream: std::sync::Mutex<Option<(axum::body::Body, usize)>>,
    ip: IpAddr,
    session: Option<crate::auth::Session>,
}

impl Req {
    /// Uppercase, as sent (`HEAD` too, on a GET route).
    pub fn method(&self) -> &str {
        &self.method
    }

    /// As sent: percent-encoded.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// A `:name` param (decoded), or `*`: the rest of the path (encoded).
    /// A decoded param may hold `/` or `..`: never join it onto a file path.
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params.get(name).map(String::as_str)
    }

    /// The first `?name=` value.
    pub fn query(&self, name: &str) -> Option<&str> {
        self.query.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    pub fn cookie(&self, name: &str) -> Option<&str> {
        self.cookies.get(name).map(String::as_str)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }

    /// At most the route's [`Route::max_body`] bytes; empty on a
    /// [`Route::streamed`] one.
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// The body as JSON.
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> crate::Result<T> {
        serde_json::from_slice(&self.body).map_err(|e| crate::Rejected::new(format!("bad JSON body: {e}")))
    }

    /// A [`Route::streamed`] route's body as it arrives, once (`None` after,
    /// or on a buffered route). A blocking read: read it off the async
    /// threads (`spawn_blocking`), e.g. into [`crate::blobs::Blobs::put`].
    /// A read past the route's `max_body` fails.
    pub fn body_reader(&self) -> Option<BodyReader> {
        use futures_util::StreamExt;
        let (body, max) = self.stream.lock().unwrap_or_else(|e| e.into_inner()).take()?;
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        tokio::spawn(async move {
            let (mut chunks, mut n) = (body.into_data_stream(), 0);
            while let Some(c) = chunks.next().await {
                let c = c.map_err(std::io::Error::other).and_then(|c| {
                    n += c.len();
                    let big = || format!("the body is too big: the limit is {max} bytes");
                    if n > max { Err(std::io::Error::new(std::io::ErrorKind::InvalidData, big())) } else { Ok(c) }
                });
                let end = c.is_err();
                if tx.send(c).await.is_err() || end {
                    return;
                }
            }
        });
        Some(BodyReader { rx, at: Default::default() })
    }

    /// The client's address: the TCP peer, or a trusted proxy's last hop ([`crate::gate`]).
    pub fn ip(&self) -> IpAddr {
        self.ip
    }

    /// With accounts, the user whose live session (`Authorization: Bearer`)
    /// came with the request (or whose token, with `Options::users`); `None`
    /// for the key or no one.
    pub fn user(&self) -> Option<&str> {
        self.session.as_ref().map(|s| s.user.as_str())
    }

    /// That user's role.
    pub fn role(&self) -> Option<&str> {
        self.session.as_ref().map(|s| s.role.as_str())
    }
}

/// A streamed body ([`Req::body_reader`]): a blocking [`std::io::Read`].
pub struct BodyReader {
    rx: tokio::sync::mpsc::Receiver<std::io::Result<axum::body::Bytes>>,
    at: axum::body::Bytes,
}

impl std::io::Read for BodyReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        while self.at.is_empty() {
            match self.rx.blocking_recv() {
                Some(c) => self.at = c?,
                None => return Ok(0),
            }
        }
        let n = buf.len().min(self.at.len());
        buf[..n].copy_from_slice(&self.at.split_to(n));
        Ok(n)
    }
}

/// What a route answers: built with `text`, `json` or `redirect`, then
/// `status`, `header` and `cookie` on top.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// A file sent as it is read, instead of `body`.
    window: Option<file::Window>,
}

impl Reply {
    fn of(content_type: &str, body: Vec<u8>) -> Self {
        Self { status: 200, headers: vec![("content-type".into(), content_type.into())], body, window: None }
    }

    pub fn text(body: impl Into<String>) -> Self {
        Self::of("text/plain; charset=utf-8", body.into().into_bytes())
    }

    /// An unserializable value is a 500, logged.
    pub fn json(v: &impl Serialize) -> Self {
        match serde_json::to_vec(v) {
            Ok(b) => Self::of("application/json; charset=utf-8", b),
            Err(e) => broken(&format!("a route's JSON answer: {e}")),
        }
    }

    /// 302 to `to`; `.status(303)` etc. for another.
    pub fn redirect(to: &str) -> Self {
        Self { status: 302, headers: vec![("location".into(), to.into())], body: Vec::new(), window: None }
    }

    pub fn status(mut self, code: u16) -> Self {
        self.status = code;
        self
    }

    /// Set `name`, replacing what it was (a content type, say).
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
        self.headers.push((name.to_ascii_lowercase(), value.into()));
        self
    }

    /// Add a cookie. A name that isn't a token makes the answer a 500,
    /// logged: the cookie is never sent half-made.
    pub fn cookie(mut self, name: &str, value: &str, opts: &CookieOpts) -> Self {
        match set_cookie(name, value, opts) {
            Some(c) => {
                self.headers.push(("set-cookie".into(), c));
                self
            }
            None => broken(&format!("invalid cookie name {name:?}: RFC 6265 token characters only")),
        }
    }

    fn into_response(self) -> axum::response::Response {
        let mut b = axum::http::Response::builder().status(self.status);
        for (k, v) in &self.headers {
            b = b.header(k, v);
        }
        let body = match self.window {
            Some(w) => {
                if !w.gzip {
                    b = b.header("content-length", w.len);
                }
                w.body()
            }
            None => axum::body::Body::from(self.body),
        };
        b.body(body).unwrap_or_else(|e| {
            eprintln!("[aio] http: a route's answer can't be sent: {e}");
            internal()
        })
    }
}

fn broken(why: &str) -> Reply {
    eprintln!("[aio] http: {why}; answered 500");
    Reply::text("Internal Server Error").status(500)
}

fn internal() -> axum::response::Response {
    let mut r = axum::response::Response::new(axum::body::Body::from("Internal Server Error"));
    *r.status_mut() = axum::http::StatusCode::INTERNAL_SERVER_ERROR;
    r
}

/// One route: a pattern, the methods it takes (none: any) and its handler.
#[derive(Clone)]
pub struct Route {
    pattern: String,
    methods: Vec<String>,
    public: bool,
    max_body: usize,
    streamed: bool,
    run: Run,
}

impl Route {
    /// Panics on a pattern that isn't a path or is the framework's (a
    /// programming error, found on the first run).
    pub fn new<F, Fut>(methods: &[&str], pattern: &str, f: F) -> Self
    where
        F: Fn(Req) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Reply> + Send + 'static,
    {
        assert!(pattern.starts_with('/'), "route {pattern:?}: a pattern starts with /");
        assert!(!reserved(pattern), "route {pattern:?}: /ws and /__aio… are aio's own paths");
        Self {
            pattern: pattern.into(),
            methods: methods.iter().map(|m| m.to_ascii_uppercase()).collect(),
            public: false,
            max_body: MAX_BODY,
            streamed: false,
            run: Arc::new(move |r| Box::pin(f(r))),
        }
    }

    pub fn get<F, Fut>(pattern: &str, f: F) -> Self
    where
        F: Fn(Req) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Reply> + Send + 'static,
    {
        Self::new(&["GET"], pattern, f)
    }

    pub fn post<F, Fut>(pattern: &str, f: F) -> Self
    where
        F: Fn(Req) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Reply> + Send + 'static,
    {
        Self::new(&["POST"], pattern, f)
    }

    /// Take bodies up to `bytes` (an upload); [`MAX_BODY`] otherwise.
    pub fn max_body(mut self, bytes: usize) -> Self {
        self.max_body = bytes;
        self
    }

    /// Hand the handler its body as it arrives ([`Req::body_reader`]),
    /// never buffered: an upload up to `max_body` costs a few chunks of
    /// memory (aio's `request.body` piped into `blobs.put`). A body said to be
    /// bigger is 413 before the handler runs.
    pub fn streamed(mut self) -> Self {
        self.streamed = true;
        self
    }

    /// Answer callers without the app's key.
    pub fn public(mut self) -> Self {
        self.public = true;
        self
    }

    fn allows(&self, method: &str) -> bool {
        let m = |x: &str| self.methods.iter().any(|y| y == x);
        self.methods.is_empty() || m(method) || (method == "HEAD" && m("GET"))
    }

    /// Its pattern as written.
    pub(crate) fn pattern(&self) -> &str {
        &self.pattern
    }

    /// Whether `o` takes requests this one takes: the same pattern with its
    /// params' names erased (`/u/:id` is `/u/:name`), a method both take.
    pub(crate) fn overlaps(&self, o: &Route) -> bool {
        fn shape(p: &str) -> impl Iterator<Item = &str> {
            p.split('/').map(|s| if s.starts_with(':') { ":" } else { s })
        }
        let methods = self.methods.is_empty() || o.methods.is_empty() || self.methods.iter().any(|m| o.allows(m));
        shape(&self.pattern).eq(shape(&o.pattern)) && methods
    }
}

/// The `Allow` header for routes that all refused `method`.
fn allow(routes: &[&Route]) -> String {
    let mut all: Vec<&str> = Vec::new();
    for m in routes.iter().flat_map(|r| &r.methods) {
        if !all.contains(&m.as_str()) {
            all.push(m);
        }
    }
    if all.contains(&"GET") && !all.contains(&"HEAD") {
        all.push("HEAD");
    }
    all.join(", ")
}

/// Which route answers `method path`: `Ok` with its params, `Err` with the
/// routes that take the path but not the method, or `None`: no route.
fn find<'a>(routes: &'a [Route], method: &str, path: &str) -> Option<Result<(&'a Route, Params), Vec<&'a Route>>> {
    if reserved(path) {
        return None;
    }
    let strict = routes.iter().filter(|r| literal(&r.pattern) && exact(&r.pattern, path)).map(|r| (r, Params::new()));
    let loose = routes.iter().filter(|r| literal(&r.pattern)).chain(routes.iter().filter(|r| !literal(&r.pattern)));
    let mut hits = strict.chain(loose.filter_map(|r| match_route(&r.pattern, path).map(|p| (r, p)))).peekable();
    hits.peek()?;
    let (ok, refused): (Vec<_>, Vec<_>) = hits.partition(|(r, _)| r.allows(method));
    Some(ok.into_iter().next().ok_or_else(|| refused.into_iter().map(|(r, _)| r).collect()))
}

/// A directory served read-only at `prefix`; the shell is the one at `/`
/// that answers route-shaped paths with its `index.html`.
#[derive(Clone)]
struct Root {
    prefix: String,
    dir: PathBuf,
    shell: bool,
    /// Read without the key.
    public: bool,
}

pub(crate) use crate::link::OnLink;
pub use crate::link::Presence;

/// One browser tab of a view drawn from this process ([`Http::ui`]): the
/// page's frames and who it is.
pub struct Tab {
    /// Who it is signed in as (accounts, or a user known by token); `None`:
    /// the key's holder, or with accounts nobody yet.
    pub user: Option<String>,
    /// The page's messages both ways, each a frame after its length (`u32`,
    /// big-endian). The tab ends when either side closes it.
    pub wire: tokio::io::DuplexStream,
}

pub(crate) type OnTab = Arc<dyn Fn(Tab) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// The app's UI page and what runs each tab of it.
#[derive(Clone)]
pub(crate) struct Ui {
    pub(crate) page: Arc<String>,
    pub(crate) tab: OnTab,
}

/// Who asks for the shell's page ([`Http::paint`]): the session's (or a
/// known token's) user and role; `None`: the key's holder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Viewer {
    pub user: Option<String>,
    pub role: Option<String>,
}

/// A page's first paint: its view's HTML for `<div id="main">`, the title
/// its view asked for, and what goes in its `<head>` (markup).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Paint {
    pub title: Option<String>,
    pub head: String,
    pub body: String,
}

pub(crate) type OnPaint = Arc<dyn Fn(&Viewer) -> Paint + Send + Sync>;

/// `page` with `p` in it (pure): its body in the page's empty `<div
/// id="main">`, its title in `<title>`, its head before `</head>`. A page
/// without them is as it was.
pub fn painted(page: &str, p: &Paint) -> String {
    let mut page = page.replacen("<div id=\"main\"></div>", &format!("<div id=\"main\">{}</div>", p.body), 1);
    if let (Some(t), Some(at), Some(end)) = (&p.title, page.find("<title>"), page.find("</title>")) {
        let t = t.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        page.replace_range(at + "<title>".len()..end, &t);
    }
    page.replacen("</head>", &format!("{}</head>", p.head), 1)
}


/// An app's HTTP surface: routes, then files; and the server functions its
/// clients call over `/ws`, and who hears of their links.
#[derive(Clone)]
pub struct Http {
    routes: Vec<Route>,
    /// Plugins' routes ([`crate::plugin`]): asked only for a path no route
    /// of the app's own takes, so the app always wins.
    plugin_routes: Vec<Route>,
    roots: Vec<Root>,
    compress: bool,
    pub(crate) fns: crate::fns::Fns,
    pub(crate) on_link: Option<OnLink>,
    blobs: Option<crate::blobs::Blobs>,
    pub(crate) users: Option<crate::auth::Users>,
    pub(crate) mail: Option<crate::auth::Mailer>,
    pub(crate) oidc: Option<Arc<crate::auth::oidc::Provider>>,
    pub(crate) ui: Option<Ui>,
    paint: Option<OnPaint>,
}

impl Default for Http {
    fn default() -> Self {
        Self {
            routes: Vec::new(),
            plugin_routes: Vec::new(),
            roots: Vec::new(),
            compress: true,
            fns: Default::default(),
            on_link: None,
            blobs: None,
            users: None,
            mail: None,
            oidc: None,
            ui: None,
            paint: None,
        }
    }
}

impl Http {
    /// Hear of each client link opening and closing (aio's `onConnect` /
    /// `onDisconnect`), for presence. A tool (`ax call`, the local door) is
    /// no link. Observe-only: a hook that panics is logged; the link goes on.
    pub fn on_link(mut self, f: impl Fn(&Presence) + Send + Sync + 'static) -> Self {
        self.on_link = Some(Arc::new(f));
        self
    }

    /// Compress answers (on by default; aio's `security.compress`). Off, they
    /// are still tagged and revalidated.
    pub fn compress(mut self, on: bool) -> Self {
        self.compress = on;
        self
    }

    /// Serve the server function `f` to the callers `access` lets in
    /// ([`crate::fns`]). Panics on a name served twice.
    pub fn server_fn<A, R, F, Fut>(mut self, f: crate::fns::ServerFn<A, R>, access: crate::fns::Access, run: F) -> Self
    where
        A: serde::de::DeserializeOwned + Send + 'static,
        R: Serialize + 'static,
        F: Fn(crate::fns::Req, A) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = crate::Result<R>> + Send + 'static,
    {
        self.fns.add(f, access, run);
        self
    }

    /// [`Http::server_fn`] for a function that sends its caller parts as it
    /// goes, before its answer ([`crate::fns::Chunks`]; `Remote::run_streamed`).
    pub fn server_fn_streamed<A, R, F, Fut>(
        mut self,
        f: crate::fns::ServerFn<A, R>,
        access: crate::fns::Access,
        run: F,
    ) -> Self
    where
        A: serde::de::DeserializeOwned + Send + 'static,
        R: Serialize + 'static,
        F: Fn(crate::fns::Req, A, crate::fns::Chunks) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = crate::Result<R>> + Send + 'static,
    {
        self.fns.add_streamed(f, access, run);
        self
    }

    /// Without accounts: the users the app knows by their token (aio
    /// `users` / `resolveUser`). Then every link, route, file and server fn
    /// needs one, and sees its user and role; the shared key opens none.
    pub fn users(mut self, users: crate::auth::Users) -> Self {
        self.users = Some(users);
        self
    }

    /// With accounts: how the app sends mail (aio's `auth.sendMail`: SMTP, an
    /// API, a log), for email verification and password resets. Without
    /// it those doors answer 501 `mail_not_configured`. Called off the
    /// async threads; an `Err` is logged (and, for a sign-up that needs
    /// verification, said as `verificationSent: false`).
    pub fn mail(mut self, f: impl Fn(&crate::auth::Mail) -> Result<(), String> + Send + Sync + 'static) -> Self {
        self.mail = Some(Arc::new(f));
        self
    }

    /// With accounts: sign in with identity provider `p` too (aio
    /// `auth.oidc`; [`crate::auth::oidc`]): a page links to
    /// `/__aio/auth/oidc/start?redirect=<path>`.
    #[cfg(feature = "oidc")]
    pub fn oidc(mut self, p: crate::auth::oidc::Oidc) -> Self {
        self.oidc = Some(Arc::new(crate::auth::oidc::Provider::new(p)));
        self
    }

    /// Serve `blobs`' bytes at `/__aio/blobs/<id>` ([`crate::blobs::url`]),
    /// to clients that show the key or a session, as the app's own files.
    pub fn blobs(mut self, blobs: crate::blobs::Blobs) -> Self {
        self.blobs = Some(blobs);
        self
    }

    pub fn route(mut self, r: Route) -> Self {
        self.routes.push(r);
        self
    }

    /// The routes, server functions and link hook of `p` (a plugin's own
    /// surface) under this one's: its routes after every one of these, its
    /// functions where these have none of that name, its hook beside this
    /// one's (both heard, each guarded). Pure.
    pub(crate) fn under(mut self, p: Http) -> Self {
        self.plugin_routes.extend(p.routes);
        self.fns.under(p.fns);
        self.on_link = match (self.on_link.take(), p.on_link) {
            (Some(own), Some(theirs)) => Some(Arc::new(move |l: &Presence| {
                for f in [&theirs, &own] {
                    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(l))).is_err() {
                        eprintln!(
                            "[aio] an on_link hook panicked (link {} by {}); the others still hear",
                            l.from, l.user
                        );
                    }
                }
            })),
            (own, theirs) => own.or(theirs),
        };
        self
    }

    /// Its own routes (not a plugin's), as declared.
    pub(crate) fn routes(&self) -> &[Route] {
        &self.routes
    }

    /// Serve a view to browser tabs drawn from this process (aio's tools:
    /// `aio_shell::web::tabs`; an app's browser page is its wasm build,
    /// [`Http::shell`] and [`Http::paint`]): `page` at `/`
    /// and every route-shaped path no route or dir takes (deep links), and
    /// each tab of it to `tab` over `/__aio/ui`. Open to who may use the
    /// app; with accounts the page is public (it is the sign-in) and a
    /// signed-out tab gets no link. It takes the shell's place.
    pub fn ui<F, Fut>(mut self, page: impl Into<String>, tab: F) -> Self
    where
        F: Fn(Tab) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.roots.retain(|r| !r.shell);
        self.ui = Some(Ui { page: Arc::new(page.into()), tab: Arc::new(move |t| Box::pin(tab(t))) });
        self
    }

    /// Serve the app shell (its built `dist`) at `/`.
    pub fn shell(mut self, dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        warn_root("the shell", &dir);
        self.ui = None;
        self.roots.retain(|r| !r.shell);
        self.roots.push(Root { prefix: "/".into(), dir, shell: true, public: false });
        self
    }

    /// The shell's page painted on the server for each viewer (aio's SSR,
    /// `aio_shell::web::painted`): `f`'s first paint in it before any
    /// script runs ([`painted`]). A painted page is never cached.
    pub fn paint(mut self, f: impl Fn(&Viewer) -> Paint + Send + Sync + 'static) -> Self {
        self.paint = Some(Arc::new(f));
        self
    }

    /// Serve `dir` read-only at `prefix` (`/shared`). Panics on a prefix that
    /// isn't a path or is the framework's.
    pub fn dir(self, prefix: &str, dir: impl Into<PathBuf>) -> Self {
        self.root(prefix, dir.into(), false)
    }

    /// [`Http::dir`] for anyone, without the key (aio's `publicDir`): only
    /// what a page shown before sign-in needs.
    pub fn public_dir(self, prefix: &str, dir: impl Into<PathBuf>) -> Self {
        self.root(prefix, dir.into(), true)
    }

    /// The dirs `aio.toml` serves (`[server] dirs`, `public_dirs`), a
    /// relative one in `files` (the app's files tier).
    pub fn configured(self, server: &crate::config::Server, files: &Path) -> Self {
        let dirs = server.dirs.iter().map(|d| (d, false)).chain(server.public_dirs.iter().map(|d| (d, true)));
        dirs.fold(self, |h, ((prefix, dir), public)| h.root(prefix, files.join(dir), public))
    }

    fn root(mut self, prefix: &str, dir: PathBuf, public: bool) -> Self {
        let prefix = prefix.trim_end_matches('/');
        assert!(prefix.starts_with('/'), "dir {prefix:?}: a prefix starts with / and isn't /");
        assert!(!reserved(prefix), "dir {prefix:?}: /ws and /__aio… are aio's own paths");
        warn_root(&format!("dir {prefix:?}"), &dir);
        self.roots.push(Root { prefix: prefix.into(), dir, shell: false, public });
        // Longest prefix first; the shell (`/`) last.
        self.roots.sort_by_key(|r| (r.shell, std::cmp::Reverse(r.prefix.len())));
        self
    }
}

/// A root that isn't a directory, said once, resolved: a relative path that
/// points elsewhere than meant is the likely mistake.
fn warn_root(what: &str, dir: &Path) {
    if let Some(w) = root_problem(what, dir) {
        eprintln!("[aio] http: {w}");
    }
}

fn root_problem(what: &str, dir: &Path) -> Option<String> {
    (!dir.is_dir()).then(|| {
        let at = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
        format!("{what} is served from {}, which is not a directory: its paths answer 404", at.display())
    })
}

/// What a file path asks for.
#[derive(Debug, PartialEq, Eq)]
enum Ask {
    /// This file (it may not exist).
    File(PathBuf),
    /// No file: a path ending in `/`, or no root takes it.
    Nothing,
    /// Never served: a dotfile, a traversal, a separator inside a segment.
    Refused,
}

/// Where `path` points under `roots` (pure), and whether that root is the shell.
fn locate<'a>(roots: &'a [Root], path: &str) -> (Option<&'a Root>, Ask) {
    let Some(root) = roots.iter().find(|r| {
        r.shell || path == r.prefix || path.strip_prefix(&r.prefix).is_some_and(|rest| rest.starts_with('/'))
    }) else {
        return (None, Ask::Nothing);
    };
    let rest = if root.shell { path } else { &path[root.prefix.len()..] };
    let segs: Vec<Cow<str>> = rest.split('/').skip(1).map(decode).collect();
    let mut file = root.dir.clone();
    for (i, s) in segs.iter().enumerate() {
        let last = i + 1 == segs.len();
        if s.is_empty() && !last {
            return (Some(root), Ask::Refused);
        }
        // `:` would turn into a drive or a stream on Windows.
        let bad = s.contains(['/', '\\', '\0', ':']) || (s.starts_with('.') && !(i == 0 && s == ".well-known"));
        if bad {
            return (Some(root), Ask::Refused);
        }
        file.push(s.as_ref());
    }
    match segs.last() {
        Some(s) if !s.is_empty() => (Some(root), Ask::File(file)),
        _ => (Some(root), Ask::Nothing),
    }
}

/// `file`, if it is a file inside `dir` once links are followed.
fn inside(dir: &Path, file: &Path) -> Option<PathBuf> {
    let (dir, file) = (dir.canonicalize().ok()?, file.canonicalize().ok()?);
    (file.starts_with(&dir) && file.is_file()).then_some(file)
}

/// The content type for a file name, by extension (any case); `None`: not
/// a kind of file this serves by name, so a missing one is route-shaped.
fn content_type(name: &str) -> Option<&'static str> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "json" | "map" => "application/json",
        "webmanifest" => "application/manifest+json",
        "wasm" => "application/wasm",
        "txt" => "text/plain",
        "md" => "text/markdown",
        "csv" => "text/csv",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "pdf" => "application/pdf",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        _ => return None,
    })
}

/// The type a file goes out as: text says UTF-8 only when it is.
fn served_type(name: &str, bytes: &[u8]) -> String {
    let t = content_type(name).unwrap_or("application/octet-stream");
    let text = t.starts_with("text/") || t.ends_with("json") || t.ends_with("xml");
    if text && std::str::from_utf8(bytes).is_ok() { format!("{t}; charset=utf-8") } else { t.into() }
}

/// The file `path` serves, or `None` (404).
fn file_for(roots: &[Root], path: &str) -> Option<PathBuf> {
    let (root, ask) = locate(roots, path);
    let root = root?;
    let shell = || root.shell.then(|| inside(&root.dir, &root.dir.join("index.html"))).flatten();
    let route_shaped = route_shaped(path);
    match ask {
        Ask::Refused => None,
        Ask::File(f) => inside(&root.dir, &f).or_else(|| (f.is_dir() || route_shaped).then(shell)?),
        Ask::Nothing => route_shaped.then(shell)?,
    }
}

/// A name with a file's extension is a file, also asked with a trailing `/`.
fn route_shaped(path: &str) -> bool {
    content_type(path.trim_end_matches('/').rsplit('/').next().unwrap_or("")).is_none()
}

fn path_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// How the sync server checks the key and hands it to a browser.
pub(crate) struct Key {
    pub(crate) token: Arc<String>,
    /// The cookie that carries it (per app: cookies ignore the port).
    pub(crate) cookie: String,
    /// Set the cookie `Secure` (served over TLS).
    pub(crate) secure: bool,
    /// The header a trusted proxy writes ([`crate::gate`]): its
    /// `X-Forwarded-Proto: https` makes the cookie `Secure` too.
    pub(crate) trust: Option<String>,
    /// Accounts: a live session opens routes as its user.
    pub(crate) auth: Option<Arc<crate::auth::Auth>>,
    /// The cookie a browser shows its session in.
    pub(crate) session_cookie: String,
    /// The shell's per-answer nonce, if asked for.
    pub(crate) page: Arc<crate::security::Page>,
    /// Wrong keys per address ([`crate::sync::unauthorized`]).
    pub(crate) guessed: crate::sync::Guessed,
    /// Users known by token: then the key opens nothing.
    pub(crate) users: Option<crate::auth::Users>,
    /// Who may frame the UI page (`[server] hosts`).
    pub(crate) ancestors: String,
    /// `[server] open`: no key is asked (with accounts, sessions still are).
    pub(crate) open: bool,
}

/// Whether the request shows the key, and the cookie to set when it showed
/// it by `?token=` only (a link), so the page's own requests carry it.
fn keyed(key: &Key, headers: &HeaderMap, query: &[(String, String)]) -> (bool, Option<String>) {
    let q: HashMap<String, String> = query.iter().filter(|(k, _)| k == "token").cloned().collect();
    let by_cookie = parse_cookies(headers).get(&key.cookie).is_some_and(|c| crate::sync::same(c, &key.token));
    let shown = crate::sync::shown_key(headers, &q).is_some_and(|k| crate::sync::same(&k, &key.token));
    let by_link = !by_cookie && q.get("token").is_some_and(|k| crate::sync::same(k, &key.token));
    let opts = CookieOpts {
        path: Some("/".into()),
        http_only: true,
        secure: key.secure || crate::gate::proxied_https(headers, key.trust.as_deref()),
        same_site: Some(SameSite::Strict),
        ..Default::default()
    };
    (shown || by_cookie, by_link.then(|| set_cookie(&key.cookie, &key.token, &opts)).flatten())
}

/// Said once: a key in a URL leaks into history, proxy logs and `Referer`.
fn warn_token_in_url() {
    static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
        eprintln!(
            "[aio] security: the app key came in a URL (?token=): it leaks into browser history, proxy logs and \
             Referer. Share links set a cookie and drop it; tools send Authorization: Bearer"
        );
    }
}

fn plain(status: u16, msg: &str) -> Reply {
    Reply::text(msg).status(status)
}

/// Answer a request no framework route took.
pub(crate) async fn answer(
    http: &Http,
    key: &Key,
    req: axum::extract::Request,
    ip: IpAddr,
) -> axum::response::Response {
    let (head, body) = req.into_parts();
    let asked = head.headers.clone();
    let method = head.method.as_str().to_ascii_uppercase();
    let path = head.uri.path().to_string();
    let query = query_pairs(&head.uri);
    // Blob bytes: the framework's path, behind the same key as the app's own.
    let blob = http.blobs.as_ref().zip(path.strip_prefix(crate::blobs::URL_PREFIX));
    if reserved(&path) && blob.is_none() {
        return plain(404, "Not Found").into_response();
    }
    let found = match blob {
        Some(_) => None,
        None => find(&http.routes, &method, &path).or_else(|| find(&http.plugin_routes, &method, &path)),
    };
    // Let through by the gate: a browser navigating by a form POST. Only a
    // client route answers it (the shell); a route never sees a form body.
    let json = asked
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|c| c.split(';').next().is_some_and(|m| m.trim().eq_ignore_ascii_case("application/json")));
    let form_nav = method == "POST" && !json;
    if form_nav && found.is_some() {
        return plain(415, "send the body as Content-Type: application/json").into_response();
    }
    // A file is public in a public dir, or in the shell when there are
    // accounts (the sign-in page must load).
    let public_file = || locate(&http.roots, &path).0.is_some_and(|r| r.public || (r.shell && key.auth.is_some()));
    let ui_page = found.is_none()
        && blob.is_none()
        && (method == "GET" || method == "HEAD" || form_nav)
        && route_shaped(&path)
        && locate(&http.roots, &path).0.is_none();
    let ui_page = http.ui.as_ref().filter(|_| ui_page);
    let public = match &found {
        Some(Ok((r, _))) => r.public,
        Some(Err(_)) => false,
        None => blob.is_none() && (public_file() || (ui_page.is_some() && key.auth.is_some())),
    };
    let (ok, cookie) = match () {
        _ if key.users.is_some() => (false, None),
        _ if key.open && key.auth.is_none() => (true, None),
        _ => keyed(key, &head.headers, &query),
    };
    let shown = crate::auth::routes::bearer(&head.headers)
        .or_else(|| parse_cookies(&head.headers).remove(&key.session_cookie).filter(|t| !t.is_empty()));
    let shown_session = shown.is_some();
    let session = match (&key.auth, &key.users, shown) {
        (Some(a), _, Some(t)) if !ok || public => {
            let a = a.clone();
            tokio::task::spawn_blocking(move || crate::link::live(&a, &t)).await.ok().flatten()
        }
        (None, Some(u), Some(t)) => u.who(&t),
        _ => None,
    };
    if !ok && !public && session.is_none() {
        let q: HashMap<String, String> = query.iter().filter(|(k, _)| k == "token").cloned().collect();
        let shown = crate::sync::shown_key(&head.headers, &q).is_some()
            || parse_cookies(&head.headers).contains_key(&key.cookie)
            || shown_session;
        return crate::sync::unauthorized(&key.guessed, ip, shown, "wrong or missing key");
    }
    if cookie.is_some() {
        warn_token_in_url();
    }
    let reply = match found {
        None if let Some((blobs, id)) = blob => serve_blob(blobs, &asked, &method, id).await,
        Some(Ok((route, params))) => match buffered(route, &asked, body).await {
            Err(_) => plain(413, &format!("the body is too big: the limit is {} bytes", route.max_body)),
            Ok((body, stream)) => {
                let cookies = parse_cookies(&head.headers);
                let r = Req {
                    method: method.clone(),
                    path: path.clone(),
                    params,
                    query,
                    cookies,
                    headers: head.headers,
                    body,
                    stream: stream.into(),
                    ip,
                    session,
                };
                std::panic::AssertUnwindSafe((route.run)(r)).catch_unwind().await.unwrap_or_else(|p| {
                    let why = p
                        .downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| p.downcast_ref::<String>().cloned())
                        .unwrap_or_default();
                    broken(&format!("route {:?} ({method} {path}) panicked: {why}", route.pattern))
                })
            }
        },
        Some(Err(refused)) => plain(405, "Method Not Allowed").header("allow", &allow(&refused)),
        None if let Some(ui) = ui_page => Reply::of("text/html; charset=utf-8", ui.page.as_bytes().to_vec())
            .header("cache-control", "no-store")
            // The page names its own policy (its scripts by hash); who may frame it is the header's.
            .header("content-security-policy", &format!("frame-ancestors {}", key.ancestors)),
        None if http.roots.is_empty() => plain(404, "Not Found"),
        None if form_nav && route_shaped(&path) && http.roots.iter().any(|r| r.shell) => {
            stamped(&key.page, serve_static(&http.roots, &HeaderMap::new(), "/").await)
        }
        None if method != "GET" && method != "HEAD" => plain(405, "Method Not Allowed").header("allow", "GET, HEAD"),
        None if locate(&http.roots, &path).0.is_some_and(|r| r.shell) => {
            let r = serve_static(&http.roots, &asked, &path).await;
            let page = route_shaped(&path) || path.ends_with("/index.html");
            let viewer = Viewer { user: session.as_ref().map(|s| s.user.clone()), role: session.map(|s| s.role) };
            let r = match http.paint.clone().filter(|_| page) {
                Some(f) => paint_reply(r, f, viewer).await,
                None => r,
            };
            stamped(&key.page, r)
        }
        None => serve_static(&http.roots, &asked, &path).await,
    };
    let reply = match cookie {
        Some(c) if (200..400).contains(&reply.status) => {
            Reply { headers: [reply.headers, vec![("set-cookie".into(), c)]].concat(), ..reply }
        }
        _ => reply,
    };
    let compress = http.compress;
    if reply.body.len() < encoding::MIN_COMPRESS {
        return encoding::encode(&method, &asked, reply, compress).into_response();
    }
    tokio::task::spawn_blocking(move || encoding::encode(&method, &asked, reply, compress))
        .await
        .map_or_else(|_| internal(), Reply::into_response)
}

/// A route's body: read whole, or (streamed) left to the handler unless its
/// said length is already over the limit.
async fn buffered(
    route: &Route,
    asked: &HeaderMap,
    body: axum::body::Body,
) -> Result<(axum::body::Bytes, Option<(axum::body::Body, usize)>), ()> {
    if !route.streamed {
        return axum::body::to_bytes(body, route.max_body).await.map(|b| (b, None)).map_err(|_| ());
    }
    let said = asked.get("content-length").and_then(|v| v.to_str().ok()?.parse::<u64>().ok());
    match said.is_some_and(|n| n > route.max_body as u64) {
        true => Err(()),
        false => Ok((Default::default(), Some((body, route.max_body)))),
    }
}

/// `GET /__aio/blobs/<id>`: the bytes as a served file is (Range, inert
/// type from its name, sandboxed), tagged by the id, cached for good (the
/// id is the content). An id that is not a blob's, or no blob: 404.
async fn serve_blob(blobs: &crate::blobs::Blobs, asked: &HeaderMap, method: &str, id: &str) -> Reply {
    if method != "GET" && method != "HEAD" {
        return plain(405, "Method Not Allowed").header("allow", "GET, HEAD");
    }
    if !crate::blobs::id_ok(id) {
        return plain(404, "Not Found");
    }
    let (blobs, id, asked) = (blobs.clone(), id.to_string(), asked.clone());
    let sent = tokio::task::spawn_blocking(move || {
        let Some(b) = blobs.info(&id)? else { return Ok(None) };
        let Some(f) = blobs.path(&id)?.map(|p| file::open(&p)).transpose()?.flatten() else { return Ok(None) };
        let ct = b.name.as_deref().map_or("application/octet-stream", file::inert_type);
        let headers = [
            ("content-type", ct),
            ("etag", &format!("\"{id}\"")),
            ("cache-control", "private, max-age=31536000, immutable, no-transform"),
            ("content-security-policy", "sandbox"),
            ("x-content-type-options", "nosniff"),
        ];
        Ok::<_, std::io::Error>(Some(file::send(
            &asked,
            &f,
            headers.map(|(k, v)| (k.to_string(), v.to_string())).to_vec(),
        )))
    });
    match sent.await {
        Ok(Ok(Some(r))) => r,
        Ok(Ok(None)) => plain(404, "Not Found"),
        Ok(Err(e)) => broken(&format!("a blob can't be served: {e}")),
        Err(e) => broken(&format!("a blob can't be served: {e}")),
    }
}

/// The shell's page `r` (whole, an HTML 200) painted for `viewer` by `f`,
/// off the async threads (a view renders); never cached, nor tagged.
async fn paint_reply(r: Reply, f: OnPaint, viewer: Viewer) -> Reply {
    let html = r.headers.iter().any(|(k, v)| k == "content-type" && v.starts_with("text/html"));
    let Some(page) = (r.status == 200 && r.window.is_none() && html)
        .then(|| String::from_utf8(r.body.clone()).ok())
        .flatten()
    else {
        return r;
    };
    match tokio::task::spawn_blocking(move || painted(&page, &f(&viewer))).await {
        Ok(page) => {
            let mut r = Reply { body: page.into_bytes(), ..r };
            r.headers.retain(|(k, _)| k != "etag" && k != "cache-control");
            r.header("cache-control", "no-store")
        }
        Err(e) => broken(&format!("the page's first paint failed: {e}")),
    }
}

/// A shell page with its scripts nonced and the policy naming the nonce,
/// when one is asked for ([`crate::security::Page::stamp`]); never cached,
/// as no two answers are the same.
fn stamped(page: &crate::security::Page, r: Reply) -> Reply {
    let html = r.headers.iter().any(|(k, v)| k == "content-type" && v.starts_with("text/html"));
    let Some((body, csp)) = (r.status == 200 && r.window.is_none() && html)
        .then(|| std::str::from_utf8(&r.body).ok())
        .flatten()
        .and_then(|h| page.stamp(h))
    else {
        return r;
    };
    let mut r = Reply { body: body.into_bytes(), ..r };
    r.headers.retain(|(k, _)| k != "etag");
    r.header("cache-control", "no-store").header("content-security-policy", &csp)
}

async fn serve_static(roots: &[Root], asked: &HeaderMap, path: &str) -> Reply {
    let (roots, asked, path) = (roots.to_vec(), asked.clone(), path.to_string());
    let read = tokio::task::spawn_blocking(move || {
        let Some(f) = file_for(&roots, &path) else { return Ok(None) };
        let Some(opened) = file::open(&f)? else { return Ok(None) };
        static_reply(&asked, &opened, &path_name(&f)).map(Some)
    });
    match read.await {
        Ok(Ok(Some(r))) => r,
        Ok(Ok(None)) => plain(404, "Not Found"),
        Ok(Err(e)) => broken(&format!("a file can't be served: {}", e.kind())),
        Err(e) => broken(&format!("a file can't be served: {e}")),
    }
}

/// A static file's answer: a small one is read whole (typed by its bytes,
/// tagged by its content, so it can be compressed); a big one is streamed
/// with its mtime+size tag. Both take a Range.
fn static_reply(asked: &HeaderMap, f: &file::Opened, name: &str) -> std::io::Result<Reply> {
    let whole = file::Window { file: f.file.clone(), start: 0, len: f.size, gzip: false };
    let all = (f.size <= BUFFER).then(|| whole.read()).transpose()?;
    let (ct, etag) = match &all {
        Some(b) => (served_type(name, b), encoding::etag_of(b)),
        None => (content_type(name).unwrap_or("application/octet-stream").into(), f.etag.clone()),
    };
    let mut headers = vec![("content-type".to_string(), ct), ("etag".into(), etag)];
    // The shell must pick up a new build at once.
    if name == "index.html" {
        headers.push(("cache-control".into(), "no-cache".into()));
    }
    let mut r = file::send(asked, f, headers);
    if let Some(b) = all
        && let Some(w) = r.window.take()
    {
        let at = |n: u64| usize::try_from(n).unwrap_or(usize::MAX).min(b.len());
        r.body = b[at(w.start)..at(w.start + w.len)].to_vec();
    }
    Ok(r)
}

pub(crate) trait Io: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<X: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> Io for X {}

/// The most of an answer [`request`] reads.
const ANSWER: u64 = 1 << 20;

/// One HTTP request on `s`: a POST of `body`, or a GET without; the status
/// and the JSON answer (at most [`ANSWER`] bytes of it are read).
pub(crate) async fn request(
    mut s: Box<dyn Io>,
    host: &str,
    path: &str,
    bearer: Option<&str>,
    body: Option<&serde_json::Value>,
) -> std::io::Result<(u16, serde_json::Value)> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let auth = bearer.map(|k| format!("Authorization: Bearer {k}\r\n")).unwrap_or_default();
    let req = match body.map(|b| b.to_string()) {
        Some(body) => format!(
            "POST {path} HTTP/1.1\r\nHost: {host}\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
        None => format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n{auth}Connection: close\r\n\r\n"),
    };
    s.write_all(req.as_bytes()).await?;
    // An answer is a few KiB: a server that sends on and on is cut at 1 MiB.
    let mut out = Vec::new();
    _ = tokio::time::timeout(std::time::Duration::from_secs(15), s.take(ANSWER).read_to_end(&mut out)).await;
    let text = String::from_utf8_lossy(&out);
    let bad = || std::io::Error::other("not an HTTP answer");
    let (head, rest) = text.split_once("\r\n\r\n").ok_or_else(bad)?;
    let status = head.get(9..12).and_then(|c| c.parse().ok()).ok_or_else(bad)?;
    Ok((status, serde_json::from_str(rest).unwrap_or_default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A server that answers on and on is read up to the cap, at once.
    #[tokio::test]
    async fn an_endless_answer_is_cut_at_its_cap() {
        use tokio::io::AsyncWriteExt;
        let (c, mut srv) = tokio::io::duplex(1 << 16);
        tokio::spawn(async move {
            _ = srv.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await;
            while srv.write_all(&[b' '; 4096]).await.is_ok() {}
        });
        let t = std::time::Instant::now();
        let (status, _) = request(Box::new(c), "h", "/", None, None).await.unwrap();
        assert_eq!(status, 200);
        assert!(t.elapsed() < std::time::Duration::from_secs(5), "{:?}", t.elapsed());
    }

    fn params(kv: &[(&str, &str)]) -> Option<Params> {
        Some(kv.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect())
    }

    #[test]
    fn a_query_decodes_as_a_form_sends_it() {
        let q = |s: &str| query_pairs(&s.parse().unwrap());
        let own = |v: &[(&str, &str)]| v.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>();
        assert_eq!(q("/p?a=1&b=x+y%20z&&c&%C3%A9=%2B"), own(&[("a", "1"), ("b", "x y z"), ("c", ""), ("é", "+")]));
        assert_eq!(q("/p?k=%FF&bad=%zz"), own(&[("k", "\u{fffd}"), ("bad", "%zz")]));
        assert!(q("/p").is_empty() && q("/p?").is_empty());
    }

    #[test]
    fn patterns_match_as_aio_and_its_client_router() {
        let none = None;
        let table: &[(&str, &str, Option<Params>)] = &[
            ("/users", "/users", params(&[])),
            ("/users", "/users/", params(&[])),
            ("/users/", "/users", params(&[])),
            ("/users", "/users//", none.clone()),
            ("/users", "/user", none.clone()),
            ("/users", "/users/42", none.clone()),
            ("/users/:id", "/users/42", params(&[("id", "42")])),
            ("/users/:id", "/users/42/", params(&[("id", "42")])),
            ("/users/:id", "/users/", none.clone()),
            ("/users/:id", "/users", none.clone()),
            ("/users/:id", "/users/1/2", none.clone()),
            ("/users/:id", "/users/a%20b", params(&[("id", "a b")])),
            ("/u/:a/:b", "/u/x/y", params(&[("a", "x"), ("b", "y")])),
            ("/a/:x/b", "/a/1/b/", params(&[("x", "1")])),
            ("/files/*", "/files", params(&[("*", "")])),
            ("/files/*", "/files/", params(&[("*", "")])),
            ("/files/*", "/files/a/b", params(&[("*", "a/b")])),
            ("/files/*", "/files/a/b/", params(&[("*", "a/b")])),
            ("/files/*", "/files/a%20b", params(&[("*", "a%20b")])),
            ("/files/*", "/filesx", none.clone()),
            ("/files/*", "/other/a", none.clone()),
            ("/files/:dir/*", "/files/x", params(&[("dir", "x"), ("*", "")])),
            ("/files/:dir/*", "/files/x/y/z", params(&[("dir", "x"), ("*", "y/z")])),
            ("/*", "/", params(&[("*", "")])),
            ("/*", "/a/b", params(&[("*", "a/b")])),
            ("/", "/", params(&[])),
            ("/", "/x", none.clone()),
            ("/a*b", "/a*b", params(&[])),
            // Written as text, asked encoded; an encoded `/` is no separator.
            ("/café", "/caf%C3%A9", params(&[])),
            ("/menü/:item", "/men%C3%BC/tee", params(&[("item", "tee")])),
            ("/with space", "/with%20space", params(&[])),
            ("/a/b", "/a%2Fb", none.clone()),
            ("/files/a%2Fb", "/files/a%2fb", none),
            // A malformed escape stays as written, never fails.
            ("/users/:id", "/users/%zz", params(&[("id", "%zz")])),
        ];
        for (pattern, path, want) in table {
            assert_eq!(&match_route(pattern, path), want, "{pattern} <- {path}");
        }
    }

    #[test]
    fn the_framework_paths_are_reserved() {
        for p in ["/ws", "/__aio", "/__aio/health", "/__aio/pin"] {
            assert!(reserved(p), "{p}");
        }
        for p in ["/", "/api/ws", "/wsx", "/aio", "/x/__aio"] {
            assert!(!reserved(p), "{p}");
        }
        assert!(std::panic::catch_unwind(|| Route::get("/__aio/x", |_| async { Reply::text("") })).is_err());
        assert!(std::panic::catch_unwind(|| Route::get("/ws", |_| async { Reply::text("") })).is_err());
        assert!(std::panic::catch_unwind(|| Http::default().dir("/__aio", ".")).is_err());
    }

    fn noop(methods: &[&str], pattern: &str) -> Route {
        Route::new(methods, pattern, |_| async { Reply::text("") })
    }

    fn found(routes: &[Route], method: &str, path: &str) -> Option<std::result::Result<String, String>> {
        find(routes, method, path).map(|r| r.map(|(r, _)| r.pattern.clone()).map_err(|refused| allow(&refused)))
    }

    #[test]
    fn methods_are_guarded_and_head_rides_with_get() {
        let routes = [noop(&["GET"], "/items"), noop(&["post"], "/items"), noop(&["PUT"], "/only"), noop(&[], "/any")];
        assert_eq!(found(&routes, "GET", "/items"), Some(Ok("/items".into())));
        assert_eq!(found(&routes, "HEAD", "/items"), Some(Ok("/items".into())));
        assert!(find(&routes, "POST", "/items").is_some_and(|r| r.is_ok_and(|(r, _)| r.methods == ["POST"])));
        assert_eq!(found(&routes, "DELETE", "/items"), Some(Err("GET, POST, HEAD".into())));
        assert_eq!(found(&routes, "GET", "/only"), Some(Err("PUT".into())));
        assert_eq!(found(&routes, "PATCH", "/any"), Some(Ok("/any".into())));
        assert_eq!(found(&routes, "GET", "/nope"), None);
    }

    #[test]
    fn a_literal_wins_and_both_spellings_keep_their_own_route() {
        // Patterns go in the order written, after every literal.
        let routes =
            [noop(&[], "/users/:id"), noop(&[], "/*"), noop(&[], "/both"), noop(&[], "/both/"), noop(&[], "/users/me")];
        assert_eq!(found(&routes, "GET", "/both"), Some(Ok("/both".into())));
        assert_eq!(found(&routes, "GET", "/both/"), Some(Ok("/both/".into())));
        assert_eq!(found(&routes, "GET", "/users/me"), Some(Ok("/users/me".into())));
        assert_eq!(found(&routes, "GET", "/users/7"), Some(Ok("/users/:id".into())));
        assert_eq!(found(&routes, "GET", "/deep/link"), Some(Ok("/*".into())));
        // A catch-all never takes the framework's paths.
        assert_eq!(found(&routes, "GET", "/__aio/health"), None);
        assert_eq!(found(&routes, "GET", "/ws"), None);
    }

    fn cookie_header(v: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(axum::http::header::COOKIE, v.parse().unwrap());
        h
    }

    #[test]
    fn cookies_parse_decoded_and_never_fail() {
        let c = parse_cookies(&cookie_header("a=1; b=hello%20world; c=; junk; =x"));
        assert_eq!(c.len(), 3);
        assert_eq!((c["a"].as_str(), c["b"].as_str(), c["c"].as_str()), ("1", "hello world", ""));
        assert!(parse_cookies(&HeaderMap::new()).is_empty());
        let c = parse_cookies(&cookie_header("sid=%zz; ok=a%20b; bare=%E0%A4%A"));
        assert_eq!((c["sid"].as_str(), c["ok"].as_str(), c["bare"].as_str()), ("%zz", "a b", "%E0%A4%A"));
    }

    #[test]
    fn set_cookie_writes_attributes_and_refuses_injecting_names() {
        let o = CookieOpts {
            http_only: true,
            path: Some("/".into()),
            same_site: Some(SameSite::Lax),
            ..Default::default()
        };
        assert_eq!(set_cookie("sid", "abc", &o).unwrap(), "sid=abc; Path=/; HttpOnly; SameSite=Lax");
        assert_eq!(set_cookie("k", "a b", &CookieOpts::default()).unwrap(), "k=a%20b");
        assert_eq!(set_cookie("sid", "a b;c", &CookieOpts::default()).unwrap(), "sid=a%20b%3Bc");
        assert_eq!(
            set_cookie("__Host-x", "1", &CookieOpts { path: Some("/".into()), ..Default::default() }).unwrap(),
            "__Host-x=1; Path=/"
        );
        let all = CookieOpts {
            domain: Some("x.local".into()),
            max_age: Some(60),
            expires: chrono::DateTime::from_timestamp(0, 0),
            secure: true,
            same_site: Some(SameSite::Strict),
            path: Some("/a; HttpOnly".into()),
            ..Default::default()
        };
        assert_eq!(
            set_cookie("s", "v", &all).unwrap(),
            "s=v; Path=/a HttpOnly; Domain=x.local; Max-Age=60; Expires=Thu, 01 Jan 1970 00:00:00 GMT; Secure; SameSite=Strict"
        );
        for bad in ["sid; Path=/", "a=b", "sid\nSet-Cookie: x=y", "s id", "", "é"] {
            assert_eq!(set_cookie(bad, "v", &CookieOpts::default()), None, "{bad:?}");
        }
    }

    #[test]
    fn replies_carry_types_headers_and_cookies() {
        let j = Reply::json(&serde_json::json!({ "id": "42" }));
        assert_eq!(
            (j.status, j.headers[0].1.as_str(), j.body.as_slice()),
            (200, "application/json; charset=utf-8", &br#"{"id":"42"}"#[..])
        );
        let t = Reply::text("hi").header("Content-Type", "text/csv").header("x-total", "1").status(201);
        assert_eq!(t.headers, [("content-type".to_string(), "text/csv".to_string()), ("x-total".into(), "1".into())]);
        assert_eq!(t.status, 201);
        let r = Reply::redirect("/login").cookie("sid", "s1", &CookieOpts::default());
        assert_eq!(
            (r.status, r.headers.clone()),
            (302, vec![("location".into(), "/login".into()), ("set-cookie".into(), "sid=s1".into())])
        );
        assert_eq!(r.clone().status(303).status, 303);
        assert_eq!(Reply::text("x").cookie("bad name", "v", &CookieOpts::default()).status, 500);
    }

    fn tree() -> (tempfile::TempDir, Http) {
        let t = tempfile::tempdir().unwrap();
        let (app, shared) = (t.path().join("dist"), t.path().join("core/lib"));
        for d in [app.join("docs"), app.join(".well-known"), shared.clone()] {
            std::fs::create_dir_all(d).unwrap();
        }
        for (f, body) in [
            (app.join("index.html"), "<html>shell</html>"),
            (app.join("app.js"), "mount()"),
            (app.join("notes.txt"), "hello"),
            (app.join("docs/x.md"), "# x"),
            (app.join(".env"), "SECRET=1"),
            (app.join(".well-known/acme"), "token"),
            (shared.join("sse.js"), "parse()"),
            (shared.join(".secret"), "nope"),
            (t.path().join("outside.js"), "outside"),
        ] {
            std::fs::write(f, body).unwrap();
        }
        let http = Http::default().shell(&app).dir("/shared", &shared);
        (t, http)
    }

    fn served(http: &Http, path: &str) -> Option<String> {
        file_for(&http.roots, path).map(|f| std::fs::read_to_string(f).unwrap())
    }

    #[test]
    fn a_painted_page_has_its_view_in_main_and_its_head_and_nothing_else_moves() {
        let page = "<html><head><title>A</title></head><body><div id=\"main\"></div><script src=x></script></body></html>";
        let p = Paint { title: None, head: "<meta name=\"d\" content=\"1\">".into(), body: "<p>3 items</p>".into() };
        assert_eq!(
            painted(page, &p),
            "<html><head><title>A</title><meta name=\"d\" content=\"1\"></head><body><div id=\"main\"><p>3 \
             items</p></div><script src=x></script></body></html>"
        );
        // The view's title replaces the page's, as text.
        let t = Paint { title: Some("Post <1>".into()), ..p.clone() };
        assert!(painted(page, &t).starts_with("<html><head><title>Post &lt;1&gt;</title><meta"));
        // A page with no empty main, or no head, keeps what it has.
        assert_eq!(painted("<p>x</p>", &t), "<p>x</p>");
    }

    #[test]
    fn the_shell_and_dirs_serve_their_files_and_deep_links() {
        let (_t, http) = tree();
        assert_eq!(served(&http, "/app.js").as_deref(), Some("mount()"));
        assert_eq!(served(&http, "/shared/sse.js").as_deref(), Some("parse()"));
        assert_eq!(served(&http, "/.well-known/acme").as_deref(), Some("token"));
        // Route-shaped paths get the shell: no name, a folder, an unknown extension.
        for p in ["/", "/some/client/route", "/some/route/", "/docs", "/docs/", "/u/john.doe", "/blog/v1.2"] {
            assert_eq!(served(&http, p).as_deref(), Some("<html>shell</html>"), "{p}");
        }
        // A missing asset is a 404, not the shell; so is a missing mapped file.
        for p in ["/missing.js", "/logo.SVG", "/shared/nope.js", "/shared"] {
            assert_eq!(served(&http, p), None, "{p}");
        }
    }

    #[test]
    fn a_mapped_dir_is_not_a_weaker_root() {
        let (t, http) = tree();
        assert!(t.path().join("outside.js").is_file(), "what the escapes aim at is there");
        for p in [
            "/.env",
            "/shared/.secret",
            "/shared/../../outside.js",
            "/shared/%2e%2e/%2e%2e/outside.js",
            "/shared/..%2F..%2Foutside.js",
            "/shared/x%5C..%5C..%5Coutside.js",
            "/notes.txt/",
            "/app.js/",
            "/shared/sse.js/",
            "//app.js",
            "/shared//sse.js",
            "/C:%5Cx",
            "/docs/.x",
        ] {
            assert_eq!(served(&http, p), None, "{p}");
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(t.path().join("outside.js"), t.path().join("core/lib/link.js")).unwrap();
            assert_eq!(served(&http, "/shared/link.js"), None);
        }
    }

    #[test]
    fn a_root_that_is_not_a_directory_says_so_resolved() {
        let w = root_problem("dir \"/shared\"", Path::new("no-such-dir-xyz")).unwrap();
        assert!(w.contains(&std::path::absolute("no-such-dir-xyz").unwrap().display().to_string()), "{w}");
        assert!(root_problem("x", Path::new(".")).is_none());
        let http = Http::default().dir("/shared", "no-such-dir-xyz");
        assert_eq!(served(&http, "/shared/sse.js"), None);
    }

    #[test]
    fn files_go_out_with_their_type_utf8_only_when_it_is() {
        assert_eq!(served_type("a.CSS", b"x"), "text/css; charset=utf-8");
        assert_eq!(served_type("b.SVG", b"<svg/>"), "image/svg+xml; charset=utf-8");
        assert_eq!(served_type("t.txt", &[0xff, 0xfe]), "text/plain");
        assert_eq!(served_type("p.png", b"x"), "image/png");
        assert_eq!(served_type("blob", b"x"), "application/octet-stream");
    }
}

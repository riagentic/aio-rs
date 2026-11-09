//! Where a UI's cell lives: in this process, or synced from a server (the
//! `server` feature).
#[cfg(feature = "server")]
use crate::sync::Remote;
use crate::{By, Cell, Door, Handle, Result, Status};
use serde_json::Value;
use std::sync::Arc;

/// What client-server and server modes serve besides sync.
#[cfg(feature = "server")]
type Http = crate::http::Http;
#[cfg(not(feature = "server"))]
#[derive(Clone, Default)]
struct Http(());

pub enum Source<T: Cell> {
    Local(Handle<T>),
    #[cfg(feature = "server")]
    Remote(Remote<T>),
}

impl<T: Cell> Clone for Source<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Local(h) => Self::Local(h.clone()),
            #[cfg(feature = "server")]
            Self::Remote(r) => Self::Remote(r.clone()),
        }
    }
}


impl<T: Cell> Source<T> {
    /// The app's own store at `path`, its schedules running.
    pub fn open(path: &std::path::Path) -> Result<Self, crate::Error> {
        Self::run(Handle::open(path)?)
    }

    /// A ported app's store: aio app `app`'s `state.db` at `path`
    /// ([`crate::Store::open_aio`]), its schedules running.
    pub fn open_aio(path: &std::path::Path, app: &str) -> Result<Self, crate::Error> {
        Self::run(Handle::open_aio(path, app)?)
    }

    fn run(h: Handle<T>) -> Result<Self, crate::Error> {
        // Problem reports show the app's cells as they are. Kept apart from
        // the store's lock: a report taken mid-call (a panic) never waits on it.
        let latest = Arc::new(std::sync::Mutex::new(h.state()));
        let l = latest.clone();
        h.listen(move |s| *l.lock().unwrap_or_else(|e| e.into_inner()) = s.clone());
        crate::report::watch(T::ID, T::REDACT || !T::REDACTED.is_empty(), move || {
            crate::store::reported(&**latest.try_lock().ok()?)
        });
        let _in = crate::rt().enter();
        drop(h.start());
        crate::on_shutdown(h.clone());
        Ok(Self::Local(h))
    }

    /// A synced copy of the cell `addr` serves, as `user`.
    #[cfg(feature = "server")]
    pub fn connect(addr: &str, token: &str, user: &str) -> std::io::Result<Self> {
        crate::rt().block_on(Remote::connect(addr, token, user)).map(Self::Remote)
    }

    /// The state the UI shows.
    pub fn state(&self) -> Arc<T> {
        match self {
            Self::Local(h) => h.state(),
            #[cfg(feature = "server")]
            Self::Remote(r) => r.state(),
        }
    }

    /// How the copy stands now.
    pub fn status(&self) -> Status {
        match self {
            Self::Local(_) => Status::LOCAL,
            #[cfg(feature = "server")]
            Self::Remote(r) => r.status(),
        }
    }

    /// [`Handle::watch`], on a local cell or a synced copy.
    pub fn watch<R: PartialEq + Send + 'static>(
        &self,
        f: impl Fn(&T) -> R + Send + Sync + 'static,
        on: impl Fn(&R) + Send + Sync + 'static,
    ) {
        let l = Arc::new(crate::derived::changes(f, on));
        let first = l.clone();
        self.listen(move |st| l(st));
        first(&self.state());
    }

    /// [`Handle::on_change`], on a local cell or a synced copy.
    pub fn on_change<R, M>(
        &self,
        immediate: bool,
        f: impl Fn(&T) -> R + Send + Sync + 'static,
        on: impl Fn(&R, Option<&R>) -> M + Send + Sync + 'static,
    ) -> crate::Watching
    where
        R: PartialEq + Clone + Send + 'static,
        M: Send + 'static,
    {
        let (l, w) = crate::derived::reaction(&*self.state(), immediate, f, on);
        let l = Arc::new(l);
        let first = l.clone();
        self.listen(move |st| l(st));
        // A commit between the first look and listening isn't missed.
        first(&self.state());
        w
    }

    /// [`Handle::on_pending`]; a synced copy never calls it.
    pub fn on_pending(&self, f: impl Fn() + Send + Sync + 'static) {
        match self {
            Self::Local(h) => h.on_pending(f),
            #[cfg(feature = "server")]
            Self::Remote(_) => {}
        }
    }

    /// Calls of `method` in flight (`None`: all): a local cell's running or
    /// queued effects ([`Handle::pending`]); a synced copy's calls waiting
    /// for the server (it runs no effects, the server does).
    pub fn pending(&self, method: Option<&str>) -> usize {
        match self {
            Self::Local(h) => h.pending(method),
            #[cfg(feature = "server")]
            Self::Remote(r) => method.map_or_else(|| r.pending(), |m| r.pending_of(m)),
        }
    }

    /// [`Handle::travel`]; a synced copy's history is the server's
    /// (`ax timetravel` there).
    pub fn travel(&self, ask: Option<crate::TimeAsk>) -> Result<crate::Timeline> {
        match self {
            Self::Local(h) => h.travel(ask),
            #[cfg(feature = "server")]
            Self::Remote(_) => crate::reject!("{}: a synced copy travels on its server (ax timetravel)", T::ID),
        }
    }

    /// The views showed the state as it is now: a synced copy tells its
    /// server how far its screen lags ([`Remote::rendered`]).
    pub fn rendered(&self) {
        #[cfg(feature = "server")]
        if let Self::Remote(r) = self {
            r.rendered();
        }
    }

    pub fn listen(&self, f: impl Fn(&Arc<T>) + Send + Sync + 'static) {
        match self {
            Self::Local(h) => h.listen(f),
            #[cfg(feature = "server")]
            Self::Remote(r) => r.listen(f),
        }
    }

    /// Serve this (local) cell to synced clients on `listen`, in the
    /// background; the port it got. A taken port is an error now, not later.
    #[cfg(feature = "server")]
    pub fn serve(&self, listen: &str, opts: crate::sync::Options) -> std::io::Result<u16> {
        let Self::Local(h) = self else {
            return Err(std::io::Error::other("a synced copy can't serve"));
        };
        let l = crate::rt()
            .block_on(tokio::net::TcpListener::bind(listen))
            .map_err(|e| std::io::Error::new(e.kind(), format!("{listen}: {e}")))?;
        let port = l.local_addr()?.port();
        let h = h.clone();
        crate::rt().spawn(async move {
            if let Err(e) = crate::sync::serve_with(h, l, opts).await {
                eprintln!("[aio] sync server stopped: {e}");
            }
        });
        Ok(port)
    }

    /// The source `aio.toml` asks for: the store in `dirs.data` (a ported
    /// app's is its aio `state.db`; served too in client-server mode), or a
    /// synced copy in client mode. A client that draws views (built with
    /// the kit: a window, desktop or Android) never fails for want of a
    /// server: with none named, `--connect`, one that can't be reached (and
    /// no copy saved) or one that refuses, the copy waits and its window
    /// shows the connect page ([`crate::connect`]) until the person picks a
    /// server. A client without the kit (a CLI) fails, saying how to name one.
    pub fn for_app(cfg: &crate::config::Config, dirs: &crate::paths::AppDirs) -> std::io::Result<Self> {
        Self::boot::<true>(cfg, dirs, Http::default())
    }

    /// [`Self::for_app`] for a binary that is never a client (a server): it
    /// then holds no sync client (200 KB of the server budget), and client
    /// mode is refused.
    pub fn for_server(cfg: &crate::config::Config, dirs: &crate::paths::AppDirs) -> std::io::Result<Self> {
        Self::boot::<false>(cfg, dirs, Http::default())
    }

    /// [`Self::for_app`], serving the app's own routes and files too.
    #[cfg(feature = "server")]
    pub fn for_app_with(
        cfg: &crate::config::Config,
        dirs: &crate::paths::AppDirs,
        http: crate::http::Http,
    ) -> std::io::Result<Self> {
        Self::boot::<true>(cfg, dirs, http)
    }

    /// `CLIENT`: client mode is built in (a constant, so a server's binary
    /// leaves the client out).
    fn boot<const CLIENT: bool>(
        cfg: &crate::config::Config,
        dirs: &crate::paths::AppDirs,
        http: Http,
    ) -> std::io::Result<Self> {
        crate::lifecycle::harden();
        use crate::config::Mode;
        match cfg.app.mode {
            Mode::Client if CLIENT => Self::client(cfg, dirs, cfg!(feature = "kit")),
            Mode::Client => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                format!("{}: mode = \"client\" needs aio::Source::for_app, not for_server", T::ID),
            )),
            mode => {
                crate::store::ONLY_PERSIST.store(cfg.data.only_persist, std::sync::atomic::Ordering::Relaxed);
                crate::store::TRAVEL.store(cfg.history.travel, std::sync::atomic::Ordering::Relaxed);
                let args: Vec<String> = std::env::args().skip(1).collect();
                let asked = crate::paths::Ask::parse(&args).is_ok_and(|a| a.no_persist);
                if !cfg.data.persist || asked {
                    let why = if asked { "--no-persist" } else { "[data] persist = false" };
                    eprintln!("[aio] {}: {why}: the state lives in memory only, nothing is saved", T::ID);
                    let s = Self::run(Handle::memory()).map_err(|e| std::io::Error::other(e.0))?;
                    let door = http.clone();
                    if matches!(mode, Mode::Standalone | Mode::ClientServer) {
                        s.serve_window(cfg, dirs, &door);
                    }
                    if matches!(mode, Mode::ClientServer | Mode::Server) {
                        s.serve_app(cfg, dirs, http)?;
                    }
                    s.serve_door(dirs, &door);
                    return Ok(s);
                }
                if let Some(app) = &dirs.port_of {
                    #[cfg(feature = "compat")]
                    crate::compat::scatter::adopt_here(app, dirs);
                    #[cfg(not(feature = "compat"))]
                    if std::path::Path::new("data.db").exists() {
                        eprintln!("[aio] data: ./data.db may be {app}'s aio store from before its home: it is not moved in without aio-rs's feature `compat`");
                    }
                }
                for p in crate::compat::data::retire(&dirs.data, &cfg.data.retire)? {
                    eprintln!("[aio] retired {} (an old store, kept: delete it once nothing is missing)", p.display());
                }
                let s = match &dirs.port_of {
                    Some(app) => Self::open_aio(&dirs.store, app),
                    None => Self::open(&dirs.store),
                }
                .map_err(|e| std::io::Error::other(e.0))?;
                // Phones always save hard: the OS may kill the app at any moment.
                let durability = if cfg!(any(target_os = "android", target_os = "ios")) {
                    crate::config::Durability::Full
                } else {
                    cfg.data.durability
                };
                if let Some(h) = s.local() {
                    h.store(|st| st.set_durability(durability)).map_err(|e| std::io::Error::other(e.0))?;
                    h.store(|st| st.set_pragmas(&cfg.data.pragmas)).map_err(|e| std::io::Error::other(e.0))?;
                    h.store(|st| st.keep_history(cfg.history.keep)).map_err(|e| std::io::Error::other(e.0))?;
                    let coalesce = std::time::Duration::from_millis(cfg.data.coalesce_ms);
                    h.store(|st| st.set_coalesce(coalesce)).map_err(|e| std::io::Error::other(e.why))?;
                }
                if let Some(every) = cfg.data.snapshot_every {
                    crate::store::snapshot_every(&dirs.store, every);
                }
                if let Some(every) = cfg.data.backup_every {
                    let home = crate::paths::parent(&dirs.data).join("backups");
                    crate::backup::timed(&cfg.app.id, &dirs.store, &home, every, cfg.data.backup_keep);
                }
                let door = http.clone();
                if matches!(mode, Mode::Standalone | Mode::ClientServer) {
                    s.serve_window(cfg, dirs, &door);
                }
                if matches!(mode, Mode::ClientServer | Mode::Server) {
                    s.serve_app(cfg, dirs, http)?;
                }
                // `[server] local_peers = false`: a release build opens no door.
                if cfg.server.local_peers || crate::door::OPERATOR {
                    s.serve_door(dirs, &door);
                }
                Ok(s)
            }
        }
    }

    /// Client mode: the launch's `--server-url`, else a kept link (`ax
    /// pair`, the connect page), else `[client] server` + the token; its pin
    /// checked. `window`: a client that draws views, so shows the connect page.
    #[cfg(feature = "server")]
    fn client(cfg: &crate::config::Config, dirs: &crate::paths::AppDirs, window: bool) -> std::io::Result<Self> {
        let user = std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_else(|_| "user".into());
        let args: Vec<String> = std::env::args().skip(1).collect();
        let ask = crate::paths::Ask::parse(&args).map_err(std::io::Error::other)?;
        let transport = match ask.transport.as_deref() {
            None => cfg.client.transport,
            Some(t) => serde_json::from_value(Value::from(t)).map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("--transport={t}: auto, ws or poll"))
            })?,
        };
        crate::sync::set_transport(transport);
        let kept = crate::connect::saved(&dirs.data);
        let named = match (&ask.server_url, kept, &cfg.client.server) {
            (Some(u), kept, _) => {
                crate::connect::key_in_argv(u).into_iter().for_each(|w| eprintln!("{w}"));
                Some(
                    crate::connect::url_link(u, kept.as_ref(), &crate::config::token(&dirs.data)?)
                        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?,
                )
            }
            (None, Some(l), _) => Some(l),
            (None, None, Some(addr)) => {
                Some(crate::sync::Link { addr: addr.clone(), token: crate::config::token(&dirs.data)?, root: None })
            }
            (None, None, None) => None,
        };
        // The server the person named: reached whatever `[capabilities] network` says.
        named.iter().for_each(|l| crate::capabilities::grant(&l.addr));
        #[cfg(feature = "tls")]
        let named = match named {
            Some(l) => Some(crate::rt().block_on(crate::connect::pinned(l, cfg.client.strict_pin || ask.strict_pin, &dirs.data))),
            None => None,
        };
        #[cfg(not(feature = "tls"))]
        let named = named.map(std::io::Result::Ok);
        let copy = dirs.data.join("copy.json");
        if !window {
            let link = named.ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("{} isn't paired: run `ax pair {} <host:port> <code>` or set [client] server", T::ID, cfg.app.id),
                )
            })??;
            return Self::linked(cfg, crate::rt().block_on(Remote::connect_saved(link, &user, &copy))?);
        }
        let had_copy = copy.exists();
        let (link, why) = match named {
            _ if ask.connect => (None, Some("Pick the server (--connect).".to_string())),
            None => (None, Some("This app has no server yet.".to_string())),
            Some(Err(e)) => (None, Some(e.to_string())),
            Some(Ok(l)) => (Some(l), None),
        };
        let at = link.as_ref().map(|l| l.addr.clone());
        let (r, failed) = crate::rt().block_on(Remote::connect_later(link, &user, &copy))?;
        // The page shows while there is nothing to show: never reached this
        // run with no copy saved, or refused (a key, another app's server).
        let synced = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(r.online()));
        let said = |e: &std::io::Error, at: &str| match e.kind() {
            std::io::ErrorKind::PermissionDenied => format!("{at} refused: {e}. Pair with it by its code."),
            _ => format!("Can't reach {at}: {e}"),
        };
        let refused = |e: &std::io::Error| {
            matches!(e.kind(), std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::Unsupported)
        };
        let why = why.or_else(|| failed.filter(|e| refused(e) || !had_copy).map(|e| said(&e, at.as_deref().unwrap_or("?"))));
        if let Some(w) = why {
            eprintln!("[aio] {}: the connect page shows: {w}", T::ID);
            crate::connect::ask(w);
        }
        let s = synced.clone();
        r.on_sync(move |_| {
            s.store(true, std::sync::atomic::Ordering::Relaxed);
            crate::connect::reached();
        });
        r.on_unreached(move |e| {
            if refused(e) || !synced.load(std::sync::atomic::Ordering::Relaxed) {
                crate::connect::ask(said(e, "the server"));
            }
        });
        let to = r.clone();
        crate::connect::wire(move |l| to.relink(l));
        Self::linked(cfg, r)
    }

    /// The copy as `[client]` tunes it.
    #[cfg(feature = "server")]
    fn linked(cfg: &crate::config::Config, r: Remote<T>) -> std::io::Result<Self> {
        r.retain(cfg.client.retention);
        let mine = cfg.client.timeouts.iter().filter_map(|(k, w)| Some((k.strip_prefix(T::ID)?.strip_prefix('.')?, w)));
        mine.for_each(|(method, w)| r.timeout(method, *w));
        Ok(Self::Remote(r))
    }

    #[cfg(not(feature = "server"))]
    fn client(_: &crate::config::Config, _: &crate::paths::AppDirs, _: bool) -> std::io::Result<Self> {
        Err(crate::left_out("server", "client mode"))
    }

    /// Client-server and server modes: serve the store as `[server]` says.
    #[cfg(feature = "server")]
    fn serve_app(&self, cfg: &crate::config::Config, dirs: &crate::paths::AppDirs, http: Http) -> std::io::Result<()> {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let ask = crate::paths::Ask::parse(&args).map_err(std::io::Error::other)?;
        let decided = crate::config::listen(&cfg.server, &ask, &|k| std::env::var_os(k).map(Into::into)).map_err(std::io::Error::other)?;
        let listen = decided.addr.as_str();
        #[cfg(feature = "tls")]
        let tls = {
            let flag = |p: &Option<String>| p.as_deref().map(std::path::absolute).transpose();
            let (cert, key) = match (flag(&ask.tls_cert)?, flag(&ask.tls_key)?) {
                (Some(c), Some(k)) => (Some(c), Some(k)),
                _ => (cfg.server.cert.clone(), cfg.server.key.clone()),
            };
            match (decided.tls, &cert, &key) {
            (false, ..) => None,
            (true, Some(cert), Some(key)) => Some(crate::tls::own(cert, key)?),
                (true, ..) => Some(crate::tls::server(&crate::paths::tool_home().join("tls"), T::ID)?),
            }
        };
        #[cfg(not(feature = "tls"))]
        for (asked, what) in [
            (decided.tls, "[server] tls"),
            (cfg.server.pairing, "[server] pairing"),
            (cfg.server.discover, "[server] discover"),
        ] {
            if asked {
                return Err(crate::left_out("tls", what));
            }
        }
        let mut http = http.configured(&cfg.server, &dirs.files()).assets(cfg.build.asset_files);
        #[cfg(feature = "updates")]
        {
            http = crate::update::beside(http);
        }
        if let Some(why) = cfg.accounts.unverifiable(http.mail.is_some()) {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, why));
        }
        let auth_db = || crate::auth::Auth::open(dirs.data.join("auth.db"), cfg.accounts.policy()).map_err(std::io::Error::other);
        // Sessions without accounts: a session is a user known by its token.
        if cfg.accounts.sessions && !cfg.accounts.on {
            let a = std::sync::Arc::new(auth_db()?);
            crate::auth::serve(&a);
            http.users = Some(crate::auth::Users::sessions(a, http.users.take()));
        }
        let opts = crate::sync::Options {
            limits: cfg.server.limits,
            token: crate::config::token(&dirs.data)?,
            #[cfg(feature = "tls")]
            pairing: cfg.server.pairing.then(crate::pairing::pairing),
            #[cfg(feature = "tls")]
            app: cfg.app.id.clone(),
            #[cfg(feature = "tls")]
            tls,
            hosts: cfg.server.hosts.clone(),
            http,
            auth: match cfg.accounts.on {
                true => Some(std::sync::Arc::new(auth_db()?)),
                false => None,
            },
            strict_origin: cfg.server.strict_origin,
            trust_proxy: cfg.server.trust_proxy.clone(),
            security: cfg.server.security.clone(),
            open: cfg.server.open,
            page_log: cfg.server.page_log,
        };
        let opts_users = opts.http.users.is_some();
        let host = listen.rsplit_once(':').map(|(h, _)| h.trim_matches(['[', ']']).to_string());
        #[cfg(feature = "tls")]
        let tls = opts.tls.is_some();
        #[cfg(not(feature = "tls"))]
        let tls = false;
        let serving = crate::sync::Serving { port: 0, tls, host };
        let port = self.serve(listen, opts)?;
        let serving =
            serde_json::to_string(&crate::sync::Serving { port, ..serving }).map_err(std::io::Error::other)?;
        crate::paths::write_atomic(&dirs.data.join("serving.json"), serving.as_bytes(), false)?;
        let bound = format!("{}:{port}", listen.rsplit_once(':').map_or(listen, |(h, _)| h));
        let bound = crate::config::Listen { addr: bound, ..decided.clone() };
        eprintln!("[aio] {}", crate::config::listen_says(&bound, T::ID));
        if cfg.server.open {
            eprintln!("[aio] WARN {}", crate::config::open_says(T::ID));
        }
        if bound.exposed {
            let gate = match () {
                _ if cfg.accounts.on => crate::config::Gate::Accounts,
                _ if cfg.accounts.sessions => crate::config::Gate::Sessions,
                _ if opts_users => crate::config::Gate::Users,
                _ if cfg.server.open => crate::config::Gate::Open,
                _ => crate::config::Gate::Key,
            };
            let lan = crate::config::lan_ipv4();
            for line in crate::config::share_says(&bound, lan, gate, &dirs.data.join("sync.token"), cfg.server.pairing) {
                eprintln!("[aio] {line}");
            }
        }
        if let Some(line) = crate::config::firewall_says(std::env::consts::OS, bound.exposed) {
            eprintln!("[aio] {line}");
        }
        #[cfg(feature = "tls")]
        if cfg.server.discover {
            let me = crate::discovery::Found {
                name: cfg.app.id.clone(),
                port,
                title: Some(cfg.app.name.clone()),
                needs_auth: !cfg.server.open,
                tls: bound.tls,
                host: None,
            };
            let udp = crate::discovery::port();
            crate::rt().spawn(async move {
                if let Err(e) = crate::discovery::respond(udp, vec![me]).await {
                    eprintln!("[aio] discovery on UDP {udp} stopped: {e}");
                }
            });
        }
        Ok(())
    }

    /// Open the cell's local door ([`crate::door`]), so this user's tools
    /// reach the running app (`ax state`, `ax call`) and other apps it as
    /// a peer (`aio::peer`, `http`'s server functions too). A door that
    /// can't open is said, never fatal: the app works without it.
    fn serve_door(&self, dirs: &crate::paths::AppDirs, http: &Http) {
        let Some(h) = self.local().cloned() else { return };
        let _in = crate::rt().enter();
        let addr = crate::door::at(&dirs.id, &dirs.data, T::ID);
        let l = addr.as_ref().map_err(|e| std::io::Error::new(e.kind(), e.to_string())).and_then(|a| crate::ipc::Listener::bind(a));
        // aio's tools see the app too (`am instances`, `am state`).
        #[cfg(feature = "compat")]
        if let (Ok(a), Ok(_)) = (&addr, &l) {
            crate::compat::local::serve(&dirs.id, &dirs.data, dirs.profile.as_deref(), T::ID, a.clone());
        }
        #[cfg(feature = "server")]
        let (calls, hooks) =
            (crate::fns::Served { fns: std::sync::Arc::new(http.fns.clone()), ..crate::link::no_calls() }, http.door_hooks());
        #[cfg(not(feature = "server"))]
        let (calls, hooks, _) = (crate::link::no_calls(), crate::link::Hooks::default(), http);
        match l {
            Ok(l) => drop(crate::rt().spawn(async move {
                let r = crate::door::serve_with(h, l, Default::default(), calls, crate::door::OPERATOR, hooks).await;
                if let Err(e) = r {
                    eprintln!("[aio] {}: the local door stopped: {e}", T::ID);
                }
            })),
            Err(e) => eprintln!("[aio] {}: no local door for tools: {e}", T::ID),
        }
    }

    /// The app's routes and files to its window, over a private socket
    /// (`aio://app/<path>`, [`crate::http::serve_window`]): no TCP port, and
    /// only the window aio-shell spawns is answered. An app that serves no
    /// path opens none; a door that can't open is said, never fatal.
    #[cfg(feature = "server")]
    fn serve_window(&self, cfg: &crate::config::Config, dirs: &crate::paths::AppDirs, http: &Http) {
        let http = http.clone().configured(&cfg.server, &dirs.files());
        if self.local().is_none() || !http.serves() {
            return;
        }
        let _in = crate::rt().enter();
        let addr = crate::paths::sockets(&dirs.id, &dirs.data).map(|d| crate::ipc::addr(&d, "http"));
        // Electron's Node connects to it as spelled.
        let l = addr.and_then(|a| crate::ipc::fits(&a).and_then(|()| crate::ipc::Listener::bind(&a)).map(|l| (a, l)));
        match l {
            Ok((addr, l)) => {
                let n = http.route_count();
                eprintln!(
                    "[aio] {}: {n} route(s) served to the window over its private socket (aio://app/<path>): no TCP \
                     port; another process reaches them only with [server] listen",
                    T::ID
                );
                crate::ipc::set_window_http(addr);
                drop(crate::rt().spawn(async move {
                    if let Err(e) = crate::http::serve_window(http, l).await {
                        eprintln!("[aio] {}: the window's route door stopped: {e}", T::ID);
                    }
                }));
            }
            Err(e) => eprintln!("[aio] {}: the window gets no routes (aio://app/<path>): {e}", T::ID),
        }
    }

    #[cfg(not(feature = "server"))]
    fn serve_window(&self, _: &crate::config::Config, _: &crate::paths::AppDirs, _: &Http) {}

    #[cfg(not(feature = "server"))]
    fn serve_app(&self, _: &crate::config::Config, _: &crate::paths::AppDirs, _: Http) -> std::io::Result<()> {
        Err(crate::left_out("server", "serving (client-server and server modes)"))
    }

    /// The cell's own handle, when it lives in this process.
    fn local(&self) -> Option<&Handle<T>> {
        match self {
            Self::Local(h) => Some(h),
            #[cfg(feature = "server")]
            Self::Remote(_) => None,
        }
    }

    /// Who calls through this source.
    pub fn by(&self, by: By) -> Self {
        match self {
            Self::Local(h) => Self::Local(h.by(by)),
            #[cfg(feature = "server")]
            Self::Remote(r) => Self::Remote(r.clone()),
        }
    }
}

impl<T: Cell> Door<T> for Source<T> {
    fn call(&self, method: &str, args: Value) -> Result<Value> {
        match self {
            Self::Local(h) => h.call(method, args),
            #[cfg(feature = "server")]
            Self::Remote(r) => r.call(method, args),
        }
    }

    fn ask(&self, method: &str, args: Value) -> Result<crate::Answer> {
        match self {
            Self::Local(h) => h.ask(method, args),
            #[cfg(feature = "server")]
            Self::Remote(r) => r.ask(method, args),
        }
    }

    fn read<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        f(&self.state())
    }

    fn own(&self, key: &str, res: Option<Box<dyn std::any::Any + Send>>) {
        match self {
            Self::Local(h) => h.own(key, res),
            #[cfg(feature = "server")]
            Self::Remote(r) => r.own(key, res),
        }
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use crate::prelude::*;

    #[cell]
    pub struct Pad {
        n: u32,
    }

    #[methods]
    impl Pad {
        pub fn bump(&mut self) {
            self.n += 1;
        }
    }

    #[test]
    fn a_client_that_draws_no_window_fails_saying_how_to_name_its_server() {
        let d = tempfile::tempdir().unwrap();
        let cfg = crate::config::Config::parse("[app]\nid = \"pad\"\nmode = \"client\"").unwrap();
        let dirs = crate::paths::AppDirs::at(d.path(), "pad").unwrap();
        let e = crate::Source::<Pad>::client(&cfg, &dirs, false).err().unwrap();
        assert!(e.to_string().contains("isn't paired: run `ax pair pad"), "{e}");
        assert!(!crate::connect::active(), "no page to show");
    }
}

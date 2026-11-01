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
        crate::report::watch(T::ID, !T::REDACTED.is_empty(), move || {
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
    /// synced copy in client mode.
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
            Mode::Client if CLIENT => Self::client(cfg, dirs),
            Mode::Client => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                format!("{}: mode = \"client\" needs aio::Source::for_app, not for_server", T::ID),
            )),
            mode => {
                let args: Vec<String> = std::env::args().skip(1).collect();
                let asked = crate::paths::Ask::parse(&args).is_ok_and(|a| a.no_persist);
                if !cfg.data.persist || asked {
                    let why = if asked { "--no-persist" } else { "[data] persist = false" };
                    eprintln!("[aio] {}: {why}: the state lives in memory only, nothing is saved", T::ID);
                    let s = Self::run(Handle::memory()).map_err(|e| std::io::Error::other(e.0))?;
                    let door = http.clone();
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
                if let Some(every) = cfg.data.backup_every {
                    let home = crate::paths::parent(&dirs.data).join("backups");
                    crate::backup::timed(&cfg.app.id, &dirs.store, &home, every, cfg.data.backup_keep);
                }
                let door = http.clone();
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

    /// Client mode: a paired link (`ax pair`) wins over `[client] server` +
    /// the token.
    #[cfg(feature = "server")]
    fn client(cfg: &crate::config::Config, dirs: &crate::paths::AppDirs) -> std::io::Result<Self> {
        let user = std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_else(|_| "user".into());
        #[cfg(feature = "tls")]
        let paired = crate::pairing::saved(&dirs.data);
        #[cfg(not(feature = "tls"))]
        let paired = None;
        let link = match paired {
            Some(l) => l,
            None => {
                let addr = cfg.client.server.clone().ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!(
                            "{} isn't paired: run `ax pair {} <host:port> <code>` or set [client] server",
                            T::ID,
                            cfg.app.id
                        ),
                    )
                })?;
                crate::sync::Link { addr, token: crate::config::token(&dirs.data)?, root: None }
            }
        };
        let r = crate::rt().block_on(Remote::connect_saved(link, &user, &dirs.data.join("copy.json")))?;
        r.retain(cfg.client.retention);
        let mine = cfg.client.timeouts.iter().filter_map(|(k, w)| Some((k.strip_prefix(T::ID)?.strip_prefix('.')?, w)));
        mine.for_each(|(method, w)| r.timeout(method, *w));
        Ok(Self::Remote(r))
    }

    #[cfg(not(feature = "server"))]
    fn client(_: &crate::config::Config, _: &crate::paths::AppDirs) -> std::io::Result<Self> {
        Err(crate::left_out("server", "client mode"))
    }

    /// Client-server and server modes: serve the store as `[server]` says.
    #[cfg(feature = "server")]
    fn serve_app(&self, cfg: &crate::config::Config, dirs: &crate::paths::AppDirs, http: Http) -> std::io::Result<()> {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let ask = crate::paths::Ask::parse(&args).map_err(std::io::Error::other)?;
        let decided = crate::config::listen(&cfg.server, &ask).map_err(std::io::Error::other)?;
        let listen = decided.addr.as_str();
        #[cfg(feature = "tls")]
        let tls = match (decided.tls, &cfg.server.cert, &cfg.server.key) {
            (false, ..) => None,
            (true, Some(cert), Some(key)) => Some(crate::tls::own(cert, key)?),
            (true, ..) => Some(crate::tls::server(&crate::paths::tool_home().join("tls"), T::ID)?),
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
        let opts = crate::sync::Options {
            limits: cfg.server.limits,
            token: crate::config::token(&dirs.data)?,
            #[cfg(feature = "tls")]
            pairing: cfg.server.pairing.then(crate::pairing::pairing),
            #[cfg(feature = "tls")]
            tls,
            hosts: cfg.server.hosts.clone(),
            http: http.configured(&cfg.server, &dirs.files()),
            auth: match cfg.accounts.on {
                true => Some(std::sync::Arc::new(
                    crate::auth::Auth::open(dirs.data.join("auth.db"), cfg.accounts.policy())
                        .map_err(std::io::Error::other)?,
                )),
                false => None,
            },
            strict_origin: cfg.server.strict_origin,
            trust_proxy: cfg.server.trust_proxy.clone(),
            security: cfg.server.security.clone(),
            open: cfg.server.open,
        };
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
        #[cfg(feature = "tls")]
        if cfg.server.discover {
            let me = crate::discovery::Found {
                name: T::ID.into(),
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
        let l = crate::door::at(&dirs.id, &dirs.data, T::ID).and_then(|a| crate::ipc::Listener::bind(&a));
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

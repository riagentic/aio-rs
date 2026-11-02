//! `aio.toml`: an app's one config file. Every key has a visible default;
//! an unknown key is an error (a typo never silently does nothing).
//! Secrets (the sync token) never live here: see `token`.
use aio_macros::Lit;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    /// Cells in the app's own process; nothing listens.
    #[default]
    Standalone,
    /// A window on cells another process serves (`[client] server`).
    Client,
    /// Cells here, served to clients (`[server] listen`), with a window.
    ClientServer,
    /// Cells here, served, no window.
    Server,
    Cli,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(deny_unknown_fields)]
pub struct App {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub mode: Mode,
    /// Who publishes it, `Name <email>`: system packages need it (`--deb`).
    #[serde(default)]
    pub maintainer: Option<String>,
    /// Ported from aio app `port_of` (its aio `appId`): it runs on that app's
    /// aio home (`~/.<port_of>`, its profiles, `AIO_APPS_DIR`) and `state.db`, which
    /// aio can still open after (docs/migration/data.md).
    #[serde(default)]
    pub port_of: Option<String>,
    /// `major.minor` (aio numbers builds from git: `1.2.<commit count>`)
    /// or a pinned `major.minor.patch`, perhaps with a stage (`1.2-beta`);
    /// none: the package's (`Cargo.toml`), as written.
    #[serde(default)]
    pub version: Option<String>,
    /// Where a client-server app shows its UI: `window` (the runtime's) or
    /// `browser` (aio's `client: "browser"`): its own page in the system
    /// browser (`aio_shell::web`), running until asked to stop.
    #[serde(default)]
    pub ui: Shown,
    /// The page's language, `<html lang>` (screen readers, spellcheck and
    /// translation read it, WCAG 3.1.1); unset: `en`.
    #[serde(default)]
    pub lang: Option<String>,
    /// The page's direction, `<html dir>`. Never guessed from `lang`.
    #[serde(default)]
    pub dir: Dir,
    /// The clock cron schedules read (`#[cron]`, `cx.cron`): `utc`, as aio's
    /// did, so a port's jobs fire when they did; `local`: this machine's time zone.
    #[serde(default)]
    pub tz: Tz,
    /// The window's own icon, `assets/icon.png` beside `aio.toml`, put in
    /// by `aio::config!()`; empty: none (the desktop shell then draws the
    /// monogram, `aio::icon`, in the app's hue).
    #[serde(skip)]
    pub icon: &'static [u8],
}

impl App {
    /// The name the monogram draws: the app's, else its id.
    pub fn label(&self) -> &str {
        if self.name.is_empty() { &self.id } else { &self.name }
    }
}

/// `[app] tz`: the clock of cron schedules.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(rename_all = "lowercase")]
pub enum Tz {
    #[default]
    Utc,
    /// This machine's local time (its DST changes too).
    Local,
}

/// `[app] dir`: which way the page's text runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(rename_all = "lowercase")]
pub enum Dir {
    #[default]
    Ltr,
    Rtl,
    /// The browser's guess from the first strong character.
    Auto,
}

/// `[app] ui`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(rename_all = "lowercase")]
pub enum Shown {
    #[default]
    Window,
    Browser,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(deny_unknown_fields)]
pub struct History {
    /// How long every step is kept before folding into a snapshot.
    #[serde(default = "History::default_keep", with = "span")]
    pub keep: Duration,
}

impl History {
    fn default_keep() -> Duration {
        Duration::from_secs(90 * 86_400)
    }
}

impl Default for History {
    fn default() -> Self {
        Self { keep: Self::default_keep() }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(deny_unknown_fields)]
pub struct Server {
    /// `host:port` to serve synced cells on. A clash stops startup.
    pub listen: Option<String>,
    /// Answer LAN discovery probes (UDP 8099, aio's format).
    #[serde(default)]
    pub discover: bool,
    /// Let new devices pair with a PIN (`POST /__aio/pair`).
    #[serde(default)]
    pub pairing: bool,
    /// Serve over TLS, with this machine's root (`aio::tls`).
    #[serde(default)]
    pub tls: bool,
    /// With `tls`: the operator's own certificate chain and key (PEM files,
    /// absolute paths: aio.toml is built into the app) instead of aio's
    /// local root, e.g. one a public CA signed.
    pub cert: Option<std::path::PathBuf>,
    pub key: Option<std::path::PathBuf>,
    /// The security headers ([`crate::security`]).
    #[serde(default)]
    pub security: crate::security::Security,
    /// Names it is reached as besides IPs, `localhost` and `.local` (a
    /// reverse proxy's, a custom domain); any other Host is refused.
    #[serde(default)]
    pub hosts: Vec<String>,
    /// Served beyond loopback: a socket upgrade must say its Origin.
    #[serde(default)]
    pub strict_origin: bool,
    /// Behind a reverse proxy: the header it overwrites with the client's
    /// address (`x-forwarded-for`), whose last hop is then the client.
    #[serde(default)]
    pub trust_proxy: Option<String>,
    /// Each link's limits (`[server.limits]`).
    #[serde(default)]
    pub limits: Limits,
    /// Extra read-only dirs by path prefix (aio's `assets`), behind the key:
    /// `[server.dirs]` `"/media" = "media"`. A relative dir is in the app's
    /// files tier (`<data>/files`: never its key or store), an absolute one
    /// the operator's.
    #[serde(default)]
    pub dirs: std::collections::BTreeMap<String, std::path::PathBuf>,
    /// The same, for anyone without the key (aio's `publicDir`): only what a
    /// page shown before sign-in needs.
    #[serde(default)]
    pub public_dirs: std::collections::BTreeMap<String, std::path::PathBuf>,
    /// Open on purpose (aio's `key: false`): links and routes need no key, so
    /// anyone who reaches the port reads the state and calls methods. Said
    /// loudly at boot and by `ax doctor`; minting a PIN keeps the key. Not
    /// with `[accounts]`, whose sessions are the gate then.
    #[serde(default)]
    pub open: bool,
    /// Other processes of this user reach a release build's cells through
    /// its local door (`ax state`, `ax call`, other apps as peers), with a
    /// client's rights (aio's `allowLocalPeers`, which aio leaves off).
    /// `false`: a release build opens no door; a debug build always does.
    #[serde(default = "yes")]
    pub local_peers: bool,
}

impl Default for Server {
    fn default() -> Self {
        Self {
            listen: None,
            discover: false,
            pairing: false,
            tls: false,
            cert: None,
            key: None,
            security: Default::default(),
            hosts: vec![],
            strict_origin: false,
            trust_proxy: None,
            limits: Default::default(),
            dirs: Default::default(),
            public_dirs: Default::default(),
            open: false,
            local_peers: true,
        }
    }
}

/// The framework's own paths: `/ws` and `/__aio…`, never an app's.
pub fn reserved(path: &str) -> bool {
    path == "/ws" || path.starts_with("/__aio")
}

/// Why `[server] dirs` / `public_dirs` can't be served as written, or `None`.
fn dirs_problem(s: &Server) -> Option<String> {
    use std::path::Component;
    let all = || s.dirs.iter().map(|d| ("dirs", d)).chain(s.public_dirs.iter().map(|d| ("public_dirs", d)));
    for (k, (prefix, dir)) in all() {
        let p = prefix.trim_end_matches('/');
        if !p.starts_with('/')
            || reserved(p)
            || p.split('/').skip(1).any(|s| s.is_empty() || (s.starts_with('.') && s != ".well-known"))
        {
            return Some(format!(
                "[server.{k}] {prefix:?} is not a prefix to serve at: a path like \"/media\" (not /, /ws or /__aio…)"
            ));
        }
        if dir.as_os_str().is_empty() || dir.components().any(|c| matches!(c, Component::ParentDir)) {
            return Some(format!(
                "[server.{k}] {prefix:?}: {:?} is a folder in the files tier or an absolute one, without ..",
                dir.display()
            ));
        }
    }
    let mut seen: Vec<&str> = all().map(|(_, (p, _))| p.trim_end_matches('/')).collect();
    seen.sort();
    seen.windows(2).find(|w| w[0] == w[1]).map(|w| format!("[server] {:?} is served twice", w[0]))
}

/// A server's limits. Each one tells the sender: a refused client gets
/// 503 with the reason, a too-big message a close (1009) naming the limit;
/// a fast sender is read more slowly, so nothing is ever dropped. Clients
/// learn them on connect, check sizes before sending and pace themselves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    /// Open links at most.
    pub clients: usize,
    /// Largest message a client may send, in bytes.
    pub max_message: usize,
    /// Calls a link may make per second (bursts up to twice that).
    pub calls_per_sec: u32,
    /// Bytes a link may send per second (bursts up to twice that).
    pub bytes_per_sec: u32,
    /// A link is sent at most one change per this many ms (aio's
    /// `syncIntervalMs`): the first of a burst at once, the rest folded
    /// into one; a person's call goes at once. `0`: each change at once.
    pub sync_interval_ms: u64,
    /// A change whose patch is more than this percent of the whole view is
    /// sent as the view (aio's `fullStateThreshold`, as a percent). `0`:
    /// always the view.
    pub full_state_percent: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            clients: 256,
            max_message: 4 << 20,
            calls_per_sec: 200,
            bytes_per_sec: 8 << 20,
            sync_interval_ms: 50,
            full_state_percent: 50,
        }
    }
}

/// Accounts (`aio::auth`): served clients sign in as users, in `data/auth.db`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(deny_unknown_fields)]
pub struct Accounts {
    /// On: the shared key no longer opens sync; a session does, and the
    /// user is the session's.
    #[serde(default)]
    pub on: bool,
    /// Sessions without accounts (aio's `sessions: true`): no users, no
    /// login doors; the app issues sessions itself (`auth::serving()`'s
    /// `issue`, say from a server fn that checked its own credential), and
    /// a session token is that user on every door until it expires or is
    /// revoked. `on` includes it.
    #[serde(default)]
    pub sessions: bool,
    /// Anyone may make an account (`POST /__aio/auth/signup`).
    #[serde(default)]
    pub signup: bool,
    /// How long a session lives unused (used past half, it is renewed).
    #[serde(default = "thirty_days", with = "span")]
    pub session: Duration,
    /// The most live sessions per user (default 10); a new one past it
    /// ends the oldest.
    #[serde(default = "ten")]
    pub max_sessions: Option<u32>,
    /// No sign-in until the account's email is verified (needs the app's
    /// mail hook, `Http::mail`, or the operator's `ax auth verify`).
    #[serde(default)]
    pub require_verified: bool,
}

fn ten() -> Option<u32> {
    Some(crate::auth::MAX_SESSIONS)
}

fn thirty_days() -> Duration {
    Duration::from_secs(30 * 86_400)
}

impl Default for Accounts {
    fn default() -> Self {
        Self {
            on: false,
            sessions: false,
            signup: false,
            session: thirty_days(),
            max_sessions: ten(),
            require_verified: false,
        }
    }
}

impl Accounts {
    /// The policy `aio::auth` runs.
    pub fn policy(&self) -> crate::auth::Policy {
        let ttl_ms = i64::try_from(self.session.as_millis()).unwrap_or(i64::MAX);
        crate::auth::Policy {
            ttl_ms,
            max_sessions: self.max_sessions,
            signup: self.signup,
            require_verified: self.require_verified,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(deny_unknown_fields)]
pub struct Client {
    /// `host:port` of the app's server.
    pub server: Option<String>,
    /// How long a call waiting for the server is kept once the queue is full
    /// (`"7d"`): then a new call drops the oldest past it, and says so. Unset
    /// or `"off"`: nothing is dropped; a full queue refuses new calls.
    #[serde(default, with = "span_or_off")]
    pub retention: Option<Duration>,
    /// Per method, `"cell.method" = "60s"` (or `"off"`: as long as it takes):
    /// how long a confirmed call waits for the server's answer (else 15s).
    #[serde(default, with = "spans_or_off")]
    pub timeouts: std::collections::BTreeMap<String, Option<Duration>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    /// Hosts aio's outbound requests may reach besides this machine;
    /// everything else is refused ([`crate::capabilities`]). The hosts this
    /// file names (`[updates] url`, `[feedback] url`, `[client] server`) must
    /// be in it: a parse error says which.
    #[serde(default)]
    pub network: Vec<String>,
}

impl Capabilities {
    /// The first host `c` names that `network` leaves out, with the fix.
    fn missing(&self, c: &Config) -> Option<String> {
        if let Some(e) = self.network.iter().find_map(|e| crate::capabilities::bad_entry(e)) {
            return Some(e);
        }
        let mut named: Vec<(&str, String)> = vec![];
        named.extend(c.client.server.iter().map(|s| ("[client] server", s.clone())));
        named.extend(c.feedback.url.iter().map(|u| ("[feedback] url", u.clone())));
        match c.updates.url.as_deref() {
            Some(u) if u.starts_with("github:") => {
                // GitHub serves a release's files from its download hosts.
                let hosts = ["github.com", "release-assets.githubusercontent.com", "objects.githubusercontent.com"];
                named.extend(hosts.map(|h| ("[updates] url", h.to_string())));
            }
            Some(u) if u.starts_with("git+file://") => {}
            Some(u) => named.push(("[updates] url", crate::update::git_remote(u).unwrap_or(u).to_string())),
            None => {}
        }
        named.into_iter().find_map(|(what, target)| {
            let (host, port) = crate::capabilities::host_port(&target);
            let wanted = if host.ends_with(".githubusercontent.com") { "*.githubusercontent.com" } else { &host };
            (!crate::capabilities::allows(&self.network, &host, port))
                .then(|| format!("{what} reaches {host}: add \"{wanted}\" to [capabilities] network"))
        })
    }
}

/// Signed updates. Off without `url`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(deny_unknown_fields)]
pub struct Updates {
    /// Where releases are: `<url>/<channel>/<os>-<arch>.json`. https only
    /// (a loopback `http://127.0.0.1` for tests), or `github:<owner>/<repo>`:
    /// its GitHub releases (`aio::update::release_dir`), or a repository
    /// (`git+https://`, `git+ssh://`, `git+file://`) whose `channel` branch
    /// or tag is built here when its commit or tag is signed by one of `keys`
    /// (`aio::update::git_remote`).
    #[serde(default)]
    pub url: Option<String>,
    /// The release keys this app trusts (`ed25519:<base64>`, from `ax keygen`);
    /// a git source's may be OpenSSH lines too (`ssh-ed25519 <base64>`).
    #[serde(default)]
    pub keys: Vec<String>,
    #[serde(default = "stable")]
    pub channel: String,
    /// How often a running app looks for a release; `"off"`: only when the
    /// app asks (the `updates` cell's `check`).
    #[serde(default = "six_hours", with = "span_or_off")]
    pub every: Option<Duration>,
    /// `every` for some channels (`{ dev = "1m", beta = "5m" }`): where you
    /// ship into it now, a check a minute; where users are, a few a day.
    #[serde(default, with = "spans_or_off")]
    pub every_by_channel: std::collections::BTreeMap<String, Option<Duration>>,
    /// `true`: a release is downloaded in the background and installed at
    /// the next start (a server, with no one to ask: at once). `false`: it is
    /// only offered, and installs when the app says (the cell's `apply`).
    #[serde(default = "yes")]
    pub auto: bool,
    /// Offer prereleases (`2.1.0-rc.1`). Unset: only off `stable`, or to an
    /// app on a prerelease already.
    #[serde(default)]
    pub prerelease: Option<bool>,
    /// Installed versions kept, the running one included (at least 2: a
    /// rollback needs the one before).
    #[serde(default = "three")]
    pub keep: usize,
}

fn stable() -> String {
    "stable".into()
}

fn six_hours() -> Option<Duration> {
    Some(Duration::from_secs(6 * 3600))
}

fn yes() -> bool {
    true
}

fn three() -> usize {
    3
}

impl Default for Updates {
    fn default() -> Self {
        Self {
            url: None,
            keys: vec![],
            channel: stable(),
            every: six_hours(),
            every_by_channel: Default::default(),
            auto: true,
            prerelease: None,
            keep: 3,
        }
    }
}

impl Updates {
    /// How often an app on `channel` looks for a release; `None`: off.
    pub fn every_on(&self, channel: &str) -> Option<Duration> {
        self.every_by_channel.get(channel).copied().unwrap_or(self.every)
    }
}

/// How hard each commit is saved.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(rename_all = "lowercase")]
pub enum Durability {
    /// Survives the app crashing; the last commits may be lost if the
    /// machine loses power (SQLite WAL, `synchronous = NORMAL`).
    #[default]
    Normal,
    /// Each commit is on disk before the call returns (`synchronous = FULL`).
    /// Phones always use it: the OS may kill the app at any moment.
    Full,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(deny_unknown_fields)]
pub struct Data {
    #[serde(default)]
    pub durability: Durability,
    /// The store's file in `data/` (aio's `dbPath`, e.g. `vault.db`).
    #[serde(default = "Data::default_store")]
    pub store: String,
    /// Files in `data/` set aside at boot as `<name>.pre-migration` (with
    /// their `-wal` and `-shm`), never deleted: an old store the app no
    /// longer opens (an aio app retiring a stray `state.db`).
    #[serde(default)]
    pub retire: Vec<String>,
    /// Pragmas set on the store's file after aio's own (aio's `dbPragmas`,
    /// e.g. `"PRAGMA cache_size = -16000"`); one SQLite ignores refuses the
    /// boot. Not `journal_mode`, `synchronous` (that is `durability`) or
    /// `user_version`: the store relies on them.
    #[serde(default)]
    pub pragmas: Vec<String>,
    /// `false`: the state lives in memory only, never saved, and starts
    /// from its defaults on every launch (aio's `persist: false`; the launch
    /// flag `--no-persist` does it for one run).
    #[serde(default = "yes")]
    pub persist: bool,
    /// How long a run of `#[coalesce]` calls is held before it goes into the
    /// log, in ms (aio's `persistDebounceMs`): a crash loses at most this much.
    #[serde(default = "Data::default_coalesce_ms")]
    pub coalesce_ms: u64,
    /// How often a running app backs its store up into `<home>/backups`
    /// (`"1d"`, `"6h"`); `"off"` (the default): only when asked (`ax backup`).
    #[serde(default, with = "span_or_off")]
    pub backup_every: Option<Duration>,
    /// Timed backups kept, the newest; older ones are removed (at least 1).
    #[serde(default = "Data::default_backup_keep")]
    pub backup_keep: usize,
}

/// The store's own pragmas, never an app's: `[data] pragmas` can't set them.
const STORE_PRAGMAS: [&str; 3] = ["journal_mode", "synchronous", "user_version"];

impl Data {
    fn default_store() -> String {
        "state.db".into()
    }

    fn default_coalesce_ms() -> u64 {
        100
    }

    fn default_backup_keep() -> usize {
        7
    }
}

impl Default for Data {
    fn default() -> Self {
        Self {
            durability: Durability::default(),
            store: Self::default_store(),
            retire: vec![],
            pragmas: vec![],
            persist: true,
            coalesce_ms: Self::default_coalesce_ms(),
            backup_every: None,
            backup_keep: Self::default_backup_keep(),
        }
    }
}

/// `[home]`: a ported app's own home rules, as its aio code resolved them;
/// `--home`, `--profile` and `--instance` still win.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(deny_unknown_fields, default)]
pub struct Home {
    /// A variable naming the home (`PURSE_HOME`): it beats `AIO_APPS_DIR`.
    pub env: Option<String>,
    /// A folder beside the app's binary (`purse-data`): the home when it
    /// exists (a portable install). Not in a debug build.
    pub portable: Option<String>,
    /// A debug build's home in the user's folder (`.purse-dev`), so a dev
    /// run never opens the real data. A profile counts from the plain home.
    pub dev: Option<String>,
}

/// `[log]`: what the log keeps. The app, error and warning logs hold at
/// most `3 × keep × file_mb` together, whatever the app does.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(deny_unknown_fields, default)]
pub struct Log {
    /// The lowest level written: `trace`, `debug`, `info`, `warn`, `error`.
    pub level: crate::log::Level,
    /// Files kept per log (`app.log`, `app.1.log` …), at least 1.
    pub keep: usize,
    /// A file rotates at this size, in MB.
    pub file_mb: u64,
    /// Seconds between `heartbeat` lines (uptime, calls, errors); 0 is none.
    pub heartbeat: u64,
    /// Methods (`notes.tick`) whose calls are not logged even at `debug`.
    pub suppress: Vec<String>,
    /// Each run starts fresh files (aio's per-run `debug.log`): the last
    /// run's move to `app.1.log` …, `keep` runs (or size steps) kept.
    pub per_run: bool,
}

impl Default for Log {
    fn default() -> Self {
        Self { level: crate::log::Level::Info, keep: 5, file_mb: 10, heartbeat: 3600, suppress: vec![], per_run: false }
    }
}

/// What `ax build <app>` makes, in order (the kinds: `ax build <app> --list`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(deny_unknown_fields)]
pub struct Build {
    /// A kind (`"deb"`), or `{ name, kind, bin }`: a labelled one, to have
    /// two of a kind or build another binary of the workspace.
    #[serde(default)]
    pub targets: Vec<Target>,
    /// The desktop binary's size budget in MB, for apps with heavy batteries
    /// (`ml`, `scene`, `maps`); unset: 10. The build fails above it and says
    /// which crates take the space.
    #[serde(default)]
    pub budget_mb: Option<u64>,
    /// A CSS toolchain run before every build and on every change in
    /// `ax dev` (Tailwind, Sass): a command line split on spaces, or argv as
    /// a list. Never through a shell.
    #[serde(default)]
    pub css: Option<Css>,
    #[serde(default)]
    pub android: AndroidBuild,
    #[serde(default)]
    pub macos: MacBuild,
    #[serde(default)]
    pub windows: WindowsBuild,
    /// The window a first start shows while Electron downloads, and its
    /// Retry / Quit question (aio's `electron.downloadWindow`).
    #[serde(default)]
    pub download_window: DownloadWindow,
    /// Every build then starts each Linux artifact it made headless, stops
    /// it and checks its log, as `--smoke` does.
    #[serde(default)]
    pub smoke: bool,
    /// The OS and CPU each desktop target is built for (`linux`,
    /// `linux-arm64`, `windows`, `windows-arm64`, `macos`, `macos-arm64`,
    /// `host`); unset: this machine. `ax build` checks the names.
    #[serde(default)]
    pub platforms: Vec<String>,
    /// Where a desktop app's Electron comes from: `shared` (one per machine,
    /// fetched at the first start) or `bundled` (an offline build: in the
    /// package beside the binary, trimmed; a macOS one is made on a Mac).
    #[serde(default)]
    pub runtime: RuntimeFrom,
    /// A bundled runtime's optional Chromium GPU extras (software Vulkan,
    /// DirectX shader compilers): `keep`, or `strip` for a smaller package
    /// that draws less well where no GPU driver works.
    #[serde(default)]
    pub chromium_extras: Extras,
    /// The splash: the app's image (and text) on screen from its start until
    /// its window has loaded, on the desktop, the web and Android.
    #[serde(default)]
    pub splash: Option<Splash>,
}

/// `[build.splash]` (aio's `build.splash`).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(deny_unknown_fields)]
pub struct Splash {
    /// A PNG, from `aio.toml`'s folder; `…@2x.png` is drawn at half its
    /// pixel size (crisp on a 2x screen). Built into the app.
    pub image: std::path::PathBuf,
    /// Under the image; `{title}` is the app's name.
    #[serde(default)]
    pub text: Option<String>,
    /// It stays at least this many milliseconds once shown.
    #[serde(default = "Splash::default_min")]
    pub min_ms: u64,
    /// No window within this many milliseconds: it goes anyway.
    #[serde(default = "Splash::default_max")]
    pub max_ms: u64,
    /// Behind the image, `["#light", "#dark"]`; unset: the app's own colours.
    #[serde(default)]
    pub background: Option<[String; 2]>,
    /// When it shows: every start, only while a first start downloads
    /// Electron, or never (a download then shows the plain download window).
    #[serde(default)]
    pub show: SplashShow,
    /// The line under the bar while a first start downloads Electron in the
    /// splash: `{title}`, `{version}`,
    /// `{got}` and `{total}` (MB), `{pct}`. Unset: `[build.download_window] text`.
    #[serde(default)]
    pub download_text: Option<String>,
    /// The window's size in px `[width, height]`; unset: the image's.
    #[serde(default)]
    pub size: Option<[u32; 2]>,
    /// How the image fills `size` when its shape differs.
    #[serde(default)]
    pub fit: Fit,
    /// The text in a strip below the image, or over its lower part.
    #[serde(default)]
    pub text_position: TextPosition,
    #[serde(default)]
    pub align: Align,
    /// The text's and bar's gap to the window's edges, px; unset: 24 at the
    /// sides, 18 at the bottom.
    #[serde(default)]
    pub margin: Option<u32>,
    /// The window's corner radius, px.
    #[serde(default)]
    pub radius: u32,
    /// A 1 px border in this colour; unset: none.
    #[serde(default)]
    pub border: Option<String>,
    /// The text, `["#light", "#dark"]`; unset: the app's.
    #[serde(default)]
    pub text_color: Option<[String; 2]>,
    /// The download bar's fill and its track, each `["#light", "#dark"]`.
    #[serde(default)]
    pub bar: Option<[String; 2]>,
    #[serde(default)]
    pub track: Option<[String; 2]>,
    /// The text's font family (Android: `sans-serif`, `serif`, `monospace`,
    /// `sans-serif-condensed`…); unset: the system's.
    #[serde(default)]
    pub font: Option<String>,
    /// The text's size (px on the desktop, points on Android); unset: the
    /// target's (13 px, 14 pt).
    #[serde(default)]
    pub font_size: Option<u16>,
    /// The text in bold.
    #[serde(default)]
    pub bold: bool,
    /// How long it fades out, in milliseconds; unset: the target's (100 on
    /// the desktop, 200 on Android).
    #[serde(default)]
    pub fade_ms: Option<u64>,
    /// The PNG's bytes, put in by `aio::config!()` (empty elsewhere).
    #[serde(skip)]
    pub png: &'static [u8],
}

impl Default for Splash {
    fn default() -> Self {
        Self {
            image: Default::default(),
            text: None,
            min_ms: Self::default_min(),
            max_ms: Self::default_max(),
            background: None,
            show: Default::default(),
            download_text: None,
            size: None,
            fit: Default::default(),
            text_position: Default::default(),
            align: Default::default(),
            margin: None,
            radius: 0,
            border: None,
            font: None,
            fade_ms: None,
            font_size: None,
            bold: false,
            text_color: None,
            bar: None,
            track: None,
            png: &[],
        }
    }
}

/// `[build.splash] show` (aio's `show`, and its run-time `splash.show`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(rename_all = "kebab-case")]
pub enum SplashShow {
    #[default]
    Always,
    FirstStart,
    Never,
}

/// `[build.splash] fit`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(rename_all = "kebab-case")]
pub enum Fit {
    #[default]
    Contain,
    Cover,
}

/// `[build.splash] text_position`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(rename_all = "kebab-case")]
pub enum TextPosition {
    #[default]
    Below,
    Over,
}

/// `[build.splash] align`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(rename_all = "kebab-case")]
pub enum Align {
    Left,
    #[default]
    Center,
    Right,
}

impl Splash {
    fn default_min() -> u64 {
        400
    }
    fn default_max() -> u64 {
        15_000
    }
    /// Drawn at half its pixel size (`…@2x.png`).
    pub fn retina(&self) -> bool {
        self.image.file_stem().is_some_and(|s| s.to_string_lossy().ends_with("@2x"))
    }
}

/// `#rgb` or `#rrggbb`.
pub fn hex_color(s: &str) -> bool {
    s.strip_prefix('#').is_some_and(|h| matches!(h.len(), 3 | 6) && h.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// `[build] runtime`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeFrom {
    #[default]
    Shared,
    Bundled,
}

/// `[build] chromium_extras`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(rename_all = "kebab-case")]
pub enum Extras {
    #[default]
    Keep,
    Strip,
}

/// `[build.android]`: the app's Android identity and release key.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(deny_unknown_fields)]
pub struct AndroidBuild {
    /// Its permanent identity on Android and in Play (`com.example.notes`);
    /// unset: `aio.rt.<id>`.
    #[serde(default)]
    pub application_id: Option<String>,
    /// The release keystore (relative: the app's dir), else the key aio
    /// makes. Its password is never here (a committed file): the build reads
    /// `AIO_ANDROID_KEYSTORE_PASSWORD`; `AIO_ANDROID_KEYSTORE` overrides it.
    #[serde(default)]
    pub keystore: Option<String>,
    /// The key in it (`AIO_ANDROID_KEY_ALIAS` overrides).
    #[serde(default)]
    pub key_alias: Option<String>,
    /// Also an Android App Bundle (`.aab`) for Play.
    #[serde(default)]
    pub aab: bool,
    /// URL schemes the app opens (`myapp://users/7`): each link reaches the
    /// running app (`aio::ui::links`; a `router!` goes there). Lower-case,
    /// none of the system's (`https`, `content`…).
    #[serde(default)]
    pub schemes: Vec<String>,
    /// The page may ask for the camera (`getUserMedia({ video: true })`):
    /// the APK declares it, and Android asks the user once. Off: refused.
    #[serde(default)]
    pub camera: bool,
    /// Light or dark whatever the phone's setting; unset: the phone's.
    #[serde(default)]
    pub color_scheme: ColorScheme,
    /// Also one APK for every CPU type (`…-android-universal.apk`), for
    /// sharing as one file; the per-CPU APKs are still made.
    #[serde(default)]
    pub universal: bool,
    /// An `android/` file that replaces a template file the template has
    /// changed since, or a `MainActivity.java` without what the template's
    /// does, fails the build instead of warning.
    #[serde(default)]
    pub strict: bool,
}

/// `[build.android] color_scheme`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(rename_all = "kebab-case")]
pub enum ColorScheme {
    #[default]
    System,
    Light,
    Dark,
}

/// Schemes Android or the WebView own: an app taking one would take every
/// web page, file or install link.
const SYSTEM_SCHEMES: [&str; 18] = [
    "about",
    "android-app",
    "blob",
    "content",
    "data",
    "file",
    "ftp",
    "geo",
    "http",
    "https",
    "intent",
    "javascript",
    "mailto",
    "market",
    "sms",
    "tel",
    "ws",
    "wss",
];

/// Why `s` can't be an app's URL scheme, if it can't: RFC 3986's form in
/// lower case (Android matches schemes case-sensitively), none of the system's.
pub fn scheme_refused(s: &str) -> Option<String> {
    let form = s.starts_with(|c: char| c.is_ascii_lowercase())
        && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "+-.".contains(c));
    if !form {
        return Some(format!(
            "{s:?} is not a URL scheme: a lower-case letter, then lower-case letters, digits, + - or ."
        ));
    }
    SYSTEM_SCHEMES.contains(&s).then(|| format!("{s:?} is the system's: it would take every {s}: link"))
}

/// `[build.macos]`: the app's macOS identity, and the Mac it is built on.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(deny_unknown_fields)]
pub struct MacBuild {
    /// Its permanent identity on macOS (`CFBundleIdentifier`,
    /// `com.example.notes`), the `.app`'s and its window's; unset:
    /// `aio.rt.<id>`.
    #[serde(default)]
    pub bundle_id: Option<String>,
    /// The Mac a build away from one runs on, over SSH (`[ssh options]
    /// [user@]host`, as `AIO_MACOS_SSH`, which it outranks).
    #[serde(default)]
    pub host: Option<String>,
}

/// `[build.download_window]` (aio's `electron.downloadWindow`): every key
/// optional, the defaults aio's. The texts take `{title}` and `{version}`
/// (Electron's); `text` also `{got}` and `{total}` (MB), `question`
/// `{reason}`. A colour set is used in light and dark mode alike; one not
/// set is the app's own, as the OS's mode is.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(deny_unknown_fields)]
pub struct DownloadWindow {
    /// Unset: "Getting {title} ready".
    #[serde(default)]
    pub heading: Option<String>,
    /// Unset: "Downloading Electron {version} — first start only · {got} / {total} MB".
    #[serde(default)]
    pub text: Option<String>,
    /// The Retry / Quit question; unset: aio's, which names what failed
    /// (`{reason}`) and how to fix it.
    #[serde(default)]
    pub question: Option<String>,
    #[serde(default)]
    pub background: Option<String>,
    /// The text's colour.
    #[serde(default)]
    pub color: Option<String>,
    /// The bar's fill and its track.
    #[serde(default)]
    pub bar: Option<String>,
    #[serde(default)]
    pub track: Option<String>,
    /// The Cancel button (closing the window cancels either way).
    #[serde(default = "yes")]
    pub cancel: bool,
}

impl Default for DownloadWindow {
    fn default() -> Self {
        Self {
            heading: None,
            text: None,
            question: None,
            background: None,
            color: None,
            bar: None,
            track: None,
            cancel: true,
        }
    }
}

/// `[build.windows]`: how the one-click download installs the app.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(deny_unknown_fields)]
pub struct WindowsBuild {
    /// A Start menu entry on the first install (aio's
    /// `build.windows.shortcut`); `false`: none.
    #[serde(default = "yes")]
    pub shortcut: bool,
}

impl Default for WindowsBuild {
    fn default() -> Self {
        Self { shortcut: true }
    }
}

impl Config {
    /// The macOS bundle id: `[build.macos] bundle_id`, else `aio.rt.<id>`.
    pub fn mac_bundle_id(&self) -> String {
        self.build.macos.bundle_id.clone().unwrap_or_else(|| format!("aio.rt.{}", self.app.id))
    }
}

/// Apple's rule for a bundle id: two or more dot-separated segments of
/// letters, digits and `-`.
pub fn bundle_id_ok(id: &str) -> bool {
    id.split('.').count() >= 2
        && id.split('.').all(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
}

/// A BCP 47 language tag's shape: `-`-separated runs of letters and digits.
pub fn lang_ok(tag: &str) -> bool {
    tag.split('-').all(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// Android's rule for a package name: two or more dot-separated segments,
/// each a letter then letters, digits or `_`.
pub fn application_id_ok(id: &str) -> bool {
    id.split('.').count() >= 2
        && id.split('.').all(|s| {
            s.starts_with(|c: char| c.is_ascii_alphabetic()) && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

/// The `[build] css` command, as argv.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Lit)]
pub struct Css(pub Vec<String>);

// By hand, not through `toml::Value`: its deserializer would be in every app.
impl<'de> Deserialize<'de> for Css {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Argv;
        impl<'de> serde::de::Visitor<'de> for Argv {
            type Value = Vec<String>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("[build] css: a command or an argv list")
            }
            fn visit_str<E>(self, s: &str) -> Result<Vec<String>, E> {
                Ok(s.split_whitespace().map(String::from).collect())
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut a: A) -> Result<Vec<String>, A::Error> {
                let mut argv = vec![];
                while let Some(s) =
                    a.next_element::<String>().map_err(|e| serde::de::Error::custom(format!("[build] css: {e}")))?
                {
                    argv.push(s);
                }
                Ok(argv)
            }
        }
        let argv = d.deserialize_any(Argv)?;
        if argv.is_empty() || argv.iter().any(String::is_empty) {
            return Err(serde::de::Error::custom(
                "[build] css: give it a command (`tailwindcss -i src/app.css -o assets/style.css`) or remove it",
            ));
        }
        Ok(Self(argv))
    }
}

/// One build target, labelled `name`, of kind `kind` (else `name`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Lit)]
pub struct Target {
    pub name: String,
    pub kind: Option<String>,
    /// The workspace binary it builds (with its own `aio.toml`); unset: the app.
    pub bin: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Labelled {
    name: String,
    kind: Option<String>,
    bin: Option<String>,
}

impl<'de> Deserialize<'de> for Target {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Kind;
        impl<'de> serde::de::Visitor<'de> for Kind {
            type Value = Labelled;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("[build] targets: a kind or { name, kind, bin }")
            }
            fn visit_str<E>(self, name: &str) -> Result<Labelled, E> {
                Ok(Labelled { name: name.into(), kind: None, bin: None })
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, m: A) -> Result<Labelled, A::Error> {
                Labelled::deserialize(serde::de::value::MapAccessDeserializer::new(m))
                    .map_err(|e| serde::de::Error::custom(format!("[build] targets: {e}")))
            }
        }
        let Labelled { name, kind, bin } = d.deserialize_any(Kind)?;
        let trim = |s: Option<String>| s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        Ok(Self { name: name.trim().to_string(), kind: trim(kind), bin: trim(bin) })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub app: App,
    #[serde(default)]
    pub data: Data,
    #[serde(default)]
    pub home: Home,
    #[serde(default)]
    pub history: History,
    #[serde(default)]
    pub server: Server,
    #[serde(default)]
    pub client: Client,
    #[serde(default)]
    pub accounts: Accounts,
    #[serde(default)]
    pub capabilities: Capabilities,
    #[serde(default)]
    pub updates: Updates,
    #[serde(default)]
    pub build: Build,
    #[serde(default)]
    pub log: Log,
    #[serde(default)]
    pub perf: crate::perf::Budgets,
    #[serde(default)]
    pub memory: Memory,
    #[serde(default)]
    pub budgets: crate::budgets::Budgets,
    #[serde(default)]
    pub feedback: Feedback,
    #[serde(default)]
    pub ui: Ui,
    #[serde(default)]
    pub vitals: crate::vitals::Vitals,
}

/// `[feedback]`: problem reports, filed by the user (the `feedback` cell)
/// or by the app when it breaks. Always kept on disk first; sent on to
/// `url` too when set.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(deny_unknown_fields, default)]
pub struct Feedback {
    /// Each report is also POSTed here as JSON: https only (a loopback
    /// `http://127.0.0.1` for tests).
    pub url: Option<String>,
    /// How long a POST may take before the report counts as not delivered.
    #[serde(with = "span")]
    pub timeout: Duration,
    /// Reports kept on disk, newest first.
    pub keep: usize,
    /// Report a problem by itself when the app breaks; `false`: only the
    /// user's reports.
    pub auto: bool,
}

impl Default for Feedback {
    fn default() -> Self {
        Self { url: None, timeout: Duration::from_secs(15), keep: crate::report::KEEP, auto: true }
    }
}

/// `[ui]`: the page's default look (aio's `ui.theme`, `ui.hue`, `ui.layout`);
/// `aio::kit::theme::sheet` makes its CSS.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(default, deny_unknown_fields)]
pub struct Ui {
    pub theme: Look,
    /// The accent's hue in degrees, instead of the one hashed from the id.
    pub hue: Option<u16>,
    /// With the look, the page layout: `<main>` as a centred page and the
    /// `.card`/`.row`/`.stack`/`.grid` classes. Off: only how elements look.
    pub layout: bool,
}

impl Default for Ui {
    fn default() -> Self {
        Self { theme: Look::Auto, hue: None, layout: true }
    }
}

/// How much of aio's look a page takes. Every one but `None` keeps the
/// `--aio-*` tokens and the kit's controls.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(rename_all = "lowercase")]
pub enum Look {
    /// Only the tokens: elements look as the browser draws them.
    Tokens,
    /// The whole look, until the app ships a stylesheet of its own (then `Tokens`).
    #[default]
    Auto,
    /// The whole look, and the app's stylesheet over it.
    Full,
    /// No aio CSS at all: the app brings every rule (the kit is bare).
    None,
}

/// `[memory]`: when this process's memory is a problem, in percent of the
/// machine's (`health`). A misspelt key is an error, never a silent default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(deny_unknown_fields, default)]
pub struct Memory {
    /// High at this percent...
    pub high: u8,
    /// ...and clear again below this one.
    pub clear: u8,
    /// Critical: about to swap or be killed.
    pub critical: u8,
    /// Growth said once per this many MB (or a tenth of the machine).
    pub growth_mb: u64,
    /// How often memory is sampled (and `health.json` written): 1 s to
    /// 10 s (aio's `memory.interval`).
    #[serde(with = "span")]
    pub every: Duration,
}

impl Default for Memory {
    fn default() -> Self {
        Self { high: 50, clear: 40, critical: 80, growth_mb: 256, every: Duration::from_secs(10) }
    }
}

/// A config value as the Rust expression that makes it (`#[derive(Lit)]`):
/// an app's `aio.toml` is parsed when it is built (`aio::config!()`), and its
/// binary holds the result, without TOML's parser or the config's.
pub trait Lit {
    fn lit(&self, o: &mut String);
}

macro_rules! lit_display {
    ($($t:ty),*) => {$(
        impl Lit for $t {
            fn lit(&self, o: &mut String) {
                o.push_str(&self.to_string());
            }
        }
    )*};
}
lit_display!(bool, u8, u16, u32, u64, usize);

impl Lit for f64 {
    fn lit(&self, o: &mut String) {
        o.push_str(&format!("f64::from_bits({})", self.to_bits()));
    }
}

impl Lit for String {
    fn lit(&self, o: &mut String) {
        o.push_str(&format!("::std::string::String::from({self:?})"));
    }
}

impl Lit for std::path::PathBuf {
    fn lit(&self, o: &mut String) {
        o.push_str(&format!("::std::path::PathBuf::from({:?})", self.to_string_lossy()));
    }
}

impl Lit for Duration {
    fn lit(&self, o: &mut String) {
        o.push_str(&format!("::std::time::Duration::new({}, {})", self.as_secs(), self.subsec_nanos()));
    }
}

impl Lit for &'static [u8] {
    fn lit(&self, o: &mut String) {
        o.push_str("&[]");
    }
}

impl<T: Lit, const N: usize> Lit for [T; N] {
    fn lit(&self, o: &mut String) {
        o.push('[');
        for v in self {
            v.lit(o);
            o.push(',');
        }
        o.push(']');
    }
}

impl<T: Lit> Lit for Option<T> {
    fn lit(&self, o: &mut String) {
        match self {
            Some(v) => {
                o.push_str("::std::option::Option::Some(");
                v.lit(o);
                o.push(')');
            }
            None => o.push_str("::std::option::Option::None"),
        }
    }
}

impl<T: Lit> Lit for Vec<T> {
    fn lit(&self, o: &mut String) {
        o.push_str("::std::vec![");
        for v in self {
            v.lit(o);
            o.push(',');
        }
        o.push(']');
    }
}

impl<K: Lit, V: Lit> Lit for std::collections::BTreeMap<K, V> {
    fn lit(&self, o: &mut String) {
        o.push_str("::std::collections::BTreeMap::from([");
        for (k, v) in self {
            o.push('(');
            k.lit(o);
            o.push(',');
            v.lit(o);
            o.push_str("),");
        }
        o.push_str("])");
    }
}

/// The app's one `aio.toml` for the crate in `dir`: the nearest from `dir`
/// up. The one rule: `aio::config!()` builds this file in, and `aio
/// config`, `ax build`, `ax doctor` read the same.
pub fn find(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    dir.ancestors().map(|d| d.join("aio.toml")).find(|f| f.is_file())
}

impl Config {
    /// The app's one hue (aio's `hue ?? appId`): `[ui] hue`, else its id's
    /// (`icon::hue`). Its page, icon, splash and download window share it.
    pub fn hue(&self) -> u16 {
        self.ui.hue.unwrap_or_else(|| crate::icon::hue(&self.app.id))
    }

    /// The config as the Rust expression that makes it (paths from `aio::`).
    pub fn rust(&self) -> String {
        let mut o = String::new();
        self.lit(&mut o);
        o
    }

    /// Parse and check `aio.toml` text.
    #[cfg(feature = "toml")]
    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| format!("aio.toml: {e}")).and_then(Self::check)
    }

    /// Check a config read from `aio.toml`: what its types alone can't say.
    pub fn check(c: Self) -> Result<Self, String> {
        crate::paths::id_ok(&c.app.id).map_err(|e| format!("aio.toml: {e}"))?;
        if let Some(port_of) = &c.app.port_of {
            crate::paths::id_ok(port_of).map_err(|e| format!("aio.toml: [app] port_of: {e}"))?;
        }
        if c.app.port_of.is_none() && c.home != Home::default() {
            return Err("aio.toml: [home] is a ported app's rules: it needs [app] port_of".into());
        }
        // File names: a store or a home never reaches out of its folder.
        let h = &c.home;
        let names = [
            ("[data] store", Some(&c.data.store)),
            ("[home] portable", h.portable.as_ref()),
            ("[home] dev", h.dev.as_ref()),
        ]
        .into_iter()
        .chain(c.data.retire.iter().map(|n| ("[data] retire", Some(n))));
        for (k, n) in names.filter_map(|(k, n)| Some((k, n?))) {
            if n.is_empty() || n == "." || n == ".." || n.contains(['/', '\\']) {
                return Err(format!("aio.toml: {k} {n:?} is a file name, without folders"));
            }
        }
        for p in &c.data.pragmas {
            let name = crate::db::pragma_name(p)
                .ok_or_else(|| format!("aio.toml: [data] pragmas: {p:?} is not a PRAGMA statement"))?;
            if STORE_PRAGMAS.contains(&name.as_str()) {
                let why = if name == "synchronous" { "set [data] durability" } else { "the store relies on it" };
                return Err(format!("aio.toml: [data] pragmas can't set {name}: {why}"));
            }
        }
        if c.data.backup_keep == 0 {
            return Err("aio.toml: [data] backup_keep must keep at least 1 backup".into());
        }
        if c.data.coalesce_ms > 60_000 {
            return Err(format!(
                "aio.toml: [data] coalesce_ms = {} holds calls over a minute: a crash would lose them all",
                c.data.coalesce_ms
            ));
        }
        if c.data.retire.contains(&c.data.store) {
            return Err(format!("aio.toml: [data] retire names the store {:?} itself", c.data.store));
        }
        if h.env.as_ref().is_some_and(|k| k.is_empty() || k.contains(['=', '\0'])) {
            return Err("aio.toml: [home] env is not a variable name".into());
        }
        let need = |ok: bool, what: &str| {
            if ok { Ok(()) } else { Err(format!("aio.toml: mode {:?} needs {what}", c.app.mode)) }
        };
        match c.app.mode {
            Mode::ClientServer | Mode::Server => need(c.server.listen.is_some(), "[server] listen")?,
            // A client may get its server from pairing instead (`ax pair`).
            Mode::Standalone | Mode::Cli | Mode::Client => {}
        }
        if c.app.ui == Shown::Browser && c.app.mode != Mode::ClientServer {
            return Err(format!(
                "aio.toml: [app] ui = \"browser\" needs mode \"client-server\" (its server serves the page), not {:?}",
                c.app.mode
            ));
        }
        if c.app.lang.as_deref().is_some_and(|l| !lang_ok(l)) {
            return Err("aio.toml: [app] lang is a language tag: letters, digits and '-' (\"en\", \"pt-BR\")".into());
        }
        let dw = &c.build.download_window;
        for (k, v) in [("background", &dw.background), ("color", &dw.color), ("bar", &dw.bar), ("track", &dw.track)] {
            if v.as_deref().is_some_and(|v| !hex_color(v)) {
                return Err(format!("aio.toml: [build.download_window] {k} is #rgb or #rrggbb"));
            }
        }
        if let Some(s) = &c.build.splash {
            if s.image.extension().is_none_or(|e| !e.eq_ignore_ascii_case("png"))
                || s.image.is_absolute()
                || s.image.components().any(|p| p == std::path::Component::ParentDir)
            {
                return Err(
                    "aio.toml: [build.splash] image is a .png in the app's folder (\"assets/splash.png\")".into()
                );
            }
            let pairs =
                [("background", &s.background), ("text_color", &s.text_color), ("bar", &s.bar), ("track", &s.track)];
            for (k, v) in pairs {
                if v.as_ref().is_some_and(|b| !b.iter().all(|c| hex_color(c))) {
                    return Err(format!(
                        "aio.toml: [build.splash] {k} is [\"#light\", \"#dark\"], each #rgb or #rrggbb"
                    ));
                }
            }
            if s.border.as_deref().is_some_and(|c| !hex_color(c)) {
                return Err("aio.toml: [build.splash] border is #rgb or #rrggbb".into());
            }
            if s.size.is_some_and(|[w, h]| w == 0 || h == 0 || w > 4096 || h > 4096) {
                return Err("aio.toml: [build.splash] size is [width, height], each 1 to 4096".into());
            }
            if s.min_ms > s.max_ms {
                return Err("aio.toml: [build.splash] min_ms is more than max_ms".into());
            }
        }
        if c.ui.hue.is_some_and(|h| h >= 360) {
            return Err("aio.toml: [ui] hue is degrees, 0 to 359".into());
        }
        let m = c.memory;
        if !(m.clear < m.high && m.high <= m.critical && m.critical <= 100) || m.growth_mb == 0 {
            return Err("aio.toml: [memory] needs clear < high <= critical <= 100, and growth_mb of 1 or more".into());
        }
        if !(Duration::from_secs(1)..=Duration::from_secs(10)).contains(&m.every) {
            return Err(
                "aio.toml: [memory] every must be 1s to 10s (ax health calls a health.json older than 30 s stuck)"
                    .into(),
            );
        }
        c.vitals.check()?;
        if c.log.keep == 0 || c.log.file_mb == 0 {
            return Err("aio.toml: [log] keep and file_mb must be at least 1".into());
        }
        if !c.accounts.on && c.accounts.sessions && (c.accounts.signup || c.accounts.require_verified) {
            return Err(
                "aio.toml: [accounts] signup and require_verified need accounts (on = true), not sessions alone".into(),
            );
        }
        if c.accounts.on || c.accounts.sessions {
            let p = c.accounts.policy();
            if !(1..=crate::auth::MAX_TTL_MS).contains(&p.ttl_ms) {
                return Err("aio.toml: [accounts] session must be 1s to 100 years".into());
            }
            if p.max_sessions == Some(0) {
                return Err("aio.toml: [accounts] max_sessions must be at least 1".into());
            }
        }
        if c.server.open && (c.accounts.on || c.accounts.sessions) {
            return Err("aio.toml: [server] open and [accounts] both say who gets in: give one".into());
        }
        // A name no request can carry would trust nothing, silently.
        if let Some(h) = &c.server.trust_proxy
            && !(!h.is_empty() && h.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)))
        {
            return Err(format!(
                "aio.toml: [server] trust_proxy {h:?} is not a header name (e.g. \"x-forwarded-for\")"
            ));
        }
        crate::security::headers(&c.server.security, &c.server.hosts, true).map_err(|e| format!("aio.toml: {e}"))?;
        if let Some(e) = dirs_problem(&c.server) {
            return Err(format!("aio.toml: {e}"));
        }
        // Half a pair would serve aio's own certificate instead, silently.
        match (&c.server.cert, &c.server.key) {
            (None, None) => {}
            (Some(cert), Some(key)) => {
                if !c.server.tls {
                    return Err("aio.toml: [server] cert and key need tls = true".into());
                }
                if let Some(p) = [cert, key].into_iter().find(|p| !p.is_absolute()) {
                    return Err(format!(
                        "aio.toml: [server] {}: an absolute path (aio.toml is built into the app, so a relative \
                         one has no folder to be relative to)",
                        p.display()
                    ));
                }
            }
            _ => {
                return Err(
                    "aio.toml: [server] cert and key go together: give both, or neither for aio-rs's own".into()
                );
            }
        }
        // No trust on first use: updates need the keys built in.
        if let Some(url) = &c.updates.url {
            if c.updates.keys.is_empty() {
                return Err("aio.toml: [updates] url needs keys (ax keygen <app>)".into());
            }
            let local = url.starts_with("http://127.0.0.1:") || url.starts_with("http://[::1]:");
            let github = url.strip_prefix("github:").map(crate::update::github_repo);
            if github == Some(None) {
                return Err("aio.toml: [updates] url github:<owner>/<repo> names no repository".into());
            }
            let git = crate::update::git_remote(url).is_some();
            if !url.starts_with("https://") && !local && github.is_none() && !git {
                return Err("aio.toml: [updates] url must be https://, github:<owner>/<repo> or a repository \
                     (git+https://, git+ssh://, git+file://)"
                    .into());
            }
            if let Some(e) = c.updates.keys.iter().find_map(|k| crate::update::ssh_key(k).err()).filter(|_| git) {
                return Err(format!("aio.toml: [updates] keys: {e}"));
            }
        }
        if let Some(id) = c.build.android.application_id.as_deref().filter(|id| !application_id_ok(id)) {
            return Err(format!(
                "aio.toml: [build.android] application_id {id:?} is not an Android package name: two or more \
                 dot-separated segments, each starting with a letter (com.example.notes)"
            ));
        }
        if let Some(why) = c.build.android.schemes.iter().find_map(|s| scheme_refused(s)) {
            return Err(format!("aio.toml: [build.android] schemes: {why}"));
        }
        if let Some(id) = c.build.macos.bundle_id.as_deref().filter(|id| !bundle_id_ok(id)) {
            return Err(format!(
                "aio.toml: [build.macos] bundle_id {id:?} is not a bundle id: two or more dot-separated \
                 segments of letters, digits and - (com.example.notes). It is used as written: it is the \
                 app's permanent identity"
            ));
        }
        if c.build.macos.host.as_deref().is_some_and(|h| h.trim().is_empty()) {
            return Err("aio.toml: [build.macos] host names no Mac: `[ssh options] [user@]host`".into());
        }
        for ch in std::iter::once(&c.updates.channel).chain(c.updates.every_by_channel.keys()) {
            if let Some(why) = crate::update::channel_refused(ch) {
                return Err(format!("aio.toml: [updates] {why}"));
            }
        }
        if let Some(url) = &c.feedback.url
            && !url.starts_with("https://")
            && !url.starts_with("http://127.0.0.1:")
            && !url.starts_with("http://[::1]:")
        {
            return Err("aio.toml: [feedback] url must be https://".into());
        }
        if c.feedback.keep == 0 || c.feedback.timeout.is_zero() {
            return Err("aio.toml: [feedback] keep and timeout must be at least 1".into());
        }
        if c.updates.keep < 2 {
            return Err("aio.toml: [updates] keep must be at least 2 (the running version and the one before)".into());
        }
        if let Some(e) = c.capabilities.missing(&c) {
            return Err(format!("aio.toml: {e}"));
        }
        Ok(c)
    }
}

/// `90d`, `12h`, `30m`, `45s`.
pub fn parse_span(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    let (n, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
    let n: u64 = n.parse().map_err(|_| format!("`{s}`: expected a number and d, h, m or s"))?;
    let secs = match unit {
        "d" => 86_400,
        "h" => 3600,
        "m" => 60,
        "s" => 1,
        _ => return Err(format!("`{s}`: expected a number and d, h, m or s")),
    };
    Ok(Duration::from_secs(n * secs))
}

/// A span as `aio.toml` spells it: the largest unit it is a whole number of.
pub fn span_text(d: Duration) -> String {
    let s = d.as_secs();
    let (n, unit) = [(86_400, "d"), (3600, "h"), (60, "m")]
        .into_iter()
        .find(|(u, _)| s > 0 && s.is_multiple_of(*u))
        .unwrap_or((1, "s"));
    format!("{}{unit}", s / n)
}

mod span {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<std::time::Duration, D::Error> {
        super::parse_span(&String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
    pub fn serialize<S: Serializer>(d: &std::time::Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&super::span_text(*d))
    }
}

/// A span, or `"off"`.
mod span_or_off {
    use serde::{Deserialize, Deserializer};
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<std::time::Duration>, D::Error> {
        match String::deserialize(d)?.as_str() {
            "off" => Ok(None),
            s => super::parse_span(s).map(Some).map_err(serde::de::Error::custom),
        }
    }
    pub fn serialize<S: serde::Serializer>(d: &Option<std::time::Duration>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&d.map_or("off".into(), super::span_text))
    }
}

/// A table of spans or `"off"`.
mod spans_or_off {
    use serde::{Deserialize, Deserializer};
    use std::collections::BTreeMap;
    type Spans = BTreeMap<String, Option<std::time::Duration>>;
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Spans, D::Error> {
        // Inserted one by one: collecting would sort what is sorted already.
        let mut spans = Spans::new();
        for (k, v) in BTreeMap::<String, String>::deserialize(d)? {
            let span = match v.as_str() {
                "off" => None,
                s => Some(super::parse_span(s).map_err(|e| serde::de::Error::custom(format!("{k}: {e}")))?),
            };
            spans.insert(k, span);
        }
        Ok(spans)
    }
    pub fn serialize<S: serde::Serializer>(m: &Spans, s: S) -> Result<S::Ok, S::Error> {
        s.collect_map(m.iter().map(|(k, d)| (k, d.map_or("off".into(), super::span_text))))
    }
}

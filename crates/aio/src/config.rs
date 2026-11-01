//! `aio.toml`: an app's one config file. Every key has a visible default;
//! an unknown key is an error (a typo never silently does nothing).
//! Secrets (the sync token) never live here: see `token`.
//!
//! Its types and checks are `aio-config`'s, shared with [`crate::config!`],
//! which parses the file as the app builds; what only a running app needs
//! (where it listens, its token) is here.
pub use aio_config::config::*;

/// The app's config, `aio.toml` parsed and checked as the app builds (a
/// mistake is a compile error at this call): `let cfg = aio::config!();`,
/// the nearest `aio.toml` from the crate's folder up (the file `aio
/// config`, `ax build` and `ax doctor` read too). `aio::config!("x.toml")`
/// names another, from the crate's folder (tests, fixtures). The app holds
/// no TOML parser and reads no config at run time: what varies per launch
/// is a typed flag or variable (`--home`, `--profile`, `--instance`,
/// `--port`, `--host`, `--expose`, `AIORT_DATA`, `AIORT_TOKEN`,
/// `AIORT_UPDATE_CHANNEL`), each said in the boot report with who set it.
#[macro_export]
macro_rules! config {
    ($($file:literal)?) => {{
        use $crate as aio;
        $crate::__config_lit!($($file)?)
    }};
}

/// An older launcher's `build.rs` (`aio::config::build("../aio.toml")`):
/// it still builds, but does nothing now that [`crate::config!`] reads the
/// file itself, and says how to move on.
#[deprecated(
    note = "aio::config!() reads aio.toml itself: delete this build.rs and the [build-dependencies] aio-rs (`ax lint --fix` does both)"
)]
pub fn build(path: &str) {
    println!("cargo::rerun-if-changed={path}");
    println!(
        "cargo::warning=build.rs: aio::config::build is no longer needed (aio::config!() reads {path} itself); \
         run `ax lint --fix`: it deletes this build.rs and the aio-rs build-dependency (one host build of aio less)"
    );
}

/// Where a server listens, decided once (aio's `--port`/`--host`/`--expose`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listen {
    /// `host:port` (port 0: a free one).
    pub addr: String,
    /// Open beyond this machine.
    pub exposed: bool,
    /// Over TLS: `[server] tls`, or a launch that exposes it.
    pub tls: bool,
    /// Who decided, for the boot report.
    pub from: &'static str,
}

/// Where `s` listens for launch `ask` (pure): `--port` and `--host` over
/// `[server] listen` (else `127.0.0.1:0`); `--expose` is `0.0.0.0`. A
/// launch that opens the app to the network (`--expose`, a host not on
/// loopback) turns TLS on: the key never crosses a LAN in the clear.
pub fn listen(s: &Server, ask: &crate::paths::Ask) -> Result<Listen, String> {
    let base = s.listen.as_deref().unwrap_or("127.0.0.1:0");
    let (host, port) =
        base.rsplit_once(':').ok_or_else(|| format!("[server] listen {base:?}: host:port (127.0.0.1:8080)"))?;
    if let Some(p) = &ask.port
        && p.parse::<u16>().is_err()
    {
        return Err(format!("--port {p}: a port number, 0 to 65535 (0: a free one)"));
    }
    if ask.expose && ask.host.is_some() {
        return Err("--expose and --host both say where to listen: give one".into());
    }
    let host = match (&ask.host, ask.expose) {
        (Some(h), _) => h.trim_matches(['[', ']']).to_string(),
        (None, true) => "0.0.0.0".into(),
        (None, false) => host.trim_matches(['[', ']']).to_string(),
    };
    let ip = host.parse::<std::net::IpAddr>().ok();
    let exposed = ip.is_none_or(|ip| !ip.is_loopback()) && host != "localhost";
    let asked = ask.expose || ask.host.is_some();
    let from = match () {
        _ if ask.expose => "--expose",
        _ if ask.host.is_some() => "--host",
        _ if ask.port.is_some() => "--port",
        _ if s.listen.is_some() => "[server] listen",
        _ => "default",
    };
    let port = ask.port.as_deref().unwrap_or(port);
    let addr = if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") };
    Ok(Listen { addr, exposed, tls: s.tls || (asked && exposed), from })
}

/// What to say of `l` at boot: how open it is, and a warning when the
/// network reaches it in the clear.
pub fn listen_says(l: &Listen, id: &str) -> String {
    match (l.exposed, l.tls) {
        (false, _) => format!("{id} listens on {} (this machine only; {})", l.addr, l.from),
        (true, true) => format!("{id} is open to the network on {} over TLS, behind its key ({})", l.addr, l.from),
        (true, false) => format!(
            "security: {id} is open to the network on {} over plain HTTP ({}): its key and state cross the network \
             in the clear. Set [server] tls = true, or serve it behind a TLS proxy (trust_proxy)",
            l.addr, l.from
        ),
    }
}

/// What boot and `ax doctor` say of an app served with `[server] open`.
pub fn open_says(id: &str) -> String {
    format!(
        "security: {id} is OPEN ([server] open = true): anyone who can reach its port reads its state and calls \
         its methods, with no key. If that is not meant, delete `open` (the key is then required)"
    )
}

/// The sync token: `AIORT_TOKEN`, else `data/sync.token` (made once, random,
/// mode 0600). Never in `aio.toml`, never logged. One set by hand that a
/// guesser could find ([`weak_token`]) is said, by where it came from.
pub fn token(data: &std::path::Path) -> std::io::Result<String> {
    let warn = |t: String, from: &str| {
        if weak_token(&t) {
            eprintln!(
                "[aio] security: the app key in {from} is weak (under 16 characters or 80 bits): a guesser can find \
                 it at network speed. Delete it for a random one, or set a long random key"
            );
        }
        t
    };
    if let Ok(t) = std::env::var("AIORT_TOKEN")
        && !t.is_empty()
    {
        return Ok(warn(t, "AIORT_TOKEN"));
    }
    let p = data.join("sync.token");
    if let Ok(t) = std::fs::read_to_string(&p) {
        return Ok(warn(t.trim().to_string(), "data/sync.token"));
    }
    let t = format!("{}{}", crate::uuid_v4().simple(), crate::uuid_v4().simple());
    crate::paths::write_private(&p, t.as_bytes())?;
    Ok(t)
}

/// A key a guesser finds at network speed: under 16 characters, or under
/// 80 bits by the alphabet its characters imply (an upper bound: a word
/// scores as random, so one called weak is weak). Pure.
pub fn weak_token(t: &str) -> bool {
    let n = t.chars().count() as f64;
    let hex = t.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        || t.chars().all(|c| c.is_ascii_digit() || ('A'..='F').contains(&c));
    let has = |f: fn(&char) -> bool| t.chars().any(|c| f(&c));
    let alphabet = [
        (has(char::is_ascii_lowercase), 26.0),
        (has(char::is_ascii_uppercase), 26.0),
        (has(char::is_ascii_digit), 10.0),
        (has(|c| !c.is_ascii_alphanumeric()), 33.0),
    ]
    .iter()
    .filter(|(on, _)| *on)
    .map(|(_, k)| k)
    .sum::<f64>();
    let bits = if hex {
        n * 4.0
    } else if alphabet > 0.0 {
        n * f64::log2(alphabet)
    } else {
        0.0
    };
    n < 16.0 || bits < 80.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_effective_config_reads_back_the_same() {
        let c = Config::parse("[app]\nid = \"todo\"\n[history]\nkeep = \"36h\"\n[updates]\nevery = \"off\"").unwrap();
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!((v["history"]["keep"].as_str(), v["updates"]["every"].as_str()), (Some("36h"), Some("off")));
        assert_eq!(v["accounts"]["session"], "30d");
        assert_eq!(serde_json::from_value::<Config>(v).unwrap(), c);
        assert_eq!([span_text(Duration::from_secs(90)), span_text(Duration::ZERO)], ["90s", "0s"]);
    }

    #[test]
    fn a_key_a_guesser_finds_is_weak() {
        for weak in ["", "secret", "password1234567", "0123456789abcdef", "aaaaaaaaaaaaaaaa"] {
            assert!(weak_token(weak), "{weak}");
        }
        for strong in ["0123456789abcdef0123", "Zx9-qL2!mN7#pR4$", "4f9c2e7a1b3d5f608e2c4a6b8d0f1e3a"] {
            assert!(!weak_token(strong), "{strong}");
        }
        assert!(!weak_token(&format!("{}{}", crate::uuid_v4().simple(), crate::uuid_v4().simple())));
    }

    #[test]
    fn updates_need_https_and_built_in_keys() {
        let c = |u: &str| {
            Config::parse(&format!("[app]\nid = \"todo\"\n[capabilities]\nnetwork = [\"*\"]\n[updates]\n{u}"))
        };
        assert_eq!(c("").unwrap().updates, Updates::default());
        assert!(c("url = \"https://x.example/todo\"").unwrap_err().contains("needs keys"));
        assert!(c("url = \"http://x.example/todo\"\nkeys = [\"ed25519:k\"]").unwrap_err().contains("https"));
        let ok =
            c("url = \"https://x.example/todo\"\nkeys = [\"ed25519:k\"]\nchannel = \"beta\"\nevery = \"1h\"").unwrap();
        assert_eq!((ok.updates.channel.as_str(), ok.updates.every), ("beta", Some(Duration::from_secs(3600))));
        let d = c("").unwrap().updates;
        assert_eq!((d.auto, d.prerelease, d.keep, d.every), (true, None, 3, Some(Duration::from_secs(6 * 3600))));
        let keys = "keys = [\"ed25519:k\"]";
        let u = c(&format!(
            "url = \"github:some_one/app.rs\"\n{keys}\nevery = \"off\"\nauto = false\nprerelease = true\nkeep = 5"
        ))
        .unwrap()
        .updates;
        assert_eq!((u.every, u.auto, u.prerelease, u.keep), (None, false, Some(true), 5));
        assert!(c(&format!("url = \"github:nobody\"\n{keys}")).unwrap_err().contains("names no repository"));
        assert!(c(&format!("url = \"github:a/..\"\n{keys}")).unwrap_err().contains("names no repository"));
        // A repository: its keys must be ones git can check signatures with.
        let (_, key) = crate::update::keygen().unwrap();
        let git = c(&format!("url = \"git+ssh://git@git.example/me/todo\"\nkeys = [\"{key}\"]\nchannel = \"main\""));
        assert_eq!(git.unwrap().updates.channel, "main");
        assert!(c(&format!("url = \"git+https://git.example/todo\"\n{keys}")).unwrap_err().contains("keys: key"));
        assert!(c(&format!("url = \"git+http://git.example/todo\"\n{keys}")).unwrap_err().contains("git+https://"));
        let only = |net: &str, url: &str| {
            Config::parse(&format!(
                "[app]\nid = \"todo\"\n[capabilities]\nnetwork = [{net}]\n[updates]\nurl = \"{url}\"\nkeys = [\"{key}\"]"
            ))
        };
        let e = only("", "git+https://git.example/me/todo.git").unwrap_err();
        assert!(e.contains("reaches git.example: add \"git.example\""), "{e}");
        assert!(only("\"git.example\"", "git+ssh://git@git.example:2222/me/todo").is_ok());
        assert!(only("", "git+file:///srv/todo.git").is_ok(), "a folder on this machine reaches no host");
        assert!(c("keep = 1").unwrap_err().contains("at least 2"));
        assert!(c("every = \"never\"").is_err());
        let u = c("every = \"6h\"\nevery_by_channel = { dev = \"1m\", nightly = \"off\" }").unwrap().updates;
        assert_eq!(u.every_on("dev"), Some(Duration::from_secs(60)));
        assert_eq!((u.every_on("nightly"), u.every_on("stable")), (None, Some(Duration::from_secs(6 * 3600))));
        assert!(c("every_by_channel = { dev = \"soon\" }").is_err());
        assert!(c("channel = \"../x\"").unwrap_err().contains("not a channel name"));
    }

    #[test]
    fn log_level_and_perf_budgets_are_config() {
        let c = Config::parse("[app]\nid = \"t\"").unwrap();
        assert_eq!((c.log.level, c.perf.clone()), (crate::log::Level::Info, crate::perf::Budgets::default()));
        let c = Config::parse(
            "[app]\nid = \"t\"\n[log]\nlevel = \"debug\"\n[perf]\nmethod = 50\n[perf.methods]\n\"t.import\" = 5000",
        )
        .unwrap();
        assert_eq!(c.log, Log { level: crate::log::Level::Debug, ..Log::default() });
        assert_eq!(c.perf.method("t.import"), Duration::from_secs(5));
        let c = Config::parse("[app]\nid = \"t\"\n[budgets]\nstate = \"1MB\"\nbroadcast_rate = \"20/s\"").unwrap();
        assert_eq!((c.budgets.state, c.budgets.broadcast_rate, c.budgets.payload), (Some(1 << 20), Some(20.0), None));
        assert!(Config::parse("[app]\nid = \"t\"\n[budgets]\nstate = \"1 XB\"").unwrap_err().contains("is not a size"));
        let c =
            Config::parse("[app]\nid = \"t\"\n[log]\nkeep = 2\nfile_mb = 1\nheartbeat = 0\nsuppress = [\"t.tick\"]")
                .unwrap();
        assert_eq!((c.log.keep, c.log.file_mb, c.log.heartbeat, c.log.suppress), (2, 1, 0, vec!["t.tick".into()]));
        assert!(Config::parse("[app]\nid = \"t\"\n[log]\nkeep = 0").unwrap_err().contains("at least 1"));
        assert!(Config::parse("[app]\nid = \"t\"\n[log]\nfile_mb = 0").is_err());
        let c = Config::parse("[app]\nid = \"t\"\n[memory]\nhigh = 70\nclear = 60").unwrap();
        assert_eq!(c.memory, Memory { high: 70, clear: 60, ..Memory::default() });
        assert!(Config::parse("[app]\nid = \"t\"\n[memory]\nhihg = 70").is_err(), "a typo is an error");
        let f = |t: &str| {
            Config::parse(&format!("[app]\nid = \"t\"\n[capabilities]\nnetwork = [\"r.example\"]\n[feedback]\n{t}"))
                .map(|c| c.feedback)
        };
        assert_eq!(f("").unwrap(), Feedback::default());
        let set = f("url = \"https://r.example/in\"\ntimeout = \"5s\"\nkeep = 7\nauto = false").unwrap();
        assert_eq!(
            set,
            Feedback {
                url: Some("https://r.example/in".into()),
                timeout: Duration::from_secs(5),
                keep: 7,
                auto: false
            }
        );
        assert!(f("url = \"http://r.example/in\"").unwrap_err().contains("https"), "never in the clear");
        assert!(f("url = \"http://127.0.0.1:9/in\"").is_ok(), "a test's own loopback");
        assert!(f("keep = 0").unwrap_err().contains("at least 1"));
        assert!(f("kep = 3").is_err(), "a typo is an error");
        assert!(Config::parse("[app]\nid = \"t\"\n[memory]\nhigh = 90").unwrap_err().contains("high <= critical"));
        assert!(Config::parse("[app]\nid = \"t\"\n[memory]\nclear = 50").is_err(), "clear below high");
        assert!(Config::parse("[app]\nid = \"t\"\n[log]\nlevle = \"debug\"").is_err());
        assert!(Config::parse("[app]\nid = \"t\"\n[perf]\nreduce = 5").is_err());
    }

    #[test]
    fn the_look_its_hue_and_layout_are_config() {
        let c = Config::parse("[app]\nid = \"t\"").unwrap();
        assert_eq!(c.ui, Ui { theme: Look::Auto, hue: None, layout: true });
        let c = Config::parse("[app]\nid = \"t\"\n[ui]\ntheme = \"full\"\nhue = 210\nlayout = false").unwrap();
        assert_eq!(c.ui, Ui { theme: Look::Full, hue: Some(210), layout: false });
        for t in ["tokens", "none"] {
            assert!(Config::parse(&format!("[app]\nid = \"t\"\n[ui]\ntheme = \"{t}\"")).is_ok());
        }
        assert!(Config::parse("[app]\nid = \"t\"\n[ui]\ntheme = \"base\"").is_err());
        assert!(Config::parse("[app]\nid = \"t\"\n[ui]\nhue = 360").unwrap_err().contains("0 to 359"));
        assert!(Config::parse("[app]\nid = \"t\"\n[ui]\nheu = 3").is_err(), "a typo is an error");
    }

    #[test]
    fn the_pages_language_and_direction_are_config_never_guessed() {
        let c = Config::parse("[app]\nid = \"t\"").unwrap();
        assert_eq!((c.app.lang, c.app.dir), (None, Dir::Ltr));
        let c = Config::parse("[app]\nid = \"t\"\nlang = \"ar\"").unwrap();
        assert_eq!((c.app.lang.as_deref(), c.app.dir), (Some("ar"), Dir::Ltr), "ar alone is not rtl");
        for (d, want) in [("rtl", Dir::Rtl), ("auto", Dir::Auto), ("ltr", Dir::Ltr)] {
            assert_eq!(Config::parse(&format!("[app]\nid = \"t\"\ndir = \"{d}\"")).unwrap().app.dir, want);
        }
        assert!(Config::parse("[app]\nid = \"t\"\ndir = \"down\"").is_err());
        for bad in ["", " ", "en US", "x><script>", "pt-"] {
            let e = Config::parse(&format!("[app]\nid = \"t\"\nlang = \"{bad}\"")).unwrap_err();
            assert!(e.contains("[app] lang"), "{bad:?}: {e}");
        }
        assert!(Config::parse("[app]\nid = \"t\"\nlang = \"pt-BR\"").is_ok());
    }

    #[test]
    fn defaults_are_standalone_and_90_days() {
        let c = Config::parse("[app]\nid = \"todo\"").unwrap();
        assert_eq!((c.app.mode, c.history.keep), (Mode::Standalone, Duration::from_secs(90 * 86_400)));
    }

    #[test]
    fn modes_need_their_addresses() {
        let c = Config::parse("[app]\nid = \"t\"\nmode = \"client-server\"\n[server]\nlisten = \"0.0.0.0:7420\"\ndiscover = true\n[history]\nkeep = \"12h\"").unwrap();
        assert_eq!((c.app.mode, c.history.keep), (Mode::ClientServer, Duration::from_secs(12 * 3600)));
        assert_eq!((c.server.discover, c.server.pairing), (true, false));
        let c = Config::parse("[app]\nid = \"t\"\n[data]\ndurability = \"full\"").unwrap();
        assert_eq!(c.data.durability, Durability::Full);
        assert!(Config::parse("[app]\nid = \"t\"\n[data]\ndurability = \"max\"").is_err());
        assert!(Config::parse("[app]\nid = \"t\"\nmode = \"server\"").unwrap_err().contains("[server] listen"));
    }

    #[test]
    fn a_browser_ui_is_a_served_apps() {
        let at = "\n[server]\nlisten = \"127.0.0.1:7420\"";
        let c = Config::parse(&format!("[app]\nid = \"t\"\nmode = \"client-server\"\nui = \"browser\"{at}")).unwrap();
        assert_eq!(c.app.ui, Shown::Browser);
        assert_eq!(Config::parse("[app]\nid = \"t\"").unwrap().app.ui, Shown::Window, "the default");
        let e = Config::parse(&format!("[app]\nid = \"t\"\nmode = \"server\"\nui = \"browser\"{at}")).unwrap_err();
        assert!(e.contains("needs mode \"client-server\""), "{e}");
        assert!(Config::parse("[app]\nid = \"t\"\nui = \"tab\"").is_err());
    }

    #[test]
    fn store_pragmas_are_the_apps_but_never_the_stores_own() {
        let toml = |p: &str| Config::parse(&format!("[app]\nid = \"t\"\n[data]\npragmas = [{p}]"));
        assert_eq!(toml("\"PRAGMA cache_size = -16000\"").unwrap().data.pragmas, ["PRAGMA cache_size = -16000"]);
        assert!(toml("\"PRAGMA main.Synchronous = OFF\"").unwrap_err().contains("set [data] durability"));
        for own in ["journal_mode = DELETE", "user_version = 9"] {
            let e = toml(&format!("\"PRAGMA {own}\"")).unwrap_err();
            assert!(e.contains("the store relies on it"), "{e}");
        }
        assert!(toml("\"DROP TABLE log\"").unwrap_err().contains("is not a PRAGMA statement"));
    }

    #[test]
    fn a_store_name_and_a_ported_apps_home_rules() {
        assert_eq!(
            (Config::parse("[app]\nid = \"t\"").unwrap().data.store, Data::default().store),
            ("state.db".into(), "state.db".into())
        );
        let c = Config::parse(
            "[app]\nid = \"t\"\nport_of = \"purse\"\n[data]\nstore = \"vault.db\"\nretire = [\"state.db\"]\n[home]\nenv = \"PURSE_HOME\"\nportable = \"purse-data\"\ndev = \".purse-dev\"",
        )
        .unwrap();
        assert_eq!((c.data.store.as_str(), &c.data.retire[..]), ("vault.db", &["state.db".to_string()][..]));
        assert_eq!(c.home.env.as_deref(), Some("PURSE_HOME"));
        let bad = |extra: &str| Config::parse(&format!("[app]\nid = \"t\"\nport_of = \"r\"\n{extra}")).unwrap_err();
        for (extra, why) in [
            ("[data]\nstore = \"../x.db\"", "a file name"),
            ("[data]\nstore = \"\"", "a file name"),
            ("[data]\nretire = [\"a/b\"]", "a file name"),
            ("[home]\ndev = \"/abs\"", "a file name"),
            ("[home]\nportable = \"..\"", "a file name"),
            ("[data]\nretire = [\"state.db\"]", "the store"),
            ("[home]\nenv = \"\"", "variable name"),
            ("[home]\nhome = \"x\"", "unknown field"),
        ] {
            assert!(bad(extra).contains(why), "{extra}: {}", bad(extra));
        }
        assert!(Config::parse("[app]\nid = \"t\"\n[home]\nenv = \"X\"").unwrap_err().contains("needs [app] port_of"));
    }

    #[test]
    fn a_clients_retention_and_call_timeouts_are_config() {
        let c = |s: &str| Config::parse(&format!("[app]\nid = \"t\"\n[client]\n{s}"));
        let off = c("").unwrap().client;
        assert_eq!((off.retention, off.timeouts.len()), (None, 0));
        assert_eq!(c("retention = \"off\"").unwrap().client.retention, None);
        let on = c("retention = \"7d\"\n[client.timeouts]\n\"t.pay\" = \"60s\"\n\"t.sync\" = \"off\"").unwrap().client;
        assert_eq!(on.retention, Some(Duration::from_secs(7 * 86_400)));
        assert_eq!((on.timeouts["t.pay"], on.timeouts["t.sync"]), (Some(Duration::from_secs(60)), None));
        // A retention it can't honour speaks (aio used to fall back to 4h).
        assert!(c("retention = \"7days\"").unwrap_err().contains("7days"));
        assert!(c("[client.timeouts]\n\"t.pay\" = \"1 min\"").unwrap_err().contains("t.pay"));
    }

    #[test]
    fn served_dirs_are_prefixes_and_folders_that_stay_put() {
        let c = |s: &str| Config::parse(&format!("[app]\nid = \"t\"\n{s}"));
        let s = c("[server.dirs]\n\"/media\" = \"media\"\n\"/srv/\" = \"/srv/www\"\n[server.public_dirs]\n\"/logo\" = \"brand/logo\"")
            .unwrap()
            .server;
        assert_eq!(s.dirs["/media"], std::path::Path::new("media"));
        assert_eq!(s.public_dirs["/logo"], std::path::Path::new("brand/logo"));
        assert!(c("[server.public_dirs]\n\"/.well-known\" = \"acme\"").is_ok());
        for bad in [
            "\"/\" = \"a\"",
            "\"media\" = \"a\"",
            "\"/ws\" = \"a\"",
            "\"/__aio/x\" = \"a\"",
            "\"/a//b\" = \"a\"",
            "\"/.git\" = \"a\"",
        ] {
            assert!(c(&format!("[server.dirs]\n{bad}")).unwrap_err().contains("not a prefix"), "{bad}");
        }
        for bad in ["\"\"", "\"../data\"", "\"a/../../x\""] {
            assert!(c(&format!("[server.dirs]\n\"/m\" = {bad}")).unwrap_err().contains("without .."), "{bad}");
        }
        let twice = c("[server.dirs]\n\"/m\" = \"a\"\n[server.public_dirs]\n\"/m/\" = \"b\"").unwrap_err();
        assert!(twice.contains("\"/m\" is served twice"), "{twice}");
        assert!(c("[server.dir]\n\"/m\" = \"a\"").is_err(), "a typo is an error");
    }

    #[test]
    fn proxy_trust_names_a_header() {
        let c = |s: &str| Config::parse(&format!("[app]\nid = \"t\"\n[server]\n{s}"));
        let off = c("").unwrap().server;
        assert_eq!((off.strict_origin, off.trust_proxy), (false, None));
        let on = c("strict_origin = true\ntrust_proxy = \"x-forwarded-for\"").unwrap().server;
        assert_eq!((on.strict_origin, on.trust_proxy.as_deref()), (true, Some("x-forwarded-for")));
        assert_eq!(off.limits, Limits::default());
        let tuned = c("[server.limits]\nclients = 8\nbytes_per_sec = 1000").unwrap().server.limits;
        assert_eq!(tuned, Limits { clients: 8, bytes_per_sec: 1000, ..Limits::default() });
        assert!(c("[server.limits]\nclient = 8").is_err(), "a typo is an error");
        for bad in ["", "x-forwarded-for ", "a:b", "[object Object]"] {
            assert!(c(&format!("trust_proxy = {bad:?}")).unwrap_err().contains("not a header name"), "{bad}");
        }
    }

    #[test]
    fn a_launch_says_where_to_listen_and_exposing_turns_tls_on() {
        let s =
            |listen: Option<&str>, tls: bool| Server { listen: listen.map(String::from), tls, ..Default::default() };
        let ask = |a: &[&str]| crate::paths::Ask::parse(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap();
        let l = |s: &Server, a: &[&str]| listen(s, &ask(a)).map(|l| (l.addr, l.exposed, l.tls, l.from));
        let o = |a: &str, e, t, f| Ok((a.to_string(), e, t, f));
        assert_eq!(l(&s(None, false), &[]), o("127.0.0.1:0", false, false, "default"));
        assert_eq!(l(&s(Some("127.0.0.1:8080"), false), &[]), o("127.0.0.1:8080", false, false, "[server] listen"));
        assert_eq!(l(&s(Some("127.0.0.1:8080"), false), &["--port=9000"]), o("127.0.0.1:9000", false, false, "--port"));
        assert_eq!(l(&s(Some("127.0.0.1:8080"), false), &["--expose"]), o("0.0.0.0:8080", true, true, "--expose"));
        assert_eq!(
            l(&s(None, false), &["--host", "192.168.1.5", "--port", "1"]),
            o("192.168.1.5:1", true, true, "--host")
        );
        assert_eq!(l(&s(None, false), &["--host=::1"]), o("[::1]:0", false, false, "--host"));
        assert!(!l(&s(None, false), &["--host=localhost"]).unwrap().1);
        // Exposed by the author's own config: said loudly, never changed under them.
        let open = listen(&s(Some("0.0.0.0:8080"), false), &ask(&[])).unwrap();
        assert!((open.exposed, open.tls) == (true, false) && listen_says(&open, "notes").contains("plain HTTP"));
        let safe = listen(&s(Some("0.0.0.0:8080"), true), &ask(&[])).unwrap();
        assert!(listen_says(&safe, "notes").contains("over TLS, behind its key"));
        assert!(listen_says(&listen(&s(None, false), &ask(&[])).unwrap(), "notes").contains("this machine only"));
        assert!(l(&s(None, false), &["--port=http"]).unwrap_err().contains("a port number"));
        assert!(l(&s(None, false), &["--expose", "--host=203.0.113.4"]).unwrap_err().contains("give one"));
    }

    #[test]
    fn the_hosts_aio_toml_names_must_be_in_its_network() {
        let c = |s: &str| Config::parse(&format!("[app]\nid = \"t\"\n{s}"));
        let keys = "keys = [\"ed25519:k\"]";
        let e = c(&format!("[updates]\nurl = \"https://dl.example.org/t\"\n{keys}")).unwrap_err();
        assert_eq!(
            e,
            "aio.toml: [updates] url reaches dl.example.org: add \"dl.example.org\" to [capabilities] network"
        );
        assert!(
            c(&format!(
                "[capabilities]\nnetwork = [\"dl.example.org\"]\n[updates]\nurl = \"https://dl.example.org/t\"\n{keys}"
            ))
            .is_ok()
        );
        // GitHub serves the files from its download hosts.
        let gh = |n: &str| c(&format!("[capabilities]\nnetwork = {n}\n[updates]\nurl = \"github:me/t\"\n{keys}"));
        assert!(gh("[\"github.com\"]").unwrap_err().contains("add \"*.githubusercontent.com\""));
        assert!(gh("[\"github.com\", \"*.githubusercontent.com\"]").is_ok());
        let e = c("[client]\nserver = \"10.0.0.5:8443\"").unwrap_err();
        assert!(e.contains("[client] server reaches 10.0.0.5"), "{e}");
        assert!(c("[capabilities]\nnetwork = [\"lan\"]\n[client]\nserver = \"10.0.0.5:8443\"").is_ok());
        assert!(c("[client]\nserver = \"127.0.0.1:8443\"").is_ok(), "this machine always");
        assert!(c("[capabilities]\nnetwork = [\"https://x.example\"]").unwrap_err().contains("is not a host"));
    }

    #[test]
    fn open_mode_is_an_explicit_choice_said_loudly() {
        let c = |s: &str| Config::parse(&format!("[app]\nid = \"t\"\n{s}"));
        assert!(!c("").unwrap().server.open, "the key is required by default");
        assert!(c("[server]\nopen = true").unwrap().server.open);
        assert!(c("[server]\nopen = true\n[accounts]\non = true").unwrap_err().contains("give one"));
        assert!(open_says("notes").starts_with("security: notes is OPEN"));
    }

    #[test]
    fn security_and_an_own_certificate_are_checked_at_parse() {
        use crate::security::{Directive, Hsts};
        let c = |s: &str| Config::parse(&format!("[app]\nid = \"t\"\n[server]\n{s}"));
        assert_eq!(c("").unwrap().server.security, crate::security::Security::default());
        let s = c("[server.security]\ncsp = \"strict\"\nnonce = true\nhsts = \"max-age=60\"\n\
                   directives = { \"base-uri\" = false, \"img-src\" = \"'self' https:\" }")
        .unwrap()
        .server
        .security;
        assert_eq!((s.csp.as_str(), s.nonce, &s.hsts), ("strict", true, &Hsts::Value("max-age=60".into())));
        assert_eq!(s.directives["base-uri"], Directive::Off(false));
        assert!(c("[server.security]\nreferrer = \"no-referrer\\n\"").unwrap_err().contains("HTTP header"));
        assert!(
            c("[server.security]\ndirectives = { \"img-src\" = \"{nonce}\" }").unwrap_err().contains("nonce is off")
        );
        assert!(c("[server.security]\ncsps = \"strict\"").is_err(), "a typo is an error");
        assert!(c("tls = true\ncert = \"/c.pem\"").unwrap_err().contains("go together"));
        assert!(c("cert = \"/c.pem\"\nkey = \"/k.pem\"").unwrap_err().contains("need tls = true"));
        assert!(c("tls = true\ncert = \"c.pem\"\nkey = \"k.pem\"").unwrap_err().contains("absolute path"));
        #[cfg(unix)]
        {
            let own = c("tls = true\ncert = \"/etc/app/chain.pem\"\nkey = \"/etc/app/key.pem\"").unwrap().server;
            assert_eq!(own.cert.as_deref(), Some(std::path::Path::new("/etc/app/chain.pem")));
        }
    }

    #[test]
    fn accounts_are_off_and_bounded() {
        let base = "[app]\nid = \"t\"\n";
        let c = Config::parse(base).unwrap();
        assert_eq!((c.accounts.on, c.accounts.policy().ttl_ms), (false, 30 * 86_400_000));
        let on = Config::parse(&format!("{base}[accounts]\non = true\nsession = \"12h\"\nmax_sessions = 3")).unwrap();
        let p = on.accounts.policy();
        assert_eq!((p.ttl_ms, p.max_sessions, p.signup, p.require_verified), (12 * 3_600_000, Some(3), false, false));
        let strict = Config::parse(&format!("{base}[accounts]\non = true\nrequire_verified = true")).unwrap();
        assert!(strict.accounts.policy().require_verified);
        assert_eq!(strict.accounts.policy().max_sessions, Some(10), "bounded unless the app says");
        assert_eq!(c.accounts.policy().max_sessions, Some(10));
        assert!(Config::parse(&format!("{base}[accounts]\non = true\nsession = \"0s\"")).is_err());
        assert!(Config::parse(&format!("{base}[accounts]\non = true\nmax_sessions = 0")).is_err());
        assert!(Config::parse(&format!("{base}[accounts]\nuser = \"x\"")).is_err());
    }

    #[test]
    fn typos_and_bad_values_are_errors() {
        assert!(Config::parse("[app]\nid = \"t\"\nmdoe = \"server\"").unwrap_err().contains("unknown field"));
        assert!(Config::parse("[app]\nid = \"../x\"").is_err());
        assert_eq!(Config::parse("[app]\nid = \"t\"\nport_of = \"notes\"").unwrap().app.port_of.as_deref(), Some("notes"));
        assert!(Config::parse("[app]\nid = \"t\"\nport_of = \"../x\"").unwrap_err().contains("[app] port_of"));
        assert!(Config::parse("[app]\nid = \"t\"\n[history]\nkeep = \"3w\"").is_err());
        assert!(parse_span("10").is_err());
    }

    #[test]
    fn build_targets_are_kinds_or_labelled_in_order() {
        let c = |t: &str| Config::parse(&format!("[app]\nid = \"t\"\n[build]\ntargets = {t}"));
        assert_eq!(Config::parse("[app]\nid = \"t\"").unwrap().build.targets, vec![]);
        let t = c(r#"["desktop", { name = " pro ", kind = "desktop", bin = "notes-pro" }, { name = "deb" }]"#)
            .unwrap()
            .build
            .targets;
        let got: Vec<_> = t.iter().map(|t| (t.name.as_str(), t.kind.as_deref(), t.bin.as_deref())).collect();
        assert_eq!(got, [("desktop", None, None), ("pro", Some("desktop"), Some("notes-pro")), ("deb", None, None)]);
        assert!(c(r#"[{ name = "pro", knd = "desktop" }]"#).unwrap_err().contains("unknown field"));
        assert!(c("[1]").unwrap_err().contains("expected [build] targets: a kind or { name, kind, bin }"));
    }

    #[test]
    fn a_heavy_app_declares_its_own_size_budget() {
        let c = |t: &str| Config::parse(&format!("[app]\nid = \"t\"\n[build]\n{t}"));
        assert_eq!(c("").unwrap().build.budget_mb, None);
        assert_eq!(c("budget_mb = 25").unwrap().build.budget_mb, Some(25));
        assert!(c("budget_mb = \"25 MB\"").is_err() && c("budget_mb = -1").is_err());
        assert!(!c("").unwrap().build.smoke && c("smoke = true").unwrap().build.smoke, "smoke on every build");
        assert!(c("").unwrap().build.platforms.is_empty());
        assert_eq!(c("platforms = [\"linux\", \"windows\"]").unwrap().build.platforms, ["linux", "windows"]);
        assert!(c("platforms = \"linux\"").is_err(), "a list");
    }

    #[test]
    fn the_macos_bundle_id_is_declared_and_checked_never_rewritten() {
        let c = |t: &str| Config::parse(&format!("[app]\nid = \"notes\"\n[build.macos]\n{t}"));
        assert_eq!(c("").unwrap().mac_bundle_id(), "aio.rt.notes");
        assert_eq!(c("bundle_id = \"com.example.my-notes\"").unwrap().mac_bundle_id(), "com.example.my-notes");
        for bad in ["notes", "com.acme.My App", "com..notes", "com.acme_x.notes", ".com.notes"] {
            let e = c(&format!("bundle_id = {bad:?}")).unwrap_err();
            assert!(e.contains("[build.macos] bundle_id") && e.contains("used as written"), "{bad}: {e}");
        }
        assert!(c("bundle = \"com.a.b\"").is_err(), "unknown keys are refused");
        let pinned = c("host = \"-p 2222 me@mac-mini\"").unwrap();
        assert_eq!(pinned.build.macos.host.as_deref(), Some("-p 2222 me@mac-mini"));
        assert!(c("host = \" \"").unwrap_err().contains("[build.macos] host names no Mac"));
    }

    #[test]
    fn the_android_identity_and_key_are_declared_and_checked() {
        let c = |t: &str| Config::parse(&format!("[app]\nid = \"t\"\n[build.android]\n{t}"));
        assert_eq!(Config::parse("[app]\nid = \"t\"").unwrap().build.android, AndroidBuild::default());
        let a = c("application_id = \"com.example.notes\"\nkeystore = \"keys/up.jks\"\nkey_alias = \"up\"\naab = true")
            .unwrap()
            .build
            .android;
        assert_eq!(a.application_id.as_deref(), Some("com.example.notes"));
        assert_eq!((a.keystore.as_deref(), a.key_alias.as_deref(), a.aab), (Some("keys/up.jks"), Some("up"), true));
        for bad in ["notes", "com.3d.viewer", "com..x", "com.ex-ample"] {
            assert!(
                c(&format!("application_id = \"{bad}\"")).unwrap_err().contains("not an Android package name"),
                "{bad}"
            );
        }
        assert!(c("keystore_password = \"x\"").unwrap_err().contains("unknown field"), "never a password");
    }

    #[test]
    fn the_android_schemes_camera_and_colour_scheme_are_declared_and_checked() {
        let c = |t: &str| Config::parse(&format!("[app]\nid = \"t\"\n[build.android]\n{t}"));
        let a = c("schemes = [\"myapp\", \"my-app.b2+x\"]\ncamera = true\ncolor_scheme = \"dark\"").unwrap().build.android;
        assert_eq!((a.schemes, a.camera, a.color_scheme), (vec!["myapp".into(), "my-app.b2+x".into()], true, ColorScheme::Dark));
        let none = AndroidBuild::default();
        assert_eq!((none.schemes.len(), none.camera, none.color_scheme), (0, false, ColorScheme::System), "all opt-in");
        for bad in ["MyApp", "1app", "my app", "", "my_app"] {
            let e = c(&format!("schemes = [\"{bad}\"]")).unwrap_err();
            assert!(e.contains("[build.android] schemes") && e.contains("is not a URL scheme"), "{bad}: {e}");
        }
        for system in ["https", "intent", "javascript", "content", "file"] {
            let e = c(&format!("schemes = [\"{system}\"]")).unwrap_err();
            assert!(e.contains("is the system's"), "{system}: {e}");
        }
        assert!(c("camera = \"yes\"").unwrap_err().contains("invalid type"), "never coerced");
        assert!(c("color_scheme = \"night\"").unwrap_err().contains("unknown variant"));
    }

    #[test]
    fn an_offline_build_is_asked_for_by_name_and_a_typo_is_refused() {
        let c = |t: &str| Config::parse(&format!("[app]\nid = \"t\"\n[build]\n{t}"));
        let b = Config::parse("[app]\nid = \"t\"").unwrap().build;
        assert_eq!((b.runtime, b.chromium_extras), (RuntimeFrom::Shared, Extras::Keep));
        let b = c("runtime = \"bundled\"\nchromium_extras = \"strip\"").unwrap().build;
        assert_eq!((b.runtime, b.chromium_extras), (RuntimeFrom::Bundled, Extras::Strip));
        assert!(c("runtime = \"bundle\"").unwrap_err().contains("unknown variant"));
        assert!(c("chromium_extras = \"none\"").unwrap_err().contains("unknown variant"));
    }

    #[test]
    fn the_css_step_is_argv_never_a_shell_and_a_bad_one_is_refused() {
        let c = |t: &str| Config::parse(&format!("[app]\nid = \"t\"\n[build]\ncss = {t}"));
        assert_eq!(Config::parse("[app]\nid = \"t\"").unwrap().build.css, None);
        let argv = |t: &str| c(t).unwrap().build.css.unwrap().0;
        assert_eq!(
            argv(r#"" tailwindcss  -i src/app.css -o assets/style.css ""#),
            ["tailwindcss", "-i", "src/app.css", "-o", "assets/style.css"]
        );
        assert_eq!(argv(r#"["sass", "a b.scss", "out.css"]"#), ["sass", "a b.scss", "out.css"]);
        for bad in [r#""  ""#, "[]", r#"["sass", ""]"#, r#"["sass", 1]"#, "3"] {
            assert!(c(bad).unwrap_err().contains("[build] css"), "{bad}");
        }
    }

    #[test]
    fn the_token_is_made_once_and_private() {
        let d = std::env::temp_dir().join(format!("aio-token-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let a = token(&d).unwrap();
        assert_eq!(a.len(), 64);
        assert_eq!(token(&d).unwrap(), a);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(d.join("sync.token")).unwrap().permissions().mode() & 0o777, 0o600);
        }
        std::fs::remove_dir_all(&d).unwrap();
    }
}

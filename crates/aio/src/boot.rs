//! An app's start: its log, problem reports and health monitor begin,
//! and the boot report says every setting and who decided it: the app's
//! `aio.toml`, an `AIORT_*` variable, a file in its data, or aio's default.
//! A setting is left out only when it doesn't exist (no server: no `listen`).
use crate::config::{Config, Mode};
use std::path::Path;

/// One boot report row: `listen = 127.0.0.1:7700 — loopback only (aio.toml)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub key: &'static str,
    pub value: String,
    pub from: &'static str,
}

fn row(key: &'static str, value: impl Into<String>, from: &'static str) -> Row {
    Row { key, value: value.into(), from }
}

/// The boot report (pure: `env` reads a variable, `exists` a file).
pub fn rows(
    cfg: &Config,
    version: &str,
    dirs: &crate::paths::AppDirs,
    env: &dyn Fn(&str) -> Option<String>,
    exists: &dyn Fn(&Path) -> bool,
) -> Vec<Row> {
    let set = |k: &str| env(k).filter(|v| !v.is_empty());
    let mut r = vec![
        row("app", &cfg.app.id, "aio.toml"),
        row("version", version, "Cargo.toml"),
        row("aio-rs", env!("CARGO_PKG_VERSION"), "built in"),
        row("config", "aio.toml built into the app", "aio::config!()"),
        row("mode", mode(cfg.app.mode), "aio.toml"),
        row("data", dirs.data.display().to_string(), dirs.from),
        row("logs", dirs.logs.display().to_string(), dirs.from),
        row("history", format!("{} days", cfg.history.keep.as_secs() / 86_400), "aio.toml"),
        // Phones always save hard (`Source::for_app`): the OS may end the app.
        if cfg!(any(target_os = "android", target_os = "ios")) {
            row("durability", String::from("full"), "phone")
        } else {
            row("durability", format!("{:?}", cfg.data.durability).to_lowercase(), "aio.toml")
        },
    ];
    if let Some(p) = &dirs.profile {
        r.push(row("profile", p, dirs.from));
    }
    if let Some(app) = &dirs.port_of {
        r.push(row("port_of", format!("{app}'s home and {}, kept readable by aio", cfg.data.store), "aio.toml"));
    }
    let token = || match set("AIORT_TOKEN") {
        Some(_) => row("token", "set", "AIORT_TOKEN"),
        None if exists(&dirs.data.join("sync.token")) => row("token", "data/sync.token", "file"),
        None => row("token", "data/sync.token (new)", "default"),
    };
    if matches!(cfg.app.mode, Mode::ClientServer | Mode::Server) {
        let listen = cfg.server.listen.clone().unwrap_or_default();
        r.push(row("listen", format!("{listen} — {}", reach(&listen)), "aio.toml"));
        r.push(row("tls", on(cfg.server.tls), "aio.toml"));
        r.push(row("pairing", on(cfg.server.pairing), "aio.toml"));
        r.push(row("discover", on(cfg.server.discover), "aio.toml"));
        if !cfg.server.hosts.is_empty() {
            r.push(row("hosts", cfg.server.hosts.join(", "), "aio.toml"));
        }
        r.push(row("strict origin", on(cfg.server.strict_origin), "aio.toml"));
        if let Some(h) = &cfg.server.trust_proxy {
            r.push(row("trusted proxy", format!("client = last hop of {h}"), "aio.toml"));
        }
        let listed = |m: &std::collections::BTreeMap<String, std::path::PathBuf>| {
            m.iter().map(|(p, d)| format!("{p} → {}", d.display())).collect::<Vec<_>>().join(", ")
        };
        if !cfg.server.dirs.is_empty() {
            r.push(row("dirs", listed(&cfg.server.dirs), "aio.toml"));
        }
        if !cfg.server.public_dirs.is_empty() {
            r.push(row("public dirs", listed(&cfg.server.public_dirs), "aio.toml"));
        }
        r.push(match cfg.server.open {
            true => row("key", "OPEN: none asked, anyone who reaches it reads and calls", "aio.toml"),
            false => token(),
        });
    }
    if cfg.app.mode == Mode::Client {
        if exists(&dirs.data.join("paired.json")) {
            r.push(row("server", "data/paired.json", "ax pair"));
        } else {
            r.push(row("server", cfg.client.server.clone().unwrap_or_default(), "aio.toml"));
            r.push(token());
        }
    }
    r
}

/// Who can reach `listen`, in words (the security posture).
fn reach(listen: &str) -> &'static str {
    match listen.parse::<std::net::SocketAddr>().map(|a| a.ip()) {
        Ok(ip) if ip.is_loopback() => "loopback only",
        Ok(ip) if ip.is_unspecified() => "every interface: open to the network",
        Ok(_) => "that address's network",
        Err(_) => "as the name resolves",
    }
}

/// As `aio.toml` spells it.
fn mode(m: Mode) -> &'static str {
    match m {
        Mode::Standalone => "standalone",
        Mode::Client => "client",
        Mode::ClientServer => "client-server",
        Mode::Server => "server",
        Mode::Cli => "cli",
    }
}

fn on(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

/// Print rows as the log's boot report.
pub fn say(rows: &[Row]) {
    for r in rows {
        eprintln!("[aio] boot {:<10} {} ({})", r.key, r.value, r.from);
    }
}

/// `AIORT_CONFIG` (a whole-file config swap), refused, never ignored:
/// an app's config is the `aio.toml` it was built with (pure: `env` reads
/// a variable).
pub fn no_config_file(env: &dyn Fn(&str) -> Option<String>) -> std::io::Result<()> {
    match env("AIORT_CONFIG").filter(|v| !v.is_empty()) {
        None => Ok(()),
        Some(p) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "AIORT_CONFIG {p}: an app reads only the aio.toml it was built with (aio::config!()); \
                 per launch: --home, --profile, --instance, --port, --host, --expose, AIORT_DATA, AIORT_TOKEN, \
                 AIORT_UPDATE_CHANNEL. Unset it"
            ),
        )),
    }
}

/// Start the app's log, problem reports and health, then say how it booted.
/// Call once, first thing after the app's lock is taken.
pub fn begin(cfg: &Config, version: &str, dirs: &crate::paths::AppDirs) -> std::io::Result<()> {
    no_config_file(&|k| std::env::var(k).ok())?;
    crate::log::configure(&cfg.log);
    crate::log::start(&dirs.logs)?;
    crate::paths::booted(dirs);
    crate::perf::set(cfg.perf.clone());
    crate::cron::set_local(cfg.app.tz == crate::config::Tz::Local);
    crate::vitals::set(cfg.vitals);
    crate::capabilities::set(&cfg.capabilities.network);
    crate::budgets::declare(cfg.budgets);
    crate::ui::set_look(cfg.ui.theme);
    crate::perf::start(&dirs.logs)?;
    crate::report::init(crate::report::About {
        id: cfg.app.id.clone(),
        version: version.into(),
        dir: dirs.data.join("reports"),
        log: dirs.logs.join("app.log"),
        channel: cfg.updates.channel.clone(),
        keep: cfg.feedback.keep,
        auto: cfg.feedback.auto,
    });
    crate::report::on_panic();
    crate::feedback::open();
    eprintln!("[aio] {} {version} starting (pid {})", cfg.app.id, std::process::id());
    say(&rows(cfg, version, dirs, &|k| std::env::var(k).ok(), &|p| p.exists()));
    crate::health::start(&dirs.data, cfg.memory);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(root: &Path) -> crate::paths::AppDirs {
        crate::paths::AppDirs::at(root, "notes").unwrap()
    }

    fn find<'a>(r: &'a [Row], k: &str) -> Option<&'a Row> {
        r.iter().find(|x| x.key == k)
    }

    #[test]
    fn every_setting_says_who_decided_it() {
        let cfg = Config::parse("[app]\nid = \"notes\"").unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let d = dirs(tmp.path());
        let r = rows(&cfg, "1.2.0", &d, &|_| None, &|_| false);
        assert_eq!(find(&r, "config").unwrap().from, "aio::config!()");
        let set = |k: &str| (k == "AIORT_CONFIG").then(|| "/x.toml".to_string());
        assert!(no_config_file(&set).unwrap_err().to_string().starts_with("AIORT_CONFIG /x.toml: an app reads only"));
        assert!(no_config_file(&|_| Some(String::new())).is_ok() && no_config_file(&|_| None).is_ok());
        assert_eq!(find(&r, "data").unwrap(), &row("data", d.data.display().to_string(), "AppDirs::at"));
        assert!(find(&r, "profile").is_none() && find(&r, "port_of").is_none());
        assert_eq!(find(&r, "mode").unwrap().value, "standalone");
        assert_eq!(find(&r, "version").unwrap().value, "1.2.0");
        assert!(find(&r, "listen").is_none(), "no server: no listen row");

        let cfg = Config::parse(
            "[app]\nid = \"notes\"\nmode = \"client-server\"\n[server]\nlisten = \"0.0.0.0:7700\"\ntls = true\n[server.dirs]\n\"/media\" = \"media\"",
        )
        .unwrap();
        let env = |k: &str| (k == "AIORT_TOKEN").then(|| "x".to_string());
        let r = rows(&cfg, "1", &d, &env, &|_| false);
        assert_eq!(find(&r, "mode").unwrap().value, "client-server");
        assert_eq!(find(&r, "listen").unwrap().value, "0.0.0.0:7700 — every interface: open to the network");
        assert_eq!(find(&r, "tls").unwrap().value, "on");
        assert_eq!(find(&r, "dirs").unwrap().value, "/media → media");
        assert!(find(&r, "public dirs").is_none());
        assert_eq!(find(&r, "token").unwrap(), &row("token", "set", "AIORT_TOKEN"), "never the token itself");
        assert_eq!(reach("127.0.0.1:1"), "loopback only");
    }

    #[test]
    fn a_profile_and_an_aio_home_are_said() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("notes-dev");
        let ask = crate::paths::Ask { profile: Some(home.display().to_string()), ..Default::default() };
        let cfg = Config::parse("[app]\nid = \"notes\"\nport_of = \"notes\"").unwrap();
        let p = crate::paths::plan("notes", Some(("notes", &cfg.home)), &ask, &|_| None).unwrap();
        let d = crate::paths::AppDirs::planned("notes", p).unwrap();
        let r = rows(&cfg, "1", &d, &|_| None, &|_| false);
        assert_eq!(find(&r, "data").unwrap(), &row("data", home.join("data").display().to_string(), "--profile"));
        assert_eq!(find(&r, "port_of").unwrap().from, "aio.toml");
        // A path names a folder, not a profile: none is said.
        assert!(find(&r, "profile").is_none());
    }
}

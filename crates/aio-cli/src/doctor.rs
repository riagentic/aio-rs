//! `ax doctor <app>` (aio: `am doctor`): is the running app the code on
//! disk, and who decided each setting it runs with. A process keeps the code
//! it started with; a rebuild or an edit after that is a finding, and the
//! finding names its fix.
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// What the doctor looks at, gathered first so the verdict is pure.
#[derive(Debug, Default)]
pub struct Facts {
    /// The running app's `health.json` (none: not running).
    pub health: Option<serde_json::Value>,
    /// The workspace's view of the app: its version, newest source edit and
    /// the binary's build time (ms), when run in its workspace.
    pub source: Option<(String, i64, Option<i64>)>,
    /// Where its config comes from, and who chose its dirs (and which).
    pub config: String,
    pub dirs: (String, PathBuf),
    /// The variables aio reads that are set, with their values.
    pub env: Vec<(String, String)>,
    /// The app's id when its config serves it with `[server] open` (no key):
    /// always a finding, running or not.
    pub open: Option<String>,
    /// When `ax dev`'s last hot-patch (or build) landed in the running app (ms).
    pub patched: Option<i64>,
    /// The Electron its binary pins (a window app's), when it says.
    pub runtime: Option<String>,
    /// Its config serves beyond loopback with no Origin check
    /// (`[server] strict_origin` undecided): what `--harden` closes.
    pub unhardened: Vec<String>,
}

/// Is runtime `v` older than `than` (pure)? Unparsable: not said.
pub fn older(v: &str, than: &str) -> bool {
    match (aio::update::parse_version(v), aio::update::parse_version(than)) {
        (Some((a, _)), Some((b, _))) => a < b,
        _ => false,
    }
}

/// The variables aio reads (`AIORT_TOKEN`'s value is never shown).
pub const VARS: [&str; 11] = [
    "AIORT_DATA",
    "AIORT_APPS_DIR",
    "AIORT_PORT",
    "AIORT_CONFIG",
    "AIORT_PROFILE",
    "AIORT_TOKEN",
    "AIORT_SANDBOX",
    "AIO_APPS_DIR",
    "AIO_PROFILE",
    "AIO_DISCOVERY_PORT",
    "AIO_FEEDBACK_DIR",
];

/// The opt-in security switches `toml` (an `aio.toml`) hasn't decided on
/// that close a door open to its users (aio's `am-harden`): each said, and
/// the text with them written (pure; `None`: nothing to write). One today:
/// a server beyond loopback whose sockets need not say their Origin.
pub fn harden(toml: &str) -> Option<(Vec<String>, String)> {
    let cfg = aio::config::Config::parse(toml).ok()?;
    let listen = cfg.server.listen.as_deref()?;
    let host = listen.rsplit_once(':').map_or(listen, |(h, _)| h).trim_matches(['[', ']']);
    if ["127.0.0.1", "localhost", "::1"].contains(&host) || host.starts_with("127.") {
        return None;
    }
    let lines: Vec<&str> = toml.lines().collect();
    let at = lines.iter().position(|l| l.trim() == "[server]")?;
    let section = lines[at + 1..].iter().take_while(|l| !l.trim_start().starts_with('['));
    if section.clone().any(|l| l.trim_start().starts_with("strict_origin")) {
        return None;
    }
    let said = format!(
        "security: it serves beyond loopback ({listen}) and takes a socket that says no Origin, which a page of \
         another site can open: [server] strict_origin = true"
    );
    let mut out: Vec<&str> = lines[..=at].to_vec();
    out.push("strict_origin = true");
    out.extend(&lines[at + 1..]);
    Some((vec![said], out.join("\n") + if toml.ends_with('\n') { "\n" } else { "" }))
}

/// The newest change (ms) to a file under `dirs`' sources, assets and
/// manifests.
pub fn newest(dirs: &[PathBuf]) -> i64 {
    fn walk(p: &Path) -> i64 {
        let Ok(m) = std::fs::metadata(p) else { return 0 };
        if m.is_dir() {
            return std::fs::read_dir(p).into_iter().flatten().flatten().map(|e| walk(&e.path())).max().unwrap_or(0);
        }
        m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as i64)
    }
    let parts = ["src", "assets", "Cargo.toml", "aio.toml"];
    dirs.iter().flat_map(|d| parts.map(|p| walk(&d.join(p)))).max().unwrap_or(0)
}

/// The findings (each with its fix) and what decided each setting.
pub fn verdict(f: &Facts) -> (Vec<String>, Vec<String>) {
    let mut found = Vec::new();
    let mut said = vec![format!("config: {}", f.config), format!("dirs: {} (from {})", f.dirs.1.display(), f.dirs.0)];
    said.extend(f.env.iter().map(|(k, v)| {
        let v = if k == "AIORT_TOKEN" { "(set)" } else { v };
        format!("{k}={v} (environment)")
    }));
    found.extend(f.open.as_deref().map(aio::config::open_says));
    // Hints (aio's): each names what ends it.
    if let Some(rt) = f.runtime.as_deref().filter(|v| older(v, aio_shell::runtime::VERSION)) {
        found.push(format!(
            "its window runs Electron {rt}, older than the {} this aio-rs is tested with: build it with this \
             aio-rs (ax pin {}, then build it again)",
            aio_shell::runtime::VERSION,
            env!("CARGO_PKG_VERSION")
        ));
    }
    found.extend(f.unhardened.iter().map(|h| format!("{h} (ax doctor --harden writes it)")));
    let Some(h) = &f.health else {
        said.insert(0, "not running: nothing to compare with the disk".into());
        return (found, said);
    };
    let started = h["updatedAt"].as_i64().unwrap_or(0) - h["uptimeS"].as_i64().unwrap_or(0) * 1000;
    let running = h["version"].as_str().unwrap_or("?");
    said.insert(
        0,
        format!(
            "running: pid {}, v{running}, aio-rs {}, since {}",
            h["pid"],
            h["aio"].as_str().unwrap_or("?"),
            aio::log::stamp(started)
        ),
    );
    if let Some(aio) = h["aio"].as_str().filter(|v| *v != env!("CARGO_PKG_VERSION")) {
        found.push(format!(
            "the app runs aio-rs {aio}, this ax is {}: what ax says of it may not fit (use the ax it was built with)",
            env!("CARGO_PKG_VERSION")
        ));
    }
    if let Some((version, edited, built)) = &f.source {
        if version != running {
            found.push(format!("the app runs v{running}, its source is v{version}: stop it and start the new build"));
        }
        // A second of slack: a build stamps files around its start.
        match built {
            Some(b) if *b > started + 1000 => {
                found.push(format!(
                    "built again at {} after it started: stop it and start it again (ax dev does)",
                    aio::log::stamp(*b)
                ));
            }
            _ => {}
        }
        // A hot-patch since it started has the edits up to then.
        let patched = f.patched.filter(|p| *p > started);
        if let Some(p) = patched {
            said.insert(1, format!("hot-patched by ax dev at {}", aio::log::stamp(p)));
        }
        if *edited > built.unwrap_or(started).max(patched.unwrap_or(0)) + 1000 {
            found.push(format!(
                "its source changed at {}, after its last build: build it again",
                aio::log::stamp(*edited)
            ));
        }
    }
    (found, said)
}

pub fn render(f: &Facts) -> String {
    let (found, said) = verdict(f);
    let head = match found.len() {
        0 => "nothing wrong found".to_string(),
        n => format!("{n} finding(s):"),
    };
    std::iter::once(head)
        .chain(found.iter().map(|l| format!("  ✗ {l}")))
        .chain(std::iter::once(String::new()))
        .chain(said.iter().map(|l| format!("  {l}")))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_name_an_old_runtime_and_an_undecided_switch() {
        assert!(older("40.1.0", "44.9.3") && !older("44.9.3", "44.9.3") && !older("x", "44.9.3"));
        let old = Facts { runtime: Some("40.1.0".into()), ..facts(None, None) };
        let (found, _) = verdict(&old);
        assert!(found.iter().any(|f| f.contains("runs Electron 40.1.0, older than")), "{found:?}");
        let now = Facts { runtime: Some(aio_shell::runtime::VERSION.into()), ..facts(None, None) };
        assert!(verdict(&now).0.is_empty());
        // aio's `am fix --harden`: what the app hasn't decided, written; a decision kept.
        let open = "[app]\nid = \"notes\"\n\n[server]\nlisten = \"0.0.0.0:8443\"\ntls = true\n";
        let (said, text) = harden(open).unwrap();
        assert!(said[0].contains("strict_origin = true"), "{said:?}");
        assert_eq!(
            text,
            "[app]\nid = \"notes\"\n\n[server]\nstrict_origin = true\nlisten = \"0.0.0.0:8443\"\ntls = true\n"
        );
        assert!(aio::config::Config::parse(&text).unwrap().server.strict_origin);
        assert_eq!(harden(&text), None, "written once");
        assert_eq!(harden(&open.replace("tls = true", "strict_origin = false")), None, "a decision either way stays");
        assert_eq!(harden(&open.replace("0.0.0.0", "127.0.0.1")), None, "loopback: no door to close");
        assert_eq!(harden("[app]\nid = \"notes\"\n"), None);
    }

    fn facts(health: Option<serde_json::Value>, source: Option<(String, i64, Option<i64>)>) -> Facts {
        Facts {
            health,
            source,
            config: "/w/notes/aio.toml".into(),
            dirs: ("default".into(), PathBuf::from("/h/notes/data")),
            env: vec![("AIORT_TOKEN".into(), "s3cret".into()), ("AIORT_PROFILE".into(), "work".into())],
            open: None,
            patched: None,
            runtime: None,
            unhardened: vec![],
        }
    }

    fn up(version: &str, aio: &str) -> serde_json::Value {
        // Started at 100 s.
        serde_json::json!({ "pid": 7, "version": version, "aio": aio, "updatedAt": 160_000, "uptimeS": 60 })
    }

    #[test]
    fn the_running_app_is_the_code_on_disk_or_a_finding_says_how_to_fix_it() {
        let me = env!("CARGO_PKG_VERSION");
        let (found, said) = verdict(&facts(Some(up("1.0.0", me)), Some(("1.0.0".into(), 90_000, Some(95_000)))));
        assert!(found.is_empty(), "{found:?}");
        assert!(said[0].starts_with("running: pid 7, v1.0.0"), "{said:?}");
        // Rebuilt after it started; edited after that; another version.
        let (found, _) = verdict(&facts(Some(up("1.0.0", me)), Some(("1.1.0".into(), 300_000, Some(200_000)))));
        assert_eq!(found.len(), 3, "{found:?}");
        assert!(found[0].contains("runs v1.0.0, its source is v1.1.0"));
        assert!(found[1].contains("built again") && found[1].contains("stop it and start it again"));
        assert!(found[2].contains("source changed") && found[2].contains("build it again"));
        // Edited after the build, but hot-patched since: it runs the edit.
        let edited = Some(("1.0.0".into(), 120_000, Some(95_000)));
        let (found, _) = verdict(&facts(Some(up("1.0.0", me)), edited.clone()));
        assert!(found[0].contains("source changed"), "{found:?}");
        let (found, said) = verdict(&Facts { patched: Some(121_000), ..facts(Some(up("1.0.0", me)), edited) });
        assert!(found.is_empty() && said[1].starts_with("hot-patched by ax dev at"), "{found:?} {said:?}");
        // Another aio built it.
        let (found, _) = verdict(&facts(Some(up("1.0.0", "0.0.1")), None));
        assert!(found[0].contains("runs aio-rs 0.0.1"), "{found:?}");
    }

    #[test]
    fn it_says_who_decided_each_setting_and_hides_the_token() {
        let (found, said) = verdict(&facts(None, None));
        assert!(found.is_empty());
        assert_eq!(said[0], "not running: nothing to compare with the disk");
        assert!(said.contains(&"config: /w/notes/aio.toml".to_string()));
        assert!(said.contains(&"dirs: /h/notes/data (from default)".to_string()), "{said:?}");
        assert!(said.contains(&"AIORT_TOKEN=(set) (environment)".to_string()) && !said.concat().contains("s3cret"));
        assert!(said.contains(&"AIORT_PROFILE=work (environment)".to_string()));
        assert!(render(&facts(None, None)).starts_with("nothing wrong found\n\n  not running"));
    }

    #[test]
    fn an_open_app_is_a_finding_running_or_not() {
        let open = Facts { open: Some("notes".into()), ..facts(None, None) };
        let (found, _) = verdict(&open);
        assert!(found.len() == 1 && found[0].starts_with("security: notes is OPEN"), "{found:?}");
        assert!(render(&open).starts_with("1 finding(s):\n  ✗ security: notes is OPEN"));
    }

    #[test]
    fn the_newest_edit_is_found_in_sources_and_manifests() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("src/ui")).unwrap();
        std::fs::write(d.path().join("src/ui/a.rs"), "").unwrap();
        assert!(newest(&[d.path().into()]) > 0);
        assert_eq!(newest(&[d.path().join("none")]), 0);
    }
}

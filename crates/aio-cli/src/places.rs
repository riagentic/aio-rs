//! Verbs about where things are: an app's dirs and what to back up
//! (`ax data`), the apps and private instances on this machine and which
//! run (`ax apps`), and where findings about aio go (`ax feedback`).
use std::path::{Path, PathBuf};

/// The bytes under `p` (0 when it isn't there).
fn size(p: &Path) -> u64 {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => std::fs::read_dir(p).into_iter().flatten().flatten().map(|e| size(&e.path())).sum(),
        Ok(m) => m.len(),
        Err(_) => 0,
    }
}

pub fn human(bytes: u64) -> String {
    match bytes {
        b if b < 1024 => format!("{b} B"),
        b if b < 1024 * 1024 => format!("{:.1} KB", b as f64 / 1024.0),
        b if b < 1024 * 1024 * 1024 => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
        b => format!("{:.1} GB", b as f64 / (1024.0 * 1024.0 * 1024.0)),
    }
}

/// Where app `id` keeps everything (`plan`, as a launch with no flags would
/// have it), and what of it to back up.
pub fn data(id: &str, plan: &aio::paths::Plan, json: bool) -> String {
    let running = aio::paths::running(&plan.data);
    let home = plan.data.parent().unwrap_or(&plan.data);
    if json {
        let dir = |p: &Path| serde_json::json!({ "path": p, "bytes": size(p) });
        return serde_json::json!({
            "id": id, "running": running, "from": plan.from, "profile": plan.profile,
            "data": dir(&plan.data), "logs": dir(&plan.logs), "cache": dir(&plan.cache),
            "sockets": aio::paths::sockets_at(id, &plan.data), "backups": home.join("backups"),
        })
        .to_string();
    }
    let sized = |p: &Path| format!("{}  {}", p.display(), human(size(p)));
    let rows = [
        ("data ①", sized(&plan.data)),
        ("logs ②", sized(&plan.logs)),
        ("cache ②", sized(&plan.cache)),
        ("sockets ③", aio::paths::sockets_at(id, &plan.data).display().to_string()),
        ("backups", home.join("backups").display().to_string()),
    ];
    let state = if running { "running" } else { "not running" };
    let profile = plan.profile.as_ref().map_or(String::new(), |p| format!(", profile {p}"));
    std::iter::once(format!("{id}  ({state}; dirs from {}{profile})", plan.from))
        .chain(rows.iter().map(|(k, v)| format!("  {k:<10} {v}")))
        .chain([
            String::new(),
            "  ① back this up: all the app can't make again (ax backup <app>)".into(),
            "  ② made again as needed: delete any time".into(),
            "  ③ the running app's doors: gone with it".into(),
        ])
        .collect::<Vec<_>>()
        .join("\n")
}

/// The apps under `apps/` and the private instances under `instances/<n>/`
/// of the tool home `root`, each running or not.
pub fn apps(root: &Path, json: bool) -> String {
    let dirs = |d: &Path| -> Vec<(String, PathBuf)> {
        let mut v: Vec<_> = std::fs::read_dir(d)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
            .collect();
        v.sort();
        v
    };
    let mut rows: Vec<(String, String, PathBuf)> =
        dirs(&root.join("apps")).into_iter().map(|(n, p)| (n, String::new(), p)).collect();
    for (inst, at) in dirs(&root.join("instances")) {
        rows.extend(dirs(&at).into_iter().map(|(n, p)| (n, format!("instance {inst}"), p)));
    }
    if json {
        let rows = rows.iter().map(|(name, inst, home)| {
            let data = home.join("data");
            let on = aio::paths::running(&data);
            let mut v = serde_json::json!({ "name": name, "home": home, "running": on });
            if let Some(i) = inst.strip_prefix("instance ") {
                v["instance"] = i.into();
            }
            if on {
                let (h, s) = (read(&data, "health.json"), read(&data, "serving.json"));
                (v["pid"], v["version"], v["aio"], v["since"]) =
                    (h["pid"].clone(), h["version"].clone(), h["aio"].clone(), h["startedAt"].clone());
                (v["port"], v["tls"], v["host"]) = (s["port"].clone(), s["tls"].clone(), s["host"].clone());
                v["sockets"] = aio::paths::sockets_at(name, &data).to_string_lossy().into();
                v["startedBy"] = by(home, &h).into();
            }
            v
        });
        return serde_json::Value::Array(rows.collect()).to_string();
    }
    if rows.is_empty() {
        return "no aio-rs apps yet".into();
    }
    let w = rows.iter().map(|(n, ..)| n.len()).max().unwrap_or(0);
    rows.iter()
        .map(|(name, inst, home)| {
            let data = home.join("data");
            let (run, about) = if aio::paths::running(&data) {
                ("running", format!("{}, started by {}", running(&data), by(home, &read(&data, "health.json"))))
            } else {
                ("-", String::new())
            };
            let inst = if inst.is_empty() { String::new() } else { format!("  ({inst})") };
            format!("{name:<w$}  {run:<7}  {}{inst}{about}", home.display())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Who started the app running at `home` (its `logs/start.json` against
/// what it says of itself): `ax start`, else `other` (`ax dev`, a
/// launcher, a terminal), with the user and whether from a terminal.
fn by(home: &Path, health: &serde_json::Value) -> String {
    let rec = read(&home.join("logs"), crate::start::RECORD);
    match crate::start::started_by(&rec, health) {
        // `started_by`'s tag (start.rs), not the text shown.
        "aio start" => {
            let user = rec["user"].as_str().unwrap_or("?");
            let from = if rec["terminal"] == true { "a terminal" } else { "no terminal (an agent or a script)" };
            format!("ax start ({user}, from {from})")
        }
        other => other.into(),
    }
}

/// A JSON file in `data` (`null` when it isn't there or isn't JSON).
fn read(data: &Path, f: &str) -> serde_json::Value {
    std::fs::read(data.join(f)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// What a running app says of itself in `data`: its pid, version and start
/// (`health.json`), and the port it serves on (`serving.json`).
fn running(data: &Path) -> String {
    let (h, s) = (read(data, "health.json"), read(data, "serving.json"));
    let mut out = String::new();
    if let (Some(pid), Some(v), Some(at)) = (h["pid"].as_u64(), h["version"].as_str(), h["startedAt"].as_str()) {
        out.push_str(&format!("  pid {pid}, v{v}, since {at}"));
    }
    if let Some(port) = s["port"].as_u64() {
        out.push_str(&format!(", serving on port {port}{}", if s["tls"] == true { " (TLS)" } else { "" }));
    }
    if let Some(v) = other_aio(h["aio"].as_str()) {
        out.push_str(&format!(", on aio-rs {v} ≠ this ax {}", env!("CARGO_PKG_VERSION")));
    }
    out
}

/// The aio an app runs on, when it isn't this one (two checkouts on one
/// machine is the usual dev setup, and a mismatch the first thing to rule
/// out); unknown is no mismatch.
fn other_aio(aio: Option<&str>) -> Option<&str> {
    aio.filter(|v| *v != env!("CARGO_PKG_VERSION"))
}

/// The apps that answered a LAN probe, one line each.
pub fn discovered(found: &[aio::discovery::Found], json: bool) -> String {
    if json {
        let rows = found.iter().map(|f| {
            serde_json::json!({ "name": f.name, "host": f.host, "port": f.port, "tls": f.tls, "needsAuth": f.needs_auth, "title": f.title })
        });
        return serde_json::Value::Array(rows.collect()).to_string();
    }
    if found.is_empty() {
        return "no aio-rs apps answered (only apps with [server] discover = true do, on this LAN)".into();
    }
    found
        .iter()
        .map(|f| {
            let at =
                f.host.map_or_else(|| format!("?:{}", f.port), |h| std::net::SocketAddr::new(h, f.port).to_string());
            let flags = [(f.tls, "tls"), (f.needs_auth, "auth")];
            let how: Vec<&str> = flags.iter().filter(|(on, _)| *on).map(|(_, w)| *w).collect();
            let title = f.title.as_ref().map_or(String::new(), |t| format!("  \"{t}\""));
            format!("{:<16} {at:<22} {}{title}", f.name, if how.is_empty() { "-".into() } else { how.join(",") })
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Where findings about aio go: `AIO_FEEDBACK_DIR`, else aio's place
/// (`<XDG data home>/aio/feedback`), outside every app and version, so
/// aio and aio-rs apps write to the same files.
pub fn feedback_dir(env: aio::paths::Env) -> PathBuf {
    env("AIO_FEEDBACK_DIR").unwrap_or_else(|| {
        let home = env("HOME").or_else(|| env("USERPROFILE")).unwrap_or_default();
        env("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local/share")).join("aio/feedback")
    })
}

/// The one file of `name`'s findings: a slug, never a path.
pub fn feedback_file(dir: &Path, name: &str) -> PathBuf {
    let slug: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '-' })
        .collect();
    let mut slug = slug.trim_matches(['-', '.']).to_string();
    // Runs of replaced characters are one dash.
    while slug.contains("--") {
        slug = slug.replace("--", "-");
    }
    slug.truncate(64);
    dir.join(format!("{}.md", if slug.is_empty() { "app" } else { &slug }))
}

/// `ax feedback`: the place and the reports in it; with `name`, that
/// app's file (made, with its heading, when `create`).
pub fn feedback(dir: &Path, name: Option<&str>, create: bool) -> Result<String, String> {
    let Some(name) = name else {
        let mut files: Vec<String> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".md"))
            .collect();
        files.sort();
        let list = if files.is_empty() { "none yet".into() } else { files.join(", ") };
        return Ok(format!(
            "feedback dir  {}\nreports       {list}\n\nOutside every app and aio-rs version: no update or uninstall touches it.\nOne app's file: ax feedback <app> [--create]",
            dir.display()
        ));
    };
    let file = feedback_file(dir, name);
    let had = file.is_file();
    if create && !had {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let head = format!(
            "# {name}: findings against aio-rs\n\nRough edges, bugs and \"this should be easier\" notes from building {name}.\nFor the aio-rs maintainers: what you tried, what happened, what you expected,\nand the aio-rs version (`ax version`) and file it happened in.\n\n## 1 ·\n"
        );
        std::fs::write(&file, head).map_err(|e| format!("{}: {e}", file.display()))?;
    }
    let status = match (had, create) {
        (true, _) => "exists".to_string(),
        (false, true) => "created".into(),
        (false, false) => format!("not made yet: ax feedback {name} --create"),
    };
    Ok(format!("report file   {}\nstatus        {status}", file.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn found_apps_say_where_and_how_to_reach_them() {
        use aio::discovery::Found;
        assert!(discovered(&[], false).starts_with("no aio-rs apps answered"));
        let host = Some("192.168.1.9".parse().unwrap());
        let a =
            Found { name: "todo".into(), port: 8100, title: Some("Todo".into()), needs_auth: true, tls: true, host };
        let b = Found { name: "notes".into(), port: 8101, title: None, needs_auth: false, tls: false, host };
        let out = discovered(&[a.clone(), b], false);
        let rows: serde_json::Value = serde_json::from_str(&discovered(&[a], true)).unwrap();
        assert_eq!(rows[0]["host"], "192.168.1.9");
        assert_eq!((rows[0]["port"].as_u64(), rows[0]["needsAuth"].as_bool()), (Some(8100), Some(true)));
        assert_eq!(out.lines().next().unwrap(), "todo             192.168.1.9:8100       tls,auth  \"Todo\"");
        assert_eq!(out.lines().nth(1).unwrap(), "notes            192.168.1.9:8101       -");
    }

    #[test]
    fn findings_have_one_place_and_one_file_per_app() {
        let vars = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| PathBuf::from(v))
        };
        assert_eq!(feedback_dir(&vars(&[("HOME", "/h")])), Path::new("/h/.local/share/aio/feedback"));
        assert_eq!(feedback_dir(&vars(&[("HOME", "/h"), ("XDG_DATA_HOME", "/x")])), Path::new("/x/aio/feedback"));
        assert_eq!(feedback_dir(&vars(&[("AIO_FEEDBACK_DIR", "/f"), ("HOME", "/h")])), Path::new("/f"));
        let d = Path::new("/f");
        assert_eq!(feedback_file(d, "My App"), d.join("my-app.md"));
        // A name never escapes the dir.
        assert_eq!(feedback_file(d, "../../etc/passwd"), d.join("etc-passwd.md"));
        assert_eq!(feedback_file(d, "///"), d.join("app.md"));
        assert_eq!(feedback_file(d, &"x".repeat(100)), d.join(format!("{}.md", "x".repeat(64))));
    }

    #[test]
    fn feedback_lists_and_makes_files() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("fb");
        assert!(feedback(&dir, None, false).unwrap().contains("reports       none yet"));
        assert!(feedback(&dir, Some("notes"), false).unwrap().contains("not made yet: ax feedback notes --create"));
        assert!(!dir.exists(), "asking makes nothing");
        assert!(feedback(&dir, Some("notes"), true).unwrap().contains("status        created"));
        assert!(std::fs::read_to_string(dir.join("notes.md")).unwrap().starts_with("# notes: findings against aio-rs"));
        std::fs::write(dir.join("notes.md"), "mine").unwrap();
        assert!(feedback(&dir, Some("notes"), true).unwrap().contains("status        exists"));
        assert_eq!(std::fs::read_to_string(dir.join("notes.md")).unwrap(), "mine", "never overwritten");
        assert!(feedback(&dir, None, false).unwrap().contains("reports       notes.md"));
    }

    #[test]
    fn data_says_where_and_what_to_back_up() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("notes");
        std::fs::create_dir_all(home.join("data")).unwrap();
        std::fs::write(home.join("data/state.db"), vec![0u8; 2048]).unwrap();
        let plan = aio::paths::Plan {
            data: home.join("data"),
            logs: home.join("logs"),
            cache: home.join("cache"),
            store: home.join("data/state.db"),
            profile: Some("work".into()),
            port_of: None,
            from: "--profile",
            asked: true,
        };
        let out = data("notes", &plan, false);
        assert!(out.starts_with("notes  (not running; dirs from --profile, profile work)"), "{out}");
        assert!(out.contains(&format!("data ①     {}  2.0 KB", home.join("data").display())), "{out}");
        assert!(out.contains(&format!("logs ②     {}  0 B", home.join("logs").display())), "{out}");
        assert!(out.contains(&format!("backups    {}", home.join("backups").display())), "{out}");
        assert!(!home.join("logs").exists(), "asking makes nothing");
        // Held by a running app: it says so.
        let _dirs = aio::paths::AppDirs::at(&home, "notes").unwrap();
        assert!(data("notes", &plan, false).starts_with("notes  (running;"));
        let v: serde_json::Value = serde_json::from_str(&data("notes", &plan, true)).unwrap();
        assert_eq!(
            (v["running"].as_bool(), v["data"]["bytes"].as_u64(), v["from"].as_str()),
            (Some(true), Some(2048), Some("--profile"))
        );
    }

    #[test]
    fn apps_lists_homes_and_instances_and_which_run() {
        // The sockets dir follows the sandbox (no XDG dirs in it): held for the
        // whole test, so a harness in a parallel test can't move it in between.
        let _world = aio::paths::sandbox().unwrap();
        let t = tempfile::tempdir().unwrap();
        assert_eq!(apps(t.path(), false), "no aio-rs apps yet");
        assert_eq!(apps(t.path(), true), "[]");
        for d in ["apps/notes/data", "apps/mail/data", "instances/agent1/notes/data"] {
            std::fs::create_dir_all(t.path().join(d)).unwrap();
        }
        let _run = aio::paths::AppDirs::at(&t.path().join("apps/mail"), "mail").unwrap();
        let health = r#"{"pid":42,"version":"1.2.0","startedAt":"2026-10-28T08:00:00.000Z"}"#;
        std::fs::write(t.path().join("apps/mail/data/health.json"), health).unwrap();
        std::fs::write(t.path().join("apps/mail/data/serving.json"), r#"{"port":8100,"tls":true}"#).unwrap();
        // A stopped app's leftovers say nothing.
        std::fs::write(t.path().join("apps/notes/data/health.json"), health).unwrap();
        let out = apps(t.path(), false);
        let rows: serde_json::Value = serde_json::from_str(&apps(t.path(), true)).unwrap();
        assert_eq!(
            (rows[0]["name"].as_str(), rows[0]["pid"].as_u64(), rows[0]["port"].as_u64()),
            (Some("mail"), Some(42), Some(8100))
        );
        // The whole record, for tools and peers: how to reach it, on which aio.
        let at = aio::paths::sockets_at("mail", &t.path().join("apps/mail/data"));
        assert_eq!(
            (rows[0]["tls"].as_bool(), rows[0]["aio"].as_str(), rows[0]["sockets"].as_str()),
            (Some(true), None, at.to_str())
        );
        assert_eq!((rows[1]["running"].as_bool(), rows[1].get("pid")), (Some(false), None));
        assert_eq!(rows[2]["instance"], "agent1");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3, "{out}");
        assert!(lines[0].starts_with("mail   running") && lines[1].starts_with("notes  -"), "{out}");
        let about = "  pid 42, v1.2.0, since 2026-10-28T08:00:00.000Z, serving on port 8100 (TLS), started by other";
        assert!(lines[0].ends_with(about) && !lines[1].contains("pid"), "{out}");
        assert!(lines[2].starts_with("notes  -") && lines[2].ends_with("(instance agent1)"), "{out}");
        assert_eq!(rows[0]["startedBy"], "other");
        // Who started it: ax start's record, when it names the running process.
        std::fs::create_dir_all(t.path().join("apps/mail/logs")).unwrap();
        let rec = r#"{"by":"ax start","user":"ann","terminal":false,"pid":42,"startedAt":"2026-10-28T08:00:00.000Z"}"#;
        std::fs::write(t.path().join("apps/mail/logs").join(crate::start::RECORD), rec).unwrap();
        let out = apps(t.path(), false);
        assert!(
            out.lines().next().unwrap().ends_with("started by ax start (ann, from no terminal (an agent or a script))")
        );
        let rows: serde_json::Value = serde_json::from_str(&apps(t.path(), true)).unwrap();
        assert!(rows[0]["startedBy"].as_str().unwrap().starts_with("ax start (ann"));
        // On another aio: said; on this one: nothing to say.
        let other = r#"{"pid":42,"version":"1.2.0","aio":"0.0.1","startedAt":"2026-10-28T08:00:00.000Z"}"#;
        std::fs::write(t.path().join("apps/mail/data/health.json"), other).unwrap();
        let first = apps(t.path(), false).lines().next().unwrap().to_string();
        assert!(first.contains(&format!(", on aio-rs 0.0.1 ≠ this ax {}", env!("CARGO_PKG_VERSION"))), "{first}");
        assert_eq!(serde_json::from_str::<serde_json::Value>(&apps(t.path(), true)).unwrap()[0]["aio"], "0.0.1");
    }

    #[test]
    fn only_another_aio_is_a_mismatch() {
        assert_eq!(other_aio(None), None, "unknown is not a mismatch");
        assert_eq!(other_aio(Some(env!("CARGO_PKG_VERSION"))), None);
        assert_eq!(other_aio(Some("0.0.1")), Some("0.0.1"));
    }
}

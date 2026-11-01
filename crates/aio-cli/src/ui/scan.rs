//! Which aio-rs apps this machine has (amui's `server/scan.server.ts`): the
//! ones that ever ran, running or not (`ax apps`: their homes), and the
//! projects on disk (a folder with `aio.toml` beside its `Cargo.toml`), found
//! without configuration by a depth-capped walk of the home folder that
//! stops at the first project, skips dot folders, build output and the
//! system's own, and never follows into network mounts. Each app is matched
//! to its project by its id, so the Codebase, Tasks and Start of a stopped
//! app reach its source.
use aio::serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// More folders to look in, separated as `PATH` is (`:`, `;` on Windows),
/// used as given (aio's `AMUI_ROOTS`).
pub const ROOTS_VAR: &str = "AIORT_UI_ROOTS";
/// How deep under each root a project is looked for.
pub const DEPTH: usize = 3;
/// ax ui's own id: listed like any app, never started or stopped from itself.
pub const SELF_ID: &str = "aio-ui";

/// How to name more roots, in this OS's spelling.
pub fn roots_example() -> String {
    if cfg!(windows) { format!("{ROOTS_VAR}=C:\\path;D:\\path2") } else { format!("{ROOTS_VAR}=/path:/path2") }
}

/// A project's `aio.toml`, as the manager shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meta {
    pub name: String,
    pub id: String,
    pub version: Option<String>,
    /// `[app] mode`: standalone, client-server, server, client, cli.
    pub mode: Option<String>,
    /// `[build] targets` by name (none: the default, desktop).
    pub targets: Vec<String>,
    /// `[server] listen`.
    pub listen: Option<String>,
    /// `[accounts] on`.
    pub accounts: bool,
    /// Its own tasks: its cargo aliases ([`aliases`]), name and command.
    pub tasks: Vec<(String, String)>,
}

/// A running app, from its `health.json` and `serving.json` (`ax apps`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Running {
    pub pid: u32,
    pub port: Option<u64>,
    pub host: Option<String>,
    pub tls: bool,
    pub started_by: Option<String>,
    pub since: Option<String>,
    pub aio: Option<String>,
    pub version: Option<String>,
}

/// One entry of the list.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Project {
    /// The list's identity: the app's id (`id#instance` for a private
    /// instance), or the folder of a project that never ran.
    pub id: String,
    /// The app's id, what its verbs name.
    pub name: String,
    pub instance: Option<String>,
    /// Its data home, once it ran.
    pub home: Option<PathBuf>,
    /// Its project folder, when found.
    pub path: Option<PathBuf>,
    pub meta: Meta,
    pub running: Option<Running>,
    pub git: bool,
    /// ax ui itself: monitored like any app, never started or stopped here.
    pub own: bool,
}

impl Project {
    /// Its logs' folder (its home's `logs`).
    pub fn logs(&self) -> Option<PathBuf> {
        self.home.as_ref().map(|h| h.join("logs"))
    }
}

/// A folder's project, if it is one: its `aio.toml` beside a `Cargo.toml`
/// (amui's `readProjectMeta`). Never fails: an unreadable or invalid
/// `aio.toml` is no project.
pub fn read_meta(dir: &Path) -> Option<Meta> {
    if !dir.join("Cargo.toml").is_file() {
        return None;
    }
    let cfg = aio::config::Config::parse(&std::fs::read_to_string(dir.join("aio.toml")).ok()?).ok()?;
    let mode = aio::serde_json::to_value(cfg.app.mode).ok().and_then(|v| v.as_str().map(String::from));
    Some(Meta {
        name: if cfg.app.name.is_empty() { cfg.app.id.clone() } else { cfg.app.name.clone() },
        id: cfg.app.id,
        version: cfg.app.version,
        mode,
        targets: cfg.build.targets.into_iter().map(|t| t.name).collect(),
        listen: cfg.server.listen,
        accounts: cfg.accounts.on,
        tasks: aliases(dir),
    })
}

/// A project's own tasks (amui's deno tasks): the `[alias]`es of the
/// `.cargo/config.toml` in its folder and each above it still in its
/// workspace (one with a `Cargo.toml`), the nearest first, by name; each
/// with what `cargo <alias>` runs.
pub fn aliases(dir: &Path) -> Vec<(String, String)> {
    let mut all: BTreeMap<String, String> = BTreeMap::new();
    for d in dir.ancestors().take_while(|d| d.join("Cargo.toml").is_file()) {
        let text =
            ["config.toml", "config"].iter().find_map(|f| std::fs::read_to_string(d.join(".cargo").join(f)).ok());
        let Some(t) = text.and_then(|t| t.parse::<toml::Table>().ok()) else { continue };
        for (name, v) in t.get("alias").and_then(toml::Value::as_table).into_iter().flatten() {
            let run = match v {
                toml::Value::String(s) => s.clone(),
                toml::Value::Array(a) => a.iter().filter_map(toml::Value::as_str).collect::<Vec<_>>().join(" "),
                _ => continue,
            };
            all.entry(name.clone()).or_insert(run);
        }
    }
    all.into_iter().collect()
}

/// Folders a project never lives in, skipped however the walk got there:
/// pseudo file systems, machine state, mounts (network ones block), and the
/// user's own app state (pure).
pub fn never_walk(home: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = if cfg!(windows) {
        let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        let var = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| format!("{drive}\\{d}"));
        [
            var("SystemRoot", "Windows"),
            var("ProgramFiles", "Program Files"),
            var("ProgramFiles(x86)", "Program Files (x86)"),
            var("ProgramData", "ProgramData"),
            format!("{drive}\\$Recycle.Bin"),
            format!("{drive}\\System Volume Information"),
            format!("{drive}\\Recovery"),
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect()
    } else {
        [
            "/proc",
            "/sys",
            "/dev",
            "/run",
            "/boot",
            "/tmp",
            "/var",
            "/etc",
            "/usr",
            "/lib",
            "/lib64",
            "/bin",
            "/sbin",
            "/snap",
            "/mnt",
            "/media",
            "/lost+found",
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect()
    };
    if cfg!(target_os = "macos") {
        v.extend(["/System", "/Library", "/private", "/Volumes", "/cores"].map(PathBuf::from));
        v.push(home.join("Library"));
    }
    if cfg!(windows) {
        v.push(home.join("AppData"));
    }
    v
}

/// One folder? Windows names folders in any case (`SystemRoot` is often
/// `C:\WINDOWS`, the folder `C:\Windows`).
fn same(a: &Path, b: &Path) -> bool {
    if cfg!(windows) { a.as_os_str().eq_ignore_ascii_case(b.as_os_str()) } else { a == b }
}

/// Folders the walk never enters by name: deps, build output.
const SKIP: [&str; 4] = ["node_modules", "target", "dist", "build"];

/// The projects under `roots`, each walked `depth` folders deep (amui's
/// `scanDisk`): a project's own folders are not walked.
pub fn scan_disk(roots: &[PathBuf], depth: usize, never: &[PathBuf]) -> BTreeMap<PathBuf, Meta> {
    fn walk(dir: &Path, left: usize, never: &[PathBuf], out: &mut BTreeMap<PathBuf, Meta>) {
        if let Some(m) = read_meta(dir) {
            out.insert(dir.to_path_buf(), m);
            return;
        }
        if left == 0 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if !e.file_type().is_ok_and(|t| t.is_dir()) || name.starts_with('.') || SKIP.contains(&name.as_ref()) {
                continue;
            }
            let p = e.path();
            if !never.iter().any(|n| same(n, &p)) {
                walk(&p, left - 1, never, out);
            }
        }
    }
    let mut out = BTreeMap::new();
    let mut seen = Vec::new();
    for r in roots {
        if !seen.contains(r) && r.is_dir() {
            seen.push(r.clone());
            walk(r, depth, never, &mut out);
        }
    }
    out
}

/// Where to look, most specific first (amui's `rootsFor`, pure): the
/// [`ROOTS_VAR`] folders as given, `~/aio-apps` (where ax ui makes apps),
/// the folder ax ui runs in, the home folder, and the parent of each
/// running app's folder (its siblings are usually projects too).
pub fn roots_for(var: &str, home: &Path, cwd: Option<&Path>, running: &[PathBuf]) -> Vec<PathBuf> {
    let sep = if cfg!(windows) { ';' } else { ':' };
    let mut roots: Vec<PathBuf> = var.split(sep).map(str::trim).filter(|r| !r.is_empty()).map(PathBuf::from).collect();
    let mut add = |p: PathBuf| {
        if !roots.contains(&p) {
            roots.push(p);
        }
    };
    add(home.join("aio-apps"));
    if let Some(c) = cwd.filter(|c| c.starts_with(home)) {
        add(c.to_path_buf());
    }
    add(home.to_path_buf());
    for cwd in running {
        if let Some(p) = cwd.parent().filter(|p| p.starts_with(home) && p.components().count() >= 4) {
            add(p.to_path_buf());
        }
    }
    roots
}

/// The apps of `ax apps --json` (its `{ok, value}` or the list alone).
pub fn apps(json: &str) -> Result<Vec<Project>, String> {
    let said: Value = aio::serde_json::from_str(json).map_err(|e| format!("ax apps: {e}"))?;
    let rows = said["value"].as_array().or(said.as_array()).ok_or("ax apps: no list of apps")?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            let name = r["name"].as_str()?.to_string();
            let instance = r["instance"].as_str().map(String::from);
            let s = |k: &str| r[k].as_str().map(String::from);
            let running = (r["running"] == true).then(|| Running {
                pid: r["pid"].as_u64().unwrap_or(0) as u32,
                port: r["port"].as_u64(),
                host: s("host"),
                tls: r["tls"] == true,
                started_by: s("startedBy"),
                since: s("since"),
                aio: s("aio"),
                version: s("version"),
            });
            Some(Project {
                id: instance.as_ref().map_or(name.clone(), |i| format!("{name}#{i}")),
                own: name == SELF_ID,
                meta: Meta { name: name.clone(), id: name.clone(), ..Meta::default() },
                home: s("home").map(PathBuf::from),
                name,
                instance,
                running,
                ..Project::default()
            })
        })
        .collect())
}

/// The list (amui's `discoverProjects`, pure): every app that ran, matched
/// to its project by id (its running process's folder first, `cwd_of`),
/// then the projects that never ran; the running first, each by name.
pub fn merge(
    mut apps: Vec<Project>,
    disk: &BTreeMap<PathBuf, Meta>,
    cwd_of: &dyn Fn(u32) -> Option<PathBuf>,
) -> Vec<Project> {
    let mut used = Vec::new();
    for a in apps.iter_mut() {
        let at_cwd =
            a.running.as_ref().and_then(|r| cwd_of(r.pid)).filter(|c| disk.get(c).is_some_and(|m| m.id == a.name));
        let found = at_cwd.or_else(|| disk.iter().find(|(_, m)| m.id == a.name).map(|(p, _)| p.clone()));
        if let Some(p) = found {
            a.meta = disk[&p].clone();
            a.git = p.join(".git").exists();
            used.push(p.clone());
            a.path = Some(p);
        }
    }
    for (p, m) in disk {
        if !used.contains(p) {
            apps.push(Project {
                id: p.display().to_string(),
                name: m.id.clone(),
                path: Some(p.clone()),
                meta: m.clone(),
                git: p.join(".git").exists(),
                own: m.id == SELF_ID,
                ..Project::default()
            });
        }
    }
    apps.sort_by(|a, b| {
        (a.running.is_none(), &a.name, &a.instance, &a.id).cmp(&(b.running.is_none(), &b.name, &b.instance, &b.id))
    });
    apps
}

/// The entry `key` names: its id, else the one project in that folder
/// (amui's `findProject`): a folder two entries share names neither.
pub fn find<'a>(projects: &'a [Project], key: &str) -> Option<&'a Project> {
    if let Some(p) = projects.iter().find(|p| p.id == key) {
        return Some(p);
    }
    let mut at = projects.iter().filter(|p| p.path.as_ref().is_some_and(|d| d.as_os_str() == key));
    match (at.next(), at.next()) {
        (Some(p), None) => Some(p),
        _ => None,
    }
}

/// Where a selection goes after a rescan (amui's `followSelection`): the
/// entry itself while listed; a project that never ran, to its app once
/// it did (the one entry in its folder). Never to a sibling instance.
pub fn follow<'a>(found: &'a [Project], id: &str) -> Option<&'a Project> {
    found.iter().find(|p| p.id == id).or_else(|| {
        let mut at = found.iter().filter(|p| p.path.as_ref().is_some_and(|d| d.as_os_str() == id));
        match (at.next(), at.next()) {
            (Some(p), None) => Some(p),
            _ => None,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aio::serde_json::json;

    fn project(dir: &Path, id: &str, extra: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        std::fs::write(dir.join("aio.toml"), format!("[app]\nid = \"{id}\"\nname = \"{id}\"\n{extra}")).unwrap();
    }

    /// amui's `readProjectMeta: parses an aio deno.json (name, target, tasks, isAio)`.
    #[test]
    fn a_project_is_read_from_its_aio_toml() {
        let d = tempfile::tempdir().unwrap();
        project(
            d.path(),
            "notes",
            "mode = \"client-server\"\nversion = \"1.2\"\n[server]\nlisten = \"0.0.0.0:7420\"\n[build]\ntargets = [\"deb\"]\n",
        );
        let m = read_meta(d.path()).unwrap();
        assert_eq!(
            (m.id.as_str(), m.mode.as_deref(), m.version.as_deref(), m.targets.as_slice()),
            ("notes", Some("client-server"), Some("1.2"), ["deb".to_string()].as_slice())
        );
        std::fs::write(d.path().join("aio.toml"), "[app]\nid = \"x\"\nnope = 1\n").unwrap();
        assert_eq!(read_meta(d.path()), None, "an invalid aio.toml is no project");
        std::fs::remove_file(d.path().join("Cargo.toml")).unwrap();
        assert_eq!(read_meta(d.path()), None, "nor one without its Cargo.toml");
    }

    /// amui's `discoverProjects: roots cover ~/aio-apps + cwd`, `scan:
    /// explicit AMUI_ROOTS is honoured verbatim` and `never walks
    /// pseudo-filesystems or machine state`.
    #[test]
    fn the_walk_finds_projects_where_people_keep_them_and_nowhere_else() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("home");
        project(&home.join("code/gen/notes"), "notes", "");
        project(&home.join("code/gen/notes/app"), "inner", "");
        project(&home.join("aio-apps/todo"), "todo", "");
        project(&home.join(".config/hidden"), "hidden", "");
        project(&home.join("a/b/c/d/deep"), "deep", "");
        project(&home.join("code/target/built"), "built", "");
        project(&d.path().join("elsewhere/x"), "x", "");
        let roots = roots_for("", &home, None, &[]);
        assert_eq!(roots, [home.join("aio-apps"), home.clone()]);
        let found = scan_disk(&roots, DEPTH, &never_walk(&home));
        let ids: Vec<&str> = found.values().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["todo", "notes"], "no dot folder, build output, nested or too deep project");
        let var = format!("{}", d.path().join("elsewhere").display());
        let roots = roots_for(&var, &home, Some(&home.join("code")), &[home.join("code/gen/notes")]);
        assert_eq!(roots[0], d.path().join("elsewhere"), "named roots as given, first");
        assert!(roots.contains(&home.join("code")) && roots.contains(&home.join("code/gen")));
        assert!(scan_disk(&roots, DEPTH, &[]).values().any(|m| m.id == "x"));
        let never = vec![home.join("code")];
        assert!(
            !scan_disk(std::slice::from_ref(&home), DEPTH, &never).values().any(|m| m.id == "notes"),
            "never walked"
        );
        assert!(
            never_walk(&home).iter().any(|p| same(p, Path::new(if cfg!(windows) { "C:\\Windows" } else { "/proc" })))
        );
        assert!(roots_example().contains(ROOTS_VAR));
    }

    /// amui's `discover runs the real scan cleanly`: the apps that ran,
    /// matched to their projects, then those that never ran; itself marked.
    #[test]
    fn the_apps_that_ran_meet_their_projects_and_ax_ui_is_itself() {
        let j = json!({ "ok": true, "value": [
            { "name": "notes", "home": "/h/notes", "running": true, "pid": 42, "port": 8080, "version": "0.1.0", "startedBy": "ax start (dev, from a terminal)" },
            { "name": "notes", "home": "/h/i/2/notes", "running": false, "instance": "2" },
            { "name": "aio-ui", "home": "/h/aio-ui", "running": true, "pid": 7 },
        ]});
        let all = apps(&j.to_string()).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!((all[0].id.as_str(), all[1].id.as_str()), ("notes", "notes#2"));
        assert_eq!(all[0].running.as_ref().map(|r| (r.pid, r.port)), Some((42, Some(8080))));
        assert!(all[2].own && !all[0].own);
        assert!(apps("no aio-rs apps yet").unwrap_err().starts_with("ax apps:"));
        let d = tempfile::tempdir().unwrap();
        project(&d.path().join("notes"), "notes", "");
        project(&d.path().join("other-notes"), "notes", "");
        project(&d.path().join("todo"), "todo", "");
        let disk = scan_disk(&[d.path().to_path_buf()], 1, &[]);
        let cwd = d.path().join("other-notes");
        let list = merge(all, &disk, &|pid| (pid == 42).then(|| cwd.clone()));
        let names: Vec<&str> = list.iter().map(|p| p.id.as_str()).collect();
        let todo = d.path().join("todo").display().to_string();
        assert_eq!(names, ["aio-ui", "notes", "notes#2", todo.as_str()], "the running first, by name");
        assert_eq!(list[1].path.as_deref(), Some(cwd.as_path()), "the running one's own folder");
        assert!(list[3].running.is_none() && list[3].meta.id == "todo", "never ran: its folder");
        assert_eq!(find(&list, &todo).map(|p| p.name.as_str()), Some("todo"));
        assert_eq!(find(&list, "nope"), None);
        assert_eq!(follow(&list, "notes#2").map(|p| p.id.as_str()), Some("notes#2"));
        // amui's order: the running first, whatever their names.
        let one = |id: &str, up: bool| Project {
            id: id.into(),
            name: id.into(),
            running: up.then(|| Running { pid: 9, ..Running::default() }),
            ..Project::default()
        };
        let list = merge(vec![one("alpha", false), one("zeta", true)], &BTreeMap::new(), &|_| None);
        assert_eq!(list.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["zeta", "alpha"]);
    }
}

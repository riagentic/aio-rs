//! The manager's brain (amui's `manager.ts`): the list of apps, the chosen
//! one's detail and its live readings, its state, logs, tasks and files,
//! and what the person asked it to do. Every field is a signal of its own,
//! so a view re-renders only for what it reads (aio's `part()`). The work
//! runs off the view's thread: `ax` verbs as children (what `ax` does, done
//! the way `ax` does it) and plain file reads; a result that comes back
//! after the person moved on is dropped, never shown on another app.
use super::proc::{self, FileNode, LogSource, Runtime};
use super::scan::{self, Project};
use aio::serde_json::{self, Value, json};
use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Runs one `ax` verb (its words, in a folder): what it printed, or why it
/// failed.
pub type Verb = Arc<dyn Fn(Vec<String>, Option<PathBuf>) -> Result<String, String> + Send + Sync>;

/// The manager's verbs: this `ax`, as a child.
pub fn child() -> Verb {
    Arc::new(|args: Vec<String>, cwd: Option<PathBuf>| {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut cmd = std::process::Command::new(exe);
        cmd.args(&args).stdin(std::process::Stdio::null());
        if let Some(d) = cwd.filter(|d| d.is_dir()) {
            cmd.current_dir(d);
        }
        let o = cmd.output().map_err(|e| e.to_string())?;
        let (out, err) = (String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
        let out = out.trim_end().to_string();
        match (o.status.success(), err.trim()) {
            (true, _) => Ok(out),
            (false, "") => Err(out),
            (false, e) => Err(e.trim_start_matches("ax: ").to_string()),
        }
    })
}

/// `verb` for one copy of an app: a private instance's words carry its
/// `--instance` (after the verb's name).
pub fn scoped(verb: &Verb, instance: Option<&str>) -> Verb {
    match instance {
        None => verb.clone(),
        Some(i) => {
            let (verb, i) = (verb.clone(), i.to_string());
            Arc::new(move |mut w: Vec<String>, cwd| {
                w.splice(1..1, ["--instance".to_string(), i.clone()]);
                verb(w, cwd)
            })
        }
    }
}

/// Where the manager looks and what it runs (tests give their own).
#[derive(Clone)]
pub struct Setup {
    pub verb: Verb,
    /// What a task runs (`ax` itself).
    pub program: PathBuf,
    pub home: PathBuf,
    /// [`scan::ROOTS_VAR`]'s value.
    pub roots: String,
    /// The folder ax ui runs in.
    pub cwd: Option<PathBuf>,
    /// Where installed apps are.
    pub installed: PathBuf,
    /// Where a new app is made (amui's `~/aio-apps`).
    pub create_in: PathBuf,
    /// Where the person's choices are kept (`None`: for this run only).
    pub prefs: Option<PathBuf>,
}

impl Setup {
    /// This machine's.
    pub fn here(verb: Verb, prefs: Option<PathBuf>) -> Self {
        let home = std::env::home_dir().unwrap_or_default();
        Setup {
            verb,
            program: std::env::current_exe().unwrap_or_else(|_| "ax".into()),
            roots: std::env::var(scan::ROOTS_VAR).unwrap_or_default(),
            cwd: std::env::current_dir().ok(),
            installed: aio::paths::tool_home().join("install"),
            create_in: home.join("aio-apps"),
            home,
            prefs,
        }
    }
}

/// A cell of the running app: its methods (name, Rust signature) and which
/// fields it saves and sends (`ax peer`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CellInfo {
    pub name: String,
    pub methods: Vec<(String, String)>,
    /// Saved at all (`None`: not said).
    pub persist: Option<bool>,
    pub server: Vec<String>,
    pub transient: Vec<String>,
}

impl CellInfo {
    /// Is field `k` saved, and sent to the window (amui's per-field flags)?
    pub fn flags(&self, k: &str) -> (bool, bool) {
        let s = |v: &[String]| v.iter().any(|f| f == k);
        (self.persist.unwrap_or(true) && !s(&self.transient), !s(&self.server))
    }
}

/// The cells of `ax peer --json` (pure).
pub fn cells_of(peer: &Value) -> Vec<CellInfo> {
    let list = |v: &Value| v.as_array().into_iter().flatten().filter_map(|s| s.as_str().map(String::from)).collect();
    peer.as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| {
            Some(CellInfo {
                name: c["cell"].as_str()?.into(),
                methods: c["methods"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|m| {
                        let name = m["method"].as_str()?;
                        Some((name.to_string(), m["sig"].as_str().unwrap_or(name).to_string()))
                    })
                    .collect(),
                persist: c["fields"]["persist"].as_bool(),
                server: list(&c["fields"]["server"]),
                transient: list(&c["fields"]["transient"]),
            })
        })
        .collect()
}

/// The chosen app (amui's `ProjectDetail`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Detail {
    pub id: String,
    pub name: String,
    pub instance: Option<String>,
    pub path: Option<PathBuf>,
    pub home: Option<PathBuf>,
    pub running: bool,
    pub pid: Option<u32>,
    pub port: Option<u64>,
    /// started, degraded, starting, stopping…
    pub status: Option<String>,
    /// dev or release, from what started it.
    pub build: Option<&'static str>,
    pub meta: scan::Meta,
    pub aio: Option<String>,
    pub version: Option<String>,
    pub git: bool,
    pub own: bool,
    pub cells: Option<Vec<CellInfo>>,
    pub cpu_pct: Option<f64>,
    pub mem_mb: Option<f64>,
    pub uptime_s: Option<u64>,
    pub connections: Option<u64>,
    pub errors: Option<Vec<String>>,
    pub schedules: Option<Vec<String>>,
    /// When it was last read (ms).
    pub at: i64,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// What started it says how it was built (pure): `ax start (dev, …)`.
pub fn build_of(started_by: Option<&str>) -> Option<&'static str> {
    let s = started_by?;
    if s.contains("(dev") || s.contains("debug") {
        Some("dev")
    } else if s.contains("release") || s.contains("installed") {
        Some("release")
    } else {
        None
    }
}

/// A list entry's detail before anything is read (amui's `detailOf`, pure).
pub fn detail_of(p: &Project) -> Detail {
    let r = p.running.as_ref();
    Detail {
        id: p.id.clone(),
        name: p.name.clone(),
        instance: p.instance.clone(),
        path: p.path.clone(),
        home: p.home.clone(),
        running: r.is_some(),
        pid: r.map(|r| r.pid),
        port: r.and_then(|r| r.port),
        status: r.map(|_| "started".into()),
        build: build_of(r.and_then(|r| r.started_by.as_deref())),
        aio: r.and_then(|r| r.aio.clone()),
        version: r.and_then(|r| r.version.clone()).or(p.meta.version.clone()),
        meta: p.meta.clone(),
        git: p.git,
        own: p.own,
        at: now_ms(),
        ..Detail::default()
    }
}

/// A fresh scan's word on the chosen app folded into its detail (amui's
/// `reconcileDetail`, pure): `None` when nothing changed. A new process
/// keeps nothing of the old one's readings, and a stopped one none at all.
pub fn reconcile(d: &Detail, fresh: &Project) -> Option<Detail> {
    let r = fresh.running.as_ref();
    let (up, pid, port) = (r.is_some(), r.map(|r| r.pid), r.and_then(|r| r.port));
    if up == d.running && pid == d.pid && port == d.port && fresh.id == d.id {
        return None;
    }
    let same = up && pid == d.pid;
    let mut n = Detail {
        id: fresh.id.clone(),
        running: up,
        pid,
        port,
        status: r.map(|_| "started".into()),
        path: fresh.path.clone().or(d.path.clone()),
        build: build_of(r.and_then(|r| r.started_by.as_deref())).or(if same { d.build } else { None }),
        ..d.clone()
    };
    if !same {
        (n.cpu_pct, n.mem_mb, n.uptime_s, n.connections) = (None, None, None, None);
        (n.cells, n.errors, n.schedules) = (None, None, None);
    }
    Some(n)
}

/// The first failed read among a set, as the banner says it (amui's
/// `controlErrorOf`): reads are only made of an app that runs, so one it
/// didn't answer is news.
pub fn control_error_of(results: &[&Result<String, String>]) -> Option<String> {
    results.iter().find_map(|r| r.as_ref().err().cloned())
}

/// Rolling readings for the charts, at most [`HIST`] each.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hist {
    pub cpu: Vec<f64>,
    pub mem: Vec<f64>,
    /// The cells' state, KB (the Rust app's counterpart of aio's heap).
    pub state: Vec<f64>,
    /// The slowest cell's p95 call, ms (aio's p95 reduce).
    pub p95: Vec<f64>,
    /// Pushes to clients a second (aio's dispatch queue).
    pub pushes: Vec<f64>,
}

/// Samples kept per chart.
pub const HIST: usize = 60;
/// Actions kept.
pub const HISTORY_MAX: usize = 60;

fn keep(v: &mut Vec<f64>, x: Option<f64>) {
    if let Some(x) = x.filter(|x| x.is_finite()) {
        v.push(x);
        let over = v.len().saturating_sub(HIST);
        v.drain(..over);
    }
}

/// The live readings of a running app (amui's `Diag`): its `health.json`,
/// its cells' sizes and links (`ax metrics`), what they sent and how long
/// their calls took (`ax cost`), who is linked (`ax clients`), its newest
/// events (`ax events`) and its window's components (`ax components`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Diag {
    pub health: Option<Value>,
    pub metrics: Option<Value>,
    pub cost: Option<Value>,
    pub clients: Option<Vec<Value>>,
    pub history: Option<Vec<Value>>,
    pub components: Option<Value>,
    /// Did any read get an answer?
    pub answered: bool,
    pub control_error: Option<String>,
}

impl Diag {
    /// The cells' state, bytes.
    pub fn state_bytes(&self) -> Option<u64> {
        let m = self.metrics.as_ref()?["cells"].as_object()?;
        Some(m.values().filter_map(|c| c["bytes"].as_u64()).sum())
    }

    /// The slowest p95 call, and all pushes a second, of the cells.
    pub fn calls(&self) -> (Option<f64>, Option<f64>) {
        let Some(c) = self.cost.as_ref().and_then(Value::as_object) else { return (None, None) };
        let p95 = c.values().filter_map(|v| v["p95Ms"].as_f64()).fold(0.0, f64::max);
        (Some(p95), Some(c.values().filter_map(|v| v["pushesPerS"].as_f64()).sum()))
    }
}

/// Read app `p`'s home file `name` as JSON.
fn home_json(home: Option<&Path>, name: &str) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(home?.join("data").join(name)).ok()?).ok()
}

/// The readings of a running app now (`verb` already scoped to its copy).
pub fn diag(verb: &Verb, name: &str, home: Option<&Path>, cwd: Option<PathBuf>) -> Diag {
    let w = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let (metrics, cost, events, comps) = std::thread::scope(|s| {
        let run = |a: Vec<String>| {
            let (v, c) = (verb.clone(), cwd.clone());
            s.spawn(move || v(a, c))
        };
        let m = run(w(&["metrics", name, "--json"]));
        let c = run(w(&["cost", name, "--json"]));
        let e = run(w(&["events", name, "--last", "60", "--json"]));
        let u = run(w(&["components", name, "--json"]));
        let j =
            |h: std::thread::ScopedJoinHandle<'_, Result<String, String>>| h.join().unwrap_or(Err("panicked".into()));
        (j(m), j(c), j(e), j(u))
    });
    let value = |r: &Result<String, String>| {
        r.as_ref()
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(t).ok())
            .map(|v| v.get("value").cloned().unwrap_or(v))
    };
    let metrics_v = value(&metrics);
    let cells: Vec<String> = metrics_v
        .as_ref()
        .and_then(|m| m["cells"].as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    // Who is linked, each cell's (at most 8 cells asked).
    let mut clients: Vec<Value> = vec![];
    for c in cells.iter().take(8) {
        let rows = value(&verb(w(&["clients", name, c, "--json"]), cwd.clone())).and_then(|v| v.as_array().cloned());
        for mut r in rows.into_iter().flatten().filter(Value::is_object) {
            r["cell"] = c.clone().into();
            clients.push(r);
        }
    }
    let history = value(&events).and_then(|v| v.as_array().cloned()).map(|mut v| {
        let over = v.len().saturating_sub(HISTORY_MAX);
        v.drain(..over);
        v
    });
    Diag {
        health: home_json(home, "health.json"),
        answered: metrics.is_ok() || cost.is_ok() || events.is_ok(),
        control_error: control_error_of(&[&metrics, &cost]),
        clients: metrics.is_ok().then_some(clients),
        metrics: metrics_v,
        cost: value(&cost),
        components: value(&comps).filter(|v| v["components"].is_array()),
        history,
    }
}

/// An app's status from its `health.json` (pure).
pub fn status_of(health: Option<&Value>) -> String {
    match health.and_then(|h| h["status"].as_str()) {
        Some("healthy") => "started".into(),
        Some(s) => s.into(),
        None => "starting".into(),
    }
}

/// One parsed log line (amui's `LogLine`); a line that isn't one of aio's
/// is shown as it is (`ts` and `level` empty).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LogLine {
    pub ts: String,
    pub level: String,
    pub scope: String,
    pub msg: String,
    pub raw: String,
}

/// A log line split into its time, level, `[scope]` and text (amui's
/// `parseLogLine`, pure): `2026-11-01T10:00:00.000Z WARN  [page] slow`; no
/// level word is info; a launch capture's lines have no time.
pub fn parse_log_line(raw: &str) -> LogLine {
    let (ts, rest) = match raw.split_once(' ') {
        Some((t, r)) if t.len() >= 20 && t.as_bytes()[4] == b'-' && t.contains('T') => (t, r),
        _ => ("", raw),
    };
    let (level, rest) = match rest.split_once(' ') {
        Some((w, r)) if matches!(w, "TRACE" | "DEBUG" | "INFO" | "WARN" | "ERROR" | "PERF") => {
            (w.to_ascii_lowercase(), r.trim_start())
        }
        _ => ("info".into(), rest),
    };
    match rest.strip_prefix('[').and_then(|r| r.split_once("] ").or_else(|| r.strip_suffix(']').map(|s| (s, "")))) {
        Some((scope, msg)) if !scope.contains(' ') => {
            LogLine { ts: ts.into(), level, scope: scope.into(), msg: msg.into(), raw: raw.into() }
        }
        _ if !ts.is_empty() => {
            LogLine { ts: ts.into(), level, scope: String::new(), msg: rest.into(), raw: raw.into() }
        }
        _ => LogLine { msg: raw.into(), raw: raw.into(), ..LogLine::default() },
    }
}

/// The State tab's load (amui's `detailState*`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StateLoad {
    pub value: Option<Value>,
    /// The entry it belongs to.
    pub of: Option<String>,
    pub size: usize,
    pub truncated: bool,
    pub loading: bool,
    pub error: Option<String>,
}

/// Above this a state is not drawn (it would freeze the tree).
pub const STATE_MAX: usize = 2_000_000;

/// The task runner's (amui's `task*`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Task {
    pub running: Option<String>,
    pub output: String,
    /// Its exit code; `Some(-1)`: stopped.
    pub code: Option<i32>,
}

/// The tasks of a project: `ax` verbs on its source (amui's deno tasks):
/// a label, the words after `ax` (`{app}`: the app).
pub const TASKS: &[(&str, &[&str])] = &[
    ("dev", &["dev", "{app}"]),
    ("dev --web", &["dev", "{app}", "--web"]),
    ("test", &["test", "{app}"]),
    ("lint", &["lint"]),
    ("build", &["build", "{app}"]),
    ("build --web", &["build", "{app}", "--web"]),
    ("build --sizes", &["build", "{app}", "--sizes"]),
    ("doctor", &["doctor", "{app}"]),
    ("config", &["config", "{app}"]),
    ("data", &["data", "{app}"]),
];

/// The words of task `label` for app `app` (pure): a build target's
/// (`build:<target>`) too.
pub fn task_words(label: &str, app: &str) -> Option<Vec<String>> {
    let fill = |w: &[&str]| w.iter().map(|w| w.replace("{app}", app)).collect();
    match label.strip_prefix("build:") {
        Some(t) => Some(["build", app, "--targets", t].map(String::from).to_vec()),
        None => TASKS.iter().find(|(l, _)| *l == label).map(|(_, w)| fill(w)),
    }
}

/// The file viewer's (amui's `fileTree`, `runtime`, `repoRoot`, `openFile*`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Files {
    pub tree: Option<Vec<FileNode>>,
    pub truncated: bool,
    pub runtime: Option<Runtime>,
    pub repo: Option<PathBuf>,
    pub repo_tree: Option<Vec<FileNode>>,
    pub repo_truncated: bool,
    pub repo_loading: bool,
}

/// The file open in the viewer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Open {
    pub path: String,
    pub base: PathBuf,
    pub content: Option<String>,
    pub notice: Option<String>,
    /// e.g. "cell "todo" defined at line 9".
    pub hint: Option<String>,
}

/// The new name of `raw` (amui's `create` slug, pure): lowercase, letters,
/// digits and dashes, none leading or trailing.
pub fn slug(raw: &str) -> String {
    let s: String = raw
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() { c } else { '-' })
        .collect();
    s.trim_matches('-').to_string()
}

/// The words of `ax call` for `cell.method` with `args`, a JSON list (empty:
/// none), and how the confirmation names the call (pure).
pub fn call_words(app: &str, cell: &str, method: &str, args: &str) -> Result<(Vec<String>, String), String> {
    let list: Vec<Value> = match args.trim() {
        "" => vec![],
        a => serde_json::from_str(a).map_err(|_| "the arguments are a JSON list: [1, \"two\"]".to_string())?,
    };
    let shown = Value::from(list.clone()).to_string();
    let note = if list.is_empty() { " It will be called with NO arguments." } else { "" };
    let words = ["call", app, cell, method, "--args", &shown].map(String::from).to_vec();
    Ok((words, format!("Run {cell}.{method}({}) on {app}?{note}", &shown[1..shown.len() - 1])))
}

/// Run `f` off the view's thread.
async fn off<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    match aio::rt().spawn_blocking(f).await {
        Ok(t) => t,
        Err(e) => std::panic::resume_unwind(e.into_panic()),
    }
}

/// The manager: each field a signal of its own. Its work runs at the
/// root (`spawn_forever`): a tab that asked and closed never drops the answer.
#[derive(Clone, Copy)]
pub struct Manager {
    setup: CopyValue<Setup>,
    pub projects: Signal<Vec<Project>>,
    pub selected: Signal<Option<String>>,
    pub detail: Signal<Option<Detail>>,
    pub detail_loading: Signal<bool>,
    pub scanning: Signal<bool>,
    pub scanned: Signal<bool>,
    pub scan_roots: Signal<Vec<String>>,
    pub hist: Signal<Hist>,
    pub diag: Signal<Diag>,
    pub control_error: Signal<Option<String>>,
    ticking: Signal<bool>,
    pub logs: Signal<Option<Vec<LogLine>>>,
    pub log_path: Signal<Option<PathBuf>>,
    pub log_source: Signal<LogSource>,
    pub log_follow: Signal<bool>,
    pub log_loading: Signal<bool>,
    pub log_error: Signal<Option<String>>,
    pub log_truncated: Signal<bool>,
    pub state: Signal<StateLoad>,
    pub task: Signal<Task>,
    cancel: Signal<Option<Arc<AtomicBool>>>,
    pub files: Signal<Files>,
    pub open: Signal<Option<Open>>,
    pub create_busy: Signal<bool>,
    pub create_msg: Signal<Option<String>>,
    pub action_msg: Signal<Option<String>>,
    pub dispatch_msg: Signal<Option<String>>,
    pub show_all: Signal<bool>,
}

impl Manager {
    /// A manager on `setup` (in a component's hook: its signals live there).
    pub fn new(setup: Setup) -> Self {
        let all = setup
            .prefs
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .is_some_and(|v| v["showAll"] == true);
        Manager {
            setup: CopyValue::new(setup),
            projects: Signal::new(vec![]),
            selected: Signal::new(None),
            detail: Signal::new(None),
            detail_loading: Signal::new(false),
            scanning: Signal::new(false),
            scanned: Signal::new(false),
            scan_roots: Signal::new(vec![]),
            hist: Signal::new(Hist::default()),
            diag: Signal::new(Diag::default()),
            control_error: Signal::new(None),
            ticking: Signal::new(false),
            logs: Signal::new(None),
            log_path: Signal::new(None),
            log_source: Signal::new(LogSource::default()),
            log_follow: Signal::new(false),
            log_loading: Signal::new(false),
            log_error: Signal::new(None),
            log_truncated: Signal::new(false),
            state: Signal::new(StateLoad::default()),
            task: Signal::new(Task::default()),
            cancel: Signal::new(None),
            files: Signal::new(Files::default()),
            open: Signal::new(None),
            create_busy: Signal::new(false),
            create_msg: Signal::new(None),
            action_msg: Signal::new(None),
            dispatch_msg: Signal::new(None),
            show_all: Signal::new(all),
        }
    }

    fn setup(&self) -> Setup {
        self.setup.read().clone()
    }

    /// The verb for entry `p`'s copy, and the folder its verbs run in: its
    /// project (a workspace names it by its id), else none of the person's.
    fn verb_for(&self, instance: Option<&str>, path: Option<&Path>) -> (Verb, Option<PathBuf>) {
        let s = self.setup();
        (scoped(&s.verb, instance), path.map(Path::to_path_buf).or_else(|| Some(aio::paths::tool_home())))
    }

    /// Keep the person's choice of all apps or the running ones.
    pub fn set_show_all(mut self, on: bool) {
        self.show_all.set(on);
        if let Some(p) = self.setup().prefs {
            _ = std::fs::write(p, json!({ "showAll": on }).to_string());
        }
    }

    /// Find the apps again (amui's `discover`).
    pub fn discover(mut self) {
        if *self.scanning.peek() {
            return;
        }
        self.scanning.set(true);
        let s = self.setup();
        spawn_forever(async move {
            let found = off(move || {
                let listed = (s.verb)(vec!["apps".into(), "--json".into()], Some(aio::paths::tool_home()))
                    .and_then(|j| scan::apps(&j));
                let apps = listed.unwrap_or_default();
                let cwds: Vec<PathBuf> =
                    apps.iter().filter_map(|a| proc::process_paths(a.running.as_ref()?.pid).1).collect();
                let roots = scan::roots_for(&s.roots, &s.home, s.cwd.as_deref(), &cwds);
                let mut disk = scan::scan_disk(&roots, scan::DEPTH, &scan::never_walk(&s.home));
                // A running app is found where it runs, wherever that is.
                disk.extend(cwds.iter().filter_map(|c| Some((c.clone(), scan::read_meta(c)?))));
                let list = scan::merge(apps, &disk, &|pid| proc::process_paths(pid).1);
                (list, roots.iter().map(|r| r.display().to_string()).collect::<Vec<_>>())
            })
            .await;
            self.apply_scan(found.0, found.1);
            self.scanning.set(false);
            self.scanned.set(true);
        });
    }

    /// Fold a fresh scan in (amui's `applyScan`): the list, and the chosen
    /// app followed, dropped if gone, its running state reconciled (its
    /// readings cleared when its process changed).
    pub fn apply_scan(mut self, found: Vec<Project>, roots: Vec<String>) {
        let sel = self.selected.peek().clone();
        let next = sel.as_deref().and_then(|id| scan::follow(&found, id)).cloned();
        self.scan_roots.set(roots);
        match (sel, next) {
            (Some(_), None) => {
                self.selected.set(None);
                self.detail.set(None);
            }
            (Some(id), Some(p)) => {
                if p.id != id {
                    self.selected.set(Some(p.id.clone()));
                }
                let changed = self.detail.peek().as_ref().and_then(|d| reconcile(d, &p));
                if let Some(d) = changed {
                    let up = d.running;
                    self.detail.set(Some(d));
                    self.clear_live();
                    if up {
                        self.select(&p.id);
                    }
                }
            }
            _ => {}
        }
        if *self.projects.peek() != found {
            self.projects.set(found);
        }
    }

    /// Drop every reading of a process that is gone (amui's
    /// `clearLiveDiagnostics`).
    pub fn clear_live(mut self) {
        self.hist.set(Hist::default());
        self.diag.set(Diag::default());
        self.control_error.set(None);
        self.state.set(StateLoad::default());
    }

    /// Choose an app (amui's `select`): its skeleton at once, then what is
    /// read of it; a newer choice drops this one's results.
    pub fn select(mut self, key: &str) {
        let Some(p) = scan::find(&self.projects.peek(), key).cloned() else { return };
        let id = p.id.clone();
        self.selected.set(Some(id.clone()));
        self.detail.set(Some(detail_of(&p)));
        self.detail_loading.set(true);
        self.files.set(Files::default());
        self.open.set(None);
        self.dispatch_msg.set(None);
        self.state.set(StateLoad::default());
        self.control_error.set(None);
        self.hist.set(Hist::default());
        self.diag.set(Diag::default());
        self.logs.set(None);
        self.log_path.set(None);
        self.log_error.set(None);
        self.log_truncated.set(false);
        self.log_follow.set(false);
        let (verb, cwd) = self.verb_for(p.instance.as_deref(), p.path.as_deref());
        let s = self.setup();
        spawn_forever(async move {
            let got = off(move || {
                let mut d = detail_of(&p);
                let r = p.running.clone();
                let (exe, run_cwd) = r.as_ref().map_or((None, None), |r| proc::process_paths(r.pid));
                let runtime = match &r {
                    Some(_) => proc::runtime_of(exe.as_deref(), p.path.as_deref().or(run_cwd.as_deref()), &s.installed),
                    None => Runtime {
                        kind: "source",
                        root: p.path.clone().unwrap_or_default(),
                        exe: None,
                        label: "not running — project source".into(),
                    },
                };
                let (tree, truncated) = proc::list_files(&runtime.root);
                let repo = p.path.as_deref().and_then(|d| proc::find_repo_root(d, &s.home));
                let files = Files { tree: Some(tree), truncated, runtime: Some(runtime), repo, ..Files::default() };
                let mut live = Diag::default();
                if let Some(r) = &r {
                    let w = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
                    let name = p.name.as_str();
                    let (peer, errors, schedules, dg) = std::thread::scope(|sc| {
                        let run = |a: Vec<String>| {
                            let (v, c) = (verb.clone(), cwd.clone());
                            sc.spawn(move || v(a, c))
                        };
                        let pe = run(w(&["peer", name, "--json"]));
                        let er = run(w(&["errors", name, "--last", "6"]));
                        let sh = run(w(&["schedules", name, "--json"]));
                        let dg = diag(&verb, name, p.home.as_deref(), cwd.clone());
                        let j = |h: std::thread::ScopedJoinHandle<'_, Result<String, String>>| {
                            h.join().unwrap_or(Err("panicked".into()))
                        };
                        (j(pe), j(er), j(sh), dg)
                    });
                    let json_of = |r: &Result<String, String>| {
                        r.as_ref()
                            .ok()
                            .and_then(|t| serde_json::from_str::<Value>(t).ok())
                            .map(|v| v.get("value").cloned().unwrap_or(v))
                    };
                    d.cells = json_of(&peer).map(|v| cells_of(&v));
                    d.errors = errors.as_ref().ok().map(|t| errors_of(t));
                    d.schedules = json_of(&schedules).map(|v| schedules_of(&v));
                    let ps = proc::ps_stats(r.pid);
                    fill(&mut d, &dg, ps);
                    live = Diag {
                        control_error: dg.control_error.clone().or(control_error_of(&[&peer])),
                        answered: dg.answered || peer.is_ok(),
                        ..dg
                    };
                }
                (d, files, live)
            })
            .await;
            if self.selected.peek().as_deref() != Some(id.as_str()) {
                return;
            }
            let (d, files, diag) = got;
            let mut h = Hist::default();
            if d.running {
                sample(&mut h, &d, &diag);
            }
            self.control_error.set(diag.control_error.clone());
            self.detail.set(Some(d));
            self.files.set(files);
            self.diag.set(diag);
            self.hist.set(h);
            self.detail_loading.set(false);
        });
    }

    /// Read the chosen running app again for the charts and live fields
    /// (amui's `tick`): one at a time, never two over the same history.
    pub fn tick(mut self) {
        let Some(d) = self.detail.peek().clone().filter(|d| d.running && d.pid.is_some()) else { return };
        if *self.ticking.peek() {
            return;
        }
        self.ticking.set(true);
        let (verb, cwd) = self.verb_for(d.instance.as_deref(), d.path.as_deref());
        let (pid, id, name, home) = (d.pid.unwrap_or(0), d.id.clone(), d.name.clone(), d.home.clone());
        spawn_forever(async move {
            let (dg, ps) = off(move || (diag(&verb, &name, home.as_deref(), cwd), proc::ps_stats(pid))).await;
            self.ticking.set(false);
            let Some(mut d) = self.detail.peek().clone().filter(|d| d.id == id && d.running) else { return };
            fill(&mut d, &dg, ps);
            if !dg.answered {
                // A reading that can't be checked is no reading.
                (d.uptime_s, d.connections) = (None, None);
            }
            self.hist.with_mut(|h| sample(h, &d, &dg));
            self.control_error.set(dg.control_error.clone());
            let kept = self.diag.peek().clone();
            // Each part degrades on its own: the last good one stays.
            self.diag.set(Diag {
                health: dg.health.or(kept.health),
                metrics: dg.metrics.or(kept.metrics),
                cost: dg.cost.or(kept.cost),
                clients: dg.clients.or(kept.clients),
                history: dg.history.or(kept.history),
                components: dg.components.or(kept.components),
                ..dg
            });
            self.detail.set(Some(d));
        });
    }

    /// The chosen app's whole state for the State tab (amui's `loadState`):
    /// on demand only, never polled; too large is refused, not drawn.
    pub fn load_state(mut self, key: &str) {
        let Some(p) = scan::find(&self.projects.peek(), key).cloned() else {
            self.state.set(StateLoad {
                of: Some(key.into()),
                error: Some("app not running".into()),
                ..StateLoad::default()
            });
            return;
        };
        if p.running.is_none() {
            self.state.set(StateLoad { of: Some(p.id), error: Some("app not running".into()), ..StateLoad::default() });
            return;
        }
        self.state.with_mut(|s| (s.loading, s.error) = (true, None));
        let (verb, cwd) = self.verb_for(p.instance.as_deref(), p.path.as_deref());
        let (id, pid) = (p.id.clone(), p.running.as_ref().map(|r| r.pid));
        spawn_forever(async move {
            let r = off(move || verb(vec!["snapshot".into(), p.name.clone(), "--json".into()], cwd)).await;
            if self.selected.peek().as_deref() != Some(id.as_str())
                || self.detail.peek().as_ref().and_then(|d| d.pid) != pid
            {
                self.state.with_mut(|s| s.loading = false);
                return;
            }
            let mut s = StateLoad { of: Some(id), ..StateLoad::default() };
            match r.and_then(|t| serde_json::from_str::<Value>(&t).map_err(|e| e.to_string())) {
                Err(e) => s.error = Some(e),
                Ok(v) => {
                    let v = v.get("value").cloned().unwrap_or(v);
                    s.size = v.to_string().len();
                    s.truncated = s.size > STATE_MAX;
                    s.value = (!s.truncated).then_some(v);
                }
            }
            self.state.set(s);
        });
    }

    /// The chosen app's log tail (amui's `loadLogs`): on demand and on the
    /// follow poll; `source` picks another log.
    pub fn load_logs(mut self, key: &str, source: Option<LogSource>) {
        let Some(p) = scan::find(&self.projects.peek(), key).cloned() else { return };
        let src = source.unwrap_or(*self.log_source.peek());
        self.log_source.set(src);
        self.log_loading.set(true);
        self.log_error.set(None);
        let id = p.id.clone();
        spawn_forever(async move {
            let raw = off(move || p.logs().map(|l| proc::read_logs(&l, src, 500))).await;
            self.log_loading.set(false);
            if self.selected.peek().as_deref() != Some(id.as_str()) {
                return;
            }
            let raw = raw.unwrap_or(proc::RawLog { missing: true, ..Default::default() });
            let word = match src {
                LogSource::Combined => "combined",
                LogSource::App => "app",
                LogSource::Error => "error",
                LogSource::Client => "client",
            };
            self.log_error
                .set(raw.missing.then(|| format!("no {word} log found (the app may not have written one yet)")));
            self.log_path.set(raw.path);
            self.log_truncated.set(raw.truncated);
            self.logs.set(Some(if raw.missing {
                vec![]
            } else {
                raw.lines.iter().map(|l| parse_log_line(l)).collect()
            }));
        });
    }

    /// Call a cell's method on the running app (amui's `dispatch`).
    pub fn dispatch(mut self, key: &str, cell: &str, method: &str, args: &str) {
        let Some(p) = scan::find(&self.projects.peek(), key).cloned().filter(|p| p.running.is_some()) else { return };
        let (words, _) = match call_words(&p.name, cell, method, args) {
            Ok(c) => c,
            Err(e) => return self.dispatch_msg.set(Some(e)),
        };
        let (verb, cwd) = self.verb_for(p.instance.as_deref(), p.path.as_deref());
        let call = format!("{cell}.{method}");
        spawn_forever(async move {
            let r = off(move || verb(words, cwd)).await;
            self.dispatch_msg.set(Some(match r {
                Ok(a) if a.is_empty() || a == "null" => format!("called {call}"),
                Ok(a) => format!("called {call} → {a}"),
                Err(e) => format!("error: {e}"),
            }));
        });
    }

    /// Why ax ui won't start or stop entry `p`: it is ax ui itself.
    fn refuse_self(&mut self, p: &Project) -> bool {
        if p.own {
            self.action_msg.set(Some("that's ax ui itself — manage it from the shell that launched it".into()));
        }
        p.own
    }

    /// Start, stop or restart the app (amui's `start`, `stop`, `restart`):
    /// its own verb says how it went; the list is read again after.
    pub fn lifecycle(mut self, key: &str, what: &'static str) {
        let Some(p) = scan::find(&self.projects.peek(), key).cloned() else { return };
        if self.refuse_self(&p) {
            return;
        }
        let mut words = vec![what.to_string(), p.name.clone()];
        if what == "start" {
            // The person's own desktop, as ax ui is on it.
            words.extend(["--display".to_string(), "current".to_string()]);
        }
        let ing = match what {
            "stop" => "stopping",
            "restart" => "restarting",
            _ => "starting",
        };
        self.action_msg.set(Some(format!("{ing} {}…", p.name)));
        let cur = self.detail.peek().clone();
        if what == "stop"
            && let Some(mut d) = cur.filter(|d| d.id == p.id)
        {
            d.status = Some("stopping".into());
            self.detail.set(Some(d));
        }
        let (verb, cwd) = self.verb_for(p.instance.as_deref(), p.path.as_deref());
        spawn_forever(async move {
            let r = off(move || verb(words, cwd)).await;
            self.action_msg.set(Some(match r {
                Ok(t) if t.is_empty() => format!("{what}: done"),
                Ok(t) => t,
                Err(e) => format!("{what} failed: {e}"),
            }));
            self.discover();
        });
    }

    /// Run task `label` in the app's project (amui's `runTask`): its
    /// output captured; cancellable, and never longer than 5 min.
    pub fn run_task(mut self, key: &str, label: &str) {
        let Some(p) = scan::find(&self.projects.peek(), key).cloned() else { return };
        let (Some(dir), Some(mut words)) = (p.path.clone(), task_words(label, &p.name)) else {
            return self
                .task
                .set(Task { output: format!("{label}: {} has no project folder here", p.name), ..Task::default() });
        };
        if self.task.peek().running.is_some() {
            return;
        }
        if let Some(i) = &p.instance {
            words.splice(1..1, ["--instance".to_string(), i.clone()]);
        }
        let shown = format!("$ ax {}", words.join(" "));
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel.set(Some(cancel.clone()));
        self.task.set(Task {
            running: Some(label.into()),
            output: format!("{shown}\n(running — cancellable)…"),
            code: None,
        });
        let program = self.setup().program;
        spawn_forever(async move {
            let r = off(move || proc::run_task(&dir, &program, &words, &cancel, proc::TASK_TIMEOUT)).await;
            let tag = match r.ended {
                "cancelled" => "cancelled".to_string(),
                "timeout" => "timed out (5 min cap)".into(),
                _ => r.code.map_or("ended by a signal".into(), |c| format!("exit {c}")),
            };
            self.cancel.set(None);
            self.task.set(Task {
                running: None,
                output: format!("{shown}\n{}\n\n[{tag}]", r.output),
                code: Some(r.code.unwrap_or(-1)),
            });
        });
    }

    /// Stop the running task (amui's `cancelTask`).
    pub fn cancel_task(&self) {
        if let Some(c) = self.cancel.peek().as_ref() {
            c.store(true, Ordering::Relaxed);
        }
    }

    /// The enclosing repo's tree for the Codebase tab (amui's `loadCodebase`).
    pub fn load_codebase(mut self) {
        let Some(root) = self.files.peek().repo.clone() else { return };
        let id = self.selected.peek().clone();
        self.files.with_mut(|f| f.repo_loading = true);
        spawn_forever(async move {
            let (tree, cut) = off(move || proc::list_files(&root)).await;
            if *self.selected.peek() != id {
                return;
            }
            self.files.with_mut(|f| (f.repo_tree, f.repo_truncated, f.repo_loading) = (Some(tree), cut, false));
        });
    }

    /// Open a file in the viewer (amui's `openFile`): read-only, guarded.
    pub fn open_file(mut self, base: PathBuf, rel: String) {
        spawn_forever(async move {
            let (b, r) = (base.clone(), rel.clone());
            let read = off(move || proc::read_file(&b, &r)).await;
            let (content, notice) = match read {
                Ok(t) => (Some(t), None),
                Err(e) => (None, Some(e)),
            };
            self.open.set(Some(Open { path: rel, base, content, notice, hint: None }));
        });
    }

    /// Open where cell `cell` is defined (amui's `openCellSource`).
    pub fn open_cell_source(mut self, cell: &str) {
        let f = self.files.peek().clone();
        let Some(base) = f.repo.or(f.runtime.map(|r| r.root)) else { return };
        let (cell, id) = (cell.to_string(), self.selected.peek().clone());
        self.open.set(None);
        spawn_forever(async move {
            let b = base.clone();
            let c = cell.clone();
            let found =
                off(move || proc::find_cell_source(&b, &c).map(|(rel, line)| (proc::read_file(&b, &rel), rel, line)))
                    .await;
            if *self.selected.peek() != id {
                return;
            }
            self.open.set(Some(match found {
                None => Open {
                    path: format!("cell \"{cell}\""),
                    base,
                    content: None,
                    notice: Some(format!("couldn't find where cell \"{cell}\" is defined in the source tree")),
                    hint: None,
                },
                Some((read, rel, line)) => Open {
                    path: rel,
                    base,
                    hint: Some(format!("cell \"{cell}\" defined at line {line}")),
                    notice: read.as_ref().err().cloned(),
                    content: read.ok(),
                },
            }));
        });
    }

    /// Make a new app (amui's `create`): `ax create` in `~/aio-apps`.
    pub fn create(mut self, raw: &str) {
        let name = slug(raw);
        if name.is_empty() {
            return self.create_msg.set(Some("enter a name".into()));
        }
        self.create_busy.set(true);
        self.create_msg.set(None);
        let s = self.setup();
        spawn_forever(async move {
            let dir = s.create_in.clone();
            let n = name.clone();
            let r = off(move || {
                std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                (s.verb)(vec!["create".into(), n], Some(dir))
            })
            .await;
            self.create_busy.set(false);
            self.create_msg.set(Some(match r {
                Ok(_) => format!("created {}", self.setup().create_in.join(&name).display()),
                Err(e) => format!("create failed: {e}"),
            }));
            self.discover();
        });
    }
}

/// The live fields of `d` from readings `dg` and the process's `ps`; where
/// `/proc` can't tell (macOS, Windows), the app's own `health.json`.
fn fill(d: &mut Detail, dg: &Diag, ps: Option<proc::Stats>) {
    let h = dg.health.as_ref();
    if d.status.as_deref() != Some("stopping") {
        d.status = Some(status_of(h));
    }
    d.cpu_pct = ps.map(|p| p.cpu_pct).or(d.cpu_pct);
    d.mem_mb =
        ps.map(|p| p.mem_mb).or(h.and_then(|h| h["memory"]["rssBytes"].as_f64()).map(|b| (b / 1_048_576.0).round()));
    d.aio = h.and_then(|h| h["aio"].as_str().map(String::from)).or(d.aio.take());
    if let Some(m) = &dg.metrics {
        d.uptime_s = m["uptimeS"].as_u64().or(d.uptime_s);
        d.connections = m["connections"].as_u64();
    } else if let Some(up) = h.and_then(|h| h["uptimeS"].as_u64()) {
        d.uptime_s = Some(up);
    }
    if dg.answered {
        d.at = now_ms();
    }
}

/// One sample of each chart from `d` and `dg`.
fn sample(h: &mut Hist, d: &Detail, dg: &Diag) {
    let (p95, pushes) = dg.calls();
    keep(&mut h.cpu, d.cpu_pct);
    keep(&mut h.mem, d.mem_mb);
    keep(&mut h.state, dg.state_bytes().map(|b| b as f64 / 1024.0));
    keep(&mut h.p95, p95);
    keep(&mut h.pushes, pushes);
}

/// The warnings and errors of `ax errors`, newest first (pure).
pub fn errors_of(text: &str) -> Vec<String> {
    let mut v: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|l| l.contains(" ERROR ") || l.contains(" WARN ") || l.contains("panicked"))
        .map(String::from)
        .collect();
    v.reverse();
    v.truncate(6);
    v
}

/// The schedules of `ax schedules --json`, one line each (pure).
pub fn schedules_of(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|s| match s {
            Value::String(t) => t.clone(),
            o => {
                let f = |k: &str| o[k].as_str().map(String::from).unwrap_or_else(|| o[k].to_string());
                format!("{} {}.{} {}", f("at"), f("cell"), f("method"), f("every"))
                    .replace("null", "")
                    .trim()
                    .to_string()
            }
        })
        .collect()
}

/// The entries `ax ui`'s sidebar lists (amui's `Sidebar` filter, pure):
/// those matching the search, else the running ones (or all), the chosen
/// one always.
pub fn listed<'a>(all: &'a [Project], q: &str, show_all: bool, selected: Option<&str>) -> Vec<&'a Project> {
    let q = q.trim().to_lowercase();
    all.iter()
        .filter(|p| {
            if q.is_empty() {
                show_all || p.running.is_some() || selected == Some(p.id.as_str())
            } else {
                p.name.to_lowercase().contains(&q)
                    || p.path.as_ref().is_some_and(|d| d.to_string_lossy().to_lowercase().contains(&q))
            }
        })
        .collect()
}

/// A JSON value's size as sent and stored, bytes (amui's `sizeOf`).
pub fn size_of(v: &Value) -> usize {
    v.to_string().len()
}

/// The children of a JSON value, keyed (pure).
pub fn entries(v: &Value) -> Vec<(String, &Value)> {
    match v {
        Value::Object(m) => m.iter().map(|(k, v)| (k.clone(), v)).collect(),
        Value::Array(a) => a.iter().enumerate().map(|(i, v)| (i.to_string(), v)).collect(),
        _ => vec![],
    }
}

/// A short preview of a value (amui's `preview`, pure).
pub fn preview(v: &Value) -> String {
    match v {
        Value::Array(a) => format!("[] {}", a.len()),
        Value::Object(m) => format!("{{}} {}", m.len()),
        Value::String(s) if s.chars().count() > 60 => format!("\"{}…\"", s.chars().take(60).collect::<String>()),
        Value::String(s) => format!("\"{s}\""),
        o => o.to_string(),
    }
}

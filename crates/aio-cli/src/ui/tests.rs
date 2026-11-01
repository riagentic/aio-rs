//! ax ui's tests (amui's `amui.test.ts`): the manager's rules as plain
//! functions, then the window itself, mounted with fake verbs and a
//! throwaway home.
use super::app::visible;
use super::manager::*;
use super::proc::FileNode;
use super::scan::{Meta, Project, Running};
use aio::serde_json::{Value, json};
use aio::testing::TestUi;
use dioxus::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn running(pid: u32) -> Option<Running> {
    Some(Running { pid, started_by: Some("ax start (dev, from a terminal)".into()), ..Running::default() })
}

fn project(id: &str, up: Option<Running>) -> Project {
    Project {
        id: id.into(),
        name: id.into(),
        meta: Meta { name: id.into(), id: id.into(), ..Meta::default() },
        running: up,
        ..Project::default()
    }
}

/// amui's `reconcileDetail` cases.
#[test]
fn a_fresh_scan_folds_into_the_detail_and_a_new_process_keeps_no_old_reading() {
    let mut d = detail_of(&project("notes", running(42)));
    (d.cpu_pct, d.mem_mb, d.uptime_s, d.connections) = (Some(3.0), Some(20.0), Some(9), Some(2));
    d.cells = Some(vec![]);
    assert_eq!(reconcile(&d, &project("notes", running(42))), None, "nothing changed: the same detail");
    let down = reconcile(&d, &project("notes", None)).unwrap();
    assert!(!down.running && down.pid.is_none() && down.status.is_none());
    assert_eq!((down.cpu_pct, down.uptime_s, down.cells.is_none()), (None, None, true), "a stopped app's readings go");
    let up = reconcile(&down, &project("notes", running(43))).unwrap();
    assert_eq!((up.running, up.pid, up.build), (true, Some(43), Some("dev")));
    let restarted = reconcile(&d, &project("notes", running(77))).unwrap();
    assert_eq!(
        (restarted.pid, restarted.cpu_pct, restarted.connections),
        (Some(77), None, None),
        "in place, a new pid"
    );
}

/// amui's `control plane: a refusal reaches the UI instead of an empty panel`.
#[test]
fn a_read_the_app_didnt_answer_is_said_never_shown_as_current() {
    let (ok, no) = (Ok("{}".to_string()), Err("notes isn't running (nothing was sent)".to_string()));
    assert_eq!(control_error_of(&[&ok, &ok]), None);
    assert_eq!(control_error_of(&[&ok, &no]).as_deref(), Some("notes isn't running (nothing was sent)"));
    assert_eq!(status_of(Some(&json!({ "status": "healthy" }))), "started");
    assert_eq!(status_of(Some(&json!({ "status": "degraded" }))), "degraded");
    assert_eq!(status_of(None), "starting");
}

/// amui's `create rejects a blank name` and `a dash-only name`.
#[test]
fn a_new_apps_name_is_made_safe_and_an_empty_one_refused() {
    assert_eq!(slug("  My Notes!  "), "my-notes");
    assert_eq!(slug("@@@"), "");
    assert_eq!(slug("   "), "");
    assert_eq!(slug("-x-"), "x", "never a leading dash (a flag)");
}

/// amui's `dispatch: a JSON array is the argument LIST, not one argument`.
#[test]
fn a_calls_arguments_are_a_json_list_and_the_confirmation_names_the_call() {
    let (w, msg) = call_words("notes", "board", "add", r#"["hi", 2]"#).unwrap();
    assert_eq!(w, ["call", "notes", "board", "add", "--args", r#"["hi",2]"#]);
    assert_eq!(msg, r#"Run board.add("hi",2) on notes?"#);
    let (w, msg) = call_words("notes", "board", "clear", " ").unwrap();
    assert_eq!(w.last().unwrap(), "[]");
    assert_eq!(msg, "Run board.clear() on notes? It will be called with NO arguments.");
    assert!(call_words("notes", "board", "add", "hi").unwrap_err().contains("JSON list"));
}

#[test]
fn tasks_are_ax_verbs_on_the_project_and_each_build_target_is_one() {
    assert_eq!(task_words("test", "notes").unwrap(), ["test", "notes"]);
    assert_eq!(task_words("dev --web", "notes").unwrap(), ["dev", "notes", "--web"]);
    assert_eq!(task_words("lint", "notes").unwrap(), ["lint"], "lint reads the folder it runs in");
    assert_eq!(task_words("build:deb", "notes").unwrap(), ["build", "notes", "--targets", "deb"]);
    assert_eq!(task_words("nope", "notes"), None);
}

#[test]
fn errors_schedules_cells_and_builds_read_as_shown() {
    let text = "start 1: 2026-11-01T10:00:00.000Z [aio] notes starting\n  2026-11-01T10:00:01.000Z WARN  [page] slow\n  2026-11-01T10:00:02.000Z ERROR [aio] boom\n";
    assert_eq!(
        errors_of(text),
        ["2026-11-01T10:00:02.000Z ERROR [aio] boom", "2026-11-01T10:00:01.000Z WARN  [page] slow"]
    );
    assert_eq!(
        schedules_of(&json!([{ "at": "10:00", "cell": "todo", "method": "tick", "every": "60s" }])),
        ["10:00 todo.tick 60s"]
    );
    let cells = cells_of(&json!([{ "cell": "todo", "v": 1,
        "methods": [{ "method": "add", "sig": "add(text: String) -> ()" }],
        "fields": { "persist": true, "server": ["owner"], "transient": ["draft"] } }]));
    assert_eq!(cells[0].methods, [("add".to_string(), "add(text: String) -> ()".to_string())]);
    assert_eq!(
        (cells[0].flags("items"), cells[0].flags("owner"), cells[0].flags("draft")),
        ((true, true), (true, false), (false, true))
    );
    let mem = CellInfo { persist: Some(false), ..CellInfo::default() };
    assert_eq!(mem.flags("x"), (false, true), "a cell kept in memory saves nothing");
    assert_eq!(
        (build_of(Some("ax start (dev, from no terminal)")), build_of(Some("other")), build_of(None)),
        (Some("dev"), None, None)
    );
}

/// amui's sidebar: the running apps by default, a search looks at all of
/// them, the chosen one stays listed.
#[test]
fn the_list_shows_the_running_apps_a_search_all_and_the_chosen_one_always() {
    let mut stopped = project("mail", None);
    stopped.path = Some(PathBuf::from("/w/apps/mail"));
    let all = [project("notes", running(1)), stopped, project("todo", None)];
    let ids = |v: Vec<&Project>| v.iter().map(|p| p.id.clone()).collect::<Vec<_>>();
    assert_eq!(ids(listed(&all, "", false, None)), ["notes"]);
    assert_eq!(ids(listed(&all, "", true, None)), ["notes", "mail", "todo"]);
    assert_eq!(ids(listed(&all, "", false, Some("todo"))), ["notes", "todo"]);
    assert_eq!(ids(listed(&all, "W/APPS", false, None)), ["mail"], "by folder too");
}

/// amui's `FileBrowser`: open by default, folded folders hide what is in
/// them, a filter shows each match with its folders.
#[test]
fn the_tree_folds_by_folder_and_a_filter_shows_matches_with_their_folders() {
    let n = |p: &str, dir: bool| FileNode { path: p.into(), name: p.rsplit('/').next().unwrap().into(), dir };
    let tree =
        [n("src", true), n("src/ui", true), n("src/ui/app.rs", false), n("src/lib.rs", false), n("aio.toml", false)];
    let paths = |v: Vec<&FileNode>| v.iter().map(|f| f.path.clone()).collect::<Vec<_>>();
    let none = std::collections::BTreeSet::new();
    assert_eq!(paths(visible(&tree, "", &none)).len(), 5);
    let ui = std::collections::BTreeSet::from(["src/ui".to_string()]);
    assert_eq!(paths(visible(&tree, "", &ui)), ["src", "src/ui", "src/lib.rs", "aio.toml"]);
    let root = std::collections::BTreeSet::from([" root".to_string()]);
    assert!(visible(&tree, "", &root).is_empty(), "the root folded: one line");
    assert_eq!(paths(visible(&tree, "APP", &root)), ["src", "src/ui", "src/ui/app.rs"], "a filter ignores folding");
}

// ── the window ──────────────────────────────────────────────────────────────

/// The verbs a window ran, and a running app called notes (with a private
/// copy, notes#2) and a stopped one, todo; aio-ui is the window itself.
fn fake(said: Arc<Mutex<Vec<String>>>, home: PathBuf) -> Verb {
    Arc::new(move |w: Vec<String>, _cwd: Option<PathBuf>| {
        said.lock().unwrap().push(w.join(" "));
        let ok = |v: Value| Ok(json!({ "ok": true, "value": v }).to_string());
        match w[0].as_str() {
            "apps" => ok(json!([
                { "name": "aio-ui", "home": "/nowhere/aio-ui", "running": true, "pid": 1 },
                { "name": "notes", "home": home.join("apps/notes"), "running": true, "pid": 4_000_000, "port": 8080, "version": "0.1.0", "aio": "0.1.0", "startedBy": "ax start (dev, from a terminal)" },
                { "name": "notes", "home": "/nowhere/i/2/notes", "running": true, "pid": 4_000_001, "instance": "2" },
                { "name": "todo", "home": "/nowhere/todo", "running": false },
            ])),
            "peer" => ok(json!([{ "cell": "board", "v": 1,
                "methods": [{ "method": "add", "sig": "add(text: String) -> ()" }, { "method": "clear", "sig": "clear() -> ()" }],
                "fields": { "persist": true, "server": ["owner"], "transient": [] } }])),
            "metrics" => ok(
                json!({ "uptimeS": 90, "connections": 2, "schedules": 0, "cells": { "board": { "bytes": 120, "links": 2 } } }),
            ),
            "cost" => ok(
                json!({ "board": { "calls": 4, "meanMs": 0.5, "p95Ms": 1.25, "pushesPerS": 2.0, "bytesPerS": 512.0, "fullResends": 0 } }),
            ),
            "events" => {
                ok(json!([{ "seq": 7, "cell": "board", "method": "add", "args": ["hi"], "at": 1_793_530_826_603i64 }]))
            }
            "clients" => ok(
                json!([{ "id": 3, "user": "dev", "from": "door", "wire": "tool", "rtt": 4, "since": 1_793_530_826_603i64 }]),
            ),
            "errors" => Ok("start 1: x\n  2026-11-01T10:00:02.000Z ERROR [aio] boom".into()),
            "schedules" => ok(json!([])),
            "components" => ok(
                json!({ "components": [{ "name": "root", "renders": 2, "ms": 1.0, "children": [{ "name": "Board", "renders": 5, "ms": 0.75, "children": [] }] }] }),
            ),
            "snapshot" => ok(json!({ "board": { "notes": ["a", "b"], "owner": "ana", "n": 2 } })),
            "call" => Ok("null".into()),
            "start" | "stop" | "restart" => Ok(format!("{}: {} done", w[1], w[0])),
            "create" => Ok(format!("created {}", w[1])),
            _ => Err(format!("no fake for {}", w[0])),
        }
    })
}

thread_local! {
    static SETUP: std::cell::RefCell<Option<Setup>> = const { std::cell::RefCell::new(None) };
}

fn mounted() -> Element {
    use_context_provider(|| SETUP.with_borrow(|s| s.clone().unwrap()));
    rsx! { super::app::App {} }
}

/// A window on the fake verbs, its home in `dir`: notes's project at
/// `dir/w/notes` (found where it runs: its folder is the scan's root), its
/// logs and health in its home.
fn window(dir: &Path, dark: bool) -> (TestUi, Arc<Mutex<Vec<String>>>) {
    let home = dir.join("home");
    let proj = home.join("w/notes");
    std::fs::create_dir_all(proj.join("src")).unwrap();
    std::fs::write(proj.join("Cargo.toml"), "[package]\nname = \"notes\"\n").unwrap();
    std::fs::write(proj.join("aio.toml"), "[app]\nid = \"notes\"\nname = \"notes\"\n").unwrap();
    std::fs::write(
        proj.join("src/lib.rs"),
        "use x;\n\n#[aio::cell(persist, id = \"board\")]\npub struct Board {\n    pub notes: Vec<String>,\n}\n",
    )
    .unwrap();
    let data = home.join("apps/notes/data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::create_dir_all(home.join("apps/notes/logs")).unwrap();
    std::fs::write(data.join("health.json"), json!({ "status": "healthy", "aio": "0.1.0", "uptimeS": 90, "memory": { "rssBytes": 48u64 << 20, "machinePct": 1, "level": "ok" }, "degraded": [], "gauges": [] }).to_string()).unwrap();
    std::fs::write(
        home.join("apps/notes/logs/app.log"),
        "2026-11-01T10:00:00.000Z [aio] up\n2026-11-01T10:00:01.000Z WARN  [page] slow\n2026-11-01T10:00:02.000Z ERROR [aio] boom\n",
    )
    .unwrap();
    let said = Arc::new(Mutex::new(vec![]));
    SETUP.set(Some(Setup {
        verb: fake(said.clone(), home.clone()),
        program: PathBuf::from("/bin/sh"),
        roots: proj.parent().unwrap().display().to_string(),
        cwd: None,
        installed: home.join("install"),
        create_in: home.join("aio-apps"),
        home,
        prefs: None,
    }));
    let m = aio::testing::Mount::default().size(1280, 900);
    (if dark { m.dark() } else { m }.app(mounted), said)
}

fn shows(ui: &mut TestUi, t: &str, has: &str) {
    for _ in 0..500 {
        if ui.present(t) && ui.text(t).contains(has) {
            return;
        }
        ui.wait(Duration::from_millis(10));
    }
    panic!("{t} never showed {has:?}: {:?}", ui.present(t).then(|| ui.text(t)));
}

fn said(s: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    s.lock().unwrap().clone()
}

/// amui's `amui shell renders the sidebar + empty state` and `select a
/// discovered project populates detail`: the running apps first, the
/// chosen one's overview, its project found where it is.
#[test]
fn the_manager_lists_the_running_apps_and_shows_the_chosen_ones_overview() {
    let d = tempfile::tempdir().unwrap();
    let (mut ui, verbs) = window(d.path(), false);
    shows(&mut ui, "apps", "notes");
    shows(&mut ui, "pick", "select a project");
    assert!(!ui.text("apps").contains("todo"), "running ones by default");
    ui.click("showAll");
    shows(&mut ui, "apps", "todo");
    ui.click("showRunning");
    ui.set_value("search", "tod");
    shows(&mut ui, "apps", "todo");
    ui.set_value("search", "");
    ui.click("app-notes");
    shows(&mut ui, "name", "notes");
    shows(&mut ui, "status", "started");
    shows(&mut ui, "tile-clients", "2");
    shows(&mut ui, "tile-uptime", "1m 30s");
    shows(&mut ui, "cellHealth", "board");
    assert!(ui.text("cellHealth").contains("120 B · 2 links"), "{}", ui.text("cellHealth"));
    assert!(ui.text("project").contains("0.1.0") && ui.text("process").contains("4000000"));
    shows(&mut ui, "errors", "boom");
    let at = Path::new("w").join("notes");
    assert!(ui.text("where").contains(&*at.to_string_lossy()), "its project, found: {}", ui.text("where"));
    assert!(said(&verbs).contains(&"peer notes --json".to_string()));
}

/// amui's `a refusal reaches the UI`, from the inline confirmation of
/// Stop and Restart, and a method's call naming exactly what it sends.
#[test]
fn stop_restart_and_a_call_ask_first_naming_exactly_what_they_do() {
    let d = tempfile::tempdir().unwrap();
    let (mut ui, verbs) = window(d.path(), false);
    shows(&mut ui, "apps", "notes");
    ui.click("app-notes");
    shows(&mut ui, "name", "notes");
    ui.click("stop");
    shows(&mut ui, "confirm", "Stop notes?");
    ui.click("confirmCancel");
    assert!(!ui.present("confirm"));
    ui.click("restart");
    ui.click("confirmOk");
    shows(&mut ui, "actionMsg", "notes: restart done");
    ui.click("tab-Cells");
    shows(&mut ui, "cell-board", "add");
    ui.click("run-board-clear");
    shows(&mut ui, "confirm", "Run board.clear() on notes? It will be called with NO arguments.");
    ui.click("confirmOk");
    shows(&mut ui, "dispatchMsg", "called board.clear");
    ui.set_value("args-board", "[\"hi\"]");
    ui.click("run-board-add");
    shows(&mut ui, "confirm", "Run board.add(\"hi\") on notes?");
    ui.click("confirmOk");
    ui.set_value("args-board", "hi");
    ui.click("run-board-add");
    shows(&mut ui, "dispatchMsg", "JSON list");
    let s = said(&verbs);
    assert!(!s.iter().any(|w| w.starts_with("stop")), "a cancelled stop never ran: {s:?}");
    assert!(s.contains(&"restart notes".to_string()));
    let calls: Vec<&String> = s.iter().filter(|w| w.starts_with("call")).collect();
    assert_eq!(calls, ["call notes board clear --args []", "call notes board add --args [\"hi\"]"]);
}

/// amui's `start / stop / restart refuses to act on amui itself`.
#[test]
fn ax_ui_lists_itself_and_never_offers_to_start_or_stop_itself() {
    let d = tempfile::tempdir().unwrap();
    let (mut ui, verbs) = window(d.path(), false);
    shows(&mut ui, "apps", "aio-ui");
    ui.click("app-aio-ui");
    shows(&mut ui, "self", "this is ax ui");
    assert!(!ui.present("stop") && !ui.present("restart") && !ui.present("start"));
    assert!(!said(&verbs).iter().any(|w| w.contains("stop aio-ui")));
}

/// A private instance is its own entry, and every verb reaches that copy.
#[test]
fn a_private_instance_is_listed_and_each_verb_reaches_that_copy() {
    let d = tempfile::tempdir().unwrap();
    let (mut ui, verbs) = window(d.path(), false);
    shows(&mut ui, "apps", "instance 2");
    ui.click("app-notes#2");
    shows(&mut ui, "instance", "instance 2");
    shows(&mut ui, "tile-clients", "2");
    let s = said(&verbs);
    assert!(s.contains(&"metrics --instance 2 notes --json".to_string()), "{s:?}");
    assert!(s.contains(&"peer --instance 2 notes --json".to_string()), "{s:?}");
}

/// amui's `StateOverview renders merged cells + persist/UI badges` and
/// `every field shows its JSON size beside it`.
#[test]
fn the_state_tab_merges_the_cells_with_saved_and_sent_badges_and_sizes() {
    let d = tempfile::tempdir().unwrap();
    let (mut ui, verbs) = window(d.path(), false);
    shows(&mut ui, "apps", "notes");
    ui.click("app-notes");
    shows(&mut ui, "name", "notes");
    ui.click("tab-State");
    shows(&mut ui, "cellState-board", "3 fields · 3 saved · 2 in UI");
    assert!(ui.text("field-board-notes").contains("[] 2") && ui.text("field-board-notes").contains("9 B"));
    assert!(ui.html().contains("aria-label=\"no ui\""), "a #[server] field is struck: not sent");
    shows(&mut ui, "stateSize", "KB");
    ui.click("field-board-notes");
    shows(&mut ui, "tree", "\"a\"");
    assert!(said(&verbs).contains(&"snapshot notes --json".to_string()));
}

/// amui's `MetricsPanel renders loop + clients + cell sizes`.
#[test]
fn the_metrics_tab_shows_trends_calls_memory_clients_sizes_and_actions() {
    let d = tempfile::tempdir().unwrap();
    let (mut ui, _) = window(d.path(), false);
    shows(&mut ui, "apps", "notes");
    ui.click("app-notes");
    shows(&mut ui, "name", "notes");
    ui.click("tab-Metrics");
    shows(&mut ui, "calls", "1.25 ms");
    for t in ["chart-CPU", "chart-Memory (RSS)", "chart-State", "chart-Call p95", "chart-Pushes"] {
        assert!(ui.present(t), "{t}");
    }
    assert!(ui.text("chart-State").contains("samples"), "{}", ui.text("chart-State"));
    shows(&mut ui, "memory", "48.0 MB");
    shows(&mut ui, "clients", "door");
    shows(&mut ui, "cellSizes", "120 B");
    shows(&mut ui, "actions", "board.add");
    shows(&mut ui, "components", "Board×5 · 0.75 ms");
}

/// amui's `LogView renders filtered lines` and `readLogs`: by source, by
/// level and text, and a log that isn't there said so.
#[test]
fn the_logs_tab_reads_by_source_and_filters_by_level_and_text() {
    let d = tempfile::tempdir().unwrap();
    let (mut ui, _) = window(d.path(), false);
    shows(&mut ui, "apps", "notes");
    ui.click("app-notes");
    shows(&mut ui, "name", "notes");
    ui.click("tab-Logs");
    shows(&mut ui, "logCount", "3/3 lines");
    assert_eq!(ui.count(".l-warn"), 2, "its level word and its text");
    ui.click("lvl-warn");
    shows(&mut ui, "logCount", "2/3 lines");
    ui.set_value("logFilter", "boom");
    shows(&mut ui, "logCount", "1/3 lines");
    ui.click("follow");
    shows(&mut ui, "follow", "following");
    ui.click("src-error");
    shows(&mut ui, "logError", "no error log found");
}

/// amui's `runTask: cancel via signal terminates promptly`: a task runs
/// in the app's project, its output shown, and Stop task ends it.
#[cfg(unix)]
#[test]
fn a_task_runs_in_the_project_and_stop_task_ends_it() {
    let d = tempfile::tempdir().unwrap();
    let (mut ui, _) = window(d.path(), false);
    // The fake `ax` is `sh`: `sh test notes` runs the project's `test`.
    let proj = d.path().join("home/w/notes");
    std::fs::write(proj.join("test"), "echo tested \"$1\"\n").unwrap();
    std::fs::write(proj.join("dev"), "echo watching; sleep 30\n").unwrap();
    shows(&mut ui, "apps", "notes");
    ui.click("app-notes");
    shows(&mut ui, "name", "notes");
    ui.click("tab-Tasks");
    ui.click("task-test");
    shows(&mut ui, "taskOutput", "tested notes");
    shows(&mut ui, "taskState", "exit 0");
    ui.click("task-dev");
    shows(&mut ui, "taskState", "running dev");
    ui.click("stopTask");
    shows(&mut ui, "taskOutput", "[cancelled]");
}

/// amui's Codebase tab and `findCellSource`: the project's tree, folded
/// and filtered, a file highlighted, and a cell's source from its card.
#[test]
fn the_codebase_tab_folds_filters_and_opens_files_and_a_cells_source() {
    let d = tempfile::tempdir().unwrap();
    let (mut ui, _) = window(d.path(), false);
    shows(&mut ui, "apps", "notes");
    ui.click("app-notes");
    shows(&mut ui, "name", "notes");
    ui.click("tab-Codebase");
    shows(&mut ui, "fileList", "lib.rs");
    assert!(ui.text("fileList").contains("aio.toml") && !ui.text("fileList").contains("Cargo.lock"));
    ui.click("collapseAll");
    assert!(!ui.text("fileList").contains("lib.rs"), "folded to its root");
    ui.click("expandAll");
    ui.set_value("fileFilter", "lib");
    shows(&mut ui, "fileList", "lib.rs");
    assert!(!ui.text("fileList").contains("aio.toml"), "filtered");
    ui.click("file-src/lib.rs");
    shows(&mut ui, "fileText", "pub struct Board");
    assert!(ui.count(".hl-keyword") >= 3);
    ui.click("closeFile");
    ui.click("tab-Cells");
    ui.click("source-board");
    shows(&mut ui, "fileHint", "cell \"board\" defined at line 4");
}

/// amui's `create`: a new app by name, in `~/aio-apps`, its name made safe.
#[test]
fn a_new_app_is_made_by_name_and_an_empty_name_refused() {
    let d = tempfile::tempdir().unwrap();
    let (mut ui, verbs) = window(d.path(), false);
    shows(&mut ui, "apps", "notes");
    ui.set_value("newApp", "@@@");
    ui.click("create");
    shows(&mut ui, "createMsg", "enter a name");
    ui.set_value("newApp", "My Notes");
    ui.click("create");
    shows(&mut ui, "createMsg", "created");
    assert!(ui.text("createMsg").contains("aio-apps"), "{}", ui.text("createMsg"));
    assert!(said(&verbs).contains(&"create my-notes".to_string()));
}

/// The owner's report: no text unreadable, dark or light (every text on
/// the page's own background, never the window's).
#[test]
fn every_text_reads_on_its_background_in_dark_and_light() {
    for dark in [true, false] {
        let d = tempfile::tempdir().unwrap();
        let (mut ui, _) = window(d.path(), dark);
        shows(&mut ui, "apps", "notes");
        ui.click("app-notes");
        shows(&mut ui, "cellHealth", "board");
        for tab in ["Overview", "Cells", "State", "Metrics", "Logs", "Tasks", "Codebase"] {
            ui.click(&format!("tab-{tab}"));
            ui.wait(Duration::from_millis(100));
            let bad = ui.contrast();
            assert!(bad.is_empty(), "{tab} (dark: {dark}): {bad:#?}");
        }
    }
}

/// No apps: where it looked, and how to add a place or make one.
#[test]
fn no_apps_says_where_it_looked() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("home")).unwrap();
    let said = Arc::new(Mutex::new(vec![]));
    let s = said.clone();
    SETUP.set(Some(Setup {
        verb: Arc::new(move |w, _| {
            s.lock().unwrap().push(w.join(" "));
            Ok(json!({ "ok": true, "value": [] }).to_string())
        }),
        program: PathBuf::from("/bin/sh"),
        roots: String::new(),
        cwd: None,
        installed: d.path().join("install"),
        create_in: d.path().join("home/aio-apps"),
        home: d.path().join("home"),
        prefs: None,
    }));
    let mut ui = TestUi::new(mounted);
    shows(&mut ui, "noApps", "no aio-rs projects found.");
    assert!(ui.text("noApps").contains("aio-apps") && ui.text("noApps").contains(super::scan::ROOTS_VAR));
}

/// aio's `am ui --client=browser`: the manager in a real browser's tab
/// (headless Chromium; skipped without one), behind the run's key. In a
/// process of its own: TestUi's event reader is global, and the tab's
/// events would reach it.
#[cfg(unix)]
#[test]
fn the_manager_runs_in_a_browser_tab_behind_the_runs_key() {
    const ALONE: &str = "AIO_TEST_UI_BROWSER";
    if std::env::var_os(ALONE).is_some() {
        return tokio::runtime::Runtime::new().unwrap().block_on(in_a_browser_tab());
    }
    let name = "ui::tests::the_manager_runs_in_a_browser_tab_behind_the_runs_key";
    let exe = std::env::current_exe().unwrap();
    let o = std::process::Command::new(exe).args([name, "--exact", "--nocapture"]).env(ALONE, "1").output().unwrap();
    assert!(o.status.success(), "{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    assert!(String::from_utf8_lossy(&o.stdout).contains("1 passed"), "the test ran");
}

#[cfg(unix)]
async fn in_a_browser_tab() {
    use crate::web::browser::{Chromium, until};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let said = Arc::new(Mutex::new(vec![]));
    let key = "a-key-made-for-this-run-0123456789";
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(super::serve(l, key.into(), fake(said.clone(), std::env::temp_dir())));
    // Without the key nothing is shown.
    let mut s = tokio::net::TcpStream::connect(&addr).await.unwrap();
    s.write_all(format!("GET / HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    let mut got = String::new();
    s.read_to_string(&mut got).await.unwrap();
    assert!(got.starts_with("HTTP/1.1 401"), "{got}");
    let url = format!("http://{addr}/?token={key}");
    let Some(c) = Chromium::open(&url) else {
        return eprintln!("no Chromium: skipped");
    };
    let mut t = c.tabs(&url, 1).await.remove(0);
    let q = |h: &str| format!("document.querySelector('[data-t=\"{h}\"]')");
    let text = |h: &str| format!("{0} && {0}.textContent", q(h));
    until(&mut t, &format!("!!{}", q("app-notes")), json!(true)).await;
    t.eval(&format!("{}.click() || true", q("app-notes"))).await.unwrap();
    until(&mut t, &text("name"), json!("notes")).await;
    t.eval(&format!("{}.click() || true", q("stop"))).await.unwrap();
    until(&mut t, &format!("!!{}", q("confirmOk")), json!(true)).await;
    t.eval(&format!("{}.click() || true", q("confirmOk"))).await.unwrap();
    until(&mut t, &text("actionMsg"), json!("notes: stop done")).await;
    assert!(said.lock().unwrap().contains(&"stop notes".to_string()));
}

/// `ax open`: a page's address from its `serving.json`; a stopped app, or
/// nothing answering, said so.
#[test]
fn a_stopped_app_says_so_and_nothing_answers_its_page() {
    use super::{open_app, opener, url};
    assert_eq!(url(&json!({ "port": 8080, "tls": false })).unwrap(), "http://localhost:8080");
    assert_eq!(url(&json!({ "port": 443, "tls": true, "host": "notes.local" })).unwrap(), "https://notes.local:443");
    assert_eq!(url(&json!({ "port": 1, "host": "0.0.0.0" })).unwrap(), "http://localhost:1");
    assert_eq!(url(&json!({})), None);
    assert_eq!((opener("linux"), opener("macos"), opener("windows")), ("xdg-open", "open", "explorer"));
    let d = tempfile::tempdir().unwrap();
    assert!(open_app("notes", d.path(), true).unwrap_err().contains("serves no page"));
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::fs::write(d.path().join("serving.json"), json!({ "port": port }).to_string()).unwrap();
    assert_eq!(open_app("notes", d.path(), true).unwrap(), format!("http://localhost:{port}"));
    std::fs::write(d.path().join("serving.json"), json!({ "port": 1 }).to_string()).unwrap();
    assert!(open_app("notes", d.path(), true).unwrap_err().starts_with("nothing answers"));
}

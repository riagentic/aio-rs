//! The manager's window (amui's `App.tsx`): on the left the apps, running
//! or all, searchable, a new one made by name; on the right the chosen one,
//! tabbed: Overview · Cells · State · Metrics · Logs · Tasks · Codebase.
//! The chosen app is read again every 2.5 s while it runs, the list every
//! 9 s, its log every 2 s while followed.
use super::logs::{Floor, LogView};
use super::manager::{self, Detail, Manager, Setup, TASKS, Verb};
use super::metrics::{MetricsPanel, clock};
use super::proc::{FileNode, LogSource};
use super::state_view::StateOverview;
use super::style;
use aio::ui::follow;
use aio::ui::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Overview,
    Cells,
    State,
    Metrics,
    Logs,
    Tasks,
    Codebase,
}

const TABS: [(Tab, &str); 7] = [
    (Tab::Overview, "Overview"),
    (Tab::Cells, "Cells"),
    (Tab::State, "State"),
    (Tab::Metrics, "Metrics"),
    (Tab::Logs, "Logs"),
    (Tab::Tasks, "Tasks"),
    (Tab::Codebase, "Codebase"),
];

/// A consequential act, asked first (amui's inline confirmation: never a
/// modal that freezes the live tiles behind it).
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    Stop,
    Restart,
    Call { cell: String, method: String, args: String },
}

/// The confirmation waiting: for which entry, what it says, what it does.
#[derive(Clone, Debug, PartialEq)]
pub struct Pending {
    pub id: String,
    pub msg: String,
    pub act: Act,
}

/// The view's own state (amui's module signals): kept across renders,
/// reset when another app is chosen.
#[derive(Clone, Copy)]
pub struct ViewState {
    pub tab: Signal<Tab>,
    pub search: Signal<String>,
    pub new_name: Signal<String>,
    /// The folders the person folded (the tree is open by default).
    pub collapsed: Signal<BTreeSet<String>>,
    pub file_filter: Signal<String>,
    /// The Codebase tab shows the repo, not the app's folder.
    pub repo: Signal<bool>,
    /// Each cell's JSON arguments for its methods.
    pub args: Signal<BTreeMap<String, String>>,
    pub pending: Signal<Option<Pending>>,
    pub tree_open: Signal<BTreeSet<String>>,
    pub state_open: Signal<BTreeSet<String>>,
    pub log_floor: Signal<Floor>,
    pub log_query: Signal<String>,
}

impl ViewState {
    fn new() -> Self {
        ViewState {
            tab: Signal::new(Tab::Overview),
            search: Signal::new(String::new()),
            new_name: Signal::new(String::new()),
            collapsed: Signal::new(BTreeSet::new()),
            file_filter: Signal::new(String::new()),
            repo: Signal::new(false),
            args: Signal::new(BTreeMap::new()),
            pending: Signal::new(None),
            tree_open: Signal::new(BTreeSet::new()),
            state_open: Signal::new(BTreeSet::new()),
            log_floor: Signal::new(Floor::All),
            log_query: Signal::new(String::new()),
        }
    }

    /// Another app chosen: its own view from the start.
    fn reset(self) {
        self.collapsed.set(BTreeSet::new());
        self.file_filter.set(String::new());
        self.repo.set(false);
        self.tree_open.set(BTreeSet::new());
        self.state_open.set(BTreeSet::new());
        self.log_floor.set(Floor::All);
        self.log_query.set(String::new());
    }
}

/// Enter or Space on something that acts like a button (amui's `press`):
/// every clickable row is reachable without a mouse.
pub fn pressed(e: &KeyboardEvent) -> bool {
    let k = e.key();
    let hit = k == Key::Enter || k == Key::Character(" ".into());
    if hit {
        e.prevent_default();
    }
    hit
}

/// The chosen app read again this often while it runs.
const TICK: Duration = Duration::from_millis(2500);
/// The list read again this often.
const RESCAN: Duration = Duration::from_secs(9);
/// A followed log read again this often.
const FOLLOW: Duration = Duration::from_secs(2);
/// How often a running task's output is shown as it grows.
const TASK_POLL: Duration = Duration::from_millis(300);

/// The window (amui's `App`): its manager on the context's [`Setup`] (else
/// this machine's, its verbs the context's [`Verb`], else `ax` itself).
#[component]
pub fn App() -> View {
    let setup = try_context::<Setup>();
    let verb = try_context::<Verb>();
    let prefs = try_context::<super::Prefs>();
    let m = once(move || {
        let s = setup.unwrap_or_else(|| Setup::here(verb.unwrap_or_else(manager::local), prefs.and_then(|p| p.0)));
        Manager::new(s)
    });
    provide(|| m);
    let view = provide(ViewState::new);
    once(move || m.discover());
    aio::kit::use_interval(TICK, true, move || {
        if m.detail.read_untracked().as_ref().is_some_and(|d| d.running) {
            m.tick();
        }
    });
    aio::kit::use_interval(RESCAN, true, move || {
        if !*m.scanning.read_untracked() && m.task.read_untracked().running.is_none() {
            m.discover();
        }
    });
    aio::kit::use_interval(TASK_POLL, true, move || m.task_progress());
    aio::kit::use_interval(FOLLOW, true, move || {
        let live = m.detail.read_untracked().as_ref().filter(|d| d.running).map(|d| d.id.clone());
        if *view.tab.read_untracked() == Tab::Logs
            && *m.log_follow.read_untracked()
            && !*m.log_loading.read_untracked()
            && let Some(id) = live
        {
            m.load_logs(&id, None);
        }
    });
    // Another app chosen: its view from the start.
    let last = signal(|| None::<String>);
    effect(move || {
        let id = m.detail.read().as_ref().map(|d| d.id.clone());
        if id != *last.read_untracked() {
            last.set(id);
            view.reset();
        }
    });
    rsx! {
        style { {style::CSS} }
        div { class: "am",
            Sidebar {}
            if m.detail.with(Option::is_some) {
                DetailPane {}
            } else {
                Pick {}
            }
        }
    }
}

/// The chosen app without what changes on every reading (its CPU, memory,
/// uptime, clients, when it was read): what the views are made from, so a
/// tick redraws only the figures (pure).
fn stable(d: &Detail) -> Detail {
    Detail { cpu_pct: None, cpu_mark: None, mem_mb: None, uptime_s: None, connections: None, at: 0, ..d.clone() }
}

/// How many apps there are, and how many run.
fn counts(m: Manager) -> (Memo<usize>, Memo<usize>) {
    (
        memo(move || m.projects.with(Vec::len)),
        memo(move || m.projects.with(|all| all.iter().filter(|p| p.running.is_some()).count())),
    )
}

/// Nothing chosen yet.
#[component]
fn Pick() -> View {
    let m = context::<Manager>();
    let (all, up) = counts(m);
    rsx! {
        div { class: "pick", "data-t": "pick",
            div { style: "font-size:15px", "select a project" }
            div { style: "font-size:13px", "{all} found · {up} running" }
        }
    }
}

/// The list of apps (amui's `Sidebar`).
#[component]
fn Sidebar() -> View {
    let m = context::<Manager>();
    let view = context::<ViewState>();
    let (all, running) = counts(m);
    let create = move || {
        let n = view.new_name.get_untracked();
        if !n.trim().is_empty() {
            m.create(&n);
            view.new_name.set(String::new());
        }
    };
    rsx! {
        nav { class: "side", "aria-label": "Apps",
            div { class: "brand",
                b { "ax ui" }
                span { "aio-rs Manager UI" }
            }
            div { class: "side-tools",
                input { class: "field", "aria-label": "search projects", "data-t": "search", placeholder: "search projects…",
                    value: view.search, oninput: move |e| view.search.set(e.value()),
                }
                div { class: "row6",
                    button { r#type: "button", class: "btn", style: "flex:1", "data-t": "rescan", disabled: m.scanning,
                        onclick: move |_| m.discover(),
                        if m.scanning.get() { "scanning…" } else { "⟳ Rescan" }
                    }
                    form { class: "row6", style: "flex:1;min-width:0",
                        onsubmit: move |e| {
                            e.prevent_default();
                            create();
                        },
                        input { class: "field", style: "flex:1;font-size:12px;padding:7px 8px", "aria-label": "new app",
                            "data-t": "newApp", name: "nn", placeholder: "new app", disabled: m.create_busy,
                            value: view.new_name, oninput: move |e| view.new_name.set(e.value()),
                        }
                        button { r#type: "submit", class: "btn go", "data-t": "create", disabled: m.create_busy, title: "Make a new aio-rs app",
                            if m.create_busy.get() { "…" } else { "＋" }
                        }
                    }
                }
                if m.create_msg.with(Option::is_some) {
                    div { class: "text2", style: "font-size:11px;word-break:break-all", "data-t": "createMsg",
                        {m.create_msg.get().unwrap_or_default()}
                    }
                }
                div { class: "row6", style: "font-size:11px", role: "radiogroup", "aria-label": "which apps to list",
                    for (all_of , t) in [(false, "showRunning"), (true, "showAll")] {
                        span { key: "{t}", class: if m.show_all.get() == all_of { "chip pill on" } else { "chip pill" },
                            style: "flex:1;text-align:center", role: "radio", tabindex: "0", "data-t": "{t}",
                            "aria-checked": "{m.show_all.get() == all_of}",
                            onclick: move |_| m.set_show_all(all_of),
                            onkeydown: move |e| if pressed(&e) { m.set_show_all(all_of) },
                            {if all_of { format!("all ({})", all.get()) } else { format!("running ({})", running.get()) }}
                        }
                    }
                }
            }
            div { class: "side-list", "data-t": "apps", SideList {} }
            div { class: "side-foot", "{running} running · {all} total" }
        }
    }
}

/// One app in the list: its id, chosen or not, where it runs, its mode and
/// instance, its folder, running or not, its name.
type Entry = (String, bool, String, String, String, bool, String);

/// The apps as the list shows them (its shape follows the list, the search,
/// the choice and the scan).
#[component]
fn SideList() -> View {
    let m = context::<Manager>();
    let view = context::<ViewState>();
    let (all, q, show_all, selected, scanning, scanned, roots) = follow(move || {
        (
            m.projects.get(),
            view.search.get(),
            m.show_all.get(),
            m.selected.get(),
            m.scanning.get(),
            m.scanned.get(),
            m.scan_roots.get(),
        )
    });
    let shown: Vec<Entry> = manager::listed(&all, &q, show_all, selected.as_deref())
        .into_iter()
        .map(|p| {
            let sel = selected.as_deref() == Some(p.id.as_str());
            let sub = match &p.running {
                Some(r) => r.port.map_or("running".into(), |port| format!(":{port}")),
                None => "stopped".into(),
            };
            let mode = p.meta.mode.clone().map_or(String::new(), |m| format!(" · {m}"));
            let inst = p.instance.as_ref().map_or(String::new(), |i| format!(" · instance {i}"));
            let title = p.path.as_ref().or(p.home.as_ref()).map(|d| d.display().to_string()).unwrap_or_default();
            (p.id.clone(), sel, sub, format!("{mode}{inst}"), title, p.running.is_some(), p.meta.name.clone())
        })
        .collect();
    let n = all.len();
    rsx! {
        if shown.is_empty() && q.is_empty() && !show_all && n > 0 {
            div { class: "note",
                "no running apps · "
                span { class: "link", role: "button", tabindex: "0", "data-t": "showAllLink",
                    onclick: move |_| m.set_show_all(true),
                    onkeydown: move |e| if pressed(&e) { m.set_show_all(true) },
                    "show all {n}"
                }
            }
        } else if shown.is_empty() {
            div { class: "note", "data-t": "noApps",
                if scanning || !scanned {
                    "scanning…"
                } else {
                    div { class: "text2 mb8",
                        if q.is_empty() { "no aio-rs projects found." } else { "no projects match your search." }
                    }
                    if q.is_empty() {
                        div { style: "margin-bottom:4px", "searched:" }
                        div { class: "mono", style: "font-size:10px",
                            for r in roots.clone() {
                                div { key: "{r}", class: "ell", title: "{r}", "{r}" }
                            }
                        }
                        div { style: "margin-top:8px;line-height:1.5",
                            "add more with "
                            span { class: "mono text2", {super::scan::roots_example()} }
                            " · or make one: ax create <name>"
                        }
                    }
                }
            }
        } else {
            for (id , sel , sub , more , title , up , name) in shown.clone() {
                div { key: "{id}", class: if sel { "proj sel" } else { "proj" }, role: "button", tabindex: "0",
                    "data-t": "app-{id}", "aria-current": sel.then_some("true"), title: "{title}",
                    onclick: move |_| m.select(&id),
                    onkeydown: move |e| if pressed(&e) { m.select(&id) },
                    span { class: if up { "dot up" } else { "dot" } }
                    div { style: "min-width:0;flex:1",
                        b { class: "ell", "{name}" }
                        small { "{sub}{more}" }
                    }
                }
            }
        }
    }
}

/// The running state, as a chip (amui's `StatusChip`).
#[component]
fn StatusChip(running: bool, status: Option<String>) -> View {
    let (cls, bg, line) = if running {
        ("green", "var(--green-bg)", "var(--green-line)")
    } else {
        ("dim", "var(--dim-bg)", "var(--border)")
    };
    let word = if running { status.unwrap_or("running".into()) } else { "stopped".into() };
    rsx! {
        span { class: "chip {cls}", style: "background:{bg};border-color:{line}", "data-t": "status", "● {word}" }
    }
}

/// The asked act, inline under the header (amui's `ConfirmBar`).
#[component]
fn ConfirmBar(id: String) -> View {
    let m = context::<Manager>();
    let view = context::<ViewState>();
    let Some(p) = follow(move || view.pending.get().filter(|p| p.id == id)) else { return rsx! {} };
    let msg = p.msg.clone();
    let run = move |_| {
        view.pending.set(None);
        match &p.act {
            Act::Stop => m.lifecycle(&p.id, "stop"),
            Act::Restart => m.lifecycle(&p.id, "restart"),
            Act::Call { cell, method, args } => m.dispatch(&p.id, cell, method, args),
        }
    };
    rsx! {
        div { class: "confirm", role: "alertdialog", "data-t": "confirm",
            span { "{msg}" }
            button { r#type: "button", class: "btn ghost no", "data-t": "confirmOk", onclick: run, "Confirm" }
            button { r#type: "button", class: "btn ghost", "data-t": "confirmCancel", onclick: move |_| view.pending.set(None), "Cancel" }
        }
    }
}

/// The chosen app's header (amui's `Header`): name, state, build, folder,
/// when last read, its actions, and the tabs.
#[component]
fn Header() -> View {
    let m = context::<Manager>();
    let view = context::<ViewState>();
    let Some(d) = follow(move || m.detail.with(|d| d.as_ref().map(stable))) else { return rsx! {} };
    let place = d.path.as_ref().or(d.home.as_ref()).map(|p| p.display().to_string()).unwrap_or_default();
    let running = d.running;
    let when = memo(move || {
        if m.detail_loading.get() {
            " · loading…".to_string()
        } else if running {
            format!(" · live · updated {}", clock(m.detail.with(|d| d.as_ref().map_or(0, |d| d.at))))
        } else {
            String::new()
        }
    });
    let ask = move |act: Act, msg: String, id: String| view.pending.set(Some(Pending { id, msg, act }));
    let (id1, id2, id3, id4) = (d.id.clone(), d.id.clone(), d.id.clone(), d.id.clone());
    let (n1, n2) = (d.name.clone(), d.name.clone());
    let build = d.build.unwrap_or_default();
    let instance = d.instance.clone().unwrap_or_default();
    rsx! {
        div { class: "head",
            div { class: "head-top",
                div { style: "min-width:0",
                    div { class: "title",
                        b { "data-t": "name", "{d.meta.name}" }
                        StatusChip { running: d.running, status: d.status.clone() }
                        if d.build.is_some() {
                            span { class: if build == "release" { "chip yellow" } else { "chip blue" }, "data-t": "build", "{build}" }
                        }
                        if d.instance.is_some() {
                            span { class: "chip dim", "data-t": "instance", "instance {instance}" }
                        }
                    }
                    div { class: "path", "data-t": "where", "{place}{when}" }
                }
                div { class: "acts",
                    button { r#type: "button", class: "btn", "data-t": "refresh", onclick: move |_| m.select(&id1), "⟳ Refresh" }
                    if d.own {
                        span { class: "btn ghost dim", style: "cursor:default", "data-t": "self",
                            title: "ax ui manages other apps — control it from the shell that launched it",
                            "★ this is ax ui"
                        }
                    } else if d.running {
                        button { r#type: "button", class: "btn ghost act", "data-t": "restart",
                            onclick: move |_| ask(Act::Restart, format!("Restart {n1}?"), id2.clone()),
                            "↻ Restart"
                        }
                        button { r#type: "button", class: "btn ghost no", "data-t": "stop",
                            onclick: move |_| ask(Act::Stop, format!("Stop {n2}?"), id3.clone()),
                            "■ Stop"
                        }
                    } else {
                        button { r#type: "button", class: "btn go", "data-t": "start", onclick: move |_| m.lifecycle(&id4, "start"), "▶ Start" }
                    }
                }
            }
            ConfirmBar { id: d.id.clone() }
            if m.action_msg.with(Option::is_some) {
                div { class: "msg", "data-t": "actionMsg", {m.action_msg.get().unwrap_or_default()} }
            }
            if running && m.control_error.with(Option::is_some) {
                div { class: "banner", role: "alert", "data-t": "controlError",
                    {format!("⚠ ax ui cannot read this app's doors — the panels below are the LAST reading, not the current one\n{}", m.control_error.get().unwrap_or_default())}
                }
            }
            div { class: "tabs", role: "tablist",
                for (t , label) in TABS {
                    button { key: "{label}", r#type: "button", role: "tab", "aria-selected": "{view.tab.get() == t}",
                        class: if view.tab.get() == t { "tab on" } else { "tab" }, "data-t": "tab-{label}",
                        onclick: move |_| view.tab.set(t),
                        "{label}"
                    }
                }
            }
        }
    }
}

/// A figure at a glance (amui's `Tile`).
#[component]
fn Tile(label: String, value: String, #[props(default)] color: String) -> View {
    rsx! {
        div { class: "card tile", "data-t": "tile-{label}",
            div { class: "label", "{label}" }
            div { class: "v", style: if color.is_empty() { String::new() } else { format!("color:{color}") }, title: "{value}", "{value}" }
        }
    }
}

/// A key and its value (amui's `Row`).
#[component]
fn Row(k: String, v: String) -> View {
    rsx! { div { class: "kv", span { "{k}" } span { "{v}" } } }
}

#[component]
fn Empty(msg: String) -> View {
    rsx! { div { class: "empty", "data-t": "empty", "{msg}" } }
}

/// How the app runs and builds (amui's `TargetsCard`): each chip runs it.
#[component]
fn TargetsCard(d: Detail) -> View {
    let dev = [("dev (default)", "dev"), ("web", "dev --web")];
    let mut compile: Vec<(String, String)> = if d.meta.targets.is_empty() {
        vec![("desktop (default)".into(), "build".into())]
    } else {
        d.meta.targets.iter().map(|t| (t.clone(), format!("build:{t}"))).collect()
    };
    compile.push(("web".into(), "build --web".into()));
    let chips: Vec<(String, String, &'static str)> = dev
        .iter()
        .map(|(l, t)| (l.to_string(), t.to_string(), "var(--blue)"))
        .chain(compile.into_iter().map(|(l, t)| (l, t, "var(--green)")))
        .collect();
    let (dev_chips, build_chips) = chips.split_at(dev.len());
    let (dev_chips, build_chips) = (dev_chips.to_vec(), build_chips.to_vec());
    let none = d.path.is_none();
    let id = d.id.clone();
    rsx! {
        div { class: "card", style: "padding:14px 16px", "data-t": "targets",
            div { style: "display:flex;gap:24px;flex-wrap:wrap;align-items:flex-start",
                div {
                    div { class: "label mb8", "dev modes" }
                    div { class: "row6", style: "flex-wrap:wrap",
                        for (l , t , c) in dev_chips.clone() {
                            TargetChip { key: "{t}", id: id.clone(), label: l.clone(), task: t.clone(), color: c, none }
                        }
                    }
                }
                div {
                    div { class: "label mb8", "build targets" }
                    div { class: "row6", style: "flex-wrap:wrap",
                        for (l , t , c) in build_chips.clone() {
                            TargetChip { key: "{t}", id: id.clone(), label: l.clone(), task: t.clone(), color: c, none }
                        }
                    }
                }
                if none {
                    div { class: "dim", style: "font-size:12px;align-self:center", "no project folder found for this app" }
                }
            }
        }
    }
}

/// One way to run or build the app, as a chip: it runs it.
#[component]
fn TargetChip(id: String, label: String, task: String, color: &'static str, none: bool) -> View {
    let m = context::<Manager>();
    let view = context::<ViewState>();
    let busy = memo(move || m.task.with(|t| t.running.is_some()));
    rsx! {
        button { r#type: "button", class: "chip pill", style: "color:{color}", disabled: busy.get() || none,
            "data-t": "target-{task}", title: "ax {task}",
            onclick: move |_| {
                view.tab.set(Tab::Tasks);
                m.run_task(&id, &task);
            },
            "{label}"
        }
    }
}

/// The Overview tab (amui's `Overview`): its figures follow every reading.
#[component]
fn Overview() -> View {
    let m = context::<Manager>();
    let (d, f, diag) = follow(move || (m.detail.get(), m.files.get(), m.diag.get()));
    let Some(d) = d else { return rsx! {} };
    let rt = f.runtime.clone();
    let health = diag.health.clone().unwrap_or_default();
    let degraded: BTreeMap<String, String> = health["degraded"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let why = format!("{} failures · {}", e["failures"], e["lastError"].as_str().unwrap_or(""));
            Some((e["name"].as_str()?.to_string(), why))
        })
        .collect();
    let sizes = diag.metrics.as_ref().and_then(|m| m["cells"].as_object().cloned()).unwrap_or_default();
    let mut names: Vec<String> = sizes.keys().cloned().collect();
    for c in d.cells.iter().flatten() {
        if !names.contains(&c.name) {
            names.push(c.name.clone());
        }
    }
    let history = diag.history.clone().unwrap_or_default();
    // Each cell's health row: its name, degraded or not, what it says.
    let health_rows: Vec<(String, bool, String)> = names
        .iter()
        .map(|name| {
            let bad = degraded.get(name).cloned();
            let c = &sizes.get(name).cloned().unwrap_or_default();
            let links =
                c["links"].as_u64().map_or(String::new(), |l| format!(" · {l} link{}", if l == 1 { "" } else { "s" }));
            let size = c["bytes"].as_u64().map_or(String::new(), |b| format!(" · {}", aio::fmt::bytes(b)));
            let (errs, last) = cell_calls(&history, name);
            let errs = if errs > 0 { format!(" · {errs} err") } else { String::new() };
            let last = last.map_or(String::new(), |l| format!(" · {l}"));
            let said = match &bad {
                Some(why) => format!("degraded · {why}"),
                None => format!("ok{size}{links}{errs}{last}"),
            };
            (name.clone(), bad.is_some(), said)
        })
        .collect();
    let work = rt.as_ref().map(|r| r.root.display().to_string()).or(d.path.as_ref().map(|p| p.display().to_string()));
    let dash = || "—".to_string();
    let state = diag.state_bytes();
    let errors = d.errors.clone().unwrap_or_default();
    let schedules = d.schedules.clone().unwrap_or_default();
    let (tasks, cells) = (
        d.meta.tasks.len() + TASKS.len(),
        d.cells.as_ref().map(|c| c.iter().map(|c| c.name.clone()).collect::<Vec<_>>().join(", ")),
    );
    let live = health["config"].clone();
    let exe = rt.as_ref().and_then(|r| r.exe.clone()).map(|e| e.display().to_string());
    let has_exe = exe.is_some();
    let exe = exe.unwrap_or_default();
    let no_names = names.is_empty();
    rsx! {
        div { class: "col",
            div { class: "wrap10",
                Tile { label: "status", value: if d.running { "running" } else { "stopped" }, color: if d.running { "var(--green)" } else { "var(--red)" } }
                Tile { label: "build", value: d.build.unwrap_or("—"), color: if d.build == Some("release") { "var(--yellow)" } else { "var(--blue)" } }
                Tile { label: "port", value: d.port.map_or(dash(), |p| p.to_string()) }
                Tile { label: "pid", value: d.pid.map_or(dash(), |p| p.to_string()) }
                Tile { label: "uptime", value: style::uptime(d.uptime_s) }
                Tile { label: "clients", value: d.connections.map_or(dash(), |c| c.to_string()) }
                Tile { label: "cpu", value: d.cpu_pct.map_or(dash(), |c| format!("{c:.1}%")) }
                Tile { label: "memory", value: d.mem_mb.map_or(dash(), |c| format!("{c:.0} MB")) }
                Tile { label: "state", value: style::bytes(state) }
            }
            div { class: "wrap",
                div { class: "card box", "data-t": "process",
                    div { class: "label mb8", "process" }
                    Row { k: "pid", v: d.pid.map_or(dash(), |p| p.to_string()) }
                    Row { k: "port", v: d.port.map_or(dash(), |p| p.to_string()) }
                    Row { k: "app id", v: d.name.clone() }
                    Row { k: "runtime", v: rt.as_ref().map_or(if d.running { "…".into() } else { dash() }, |r| format!("{} — {}", r.kind, r.label)) }
                    Row { k: "work dir", v: work.clone().unwrap_or_else(dash) }
                    if has_exe {
                        Row { k: "exe", v: exe.clone() }
                    }
                }
                div { class: "card box", "data-t": "cellHealth",
                    div { class: "label mb8", "cell health" }
                    div {
                        if no_names {
                            div { class: "dim", style: "font-size:13px",
                                if d.running { "no cell has written yet" } else { "app not running" }
                            }
                        }
                        for (name , bad , said) in health_rows.clone() {
                            div { key: "{name}", class: "hrow",
                                span { class: if bad { "red" } else { "green" }, style: "font-size:10px", "●" }
                                span { class: "blue", style: "min-width:100px", "{name}" }
                                span { class: "dim", style: "font-size:11px", "{said}" }
                            }
                        }
                    }
                }
            }
            div { class: "wrap",
                div { class: "card box", "data-t": "project",
                    div { class: "label mb8", "project" }
                    Row { k: "name", v: d.meta.name.clone() }
                    Row { k: "version", v: d.version.clone().unwrap_or_else(dash) }
                    Row { k: "aio-rs", v: d.aio.clone().unwrap_or_else(dash) }
                    Row { k: "mode", v: d.meta.mode.clone().unwrap_or("standalone (default)".into()) }
                    Row { k: "git", v: if d.git { "yes" } else { "no" } }
                    Row { k: "tasks", v: if d.path.is_some() { tasks.to_string() } else { "0 (no project folder)".into() } }
                }
                div { class: "card box", "data-t": "config",
                    div { class: "label mb8", "runtime config" }
                    if d.running {
                        Row { k: "title", v: live["title"].as_str().map_or(d.meta.name.clone(), String::from) }
                        Row { k: "port", v: d.port.map(|p| p.to_string()).or(d.meta.listen.clone()).unwrap_or_else(dash) }
                        Row { k: "auth", v: live["auth"].as_str().unwrap_or(if d.meta.accounts { "accounts" } else { "none" }).to_string() }
                        Row { k: "mode", v: d.build.unwrap_or("—") }
                        Row { k: "cells", v: cells.clone().unwrap_or_else(dash) }
                    } else {
                        div { class: "dim", style: "font-size:13px", "app not running" }
                    }
                }
            }
            TargetsCard { d: d.clone() }
            if !errors.is_empty() || !schedules.is_empty() {
                div { class: "wrap",
                    div { class: "card box", style: "padding:12px 16px", "data-t": "errors",
                        div { class: if errors.is_empty() { "label mb6" } else { "label mb6 red" }, "errors ({errors.len()})" }
                        for (i , e) in errors.clone().into_iter().enumerate() {
                            div { key: "{i}", class: "mb6", style: "overflow-wrap:anywhere",
                                div { class: "small red", "{e.head.raw}" }
                                for (j , l) in e.more.iter().take(8).cloned().enumerate() {
                                    div { key: "{j}", class: "small dim", style: "padding-left:12px", "{l}" }
                                }
                            }
                        }
                        if errors.is_empty() {
                            div { class: "dim", style: "font-size:12px", "none" }
                        }
                    }
                    div { class: "card box", style: "padding:12px 16px", "data-t": "schedules",
                        div { class: "label mb6", "schedules ({schedules.len()})" }
                        for (i , s) in schedules.clone().into_iter().enumerate() {
                            div { key: "{i}", class: "small text2", "{s}" }
                        }
                        if schedules.is_empty() {
                            div { class: "dim", style: "font-size:12px", "none" }
                        }
                    }
                }
            }
        }
    }
}

/// A cell's refused calls and its newest action among `history` (`ax
/// events --refused`'s rows), as the cell health row says them (pure).
pub fn cell_calls(history: &[aio::serde_json::Value], cell: &str) -> (usize, Option<String>) {
    let mine: Vec<_> = history.iter().filter(|e| e["cell"] == cell).collect();
    let errs = mine.iter().filter(|e| e["refused"].is_string()).count();
    let last =
        mine.iter().rev().find(|e| !e["refused"].is_string()).and_then(|e| e["method"].as_str().map(String::from));
    (errs, last)
}

/// The Cells tab (amui's `CellsTab`): each cell and its methods; a method
/// runs on the running app with its JSON arguments, once confirmed naming
/// the exact call; `</> source` opens where the cell is defined.
#[component]
fn CellsTab() -> View {
    let m = context::<Manager>();
    let view = context::<ViewState>();
    let (d, source) = follow(move || (m.detail.with(|d| d.as_ref().map(stable)), m.files.with(|f| f.repo.is_some())));
    let Some(d) = d else { return rsx! {} };
    if !d.running {
        return rsx! { Empty { msg: "start the app to inspect its cells" } };
    }
    let source = source || d.path.is_some();
    let cells = d.cells.clone().unwrap_or_default();
    let no_cells = cells.is_empty();
    // A cell's arguments, as typed.
    let args_of = move |name: &str| view.args.with(|a| a.get(name).cloned().unwrap_or_default());
    rsx! {
        div {
            if m.dispatch_msg.with(Option::is_some) {
                div { class: "text2 mono mb10", style: "font-size:12px", "data-t": "dispatchMsg",
                    {m.dispatch_msg.get().unwrap_or_default()}
                }
            }
            div {
                if no_cells {
                    Empty { msg: "no cells (a cell answers once it has written)" }
                }
                for c in cells.clone() {
                    div { key: "{c.name}", class: "card cellcard", "data-t": "cell-{c.name}",
                        div { style: "display:flex;align-items:center;gap:12px;flex-wrap:wrap",
                            span { class: "cellname",
                                "{c.name}"
                                if source {
                                    button { r#type: "button", class: "btn ghost mini", title: "Open {c.name}'s source code",
                                        "data-t": "source-{c.name}",
                                        onclick: move |_| {
                                            m.open_cell_source(&c.name);
                                            view.tab.set(Tab::Codebase);
                                        },
                                        "</> source"
                                    }
                                }
                            }
                            div { class: "row6", style: "flex-wrap:wrap",
                                if c.methods.is_empty() {
                                    span { class: "dim", style: "font-size:12px", "state-only cell" }
                                }
                                for (method , sig) in c.methods.clone() {
                                    button { key: "{method}", r#type: "button", class: "btn run", title: "{sig}",
                                        "data-t": "run-{c.name}-{method}",
                                        onclick: move |_| {
                                            let (id, app, cell) = (d.id.clone(), d.name.clone(), c.name.clone());
                                            let a = args_of(&cell);
                                            match manager::call_words(&app, &cell, &method, &a) {
                                                Ok((_, msg)) => view.pending.set(Some(Pending {
                                                    id,
                                                    msg,
                                                    act: Act::Call { cell, method: method.clone(), args: a },
                                                })),
                                                Err(e) => {
                                                    let d = m.dispatch_msg;
                                                    d.set(Some(e));
                                                }
                                            }
                                        },
                                        "▶ {method}"
                                    }
                                }
                            }
                        }
                        if !c.methods.is_empty() {
                            div { style: "display:flex;align-items:center;gap:8px;margin-top:8px",
                                span { class: "small dim", "args" }
                                input { class: "args", "aria-label": "JSON arguments", "data-t": "args-{c.name}",
                                    placeholder: "[] — JSON arguments, e.g. [\"ada@example.com\"]",
                                    value: args_of(&c.name),
                                    oninput: move |e| view.args.update(|m| _ = m.insert(c.name.clone(), e.value())),
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The State tab (amui's `StateTab`): loaded on first view, never polled.
#[component]
fn StateTab(id: String) -> View {
    let m = context::<Manager>();
    let (running, cells) = follow(move || {
        m.detail.with(|d| d.as_ref().map(|d| (d.running, d.cells.clone().unwrap_or_default())).unwrap_or_default())
    });
    let st = follow(move || m.state.get());
    let shared = memo(move || m.state.with(|s| s.value.clone()).map(|v| super::json_tree::Shared(Rc::new(v))));
    let i = id.clone();
    once(move || {
        if running && m.state.with_untracked(|s| s.of.as_deref() != Some(i.as_str())) {
            m.load_state(&i);
        }
    });
    if !running {
        return rsx! { Empty { msg: "start the app to view its live state" } };
    }
    let loaded = st.of.as_deref() == Some(id.as_str());
    let error = st.error.clone();
    let has_error = error.is_some();
    let error = error.unwrap_or_default();
    rsx! {
        div { class: "col10",
            div { style: "display:flex;align-items:center;gap:10px",
                button { r#type: "button", class: "btn", "data-t": "stateReload", disabled: st.loading,
                    onclick: move |_| m.load_state(&id),
                    if st.loading { "loading…" } else { "⟳ Reload" }
                }
                if loaded && st.size > 0 {
                    span { class: "small dim", style: "font-size:12px", "data-t": "stateSize", {format!("{:.1} KB", st.size as f64 / 1024.0)} }
                }
            }
            if has_error {
                div { class: "red", style: "font-size:13px", "data-t": "stateError", "state error: {error}" }
            } else if st.truncated {
                div { class: "yellow", style: "font-size:13px;line-height:1.5", "data-t": "stateError",
                    {format!("state is too large to render safely ({:.1} MB). Read it a part at a time: ax state <app> <cell> <path>.", st.size as f64 / 1e6)}
                }
            } else if st.loading && !loaded {
                div { class: "dim", style: "font-size:13px;padding:12px", "loading state…" }
            } else if shared.with(Option::is_some) {
                {shared.get().map(|state| rsx! { StateOverview { state, cells: cells.clone() } })}
            }
        }
    }
}

/// The Logs tab (amui's `LogsTab`): its first read on first view.
#[component]
fn LogsTab(id: String) -> View {
    let m = context::<Manager>();
    let i = id.clone();
    once(move || {
        if m.logs.with_untracked(Option::is_none) && !m.log_loading.get_untracked() {
            m.load_logs(&i, None);
        }
    });
    let (a, b) = (id.clone(), id.clone());
    let follow = m.log_follow;
    rsx! {
        LogView {
            logs: m.logs.get(),
            loading: m.log_loading.get(),
            error: m.log_error.get(),
            source: m.log_source.get(),
            path: m.log_path.with(|p| p.as_ref().map(|p| p.display().to_string())),
            truncated: m.log_truncated.get(),
            follow: m.log_follow.get(),
            on_reload: move |_| m.load_logs(&a, None),
            on_source: move |s: LogSource| m.load_logs(&b, Some(s)),
            on_follow: move |_| {
                let on = !follow.get_untracked();
                follow.set(on);
            },
        }
    }
}

/// The Tasks tab (amui's `TasksTab`): `ax` verbs on the app's project, its
/// output captured; cancellable, 5 min at most.
#[component]
fn TasksTab(id: String, has_project: bool, own: Vec<(String, String)>) -> View {
    let m = context::<Manager>();
    let busy = memo(move || m.task.with(|t| t.running.is_some()));
    let running = memo(move || m.task.with(|t| t.running.clone()));
    let no_own = own.is_empty();
    let own: Vec<(String, String, String)> = own.into_iter().map(|(n, r)| (format!("cargo {n}"), n, r)).collect();
    let i = id.clone();
    rsx! {
        div { class: "col",
            if has_project {
                div { class: "card", style: "padding:14px 16px", "data-t": "ownTasks",
                    div { class: "label mb10", "cargo aliases" }
                    if no_own {
                        div { class: "dim", style: "font-size:13px", "no [alias] in .cargo/config.toml" }
                    }
                    div { style: "display:flex;gap:8px;flex-wrap:wrap",
                        for (label , name , run) in own.clone() {
                            button { key: "{name}", r#type: "button",
                                class: if running.get().as_deref() == Some(label.as_str()) { "btn act" } else { "btn" }, disabled: busy.get(),
                                title: "cargo {run}", "data-t": "task-{label}",
                                onclick: move |_| m.run_task(&i, &label),
                                if running.get().as_deref() == Some(label.as_str()) { "◇ " } else { "▷ " }
                                "{name}"
                            }
                        }
                    }
                }
            }
            div { class: "card", style: "padding:14px 16px",
                div { class: "label mb10", "ax tasks" }
                if !has_project {
                    div { class: "dim", style: "font-size:13px", "no project folder found for this app (add its folder with {super::scan::ROOTS_VAR})" }
                } else {
                    div { style: "display:flex;gap:8px;flex-wrap:wrap",
                        for (label , words) in TASKS.iter().copied() {
                            button { key: "{label}", r#type: "button",
                                class: if running.get().as_deref() == Some(label) { "btn act" } else { "btn" }, disabled: busy.get(),
                                title: "ax {words.join(\" \")}", "data-t": "task-{label}",
                                onclick: move |_| m.run_task(&id, label),
                                if running.get().as_deref() == Some(label) { "◇ " } else { "▷ " }
                                "{label}"
                            }
                        }
                    }
                }
                if busy.get() {
                    div { style: "margin-top:10px;display:flex;align-items:center;gap:10px",
                        button { r#type: "button", class: "btn ghost no", "data-t": "stopTask", onclick: move |_| m.cancel_task(), "■ Stop task" }
                        span { class: "dim", style: "font-size:11px", "long-running tasks (dev) are safe — cancel any time, 5 min cap" }
                    }
                }
            }
            if m.task.with(|t| !t.output.is_empty()) || busy.get() {
                div { class: "card", style: "padding:0;overflow:hidden",
                    div { class: "label outhead",
                        span { "output" }
                        span { "data-t": "taskState",
                            {match m.task.with(|t| (t.running.clone(), t.code)) {
                                (Some(r), _) => rsx! { span { class: "blue", "running {r}…" } },
                                (None, Some(0)) => rsx! { span { class: "green", "exit 0" } },
                                (None, Some(c)) if c > 0 => rsx! { span { class: "red", "exit {c}" } },
                                (None, Some(_)) => rsx! { span { class: "red", "stopped" } },
                                _ => rsx! {},
                            }}
                        }
                    }
                    pre { class: "out", "data-t": "taskOutput",
                        {m.task.with(|t| if t.output.is_empty() { "…".to_string() } else { t.output.clone() })}
                    }
                }
            }
        }
    }
}

/// The nodes a tree shows (amui's `FileBrowser` filter, pure): a filter
/// shows each matching file and its folders, folding ignored; else all,
/// but what is under a folded folder (or the folded root: none).
pub fn visible<'a>(tree: &'a [FileNode], q: &str, collapsed: &BTreeSet<String>) -> Vec<&'a FileNode> {
    let q = q.trim().to_lowercase();
    let parents = |p: &str| -> Vec<String> {
        let parts: Vec<&str> = p.split('/').collect();
        (1..parts.len()).map(|i| parts[..i].join("/")).collect()
    };
    if !q.is_empty() {
        let mut keep = BTreeSet::new();
        for f in tree.iter().filter(|f| !f.dir && f.path.to_lowercase().contains(&q)) {
            keep.insert(f.path.clone());
            keep.extend(parents(&f.path));
        }
        return tree.iter().filter(|f| keep.contains(&f.path)).collect();
    }
    if collapsed.contains(ROOT) {
        return vec![];
    }
    tree.iter().filter(|f| !parents(&f.path).iter().any(|p| collapsed.contains(p))).collect()
}

/// The root row's key among the folded folders.
const ROOT: &str = " root";

/// A file tree beside its viewer (amui's `FileBrowser`).
#[component]
fn FileBrowser(nodes: Vec<FileNode>, truncated: bool, base: String, root_name: String) -> View {
    let view = context::<ViewState>();
    let files = nodes.iter().filter(|f| !f.dir).count();
    rsx! {
        div { class: "files",
            div { class: "card tree",
                div { class: "tree-tools",
                    input { class: "field mono", style: "padding:6px 9px;font-size:12px", "aria-label": "filter files",
                        "data-t": "fileFilter", placeholder: "filter files…", value: view.file_filter,
                        oninput: move |e| view.file_filter.set(e.value()),
                    }
                    div { style: "display:flex;gap:6px;align-items:center;font-size:11px", class: "dim",
                        span { class: "link", role: "button", tabindex: "0", "data-t": "expandAll",
                            onclick: move |_| view.collapsed.set(BTreeSet::new()),
                            onkeydown: move |e| if pressed(&e) { view.collapsed.set(BTreeSet::new()) },
                            "expand all"
                        }
                        span { "·" }
                        span { class: "link", role: "button", tabindex: "0", "data-t": "collapseAll",
                            onclick: move |_| view.collapsed.set(BTreeSet::from([ROOT.to_string()])),
                            onkeydown: move |e| if pressed(&e) { view.collapsed.set(BTreeSet::from([ROOT.to_string()])) },
                            "collapse all"
                        }
                        span { style: "margin-left:auto", "data-t": "fileCount", "{files} files" }
                    }
                }
                div { style: "overflow-y:auto;flex:1;padding:6px", "data-t": "fileList",
                    FileList { nodes, truncated, base: base.clone(), root_name }
                }
            }
            div { class: "card viewer", Viewer {} }
        }
    }
}

/// A file tree's rows (its shape follows the filter, the folds and the open file).
#[component]
fn FileList(nodes: Vec<FileNode>, truncated: bool, base: String, root_name: String) -> View {
    let m = context::<Manager>();
    let view = context::<ViewState>();
    let (q, collapsed, open) = follow(move || {
        (view.file_filter.get(), view.collapsed.get(), m.open.with(|o| o.as_ref().map(|o| o.path.clone())))
    });
    let filtering = !q.trim().is_empty();
    // Each row: its path, name, depth, a folder or not, open, chosen.
    let rows: Vec<(String, String, usize, bool, bool, bool)> = visible(&nodes, &q, &collapsed)
        .into_iter()
        .map(|f| {
            let depth = f.path.matches('/').count() + usize::from(!filtering);
            let is_open = f.dir && (filtering || !collapsed.contains(&f.path));
            let chosen = open.as_deref() == Some(f.path.as_str());
            (f.path.clone(), f.name.clone(), depth, f.dir, is_open, chosen)
        })
        .collect();
    let root_folded = collapsed.contains(ROOT) && !filtering;
    let toggle = move |p: String| {
        view.collapsed.update(|s| {
            if !s.remove(&p) {
                s.insert(p);
            }
        })
    };
    let q = q.trim().to_string();
    rsx! {
        if nodes.is_empty() {
            div { class: "dim", style: "font-size:12px;padding:6px", "no files" }
        } else {
            if !filtering {
                div { class: "node root", role: "button", tabindex: "0", title: "{base}", "data-t": "fileRoot",
                    onclick: move |_| toggle(ROOT.to_string()),
                    onkeydown: move |e| if pressed(&e) { toggle(ROOT.to_string()) },
                    span { class: "dim", if root_folded { "▸ " } else { "▾ " } }
                    "📂 {root_name}"
                }
            }
            if rows.is_empty() && filtering {
                div { class: "dim", style: "font-size:12px;padding:6px", "no files match “{q}”" }
            }
            for (path , name , depth , dir , is_open , chosen) in rows.clone() {
                div { key: "{path}", class: if dir { "node dir" } else if chosen { "node open" } else { "node" }, role: "button", tabindex: "0", title: "{path}",
                    "data-t": "file-{path}", style: "padding-left:{6 + depth * 12}px",
                    onclick: move |_| if dir { toggle(path.clone()) } else { m.open_file(base.clone().into(), path.clone()) },
                    onkeydown: move |e| if pressed(&e) { if dir { toggle(path.clone()) } else { m.open_file(base.clone().into(), path.clone()) } },
                    span { class: "dim", if dir { if is_open { "▾ " } else { "▸ " } } else { "  " } }
                    if dir { "📁 " } else { "📄 " }
                    "{name}"
                }
            }
            if truncated {
                div { class: "dim", style: "font-size:10px;padding:6px", "… tree truncated (very large project)" }
            }
        }
    }
}

/// The open file, read-only (it follows which file is open).
#[component]
fn Viewer() -> View {
    let m = context::<Manager>();
    let Some(o) = follow(move || m.open.get()) else {
        return rsx! { div { class: "dim", style: "padding:24px;font-size:13px", "select a file to view (read-only)" } };
    };
    let close = move || {
        let o = m.open;
        o.set(None)
    };
    let runs: Vec<(usize, Option<&'static str>, String)> =
        super::highlight::highlight(o.content.as_deref().unwrap_or(""), &o.path)
            .into_iter()
            .enumerate()
            .map(|(i, (kind, run))| (i, kind, run.to_string()))
            .collect();
    let hint = o.hint.clone().unwrap_or_default();
    let notice = o.notice.clone().unwrap_or_default();
    rsx! {
        div { class: "label outhead",
            span { class: "mono text2", style: "text-transform:none", "data-t": "openPath",
                "{o.path}"
                if o.hint.is_some() {
                    span { class: "blue", style: "margin-left:10px", "data-t": "fileHint", "{hint}" }
                }
            }
            span { role: "button", tabindex: "0", "aria-label": "close file", style: "cursor:pointer", "data-t": "closeFile",
                onclick: move |_| close(),
                onkeydown: move |e| if pressed(&e) { close() },
                "✕"
            }
        }
        if o.notice.is_some() {
            div { class: "mono yellow", style: "padding:24px;font-size:13px", "data-t": "fileNotice", "ⓘ {notice}" }
        } else {
            pre { class: "code", "data-t": "fileText",
                for (i , kind , run) in runs.clone() {
                    Fragment { key: "{i}",
                        {match kind {
                            Some(k) => rsx! { span { class: "hl-{k}", "{run}" } },
                            None => rsx! { "{run}" },
                        }}
                    }
                }
            }
        }
    }
}

/// The Codebase tab (amui's `CodebaseTab`): the files of what runs, or of
/// the whole repo it is in when that is a wider folder.
#[component]
fn CodebaseTab() -> View {
    let m = context::<Manager>();
    let view = context::<ViewState>();
    let (f, repo, loading) = follow(move || (m.files.get(), view.repo.get(), m.detail_loading.get()));
    let app_dir = f.runtime.as_ref().map(|r| r.root.clone());
    let can_repo = f.repo.is_some() && f.repo != app_dir;
    let use_repo = can_repo && repo;
    let base = if use_repo { f.repo.clone() } else { app_dir.clone() }.unwrap_or_default();
    let (nodes, truncated) =
        if use_repo { (f.repo_tree.clone(), f.repo_truncated) } else { (f.tree.clone(), f.truncated) };
    let kind = f.runtime.as_ref().map(|r| r.kind);
    let pill = move |repo: bool, text: &'static str, t: &'static str| {
        let on = use_repo == repo;
        rsx! {
            span { class: if on { "chip pill on" } else { "chip pill" }, role: "button", tabindex: "0", "data-t": "{t}",
                onclick: move |_| {
                    view.repo.set(repo);
                    if repo && m.files.with_untracked(|f| f.repo_tree.is_none() && !f.repo_loading) {
                        m.load_codebase();
                    }
                },
                "{text}"
            }
        }
    };
    let name = base.file_name().map_or(base.display().to_string(), |n| n.to_string_lossy().into_owned());
    let k = kind.unwrap_or_default();
    let has_repo = f.repo.is_some();
    let waiting = nodes.is_none() && (use_repo || loading);
    let base = base.display().to_string();
    rsx! {
        div { class: "col", style: "gap:8px",
            div { class: "small dim", style: "display:flex;gap:8px;align-items:center;flex-wrap:wrap",
                if kind.is_some() {
                    span { class: if matches!(k, "release" | "installed" | "appimage") { "chip yellow" } else { "chip blue" }, "{k}" }
                }
                span { class: if has_repo { "green" } else { "dim" }, if has_repo { "git" } else { "no git" } }
                if can_repo {
                    span { class: "row6", style: "margin-left:4px",
                        {pill(false, "app dir", "srcApp")}
                        {pill(true, "repo root", "srcRepo")}
                    }
                }
                span { class: "text2", style: "margin-left:auto", "data-t": "codebaseBase", "{base}" }
            }
            if waiting {
                Empty { msg: "loading codebase…" }
            } else {
                FileBrowser { nodes: nodes.clone().unwrap_or_default(), truncated, base: base.clone(), root_name: name.clone() }
            }
        }
    }
}

/// The chosen app (amui's `Detail`): its tabs follow the tab and the app
/// (and its process: a restart reads it anew); each tab follows the rest.
#[component]
fn DetailPane() -> View {
    let m = context::<Manager>();
    let view = context::<ViewState>();
    let (tab, shown) = follow(move || {
        let d = m
            .detail
            .with(|d| d.as_ref().map(|d| (d.id.clone(), d.pid, d.running, d.path.is_some(), d.meta.tasks.clone())));
        (view.tab.get(), d)
    });
    let Some((id, pid, _, has_project, own)) = shown else { return rsx! {} };
    let pid = pid.unwrap_or(0);
    let body = match tab {
        Tab::Overview => rsx! { Overview {} },
        Tab::Cells => rsx! { CellsTab {} },
        Tab::State => rsx! { StateTab { key: "{id}@{pid}", id: id.clone() } },
        Tab::Metrics => rsx! { MetricsTab {} },
        Tab::Logs => rsx! { LogsTab { key: "{id}", id: id.clone() } },
        Tab::Tasks => rsx! { TasksTab { id: id.clone(), has_project, own } },
        Tab::Codebase => rsx! { CodebaseTab {} },
    };
    rsx! {
        div { class: "detail",
            Header {}
            div { class: "body", "data-t": "body", {body} }
        }
    }
}

/// The Metrics tab (amui's `MetricsTab`): it follows every reading.
#[component]
fn MetricsTab() -> View {
    let m = context::<Manager>();
    let (running, connections, diag, hist) = follow(move || {
        let (running, connections) =
            m.detail.with(|d| d.as_ref().map_or((false, None), |d| (d.running, d.connections)));
        (running, connections, m.diag.get(), m.hist.get())
    });
    if !running && hist.cpu.is_empty() {
        return rsx! { Empty { msg: "metrics stream once the app is running & selected" } };
    }
    rsx! { MetricsPanel { diag, hist, connections } }
}

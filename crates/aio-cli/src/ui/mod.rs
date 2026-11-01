//! `ax ui` (aio's `am ui`, the app manager amui): every aio-rs app on this
//! machine, running or on disk, inspected deeply and run, in a window of
//! aio-rs's own on the shared runtime. A port of amui, file for file:
//!
//! | amui | here |
//! |---|---|
//! | `app.ts` | this file: the window, `--browser`, `ax open` |
//! | `App.tsx` | [`app`]: sidebar, header, the seven tabs |
//! | `manager.ts` | [`manager`]: the state and what changes it |
//! | `server/scan.server.ts` | [`scan`]: the apps and projects found |
//! | `server/proc.server.ts` | [`proc`]: processes, files, logs, tasks |
//! | `ui/metrics.tsx`, `charts.tsx` | [`metrics`], [`charts`] |
//! | `ui/logs.tsx`, `state-view.tsx`, `json-tree.tsx` | [`logs`], [`state_view`], [`json_tree`] |
//! | `ui/highlight.tsx`, `style.ts` | [`highlight`], [`style`] |
//!
//! What it reads of a running app it reads as `ax` does: its verbs, run as
//! children, and its files. `ax ui --browser` serves the same view to the
//! browser's tabs.
//!
//! `ax open <app>` (aio's `am open`): the running app's own page in the
//! browser, once it answers (`--print`: only its address).
pub mod app;
pub mod charts;
pub mod highlight;
pub mod json_tree;
pub mod logs;
pub mod manager;
pub mod metrics;
pub mod proc;
pub mod scan;
pub mod state_view;
pub mod style;
#[cfg(test)]
mod tests;

use aio::serde_json::Value;
use aio::ui::prelude::*;
pub use manager::{Verb, child};
use std::path::PathBuf;
use std::time::Duration;

/// Where the person's choices are kept (the context of [`app::App`]).
#[derive(Clone)]
pub struct Prefs(pub Option<PathBuf>);

/// What a running app is reached at, from its `serving.json` (pure).
pub fn url(serving: &Value) -> Option<String> {
    let port = serving["port"].as_u64()?;
    let scheme = if serving["tls"] == true { "https" } else { "http" };
    let host =
        serving["host"].as_str().filter(|h| !h.is_empty() && *h != "0.0.0.0" && *h != "::").unwrap_or("localhost");
    Some(format!("{scheme}://{host}:{port}"))
}

/// The program that opens a URL on `os` (pure).
pub fn opener(os: &str) -> &'static str {
    match os {
        "macos" => "open",
        "windows" => "explorer",
        _ => "xdg-open",
    }
}

/// `ax open <app>`: its page in the browser, once something answers there
/// (`print`: only the address).
pub fn open_app(name: &str, data: &std::path::Path, print: bool) -> Result<String, String> {
    let serving: Value = std::fs::read(data.join("serving.json"))
        .ok()
        .and_then(|b| aio::serde_json::from_slice(&b).ok())
        .ok_or_else(|| format!("{name} serves no page: start it with `ax start {name}` (a window app has none)"))?;
    let at = url(&serving).ok_or_else(|| format!("{name}: its serving.json names no port"))?;
    let port = serving["port"].as_u64().unwrap_or(0) as u16;
    if std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_secs(2)).is_err() {
        return Err(format!("nothing answers on {at}: start it with `ax start {name}`"));
    }
    if print {
        return Ok(at);
    }
    let os = std::env::consts::OS;
    let opened = std::process::Command::new(opener(os)).arg(&at).status();
    // `explorer` answers 1 when it worked.
    Ok(match opened {
        Ok(s) if s.success() || os == "windows" => format!("opened {at}"),
        _ => format!("open it: {at}"),
    })
}

/// The manager's window (amui's: 1240×800, its own dark until it paints).
fn window() -> aio_shell::Window {
    aio_shell::Window {
        title: "ax ui — aio-rs Manager UI",
        width: 1240,
        height: 800,
        background: Some(style::BACKGROUND),
        ..Default::default()
    }
}

/// Where this run keeps the person's choices.
static PREFS: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// The window's root: the manager, with its choices kept.
fn root() -> View {
    provide(|| Prefs(PREFS.get().cloned()));
    if let Some(v) = TAB_VERB.get() {
        provide(|| v.clone());
    }
    rsx! { app::App {} }
}

/// `ax ui`: the manager in a window of its own (`window`: here; else a
/// detached `ax ui --window`, so the terminal stays free); `browser`: in
/// the browser instead, served from here until Ctrl-C.
pub fn open(window: bool, browser: bool) -> Result<String, String> {
    if window || browser {
        let dirs = aio::paths::app(scan::SELF_ID).map_err(|e| format!("ax ui: {e} (is one open already?)"))?;
        _ = PREFS.set(dirs.data.join("ui.json"));
        if browser {
            return in_browser().map(|_| String::new());
        }
        return aio_shell::run(root, self::window(), &dirs).map(|_| String::new()).map_err(|e| e.to_string());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let log = std::env::temp_dir().join(format!("aio-ui-{}.log", std::process::id()));
    let mut cmd = crate::start::command(&exe, &["ui".into(), "--window".into()], &log, None)?;
    let child = cmd.spawn().map_err(|e| format!("ax ui: {e}"))?;
    Ok(format!("ax ui opened (pid {}); its log: {}", child.id(), log.display()))
}

/// What the browser's tabs link to: the manager reads verbs, not cells.
#[aio::cell(shared)]
pub struct Desk {}

#[aio::methods]
impl Desk {}

/// The verbs a browser tab's manager runs ([`serve`]).
static TAB_VERB: std::sync::OnceLock<Verb> = std::sync::OnceLock::new();

/// Serve the manager on `l` to browsers that show `key` (its `?token=`
/// link sets the key's HttpOnly cookie), each tab running `verb`, until
/// the server stops.
pub async fn serve(l: tokio::net::TcpListener, key: String, verb: Verb) -> std::io::Result<()> {
    _ = TAB_VERB.set(verb);
    let http = aio_shell::web::tabs(aio::http::Http::default(), root, window());
    let opts = aio::sync::Options { token: key, http, ..Default::default() };
    aio::sync::serve_with(aio::Handle::<Desk>::memory(), l, opts).await
}

/// `ax ui --browser` (aio's `am ui --client=browser`): the manager as a page
/// of this machine's browser, on a loopback port behind a key made for
/// this run; said with its link when no browser opens.
fn in_browser() -> Result<(), String> {
    let key = format!("{}{}", aio::uuid_v4().simple(), aio::uuid_v4().simple());
    aio::rt().block_on(async move {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.map_err(|e| format!("ax ui: {e}"))?;
        let at = format!("http://{}/", l.local_addr().map_err(|e| e.to_string())?);
        let link = format!("{at}?token={key}");
        match aio_shell::os::open_url(&link).await {
            Ok(()) => eprintln!("ax ui: {at} (Ctrl-C ends it)"),
            Err(e) => eprintln!("ax ui: open {link} in a browser ({e}); Ctrl-C ends it"),
        }
        serve(l, key, child()).await.map_err(|e| format!("ax ui: {e}"))
    })
}

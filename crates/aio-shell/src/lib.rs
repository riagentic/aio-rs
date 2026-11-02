//! aio's desktop host: the app's view tree lives in Rust, and the shared
//! Electron runtime (as in aio) draws it. DOM edits and events travel as
//! length-prefixed frames over a private socket (a 0700 runtime dir): no
//! listening port, no network for the page (it loads from `aio://app`).
use aio::ui::View;
use bytes::Bytes;
use dioxus_core::VirtualDom;
use dioxus_liveview::{LiveViewError, LiveViewPool, LiveViewSocket};
use futures_util::{SinkExt, StreamExt};
use std::path::Path;
use std::sync::Arc;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

#[derive(Clone, Copy, Default)]
pub struct Window {
    pub title: &'static str,
    pub width: u32,
    pub height: u32,
    /// The window's icon: its title bar, taskbar, Alt-Tab, Dock, tray and
    /// notifications, and the page's favicon. PNG bytes, shown at most 256 px
    /// a side; `Window::of` gives the app's. Unset: the app's monogram (its
    /// title's letter in its hue, aio's generated icon).
    pub icon: Option<&'static [u8]>,
    /// The background until the page paints, `[light, dark]` by the OS theme
    ///; unset: the kit's page background.
    pub background: Option<[&'static str; 2]>,
    /// The page's language, `<html lang>` (screen readers and spellcheck
    /// read it, WCAG 3.1.1); unset: `en`.
    pub lang: Option<&'static str>,
    /// The page's direction, `<html dir>`. Never guessed from `lang`.
    pub dir: Dir,
    /// The `<meta name="viewport">` content; unset: the device's width at
    /// scale 1. `Some("")`: no tag (a fixed-width page on a phone).
    pub viewport: Option<&'static str>,
    /// A tray icon with this menu (`desktop::Tray`); unset: none.
    pub tray: Option<desktop::Tray>,
    /// The app's own `<head>` markup, verbatim (aio's `ui.head`): a `<style>`,
    /// `<meta>` tags. After aio's policy, which it can only tighten.
    pub head: Option<&'static str>,
    /// Other sites inside the app: `aio::kit::Browser`, child windows,
    /// their preloads (`guest::Web`); unset: none.
    pub web: guest::Web,
    /// What the page may also hold beyond copying text and fullscreen
    /// (`guest::PERMISSIONS`: `"clipboard-read"`…); never a guest's.
    pub permissions: &'static [&'static str],
    /// A page's beforeunload veto asks Leave / Stay; unset: the window
    /// stays, and the first veto is said in the log.
    pub confirm_unload: bool,
    /// The window's own keys given to the page (`guest::Keys`).
    pub keys: guest::Keys,
    /// No window rather than one without Chromium's sandbox (aio's
    /// `requireSandbox`): on a host that can't sandbox (a Linux with user
    /// namespaces closed and no setuid helper) the app stops, saying why,
    /// instead of starting the runtime with `--no-sandbox`.
    pub require_sandbox: bool,
    /// The window waits for its page's first paint at most this long
    /// (aio's `showWhenPainted: { capMs }`); `Some(0)`: shown at once
    /// (`false`); unset: 5000.
    pub show_cap_ms: Option<u32>,
    /// The app goes on serving once its window closes (or crashes), until
    /// asked to stop (aio's `keepServer`; `--keep-server` asks it per launch).
    pub keep_server: bool,
    /// The app's say over its splash at run time (aio's `splash.show`),
    /// asked just before it would open: a setting "no splash at start" is
    /// `|| SplashShow::FirstStart`. Unset: `[build.splash] show`. It may
    /// take 50 ms; one that panics or takes longer leaves the build's
    /// `show`, said in the log.
    pub splash_show: Option<fn() -> SplashShow>,
}

pub use aio::config::SplashShow;

impl Window {
    /// The window `[app]` describes: its name as the title, its own icon
    /// (`assets/icon.png`, built in by `aio::config!()`; none: the shell draws
    /// its monogram), its `lang` and `dir`; the rest default (`Window { width: 900,
    /// ..Window::of(&cfg) }`).
    pub fn of(cfg: &aio::config::Config) -> Self {
        let lang = cfg.app.lang.clone().map(|l| &*l.leak());
        let icon = Some(cfg.app.icon).filter(|i| !i.is_empty());
        Self { title: cfg.app.name.clone().leak(), icon, lang, dir: cfg.app.dir, ..Self::default() }
    }
}

/// Which way the page's text runs (`[app] dir`).
pub use aio::config::Dir;

#[cfg(target_os = "android")]
pub mod android;
pub mod delta;
pub mod desktop;
mod dialog;
mod feedback;
mod firstpage;
mod firstrun;
mod fresh;
pub mod fuses;
#[cfg(all(target_os = "linux", any(test, feature = "native-splash")))]
mod gtksplash;
pub mod guest;
#[cfg(all(test, target_os = "linux"))]
mod guests;
#[cfg(any(target_os = "android", all(test, unix)))]
mod handover;
pub mod inspect;
pub mod instance;
#[cfg(all(test, unix))]
mod islands;
pub mod macmove;
pub mod oneclick;
pub mod os;
pub mod page;
pub mod pidlock;
pub mod place;
// The OS's splash (`native-splash`, which `ax build` turns on for an app
// with a `[build.splash]`); without it Electron draws the splash.
#[cfg(any(test, feature = "native-splash"))]
mod presplash;
#[cfg(not(any(test, feature = "native-splash")))]
#[path = "presplash_off.rs"]
mod presplash;
pub mod prune;
#[cfg(debug_assertions)]
mod renders;
pub mod runtime;
pub mod splash;
mod stderr;
pub mod swap;
pub mod switches;
pub mod update;
mod urls;
#[cfg(feature = "web")]
pub mod web;
#[cfg(windows)]
mod winstall;
#[cfg(target_os = "linux")]
mod xroom;
// The guide's pages that call this crate (`docs/`), as doctests.
#[cfg(doctest)]
mod guide;

pub use guest::{Child, Web, clear_partition, open_window};
pub use instance::{Launch, Start, launches, start};
/// The page: an empty root, the DOM-edit interpreter and the page scripts
/// (`aio::page`), under its policy (`aio::page::sealed`).
fn page(win: &Window) -> String {
    page_with(win, "")
}

pub use aio::page::overlay;

/// The page, with `bridge` defining `window.__aioWire` first where no
/// preload does (Android's WebView). Its language, direction, viewport and
/// title are the window's, escaped. Its policy names each inline script
/// (the app's own `head` too) and connects to nothing.
pub(crate) fn page_with(win: &Window, bridge: &str) -> String {
    page_html(win, bridge, "'none'", "")
}

/// [`page_with`] for a browser tab, connecting to `connect` (its own
/// server), with the window's icon (else the app's monogram) as its
/// favicon (aio's `/__aio/icon`).
#[cfg(any(feature = "web", test))]
pub(crate) fn page_of(win: &Window, bridge: &str, connect: &str) -> String {
    use base64::Engine;
    let png = win.icon.map(<[u8]>::to_vec).unwrap_or_else(|| {
        let id = instance::APP.get().map_or("app", |(id, _)| id.as_str());
        let hue = instance::LOOK.get().map_or_else(|| aio::colors::hue(id), |l| l.0);
        aio_config::icon::icon_png(if win.title.is_empty() { id } else { win.title }, hue, 64)
    });
    let b64 = base64::engine::general_purpose::STANDARD.encode(png);
    let icon = format!(r#"<link rel="icon" type="image/png" href="data:image/png;base64,{b64}">"#);
    page_html(win, bridge, connect, &icon)
}

fn page_html(win: &Window, bridge: &str, connect: &str, icon: &str) -> String {
    let html = html_open(win.lang, win.dir);
    let viewport = match win.viewport.unwrap_or("width=device-width, initial-scale=1") {
        "" => String::new(),
        v => format!(r#"<meta name="viewport" content="{}">"#, esc(v)),
    };
    let page = format!(
        r#"<!doctype html>{html}<head><meta charset="utf-8">{viewport}<title>{}</title>{icon}{}</head><body>{}</body></html>"#,
        esc(win.title),
        win.head.unwrap_or(""),
        aio::page::body(bridge)
    );
    aio::page::sealed(&page, connect)
}

/// The page's `<html>` tag: its language (`en` unless set) and its
/// direction, escaped (pure; the web build's page uses it too).
pub fn html_open(lang: Option<&str>, dir: Dir) -> String {
    let lang = esc(lang.map(str::trim).filter(|l| !l.is_empty()).unwrap_or("en"));
    let dir = match dir {
        Dir::Ltr => "",
        Dir::Rtl => r#" dir="rtl""#,
        Dir::Auto => r#" dir="auto""#,
    };
    format!(r#"<html lang="{lang}"{dir}>"#)
}

/// `s` as HTML text or a quoted attribute value.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

/// One window's frames as the interpreter's socket, a held key's stale
/// repeats left out ([`fresh`]).
fn wire(s: aio::ipc::Stream) -> impl LiveViewSocket {
    let codec = LengthDelimitedCodec::builder().max_frame_length(256 << 20).new_codec();
    fresh::Fresh::new(
        Framed::new(s, codec)
            .map(|r| r.map(|b| b.to_vec()).map_err(|_| LiveViewError::SendingFailed))
            .with(|v: Vec<u8>| std::future::ready(Ok::<_, std::io::Error>(Bytes::from(v))))
            .sink_map_err(|_| LiveViewError::SendingFailed),
    )
}

/// The monogram's edge: the largest a window or a tray shows an icon at
/// (as aio draws it for its window).
const MONOGRAM_PX: u32 = 256;

/// The monogram of an app with no icon (aio's generated one): `label`'s
/// letter in `hue`, as a PNG in `dir`, drawn again only when they change.
fn monogram(dir: &Path, label: &str, hue: u16) -> std::io::Result<std::path::PathBuf> {
    let (png, key) = (dir.join("monogram.png"), dir.join("monogram.key"));
    let want = format!("{label}\n{hue}\n{MONOGRAM_PX}\n{}\n", env!("CARGO_PKG_VERSION"));
    if !png.is_file() || std::fs::read_to_string(&key).ok().as_deref() != Some(want.as_str()) {
        std::fs::write(&png, aio_config::icon::icon_png(label, hue, MONOGRAM_PX))?;
        std::fs::write(&key, want)?;
    }
    Ok(png)
}

/// Write the shell files into `dir` (mode 0700: the socket next to them is
/// the app's door).
/// `id` names the runtime's app, so the OS sees each aio-rs app as its own
/// (window class / Wayland app id, taskbar grouping).
fn install(dir: &Path, id: &str, win: &Window) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    std::fs::write(dir.join("main.js"), include_str!("main.js"))?;
    std::fs::write(dir.join("preload.js"), include_str!("preload.js"))?;
    // `desktopName`: the menu entry the window belongs to (Wayland's app_id).
    let pkg = aio::serde_json::json!({ "name": id, "productName": win.title, "desktopName": format!("{id}.desktop"), "main": "main.js" })
        .to_string();
    std::fs::write(dir.join("package.json"), pkg)?;
    std::fs::write(dir.join("tray.json"), desktop::tray_json(win.tray.as_ref(), win.title))?;
    guest::install(dir, win)?;
    // No splash until `splash::install` lays one out.
    std::fs::write(dir.join("splash.json"), "null")?;
    assets(dir, instance::ASSETS.get().copied().unwrap_or_default())?;
    std::fs::write(dir.join("index.html"), page(win))
}

/// The app's built-in files (`[build] assets`) in `dir/assets` at their
/// URL paths, where the page's `aio://app/…` finds them (main.js); written
/// again only when they changed (`assets.key`: their paths and hash).
fn assets(dir: &Path, files: &[(&str, &[u8])]) -> std::io::Result<()> {
    let (root, key) = (dir.join("assets"), dir.join("assets.key"));
    let mut h = aio::sha256::Sha256::new();
    for (path, bytes) in files {
        h.update(path);
        h.update([0]);
        h.update(bytes);
    }
    let want = aio::sha256::hex(&h.finalize());
    if root.is_dir() && std::fs::read_to_string(&key).ok().as_deref() == Some(want.as_str()) {
        return Ok(());
    }
    _ = std::fs::remove_dir_all(&root);
    for (path, bytes) in files {
        let to = root.join(aio::config::asset_path(path));
        std::fs::create_dir_all(to.parent().unwrap_or(&root))?;
        std::fs::write(to, bytes)?;
    }
    std::fs::create_dir_all(&root)?;
    std::fs::write(key, want)
}

/// `ax release` asks what data this build reads (`AIORT_DATA_CONTRACT`:
/// the file to say it in): the cells `main` opened before [`run`] or
/// [`serve`], said instead of opening a window or serving. None: not asked.
fn said_contract() -> Option<std::io::Result<()>> {
    let f = std::env::var_os("AIORT_DATA_CONTRACT").filter(|v| !v.is_empty())?;
    aio::health::stop();
    Some(aio::serde_json::to_vec(&aio::update::contract()).map_err(Into::into).and_then(|b| std::fs::write(f, b)))
}

/// Open `app` in the shared runtime; returns when the window closes.
/// The runtime's profile and the socket live in the app's own dirs.
pub fn run(app: fn() -> View, win: Window, dirs: &aio::paths::AppDirs) -> std::io::Result<()> {
    let dev = std::env::var_os("AIORT_DEV_CLOSED").filter(|_| cfg!(debug_assertions));
    // Under `ax dev`'s dx: a dx that goes (its session killed) stops us
    // gracefully, as a parent that goes does; dx sets no watch of its own.
    #[cfg(target_os = "linux")]
    if dev.is_some() {
        // SAFETY: plain prctl on this process.
        unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) };
    }
    let shown = show(app, win, dirs);
    #[cfg(target_os = "macos")]
    macmove::close();
    // The person closed the window, so the dev session ends too; a stop
    // asked (dx restarting the app after a rebuild, `ax stop`) is not that.
    if let Some(f) = dev.filter(|_| shown.is_ok() && !STOP_ASKED.load(std::sync::atomic::Ordering::Relaxed)) {
        _ = std::fs::write(f, b"closed");
    }
    shown
}

thread_local!(static WINDOW_THREAD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });

/// A panic on a window's thread (a render, a hot-patch dx's devtools failed
/// to apply) is said as the window's: the pool it runs on would only say
/// "sending failed", and the window would freeze without a word.
fn say_window_panics() {
    let before = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        before(info);
        if WINDOW_THREAD.get() {
            eprintln!("{}", window_panicked(info.payload()));
        }
    }));
}

/// What is said when the window's task panicked (pure).
fn window_panicked(payload: &(dyn std::any::Any + Send)) -> String {
    format!("[aio-shell] window panicked: {} (it no longer updates; restart the app)", aio::report::panic_text(payload))
}

/// The window ended because a stop was asked (SIGTERM, Ctrl-C), not closed.
static STOP_ASKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn show(app: fn() -> View, win: Window, dirs: &aio::paths::AppDirs) -> std::io::Result<()> {
    aio::lifecycle::harden();
    if let Some(said) = said_contract() {
        return said;
    }
    // `ax dev`: say when a hot-patch lands, so a patch that didn't is seen;
    // numbered, so the log never folds patches in a row into one line.
    #[cfg(all(debug_assertions, feature = "dev"))]
    subsecond::register_handler(Arc::new(|| {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        // SAFETY: read at once, not kept across patches.
        let fns = unsafe { subsecond::get_jump_table() }.map_or(0, |t| t.map.len());
        eprintln!("{}", aio::log::hot_patched(n, fns))
    }));
    // A dev build says a component caught in a render loop, by name, and
    // keeps the component tree for `ax components`.
    #[cfg(debug_assertions)]
    {
        let wire = renders::Tripwire::new(|l| aio::warn!(cat: "ui", "{l}"));
        _ = renders::TREE.set(wire.tree());
        _ = tracing::subscriber::set_global_default(wire);
    }
    if instance::SHOWN.get() == Some(&aio::config::Shown::Browser) {
        return in_browser(dirs);
    }
    // `ax dev --size WxH` (a debug build's): the window opens at that size;
    // its saved size and place are neither used nor changed.
    let sized = asked_size(std::env::var("AIORT_WINDOW_SIZE").ok().filter(|_| cfg!(debug_assertions)).as_deref());
    let win = match sized {
        Some((width, height)) => Window { width, height, ..win },
        None => win,
    };
    let win = Window { title: titled(std::env::var("AIORT_TITLE").ok(), win.title), ..win };
    let data = dirs.data.join("electron");
    let dir = dirs.cache.join("shell");
    _ = ASK_MOVE.set((win.title, dirs.data.clone()));
    install(&dir, &dirs.id, &win)?;
    let look = instance::LOOK.get();
    let colors = splash::colors(look.map_or(0, |l| l.0), win.background);
    let splashed = look.and_then(|l| l.1.as_ref());
    let splash_colors = match splashed.and_then(|s| s.background.as_ref()) {
        Some([l, d]) => splash::colors(look.map_or(0, |l| l.0), Some([l.as_str(), d.as_str()])),
        None => colors.clone(),
    };
    // The download window opens over the app's window, as it was last placed.
    let saved = place::load(&dirs.data.join("window.json"));
    let at = saved.map(|s| (s.normal.x + s.normal.width / 2, s.normal.y + s.normal.height / 2));
    let back = instance::APP.get().filter(|(id, v)| update::can_go_back(id, v)).map(|(id, _)| id.clone());
    // `[build.splash] show`, or the app's say at run time (aio's `splash.show`).
    let (show, by_app) =
        splashed.map_or((aio::config::SplashShow::Never, false), |s| splash::run_time_show(win.splash_show, s.show));
    let (at_start, in_download) = splash::shown(show);
    let downloading = splashed.filter(|_| in_download);
    // Drawn by the OS before Electron starts (`presplash`), when it can.
    let os = std::env::consts::OS;
    let native = downloading.and_then(|s| {
        let plan = presplash::Plan {
            splash: s,
            title: win.title,
            version: instance::APP.get().map_or("", |(_, v)| v.as_str()),
            colors: &splash::look_colors(splash_colors.clone(), s),
            target: Some(splash::target(saved.as_ref(), win.width, win.height)),
            at_start,
            by_app,
            may_download: runtime::may_download(),
        };
        presplash::Session::start(&plan, presplash::kind(os, &|k| std::env::var(k).ok())?, os)
    });
    // However the start ends, the splash goes with it.
    let _splash_ends = presplash::Ends;
    // A first start draws the splash in the download window, with its bar,
    // when the OS's splash cannot.
    let dl_image = downloading.and_then(|s| Some((s.png, splash::image_box(s)?)));
    let dl_colors = match downloading {
        Some(s) => splash::look_colors(splash_colors.clone(), s),
        None => firstrun::recolored(colors, firstrun::words()),
    };
    let icon = match win.icon {
        Some(png) => match place::icon_size(png) {
            Some(size) => {
                std::fs::write(dir.join("icon.png"), png)?;
                Some((dir.join("icon.png"), size))
            }
            None => {
                eprintln!("[aio] {}: the window icon is not a PNG; Electron's is shown", dirs.id);
                None
            }
        },
        // None of its own: its monogram (aio's), so no window, taskbar
        // entry or tray wears Electron's logo.
        None => {
            let label = if win.title.is_empty() { dirs.id.as_str() } else { win.title };
            let hue = instance::LOOK.get().map_or_else(|| aio::colors::hue(&dirs.id), |l| l.0);
            Some((monogram(&dir, label, hue)?, (MONOGRAM_PX, MONOGRAM_PX)))
        }
    };
    let socks = aio::paths::sockets(&dirs.id, &dirs.data)?;
    let (sock, ctl_sock, hand) =
        (aio::ipc::addr(&socks, "wire"), aio::ipc::addr(&socks, "ctl"), instance::hand_addr(&dirs.id, &dirs.data)?);
    let r = tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(async {
        let mut listener = aio::ipc::Listener::bind(&sock)?;
        // The runtime's control line (Rust → Electron main): "focus".
        let mut ctl_listener = aio::ipc::Listener::bind(&ctl_sock)?;
        // Both serve the window this spawns and no other process of the user.
        let only = aio::ipc::Only::default();
        let ctl: Ctl = Arc::new(tokio::sync::Mutex::new(None));
        _ = CTL.set(ctl.clone());
        let (c, state, ctl_only) = (ctl.clone(), sized.is_none().then(|| dirs.data.join("window.json")), only.clone());
        tokio::spawn(async move {
            while let Ok(s) = ctl_listener.accept_only(&ctl_only).await {
                use tokio::io::AsyncWriteExt;
                let (r, mut w) = tokio::io::split(s);
                // What it missed: the icons the app set before it connected.
                for l in desktop::icon_lines() {
                    _ = w.write_all(format!("{l}\n").as_bytes()).await;
                }
                *c.lock().await = Some(w);
                tokio::spawn(control(r, c.clone(), state.clone(), win));
            }
        });
        // Later launches of the app hand their arguments over here.
        tokio::spawn(take_launches(aio::ipc::Listener::bind(&hand)?, ctl.clone(), aio::lifecycle::ask_stop));
        // `ax surface`, `trigger`, `shot` and `eval` reach the window here:
        // a dev build's only (aio's control API was dev-only); a release
        // build opens no DevTools protocol (AIORT_CDP is a debug build's).
        if cfg!(debug_assertions) {
            let ui = aio::ipc::Listener::bind(&aio::ipc::addr(&socks, inspect::DOOR))?;
            tokio::spawn(inspect::serve(ui, inspect::hub().clone(), ctl.clone()));
        }
        say_window_panics();
        let (pool, wire_only) = (Arc::new(LiveViewPool::new()), only.clone());
        tokio::spawn(async move {
            while let Ok(s) = listener.accept_only(&wire_only).await {
                let pool = pool.clone();
                tokio::spawn(async move {
                    let dom = move || {
                        WINDOW_THREAD.set(true);
                        VirtualDom::new(app)
                    };
                    if let Err(e) = pool.launch_virtualdom(wire(s), dom).await {
                        eprintln!("[aio-shell] window: {e}");
                    }
                });
            }
        });
        aio::health::watch(tokio::runtime::Handle::current());
        // What cells tell the people using the app is shown by this window.
        let here = tokio::runtime::Handle::current();
        aio::tell::show_with(move |t| desktop::show(&here, t));
        let shown = icon.as_ref().map(|(p, _)| p.clone());
        let rt = tokio::task::spawn_blocking(move || {
            let go_back = |id: &str| update::go_back(id);
            let back = back.as_deref().map(|id| move || go_back(id));
            let shown = shown.as_deref();
            runtime::ensure_as(
                win.title,
                &dl_colors,
                at,
                shown,
                dl_image,
                back.as_ref().map(|f| f as &dyn Fn() -> std::io::Result<()>),
            )
        })
        .await
        .map_err(std::io::Error::other)??;
        // Electron is here: its main process draws the start's splash unless
        // the OS's is up (main.js reads `splash.json`).
        let electron_draws = native.as_ref().is_none_or(|n| n.handoff());
        let t = splash::target(saved.as_ref(), win.width, win.height);
        let target = t.0.zip(t.1).map(|(x, y)| [x, y, t.2, t.3]);
        splash::install(&dir, splashed.filter(|_| at_start && electron_draws), win.title, &splash_colors, target)?;
        let sandbox = runtime::sandbox_for(&rt, win.require_sandbox)?;
        // a GPU that failed here once is never asked again (AIORT_GPU=hardware is).
        let gpu_failed = data.join("gpu-failed");
        let (gpu, _) = gpu_choice(gpu_failed.exists(), &|k| std::env::var(k).ok());
        let env = |k: &str| std::env::var(k).ok();
        aio::boot::say(&runtime_rows(&rt, &sandbox, win.require_sandbox, gpu_failed.exists(), &env));
        // macOS: the app's own window bundle, its name in the Dock.
        let exe = if cfg!(target_os = "macos") {
            let (rt, id, name) = (rt.clone(), dirs.id.clone(), win.title.to_string());
            let version = instance::APP.get().map_or("0.0.0".into(), |(_, v)| v.clone());
            let bundle_id = instance::BUNDLE_ID.get().cloned().unwrap_or_else(|| format!("aio.rt.{id}"));
            let icon = icon.as_ref().map(|(p, _)| p.clone());
            tokio::task::spawn_blocking(move || {
                runtime::window(
                    &rt,
                    &runtime::Identity {
                        id: &id,
                        bundle_id: &bundle_id,
                        name: &name,
                        version: &version,
                        icon: icon.as_deref(),
                    },
                )
            })
            .await
            .map_err(std::io::Error::other)??
        } else {
            runtime::exe(&rt)
        };
        // The store learns this app starts from it (and its macOS window), in
        // aio's record, and sheds what nothing does; a runtime bundled with
        // the app is none of its business.
        if std::env::var_os("AIORT_RUNTIME").is_none() && rt.starts_with(runtime::root()) {
            let root = runtime::root();
            let name = rt.file_name().unwrap_or_default().to_string_lossy().into_owned();
            // The installed program: an AppImage's file, not its mount.
            let me = update::appimage().or_else(|| std::env::current_exe().ok()).unwrap_or_default();
            let (version, slug) = name.split_once('-').unwrap_or((&name, ""));
            let window = exe.starts_with(runtime::window_home(&dirs.id)).then(|| runtime::window_home(&dirs.id));
            let used = prune::Use {
                exe: me.to_string_lossy().into(),
                version: version.into(),
                slug: slug.into(),
                window: window.map(|w| w.to_string_lossy().into()),
                pending: false,
            };
            std::thread::spawn(move || match prune::record(&root, &used) {
                Ok(()) => _ = prune::auto(&root, &name, std::time::SystemTime::now()),
                Err(e) => eprintln!(
                    "[aio] runtime: could not record that this app uses Electron {} in {} ({e}): `ax prune` or \
                     `am prune` may then offer to delete it",
                    used.version,
                    root.display()
                ),
            });
        }
        // A full X server would show the window empty: said instead.
        #[cfg(target_os = "linux")]
        if let Some(why) = xroom::refuses(&|k| std::env::var(k).ok()) {
            return Err(std::io::Error::other(why));
        }
        let mut cmd = tokio::process::Command::new(exe);
        // no hijackers, no inherited AppImage, no aio settings (the sync key).
        aio::lifecycle::scrub(cmd.as_std_mut(), false);
        let mut child =
            cmd.arg(&dir)
                .args(&sandbox)
                .args(switches::extra(&|k| std::env::var(k).ok()))
                .arg(format!("--user-data-dir={}", data.join("profile").display()))
                // native on Wayland when that is the session; its app_id
                // comes from package.json's `desktopName` (Electron sets
                // CHROME_DESKTOP from it, over any env), like WM_CLASS on X11.
                .arg("--ozone-platform-hint=auto")
                .arg(format!("--aio-sock={sock}"))
                .arg(format!("--aio-ctl={ctl_sock}"))
                .arg(format!("--aio-id={}", dirs.id))
                .arg(format!("--aio-title={}", win.title))
                .arg(format!("--aio-width={}", win.width))
                .arg(format!("--aio-height={}", win.height))
                .arg(format!("--aio-gpu={gpu}"))
                // A dev build's keys: DevTools, a clean reload (guest.js).
                .args(cfg!(debug_assertions).then_some("--aio-dev=1"))
                // A contained run (AIORT_NO_OPEN): its links open nowhere (main.js).
                .args(std::env::var_os("AIORT_NO_OPEN").filter(|v| !v.is_empty()).map(|_| "--aio-no-open=1"))
                .args(win.background.map(|[l, d]| format!("--aio-bg={l}|{d}")))
                .args(icon.iter().flat_map(|(p, (w, h))| {
                    [format!("--aio-icon={}", p.display()), format!("--aio-icon-size={w}x{h}")]
                }))
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn()?;
        let pid = child.id();
        pid.into_iter().for_each(|p| only.arm(p));
        // Its stderr, Chromium's noise dropped, into the app's log.
        let said = child.stderr.take().map(|e| tokio::spawn(stderr::forward(e)));
        use tokio::io::AsyncWriteExt;
        // Listening from here: a parent that goes (`ax dev`, a test) stops us gracefully.
        let asked = aio::lifecycle::stop_asked()?;
        aio::lifecycle::watch_parent();
        let r = tokio::select! {
            // A runtime that ends badly (a crash loop) fails the app (exit 1).
            r = child.wait() => match r? {
                st if st.success() => Ok(()),
                st => {
                    // Its last words, once the pipe is drained.
                    let tail = match said {
                        Some(t) => tokio::time::timeout(QUIT_BUDGET, t).await.ok().and_then(Result::ok),
                        None => None,
                    };
                    let tail = tail.unwrap_or_default();
                    let gpu = if stderr::gpu_failed(&tail) && gpu != "software" {
                        _ = std::fs::write(&gpu_failed, "");
                        "; the GPU failed: the next start draws in software"
                    } else {
                        ""
                    };
                    let last = if tail.is_empty() { String::new() } else { format!(": {}", tail.join(" | ")) };
                    Err(std::io::Error::other(format!("the runtime exited with {st}{last}{gpu}")))
                }
            },
            _ = asked => {
                eprintln!("[aio] {}: asked to stop", dirs.id);
                STOP_ASKED.store(true, std::sync::atomic::Ordering::Relaxed);
                if let Some(c) = ctl.lock().await.as_mut() {
                    _ = c.write_all(b"quit\n").await;
                }
                if tokio::time::timeout(QUIT_BUDGET, child.wait()).await.is_err() {
                    _ = child.kill().await;
                }
                Ok(())
            }
        };
        pid.into_iter().for_each(|p| only.disarm(p));
        r
    });
    // aio's keepServer: the window is gone (closed, or crashed: said), the
    // app serves on until asked to stop. A sandbox it requires and lacks
    // (`require_sandbox`, `AIORT_SANDBOX=1`) stops it anyway.
    let keep = win.keep_server || instance::KEEP_SERVER.get() == Some(&true);
    let r = match r {
        Err(e) if !keep || e.kind() == std::io::ErrorKind::PermissionDenied => Err(e),
        _ if STOP_ASKED.load(std::sync::atomic::Ordering::Relaxed) || !keep => r,
        _ => {
            if let Err(e) = &r {
                eprintln!("[aio] {}: the window failed: {e}", dirs.id);
            }
            eprintln!("{}", kept_serving(&dirs.id, std::fs::read_to_string(dirs.data.join("serving.json")).ok()));
            instance::take_stops(&dirs.id, &dirs.data);
            aio::rt().block_on(async {
                aio::lifecycle::stop_asked()?.await;
                std::io::Result::Ok(())
            })
        }
    };
    // The window is gone: let the stores finish their effects.
    let aborted = aio::shutdown(QUIT_BUDGET);
    // Its doors are unlinked by now: their folder goes too.
    _ = std::fs::remove_dir(&socks);
    aio::health::stop();
    eprintln!(
        "[aio] {} stopped{}",
        dirs.id,
        if aborted.is_empty() { String::new() } else { format!(" (aborted: {})", aborted.join(", ")) }
    );
    aio::log::finish();
    r
}

/// What a kept server says once its window is gone (pure): where it
/// serves (its `serving.json`), and how to stop it.
fn kept_serving(id: &str, serving: Option<String>) -> String {
    let port = serving.and_then(|t| aio::serde_json::from_str::<aio::serde_json::Value>(&t).ok()?["port"].as_u64());
    let at = port.map_or("its local door".to_string(), |p| format!("port {p} and its local door"));
    format!(
        "[aio] {id}: the window closed; the app goes on serving on {at} (keep_server) until asked to stop: ax stop {id}"
    )
}

/// A server without a window: serve until asked to stop (SIGTERM, Ctrl-C,
/// `ax stop`), then stop as [`run`] does when its window closes: the
/// stores drained, no `health.json` left (`ax health`: not running).
pub fn serve(dirs: &aio::paths::AppDirs) -> std::io::Result<()> {
    if let Some(said) = said_contract() {
        return said;
    }
    // Inside the runtime: its signal handlers need it.
    aio::rt().block_on(async {
        let asked = aio::lifecycle::stop_asked()?;
        // A parent that goes (`ax dev`, a test) stops us gracefully.
        aio::lifecycle::watch_parent();
        asked.await;
        std::io::Result::Ok(())
    })?;
    let aborted = aio::shutdown(QUIT_BUDGET);
    _ = std::fs::remove_dir(aio::paths::sockets_at(&dirs.id, &dirs.data));
    aio::health::stop();
    eprintln!(
        "[aio] {} stopped{}",
        dirs.id,
        if aborted.is_empty() { String::new() } else { format!(" (aborted: {})", aborted.join(", ")) }
    );
    aio::log::finish();
    Ok(())
}

/// `[app] ui = "browser"` (aio's `client: "browser"`): the app's page in the
/// system browser, by a link with the key (it sets the page's cookie), then
/// served until asked to stop. A browser that can't be opened is said with
/// the address, never fatal: the server is up.
fn in_browser(dirs: &aio::paths::AppDirs) -> std::io::Result<()> {
    let serving = std::fs::read_to_string(dirs.data.join("serving.json")).ok();
    let serving = serving.and_then(|t| aio::serde_json::from_str(&t).ok()).unwrap_or_default();
    let url = browser_url(&serving, &aio::config::token(&dirs.data)?).ok_or_else(|| {
        std::io::Error::other(
            "[app] ui = \"browser\": the app serves nothing (open its cells with Source::for_app_with)",
        )
    })?;
    aio::rt().block_on(async {
        if let Err(e) = os::open_url(&url).await {
            let at = url.split_once("?token=").map_or(&*url, |(u, _)| u);
            eprintln!("[aio] {}: open {at} in a browser ({e})", dirs.id);
        }
    });
    serve(dirs)
}

/// The link that opens the served app's page with `key` (pure): this
/// machine's address for one that listens on every interface.
/// `None`: `serving` (its `serving.json`) names no port.
fn browser_url(serving: &aio::serde_json::Value, key: &str) -> Option<String> {
    let port = serving["port"].as_u64().filter(|p| *p > 0)?;
    let host = match serving["host"].as_str() {
        None | Some("" | "0.0.0.0") => "127.0.0.1".to_string(),
        Some("::") => "[::1]".into(),
        Some(h) if h.contains(':') => format!("[{h}]"),
        Some(h) => h.into(),
    };
    let scheme = if serving["tls"] == true { "https" } else { "http" };
    let safe = |b: u8| b.is_ascii_alphanumeric() || b"-._~".contains(&b);
    let key: String =
        key.bytes().map(|b| if safe(b) { char::from(b).to_string() } else { format!("%{b:02X}") }).collect();
    Some(format!("{scheme}://{host}:{port}/?token={key}"))
}

/// How the window draws, and who said: `AIORT_GPU`, else software where
/// the GPU failed before (`failed`), else the GPU.
fn gpu_choice(failed: bool, env: &dyn Fn(&str) -> Option<String>) -> (String, &'static str) {
    match env("AIORT_GPU").filter(|v| !v.is_empty()) {
        Some(g) => (g, "AIORT_GPU"),
        None if failed => ("software".into(), "detected: the GPU failed here before (AIORT_GPU=hardware tries it)"),
        None => ("hardware".into(), "default"),
    }
}

/// The runtime's rows of the boot report.
fn runtime_rows(
    rt: &Path,
    sandbox: &[String],
    require: bool,
    gpu_failed: bool,
    env: &dyn Fn(&str) -> Option<String>,
) -> Vec<aio::boot::Row> {
    let set = |k: &str| env(k).filter(|v| !v.is_empty());
    let row = |key, value: String, from| aio::boot::Row { key, value, from };
    vec![
        row(
            "runtime",
            rt.display().to_string(),
            if set("AIORT_RUNTIME").is_some() { "AIORT_RUNTIME" } else { "default" },
        ),
        {
            let (g, from) = gpu_choice(gpu_failed, env);
            row("gpu", g, from)
        },
        match (sandbox.is_empty(), set("AIORT_SANDBOX")) {
            (true, _) if require => row("sandbox", "strict".into(), "Window { require_sandbox }"),
            (true, Some(_)) => row("sandbox", "strict".into(), "AIORT_SANDBOX"),
            (true, None) => row("sandbox", "on".into(), "default"),
            (false, _) => row("sandbox", "off: the runtime's sandbox helper can't be used".into(), "detected"),
        },
    ]
}

/// The window's news: a hang or a crashed renderer degrades "window"
/// and is reported; a (re)loaded or responsive page clears it.
fn window_event(m: &aio::serde_json::Value) {
    let state = m["state"].as_str().unwrap_or("");
    match state {
        "loaded" | "responsive" => {
            static READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if state == "loaded" && !READY.swap(true, std::sync::atomic::Ordering::Relaxed) {
                eprintln!("[aio] window ready");
                // macOS: once the window is up, an app run from its disk image may move.
                #[cfg(target_os = "macos")]
                if let Some((title, data)) = ASK_MOVE.get() {
                    std::thread::spawn(move || macmove::run(title, data));
                }
                // A new version's first boot succeeded: the update stays.
                if let Some((id, version)) = instance::APP.get() {
                    update::confirm(id, version);
                }
            }
            aio::health::ok("window")
        }
        // On screen (main.js): the OS's splash may go once it has been up its minimum.
        "shown" => presplash::ready(splash::rect_of(&m["normal"])),
        "unresponsive" => {
            aio::report::problem("window:frozen", "the window is not responding");
            aio::health::degrade("window", "not responding");
        }
        "gone" => {
            let why = m["reason"].as_str().unwrap_or("unknown");
            let again = m["reloading"] == true;
            let next = if again { "reloading it" } else { "4 crashes in a minute: closing" };
            aio::report::problem("window:crashed", &format!("renderer gone: {why}; {next}"));
            aio::health::degrade("window", &format!("renderer gone: {why}"));
        }
        _ => {}
    }
}

/// The window's title and the app's data, for what asks once it is up.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
static ASK_MOVE: std::sync::OnceLock<(&'static str, std::path::PathBuf)> = std::sync::OnceLock::new();

/// The control line to Electron's main process, once it has connected.
pub(crate) type Ctl = Arc<tokio::sync::Mutex<Option<tokio::io::WriteHalf<aio::ipc::Stream>>>>;

/// This process's control line, for views asking the runtime (`page::print`).
static CTL: std::sync::OnceLock<Ctl> = std::sync::OnceLock::new();

/// Send the runtime one control line: false without a window (tests).
pub(crate) async fn send_runtime(line: String) -> bool {
    use tokio::io::AsyncWriteExt;
    let Some(ctl) = CTL.get() else { return false };
    match ctl.lock().await.as_mut() {
        Some(c) => c.write_all(format!("{line}\n").as_bytes()).await.is_ok(),
        None => false,
    }
}

/// [`send_runtime`] from sync code.
pub(crate) fn tell_runtime(line: String) {
    if let Ok(rt) = tokio::runtime::Handle::try_current() {
        rt.spawn(send_runtime(line));
    }
}

/// `AIORT_WINDOW_SIZE` (`ax dev --size`) as (width, height), when it is `WxH` (pure).
fn asked_size(v: Option<&str>) -> Option<(u32, u32)> {
    let (w, h) = v?.split_once('x')?;
    Some((w.parse().ok().filter(|n| *n > 0)?, h.parse().ok().filter(|n| *n > 0)?))
}

/// The window's title for this run: `AIORT_TITLE` (aio's `--title=`,
/// `ax dev --title`) when set, else the app's (pure).
fn titled(asked: Option<String>, app: &'static str) -> &'static str {
    asked.filter(|t| !t.trim().is_empty()).map_or(app, |t| t.leak())
}

/// What the runtime says on its control line: its screens (answered with
/// where to open), the window's bounds as they change (saved),
/// the answers to open dialogs (`page::pick`), clicks on a notice or the
/// tray (`desktop`), the window's health, a mouse's back key and the
/// DevTools protocol's answers and events (`inspect`).
async fn control(r: tokio::io::ReadHalf<aio::ipc::Stream>, ctl: Ctl, state: Option<std::path::PathBuf>, win: Window) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let mut lines = tokio::io::BufReader::new(r).lines();
    while let Ok(Some(l)) = lines.next_line().await {
        let Ok(m) = aio::serde_json::from_str::<aio::serde_json::Value>(&l) else { continue };
        match m["t"].as_str() {
            Some("screens") => {
                let a = place::answer(&m, win.width, win.height, state.as_deref().and_then(place::load).as_ref());
                if let Some(c) = ctl.lock().await.as_mut() {
                    _ = c.write_all(format!("{a}\n").as_bytes()).await;
                }
            }
            Some("bounds") => {
                if let Some(r) = splash::rect_of(&m["normal"]) {
                    presplash::moved(r);
                }
                if let Some(s) = &state
                    && let Err(e) = place::save(s, &m, win.width, win.height)
                {
                    eprintln!("[aio] window state not saved: {e}");
                }
            }
            Some("picked") => page::picked(&m),
            Some("opened" | "done") => guest::opened(&m),
            Some("clicked" | "tray") => desktop::asks(&m, win.tray.as_ref()).into_iter().for_each(desktop::deliver),
            Some("window") => window_event(&m),
            Some("cdp" | "cdp-event") => inspect::hub().heard(&m),
            // A mouse's back key: the views' Back handlers; untaken, nothing
            // (a desktop window has no default to leave by).
            Some("back") => _ = tokio::spawn(aio::ui::back_pressed()),
            _ => {}
        }
    }
}

/// Take later launches' hand-overs: each is delivered to the app
/// (`instance::launches`) and brings the window forward ("focus"), if
/// there is one; a stop asked is `stop()`.
pub(crate) async fn take_launches(mut listener: aio::ipc::Listener, ctl: Ctl, stop: fn()) {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
    while let Ok(s) = listener.accept().await {
        let (r, mut w) = tokio::io::split(s);
        let mut line = String::new();
        // A hand-over is a line of paths: 1 MiB is plenty, and the most held.
        if tokio::io::BufReader::new(r.take(1 << 20)).read_line(&mut line).await.is_err() {
            continue;
        }
        // `ax` stopping the app where there is no SIGTERM (Windows).
        if line.trim() == instance::QUIT {
            // Stop first: an "ok" says the stop is already under way.
            stop();
            _ = w.write_all(b"ok\n").await;
            continue;
        }
        let Some(launch) = instance::read_launch(&line) else {
            _ = w.write_all(b"bad launch\n").await;
            continue;
        };
        eprintln!("[aio] handed a launch: {:?}", launch.args);
        instance::deliver(launch);
        if let Some(c) = ctl.lock().await.as_mut() {
            _ = c.write_all(b"focus\n").await;
        }
        _ = w.write_all(b"ok\n").await;
    }
}

/// How long Electron, then the stores, get to finish at shutdown.
const QUIT_BUDGET: std::time::Duration = std::time::Duration::from_secs(5);

#[cfg(test)]
mod tests {
    #[test]
    fn a_window_panic_is_said_as_the_windows() {
        let said = super::window_panicked(&String::from("called `Result::unwrap()` on an `Err` value"));
        assert_eq!(
            said,
            "[aio-shell] window panicked: called `Result::unwrap()` on an `Err` value (it no longer updates; restart the app)"
        );
    }

    use super::*;
    use aio::page::{DIALOG, OVERLAY, RENDER, TYPING, render_probe};

    #[test]
    fn the_browser_opens_where_the_app_serves_with_its_key() {
        let s = |v: aio::serde_json::Value| v;
        let url = |v, k| browser_url(&s(v), k);
        let at = aio::serde_json::json!({ "port": 7420, "tls": false, "host": "0.0.0.0" });
        assert_eq!(url(at, "k+y/=").as_deref(), Some("http://127.0.0.1:7420/?token=k%2By%2F%3D"));
        let tls = aio::serde_json::json!({ "port": 443, "tls": true, "host": "fe80::1" });
        assert_eq!(url(tls, "k").as_deref(), Some("https://[fe80::1]:443/?token=k"));
        let named = aio::serde_json::json!({ "port": 8, "tls": false, "host": "box.local" });
        assert_eq!(url(named, "k").as_deref(), Some("http://box.local:8/?token=k"));
        assert_eq!(url(aio::serde_json::Value::Null, "k"), None, "serving nothing");
    }

    #[test]
    fn each_app_is_its_own_window_class() {
        // Electron takes WM_CLASS from the package name and Wayland's app_id
        // from the desktop name: both the app id.
        let d = tempfile::tempdir().unwrap();
        install(d.path(), "todo", &Window { title: "Todo", ..Default::default() }).unwrap();
        let pkg: aio::serde_json::Value =
            aio::serde_json::from_slice(&std::fs::read(d.path().join("package.json")).unwrap()).unwrap();
        assert_eq!((pkg["name"].as_str(), pkg["productName"].as_str()), (Some("todo"), Some("Todo")));
        assert_eq!(pkg["desktopName"], "todo.desktop");
        // Windows files the window, its taskbar entry and notices under the id.
        assert!(include_str!("main.js").contains("app.setAppUserModelId(arg('id'))"));
        // The app's own page opens a link only after real input, as a guest (guest.js popup).
        assert!(include_str!("main.js").contains("popups(w.webContents, true)"));
        // macOS: the keys a text field needs are menu items (aio tmplAppMenu).
        assert!(include_str!("main.js").contains("{ role: 'editMenu' }"));
        // aio's print fix, and Linux's lighter network service.
        assert!(include_str!("main.js").contains("appendSwitch('disable-features', 'CloudPrintEnable')"));
        assert!(include_str!("main.js").contains(
            "if (process.platform === 'linux') app.commandLine.appendSwitch('enable-features', 'NetworkServiceInProcess2')"
        ));
    }

    /// Two connected ends, in memory.
    fn pair() -> (aio::ipc::Stream, tokio::io::DuplexStream) {
        let (a, b) = tokio::io::duplex(1 << 16);
        (Box::new(a), b)
    }

    #[test]
    fn a_kept_server_says_where_it_serves_and_how_to_stop_it() {
        // aio electronClosedPlan with keepServer: "server still running <where>".
        let said = kept_serving("notes", Some(r#"{"port":7420,"host":"127.0.0.1"}"#.into()));
        assert_eq!(
            said,
            "[aio] notes: the window closed; the app goes on serving on port 7420 and its local door (keep_server) \
             until asked to stop: ax stop notes"
        );
        assert!(kept_serving("notes", None).contains("serving on its local door (keep_server)"));
        assert!(aio::paths::Ask::parse(&["--keep-server".into()]).unwrap().keep_server);
    }

    #[test]
    fn a_dev_size_is_w_by_h() {
        assert_eq!(asked_size(Some("390x844")), Some((390, 844)));
        assert_eq!([asked_size(None), asked_size(Some("390")), asked_size(Some("0x9"))], [None, None, None]);
        assert_eq!(titled(Some("Notes (QA)".into()), "Notes"), "Notes (QA)");
        assert_eq!([titled(None, "Notes"), titled(Some(" ".into()), "Notes")], ["Notes", "Notes"]);
    }

    #[tokio::test]
    async fn the_runtime_is_told_where_to_open_and_its_bounds_are_saved() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let d = tempfile::tempdir().unwrap();
        let state = d.path().join("window.json");
        let (ours, mut electron) = pair();
        let (r, w) = tokio::io::split(ours);
        let ctl: Ctl = Arc::new(tokio::sync::Mutex::new(Some(w)));
        let win = Window { title: "t", width: 1180, height: 780, ..Default::default() };
        tokio::spawn(control(r, ctl, Some(state.clone()), win));
        let screen = r#"{"x":0,"y":0,"width":1920,"height":1040}"#;
        let bounds = r#"{"t":"bounds","normal":{"x":5,"y":5,"width":900,"height":600},"maximized":true}"#;
        electron.write_all(format!("{bounds}\n").as_bytes()).await.unwrap();
        electron
            .write_all(format!(r#"{{"t":"screens","screens":[{screen}],"primary":{screen}}}"#).as_bytes())
            .await
            .unwrap();
        electron.write_all(b"\n").await.unwrap();
        let mut line = String::new();
        tokio::io::BufReader::new(&mut electron).read_line(&mut line).await.unwrap();
        let p: aio::serde_json::Value = aio::serde_json::from_str(&line).unwrap();
        // The saved rect, pulled 64 px inside the screen, maximized again.
        assert_eq!((p["x"].as_i64(), p["width"].as_i64(), p["maximized"].as_bool()), (Some(64), Some(900), Some(true)));
    }

    #[tokio::test]
    async fn clicks_on_the_tray_and_a_notice_reach_the_views() {
        use tokio::io::AsyncWriteExt;
        static MENU: [desktop::Item; 2] = [desktop::Item::SEPARATOR, desktop::Item::go("Library", "/ctl-lib")];
        let d = tempfile::tempdir().unwrap();
        let mut asks = desktop::channel().subscribe();
        let (ours, mut electron) = pair();
        let (r, w) = tokio::io::split(ours);
        let ctl: Ctl = Arc::new(tokio::sync::Mutex::new(Some(w)));
        let win = Window { tray: Some(desktop::Tray { menu: &MENU, ..Default::default() }), ..Default::default() };
        tokio::spawn(control(r, ctl, Some(d.path().join("window.json")), win));
        electron.write_all(b"{\"t\":\"tray\",\"i\":1}\n{\"t\":\"clicked\",\"route\":\"/ctl-n\"}\n").await.unwrap();
        let mut got = vec![];
        while got.len() < 2 {
            // Other tests' asks share the channel.
            if let desktop::Ask::Go(r) = asks.recv().await.unwrap()
                && r.starts_with("/ctl-")
            {
                got.push(r);
            }
        }
        assert_eq!(got, ["/ctl-lib", "/ctl-n"]);
    }

    #[tokio::test]
    async fn a_mouses_back_key_asks_the_views_back_handlers() {
        use tokio::io::AsyncWriteExt;
        assert!(include_str!("main.js").contains("if (cmd === 'browser-backward') tell({ t: 'back' })"));
        let d = tempfile::tempdir().unwrap();
        let mut views = aio::ui::back_presses();
        let (ours, mut electron) = pair();
        let (r, w) = tokio::io::split(ours);
        let ctl: Ctl = Arc::new(tokio::sync::Mutex::new(Some(w)));
        tokio::spawn(control(r, ctl, Some(d.path().join("window.json")), Window::default()));
        electron.write_all(b"{\"t\":\"back\"}\n").await.unwrap();
        views.recv().await.expect("Back asked").send(true).unwrap();
    }

    /// A syntax error in the shell is a window that never opens (aio tests
    /// the generated main parses).
    #[test]
    fn the_shell_parses() {
        let d = tempfile::tempdir().unwrap();
        install(d.path(), "x", &Window::default()).unwrap();
        let Ok(out) = std::process::Command::new("node").arg("--check").arg(d.path().join("main.js")).output() else {
            return eprintln!("skipped: no node");
        };
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(std::fs::read_to_string(d.path().join("tray.json")).unwrap(), "null", "no tray unless declared");
    }

    #[test]
    fn a_gpu_that_failed_here_draws_in_software_until_asked_again() {
        // detected on a crash, forced by AIORT_GPU.
        let none = |_: &str| None;
        assert_eq!(gpu_choice(false, &none), ("hardware".into(), "default"));
        assert_eq!(gpu_choice(true, &none).0, "software");
        let hw = |k: &str| (k == "AIORT_GPU").then(|| "hardware".to_string());
        assert_eq!(gpu_choice(true, &hw), ("hardware".into(), "AIORT_GPU"));
        let rows = runtime_rows(Path::new("/rt"), &[], false, true, &none);
        assert!(rows.iter().any(|r| r.key == "gpu" && r.value == "software" && r.from.contains("failed here")));
    }

    #[test]
    fn the_shell_logs_every_renderer_failure() {
        // aio tests/electron-renderer-log.test.ts: each hook writes a line.
        let js = include_str!("main.js");
        for hook in
            ["'console-message'", "'render-process-gone'", "'preload-error'", "'unresponsive'", "'did-fail-load'"]
        {
            assert!(js.contains(&format!("on({hook}")), "{hook}");
        }
        // A page line names where it came from, file and line.
        assert!(js.contains("${String(e.sourceId).slice(0, 512).replace(/\\s/g, '%20')}${line}"));
        assert!(js.contains("`:${e.lineNumber}`"));
    }

    #[tokio::test]
    async fn a_later_launch_hands_over_its_file_and_brings_the_window_forward() {
        use tokio::io::AsyncReadExt;
        let d = tempfile::tempdir().unwrap();
        let sock = aio::ipc::addr(d.path(), &format!("hand{}", std::process::id()));
        let (to_electron, mut electron) = pair();
        let ctl: Ctl = Arc::new(tokio::sync::Mutex::new(Some(tokio::io::split(to_electron).1)));
        let mut got = instance::launches();
        tokio::spawn(take_launches(aio::ipc::Listener::bind(&sock).unwrap(), ctl, || {}));
        let (s, cwd) = (sock.clone(), d.path().to_path_buf());
        tokio::task::spawn_blocking(move || instance::hand_over(&s, &cwd, &["notes.md".into()]))
            .await
            .unwrap()
            .unwrap();
        let launch = got.recv().await.unwrap();
        assert_eq!(launch.args, ["notes.md"]);
        assert_eq!(launch.path("notes.md"), d.path().join("notes.md").to_string_lossy());
        let mut b = [0u8; 6];
        electron.read_exact(&mut b).await.unwrap();
        assert_eq!(&b, b"focus\n");
    }

    #[test]
    fn the_page_has_no_network_and_the_wire_comes_first() {
        let p = page(&Window { title: "t", ..Default::default() });
        assert!(p.contains("default-src 'none'"));
        assert!(p.contains("connect-src 'none'") && !p.contains("unsafe-eval"), "{p}");
        assert!(p.find("window.WebSocket = class").unwrap() < p.find("aio://wire").unwrap());
        assert!(p.contains("[autofocus]") && p.contains("data-select"));
        assert!(
            p.find("aio://wire").unwrap() < p.find("const sent = new WeakMap()").unwrap(),
            "typing wraps a live interpreter"
        );
        assert!(p.contains(DIALOG));
    }

    /// The dialog script on a small DOM, in Node (aio tests/ui-modal-focus):
    /// into the dialog on open, Tab and Shift+Tab wrap, an inner focus is
    /// kept, and the opener gets focus back once it closes.
    #[test]
    fn a_modal_dialog_holds_focus_and_gives_it_back() {
        const DOM: &str = r#"
          class El {
            constructor(tag, attrs = {}, kids = []) {
              Object.assign(this, { tag, attrs, kids, parent: null, on: {}, nodeType: 1 });
              for (const k of kids) k.parent = this;
            }
            get isConnected() { let n = this; while (n.parent) n = n.parent; return n === root; }
            getAttribute(n) { return n in this.attrs ? this.attrs[n] : null; }
            contains(o) { for (let n = o; n; n = n.parent) if (n === this) return true; return false; }
            all() { return this.kids.flatMap((k) => [k, ...k.all()]); }
            querySelectorAll(sel) {
              const t = (e) => (['a', 'button', 'input'].includes(e.tag) && !('disabled' in e.attrs)) || 'tabindex' in e.attrs;
              return this.all().filter(sel === '[aria-modal="true"]' ? (e) => e.attrs['aria-modal'] === 'true' : t);
            }
            closest() { return null; }
            addEventListener(t, f) { (this.on[t] ||= []).push(f); }
            focus() {
              const relatedTarget = document.activeElement;
              document.activeElement = this;
              for (let n = this; n; n = n.parent) for (const f of n.on.focusin || []) f({ target: this, relatedTarget });
            }
            add(k) { k.parent = this; this.kids.push(k); }
            drop(k) { k.parent = null; this.kids = this.kids.filter((x) => x !== k); }
          }
          const root = new El('html'), body = new El('body'), main = new El('div');
          root.add(body); body.add(main);
          let observed = null, keys = [];
          globalThis.MutationObserver = class { constructor(f) { observed = f; } observe() {} };
          globalThis.document = { body, activeElement: body, getElementById: () => main, addEventListener: (_, f) => keys.push(f) };
          const tab = (shiftKey) => {
            const e = { key: 'Tab', shiftKey, defaultPrevented: false, preventDefault() { this.defaultPrevented = true; } };
            for (const f of keys) f(e);
            return e.defaultPrevented;
          };
        "#;
        const RUN: &str = r#"
          const name = () => document.activeElement.attrs.t;
          const opener = new El('button', { t: 'opener' });
          main.add(opener);
          opener.focus();
          const [a, skip, b] = [new El('input', { t: 'a' }), new El('button', { t: 'skip', tabindex: '-1' }), new El('button', { t: 'b' })];
          const box = new El('div', { t: 'box', role: 'dialog', 'aria-modal': 'true', tabindex: '-1' }, [a, skip, b]);
          const backdrop = new El('div', { t: 'backdrop' }, [new El('span', { tabindex: '0' }), box, new El('span', { tabindex: '0' })]);
          main.add(backdrop);
          observed([{ addedNodes: [backdrop], removedNodes: [] }]);
          const out = [];
          box.focus(); out.push(name());
          out.push(tab(true) && name());
          out.push(tab(false) && name());
          out.push(tab(false) ? 'moved' : 'the browser moves it');
          b.focus(); box.focus(); out.push(name());
          main.drop(backdrop);
          observed([{ addedNodes: [], removedNodes: [backdrop] }]);
          document.activeElement = body;
          const empty = new El('div', { t: 'empty', 'aria-modal': 'true', tabindex: '-1' });
          queueMicrotask(() => {
            out.push(name());
            main.add(empty);
            observed([{ addedNodes: [empty], removedNodes: [] }]);
            out.push(tab(false) && name());
            console.log(JSON.stringify(out));
          });
        "#;
        let js = format!("{DOM}\n{DIALOG}\n{RUN}");
        let Ok(out) = std::process::Command::new("node").args(["-e", &js]).output() else {
            return eprintln!("skipped: no node");
        };
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let got: Vec<String> = aio::serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(got, ["a", "b", "a", "the browser moves it", "b", "opener", "empty"]);
    }

    #[test]
    fn the_page_says_its_language_direction_and_viewport_and_escapes_them() {
        // aio tests/html-shell.test.ts, headless-page-title-escaped.test.ts.
        let p = page(&Window { title: "<script>x</script> & co", ..Default::default() });
        assert!(p.contains(r#"<html lang="en"><head>"#), "en, left to right: no dir");
        assert!(p.contains("<title>&lt;script&gt;x&lt;/script&gt; &amp; co</title>"));
        assert!(p.contains(r#"<meta name="viewport" content="width=device-width, initial-scale=1">"#));
        let p = page(&Window { lang: Some("ar"), dir: Dir::Rtl, viewport: Some("width=1280"), ..Default::default() });
        assert!(p.contains(r#"<html lang="ar" dir="rtl">"#));
        assert!(p.contains(r#"<meta name="viewport" content="width=1280">"#));
        assert!(
            page(&Window { dir: Dir::Auto, lang: Some(" "), ..Default::default() })
                .contains(r#"<html lang="en" dir="auto">"#)
        );
        assert!(!page(&Window { viewport: Some(""), ..Default::default() }).contains(r#"name="viewport""#));
        let p = page(&Window { lang: Some(r#""><script>x"#), ..Default::default() });
        assert!(p.contains(r#"lang="&quot;&gt;&lt;script&gt;x""#), "a hostile value stays in its attribute");
    }

    #[test]
    fn every_window_wears_the_apps_icon_or_its_monogram_and_a_browser_tab_too() {
        let png: &'static [u8] = b"\x89PNG\r\n\x1a\n";
        let mut cfg = aio::config::Config::default();
        cfg.app.name = "Notes".into();
        assert_eq!(Window::of(&cfg).icon, None, "none of its own: the shell draws its monogram");
        cfg.app.icon = png;
        assert_eq!(Window::of(&cfg).icon, Some(png));
        // aio's /__aio/icon: a browser tab's favicon; the window's page needs none.
        let win = Window::of(&cfg);
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(png);
        let link = format!(r#"<link rel="icon" type="image/png" href="data:image/png;base64,{b64}">"#);
        assert!(page_of(&win, "", "'self'").contains(&link));
        assert!(!page(&win).contains("rel=\"icon\""));
        let tab = page_of(&Window { title: "Notes", ..Default::default() }, "", "'self'");
        assert!(
            tab.contains(r#"<link rel="icon" type="image/png" href="data:image/png;base64,iVBORw0KGgo"#),
            "else its monogram"
        );
        // The window wears the monogram, drawn once while its letter and hue stay.
        let d = tempfile::tempdir().unwrap();
        let p = monogram(d.path(), "Notes", 210).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), aio_config::icon::icon_png("Notes", 210, MONOGRAM_PX));
        std::fs::write(&p, b"kept").unwrap();
        assert_eq!(std::fs::read(monogram(d.path(), "Notes", 210).unwrap()).unwrap(), b"kept");
        assert_ne!(std::fs::read(monogram(d.path(), "Notes", 20).unwrap()).unwrap(), b"kept", "a new hue: drawn again");
    }

    #[test]
    fn the_apps_own_head_is_kept_verbatim_after_the_policy() {
        // aio tests/html-shell.test.ts `ui.head`.
        let head = r#"<meta name="theme-color" content="navy"><style>body{font:16px serif}</style>"#;
        let p = page(&Window { title: "t", head: Some(head), ..Default::default() });
        let at = p.find(head).expect("verbatim");
        assert!(p.find("Content-Security-Policy").unwrap() < at && at < p.find("</head>").unwrap());
    }

    /// aio tests/dev-overlay.test.ts, in Node: a page error is on the page,
    /// once with a count, bounded, behind a badge; a release page has none.
    #[test]
    fn a_dev_page_shows_its_errors_on_the_page() {
        assert!(overlay(false).is_empty(), "a release page carries no overlay");
        assert!(page(&Window::default()).contains("data-aio-dev"), "a dev build's page does");
        let js = format!(
            "const on = {{}};
             globalThis.window = {{ addEventListener: (k, f) => on[k] = f }};
             const el = (tag) => ({{ tag, kids: [], style: {{}}, attrs: {{}}, textContent: '',
               setAttribute(k, v) {{ this.attrs[k] = v; }},
               appendChild(c) {{ this.kids.push(c); }},
               set textContent(t) {{ this._t = t; if (t === '') this.kids = []; }}, get textContent() {{ return this._t; }} }});
             globalThis.document = {{ body: el('body'), createElement: el }};
             {OVERLAY}
             for (let i = 0; i < 3; i++) on.error({{ message: 'boom', filename: 'aio://app/', lineno: 1 }});
             on.unhandledrejection({{ reason: 'nope' }});
             for (let i = 0; i < 30; i++) window.__aioDiag('e' + i);
             const root = document.body.kids[0], [list, badge] = root.kids;
             console.log(JSON.stringify([document.body.kids.length, list.kids.length, list.kids[0].textContent,
               badge.textContent, list.style.cssText.includes('display:none'), root.style.cssText.includes('pointer-events:none')]));
             window.__aioDiag('build failed', 'error[E0609]', true);
             const failed = badge.textContent;
             window.__aioDiagDrop('build failed');
             console.log(JSON.stringify([failed, badge.textContent, list.kids.length]));"
        );
        let Ok(out) = std::process::Command::new("node").args(["-e", &js]).output() else {
            return eprintln!("skipped: no node");
        };
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let out = String::from_utf8_lossy(&out.stdout);
        let mut lines = out.lines().map(|l| aio::serde_json::from_str::<aio::serde_json::Value>(l).unwrap());
        // 32 errors, 20 rows (the oldest went), behind a closed badge, never over the page.
        assert_eq!(lines.next(), Some(aio::serde_json::json!([1, 20, "e10", "34 page errors", true, true])));
        // A fixed build takes its failure away, the page's own errors stay.
        assert_eq!(lines.next(), Some(aio::serde_json::json!(["35 page errors", "34 page errors", 19])));
    }

    /// aio tests/render-meter: each paint graded by `[vitals] render`; a step
    /// up to warning or frozen said once, the way back as recovered; a hidden
    /// window is never slow.
    #[test]
    fn a_slow_paint_is_graded_said_on_each_step_and_a_hidden_window_never_is() {
        let run = |v: &aio::vitals::Vitals, paints: &str| {
            let js = format!(
                "let now = 0, raf = [], said = [], vis;
                 globalThis.performance = {{ now: () => now }};
                 globalThis.requestAnimationFrame = (f) => raf.push(f);
                 console.warn = (m) => said.push('W ' + m);
                 console.info = (m) => said.push('I ' + m);
                 globalThis.document = {{ hidden: false, addEventListener: (_, f) => vis = f }};
                 globalThis.window = {{}};
                 {}{RENDER}
                 const frame = (paintAt) => {{ window.__aioPainted(now); now += paintAt; raf.shift()(); }};
                 {paints}
                 window.__aioPainted(now); document.hidden = true; now += 9000; document.hidden = false; vis(); raf.shift()();
                 document.hidden = true; for (let i = 0; i < 100; i++) window.__aioPainted(now);
                 const hidden = raf.length; document.hidden = false;
                 for (let i = 0; i < 3; i++) {{ window.__aioPainted(now); now += 100; }}
                 const asked = raf.length; raf.shift()(); window.__aioPainted(now);
                 said.push('frames ' + [hidden, asked, raf.length]);
                 console.log(JSON.stringify(said));",
                render_probe(v)
            );
            let out = std::process::Command::new("node").args(["-e", &js]).output().ok()?;
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
            Some(aio::serde_json::from_slice::<Vec<String>>(&out.stdout).unwrap())
        };
        // aio's defaults: degraded 50, warning 200, frozen 2000 ms.
        let v = aio::vitals::Vitals::default();
        let Some(said) = run(&v, "frame(16); frame(120); frame(250); frame(400); frame(2500); frame(16); frame(120);")
        else {
            return eprintln!("skipped: no node");
        };
        let hint = " · likely: a large DOM update or a script in the page held it (DevTools: Performance)";
        assert_eq!(
            said,
            [
                format!("W [vitals] render warning: an update took 250 ms to paint{hint}"),
                format!("W [vitals] render frozen: an update took 2500 ms to paint{hint}"),
                "I [vitals] render recovered: an update painted in 16 ms".to_string(),
                // Hidden: no frame asked (they'd pile up until it shows);
                // three updates in one frame: one, timed from the first.
                "W [vitals] render warning: an update took 300 ms to paint · likely: a large DOM update or a script in the page held it (DevTools: Performance)".to_string(),
                "frames 0,1,1".to_string(),
            ]
        );
        let t = aio::vitals::Tier { degraded: 10, warning: 20, frozen: 30 };
        let own = aio::vitals::Vitals { hints: false, render: t, ..v };
        let said = run(&own, "frame(25);").unwrap();
        assert_eq!(said[0], "W [vitals] render warning: an update took 25 ms to paint");
        // The runtime keeps the probe's lines, at their level.
        let js = include_str!("main.js");
        assert!(js.contains("const word = { error: 'ERROR', warning: 'WARN ', debug: 'DEBUG' }[e.level] || 'INFO '"));
    }

    /// The race on a phone, replayed in Node: keys typed ahead of their echoes stay.
    #[test]
    fn typing_ahead_of_the_apps_echo_keeps_every_key() {
        let js = format!(
            "let onInput; const applied = [];
             globalThis.document = {{ addEventListener: (_, f) => onInput = f }};
             globalThis.window = {{ interpreter: {{ setAttributeInner(n, f, v) {{ applied.push(v); if (f === 'value') n.value = v; }} }} }};
             {TYPING}
             const it = window.interpreter, el = {{ value: '' }};
             const type = (v) => {{ el.value = v; onInput({{ target: el }}); }};
             type('a'); type('ab'); type('abc');
             it.setAttributeInner(el, 'value', 'a');    // a stale echo: skipped
             it.setAttributeInner(el, 'value', 'ab');   // stale too
             it.setAttributeInner(el, 'value', 'abc');  // caught up: nothing to do
             it.setAttributeInner(el, 'value', '');     // the app clears it: applied
             type('x');
             it.setAttributeInner(el, 'value', '2.5');  // the app's own value: applied
             it.setAttributeInner(el, 'class', 'x');    // other attributes: as ever
             console.log(JSON.stringify([el.value, applied]));"
        );
        let Ok(out) = std::process::Command::new("node").args(["-e", &js]).output() else {
            return eprintln!("skipped: no node");
        };
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), r#"["2.5",["","2.5","x"]]"#);
    }

    #[tokio::test]
    async fn frames_round_trip_over_the_socket() {
        let (a, b) = pair();
        let mut a = wire(a);
        let mut b = Framed::new(b, LengthDelimitedCodec::new());
        a.send(b"edits".to_vec()).await.unwrap();
        assert_eq!(&b.next().await.unwrap().unwrap()[..], b"edits");
        b.send(Bytes::from_static(b"{\"event\":1}")).await.unwrap();
        assert_eq!(a.next().await.unwrap().unwrap(), b"{\"event\":1}");
    }

    #[test]
    fn built_in_assets_are_laid_out_at_their_paths_and_rewritten_only_when_changed() {
        let d = tempfile::tempdir().unwrap();
        assets(d.path(), &[("/media/a%20b.txt", b"one"), ("/media/x/y.txt", b"two")]).unwrap();
        assert_eq!(std::fs::read(d.path().join("assets/media/a b.txt")).unwrap(), b"one");
        assert_eq!(std::fs::read(d.path().join("assets/media/x/y.txt")).unwrap(), b"two");
        // The same files: nothing written (a file changed by hand stays).
        std::fs::write(d.path().join("assets/media/x/y.txt"), "hand").unwrap();
        assets(d.path(), &[("/media/a%20b.txt", b"one"), ("/media/x/y.txt", b"two")]).unwrap();
        assert_eq!(std::fs::read(d.path().join("assets/media/x/y.txt")).unwrap(), b"hand");
        // Another build: its files only.
        assets(d.path(), &[("/media/a%20b.txt", b"new")]).unwrap();
        assert_eq!(std::fs::read(d.path().join("assets/media/a b.txt")).unwrap(), b"new");
        assert!(!d.path().join("assets/media/x").exists());
    }

    #[test]
    fn install_is_private() {
        let d = std::env::temp_dir().join(format!("aio-shell-test-{}", std::process::id()));
        install(&d, "my-app", &Window { title: "T \"x\"", ..Default::default() }).unwrap();
        let pkg = std::fs::read_to_string(d.join("package.json")).unwrap();
        let pkg: aio::serde_json::Value = aio::serde_json::from_str(&pkg).unwrap();
        assert_eq!((pkg["name"].as_str(), pkg["productName"].as_str()), (Some("my-app"), Some("T \"x\"")));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&d).unwrap().permissions().mode() & 0o777, 0o700);
        }
        assert!(d.join("preload.js").exists() && d.join("index.html").exists());
        std::fs::remove_dir_all(&d).unwrap();
    }
}

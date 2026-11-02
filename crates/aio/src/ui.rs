//! UI helpers every app gets from aio-rs (aio: `aio/ui`, the page shell, the
//! client), and with feature `kit` the view API apps are written against:
//! [`prelude`] (`use aio::ui::prelude::*;`). Its names are renderer-neutral
//! (docs/design/air.md): an app names no renderer.

/// The page side of reading and driving a page as a person meets it, by
/// TestUi's rules (roles, names, auto names, handles, paths): `ax surface`
/// and `ax trigger` in a live window, `aio::testing::TestUi` in its test
/// page. `(SURFACE_JS)(op, arg)` answers `{ ok }` or `{ error }`.
pub const SURFACE_JS: &str = include_str!("surface.js");

/// Key chords (`"mod+s"`), parsed and matched: the kit's bindings and `TestUi`'s keys.
#[cfg(any(feature = "kit", feature = "air"))]
#[doc(hidden)]
pub mod chord;
#[cfg(feature = "kit")]
mod view;
#[cfg(feature = "kit")]
pub use view::*;
#[cfg(all(feature = "air", not(feature = "kit")))]
mod css;
#[cfg(all(feature = "air", not(feature = "kit")))]
pub use css::{collect_css, css};
#[cfg(all(feature = "air", not(feature = "kit")))]
mod on_air;
#[cfg(all(feature = "air", not(feature = "kit")))]
pub use on_air::*;
/// What a view returns (with `kit`, [`prelude`] has the rest).
#[cfg(all(feature = "liveview", not(any(feature = "kit", feature = "air"))))]
pub type View = dioxus_core::Element;

/// Whether the views are air's (the facade's renderer), for a host that
/// must match it (aio-shell on air).
#[doc(hidden)]
pub const ON_AIR: bool = cfg!(all(feature = "air", not(feature = "kit")));

#[cfg(all(feature = "air", feature = "liveview", not(feature = "kit")))]
compile_error!(
    "aio-rs: one renderer per build: `air` with Dioxus's page (`liveview`, aio-shell's default features); \
     an app on air takes aio-shell with `default-features = false, features = [\"air\"]`"
);

/// Everything a view file needs: `use aio::ui::prelude::*;`. `rsx!`,
/// `#[component]`, [`View`], state ([`Signal`], [`signal`], [`memo`],
/// [`effect`], [`watch`], [`once`], [`on_cleanup`], [`Global`]), context
/// ([`provide`], [`context`]), async work ([`spawn`], [`spawn_forever`],
/// [`task`], [`resource`]), boundaries ([`ErrorBoundary`], [`Suspense`]),
/// handlers ([`EventHandler`], [`Callback`]), the event types
/// (`KeyboardEvent`, `MouseEvent`, `FormEvent`, …) with [`Key`](prelude::Key),
/// [`Modifiers`](prelude::Modifiers), and the page ([`eval`]).
#[cfg(feature = "kit")]
pub mod prelude {
    #[doc(hidden)]
    pub use super::view::__rsx::*;
    pub use super::view::{
        Attribute, Callback, ErrorBoundary, Eval, EvalError, EventHandler, Fragment, Global, GlobalMemo, Memo, Props,
        Resource, Signal, Suspense, Task, View, ViewError, callback, component, context, effect, eval, in_views,
        key_event, memo, on_cleanup, once, provide, provide_root, resource, rsx, signal, spawn, spawn_forever, task,
        try_context, watch,
    };
    pub use crate::{act, use_css};
    #[doc(no_inline)]
    pub use dioxus::html::{
        GlobalAttributesExtension, SvgAttributesExtension, events::*, extensions::*, geometry, global_attributes,
        input_data::MouseButton, svg_attributes, traits::*,
    };
    pub use dioxus::prelude::{Code, Event, Key, Location, Modifiers};
}

/// Everything a view file needs, on air: the same names as on Dioxus.
#[cfg(all(feature = "air", not(feature = "kit")))]
pub mod prelude {
    pub use super::on_air::events::*;
    pub use super::on_air::{
        Attribute, Callback, Code, Component, ErrorBoundary, ErrorBoundaryProps, Eval, EvalError, EventHandler,
        Fragment, FragmentProps, Global, GlobalMemo, Key, Location, Memo, Modifiers, MouseButton, Prop, Props,
        Resource, Signal, Suspense, SuspenseProps, Task, View, ViewError, callback, component, context, effect, eval,
        geometry, in_views, key_event, memo, on_cleanup, once, provide, provide_root, resource, rsx, signal, spawn,
        spawn_forever, task, try_context, watch,
    };
    pub use crate::{act, use_css};
    /// `rsx!`'s paths start at `air`: an app needs no dependency of its own.
    #[doc(hidden)]
    pub use ::air;
}

/// The views' clock: a moment and a wait, on the page's timer in a
/// browser, else on tokio's (a test's paused clock too).
#[cfg(any(feature = "kit", feature = "air"))]
pub(crate) mod clock {
    pub(crate) async fn sleep(d: std::time::Duration) {
        #[cfg(target_arch = "wasm32")]
        crate::web::sleep(u32::try_from(d.as_millis()).unwrap_or(u32::MAX)).await;
        #[cfg(not(target_arch = "wasm32"))]
        tokio::time::sleep(d).await;
    }

    /// Tokio's: a UI test's `advance` moves it.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) use tokio::time::Instant;

    /// A moment, on the page's clock: a browser has no `std` one.
    #[cfg(target_arch = "wasm32")]
    #[derive(Clone, Copy)]
    pub(crate) struct Instant(f64);

    #[cfg(target_arch = "wasm32")]
    impl Instant {
        pub(crate) fn now() -> Self {
            Self(js_sys::Date::now())
        }
    }

    #[cfg(target_arch = "wasm32")]
    impl std::ops::Sub for Instant {
        type Output = std::time::Duration;

        fn sub(self, earlier: Self) -> Self::Output {
            std::time::Duration::from_secs_f64((self.0 - earlier.0).max(0.0) / 1000.0)
        }
    }
}

/// A copyable day or id for props and handlers (a `String` must be cloned into
/// every closure). Stand-in for aio-rs's `Copy` id/date handles: each distinct
/// string is kept once for the app's life.
pub type Sym = &'static str;

pub fn sym(s: &str) -> Sym {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    static TABLE: OnceLock<Mutex<HashSet<Sym>>> = OnceLock::new();
    let mut t = TABLE.get_or_init(Default::default).lock().expect("sym table");
    match t.get(s) {
        Some(&k) => k,
        None => {
            let k: Sym = Box::leak(s.to_owned().into_boxed_str());
            t.insert(k);
            k
        }
    }
}

/// Class names joined, the false ones left out (aio's `cx`).
pub fn cx(parts: &[(&str, bool)]) -> String {
    parts.iter().filter(|(_, on)| *on).map(|(c, _)| *c).collect::<Vec<_>>().join(" ")
}

/// A kit button: `aio-btn aio-btn--{variant} aio-btn--md`, styled by the kit's
/// theme ([`crate::kit::theme_css`], first in [`use_css!`](crate::use_css)).
pub fn btn(variant: &str) -> String {
    format!("aio-btn aio-btn--{variant} aio-btn--md")
}

/// A kit input: `aio-input`, invalid-aware.
pub fn input_cls(extra: &str, invalid: bool) -> String {
    cx(&[("aio-input", true), ("aio-input--invalid", invalid), (extra, !extra.is_empty())])
}

/// A fresh id for a new record.
pub fn new_id() -> String {
    // A page has no system clock: its ids are time-ordered UUIDs.
    #[cfg(target_arch = "wasm32")]
    let n = crate::uuid_v7().as_u128();
    #[cfg(not(target_arch = "wasm32"))]
    let n = {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos())
    };
    format!("{n:x}")
}

/// An id unique in the process (`aio-1`, `aio-2`…) for `for`/`aria-*`
/// wiring; CSS-safe, unlike aio's `:r0:`. Stable per component instance as a
/// hook: `let id = use_hook(aio::ui::unique_id);` (aio's `useId`).
pub fn unique_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!("aio-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

/// The ARIA role an element's tag implies (`button`, `link`, `textbox`…;
/// `generic`: none); `ty` is its `type`, `href` whether it has one.
pub fn implied_role(tag: &str, ty: &str, href: bool) -> &'static str {
    match (tag, ty) {
        ("button", _) | ("input", "button" | "submit" | "reset") => "button",
        ("a", _) if href => "link",
        ("input", "checkbox") => "checkbox",
        ("input", "radio") => "radio",
        ("input", "range") => "slider",
        ("input", _) | ("textarea", _) => "textbox",
        ("select", _) => "combobox",
        ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", _) => "heading",
        ("img", _) => "img",
        ("dialog", _) => "dialog",
        _ => "generic",
    }
}

/// A label as one word of up to five, `"buy milk!"` → `BuyMilk` (letters,
/// marks and digits of any script; at most 32 characters).
fn pascal(s: &str) -> String {
    s.split(|c: char| !c.is_alphanumeric() && !is_mark(c))
        .filter(|w| !w.is_empty())
        .take(5)
        .map(|w| {
            let mut c = w.chars();
            c.next().map_or(String::new(), |f| f.to_uppercase().chain(c).collect())
        })
        .collect::<String>()
        .chars()
        .take(32)
        .collect()
}

/// A combining mark (Thai, Devanagari and Arabic vowels are marks).
fn is_mark(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x0483..=0x0489 | 0x0591..=0x05C7 | 0x0610..=0x061A | 0x064B..=0x065F
        | 0x0670 | 0x06D6..=0x06ED | 0x0900..=0x0903 | 0x093A..=0x094F | 0x0951..=0x0957 | 0x0962..=0x0963
        | 0x0981..=0x0983 | 0x09BC..=0x09D7 | 0x0E31 | 0x0E34..=0x0E3A | 0x0E47..=0x0E4E | 0x1AB0..=0x1AFF
        | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
}

/// An element's auto name before numbering (aio auto names, as `TestUi`
/// and `ax surface` give them): its accessible `name` then its role word,
/// `<button>Save</button>` → `SaveButton`; `None` for one with no role a
/// person acts on or reads (a plain `div`). A row, item or form is named by
/// its own text.
pub fn auto_name(tag: &str, role: &str, name: &str) -> Option<String> {
    let word = match (role, tag) {
        ("generic", "tr") => "Row".into(),
        ("generic", "li") => "Item".into(),
        ("generic", "form") => "Form".into(),
        ("generic", _) => return None,
        ("textbox", _) => "Input".into(),
        ("combobox", _) => "Select".into(),
        (r, _) => pascal(r),
    };
    // A long text is content, not a name.
    let label = if name.chars().count() <= 60 { pascal(name) } else { String::new() };
    Some(if label.ends_with(&word) { label } else { label + &word })
}

/// Auto names in document order, numbered: the same again is `SaveButton2`,
/// `SaveButton3`.
pub fn numbered(bases: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashMap::<String, usize>::new();
    bases
        .into_iter()
        .map(|b| {
            let n = seen.entry(b.clone()).or_default();
            *n += 1;
            if *n == 1 { b } else { format!("{b}{n}") }
        })
        .collect()
}

/// The page CSS: aio's base rules (server-html-gen), the app's stylesheets
/// unchanged, and shims. Dioxus native can't set attributes on `<html>`,
/// so the theme lives on the app root: `:root[data-theme]` maps to it and the
/// root repeats body's colours (which would read the light tokens). Tokens
/// set on `:root` are set on that root too, so the ones made from others
/// (`--aio-tint` from the accent) are made from the app's, not inherited
/// ready-made. It mounts into `<main id="main">`, which aio styles as a
/// centred page, so that box is reset to aio's full-window `#root`.
/// `scheme` decides `light-dark()` and `prefers-color-scheme`: the OS's as
/// it changes ([`Scheme::System`], what [`scheme`] gives by default), or
/// the app's own choice (a `bool`: `true` dark; a phone in dark mode then
/// never darkens half of a light app). Under `[ui] theme = "none"` ([`set_look`])
/// even aio's base rules are left out: the app brings every rule. The last
/// page CSS is kept for [`page_sheet`].
pub fn page_css(sheets: &[&str], scheme: impl Into<Scheme>) -> String {
    let css = sheets_css(sheets, scheme.into(), !BARE.load(std::sync::atomic::Ordering::Relaxed));
    *PAGE.write().unwrap_or_else(|e| e.into_inner()) = css.clone();
    css
}

/// A stylesheet file of the app: its path and the text it had when built.
pub type Sheet = (&'static str, &'static str);

/// What [`use_css!`](crate::use_css) styles the page with: in a release
/// build the sheets as built; in a dev build each file as it is now on disk,
/// so a saved edit shows without a compile (aio's live CSS), in a browser as
/// `ax dev --web` serves it. A file that is gone keeps its built text.
pub fn sheet_texts(sheets: &[Sheet]) -> Vec<String> {
    sheets
        .iter()
        .map(|(path, built)| {
            #[cfg(all(debug_assertions, target_arch = "wasm32"))]
            if let Some(t) = LIVE.with_borrow(|l| l.get(path).cloned()) {
                return t;
            }
            match cfg!(debug_assertions) && cfg!(not(target_arch = "wasm32")) {
                true => std::fs::read_to_string(path).unwrap_or_else(|_| built.to_string()),
                false => built.to_string(),
            }
        })
        .collect()
}

#[cfg(all(debug_assertions, target_arch = "wasm32"))]
thread_local! {
    /// A dev page's sheets as `ax dev --web` last served them, by path.
    static LIVE: std::cell::RefCell<std::collections::BTreeMap<&'static str, String>> = Default::default();
}

/// Waits until one of `sheets` changes on disk (a dev build; it polls every
/// 20 ms, a few file stats, so a saved edit shows within 0.05 s; a browser asks `ax dev
/// --web` every 250 ms, and with no dev server stops asking). A release
/// build never returns: its sheets are built in.
pub async fn sheets_changed(sheets: &[Sheet]) {
    #[cfg(all(debug_assertions, not(target_arch = "wasm32")))]
    {
        let stamp =
            || sheets.iter().map(|(p, _)| std::fs::metadata(p).and_then(|m| m.modified()).ok()).collect::<Vec<_>>();
        let was = stamp();
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            if stamp() != was {
                return;
            }
        }
    }
    #[cfg(all(debug_assertions, target_arch = "wasm32"))]
    'asking: loop {
        crate::web::sleep(250).await;
        for (path, built) in sheets {
            let q: String = path
                .bytes()
                .map(|b| match b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
                    true => char::from(b).to_string(),
                    false => format!("%{b:02X}"),
                })
                .collect();
            let Ok((status, v)) = crate::web::fetch(&format!("/aio-dev/sheet?path={q}"), None, None).await else {
                continue;
            };
            if status == 404 {
                break 'asking;
            }
            let (Some(text), now) = (v["text"].as_str(), sheet_texts(&[(path, built)]).remove(0)) else { continue };
            if text != now {
                LIVE.with_borrow_mut(|l| l.insert(path, text.to_string()));
                return;
            }
        }
    }
    #[allow(unreachable_code)]
    {
        _ = sheets;
        std::future::pending::<()>().await
    }
}

/// [`use_css!`](crate::use_css)'s subscription to its sheets' edits (a dev
/// build's): the render (Dioxus) or the hole (air) that reads them runs
/// again when one is saved.
#[doc(hidden)]
#[cfg(feature = "kit")]
pub fn follow_sheets(sheets: &'static [Sheet]) {
    let edits = signal(|| 0u64);
    once(move || {
        if cfg!(debug_assertions) {
            spawn(async move {
                loop {
                    sheets_changed(sheets).await;
                    edits.update(|n| *n += 1);
                }
            });
        }
    });
    let _ = edits.get();
}

/// On air the call sits in a hole (an effect, no hooks): one signal per set
/// of sheets for the thread; its watcher lives as long as the app (a root
/// mounted again, a test's next one, starts it again).
#[doc(hidden)]
#[cfg(all(feature = "air", not(feature = "kit")))]
pub fn follow_sheets(sheets: &'static [Sheet]) {
    type Watch = (air::Signal<u64>, air::Task);
    if !cfg!(debug_assertions) {
        return;
    }
    thread_local! {
        static EDITS: std::cell::RefCell<std::collections::HashMap<Vec<&'static str>, Watch>> = Default::default();
    }
    let watch = |edits: air::Signal<u64>| {
        air::spawn_forever(async move {
            loop {
                sheets_changed(sheets).await;
                edits.update(|n| *n += 1);
            }
        })
    };
    let key = sheets.iter().map(|s| s.0).collect();
    let edits = EDITS.with_borrow_mut(|m| {
        let w = m.entry(key).or_insert_with(|| {
            air::detached(|| {
                let edits = air::Signal::new(0u64);
                (edits, watch(edits))
            })
        });
        if !w.1.alive() {
            w.1 = watch(w.0);
        }
        w.0
    });
    let _ = edits.get();
}

/// The page's CSS from the app's stylesheet files, as [`page_css`] makes it,
/// live while you edit: `style { {aio::use_css!(dark; "assets/style.css")} }`.
/// `dark` (a `bool`) is the app's theme, not the OS's: `true` applies the
/// sheets' `prefers-color-scheme: dark` rules and `light-dark()`'s dark side,
/// `false` the light ones. Sheets made in code go before the semicolon and
/// come first; the kit's classes (`aio::ui::btn`, the kit's components) are
/// drawn only with its theme in:
/// `aio::use_css!(dark, &aio::kit::theme_css(hue); "assets/style.css")`.
/// Paths are from the package's folder. A dev build re-reads a saved file and
/// redraws within 0.05 s, no compile; a release build has the files built in.
/// Unlike CSS kept in `use_hook` (made once, so stale after an edit), it is
/// made again on every render.
#[macro_export]
macro_rules! use_css {
    ($dark:expr $(, $made:expr)*; $($path:literal),+ $(,)?) => {{
        // A release build reads no file: the build machine's path stays out of it.
        const SHEETS: &[$crate::ui::Sheet] = &[$((
            if cfg!(debug_assertions) { concat!(env!("CARGO_MANIFEST_DIR"), "/", $path) } else { $path },
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/", $path)),
        )),+];
        $crate::ui::follow_sheets(SHEETS);
        let texts = $crate::ui::sheet_texts(SHEETS);
        let files = texts.iter().map(String::as_str);
        $crate::ui::page_css(&[$(::core::convert::AsRef::<str>::as_ref($made)),*].into_iter().chain(files).collect::<Vec<&str>>(), $dark)
    }};
}

/// [`page_css`] without the globals: `base` keeps aio's base rules.
fn sheets_css(sheets: &[&str], scheme: Scheme, base: bool) -> String {
    let base: &[&str] = if base { &["*,*::before,*::after{box-sizing:border-box}body{margin:0}"] } else { &[] };
    let css = [base, sheets].concat().join("\n");
    let css = css
        .replace(":root[data-theme", "[data-theme")
        .replace(":root{", ":root,[data-theme]{")
        .replace(":root {", ":root,[data-theme] {");
    let css = match scheme {
        // The OS's: the sheets as written, the page following `prefers-color-scheme`.
        Scheme::System => css + "\n:root{color-scheme:light dark}",
        fixed => {
            let dark = fixed == Scheme::Dark;
            let media = if dark { "@media all" } else { "@media not all" };
            let css = css
                .replace("@media (prefers-color-scheme: dark)", media)
                .replace("@media (prefers-color-scheme:dark)", media);
            light_dark(&css, dark)
        }
    };
    css + "\n#main{max-width:none;margin:0;padding:0;height:100%;overflow:hidden}"
        + "\n[data-theme]{background:var(--aio-bg);color:var(--aio-text)}"
}

pub use aio_config::config::Scheme;

/// The app's `[ui] scheme` (what `aio::boot::begin` took; `System` before
/// it, and in tests): the template's `use_css!(aio::ui::scheme(), …)`.
pub fn scheme() -> Scheme {
    match SCHEME.load(std::sync::atomic::Ordering::Relaxed) {
        1 => Scheme::Light,
        2 => Scheme::Dark,
        _ => Scheme::System,
    }
}

/// Take the app's `[ui] scheme` for [`scheme`] (`aio::boot::begin` does it).
pub fn set_scheme(s: Scheme) {
    let n = match s {
        Scheme::System => 0,
        Scheme::Light => 1,
        Scheme::Dark => 2,
    };
    SCHEME.store(n, std::sync::atomic::Ordering::Relaxed);
}

static SCHEME: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Is the page dark now (a hook): the app's `[ui] scheme`, or under
/// `system` the OS's `prefers-color-scheme`, followed as it changes (a
/// canvas or a chart that draws its own colours reads it). The page's CSS
/// follows the OS without it ([`page_css`] with [`Scheme::System`]).
#[cfg(any(feature = "kit", feature = "air"))]
pub fn use_dark() -> Memo<bool> {
    let fixed = scheme();
    let os = signal(|| false);
    once(move || {
        if fixed == Scheme::System {
            spawn(async move {
                let mut page = eval(
                    "const m = matchMedia('(prefers-color-scheme: dark)'); aio.send(m.matches);
                     m.addEventListener('change', (e) => aio.send(e.matches)); await new Promise(() => {});",
                );
                while let Ok(dark) = page.recv::<bool>().await {
                    os.set(dark);
                }
            });
        }
    });
    memo(move || match fixed {
        Scheme::System => os.get(),
        s => s == Scheme::Dark,
    })
}

/// Take the app's `[ui] theme` for [`page_css`] (`aio::boot::begin` does it).
#[cfg(not(target_arch = "wasm32"))]
pub fn set_look(look: crate::config::Look) {
    BARE.store(look == crate::config::Look::None, std::sync::atomic::Ordering::Relaxed);
}

static BARE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

static PAGE: std::sync::RwLock<String> = std::sync::RwLock::new(String::new());

/// The CSS [`page_css`] last made ("" before it ran): what styles the page.
pub fn page_sheet() -> String {
    PAGE.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Every `light-dark(a, b)` in `css` replaced by `a`, or `b` when `dark`.
pub fn light_dark(css: &str, dark: bool) -> String {
    const F: &str = "light-dark(";
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(at) = rest.find(F) {
        out.push_str(&rest[..at]);
        let args = &rest[at + F.len()..];
        // The top-level comma and the closing paren, skipping nested calls.
        let (mut depth, mut comma, mut end) = (0, None, args.len());
        for (i, c) in args.char_indices() {
            match c {
                '(' => depth += 1,
                ')' if depth == 0 => {
                    end = i;
                    break;
                }
                ')' => depth -= 1,
                ',' if depth == 0 => comma = comma.or(Some(i)),
                _ => {}
            }
        }
        let comma = comma.unwrap_or(end);
        let pick = if dark { args.get(comma + 1..end).unwrap_or("") } else { &args[..comma] };
        out.push_str(pick.trim());
        rest = args.get(end + 1..).unwrap_or("");
    }
    out + rest
}

/// A local image as the page can load it (the shell serves image files
/// only, from `aio://app/file/<path>`, on Android from the page's own
/// origin, `https://app.invalid/file/<path>`; `file://` is blocked).
pub fn local_file_url(abs: &str) -> String {
    let mut out = String::from(if cfg!(target_os = "android") { "https://app.invalid/file" } else { "aio://app/file" });
    for b in abs.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// A Back press waiting for the views' answer: did a handler take it?
pub type BackAsk = tokio::sync::oneshot::Sender<bool>;

/// Where Back presses go: the views listening, oldest first.
static BACK: std::sync::Mutex<Vec<tokio::sync::mpsc::UnboundedSender<BackAsk>>> = std::sync::Mutex::new(Vec::new());

/// The Android Back button (aio's `onBackButton`): the views' handlers are
/// asked, the latest first; `false` when none took it (then the host does
/// its default: the page's history, then leave). Asked now, answered once
/// the views ran; with no views listening, at once `false`. Each listener
/// ([`back_presses`]) is asked in turn, the latest first, until one takes it;
/// the first is asked at once.
pub fn back_pressed() -> impl std::future::Future<Output = bool> {
    let views: Vec<_> = {
        let mut all = BACK.lock().unwrap_or_else(|e| e.into_inner());
        all.retain(|v| !v.is_closed());
        all.iter().rev().cloned().collect()
    };
    let ask = |v: &tokio::sync::mpsc::UnboundedSender<BackAsk>| {
        let (tx, rx) = tokio::sync::oneshot::channel();
        _ = v.send(tx);
        rx
    };
    let first = views.first().map(ask);
    async move {
        if let Some(rx) = first
            && rx.await.unwrap_or(false)
        {
            return true;
        }
        for v in views.iter().skip(1) {
            if ask(v).await.unwrap_or(false) {
                return true;
            }
        }
        false
    }
}

/// The Back presses from now on, for the views (`aio::kit::use_back_button`,
/// the `router!`): asked before those who listened earlier.
pub fn back_presses() -> tokio::sync::mpsc::UnboundedReceiver<BackAsk> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    BACK.lock().unwrap_or_else(|e| e.into_inner()).push(tx);
    rx
}

/// The links the app is opened with (Android's deep links: `[build.android]
/// schemes`): who listens, and those that came before anyone did.
type Links = (Vec<tokio::sync::mpsc::UnboundedSender<String>>, Vec<String>);
static LINKS: std::sync::Mutex<Links> = std::sync::Mutex::new((Vec::new(), Vec::new()));

/// The app was opened with `url` (`myapp://users/7`), by the host: every
/// listener ([`links`]) gets it; with none yet (a cold start), the first to
/// listen does. Any app on the phone can send one: it is data, never code.
pub fn open_link(url: &str) {
    let mut l = LINKS.lock().unwrap_or_else(|e| e.into_inner());
    l.0.retain(|v| !v.is_closed());
    if l.0.is_empty() {
        l.1.push(url.to_string());
    }
    for v in &l.0 {
        _ = v.send(url.to_string());
    }
}

/// The links the app is opened with from now on, and those it was opened
/// with before anyone listened (the `router!` listens: it goes there,
/// [`crate::route::of_link`]).
pub fn links() -> tokio::sync::mpsc::UnboundedReceiver<String> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut l = LINKS.lock().unwrap_or_else(|e| e.into_inner());
    for url in l.1.drain(..) {
        _ = tx.send(url);
    }
    l.0.push(tx);
    rx
}

/// Did a Back handler take it: each asked from the last (innermost) one
/// until one does. One that panics took it (the app stays; the panic is logged).
pub fn back_taken(handlers: &[&dyn Fn() -> bool]) -> bool {
    handlers.iter().rev().any(|h| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(h)).unwrap_or_else(|_| {
            crate::error!(cat: "ui", "a Back handler panicked; Back is taken as handled");
            true
        })
    })
}

/// A browser tab's place in the app (`aio_shell::web`), a root context of
/// its view: who it is, and its own copies of the cells it shows. In a tab,
/// [`crate::client!`] reads only these: a cell the tab has no copy of is an empty
/// one, never the process's (which holds every user's data).
#[derive(Clone, Default)]
pub struct Tab {
    /// Who it is signed in as; `None`: the key's holder, or nobody yet.
    pub user: Option<String>,
    sources: std::collections::HashMap<std::any::TypeId, std::sync::Arc<dyn std::any::Any + Send + Sync>>,
}

impl Tab {
    pub fn new(user: Option<String>) -> Self {
        Self { user, sources: Default::default() }
    }

    /// With `s` as its copy of `T`.
    pub fn with<T: crate::Cell>(mut self, s: crate::Source<T>) -> Self {
        self.sources.insert(std::any::TypeId::of::<T>(), std::sync::Arc::new(s));
        self
    }

    /// Its copy of `T`, if it has one.
    pub fn source<T: crate::Cell>(&self) -> Option<crate::Source<T>> {
        self.sources.get(&std::any::TypeId::of::<T>())?.downcast_ref::<crate::Source<T>>().cloned()
    }
}

/// How often commits made off the UI thread (prices, balances, effects,
/// schedules) re-render the views at most: aio's broadcast coalescer. A call
/// from the views renders at once, and with it whatever came meanwhile (aio
/// `flushAllUrgent`).
pub const BACKGROUND: std::time::Duration = std::time::Duration::from_millis(150);

/// A part of a cell's state, from `client!`'s `part(|c| &c.field)`: a
/// `Copy` handle (when its picker captures nothing) whose reads subscribe
/// only to the top-level field the picker points into (aio's
/// `trackCell`), so a view re-renders when that field changes, not on every
/// commit. Read it with `.get()`, `.with(|v| …)` or `"{part}"`; it reads
/// the state the views were last told of.
#[cfg(any(feature = "kit", feature = "air"))]
pub struct Part<T: 'static, R: ?Sized, F> {
    f: F,
    parts: &'static Global<Parts<T>>,
    whole: &'static Global<u64>,
    cell: &'static str,
    at: &'static std::panic::Location<'static>,
    _r: std::marker::PhantomData<fn(&T) -> &R>,
}

#[cfg(any(feature = "kit", feature = "air"))]
impl<T: crate::CellData, R: ?Sized, F: Fn(&T) -> &R> Part<T, R, F> {
    #[doc(hidden)]
    pub fn new(
        f: F,
        parts: &'static Global<Parts<T>>,
        whole: &'static Global<u64>,
        cell: &'static str,
        at: &'static std::panic::Location<'static>,
    ) -> Self {
        Self { f, parts, whole, cell, at, _r: std::marker::PhantomData }
    }

    /// The state the views were told of and the field's stamp, the reader
    /// subscribed to that field when `track` (to every commit when the
    /// picker points outside the fields: a dev build says so).
    fn told(&self, track: bool) -> (std::sync::Arc<T>, u64) {
        let (st, sig) = {
            let p = self.parts.read_untracked();
            let st = p.shown();
            p.find((self.f)(&st))
        };
        let ver = match (sig, track) {
            (Some(s), true) => s.get(),
            (Some(s), false) => s.get_untracked(),
            (None, true) => {
                if cfg!(debug_assertions) {
                    whole_read(self.cell, self.at, true);
                }
                self.whole.get()
            }
            (None, false) => self.whole.get_untracked(),
        };
        (st, ver)
    }

    /// `g` of the part; subscribes.
    pub fn with<O>(&self, g: impl FnOnce(&R) -> O) -> O {
        g((self.f)(&self.told(true).0))
    }

    pub fn with_untracked<O>(&self, g: impl FnOnce(&R) -> O) -> O {
        g((self.f)(&self.told(false).0))
    }

    /// The part, owned (a `str` part gives a `String`); subscribes.
    pub fn get(&self) -> R::Owned
    where
        R: ToOwned,
    {
        self.with(R::to_owned)
    }

    pub fn get_untracked(&self) -> R::Owned
    where
        R: ToOwned,
    {
        self.with_untracked(R::to_owned)
    }

    /// The field's stamp: it moves exactly when the field changes, so it is
    /// a cache key for what is made of it ([`crate::Derived`],
    /// [`crate::Cache`]; aio's array identity), never a deep compare. Subscribes.
    pub fn ver(&self) -> u64 {
        self.told(true).1
    }
}

#[cfg(any(feature = "kit", feature = "air"))]
impl<T, R: ?Sized, F: Clone> Clone for Part<T, R, F> {
    fn clone(&self) -> Self {
        Self { f: self.f.clone(), ..*self }
    }
}

#[cfg(any(feature = "kit", feature = "air"))]
impl<T, R: ?Sized, F: Copy> Copy for Part<T, R, F> {}

#[cfg(any(feature = "kit", feature = "air"))]
impl<T: crate::CellData, R: ?Sized + std::fmt::Display, F: Fn(&T) -> &R> std::fmt::Display for Part<T, R, F> {
    /// Subscribes: `"{n}"` in a view follows the part.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.with(|v| v.fmt(f))
    }
}

#[cfg(any(feature = "kit", feature = "air"))]
impl<T: crate::CellData, R: ?Sized + std::fmt::Debug, F: Fn(&T) -> &R> std::fmt::Debug for Part<T, R, F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.with(|v| v.fmt(f))
    }
}

/// The views' side of a cell's fields (`client!`): the state they were
/// last told of, one signal per field (aio's signal per cell) and when.
#[doc(hidden)]
#[cfg(any(feature = "kit", feature = "air"))]
pub struct Parts<T> {
    shown: std::sync::Arc<T>,
    sigs: Vec<Signal<u64>>,
    /// When commits off the UI thread were last told; `None`: a call from
    /// the views closed the window since, so the next one goes at once.
    told: Option<clock::Instant>,
}

#[cfg(any(feature = "kit", feature = "air"))]
impl<T: crate::CellData> Parts<T> {
    /// In the root scope (a global's constructor runs there).
    pub fn new(shown: std::sync::Arc<T>) -> Self {
        let sigs = (0..T::FIELDS.len().max(1)).map(|_| Signal::new(0)).collect();
        Self { shown, sigs, told: None }
    }

    /// The state the views were told of, and the signal of the field `at`
    /// points into (`None`: not into the state itself, e.g. into a list's
    /// heap: then the reader follows the whole state).
    pub fn find<R: ?Sized>(&self, at: &R) -> (std::sync::Arc<T>, Option<Signal<u64>>) {
        let base = std::sync::Arc::as_ptr(&self.shown) as *const u8 as usize;
        let p = at as *const R as *const u8 as usize;
        let field = p
            .checked_sub(base)
            .filter(|o| *o < std::mem::size_of::<T>())
            .and_then(|o| crate::Field::holding(T::FIELDS, o));
        (self.shown.clone(), field.map(|i| self.sigs[i]))
    }

    /// The state the views were told of.
    pub fn shown(&self) -> std::sync::Arc<T> {
        self.shown.clone()
    }

    /// The views are told of `now`: the signals of the fields that changed
    /// since the state they were last told of (to bump), and whether any did.
    /// `urgent`: for a call from the views, which closes the background
    /// window (aio's `flushUrgent`: the next commit off the UI thread goes at once).
    pub fn tell(&mut self, now: std::sync::Arc<T>, urgent: bool) -> (Vec<Signal<u64>>, bool) {
        if urgent {
            self.told = None;
        }
        if !self.behind(&now) {
            return (vec![], false);
        }
        if !urgent {
            self.told = Some(clock::Instant::now());
        }
        let mask = match T::FIELDS.is_empty() {
            true => !0,
            false => now.changed_fields(&self.shown),
        };
        self.shown = now;
        let bump = self.sigs.iter().enumerate().filter(|(i, _)| mask & (1u64 << (*i).min(63)) != 0).map(|(_, s)| *s);
        (bump.collect(), mask != 0)
    }

    /// Is `now` a state the views haven't been told of?
    pub fn behind(&self, now: &std::sync::Arc<T>) -> bool {
        !std::sync::Arc::ptr_eq(&self.shown, now)
    }

    /// How long a commit made off the UI thread waits before the views are
    /// told: [`BACKGROUND`] after such a commit last was (none since a call
    /// from the views: at once).
    pub fn wait(&self) -> std::time::Duration {
        self.told.map_or(std::time::Duration::ZERO, |t| BACKGROUND.saturating_sub(clock::Instant::now() - t))
    }
}

/// [`BACKGROUND`]'s wait (`client!`'s `use_live`): while it lasts, the views
/// have a commit still to be told of, and a UI test's `settle` waits for it.
#[doc(hidden)]
#[cfg(any(feature = "kit", feature = "air"))]
pub async fn sleep(d: std::time::Duration) {
    struct Waiting;
    impl Drop for Waiting {
        fn drop(&mut self) {
            WAITING.set(WAITING.get() - 1);
        }
    }
    WAITING.set(WAITING.get() + 1);
    let _w = Waiting;
    clock::sleep(d).await
}

#[cfg(any(feature = "kit", feature = "air"))]
thread_local!(static WAITING: std::cell::Cell<usize> = const { std::cell::Cell::new(0) });

/// Has a view on this thread a commit still to be told of ([`sleep`])?
#[cfg(all(any(feature = "kit", feature = "air"), feature = "testing"))]
pub(crate) fn telling() -> bool {
    WAITING.get() > 0
}

/// The client side of a cell for Dioxus views (feature `kit`): `client!(Todo, todo,
/// todo_at)` makes `SOURCE` (set by `main`:
/// `SOURCE.set(aio::Source::for_app(&cfg, &dirs)?)`; unset = memory, for
/// tests; `DB`, a store's path instead, is the older way), the
/// global `HANDLE`, `part(|p| &p.field)` (a view reads a field and
/// re-renders only when it changes: aio's signal per cell, the way to read
/// in a render), a `todo()` reader (the whole state: it subscribes the
/// view to every commit), a `todo_at(|p| …)` hook (a value made of the
/// state, compared after each commit), `use_status()` (online, syncing) and
/// `use_pending(method)` hooks, `use_time_travel()` with `time_travel(ask)`
/// (the cell's history, for dev tools), and the `Ui` door that `act!` calls
/// through: a view's call renders at once. Global like aio's module signals:
/// one per app instance (so per test). Call `use_live()` once in the root
/// component so commits made off the UI thread (effects, schedules)
/// re-render too, at most once per [`BACKGROUND`].
#[macro_export]
macro_rules! client {
    ($cell:ty, $read:ident $(, $at:ident)?) => {
        pub static DB: ::std::sync::OnceLock<::std::path::PathBuf> = ::std::sync::OnceLock::new();

        /// Set by `main` to use another source (a synced copy); else the
        /// store at `DB`; with neither (tests), a fresh memory store per app instance.
        pub static SOURCE: ::std::sync::OnceLock<$crate::Source<$cell>> = ::std::sync::OnceLock::new();

        // Dioxus keys a global by its source location, and every global this
        // macro makes has the same one: each gets a name instead (friction #29).
        pub static HANDLE: $crate::ui::Global<$crate::Source<$cell>> =
            $crate::ui::Global::named(
                || match (SOURCE.get(), DB.get()) {
                    // A browser tab's own copy ([`$crate::ui::Tab`]): its
                    // user's view, never the process's; none: an empty one.
                    _ if let Some(t) = $crate::ui::try_context::<$crate::ui::Tab>() => t
                        .source::<$cell>()
                        .unwrap_or_else(|| $crate::Source::Local($crate::Handle::memory())),
                    (Some(s), _) => s.clone(),
                    // One store per process, shared by all windows (one writer per log).
                    (None, Some(path)) => {
                        SOURCE.get_or_init(|| $crate::Source::open(path).expect("open state")).clone()
                    }
                    (None, None) => $crate::Source::Local($crate::Handle::memory()),
                },
                concat!(module_path!(), "::HANDLE"),
            );

        /// Bumped on every change of the state; reading it subscribes a view.
        static VERSION: $crate::ui::Global<u64> = $crate::ui::Global::named(|| 0, concat!(module_path!(), "::VERSION"));

        /// Bumped whenever the views are told anything (the copy's status,
        /// calls in flight): `use_status` and `use_pending` follow it.
        static TICK: $crate::ui::Global<u64> = $crate::ui::Global::named(|| 0, concat!(module_path!(), "::TICK"));

        /// The state the views were last told of, a signal per field.
        static PARTS: $crate::ui::Global<$crate::ui::Parts<$cell>> =
            $crate::ui::Global::named(
                || $crate::ui::Parts::new(HANDLE.read_untracked().state()),
                concat!(module_path!(), "::PARTS"),
            );

        /// Tell the views of the state now: the readers of each field that
        /// changed re-render, the whole state's readers if any did.
        fn tell(urgent: bool) {
            let now = HANDLE.read_untracked().state();
            let (bump, changed) = PARTS.update(|p| p.tell(now, urgent));
            for s in bump {
                s.update(|n| *n += 1);
            }
            if changed {
                VERSION.update(|n| *n += 1);
            }
            TICK.update(|n| *n += 1);
        }

        /// A part of the state for this view (aio reading a cell:
        /// `nav.activeIndex[p]` is `part(|w| &w.nav).with(|n| n.active_index[p])`):
        /// a handle whose reads re-render the view only when the top-level
        /// field `f` points into changes, not on every commit. Pick the
        /// field, then index it: a reference into a list's own heap
        /// (`&w.rows[3]`) can't name its field, so that reader follows every
        /// commit (a dev build says so). Not a hook: call it anywhere.
        #[track_caller]
        pub fn part<R: ?Sized + 'static, F: Fn(&$cell) -> &R>(f: F) -> $crate::ui::Part<$cell, R, F> {
            $crate::ui::Part::new(f, &PARTS, &VERSION, stringify!($cell), ::std::panic::Location::caller())
        }

        /// The current state (views render from it, handlers call methods).
        /// Read in a render it re-renders the view on every commit: a dev
        /// build says so once per place (`hint`), naming the reader of parts.
        #[track_caller]
        pub fn $read() -> ::std::sync::Arc<$cell> {
            if cfg!(debug_assertions) {
                $crate::ui::whole_read(stringify!($cell), ::std::panic::Location::caller(), false);
            }
            let _ = (VERSION.get(), PARTS.read_untracked());
            HANDLE.read_untracked().state()
        }

        $(
        /// A part of the state for this view (AIR's path subscriptions): `f`
        /// runs after each commit, and the view re-renders only when its
        /// answer changed. A hook: call it at the top of a component.
        pub fn $at<R: PartialEq + 'static>(f: impl Fn(&$cell) -> R + 'static) -> $crate::ui::Memo<R> {
            $crate::ui::memo(move || f(&$read()))
        }
        )?

        /// How the copy stands (online, calls waiting, backed up; aio
        /// `useConnected`), re-rendering the view when that changes. A hook;
        /// needs `use_live()`. No `ready` flag: the state is there before the
        /// first render (a synced copy starts from the server's or the saved one).
        pub fn use_status() -> $crate::ui::Memo<$crate::Status> {
            $crate::ui::memo(|| {
                let _ = TICK.get();
                HANDLE.read_untracked().status()
            })
        }

        /// Calls of `method` in flight (`None`: all), for a spinner on its
        /// button: on a synced copy, those waiting for the server (aio
        /// `cell.method.pending()`); on a local cell, its async effects
        /// running or queued. A hook; needs `use_live()`.
        pub fn use_pending(method: Option<&'static str>) -> $crate::ui::Memo<usize> {
            $crate::ui::memo(move || {
                let _ = TICK.get();
                HANDLE.read_untracked().pending(method)
            })
        }

        /// The cell's history and where it is (aio's `useTimeTravel`), re-read
        /// after each commit and step; `None` on a synced copy (its history
        /// is the server's). A hook; needs `use_live()`. Feeds
        /// `aio::kit::TimeTravelPanel`. A page's cells keep no history.
        #[cfg(not(target_arch = "wasm32"))]
        pub fn use_time_travel() -> $crate::ui::Memo<Option<$crate::Timeline>> {
            $crate::ui::memo(|| {
                let _ = TICK.get();
                HANDLE.read_untracked().travel(None).ok()
            })
        }

        /// Take a time-travel step (undo, redo, goto, pause, resume); where
        /// the cell is then. A step pauses the cell: calls are refused
        /// until `TimeAsk::Resume`.
        #[cfg(not(target_arch = "wasm32"))]
        pub fn time_travel(ask: $crate::TimeAsk) -> $crate::Result<$crate::Timeline> {
            let r = HANDLE.read_untracked().travel(Some(ask));
            tell(true);
            r
        }

        pub fn use_live() {
            let tx = $crate::ui::once(|| {
                let _ = PARTS.read_untracked();
                let (tx, mut rx) = $crate::__tokio::sync::mpsc::unbounded_channel::<()>();
                let (first, busy) = (tx.clone(), tx.clone());
                HANDLE.read_untracked().listen(move |_| _ = first.send(()));
                HANDLE.read_untracked().on_pending(move || _ = busy.send(()));
                // Off the UI thread's commits: at most one render per
                // BACKGROUND (a call from the views tells them at once).
                $crate::ui::spawn(async move {
                    while rx.recv().await.is_some() {
                        while rx.try_recv().is_ok() {}
                        // The views' own calls were told at once; calls in
                        // flight (no new state) are told now.
                        if PARTS.read_untracked().behind(&HANDLE.read_untracked().state()) {
                            let wait = PARTS.read_untracked().wait();
                            if !wait.is_zero() {
                                $crate::ui::sleep(wait).await;
                            }
                            while rx.try_recv().is_ok() {}
                        }
                        tell(false);
                    }
                });
                tx
            });
            // A source put in `HANDLE` later (a test's seed mid-way) is
            // listened to as well, and every view re-reads it.
            let seen = $crate::ui::once(|| ::std::rc::Rc::new(::std::cell::Cell::new(false)));
            $crate::ui::effect(move || {
                let h = HANDLE.get();
                if seen.replace(true) {
                    let (first, busy) = (tx.clone(), tx.clone());
                    h.listen(move |_| _ = first.send(()));
                    h.on_pending(move || _ = busy.send(()));
                    tell(true);
                }
            });
        }

        /// The views' door to the cell.
        #[derive(Clone, Copy)]
        pub struct Ui;

        impl $crate::Door<$cell> for Ui {
            fn call(&self, method: &str, args: $crate::serde_json::Value) -> $crate::Result<$crate::serde_json::Value> {
                // The state before the call is the one the views know of.
                let _ = PARTS.read_untracked();
                let h = HANDLE.get_untracked();
                let r = $crate::Door::call(&h, method, args);
                // At once, with what came off the UI thread meanwhile (aio
                // `flushAllUrgent`); a call that changed nothing (Down on the
                // last row) re-renders nothing.
                tell(true);
                r
            }

            fn ask(&self, method: &str, args: $crate::serde_json::Value) -> $crate::Result<$crate::Answer> {
                let _ = PARTS.read_untracked();
                let h = HANDLE.get_untracked();
                let r = $crate::Door::ask(&h, method, args);
                tell(true);
                r
            }

            fn read<R>(&self, f: impl FnOnce(&$cell) -> R) -> R {
                // In a render it follows every commit, as `$read()` (no
                // hint: a flow reads through its door too).
                let _ = VERSION.get();
                f(&HANDLE.read_untracked().state())
            }

            fn own(&self, key: &str, res: Option<Box<dyn ::std::any::Any + Send>>) {
                HANDLE.read_untracked().own(key, res)
            }
        }
    };
}

/// A dev build's hint (`client!`'s whole-state reader): `at` read all of
/// `cell` in a component's render, so the component re-renders on every
/// commit (a cursor step, a price tick), whatever it shows. Said once per
/// place; `heap`: through a `part` that points outside the state's fields.
#[doc(hidden)]
#[cfg(any(feature = "kit", feature = "air"))]
pub fn whole_read(cell: &str, at: &'static std::panic::Location<'static>, heap: bool) {
    thread_local! {
        static SEEN: std::cell::RefCell<std::collections::HashSet<(&'static str, u32, u32)>> = Default::default();
    }
    let key = (at.file(), at.line(), at.column());
    if SEEN.with(|s| s.borrow().contains(&key)) {
        return;
    }
    // A component's render, not a handler or a memo (their contexts have no scope).
    #[cfg(feature = "kit")]
    let render = view::in_render();
    #[cfg(not(feature = "kit"))]
    let render = on_air::in_render();
    if !render || !SEEN.with(|s| s.borrow_mut().insert(key)) {
        return;
    }
    let (what, instead) = match heap {
        false => ("reads all of", "part(|c| &c.field) re-renders it only when that field changes"),
        true => ("reads a part(…) inside a list's heap of", "pick the field (part(|c| &c.rows)), then index it"),
    };
    crate::info!(cat: "hint", "{at}: a render {what} {cell}, so it re-renders on every commit; {instead}");
}

/// Call a cell method from a view: `act!(toggle(id, day))`; a refusal is
/// logged and gives `None` (needs `Ui` and the `<Cell>Calls` trait in scope).
#[macro_export]
macro_rules! act {
    ($($call:tt)*) => { $crate::call!(Ui, $($call)*) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_asks_the_last_handler_first_until_one_takes_it() {
        // aio tests/on-back-button.test.ts.
        let seen = std::cell::RefCell::new(vec![]);
        let say = |n: &'static str, took: bool| {
            let seen = &seen;
            move || {
                seen.borrow_mut().push(n);
                took
            }
        };
        let (screen, dialog) = (say("screen", true), say("dialog", false));
        assert!(back_taken(&[&screen, &dialog]));
        assert_eq!(*seen.borrow(), ["dialog", "screen"], "the latest first, then on down");
        let none = say("home", false);
        assert!(!back_taken(&[&none]), "nobody took it: the host's default");
        assert!(!back_taken(&[]));
        let boom = || -> bool { panic!("bad handler") };
        let after = say("after", false);
        assert!(back_taken(&[&screen, &after, &boom]), "a panicking handler took it");
        assert_eq!(seen.borrow().last(), Some(&"home"), "and nobody below it ran");
    }

    #[tokio::test]
    async fn a_link_before_anyone_listens_waits_for_the_first_and_later_ones_reach_all() {
        // aio tests/android-deep-links.test.ts: cold (before the page) and warm.
        open_link("myapp://users/7");
        let mut first = links();
        assert_eq!(first.try_recv().ok().as_deref(), Some("myapp://users/7"), "the cold start's link");
        let mut second = links();
        assert!(second.try_recv().is_err(), "handed over once");
        open_link("myapp://a?q=<script>");
        assert_eq!(first.try_recv().ok().as_deref(), Some("myapp://a?q=<script>"), "as data, as sent");
        assert_eq!(second.try_recv().ok().as_deref(), Some("myapp://a?q=<script>"));
        drop((first, second));
        open_link("myapp://later");
        assert_eq!(links().try_recv().ok().as_deref(), Some("myapp://later"), "nobody left: it waits again");
    }

    #[tokio::test]
    async fn back_with_no_views_listening_is_not_taken() {
        let mut views = back_presses();
        let asked = back_pressed();
        views.recv().await.unwrap().send(true).unwrap();
        assert!(asked.await, "the views' answer");
        // A later listener (the kit over the router) is asked first; its
        // "no" passes Back to the one before it.
        let mut later = back_presses();
        let asked = tokio::spawn(back_pressed());
        later.recv().await.unwrap().send(false).unwrap();
        views.recv().await.unwrap().send(true).unwrap();
        assert!(asked.await.unwrap(), "the earlier listener took it");
        drop((views, later));
        assert!(!back_pressed().await, "nobody listening");
    }

    #[test]
    fn a_fixed_scheme_rewrites_the_sheets_the_oss_keeps_them_and_tokens_are_made_on_the_app_root() {
        let kit = ":root{--accent:green;--tint:color-mix(in oklab,var(--accent) 10%,white)}\n\
                   @media (prefers-color-scheme: dark){:root{--bg:black}}";
        let app = ":root[data-theme=\"light\"]{--accent:blue}";
        let light = page_css(&[kit, app], false);
        assert!(light.contains(":root,[data-theme]{--accent:green;--tint:"), "{light}");
        assert!(light.contains("@media not all{:root,[data-theme]{--bg:black}}"), "{light}");
        assert!(light.contains("[data-theme=\"light\"]{--accent:blue}"));
        assert!(page_css(&[kit], true).contains("@media all{"));
        let os = page_css(&[kit], Scheme::System);
        assert!(os.contains("@media (prefers-color-scheme: dark){:root,[data-theme]{--bg:black}}"), "the OS's: {os}");
        assert!(os.contains(":root{color-scheme:light dark}"));
    }

    #[test]
    fn theme_none_drops_even_the_base_rules() {
        // aio server-html-gen: `theme: "none"` emits no data-aio-box-base.
        let base = "*,*::before,*::after{box-sizing:border-box}body{margin:0}";
        assert!(sheets_css(&["a{}"], Scheme::Light, true).starts_with(base));
        let bare = sheets_css(&["a{}"], Scheme::Light, false);
        assert!(!bare.contains("box-sizing") && bare.starts_with("a{}"), "{bare}");
    }
}

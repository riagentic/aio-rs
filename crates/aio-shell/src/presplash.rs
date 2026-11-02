//! The splash drawn by the OS before Electron starts (aio's `splash.ts`,
//! `splash-session.ts` and the native helpers of `runtime-dialog.ts`): up
//! from the app's first moment on every start (`[build.splash] show =
//! "always"`), and the screen a first start's Electron download runs on, its
//! bar under the text. Linux draws it with GTK 3 over FFI ([`gtksplash`]),
//! macOS with JavaScript for Automation over Cocoa, Windows with
//! PowerShell's Windows Forms. On Windows, when Electron is already here,
//! Electron's main process draws it instead (main.js, `splash.rs`), as aio's
//! does: the Forms helper takes longer to show than the app's window.
//!
//! Its life is the pure [`step`]: it goes once the app's window has shown
//! and it has been visible `min_ms`, when Electron exits, at its cap
//! (`max_ms` with no window and no download running), when the user
//! dismisses it (its ✕ during a download), when the download fails (the
//! Retry / Quit question takes over) or when its helper never shows it.
//! Nothing waits for it: the app's window shows the moment it is ready,
//! under the splash while the splash is still up. A helper that cannot draw
//! (no GTK, no display) hands the start splash back to Electron and the
//! download to the plain download window, with the reason said once.
//!
//! [`gtksplash`]: crate::gtksplash
use crate::firstrun::Shown;
use crate::place::Rect;
use crate::splash::Target;
use aio::config::{Align, BarPosition, BarStyle, Fit, Splash, SplashPosition, TextPosition};
use serde::Serialize;
use std::path::PathBuf;
use std::process::Child;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

// ── the look ───────────────────────────────────────────────────────────────

/// The look as a native helper reads it (aio's `helperLook`, one JSON
/// argument): every "not set" resolved to that OS's own default. `sized`:
/// the window is not the image's own size (`size` set, or an image larger
/// than [`MAX_BOX`] scaled down), so the image is fitted into it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Look {
    pub(crate) sized: bool,
    pub(crate) w: u32,
    pub(crate) h: u32,
    /// The image's own size in logical px (`@2x` halved).
    pub(crate) iw: u32,
    pub(crate) ih: u32,
    pub(crate) fit: Fit,
    pub(crate) mode: SplashPosition,
    pub(crate) target: Option<Target>,
    pub(crate) below: bool,
    pub(crate) align: Align,
    pub(crate) side: u32,
    pub(crate) bottom: u32,
    pub(crate) bar_edge: bool,
    pub(crate) bar_radius: Option<u32>,
    pub(crate) radius: u32,
    pub(crate) border: Option<String>,
    pub(crate) shadow: bool,
    pub(crate) window_shadow: bool,
    pub(crate) fade_ms: u64,
    pub(crate) cancel: bool,
    /// The bar's height in logical px; 0: no bar.
    pub(crate) bp: u32,
}

/// The largest an image drawn at its own size is shown, in logical px: 80 %
/// of a 1366×768 screen, the smallest common one. Larger is scaled down, its
/// aspect kept (a 2560×1440 image made a full-screen always-on-top window).
pub(crate) const MAX_BOX: (u32, u32) = (1092, 614);

/// The bar's thickness in points (`bar_style`).
fn bar_pt(s: BarStyle) -> f64 {
    match s {
        BarStyle::Solid => 6.0,
        BarStyle::Thin => 3.0,
    }
}

/// `s`'s look on `os` (`"linux"`, `"macos"`, `"windows"`), opened over
/// `target`; `None`: the image is not a PNG. Pure.
pub(crate) fn look(s: &Splash, os: &str, target: Option<Target>) -> Option<Look> {
    let (iw, ih) = crate::splash::size(s)?;
    let k = match s.size {
        Some(_) => 1.0,
        None => 1f64.min(MAX_BOX.0 as f64 / iw as f64).min(MAX_BOX.1 as f64 / ih as f64),
    };
    let (w, h) = s.size.map_or(((iw as f64 * k).round() as u32, (ih as f64 * k).round() as u32), |[w, h]| (w, h));
    let bp = match s.progress {
        true => s.bar_height.unwrap_or_else(|| ((bar_pt(s.bar_style) * 96.0 / 72.0).round() as u32).max(2)),
        false => 0,
    };
    Some(Look {
        sized: s.size.is_some() || k < 1.0,
        w,
        h,
        iw,
        ih,
        fit: s.fit,
        mode: s.position,
        target,
        below: s.text_position == TextPosition::Below,
        align: s.align,
        side: s.margin.unwrap_or(24),
        bottom: s.margin.unwrap_or(18),
        bar_edge: s.bar_position == BarPosition::BottomEdge,
        bar_radius: s.bar_radius,
        radius: s.radius,
        border: s.border.as_deref().map(crate::firstrun::hex6),
        shadow: s.shadow.unwrap_or(os != "windows"),
        window_shadow: s.window_shadow.unwrap_or(os == "macos"),
        fade_ms: s.fade_ms.unwrap_or(100),
        cancel: s.cancel,
        bp,
    })
}

/// The parts of the splash below or over the image, laid out the same on
/// every OS (aio's `splashFrame`): the bar `bottom` px above the window's
/// bottom edge (or along it, full width), `side` px from the sides; the
/// text's bottom 8 px above the bar (or `bottom` px above the edge). Text
/// below the image adds a strip as tall as it holds. `lh`: the text's
/// height, 0 for none.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))] // GTK's; the helpers place themselves
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Frame {
    pub(crate) th: i32,
    pub(crate) yb: i32,
    pub(crate) bx: i32,
    pub(crate) bw: i32,
    pub(crate) label_bottom: i32,
}

/// [`Frame`] of `l` with a text `lh` px high. Pure.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))] // GTK's; the helpers place themselves
pub(crate) fn frame(l: &Look, lh: i32) -> Frame {
    let (w, h, side, bottom, bp) = (l.w as i32, l.h as i32, l.side as i32, l.bottom as i32, l.bp as i32);
    let under = bp > 0 && !l.bar_edge;
    let th = if l.below && lh > 0 { h + 8 + lh + bottom + if under { 8 + bp } else { 0 } } else { h };
    let yb = if l.bar_edge { th - bp } else { th - bottom - bp };
    Frame {
        th,
        yb,
        bx: if l.bar_edge { 0 } else { side },
        bw: if l.bar_edge { w } else { w - 2 * side },
        label_bottom: if under { yb - 8 } else { th - bottom },
    }
}

/// Where an `iw`×`ih` image is drawn in a `w`×`h` box: `cover` fills it
/// (the overflow cropped, centred), `contain` shows it whole. Pure.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))] // GTK's; the helpers place themselves
pub(crate) fn fit_rect(iw: u32, ih: u32, w: u32, h: u32, fit: Fit) -> Rect {
    let (iw, ih, w, h) = (iw.max(1) as f64, ih.max(1) as f64, w as f64, h as f64);
    let k = match fit {
        Fit::Contain => (w / iw).min(h / ih),
        Fit::Cover => (w / iw).max(h / ih),
    };
    let (dw, dh) = ((iw * k).round(), (ih * k).round());
    Rect { x: ((w - dw) / 2.0).round() as i32, y: ((h - dh) / 2.0).round() as i32, width: dw as i32, height: dh as i32 }
}

/// A rounded rectangle as one-pixel rows plus its middle: what an X11
/// window shape takes (no anti-aliasing, as on Windows). Pure.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))] // GTK's; the helpers place themselves
pub(crate) fn rounded_rows(w: i32, h: i32, r: i32) -> Vec<Rect> {
    let r = r.min(w / 2).min(h / 2);
    if r <= 0 {
        return vec![Rect { x: 0, y: 0, width: w, height: h }];
    }
    let mut rows = Vec::with_capacity(2 * r as usize + 1);
    for y in 0..r {
        let dy = f64::from(r - y) - 0.5;
        let inset = (f64::from(r) - (f64::from(r * r) - dy * dy).sqrt()).round() as i32;
        rows.push(Rect { x: inset, y, width: w - 2 * inset, height: 1 });
        rows.push(Rect { x: inset, y: h - 1 - y, width: w - 2 * inset, height: 1 });
    }
    rows.push(Rect { x: 0, y: r, width: w, height: h - 2 * r });
    rows
}

/// The top-left of a `w`×`h` window centred on `r`, kept
/// [`crate::place::EDGE_MARGIN`] inside the screen `s` (one wider than that
/// sits at the margin). Pure.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))] // GTK's; the helpers place themselves
pub(crate) fn centred_on(r: Rect, s: Rect, (w, h): (i32, i32)) -> (i32, i32) {
    let m = crate::place::EDGE_MARGIN;
    let (mx, my) = (m.min(s.width / 8), m.min(s.height / 8));
    let fit = |v: i32, lo: i32, n: i32, span: i32| v.min(lo + (span - n).max(0)).max(lo);
    (
        fit(
            (f64::from(r.x) + f64::from(r.width) / 2.0 - f64::from(w) / 2.0).round() as i32,
            s.x + mx,
            w,
            s.width - 2 * mx,
        ),
        fit(
            (f64::from(r.y) + f64::from(r.height) / 2.0 - f64::from(h) / 2.0).round() as i32,
            s.y + my,
            h,
            s.height - 2 * my,
        ),
    )
}

/// The screen holding `p`, else `primary`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))] // GTK's; the helpers place themselves
fn screen_at(screens: &[Rect], primary: Rect, (x, y): (i32, i32)) -> Rect {
    *screens.iter().find(|s| x >= s.x && y >= s.y && x < s.x + s.width && y < s.y + s.height).unwrap_or(&primary)
}

/// The top-left of the `size` splash (aio's `splashOrigin`): `window`,
/// centred where the app's window opens (`place::place` of its target; none:
/// the primary's centre); `pointer`, the centre of the screen under it;
/// `primary`, the primary's centre. Pure.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))] // GTK's; the helpers place themselves
pub(crate) fn origin(
    l: &Look,
    screens: &[Rect],
    primary: Rect,
    pointer: Option<(i32, i32)>,
    size: (i32, i32),
) -> (i32, i32) {
    match (l.mode, l.target, pointer) {
        (SplashPosition::Window, Some((x, y, w, h)), _) => {
            let saved = x.zip(y).map(|(x, y)| crate::place::Saved {
                normal: Rect { x, y, width: w, height: h },
                declared_width: w as u32,
                declared_height: h as u32,
                ..Default::default()
            });
            let r = crate::place::place(w as u32, h as u32, saved.as_ref(), screens, primary).rect;
            let s = screen_at(screens, primary, (r.x + r.width / 2, r.y + r.height / 2));
            centred_on(r, s, size)
        }
        (SplashPosition::Pointer, _, Some(p)) => {
            let s = screen_at(screens, primary, p);
            centred_on(s, s, size)
        }
        _ => centred_on(primary, primary, size),
    }
}

/// The top-left of the `size` splash centred on the app's window where it
/// really is (`[x, y, w, h]`), only its screen chosen. Pure.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))] // GTK's; the helpers place themselves
pub(crate) fn onto([x, y, w, h]: [i32; 4], screens: &[Rect], primary: Rect, size: (i32, i32)) -> (i32, i32) {
    let r = Rect { x, y, width: w, height: h };
    centred_on(r, screen_at(screens, primary, (x + w / 2, y + h / 2)), size)
}

// ── the lifetime ───────────────────────────────────────────────────────────

/// Why the splash went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Close {
    /// The app's window showed and the splash had been visible `min_ms`.
    Ready,
    /// Electron exited, or never started.
    Exit,
    /// No window within `max_ms` (no download running).
    Cap,
    /// The user closed it (its ✕ during a download).
    Dismissed,
    /// The download failed: the Retry / Quit question takes over.
    Failed,
    /// Its helper never showed a window.
    Lost,
}

/// The splash's own timeline, in ms since it was asked for. Nothing waits
/// on it: the app's window shows the moment it is ready.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct State {
    pub(crate) closed: Option<Close>,
    /// When the window was on screen.
    pub(crate) shown_at: Option<u64>,
    /// When the app's window showed.
    pub(crate) ready_at: Option<u64>,
    pub(crate) downloading: bool,
    /// The cap counts from here: the open, or the end of the last download.
    pub(crate) cap_from: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    Shown(u64),
    Ready(u64),
    Tick(u64),
    Download(bool, u64),
    End(Close),
}

/// The splash's next state (aio's `splashStep`). It goes (`Ready`) once the
/// app's window has shown and it has been VISIBLE `min_ms`: the window is up
/// by then, under it. Ready before the splash is on screen: there is nothing
/// to cover any more, it goes at once. The other ends close it at once; the
/// cap never runs during a download. Pure.
pub(crate) fn step(s: State, e: Event, min_ms: u64, cap_ms: u64) -> State {
    let settle = |n: State, at: u64| match (n.closed, n.ready_at, n.shown_at) {
        (None, Some(_), None) => State { closed: Some(Close::Ready), ..n },
        (None, Some(_), Some(shown)) if at.saturating_sub(shown) >= min_ms => State { closed: Some(Close::Ready), ..n },
        _ => n,
    };
    match e {
        Event::Ready(at) => return settle(State { ready_at: s.ready_at.or(Some(at)), ..s }, at),
        Event::Tick(at) => {
            let n = settle(s, at);
            if n != s || s.closed.is_some() {
                return n;
            }
            return match !s.downloading && at.saturating_sub(s.cap_from) >= cap_ms {
                true => State { closed: Some(Close::Cap), ..s },
                false => s,
            };
        }
        _ if s.closed.is_some() => return s,
        _ => {}
    }
    match e {
        Event::Shown(at) => State { shown_at: Some(at), ..s },
        Event::Download(true, _) => State { downloading: true, ..s },
        Event::Download(false, at) => State { downloading: false, cap_from: at, ..s },
        Event::End(c) => State { closed: Some(c), ..s },
        Event::Ready(_) | Event::Tick(_) => s,
    }
}

// ── the helpers ────────────────────────────────────────────────────────────

/// What draws the splash on this OS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Gtk,
    Cocoa,
    Forms,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Gtk => "GTK",
            Kind::Cocoa => "Cocoa",
            Kind::Forms => "Windows Forms",
        }
    }
}

/// The helper for `os`, or `None`: no desktop to draw on
/// (`AIORT_DIALOG=none` too: a contained run draws nothing).
pub(crate) fn kind(os: &str, env: &dyn Fn(&str) -> Option<String>) -> Option<Kind> {
    if env("AIORT_DIALOG").is_some_and(|v| v == "none" || v == "browser") || crate::os::desktop(os, env).is_err() {
        return None;
    }
    match os {
        "linux" => Some(Kind::Gtk),
        "macos" => Some(Kind::Cocoa),
        "windows" => Some(Kind::Forms),
        _ => None,
    }
}

/// What a splash window is opened with.
pub(crate) struct Open {
    pub(crate) title: String,
    pub(crate) text: String,
    pub(crate) has_text: bool,
    pub(crate) png: &'static [u8],
    /// 2 for an `@2x` image: drawn at half its pixels.
    pub(crate) scale: i32,
    pub(crate) look: Look,
    /// `[light, dark]`, each background, text, bar, track (`#rrggbb`).
    pub(crate) colors: [[String; 4]; 2],
    pub(crate) font: Option<String>,
    pub(crate) font_px: u16,
    pub(crate) bold: bool,
}

/// The window's news, written by its helper's side, read by the session.
#[derive(Debug, Default)]
pub(crate) struct Status {
    /// On screen: what was drawn (the helper's own line).
    pub(crate) shown: Option<String>,
    pub(crate) cancelled: bool,
    pub(crate) lost: Option<String>,
}

/// A splash helper process (Cocoa, Forms) and its files: `progress`
/// (`<pct>\t<text>`, `-1` hides the bar, or `close`), `shown` (the ack,
/// what was drawn), `at` (the app's window where it really is).
#[cfg_attr(target_os = "linux", allow(dead_code))] // GTK draws in the process there
struct Helper {
    child: Child,
    dir: PathBuf,
    last: String,
}

#[cfg_attr(target_os = "linux", allow(dead_code))]
enum View {
    #[cfg(target_os = "linux")]
    Gtk(crate::gtksplash::Window),
    Helper(Helper),
}

/// The progress file's line: the bar's percent (`-1`: none) and the text.
pub(crate) fn line(pct: Option<u64>, text: &str) -> String {
    format!("{}\t{}", pct.map_or(-1, |p| p.min(100) as i64), text.replace(['\r', '\n', '\t'], " "))
}

#[cfg(any(target_os = "macos", windows))]
fn spawn(kind: Kind, o: &Open) -> std::io::Result<Helper> {
    let dir = crate::firstrun::helper_dir()?;
    let made = (|| {
        let (file, ack, at, image) = (dir.join("progress"), dir.join("shown"), dir.join("at"), dir.join("splash.png"));
        std::fs::write(&file, line(None, &o.text))?;
        std::fs::write(&image, o.png)?;
        let look = aio::serde_json::to_string(&o.look).map_err(std::io::Error::other)?;
        let pid = std::process::id().to_string();
        let [light, dark] = o.colors.clone().map(|c| c.join(" "));
        let mut cmd = match kind {
            #[cfg(target_os = "macos")]
            Kind::Cocoa => {
                let script = dir.join("splash.js");
                std::fs::write(&script, [MAC_PLACE_JXA, MAC_SPLASH_JXA].concat())?;
                let mut cmd = crate::firstrun::quiet(std::process::Command::new("/usr/bin/osascript"));
                cmd.args(["-l", "JavaScript"]).arg(&script);
                cmd.args([o.title.as_str(), &o.text, if o.has_text { "1" } else { "0" }])
                    .arg(&file)
                    .arg(&ack)
                    .args([&pid, &light, &dark])
                    .arg(&image)
                    .arg(o.scale.to_string())
                    .arg(o.font.as_deref().unwrap_or(""))
                    .arg(o.font_px.to_string())
                    .arg(if o.bold { "1" } else { "0" })
                    .arg(&look)
                    .arg(&at);
                cmd
            }
            #[cfg(windows)]
            Kind::Forms => {
                let script = dir.join("splash.ps1");
                std::fs::write(&script, [PS_DPI, PS_PLACE, WINDOWS_SPLASH_PS].concat())?;
                let mut cmd = crate::firstrun::powershell(&script);
                cmd.env("AIO_SPL_LOOK", &look)
                    .env("AIO_SPL_TITLE", &o.title)
                    .env("AIO_SPL_TEXT", &o.text)
                    .env("AIO_SPL_HASTEXT", if o.has_text { "1" } else { "0" })
                    .env("AIO_SPL_LIGHT", &light)
                    .env("AIO_SPL_DARK", &dark)
                    .env("AIO_SPL_IMAGE", &image)
                    .env("AIO_SPL_SCALE", o.scale.to_string())
                    .env("AIO_SPL_FONT", o.font.as_deref().unwrap_or(""))
                    .env("AIO_SPL_SIZE", o.font_px.to_string())
                    .env("AIO_SPL_BOLD", if o.bold { "1" } else { "0" })
                    .env("AIO_SPL_PARENT", &pid)
                    .env("AIO_SPL_FILE", &file)
                    .env("AIO_SPL_ACK", &ack)
                    .env("AIO_SPL_AT", &at);
                cmd
            }
            // Each OS's own helper only.
            _ => return Err(std::io::Error::other("no splash helper for this kind")),
        };
        cmd.spawn()
    })();
    match made {
        Ok(child) => Ok(Helper { child, dir, last: String::new() }),
        Err(e) => {
            _ = std::fs::remove_dir_all(&dir);
            Err(e)
        }
    }
}

/// How long a helper process has to show its window (PowerShell and
/// osascript start slowly on a loaded machine).
#[cfg_attr(target_os = "linux", allow(dead_code))]
const HELPER_SHOWS_WITHIN: Duration = Duration::from_secs(15);

impl View {
    fn open(kind: Kind, o: Open, status: &Arc<Mutex<Status>>) -> Result<View, String> {
        match kind {
            #[cfg(target_os = "linux")]
            Kind::Gtk => crate::gtksplash::open(o, status.clone()).map(View::Gtk),
            #[cfg(any(target_os = "macos", windows))]
            _ => {
                _ = status;
                spawn(kind, &o).map(View::Helper).map_err(|e| format!("its helper did not start ({e})"))
            }
            #[cfg(not(any(target_os = "macos", windows)))]
            _ => Err("no splash helper on this OS".into()),
        }
    }

    fn update(&mut self, pct: Option<u64>, text: &str) {
        match self {
            #[cfg(target_os = "linux")]
            View::Gtk(w) => w.update(pct, text),
            View::Helper(h) => {
                let l = line(pct, text);
                if l != h.last {
                    // A skipped frame is all a failed write costs: the next one writes again.
                    _ = std::fs::write(h.dir.join("progress"), &l);
                    h.last = l;
                }
            }
        }
    }

    fn move_to(&mut self, r: [i32; 4]) {
        match self {
            #[cfg(target_os = "linux")]
            View::Gtk(w) => w.move_to(r),
            View::Helper(h) => _ = std::fs::write(h.dir.join("at"), aio::serde_json::json!(r).to_string()),
        }
    }

    /// A helper process's news into `st` (GTK writes its own).
    fn poll(&mut self, st: &mut Status, since: Instant) {
        #[cfg(target_os = "linux")]
        let View::Helper(h) = self else { return };
        #[cfg(not(target_os = "linux"))]
        let View::Helper(h) = self;
        if st.shown.is_none()
            && let Ok(what) = std::fs::read_to_string(h.dir.join("shown"))
            && !what.is_empty()
        {
            st.shown = Some(what);
        }
        match h.child.try_wait() {
            Ok(Some(e)) if e.code() == Some(3) => st.cancelled = true,
            Ok(Some(e)) if st.lost.is_none() => {
                st.lost = Some(match st.shown {
                    Some(_) => format!("its helper ended ({e})"),
                    None if e.code() == Some(4) => "its helper could not read the image".into(),
                    None => format!("its helper exited ({e}) before its window showed"),
                })
            }
            _ if st.shown.is_none() && st.lost.is_none() && since.elapsed() > HELPER_SHOWS_WITHIN => {
                st.lost = Some(format!("its window did not show within {} s", HELPER_SHOWS_WITHIN.as_secs()))
            }
            _ => {}
        }
    }

    /// Fades it out and lets it go (2 s at most past the fade).
    fn close(self, fade_ms: u64) {
        match self {
            #[cfg(target_os = "linux")]
            View::Gtk(w) => w.close(fade_ms),
            View::Helper(mut h) => {
                _ = std::fs::write(h.dir.join("progress"), "close");
                let until = Instant::now() + Duration::from_millis(fade_ms) + Duration::from_secs(2);
                while matches!(h.child.try_wait(), Ok(None)) && Instant::now() < until {
                    std::thread::sleep(Duration::from_millis(20));
                }
                _ = h.child.kill();
                _ = h.child.wait();
                _ = std::fs::remove_dir_all(&h.dir);
            }
        }
    }
}

// ── the session ────────────────────────────────────────────────────────────

/// One splash per start, owned by the app's process.
pub(crate) struct Session {
    inner: Mutex<Inner>,
    status: Arc<Mutex<Status>>,
    t0: Instant,
    kind: Kind,
    min_ms: u64,
    cap_ms: u64,
    fade_ms: u64,
    progress: bool,
    follow: bool,
    by_app: bool,
}

struct Inner {
    st: State,
    view: Option<View>,
    /// What opens it, until it has (`None`: opened, or never will).
    open: Option<Open>,
    /// Opened only for a download (`first-start`, or Electron draws).
    deferred: bool,
    opened_at: Option<Instant>,
    /// The start's text, and the download's while one runs.
    text: String,
    dl: Option<(Option<u64>, String)>,
    at: Option<[i32; 4]>,
    said_shown: bool,
}

/// This start's splash, while it is up (`ready`, `moved`, `end` reach it).
static CURRENT: Mutex<Option<Arc<Session>>> = Mutex::new(None);

pub(crate) fn current() -> Option<Arc<Session>> {
    CURRENT.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// How this start's splash is drawn, from `[build.splash]`.
pub(crate) struct Plan<'a> {
    pub(crate) splash: &'a Splash,
    pub(crate) title: &'a str,
    pub(crate) version: &'a str,
    /// `[light, dark]` (`splash::look_colors`).
    pub(crate) colors: &'a [[String; 4]; 2],
    pub(crate) target: Option<Target>,
    /// Up from the start (`show = "always"`), else only for a download.
    pub(crate) at_start: bool,
    /// `show` is the app's run-time one.
    pub(crate) by_app: bool,
    /// This start may download Electron (it is not here yet).
    pub(crate) may_download: bool,
}

/// The start's text: `{title}` and `{version}` filled.
fn start_text(t: Option<&str>, title: &str, version: &str) -> String {
    t.map_or(String::new(), |t| t.replace("{title}", title).replace("{version}", version))
}

impl Session {
    /// The splash for this start, drawn by `kind`; opened at once unless it
    /// waits for a download. `None`: no PNG.
    pub(crate) fn start(p: &Plan, kind: Kind, os: &str) -> Option<Arc<Session>> {
        let s = p.splash;
        let look = look(s, os, p.target)?;
        let text = start_text(s.text.as_deref(), p.title, p.version);
        let fade_ms = look.fade_ms;
        let open = Open {
            title: p.title.into(),
            text: text.clone(),
            has_text: s.text.is_some() || p.may_download,
            png: s.png,
            scale: if s.retina() { 2 } else { 1 },
            look,
            colors: p.colors.clone().map(|c| c.map(|x| crate::firstrun::hex6(&x))),
            font: s.font.clone().filter(|f| !f.trim().is_empty()),
            font_px: s.font_size.unwrap_or(13),
            bold: s.bold,
        };
        // Windows: Electron's main process draws the start's splash when
        // Electron is here (main.js); the Forms helper opens only for a download.
        let deferred = !p.at_start || kind == Kind::Forms;
        let me = Arc::new(Session {
            inner: Mutex::new(Inner {
                st: State::default(),
                view: None,
                open: Some(open),
                deferred,
                opened_at: None,
                text,
                dl: None,
                at: None,
                said_shown: false,
            }),
            status: Default::default(),
            t0: Instant::now(),
            kind,
            min_ms: s.min_ms,
            cap_ms: s.max_ms,
            fade_ms,
            progress: s.progress,
            follow: s.position == SplashPosition::Window && s.follow_window,
            by_app: p.by_app,
        });
        *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(me.clone());
        if !deferred {
            me.open_now();
        }
        Some(me)
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn now(&self) -> u64 {
        self.t0.elapsed().as_millis() as u64
    }

    fn open_now(self: &Arc<Self>) {
        let mut i = self.lock();
        let Some(o) = i.open.take() else { return };
        i.opened_at = Some(Instant::now());
        match View::open(self.kind, o, &self.status) {
            Ok(v) => {
                i.view = Some(v);
                self.push(&mut i);
                drop(i);
                let me = self.clone();
                _ = std::thread::Builder::new().name("aio-splash".into()).spawn(move || me.run());
            }
            Err(why) => {
                drop(i);
                self.lost(&why);
            }
        }
    }

    fn lost(&self, why: &str) {
        eprintln!("[aio] splash: not shown ({why})");
        self.event(Event::End(Close::Lost));
        self.finish();
    }

    /// The bar and the text as they are now, to the window.
    fn push(&self, i: &mut Inner) {
        let (pct, text) = match &i.dl {
            Some((pct, t)) => (pct.filter(|_| self.progress), t.clone()),
            None => (None, i.text.clone()),
        };
        if let Some(v) = i.view.as_mut() {
            v.update(pct, &text);
        }
    }

    /// Steps the state; said once when the app's window is ready.
    fn event(&self, e: Event) -> State {
        let mut i = self.lock();
        let was = i.st;
        i.st = step(i.st, e, self.min_ms, self.cap_ms);
        if i.st.ready_at.is_some() && was.ready_at.is_none() && i.opened_at.is_some() {
            match i.st.shown_at {
                None => eprintln!("[aio] splash: the app's window showed before the splash was on screen"),
                Some(at) => eprintln!(
                    "[aio] splash: the app's window showed {} ms after the splash appeared",
                    self.now().saturating_sub(at)
                ),
            }
        }
        i.st
    }

    /// Its own thread: the helper's news, the cap, and the close.
    fn run(self: Arc<Self>) {
        loop {
            std::thread::sleep(Duration::from_millis(50));
            let st = {
                let mut i = self.lock();
                let since = i.opened_at.unwrap_or(self.t0);
                let mut s = self.status.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(v) = i.view.as_mut() {
                    v.poll(&mut s, since);
                }
                let (shown, cancelled, lost) = (s.shown.clone(), s.cancelled, s.lost.clone());
                drop(s);
                if let Some(what) = shown.filter(|_| !i.said_shown) {
                    i.said_shown = true;
                    let ms = since.elapsed().as_millis();
                    eprintln!("[aio] splash: visible {ms} ms after it was asked for ({}): {what}", self.kind.name());
                    drop(i);
                    self.event(Event::Shown(self.now()));
                    // Where the app's window is, if it said so before this.
                    let mut i = self.lock();
                    if let (Some(at), Some(v)) = (i.at, i.view.as_mut()) {
                        v.move_to(at);
                    }
                } else {
                    drop(i);
                }
                if cancelled {
                    self.event(Event::End(Close::Dismissed));
                }
                if let Some(why) = lost
                    && self.lock().st.closed.is_none()
                {
                    eprintln!("[aio] splash: not shown ({why})");
                    self.event(Event::End(Close::Lost));
                }
                self.event(Event::Tick(self.now()))
            };
            if st.closed.is_some() {
                return self.finish();
            }
        }
    }

    /// Closes the window once the state has (the first caller does), said.
    fn finish(&self) {
        let (why, view) = {
            let mut i = self.lock();
            (i.st.closed, i.view.take())
        };
        // Current until its window is gone: whoever waits on it sees it done.
        let forget = || {
            let mut cur = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
            if cur.as_ref().is_some_and(|c| std::ptr::eq(&**c, self)) {
                *cur = None;
            }
        };
        let Some(view) = view else { return forget() };
        match why {
            Some(Close::Cap) => eprintln!(
                "[aio] splash: closed after {} s without the app's window ([build.splash] max_ms)",
                self.cap_ms as f64 / 1000.0
            ),
            Some(Close::Lost) | None => {}
            Some(c) => eprintln!("[aio] splash: closed ({})", format!("{c:?}").to_lowercase()),
        }
        view.close(self.fade_ms);
        forget();
    }

    /// The download is starting: false when the splash is gone (the plain
    /// download window then takes it).
    pub(crate) fn download(self: &Arc<Self>) -> bool {
        let deferred = {
            let mut i = self.lock();
            if i.st.closed.is_some() {
                return false;
            }
            i.dl = Some((None, String::new()));
            std::mem::replace(&mut i.deferred, false) && i.open.is_some()
        };
        if deferred {
            self.open_now();
        }
        self.event(Event::Download(true, self.now())).closed.is_none()
    }

    /// The download's bar and line; `Cancelled`: the user dismissed it.
    pub(crate) fn progress(&self, pct: Option<u64>, text: &str) -> Shown {
        let mut i = self.lock();
        match i.st.closed {
            Some(Close::Dismissed) => return Shown::Cancelled,
            Some(_) => return Shown::Lost("the splash is gone".into()),
            None => {}
        }
        i.dl = Some((pct, text.into()));
        self.push(&mut i);
        Shown::On
    }

    /// The download is over: the start's text again, the cap from now.
    pub(crate) fn downloaded(&self) {
        {
            let mut i = self.lock();
            i.dl = None;
            self.push(&mut i);
        }
        self.event(Event::Download(false, self.now()));
    }

    /// Electron is found: true when Electron's main process draws the
    /// start's splash (this one never opened, or could not draw); this
    /// session is then done.
    pub(crate) fn handoff(&self) -> bool {
        let mut i = self.lock();
        if i.open.is_some() {
            i.open = None;
            i.st.closed = Some(Close::Ready);
            if self.kind != Kind::Forms {
                let whose = if self.by_app { "the app's run-time show" } else { "show" };
                eprintln!("[aio] splash: not shown ({whose} is \"first-start\", no download)");
            }
            drop(i);
            self.finish();
            return true;
        }
        i.st.closed == Some(Close::Lost)
    }
}

#[cfg(test)]
impl Session {
    /// Clicks its ✕ (GTK's own "clicked": no pointer device in a test).
    #[cfg(target_os = "linux")]
    fn click(&self) {
        if let Some(View::Gtk(w)) = &self.lock().view {
            w.click();
        }
    }
    fn state(&self) -> State {
        self.lock().st
    }
}

/// However the start ends (an error before Electron, Electron gone), the
/// splash goes with it (`Close::Exit`).
pub(crate) struct Ends;

impl Drop for Ends {
    fn drop(&mut self) {
        end(Close::Exit);
    }
}

/// The app's window has shown (main.js says so), at `normal` where it
/// really is.
pub(crate) fn ready(normal: Option<[i32; 4]>) {
    let Some(s) = current() else { return };
    if let Some(r) = normal {
        moved(r);
    }
    s.event(Event::Ready(s.now()));
}

/// The app's window is at `r` (`[x, y, w, h]`): with `position = "window"`
/// and `follow_window`, the splash re-centres on it.
pub(crate) fn moved(r: [i32; 4]) {
    let Some(s) = current().filter(|s| s.follow) else { return };
    let mut i = s.lock();
    i.at = Some(r);
    if i.said_shown
        && let Some(v) = i.view.as_mut()
    {
        v.move_to(r);
    }
}

/// Electron exited (or never started), or the app is stopping: the splash
/// goes now.
pub(crate) fn end(why: Close) {
    if let Some(s) = current() {
        s.event(Event::End(why));
        s.finish();
    }
}

// ── the scripts ────────────────────────────────────────────────────────────

/// Per-monitor DPI awareness for the Windows helper, set before any window
/// exists: its coordinates are then physical screen pixels. `AioScaleAt x
/// y`: the scale (DPI / 96) of the monitor at that physical point, what the
/// helper sizes its contents by (aio's `PS_DPI`).
#[cfg(windows)]
pub(crate) const PS_DPI: &str = r#"
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type -WarningAction SilentlyContinue -TypeDefinition @"
using System; using System.Runtime.InteropServices;
public static class AioDpi {
  [StructLayout(LayoutKind.Sequential)] public struct PT { public int X; public int Y; }
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern IntPtr MonitorFromPoint(PT p, uint f);
  [DllImport("shcore.dll")] public static extern int GetDpiForMonitor(IntPtr m, int t, out uint x, out uint y);
  public static double At(int x, int y) {
    try {
      PT p; p.X = x; p.Y = y;
      uint dx, dy;
      if (GetDpiForMonitor(MonitorFromPoint(p, 2), 0, out dx, out dy) == 0 && dx > 0) return dx / 96.0;
    } catch {}
    return 1.0;
  }
}
"@
$aioPm = $false
try { $aioPm = [AioDpi]::SetProcessDpiAwarenessContext([IntPtr](-4)) } catch {}
if (-not $aioPm) { try { [void][AioDpi]::SetProcessDPIAware() } catch {} }
function AioScaleAt($x, $y) { if ($aioPm) { [AioDpi]::At([int]$x, [int]$y) } else { 1.0 } }
function AioScalePrimary {
  $w = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
  AioScaleAt ($w.X + $w.Width / 2) ($w.Y + $w.Height / 2)
}
"#;

/// The placement rule ([`origin`], `place::place`) in PowerShell, line for
/// line (aio's `PS_PLACE`): `$T` is `[x|null, y|null, width, height]` in
/// physical pixels, or `$null` for `$p`'s centre. Returns `@(x, y)`.
/// JavaScript's and Rust's round is `Floor(v + 0.5)` here (.NET rounds to even).
#[cfg(windows)]
pub(crate) const PS_PLACE: &str = r#"
function AioAll { @([System.Windows.Forms.Screen]::AllScreens | ForEach-Object { $_.WorkingArea }) }
function AioPrimary { [System.Windows.Forms.Screen]::PrimaryScreen.WorkingArea }
function AioFit($v, $lo, $n, $span) { if ($n -ge $span) { $lo } else { [Math]::Max($lo, [Math]::Min($v, $lo + $span - $n)) } }
function AioM($n) { [Math]::Min(64, [Math]::Floor($n / 8)) }
function AioPlace($T, $W, $H, $all, $p, $real = $false) {
  if (@($all).Count -eq 0) { $all = @($p) }
  if ($null -eq $T) { $s = $p; $rx = $p.X; $ry = $p.Y; $rw = $p.Width; $rh = $p.Height }
  else {
    $rw = [int]$T[2]; $rh = [int]$T[3]
    $placed = -not ($null -eq $T[0] -or $null -eq $T[1])
    if (-not $placed) { $s = $p } else {
      $x = [int]$T[0]; $y = [int]$T[1]; $s = $null
      foreach ($k in $all) { if ($x -ge $k.X -and $y -ge $k.Y -and $x + $rw -le $k.X + $k.Width -and $y + $rh -le $k.Y + $k.Height) { $s = $k; break } }
      if ($null -eq $s) {
        $best = -1
        foreach ($k in $all) {
          $ix = [Math]::Max(0, [Math]::Min($x + $rw, $k.X + $k.Width) - [Math]::Max($x, $k.X))
          $iy = [Math]::Max(0, [Math]::Min($y + $rh, $k.Y + $k.Height) - [Math]::Max($y, $k.Y))
          if ($ix * $iy -gt $best) { $best = $ix * $iy; $s = $k }
        }
      }
    }
    if ($real -and $placed) { $rx = $x; $ry = $y } else {
      $mx = AioM $s.Width; $my = AioM $s.Height
      $iw = [Math]::Max(1, $s.Width - 2 * $mx); $ih = [Math]::Max(1, $s.Height - 2 * $my)
      $rw = [Math]::Min($rw, $iw); $rh = [Math]::Min($rh, $ih)
      if ($placed) {
        $rx = AioFit $x ($s.X + $mx) $rw $iw
        $ry = AioFit $y ($s.Y + $my) $rh $ih
      } else {
        $rx = $s.X + $mx + [Math]::Floor(($iw - $rw) / 2)
        $ry = $s.Y + $my + [Math]::Floor(($ih - $rh) / 2)
      }
    }
  }
  $mx = AioM $s.Width; $my = AioM $s.Height
  $cx = AioFit ([Math]::Floor($rx + $rw / 2 - $W / 2 + 0.5)) ($s.X + $mx) $W ($s.Width - 2 * $mx)
  $cy = AioFit ([Math]::Floor($ry + $rh / 2 - $H / 2 + 0.5)) ($s.Y + $my) $H ($s.Height - 2 * $my)
  return @([int]$cx, [int]$cy)
}
"#;

/// The splash over Windows Forms (aio's `WINDOWS_SPLASH_PS`): borderless,
/// the image IS the window (its size, or `size` with the image fitted),
/// placed where the app's window will open, on top, never activated, no
/// taskbar entry; the text and the bar over its lower part (or in a strip
/// below the image). Reads `AIO_SPL_*` (`AIO_SPL_LOOK`: [`Look`] as JSON)
/// and the progress file; writes the ack once shown. Exit 3: dismissed.
#[cfg(windows)]
pub(crate) const WINDOWS_SPLASH_PS: &str = r#"
$L = $env:AIO_SPL_LOOK | ConvertFrom-Json
# Electron's logical pixels to this screen's physical ones.
$K0 = AioScalePrimary
$T = $null
if ($L.target) { $T = @($(if ($null -ne $L.target[0]) { [int]($L.target[0] * $K0) } else { $null }), $(if ($null -ne $L.target[1]) { [int]($L.target[1] * $K0) } else { $null }), [int]($L.target[2] * $K0), [int]($L.target[3] * $K0)) }
$K = $(if ($L.mode -eq 'window' -and $T -and $null -ne $T[0]) { AioScaleAt ($T[0] + $T[2] / 2) ($T[1] + $T[3] / 2) } elseif ($L.mode -eq 'pointer') { $q = [System.Windows.Forms.Cursor]::Position; AioScaleAt $q.X $q.Y } else { $K0 })
$dark = $false
try {
  $dark = (Get-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize' -Name AppsUseLightTheme).AppsUseLightTheme -eq 0
} catch {}
$c = $(if ($dark) { $env:AIO_SPL_DARK } else { $env:AIO_SPL_LIGHT }).Split(' ')
$fg = [System.Drawing.ColorTranslator]::FromHtml($c[1])
$bg = [System.Drawing.ColorTranslator]::FromHtml($c[0])
$clear = [System.Drawing.Color]::Transparent
$img = [System.Drawing.Image]::FromFile($env:AIO_SPL_IMAGE)
$sc = [int]$env:AIO_SPL_SCALE
$FW = [int]($img.Width / $sc * $K)
$FH = [int]($img.Height / $sc * $K)
if ($L.sized) { $FW = [int]($L.w * $K); $FH = [int]($L.h * $K) }
function RoundPath($x, $y, $w, $h, $r) {
  $p = New-Object System.Drawing.Drawing2D.GraphicsPath
  $d = [Math]::Min([Math]::Min($w, $h), 2 * $r)
  $p.AddArc($x, $y, $d, $d, 180, 90)
  $p.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
  $p.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90)
  $p.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
  $p.CloseFigure()
  return $p
}
# Above every normal window (the app's, shown under it) and never activated:
# the focus stays where it is and goes to the app's window when it shows.
Add-Type -WarningAction SilentlyContinue -ReferencedAssemblies System.Windows.Forms -TypeDefinition @"
public class AioSplashForm : System.Windows.Forms.Form {
  protected override bool ShowWithoutActivation { get { return true; } }
  protected override System.Windows.Forms.CreateParams CreateParams {
    get { var p = base.CreateParams; p.ExStyle |= 0x08000008; return p; }
  }
}
"@
$f = New-Object AioSplashForm
$f.Text = $env:AIO_SPL_TITLE
$f.FormBorderStyle = 'None'
$f.StartPosition = 'Manual'
$f.ShowInTaskbar = $false
$f.TopMost = $true
$f.BackColor = $bg
$bp = [int]([int]$L.bp * $K)
$side = [int]($L.side * $K)
$bottom = [int]($L.bottom * $K)
$rad = [int]($L.radius * $K)
$d = $null
$dh = 0
if ($env:AIO_SPL_HASTEXT -eq '1') {
  $fam = $(if ($env:AIO_SPL_FONT) { $env:AIO_SPL_FONT } else { [System.Drawing.SystemFonts]::MessageBoxFont.FontFamily.Name })
  $st = $(if ($env:AIO_SPL_BOLD -eq '1') { [System.Drawing.FontStyle]::Bold } else { [System.Drawing.FontStyle]::Regular })
  $d = New-Object System.Windows.Forms.Label
  # Logical px at the monitor's scale: the process's own DPI is not the
  # monitor's on a mixed-scale desktop.
  $d.Font = New-Object System.Drawing.Font($fam, [single]([single]$env:AIO_SPL_SIZE * $K), $st, [System.Drawing.GraphicsUnit]::Pixel)
  $d.ForeColor = $fg
  $d.BackColor = $clear
  $d.Text = $env:AIO_SPL_TEXT
  $d.AutoEllipsis = $true
  $d.TextAlign = $(if ($L.align -eq 'center') { 'TopCenter' } elseif ($L.align -eq 'right') { 'TopRight' } else { 'TopLeft' })
  $dh = $d.Font.Height + 4
}
# Below the image: a strip as tall as what it holds.
$TH = $FH
if ($L.below -and $d) {
  $TH = $FH + 8 + $dh + $bottom + $(if ($bp -gt 0 -and -not $L.barEdge) { 8 + $bp } else { 0 })
}
$f.ClientSize = New-Object System.Drawing.Size($FW, $TH)
if ($L.sized -or $TH -ne $FH) {
  # The image fitted into its box (cover: cropped; contain: whole, on the
  # background), the box at the window's top.
  $bm = New-Object System.Drawing.Bitmap($FW, $TH)
  $gr = [System.Drawing.Graphics]::FromImage($bm)
  $gr.Clear($bg)
  $gr.InterpolationMode = 'HighQualityBicubic'
  $iw = $img.Width / $sc * $K
  $ih = $img.Height / $sc * $K
  $k = $(if (-not $L.sized) { 1 } elseif ($L.fit -eq 'contain') { [Math]::Min($FW / $iw, $FH / $ih) } else { [Math]::Max($FW / $iw, $FH / $ih) })
  $dw = $iw * $k
  $dhh = $ih * $k
  $gr.SetClip((New-Object System.Drawing.RectangleF(0, 0, $FW, $FH)))
  $gr.DrawImage($img, (New-Object System.Drawing.RectangleF((($FW - $dw) / 2), (($FH - $dhh) / 2), $dw, $dhh)))
  $gr.Dispose()
  $f.BackgroundImage = $bm
  $f.BackgroundImageLayout = 'None'
} else {
  $f.BackgroundImage = $img
  $f.BackgroundImageLayout = 'Stretch'
}
$yb = $(if ($L.barEdge) { $TH - $bp } else { $TH - $bottom - $bp })
$bx = $(if ($L.barEdge) { 0 } else { $side })
$bw = $(if ($L.barEdge) { $FW } else { $FW - 2 * $side })
if ($d) {
  $d.Size = New-Object System.Drawing.Size(($FW - 2 * $side), $dh)
  $d.Location = New-Object System.Drawing.Point($side, ($(if ($bp -gt 0 -and -not $L.barEdge) { $yb - 8 } else { $TH - $bottom }) - $dh))
  $f.Controls.Add($d)
}
if ($rad -gt 0) { $f.Region = New-Object System.Drawing.Region((RoundPath 0 0 $FW $TH $rad)) }
if ($L.border) {
  $bpen = New-Object System.Drawing.Pen([System.Drawing.ColorTranslator]::FromHtml($L.border), 1)
  $f.Add_Paint({
    param($s, $e)
    if ($rad -gt 0) { $e.Graphics.SmoothingMode = 'AntiAlias'; $e.Graphics.DrawPath($bpen, (RoundPath 0 0 ($FW - 1) ($TH - 1) $rad)) }
    else { $e.Graphics.DrawRectangle($bpen, 0, 0, $FW - 1, $TH - 1) }
  })
}
$br = $(if ($null -ne $L.barRadius) { [int]($L.barRadius * $K) } else { [int]($bp / 2) })
function Rounded($ctl) {
  if ($br -gt 0 -and $ctl.Width -gt 0) { $ctl.Region = New-Object System.Drawing.Region((RoundPath 0 0 $ctl.Width $ctl.Height $br)) }
}
$tr = $null
$xb = $null
if ($bp -gt 0) {
  $tr = New-Object System.Windows.Forms.Panel
  $tr.BackColor = [System.Drawing.ColorTranslator]::FromHtml($c[3])
  $tr.Location = New-Object System.Drawing.Point($bx, $yb)
  $tr.Size = New-Object System.Drawing.Size($bw, $bp)
  $tr.Visible = $false
  Rounded $tr
  $fl = New-Object System.Windows.Forms.Panel
  $fl.BackColor = [System.Drawing.ColorTranslator]::FromHtml($c[2])
  $fl.Size = New-Object System.Drawing.Size(0, $bp)
  $tr.Controls.Add($fl)
  $f.Controls.Add($tr)
  if ($L.cancel) {
    # During a download, this cancels it (as a dialog's close button does).
    $xb = New-Object System.Windows.Forms.Label
    $xb.Text = [string][char]0x2715
    $xb.ForeColor = $fg
    $xb.BackColor = $clear
    $xb.AutoSize = $true
    $xb.Cursor = [System.Windows.Forms.Cursors]::Hand
    $xb.Location = New-Object System.Drawing.Point(($FW - [int](26 * $K)), [int](6 * $K))
    $xb.Visible = $false
    $xb.Add_Click({ $f.Close() })
    $f.Controls.Add($xb)
  }
}
$p = AioPrimary
$T2 = $null
if ($L.mode -eq 'window') { $T2 = $T }
elseif ($L.mode -eq 'pointer') { $p = [System.Windows.Forms.Screen]::FromPoint([System.Windows.Forms.Cursor]::Position).WorkingArea }
$o = AioPlace $T2 $FW $TH (AioAll) $p
$f.Location = New-Object System.Drawing.Point($o[0], $o[1])
function Fade {
  $n = [int][Math]::Round([int]$L.fadeMs / 16)
  for ($j = 0; $j -lt $n; $j++) {
    $f.Opacity = 0.85 * ($n - $j) / $n
    [System.Windows.Forms.Application]::DoEvents()
    Start-Sleep -Milliseconds 16
  }
}
$script:n = 0
$script:at = $null
$t2 = New-Object System.Windows.Forms.Timer
$t2.Interval = 100
$t2.Add_Tick({
  $script:n++
  # The app that opened this window is gone: nobody will close it.
  if ($script:n % 30 -eq 0 -and -not (Get-Process -Id ([int]$env:AIO_SPL_PARENT) -ErrorAction SilentlyContinue)) {
    $t2.Stop(); $f.Tag = 'done'; $f.Close(); return
  }
  # The app's window, where it really is: the splash moves onto it.
  $a2 = $null
  try { $a2 = [IO.File]::ReadAllText($env:AIO_SPL_AT) } catch {}
  if ($a2 -and $a2 -ne $script:at) {
    $script:at = $a2
    $r = @($a2 | ConvertFrom-Json | ForEach-Object { [int]($_ * $K0) })
    $o2 = AioPlace $r $f.Width $f.Height (AioAll) (AioPrimary) $true
    $f.Location = New-Object System.Drawing.Point($o2[0], $o2[1])
  }
  $s2 = $null
  try { $s2 = [IO.File]::ReadAllText($env:AIO_SPL_FILE) } catch { return }
  if ($s2 -eq 'close') { $t2.Stop(); Fade; $f.Tag = 'done'; $f.Close(); return }
  $p2 = $s2.Split([char]9, 2)
  if ($p2.Length -lt 2) { return }
  if ($d) { $d.Text = $p2[1] }
  if ($tr) {
    $k2 = -1
    [void][int]::TryParse($p2[0], [ref]$k2)
    if ($k2 -ge 0) {
      $fl.Width = [int]($tr.Width * [Math]::Min(100, $k2) / 100)
      Rounded $fl
      $tr.Visible = $true; if ($xb) { $xb.Visible = $true }
    } else { $tr.Visible = $false; if ($xb) { $xb.Visible = $false } }
  }
})
$t2.Start()
$f.Add_Shown({
  $what = "shown splash size=$($FW)x$($TH) scale=$sc dark=$([int]$dark) text=$([int][bool]$d) barpx=$bp at=$($f.Left),$($f.Top) colors=$($c -join ',')"
  try { [IO.File]::WriteAllText($env:AIO_SPL_ACK, $what) } catch {}
})
# Closed while PowerShell was starting (the app stopped, or its start
# failed): never shown.
try { if ([IO.File]::ReadAllText($env:AIO_SPL_FILE) -eq 'close') { exit 0 } } catch {}
# A message loop, not a modal show: that activates the window, whatever it asks.
[System.Windows.Forms.Application]::Run($f)
if ($f.Tag -eq 'done') { exit 0 } else { exit 3 }
"#;

/// The placement rule for the Cocoa helper (aio's `MAC_PLACE_JS`):
/// `windowRect` (`place::place`) and `centredOn` ([`centred_on`]) in
/// JavaScript, over the screens in Electron's coordinates (origin at the
/// primary's top-left). `aioPlace(T, w, h, real)` is the Cocoa origin of a
/// w×h frame centred where the app's window opens (`real`: where it is).
#[cfg(target_os = "macos")]
pub(crate) const MAC_PLACE_JXA: &str = r#"
ObjC.import('Cocoa');
ObjC.import('stdlib');
function windowRect(b, screens, primary) {
  const w0 = b.width >= 1 ? Math.round(b.width) : 800, h0 = b.height >= 1 ? Math.round(b.height) : 600;
  const placed = Number.isFinite(b.x) && Number.isFinite(b.y);
  const all = screens.length ? screens : [primary];
  let s = primary;
  if (placed) {
    const x = b.x, y = b.y;
    s = all.find((k) => x >= k.x && y >= k.y && x + w0 <= k.x + k.width && y + h0 <= k.y + k.height);
    if (!s) {
      let best = -1;
      for (const k of all) {
        const ix = Math.max(0, Math.min(x + w0, k.x + k.width) - Math.max(x, k.x));
        const iy = Math.max(0, Math.min(y + h0, k.y + k.height) - Math.max(y, k.y));
        if (ix * iy > best) { best = ix * iy; s = k; }
      }
    }
  }
  const mx = Math.min(64, Math.floor(s.width / 8)), my = Math.min(64, Math.floor(s.height / 8));
  const iw = Math.max(1, s.width - 2 * mx), ih = Math.max(1, s.height - 2 * my);
  const w = Math.min(w0, iw), h = Math.min(h0, ih), lx = s.x + mx, ly = s.y + my;
  return {
    x: placed ? Math.max(lx, Math.min(Math.round(b.x), lx + iw - w)) : lx + Math.floor((iw - w) / 2),
    y: placed ? Math.max(ly, Math.min(Math.round(b.y), ly + ih - h)) : ly + Math.floor((ih - h) / 2),
    width: w, height: h, screen: s,
  };
}
function centredOn(r, w, h) {
  const s = r.screen;
  const mx = Math.min(64, Math.floor(s.width / 8)), my = Math.min(64, Math.floor(s.height / 8));
  return {
    x: Math.max(s.x + mx, Math.min(Math.round(r.x + r.width / 2 - w / 2), s.x + mx + Math.max(0, s.width - 2 * mx - w))),
    y: Math.max(s.y + my, Math.min(Math.round(r.y + r.height / 2 - h / 2), s.y + my + Math.max(0, s.height - 2 * my - h))),
  };
}
function aioAreas() {
  const scr = $.NSScreen.screens;
  const PH = scr.objectAtIndex(0).frame.size.height;
  const areas = [];
  for (let i = 0; i < scr.count; i++) {
    const v = scr.objectAtIndex(i).visibleFrame;
    areas.push({ x: v.origin.x, y: PH - (v.origin.y + v.size.height), width: v.size.width, height: v.size.height });
  }
  return { PH, areas };
}
function aioPlace(T, w, h, real) {
  const { PH, areas } = aioAreas();
  const p = areas[0];
  const b = T && (T[0] === null ? { width: T[2], height: T[3] } : { x: T[0], y: T[1], width: T[2], height: T[3] });
  const r = b && windowRect(b, areas, p);
  const o = b
    ? centredOn(real && T[0] !== null ? { x: b.x, y: b.y, width: b.width, height: b.height, screen: r.screen } : r, w, h)
    : centredOn({ x: p.x, y: p.y, width: p.width, height: p.height, screen: p }, w, h);
  return $.NSMakePoint(o.x, PH - (o.y + h));
}
"#;

/// The splash over Cocoa (aio's `MAC_SPLASH_JXA`): borderless, the image IS
/// the window (its point size, an @2x image crisp on a Retina screen; or
/// `size`, the image fitted), where the app's window will open, floating
/// above other windows without taking focus and without a Dock icon; the
/// text (with a soft shadow) and a small bar over its lower part, or in a
/// strip below the image. argv: title, text, has text (1/0), progress file,
/// ack file, the app's pid, light colours, dark colours, image, scale, font
/// family, size (px), bold (1/0), the look ([`Look`], JSON), the move file.
/// Same file protocol as the Windows splash; `close` fades it out.
#[cfg(target_os = "macos")]
pub(crate) const MAC_SPLASH_JXA: &str = r#"
function run(argv) {
  const text = argv[1], hasText = argv[2] === '1', file = argv[3], ack = argv[4];
  const parent = argv[5], light = argv[6], dark = argv[7], image = argv[8];
  const scale = parseFloat(argv[9]), family = argv[10], size = parseFloat(argv[11]);
  const bold = argv[12] === '1';
  const L = JSON.parse(argv[13]);
  const atFile = argv[14] || '';
  let lastAt = '';
  let ticks = 0;
  const app = $.NSApplication.sharedApplication;
  // An accessory: no Dock icon, no menu bar, and it never becomes active.
  app.setActivationPolicy($.NSApplicationActivationPolicyAccessory);
  const st = $.NSUserDefaults.standardUserDefaults.stringForKey('AppleInterfaceStyle');
  const isDark = !!st && !st.isNil() && st.js === 'Dark';
  const hex = (h) => $.NSColor.colorWithSRGBRedGreenBlueAlpha(
    parseInt(h.slice(1, 3), 16) / 255, parseInt(h.slice(3, 5), 16) / 255,
    parseInt(h.slice(5, 7), 16) / 255, 1);
  const cs = (isDark ? dark : light).split(' ');
  const img = $.NSImage.alloc.initWithContentsOfFile(image);
  if (!img || img.isNil()) $.exit(4);
  const rep = img.representations.objectAtIndex(0);
  const iw = Math.round(rep.pixelsWide / scale), ih = Math.round(rep.pixelsHigh / scale);
  img.size = $.NSMakeSize(iw, ih);
  const W = L.sized ? L.w : iw, H = L.sized ? L.h : ih;
  const bp = L.bp;
  const side = L.side, bottom = L.bottom, edge = L.barEdge;
  let label = null, lh = 0;
  if (hasText) {
    let f = family ? $.NSFont.fontWithNameSize(family, size) : null;
    if (!f || f.isNil()) f = $.NSFont.systemFontOfSize(size);
    if (bold) f = $.NSFontManager.sharedFontManager.convertFontToHaveTrait(f, 2); // NSBoldFontMask
    label = $.NSTextField.labelWithString(text);
    label.font = f;
    label.textColor = hex(cs[1]);
    label.lineBreakMode = 4; // NSLineBreakByTruncatingTail
    if (L.shadow) {
      const sh = $.NSShadow.alloc.init;
      sh.shadowColor = $.NSColor.colorWithSRGBRedGreenBlueAlpha(0, 0, 0, 0.45);
      sh.shadowOffset = $.NSMakeSize(0, -1);
      sh.shadowBlurRadius = 2;
      label.shadow = sh;
    }
    lh = Math.ceil(label.fittingSize.height);
  }
  // Below the image: a strip as tall as what it holds.
  const TH = L.below && label ? H + 8 + lh + bottom + (bp > 0 && !edge ? 8 + bp : 0) : H;
  const win = $.NSWindow.alloc.initWithContentRectStyleMaskBackingDefer(
    $.NSMakeRect(0, 0, W, TH), $.NSWindowStyleMaskBorderless, $.NSBackingStoreBuffered, false);
  win.releasedWhenClosed = false;
  win.backgroundColor = hex(cs[0]);
  win.hasShadow = !!L.windowShadow;
  win.level = 3; // NSFloatingWindowLevel: above the app windows
  const view = win.contentView;
  // Rounded corners and a border: no CGColor (a CGColorRef through the JXA
  // bridge crashed osascript, aio measured), so the background and the
  // border are NSBoxes, and the layer only clips.
  const frameBox = (fillCol, border) => {
    const b = $.NSBox.alloc.initWithFrame($.NSMakeRect(0, 0, W, TH));
    b.boxType = 4; // NSBoxCustom
    b.titlePosition = 0; // NSNoTitle
    b.cornerRadius = L.radius;
    b.fillColor = fillCol;
    b.borderWidth = border ? 1 : 0;
    if (border) b.borderColor = hex(border);
    view.addSubview(b);
  };
  if (L.radius > 0) {
    win.opaque = false;
    win.backgroundColor = $.NSColor.clearColor;
    view.wantsLayer = true;
    view.layer.cornerRadius = L.radius;
    view.layer.masksToBounds = true;
    frameBox(hex(cs[0]), null);
  }
  if (!L.sized && TH === H) {
    const iv = $.NSImageView.alloc.initWithFrame($.NSMakeRect(0, 0, W, H));
    iv.imageScaling = 1; // NSImageScaleAxesIndependently: the window has its size
    iv.image = img;
    view.addSubview(iv);
  } else {
    // The image fitted into its box at the window's top.
    const pic = $.NSView.alloc.initWithFrame($.NSMakeRect(0, TH - H, W, H));
    pic.wantsLayer = true;
    pic.layer.contents = img;
    pic.layer.contentsGravity = !L.sized ? 'resize' : L.fit === 'contain' ? 'resizeAspect' : 'resizeAspectFill';
    pic.layer.masksToBounds = true;
    view.addSubview(pic);
  }
  const yb = edge ? 0 : bottom, bx = edge ? 0 : side, bw = edge ? W : W - 2 * side;
  const br = L.barRadius === null ? bp / 2 : L.barRadius;
  let track = null, fill = null, close = null;
  // Aligned by its frame, sized to its text: NSTextAlignment's numbers
  // differ by architecture (center is 1 on Apple silicon, 2 on Intel) and
  // JXA bridges Intel's.
  const lay = () => {
    const w = Math.min(Math.ceil(label.fittingSize.width) + 2, W - 2 * side);
    const x = L.align === 'center' ? Math.round((W - w) / 2) : L.align === 'right' ? W - side - w : side;
    label.frame = $.NSMakeRect(x, (bp > 0 && !edge ? yb + bp + 8 : bottom), w, lh);
  };
  if (label) {
    lay();
    view.addSubview(label);
  }
  const box = (w, col) => {
    const b = $.NSBox.alloc.initWithFrame($.NSMakeRect(bx, yb, w, bp));
    b.boxType = 4; // NSBoxCustom
    b.titlePosition = 0; // NSNoTitle
    b.borderWidth = 0;
    b.cornerRadius = br;
    b.fillColor = col;
    b.hidden = true;
    view.addSubview(b);
    return b;
  };
  if (bp > 0) {
    track = box(bw, hex(cs[3]));
    fill = box(0, hex(cs[2]));
    if (L.cancel) {
      // During a download, this cancels it. No target: its state is read below.
      close = $.NSButton.alloc.initWithFrame($.NSMakeRect(W - 30, TH - 28, 22, 22));
      close.title = '✕';
      close.bordered = false;
      close.setButtonType(1); // NSButtonTypePushOnPushOff: a click sets state 1
      close.hidden = true;
      view.addSubview(close);
    }
  }
  if (L.border) frameBox($.NSColor.clearColor, L.border); // over everything
  if (L.mode === 'pointer') {
    const { PH, areas } = aioAreas();
    const m = $.NSEvent.mouseLocation;
    const p = { x: m.x, y: PH - m.y };
    const s = areas.find((a) => p.x >= a.x && p.y >= a.y && p.x < a.x + a.width && p.y < a.y + a.height) || areas[0];
    const o = centredOn({ x: s.x, y: s.y, width: s.width, height: s.height, screen: s }, W, TH);
    win.setFrameOrigin($.NSMakePoint(o.x, PH - (o.y + TH)));
  } else win.setFrameOrigin(aioPlace(L.mode === 'window' ? L.target : null, W, TH));
  const PH0 = $.NSScreen.screens.objectAtIndex(0).frame.size.height;
  const X = win.frame.origin.x, Y = PH0 - (win.frame.origin.y + win.frame.size.height);
  // Closed while the helper was starting (the app stopped, or its start
  // failed): never shown.
  const pre = $.NSString.stringWithContentsOfFileEncodingError(file, $.NSUTF8StringEncoding, null);
  if (pre && !pre.isNil() && pre.js === 'close') $.exit(0);
  win.orderFrontRegardless; // shown, not activated: focus stays where it is
  $('shown splash size=' + W + 'x' + TH + ' scale=' + scale + ' dark=' + (isDark ? 1 : 0) +
    ' text=' + (hasText ? 1 : 0) + ' barpx=' + bp + ' at=' + X + ',' + Y + ' colors=' + cs.join(','))
    .writeToFileAtomicallyEncodingError(ack, true, $.NSUTF8StringEncoding, null);
  const turn = (sec) => {
    const ev = app.nextEventMatchingMaskUntilDateInModeDequeue(
      $.NSEventMaskAny, $.NSDate.dateWithTimeIntervalSinceNow(sec),
      $.NSDefaultRunLoopMode, true);
    if (ev && !ev.isNil()) app.sendEvent(ev);
  };
  const fade = () => {
    const n = Math.round(L.fadeMs / 16);
    for (let j = 0; j < n; j++) { win.alphaValue = 0.85 * (n - j) / n; win.displayIfNeeded; turn(0.016); }
    win.orderOut(null);
  };
  app.finishLaunching;
  // The event loop, by hand (ObjC.registerSubclass never returned on macOS
  // 14, aio measured): each turn waits up to 100 ms for an event, then ticks.
  for (;;) {
    turn(0.1);
    // JXA hands NSInteger back as a string ("1"), measured on macOS 14.
    if (close && Number(close.state) === 1) $.exit(3);
    // The app's window, where it really is: the splash moves onto it.
    if (atFile) {
      const av = $.NSString.stringWithContentsOfFileEncodingError(atFile, $.NSUTF8StringEncoding, null);
      if (av && !av.isNil() && av.js !== lastAt) {
        lastAt = av.js;
        win.setFrameOrigin(aioPlace(JSON.parse(lastAt), win.frame.size.width, win.frame.size.height, true));
      }
    }
    // The app that opened this window is gone: nobody will close it.
    if (++ticks % 30 === 0 && parent) {
      const k = $.NSTask.launchedTaskWithLaunchPathArguments('/bin/kill', ['-0', parent]);
      k.waitUntilExit;
      if (k.terminationStatus !== 0) $.exit(0);
    }
    const sv = $.NSString.stringWithContentsOfFileEncodingError(file, $.NSUTF8StringEncoding, null);
    if (!sv || sv.isNil()) continue;
    const str = sv.js;
    if (str === 'close') { fade(); $.exit(0); }
    const i = str.indexOf('\t');
    if (i < 0) continue;
    if (label && label.stringValue.js !== str.slice(i + 1)) { label.stringValue = str.slice(i + 1); lay(); }
    if (track) {
      const n = parseInt(str.slice(0, i), 10);
      const on = n >= 0;
      track.hidden = !on; fill.hidden = !on; if (close) close.hidden = !on;
      if (on) fill.frame = $.NSMakeRect(bx, yb, Math.round(bw * Math.min(100, n) / 100), bp);
    }
  }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use aio::config::SplashShow;

    fn splash(w: u32, h: u32) -> Splash {
        Splash { image: "assets/splash.png".into(), png: crate::splash::tests::png(w, h), ..Default::default() }
    }

    /// A 480×300 gradient: something to see in a screenshot.
    fn picture() -> &'static [u8] {
        let (w, h) = (480u32, 300u32);
        let rgba: Vec<u8> = (0..w * h)
            .flat_map(|i| {
                let (x, y) = (i % w, i / w);
                [(40 + x * 120 / w) as u8, (60 + y * 100 / h) as u8, 170, 255]
            })
            .collect();
        Box::leak(aio_config::icon::png(&rgba, w, h).into_boxed_slice())
    }

    fn r(x: i32, y: i32, width: i32, height: i32) -> Rect {
        Rect { x, y, width, height }
    }

    /// aio's `helperLook`: every "not set" is the OS's own default.
    #[test]
    fn the_look_resolves_each_os_default() {
        let s = splash(640, 400);
        let l = look(&s, "linux", None).unwrap();
        assert_eq!((l.sized, l.w, l.h, l.iw, l.ih), (false, 640, 400, 640, 400));
        assert_eq!((l.side, l.bottom, l.fade_ms, l.radius, l.cancel), (24, 18, 100, 0, true));
        assert_eq!((l.shadow, l.window_shadow, l.bp, l.below), (true, false, 8, true), "solid: 6 pt at 96 dpi");
        let mac = look(&s, "macos", None).unwrap();
        assert_eq!((mac.shadow, mac.window_shadow), (true, true));
        let win = look(&s, "windows", None).unwrap();
        assert_eq!((win.shadow, win.window_shadow), (false, false));
        // Every key the app set is exactly what it set.
        let set = Splash {
            size: Some([480, 300]),
            margin: Some(10),
            shadow: Some(false),
            window_shadow: Some(true),
            bar_style: BarStyle::Thin,
            bar_position: BarPosition::BottomEdge,
            bar_radius: Some(0),
            border: Some("#abc".into()),
            fade_ms: Some(250),
            cancel: false,
            position: SplashPosition::Pointer,
            text_position: TextPosition::Over,
            ..s.clone()
        };
        let l = look(&set, "windows", Some((Some(5), Some(6), 900, 700))).unwrap();
        assert_eq!((l.sized, l.w, l.h, l.side, l.bottom, l.bp), (true, 480, 300, 10, 10, 4));
        assert_eq!((l.shadow, l.window_shadow, l.bar_edge, l.bar_radius, l.fade_ms), (false, true, true, Some(0), 250));
        assert_eq!(
            (l.border.as_deref(), l.cancel, l.mode, l.below),
            (Some("#aabbcc"), false, SplashPosition::Pointer, false)
        );
        assert_eq!(look(&Splash { bar_height: Some(5), ..s.clone() }, "linux", None).unwrap().bp, 5);
        assert_eq!(look(&Splash { progress: false, ..s.clone() }, "linux", None).unwrap().bp, 0, "no bar");
        // An image larger than the box is scaled down, its aspect kept.
        let big = look(&splash(2560, 1440), "linux", None).unwrap();
        assert_eq!((big.sized, big.w, big.h), (true, 1092, 614));
        // @2x: half its pixels.
        let hi = look(&Splash { image: "a/s@2x.png".into(), ..splash(1280, 800) }, "macos", None).unwrap();
        assert_eq!((hi.w, hi.h, hi.sized), (640, 400, false));
        assert!(look(&Splash { png: b"GIF89a", ..s }, "linux", None).is_none(), "not a PNG");
        // What the helpers read: camelCase JSON.
        let json = aio::serde_json::to_value(&l).unwrap();
        assert_eq!((json["barEdge"].as_bool(), json["mode"].as_str()), (Some(true), Some("pointer")));
        assert_eq!(json["target"], aio::serde_json::json!([5, 6, 900, 700]));
    }

    /// The app's window target: the saved size while the declared one is
    /// unchanged, the saved position whenever there is one.
    #[test]
    fn the_target_is_where_the_window_opens() {
        let saved = crate::place::Saved {
            normal: r(100, 50, 1000, 700),
            declared_width: 1180,
            declared_height: 800,
            ..Default::default()
        };
        assert_eq!(crate::splash::target(Some(&saved), 1180, 800), (Some(100), Some(50), 1000, 700));
        assert_eq!(crate::splash::target(Some(&saved), 900, 600), (Some(100), Some(50), 900, 600));
        assert_eq!(crate::splash::target(None, 900, 600), (None, None, 900, 600));
        let tiny = crate::place::Saved { normal: r(1, 1, 10, 10), ..saved };
        assert_eq!(crate::splash::target(Some(&tiny), 900, 600), (None, None, 900, 600), "a broken saved size is none");
    }

    /// aio's `splashFrame`, `fitRect` and `roundedRows`.
    #[test]
    fn the_parts_are_laid_out_as_on_every_os() {
        let l = look(&splash(400, 200), "linux", None).unwrap();
        // Below: the strip holds the text and the bar.
        assert_eq!(
            frame(&l, 20),
            Frame { th: 200 + 8 + 20 + 18 + 8 + 8, yb: 262 - 18 - 8, bx: 24, bw: 352, label_bottom: 228 }
        );
        assert_eq!(frame(&l, 0).th, 200, "no text: no strip");
        let over = Look { below: false, bar_edge: true, ..l.clone() };
        assert_eq!(frame(&over, 20), Frame { th: 200, yb: 192, bx: 0, bw: 400, label_bottom: 182 });
        let nobar = Look { below: false, bp: 0, ..l };
        assert_eq!(frame(&nobar, 20).label_bottom, 182);
        assert_eq!(fit_rect(400, 200, 200, 200, Fit::Contain), r(0, 50, 200, 100));
        assert_eq!(fit_rect(400, 200, 200, 200, Fit::Cover), r(-100, 0, 400, 200));
        assert_eq!(rounded_rows(10, 10, 0), vec![r(0, 0, 10, 10)]);
        let rows = rounded_rows(100, 50, 8);
        assert_eq!(rows.len(), 17);
        assert_eq!((rows[0].x, rows[0].width, rows[1].y), (5, 90, 49), "the top row inset most");
        assert_eq!(rows.last(), Some(&r(0, 8, 100, 34)));
    }

    /// aio's `splashOrigin`: on the app's window as it opens, the primary's
    /// centre, or the pointer's screen; kept 64 px inside.
    #[test]
    fn the_splash_opens_where_the_window_will() {
        let primary = r(0, 0, 1920, 1040);
        let second = r(1920, 0, 1280, 1000);
        let screens = [primary, second];
        let l = look(&splash(400, 200), "linux", None).unwrap();
        assert_eq!(origin(&l, &screens, primary, None, (400, 200)), (760, 420), "no target: the primary's centre");
        let on = Look { target: Some((Some(2000), Some(100), 800, 600)), ..l.clone() };
        assert_eq!(origin(&on, &screens, primary, None, (400, 200)), (2000 + 400 - 200, 100 + 300 - 100));
        // A target with no saved position: the window opens centred on the primary.
        let unplaced = Look { target: Some((None, None, 800, 600)), ..l.clone() };
        assert_eq!(origin(&unplaced, &screens, primary, None, (400, 200)), (760, 420));
        // Kept inside its screen's margin.
        let edge = Look { target: Some((Some(-500), Some(-500), 300, 300)), ..l.clone() };
        let (x, y) = origin(&edge, &screens, primary, None, (400, 200));
        assert!(x >= 64 && y >= 64, "{x},{y}");
        let ptr = Look { mode: SplashPosition::Pointer, ..l.clone() };
        assert_eq!(origin(&ptr, &screens, primary, Some((2500, 10)), (400, 200)), (1920 + 440, 400));
        let prim = Look { mode: SplashPosition::Primary, ..on };
        assert_eq!(origin(&prim, &screens, primary, None, (400, 200)), (760, 420));
        // Followed: centred on the window where it really is.
        assert_eq!(onto([2000, 100, 800, 600], &screens, primary, (400, 200)), (2200, 300));
    }

    const MIN: u64 = 500;
    const CAP: u64 = 10_000;

    fn run(events: &[Event]) -> State {
        events.iter().fold(State::default(), |s, e| step(s, *e, MIN, CAP))
    }

    /// aio's `splashStep`: two timelines, the splash's end gates nothing.
    #[test]
    fn the_splash_goes_once_ready_and_up_its_minimum() {
        use Event::*;
        // Ready early: it stays until it has been visible MIN.
        let s = run(&[Shown(100), Ready(200)]);
        assert_eq!(s.closed, None);
        assert_eq!(step(s, Tick(599), MIN, CAP).closed, None);
        assert_eq!(step(s, Tick(600), MIN, CAP).closed, Some(Close::Ready));
        // Ready after its minimum: it goes at once.
        assert_eq!(run(&[Shown(100), Ready(900)]).closed, Some(Close::Ready));
        // Ready before it was ever on screen: nothing to cover, it goes.
        assert_eq!(run(&[Ready(50)]).closed, Some(Close::Ready));
        // The cap: never during a download, counted from its end.
        assert_eq!(run(&[Shown(10), Tick(CAP)]).closed, Some(Close::Cap));
        assert_eq!(run(&[Shown(10), Download(true, 20), Tick(CAP * 3)]).closed, None);
        let s = run(&[Shown(10), Download(true, 20), Download(false, 50_000)]);
        assert_eq!(step(s, Tick(50_000 + CAP - 1), MIN, CAP).closed, None);
        assert_eq!(step(s, Tick(50_000 + CAP), MIN, CAP).closed, Some(Close::Cap));
        // The other ends close it at once, and closed stays closed.
        for c in [Close::Exit, Close::Dismissed, Close::Failed, Close::Lost] {
            let s = run(&[Shown(1), End(c)]);
            assert_eq!(s.closed, Some(c));
            assert_eq!(run(&[Shown(1), End(c), Ready(2), End(Close::Exit), Tick(CAP * 9)]).closed, Some(c));
        }
    }

    fn quick() -> SplashShow {
        SplashShow::FirstStart
    }
    fn panics() -> SplashShow {
        panic!("no settings")
    }
    fn slow() -> SplashShow {
        std::thread::sleep(crate::splash::RUN_TIME_SHOW + Duration::from_millis(20));
        SplashShow::Never
    }

    /// aio's `withRunTimeShow`: the app's say, unless it fails or is slow.
    #[test]
    fn the_app_has_its_say_at_run_time() {
        assert_eq!(crate::splash::run_time_show(None, SplashShow::Always), (SplashShow::Always, false));
        assert_eq!(crate::splash::run_time_show(Some(quick), SplashShow::Always), (SplashShow::FirstStart, true));
        assert_eq!(crate::splash::run_time_show(Some(panics), SplashShow::Always), (SplashShow::Always, false));
        assert_eq!(crate::splash::run_time_show(Some(slow), SplashShow::FirstStart), (SplashShow::FirstStart, false));
        assert_eq!(crate::splash::show_name(SplashShow::FirstStart), "first-start");
    }

    #[test]
    fn each_os_has_its_helper_and_a_contained_run_none() {
        let env = |k: &str| (k == "DISPLAY").then(|| ":9".to_string());
        assert_eq!(kind("linux", &env), Some(Kind::Gtk));
        assert_eq!(kind("macos", &|_| None), Some(Kind::Cocoa));
        assert_eq!(kind("windows", &|_| None), Some(Kind::Forms));
        assert_eq!(kind("linux", &|_| None), None, "no display");
        assert_eq!(kind("macos", &|k| (k == "AIORT_DIALOG").then(|| "none".into())), None);
        assert_eq!(line(Some(140), "a\tb\nc"), "100\ta b c");
        assert_eq!(line(None, "x"), "-1\tx");
        assert_eq!(start_text(Some("{title} {version}"), "Notes", "1.2.0"), "Notes 1.2.0");
        assert_eq!(
            crate::splash::rect_of(&aio::serde_json::json!({"x": 1, "y": 2.4, "width": 3, "height": 4})),
            Some([1, 2, 3, 4])
        );
    }

    fn plan<'a>(s: &'a Splash, colors: &'a [[String; 4]; 2], at_start: bool) -> Plan<'a> {
        Plan {
            splash: s,
            title: "Notes",
            version: "1.0.0",
            colors,
            target: None,
            at_start,
            by_app: false,
            may_download: true,
        }
    }

    /// Opened only for a download on Windows (Electron draws the start's) and
    /// with `first-start`: no download, so Electron's main process (or no
    /// one) has the start, and nothing was opened.
    #[test]
    fn a_splash_that_waits_for_a_download_hands_the_start_back() {
        let s = splash(64, 40);
        let c = crate::splash::colors(210, None);
        for (kind, at_start) in [(Kind::Forms, true), (Kind::Gtk, false), (Kind::Cocoa, false)] {
            let n = Session::start(&plan(&s, &c, at_start), kind, "linux").unwrap();
            assert!(n.lock().view.is_none(), "{kind:?}: not opened");
            assert!(n.handoff(), "{kind:?}: Electron draws the start's");
            assert!(current().is_none() && !n.download(), "{kind:?}: done");
        }
    }

    /// GTK in a private Xvfb (a child test process, its own display): the
    /// splash shows, the download draws its bar, its ✕ cancels the
    /// download, and a ready app's splash goes after its minimum.
    /// `AIO_SPLASH_SHOT=<file.png>`: a screenshot of it with the bar.
    #[cfg(target_os = "linux")]
    #[test]
    fn gtk_draws_the_splash_before_electron() {
        if std::env::var_os("AIO_GTK_CHILD").is_some() {
            return gtk_child();
        }
        let Some(xvfb) = crate::firstrun::on_path("Xvfb") else {
            return eprintln!("skipped: no Xvfb");
        };
        use std::io::BufRead;
        let mut x = std::process::Command::new(xvfb)
            .args(["-displayfd", "1", "-nolisten", "tcp", "-screen", "0", "1280x800x24"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut n = String::new();
        std::io::BufReader::new(x.stdout.take().unwrap()).read_line(&mut n).unwrap();
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["presplash::tests::gtk_draws_the_splash_before_electron", "--exact", "--nocapture"])
            .env("AIO_GTK_CHILD", "1")
            .env("DISPLAY", format!(":{}", n.trim()))
            .env_remove("WAYLAND_DISPLAY")
            .output()
            .unwrap();
        _ = x.kill();
        _ = x.wait();
        let said = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{said}\n{}", String::from_utf8_lossy(&out.stdout));
        assert!(said.contains("[aio] splash: visible") && said.contains("shown splash size="), "{said}");
    }

    #[cfg(target_os = "linux")]
    fn gtk_child() {
        let until = |what: &str, ok: &dyn Fn() -> bool| {
            let t = Instant::now();
            while !ok() {
                assert!(t.elapsed() < Duration::from_secs(20), "never {what}");
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        let s = Splash {
            text: Some("Starting {title}".into()),
            min_ms: 400,
            radius: 10,
            border: Some("#888".into()),
            png: picture(),
            ..splash(1, 1)
        };
        let c = crate::splash::look_colors(crate::splash::colors(210, None), &s);
        let n = Session::start(&plan(&s, &c, true), Kind::Gtk, "linux").unwrap();
        until("shown", &|| n.state().shown_at.is_some());
        assert!(n.download());
        assert_eq!(n.progress(Some(40), "Getting Notes ready · 40 of 100 MB"), Shown::On);
        if let Some(shot) = std::env::var_os("AIO_SPLASH_SHOT") {
            std::thread::sleep(Duration::from_millis(300));
            let d = std::env::var("DISPLAY").unwrap();
            let ok = std::process::Command::new("ffmpeg")
                .args([
                    "-loglevel",
                    "error",
                    "-y",
                    "-f",
                    "x11grab",
                    "-video_size",
                    "1280x800",
                    "-i",
                    &d,
                    "-frames:v",
                    "1",
                ])
                .arg(&shot)
                .status()
                .is_ok_and(|s| s.success());
            assert!(ok, "the screenshot");
        }
        n.click();
        until("dismissed", &|| n.progress(Some(50), "x") == Shown::Cancelled);
        until("gone", &|| current().is_none());
        // A start with Electron here: the app's window shows, the splash
        // stays its minimum, then goes.
        let n = Session::start(&plan(&s, &c, true), Kind::Gtk, "linux").unwrap();
        until("shown again", &|| n.state().shown_at.is_some());
        assert!(!n.handoff(), "it is up: Electron draws none");
        ready(Some([100, 100, 800, 600]));
        until("closed", &|| current().is_none());
        let st = n.state();
        assert_eq!(st.closed, Some(Close::Ready));
        assert!(n.now() - st.shown_at.unwrap() >= s.min_ms, "it stayed its minimum");
    }

    /// The Cocoa and Windows Forms helpers on a real desktop
    /// (`AIO_SPLASH_LIVE=1`: the Mac, the Windows VM): shown, the bar, the
    /// close. `AIO_SPLASH_HOLD_MS`: it stays up that long with its bar.
    #[cfg(any(target_os = "macos", windows))]
    #[test]
    fn the_os_helper_draws_the_splash() {
        if std::env::var_os("AIO_SPLASH_LIVE").is_none() {
            return eprintln!("skipped: AIO_SPLASH_LIVE is not set");
        }
        let os = std::env::consts::OS;
        let s = Splash { text: Some("Starting {title}".into()), radius: 10, png: picture(), ..splash(1, 1) };
        let c = crate::splash::look_colors(crate::splash::colors(210, None), &s);
        let n = Session::start(&plan(&s, &c, true), kind(os, &|_| None).unwrap(), os).unwrap();
        assert!(n.download(), "Windows: opened for the download");
        let t = Instant::now();
        while n.state().shown_at.is_none() {
            assert!(t.elapsed() < Duration::from_secs(30) && n.state().closed.is_none(), "never shown");
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(n.progress(Some(40), "Getting Notes ready · 40 of 100 MB"), Shown::On);
        let hold = std::env::var("AIO_SPLASH_HOLD_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(500);
        std::thread::sleep(Duration::from_millis(hold));
        n.downloaded();
        ready(None);
        let t = Instant::now();
        while current().is_some() {
            assert!(t.elapsed() < Duration::from_secs(10), "never closed");
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(n.state().closed, Some(Close::Ready));
    }
}

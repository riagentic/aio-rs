//! The Linux splash drawn by the app itself: GTK 3 over FFI
//! (`libgtk-3.so.0`, which every machine that runs Electron has, as
//! Electron links it), opened with `dlopen` (no crate, no link-time
//! dependency: a machine without GTK just has no native splash), on its own
//! thread so the start never waits on a redraw (aio's `gtk-splash.ts` and
//! `gtk-splash-worker.ts`). Borderless, above other windows, no taskbar
//! entry, where the app's window will open, never takes focus; the image at
//! its point size (`@2x` halved) or fitted to `size`, the text and the bar
//! placed as Windows and macOS place them ([`crate::presplash::frame`]), in
//! the splash's font and colours; a ✕ cancels a download.
//!
//! GTK is touched from one thread only, for the process's life: it starts
//! once (GTK initialises once per process) and draws one window at a time.
//! The library is never closed: GTK leaves threads and callbacks running
//! its code after its window is gone (aio measured the segfault).
use crate::presplash::{Open, Status, fit_rect, frame, origin, rounded_rows};
use std::ffi::{CString, c_char, c_int, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, channel, sync_channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

type P = *mut c_void;
type S = *const c_char;

/// The symbols, looked up in libgtk and the libraries it loads (GDK,
/// GdkPixbuf, cairo, GObject), as aio's worker declares them.
macro_rules! gtk_fns {
    ($($name:ident: fn($($a:ty),*) $(-> $r:ty)?;)*) => {
        #[allow(non_snake_case, dead_code)]
        struct Gtk { $($name: unsafe extern "C" fn($($a),*) $(-> $r)?,)* }
        impl Gtk {
            /// SAFETY: `h` is a live `dlopen` handle; each symbol has the C
            /// signature written here.
            unsafe fn load(h: P) -> Result<Self, String> {
                Ok(Self { $($name: {
                    // SAFETY: `h` is a live dlopen handle (`load`'s contract) and the name a NUL-terminated literal.
                    let p = unsafe { libc::dlsym(h, concat!(stringify!($name), "\0").as_ptr().cast()) };
                    if p.is_null() {
                        return Err(format!("libgtk-3.so.0 has no {}", stringify!($name)));
                    }
                    // SAFETY: `p` is the non-null address of the C function named, whose signature is the one written (`load`'s contract); fn pointers and data pointers have one size here.
                    unsafe { std::mem::transmute::<P, unsafe extern "C" fn($($a),*) $(-> $r)?>(p) }
                },)* })
            }
        }
    };
}

gtk_fns! {
    gtk_disable_setlocale: fn();
    gtk_init_check: fn(P, P) -> c_int;
    gtk_events_pending: fn() -> c_int;
    gtk_main_iteration_do: fn(c_int) -> c_int;
    gtk_window_new: fn(c_int) -> P;
    gtk_window_set_title: fn(P, S);
    gtk_window_set_decorated: fn(P, c_int);
    gtk_window_set_skip_taskbar_hint: fn(P, c_int);
    gtk_window_set_skip_pager_hint: fn(P, c_int);
    gtk_window_set_keep_above: fn(P, c_int);
    gtk_window_set_type_hint: fn(P, c_int);
    gtk_window_set_resizable: fn(P, c_int);
    gtk_window_set_accept_focus: fn(P, c_int);
    gtk_window_set_focus_on_map: fn(P, c_int);
    gtk_window_set_default_size: fn(P, c_int, c_int);
    gtk_window_move: fn(P, c_int, c_int);
    gtk_widget_shape_combine_region: fn(P, P);
    gtk_widget_set_name: fn(P, S);
    gtk_widget_set_size_request: fn(P, c_int, c_int);
    gtk_widget_get_preferred_height: fn(P, *mut c_int, *mut c_int);
    gtk_widget_show_all: fn(P);
    gtk_widget_show: fn(P);
    gtk_widget_hide: fn(P);
    gtk_widget_set_no_show_all: fn(P, c_int);
    gtk_widget_get_mapped: fn(P) -> c_int;
    gtk_widget_get_scale_factor: fn(P) -> c_int;
    gtk_widget_set_opacity: fn(P, f64);
    gtk_widget_destroy: fn(P);
    gtk_widget_get_window: fn(P) -> P;
    gtk_container_add: fn(P, P);
    gtk_fixed_new: fn() -> P;
    gtk_fixed_put: fn(P, P, c_int, c_int);
    gtk_fixed_move: fn(P, P, c_int, c_int);
    gtk_image_new_from_surface: fn(P) -> P;
    gtk_label_new: fn(S) -> P;
    gtk_label_set_text: fn(P, S);
    gtk_label_set_xalign: fn(P, f32);
    gtk_label_set_ellipsize: fn(P, c_int);
    gtk_box_new: fn(c_int, c_int) -> P;
    gtk_button_new_with_label: fn(S) -> P;
    gtk_button_set_relief: fn(P, c_int);
    gtk_button_clicked: fn(P);
    gtk_css_provider_new: fn() -> P;
    gtk_css_provider_load_from_data: fn(P, S, isize, P) -> c_int;
    gtk_style_context_add_provider_for_screen: fn(P, P, u32);
    gtk_settings_get_default: fn() -> P;
    gdk_screen_get_default: fn() -> P;
    gdk_window_raise: fn(P);
    gdk_window_get_origin: fn(P, *mut c_int, *mut c_int) -> c_int;
    gdk_display_get_default: fn() -> P;
    gdk_display_open: fn(S) -> P;
    gdk_display_manager_get: fn() -> P;
    gdk_display_manager_set_default_display: fn(P, P);
    gdk_display_get_n_monitors: fn(P) -> c_int;
    gdk_display_get_monitor: fn(P, c_int) -> P;
    gdk_display_get_primary_monitor: fn(P) -> P;
    gdk_monitor_get_workarea: fn(P, *mut [c_int; 4]);
    gdk_display_get_default_seat: fn(P) -> P;
    gdk_seat_get_pointer: fn(P) -> P;
    gdk_device_get_position: fn(P, P, *mut c_int, *mut c_int);
    gdk_cairo_surface_create_from_pixbuf: fn(P, c_int, P) -> P;
    gdk_pixbuf_loader_new: fn() -> P;
    gdk_pixbuf_loader_write: fn(P, *const u8, usize, P) -> c_int;
    gdk_pixbuf_loader_close: fn(P, P) -> c_int;
    gdk_pixbuf_loader_get_pixbuf: fn(P) -> P;
    gdk_pixbuf_scale_simple: fn(P, c_int, c_int, c_int) -> P;
    gdk_pixbuf_new_subpixbuf: fn(P, c_int, c_int, c_int, c_int) -> P;
    cairo_surface_destroy: fn(P);
    cairo_region_create: fn() -> P;
    cairo_region_union_rectangle: fn(P, *const [c_int; 4]) -> c_int;
    cairo_region_destroy: fn(P);
    g_object_unref: fn(P);
    g_free: fn(P);
    g_signal_connect_data: fn(P, S, P, P, P, c_int) -> u64;
}

enum Cmd {
    Open(Box<Open>, Arc<Mutex<Status>>),
    Update(Option<u64>, String),
    Move([i32; 4]),
    /// Clicks the ✕ through GTK's own "clicked" signal (tests: no pointer).
    #[cfg(test)]
    Click,
    /// Fades out over this many ms, then goes.
    Close(u64, SyncSender<()>),
}

/// The open window, drawn on GTK's thread.
pub(crate) struct Window {
    tx: Sender<Cmd>,
}

impl Window {
    pub(crate) fn update(&self, pct: Option<u64>, text: &str) {
        _ = self.tx.send(Cmd::Update(pct, text.into()));
    }
    pub(crate) fn move_to(&self, r: [i32; 4]) {
        _ = self.tx.send(Cmd::Move(r));
    }
    #[cfg(test)]
    pub(crate) fn click(&self) {
        _ = self.tx.send(Cmd::Click);
    }
    /// Fades it out and waits until it is gone (2 s at most past the fade).
    pub(crate) fn close(self, fade_ms: u64) {
        let (tx, rx) = sync_channel(1);
        if self.tx.send(Cmd::Close(fade_ms, tx)).is_ok() {
            _ = rx.recv_timeout(Duration::from_millis(fade_ms) + Duration::from_secs(2));
        }
    }
}

/// GTK's thread, up with the display open, or why GTK cannot draw here.
static THREAD: OnceLock<Result<Mutex<Sender<Cmd>>, String>> = OnceLock::new();

/// How long GTK has to say whether it can draw.
const INIT: Duration = Duration::from_secs(5);

/// Open the splash: `Err` says why GTK cannot draw here (no library, no
/// display); the window's news comes in `status`.
pub(crate) fn open(o: Open, status: Arc<Mutex<Status>>) -> Result<Window, String> {
    let tx =
        THREAD.get_or_init(start).as_ref().map_err(Clone::clone)?.lock().unwrap_or_else(|e| e.into_inner()).clone();
    tx.send(Cmd::Open(Box::new(o), status)).map_err(|_| "GTK's thread is gone".to_string())?;
    Ok(Window { tx })
}

fn start() -> Result<Mutex<Sender<Cmd>>, String> {
    let (tx, rx) = channel();
    let (ok_tx, ok_rx) = sync_channel(1);
    std::thread::Builder::new()
        .name("aio-gtk-splash".into())
        .spawn(move || match init() {
            Ok(g) => {
                _ = ok_tx.send(Ok(()));
                serve(&g, &rx);
            }
            Err(e) => _ = ok_tx.send(Err(e)),
        })
        .map_err(|e| format!("its thread did not start ({e})"))?;
    match ok_rx.recv_timeout(INIT) {
        Ok(r) => r.map(|()| Mutex::new(tx)),
        Err(_) => Err(format!("GTK did not answer within {} s", INIT.as_secs())),
    }
}

fn init() -> Result<Gtk, String> {
    // SAFETY: dlopen of a system library by name; never closed (see the module).
    let h = unsafe { libc::dlopen(c"libgtk-3.so.0".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
    if h.is_null() {
        // SAFETY: dlerror's text, read at once.
        let why = unsafe { libc::dlerror() };
        let why = if why.is_null() {
            String::new()
        } else {
            // SAFETY: dlerror's text is a NUL-terminated string, non-null here, read before any other dl call.
            unsafe { std::ffi::CStr::from_ptr(why) }.to_string_lossy().into_owned()
        };
        return Err(format!("libgtk-3.so.0 could not be opened ({why})"));
    }
    // SAFETY: the handle is live; the signatures are GTK 3's.
    let g = unsafe { Gtk::load(h) }?;
    // SAFETY: the table was just loaded from a live handle; GTK's first calls, on this thread, which stays GTK's only one.
    unsafe {
        // The app's process keeps its C locale: GTK's own setlocale would
        // change it for every thread.
        (g.gtk_disable_setlocale)();
        // GTK initialises once per process: after a failed start a second
        // gtk_init_check says yes with no display behind it, so the display
        // is what is checked, and opened when an earlier try could not.
        let mut display = if (g.gtk_init_check)(std::ptr::null_mut(), std::ptr::null_mut()) != 0 {
            (g.gdk_display_get_default)()
        } else {
            std::ptr::null_mut()
        };
        if display.is_null() && (g.gtk_init_check)(std::ptr::null_mut(), std::ptr::null_mut()) != 0 {
            display = (g.gdk_display_open)(std::ptr::null());
            if !display.is_null() {
                (g.gdk_display_manager_set_default_display)((g.gdk_display_manager_get)(), display);
            }
        }
        if display.is_null() {
            let d = std::env::var("WAYLAND_DISPLAY").or_else(|_| std::env::var("DISPLAY")).unwrap_or_default();
            return Err(format!("GTK could not open the display {d}").trim_end().into());
        }
    }
    Ok(g)
}

/// The ✕ or the window manager's close (Alt+F4): the user dismissed it.
static DISMISSED: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn on_delete(_: P, _: P, _: P) -> c_int {
    DISMISSED.store(true, Ordering::Relaxed);
    1 // kept: the session closes it
}

unsafe extern "C" fn on_click(_: P, _: P) {
    DISMISSED.store(true, Ordering::Relaxed);
}

fn z(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

/// One window's widgets.
struct Drawn {
    win: P,
    label: P,
    track: P,
    fill: P,
    close: P,
    bw: i32,
    bp: i32,
    size: (i32, i32),
    status: Arc<Mutex<Status>>,
    t0: Instant,
    shown: bool,
    ticks: u32,
    what: String,
}

fn serve(g: &Gtk, rx: &Receiver<Cmd>) {
    let mut cur: Option<Drawn> = None;
    loop {
        let cmd = match &cur {
            None => match rx.recv() {
                Ok(c) => Some(c),
                Err(_) => return,
            },
            Some(_) => match rx.recv_timeout(Duration::from_millis(16)) {
                Ok(c) => Some(c),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
                Err(_) => return,
            },
        };
        // SAFETY: every call below is on this, GTK's only, thread.
        unsafe {
            match cmd {
                Some(Cmd::Open(o, status)) => {
                    if let Some(d) = cur.take() {
                        destroy(g, d.win);
                    }
                    DISMISSED.store(false, Ordering::Relaxed);
                    match build(g, &o, status.clone()) {
                        Ok(d) => cur = Some(d),
                        Err(why) => lock(&status).lost = Some(why),
                    }
                }
                Some(Cmd::Update(pct, text)) => {
                    if let Some(d) = &cur {
                        update(g, d, pct, &text);
                    }
                }
                Some(Cmd::Move(r)) => {
                    if let Some(d) = &cur {
                        let (screens, primary) = screens(g);
                        let o = crate::presplash::onto(r, &screens, primary.unwrap_or(rect(r)), d.size);
                        (g.gtk_window_move)(d.win, o.0, o.1);
                    }
                }
                #[cfg(test)]
                Some(Cmd::Click) => {
                    if let Some(d) = cur.as_ref().filter(|d| !d.close.is_null()) {
                        (g.gtk_button_clicked)(d.close);
                    }
                }
                Some(Cmd::Close(fade_ms, done)) => {
                    if let Some(d) = cur.take() {
                        let n = (fade_ms / 16) as i32;
                        for j in 0..n {
                            (g.gtk_widget_set_opacity)(d.win, 0.85 * f64::from(n - j) / f64::from(n));
                            pump(g);
                            std::thread::sleep(Duration::from_millis(16));
                        }
                        destroy(g, d.win);
                    }
                    _ = done.send(());
                }
                None => {}
            }
            pump(g);
            if let Some(d) = &mut cur
                && !tick(g, d)
            {
                destroy(g, d.win);
                cur = None;
            }
        }
    }
}

fn lock(s: &Mutex<Status>) -> std::sync::MutexGuard<'_, Status> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

/// Every pending GTK event, handled now: never waits.
unsafe fn pump(g: &Gtk) {
    // SAFETY: `pump`'s contract: on GTK's thread, with the table loaded.
    unsafe {
        for _ in 0..500 {
            if (g.gtk_events_pending)() == 0 {
                break;
            }
            (g.gtk_main_iteration_do)(0);
        }
    }
}

unsafe fn destroy(g: &Gtk, win: P) {
    // SAFETY: `destroy`'s contract: on GTK's thread, `win` a live widget made by `build`.
    unsafe {
        (g.gtk_widget_destroy)(win);
        pump(g);
    }
}

/// One turn: on screen yet (said once), kept on top; false: it never
/// appeared and is given up.
unsafe fn tick(g: &Gtk, d: &mut Drawn) -> bool {
    // SAFETY: `tick`'s contract: on GTK's thread; `d`'s widgets are live until `destroy`.
    unsafe {
        if DISMISSED.load(Ordering::Relaxed) {
            lock(&d.status).cancelled = true;
        }
        if d.shown {
            // Back on top of the app's window, which shows under it: keep-above
            // is a request a window manager may ignore (and with none, nothing
            // keeps it), so it is re-raised; a restack only, never a focus change.
            d.ticks += 1;
            if d.ticks.is_multiple_of(6) {
                (g.gdk_window_raise)((g.gtk_widget_get_window)(d.win));
            }
            return true;
        }
        if (g.gtk_widget_get_mapped)(d.win) != 0 {
            d.shown = true;
            let (mut x, mut y) = (0, 0);
            (g.gdk_window_get_origin)((g.gtk_widget_get_window)(d.win), &mut x, &mut y);
            let scale = (g.gtk_widget_get_scale_factor)(d.win);
            lock(&d.status).shown = Some(format!("{} device-scale={scale} at={x},{y}", d.what));
            true
        } else if d.t0.elapsed() > Duration::from_secs(5) {
            // A window that never maps (a compositor refusing it, a broken
            // display): said, never waited on.
            lock(&d.status).lost = Some("the GTK window never appeared within 5 s".into());
            false
        } else {
            true
        }
    }
}

unsafe fn update(g: &Gtk, d: &Drawn, pct: Option<u64>, text: &str) {
    // SAFETY: `update`'s contract: on GTK's thread; `d`'s widgets are live (each null one skipped); `z` keeps the text's CString alive for the call.
    unsafe {
        if !d.label.is_null() {
            (g.gtk_label_set_text)(d.label, z(text).as_ptr());
        }
        if d.track.is_null() {
            return;
        }
        for w in [d.track, d.fill, d.close] {
            if !w.is_null() {
                if pct.is_some() { (g.gtk_widget_show)(w) } else { (g.gtk_widget_hide)(w) }
            }
        }
        if let Some(p) = pct {
            let fw = (f64::from(d.bw) * p.min(100) as f64 / 100.0).round() as i32;
            (g.gtk_widget_set_size_request)(d.fill, fw, d.bp);
            if fw > 0 { (g.gtk_widget_show)(d.fill) } else { (g.gtk_widget_hide)(d.fill) }
        }
    }
}

fn rect([x, y, width, height]: [i32; 4]) -> crate::place::Rect {
    crate::place::Rect { x, y, width, height }
}

/// The display's monitors' work areas and its primary (the first when none
/// is marked).
unsafe fn screens(g: &Gtk) -> (Vec<crate::place::Rect>, Option<crate::place::Rect>) {
    // SAFETY: `screens`'s contract: on GTK's thread after init (a default display); each monitor pointer is GDK's, valid for the call.
    unsafe {
        let d = (g.gdk_display_get_default)();
        let area = |m: P| {
            let mut r = [0; 4];
            (g.gdk_monitor_get_workarea)(m, &mut r);
            rect(r)
        };
        let all: Vec<_> =
            (0..(g.gdk_display_get_n_monitors)(d)).map(|i| area((g.gdk_display_get_monitor)(d, i))).collect();
        let pm = (g.gdk_display_get_primary_monitor)(d);
        let primary = if pm.is_null() { all.first().copied() } else { Some(area(pm)) };
        (all, primary)
    }
}

/// The GTK theme is dark: the app asks for it, or its name says so, or
/// `GTK_THEME` ends in `:dark`.
unsafe fn theme_dark(g: &Gtk) -> bool {
    let env = std::env::var("GTK_THEME").ok();
    // SAFETY: `theme_dark`'s contract: on GTK's thread. g_object_get is looked up from the library already loaded (RTLD_NOLOAD), its variadic C signature written out, and called with property-name/out-pointer pairs ending in NULL.
    unsafe {
        let s = (g.gtk_settings_get_default)();
        if s.is_null() {
            return crate::firstrun::linux_dark(env.as_deref(), "");
        }
        // g_object_get is variadic: looked up apart from the table.
        let h = libc::dlopen(c"libgtk-3.so.0".as_ptr(), libc::RTLD_NOW | libc::RTLD_NOLOAD);
        let get = if h.is_null() { h } else { libc::dlsym(h, c"g_object_get".as_ptr()) };
        if get.is_null() {
            return crate::firstrun::linux_dark(env.as_deref(), "");
        }
        let get: unsafe extern "C" fn(P, S, ...) = std::mem::transmute(get);
        let (mut prefer, mut name): (c_int, *mut c_char) = (0, std::ptr::null_mut());
        get(
            s,
            c"gtk-application-prefer-dark-theme".as_ptr(),
            &mut prefer as *mut c_int,
            c"gtk-theme-name".as_ptr(),
            &mut name as *mut *mut c_char,
            std::ptr::null::<c_char>(),
        );
        let theme = if name.is_null() {
            String::new()
        } else {
            let t = std::ffi::CStr::from_ptr(name).to_string_lossy().to_lowercase();
            (g.g_free)(name.cast());
            t
        };
        prefer != 0 || theme.contains("dark") || crate::firstrun::linux_dark(env.as_deref(), "")
    }
}

/// One colour mode's style sheet (aio's `gtkSplashCss`). Pure.
pub(crate) fn css([bg, text, bar, track]: &[String; 4], o: &Open) -> String {
    let l = &o.look;
    let family = o
        .font
        .as_deref()
        .map(|f| {
            format!("font-family:\"{}\";", f.replace(['\\', '"'], "").split_whitespace().collect::<Vec<_>>().join(" "))
        })
        .unwrap_or_default();
    let r = format!("border-radius:{}px;", l.bar_radius.unwrap_or(l.bp / 2));
    let bp = l.bp;
    let shadow = if l.shadow { "0 1px 2px rgba(0,0,0,0.45)" } else { "none" };
    let weight = if o.bold { "bold" } else { "normal" };
    let border = l.border.as_deref().map_or(String::new(), |b| {
        format!("#aio-splash-border{{border:1px solid {b};border-radius:{}px;background:none;}}", l.radius)
    });
    format!(
        "#aio-splash{{background-color:{bg};}}\
         #aio-splash-text{{color:{text};{family}font-size:{}px;font-weight:{weight};text-shadow:{shadow};}}\
         #aio-splash-track{{background-color:{track};{r}min-height:{bp}px;}}\
         #aio-splash-fill{{background-color:{bar};{r}min-height:{bp}px;}}{border}\
         #aio-splash-close,#aio-splash-close label{{color:{text};background:none;border:none;box-shadow:none;\
         text-shadow:none;padding:0 4px;min-width:0;min-height:0;}}",
        o.font_px
    )
}

unsafe fn build(g: &Gtk, o: &Open, status: Arc<Mutex<Status>>) -> Result<Drawn, String> {
    let l = &o.look;
    let (w, h) = (l.w as i32, l.h as i32);
    let bp = l.bp as i32;
    // SAFETY: `build`'s contract: on GTK's thread with the table loaded; every C string passed is a CString kept alive past its call.
    unsafe {
        let dark = theme_dark(g);
        let css_src = z(&css(&o.colors[dark as usize], o));
        let provider = (g.gtk_css_provider_new)();
        (g.gtk_css_provider_load_from_data)(provider, css_src.as_ptr(), -1, std::ptr::null_mut());
        // GTK_STYLE_PROVIDER_PRIORITY_APPLICATION + 1: over any theme.
        (g.gtk_style_context_add_provider_for_screen)((g.gdk_screen_get_default)(), provider, 601);
        (g.g_object_unref)(provider);

        let loader = (g.gdk_pixbuf_loader_new)();
        let ok_w = (g.gdk_pixbuf_loader_write)(loader, o.png.as_ptr(), o.png.len(), std::ptr::null_mut());
        let ok_c = (g.gdk_pixbuf_loader_close)(loader, std::ptr::null_mut());
        let pixbuf = (g.gdk_pixbuf_loader_get_pixbuf)(loader);
        if ok_w == 0 || ok_c == 0 || pixbuf.is_null() {
            (g.g_object_unref)(loader);
            return Err("GdkPixbuf could not read the image".into());
        }

        let win = (g.gtk_window_new)(0); // GTK_WINDOW_TOPLEVEL
        (g.gtk_widget_set_name)(win, c"aio-splash".as_ptr());
        (g.gtk_window_set_title)(win, z(&o.title).as_ptr());
        (g.gtk_window_set_decorated)(win, 0);
        (g.gtk_window_set_skip_taskbar_hint)(win, 1);
        (g.gtk_window_set_skip_pager_hint)(win, 1);
        (g.gtk_window_set_keep_above)(win, 1);
        (g.gtk_window_set_type_hint)(win, 4); // GDK_WINDOW_TYPE_HINT_SPLASHSCREEN
        (g.gtk_window_set_resizable)(win, 0);
        (g.gtk_window_set_accept_focus)(win, 0); // focus stays where it is
        (g.gtk_window_set_focus_on_map)(win, 0);
        // A window manager's close (no frame, but Alt+F4 reaches it): the
        // user closed it, the start's cancel, as on the other OSes.
        (g.g_signal_connect_data)(
            win,
            c"delete-event".as_ptr(),
            on_delete as *const () as P,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
        );
        let fixed = (g.gtk_fixed_new)();
        (g.gtk_container_add)(win, fixed);
        // The image at its point size: an @2x image is a scale-2 surface,
        // drawn at half its pixels. Sized, it is first scaled to the box it
        // fills (`fit`), cropped for `cover`.
        let (mut pb, mut at, mut owned) = (pixbuf, (0, 0), vec![]);
        if l.sized {
            let r = fit_rect(l.iw, l.ih, l.w, l.h, l.fit);
            let k = o.scale;
            let scaled = (g.gdk_pixbuf_scale_simple)(pixbuf, r.width * k, r.height * k, 2); // GDK_INTERP_BILINEAR
            owned.push(scaled);
            pb = scaled;
            at = (r.x.max(0), r.y.max(0));
            if r.x < 0 || r.y < 0 {
                let sub = (g.gdk_pixbuf_new_subpixbuf)(
                    scaled,
                    (-r.x).max(0) * k,
                    (-r.y).max(0) * k,
                    r.width.min(w) * k,
                    r.height.min(h) * k,
                );
                owned.push(sub);
                pb = sub;
            }
        }
        let surface = (g.gdk_cairo_surface_create_from_pixbuf)(pb, o.scale, std::ptr::null_mut());
        let image = (g.gtk_image_new_from_surface)(surface);
        (g.cairo_surface_destroy)(surface);
        for p in owned {
            (g.g_object_unref)(p);
        }
        (g.g_object_unref)(loader); // owns the pixbuf
        if !l.sized {
            (g.gtk_widget_set_size_request)(image, w, h);
        }
        (g.gtk_fixed_put)(fixed, image, at.0, at.1);

        let (mut label, mut lh) = (std::ptr::null_mut(), 0);
        if o.has_text {
            label = (g.gtk_label_new)(z(&o.text).as_ptr());
            (g.gtk_widget_set_name)(label, c"aio-splash-text".as_ptr());
            let x = match l.align {
                aio::config::Align::Left => 0.0,
                aio::config::Align::Center => 0.5,
                aio::config::Align::Right => 1.0,
            };
            (g.gtk_label_set_xalign)(label, x);
            (g.gtk_label_set_ellipsize)(label, 3); // PANGO_ELLIPSIZE_END
            (g.gtk_widget_set_size_request)(label, w - 2 * l.side as i32, -1);
            // Measured in place (its style is the window's), then moved up by it.
            (g.gtk_fixed_put)(fixed, label, l.side as i32, 0);
            (g.gtk_widget_show)(label); // a hidden widget measures 0
            let (mut min, mut nat) = (0, 0);
            (g.gtk_widget_get_preferred_height)(label, &mut min, &mut nat);
            lh = nat;
        }
        let f = frame(l, lh);
        if !label.is_null() {
            (g.gtk_fixed_move)(fixed, label, l.side as i32, f.label_bottom - lh);
        }
        (g.gtk_window_set_default_size)(win, w, f.th);
        (g.gtk_widget_set_size_request)(win, w, f.th);
        let (mut track, mut fill, mut close) = (std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
        if bp > 0 {
            let bar = |name: &std::ffi::CStr, bw: i32| {
                let b = (g.gtk_box_new)(0, 0);
                (g.gtk_widget_set_name)(b, name.as_ptr());
                (g.gtk_widget_set_size_request)(b, bw, bp);
                (g.gtk_widget_set_no_show_all)(b, 1); // hidden until a download runs
                (g.gtk_fixed_put)(fixed, b, f.bx, f.yb);
                b
            };
            track = bar(c"aio-splash-track", f.bw);
            fill = bar(c"aio-splash-fill", 0);
            if l.cancel {
                // During a download, this cancels it (as a dialog's close does).
                close = (g.gtk_button_new_with_label)(c"\u{2715}".as_ptr());
                (g.gtk_widget_set_name)(close, c"aio-splash-close".as_ptr());
                (g.gtk_button_set_relief)(close, 2); // GTK_RELIEF_NONE
                (g.gtk_widget_set_no_show_all)(close, 1);
                (g.g_signal_connect_data)(
                    close,
                    c"clicked".as_ptr(),
                    on_click as *const () as P,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    0,
                );
                (g.gtk_fixed_put)(fixed, close, w - 30, 4);
            }
        }
        if l.border.is_some() {
            // On top of everything: the 1 px border (and its rounded corners).
            let b = (g.gtk_box_new)(0, 0);
            (g.gtk_widget_set_name)(b, c"aio-splash-border".as_ptr());
            (g.gtk_widget_set_size_request)(b, w, f.th);
            (g.gtk_fixed_put)(fixed, b, 0, 0);
        }
        if l.radius > 0 {
            // The window's shape: rounded corners without a compositor (X11 SHAPE).
            let region = (g.cairo_region_create)();
            for r in rounded_rows(w, f.th, l.radius as i32) {
                (g.cairo_region_union_rectangle)(region, &[r.x, r.y, r.width, r.height]);
            }
            (g.gtk_widget_shape_combine_region)(win, region);
            (g.cairo_region_destroy)(region);
        }
        let (all, primary) = screens(g);
        let primary = primary.unwrap_or(crate::place::Rect { x: 0, y: 0, width: w, height: f.th });
        let pointer = (l.mode == aio::config::SplashPosition::Pointer).then(|| {
            let dev = (g.gdk_seat_get_pointer)((g.gdk_display_get_default_seat)((g.gdk_display_get_default)()));
            let (mut x, mut y) = (0, 0);
            if !dev.is_null() {
                (g.gdk_device_get_position)(dev, std::ptr::null_mut(), &mut x, &mut y);
            }
            (x, y)
        });
        let (x, y) = origin(l, &all, primary, pointer, (w, f.th));
        (g.gtk_window_move)(win, x, y);
        (g.gtk_widget_show_all)(win);
        let [c0, c1] = &o.colors;
        let what = format!(
            "shown splash size={w}x{} scale={} dark={} text={} barpx={bp} colors={} label-h={lh}",
            f.th,
            o.scale,
            dark as u8,
            o.has_text as u8,
            if dark { c1 } else { c0 }.join(",")
        );
        Ok(Drawn {
            win,
            label,
            track,
            fill,
            close,
            bw: f.bw,
            bp,
            size: (w, f.th),
            status,
            t0: Instant::now(),
            shown: false,
            ticks: 0,
            what,
        })
    }
}

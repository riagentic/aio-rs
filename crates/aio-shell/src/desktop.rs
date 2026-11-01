//! The desktop's own surfaces (aio's `notify()`, `ui.tray`): notifications and
//! a tray menu, both drawn by the runtime's main process, and what their
//! clicks ask of the app. A click lands in the app's views through
//! [`use_asks`], so a tray item runs where every button runs (cells,
//! signals, the router) and a route goes through the app's own router.
use aio::serde_json::{Value, json};
use std::io;
use tokio::sync::broadcast;

/// A desktop notification. A click brings the window forward (also from
/// the tray) and goes to `route`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Notice {
    pub title: String,
    pub body: String,
    /// A later notice with the same tag replaces this one.
    pub tag: Option<String>,
    pub silent: bool,
    pub route: Option<String>,
}

/// Show `n`, with the app's icon. The runtime needs no permission for it
/// (the page's own `Notification` is refused: one door). Never silent: a
/// blank title or no window is an error here, and a desktop that shows no
/// notifications is said once in the app's log.
pub async fn notify(n: &Notice) -> io::Result<()> {
    let line = notice_line(n)?;
    match crate::send_runtime(line).await {
        true => Ok(()),
        false => Err(io::Error::new(io::ErrorKind::NotConnected, format!("no window to show \"{}\"", n.title))),
    }
}

/// The control line asking the runtime for `n`.
fn notice_line(n: &Notice) -> io::Result<String> {
    if n.title.trim().is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "a notification needs a non-empty title"));
    }
    Ok(json!({ "t": "notify", "title": n.title, "body": n.body, "tag": n.tag, "silent": n.silent, "route": n.route })
        .to_string())
}

/// The tray icon (the window's icon) and its menu. The menu ends with
/// Show, Hide and Quit; a click on the icon shows or hides the window.
#[derive(Clone, Copy, Debug, Default)]
pub struct Tray {
    /// Unset: the window's title.
    pub tooltip: Option<&'static str>,
    /// Closing the window hides it to the tray; Quit (or the app stopping)
    /// still closes it. A desktop with no tray minimizes it instead.
    pub close_to_tray: bool,
    pub menu: &'static [Item],
}

/// A tray menu item: `run` in the app's views, then the window forward and
/// `route`, as a click on a notice. No label: a separator.
#[derive(Clone, Copy, Debug, Default)]
pub struct Item {
    pub label: &'static str,
    pub run: Option<fn()>,
    pub route: Option<&'static str>,
}

impl Item {
    pub const SEPARATOR: Item = Item { label: "", run: None, route: None };

    pub const fn run(label: &'static str, run: fn()) -> Self {
        Item { label, run: Some(run), route: None }
    }

    pub const fn go(label: &'static str, route: &'static str) -> Self {
        Item { label, run: None, route: Some(route) }
    }
}

/// The tray's text when the pointer rests on it, from now on.
pub fn tooltip(text: &str) {
    crate::tell_runtime(json!({ "t": "tooltip", "text": text }).to_string());
}

/// Which icon [`icon`] changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    /// The window's and its taskbar entry's.
    Window,
    /// The tray's (a declared `Window::tray`).
    Tray,
}

/// At most this many frames in an animation, and bytes in each.
pub const ICON_FRAMES: usize = 64;
pub const ICON_BYTES: usize = 512 * 1024;
/// The fastest animation: tray panels lag above it.
pub const ICON_FPS: u8 = 12;

/// `which` icon from now on (aio's `appIcon`): one PNG, or PNG frames looped
/// at `fps` (at most [`ICON_FPS`]) by the runtime, so a frame costs no
/// message; none: the app's own icon again. Kept, so a window that opens
/// later shows it too. A bad image is refused here, where it is passed.
pub fn icon(which: Icon, frames: &[&[u8]], fps: u8) -> io::Result<()> {
    let line = icon_line(which, frames, fps)?;
    let mut kept = ICONS.lock().unwrap_or_else(|e| e.into_inner());
    kept.retain(|(w, _)| *w != which);
    kept.push((which, line.clone()));
    crate::tell_runtime(line);
    Ok(())
}

/// The icons set so far, as their control lines: said again to a runtime
/// that connects later.
static ICONS: std::sync::Mutex<Vec<(Icon, String)>> = std::sync::Mutex::new(Vec::new());

pub(crate) fn icon_lines() -> Vec<String> {
    ICONS.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(_, l)| l.clone()).collect()
}

/// What a cell's call told the people using the app (`cx.tell`), as this
/// desktop shows it.
#[derive(Debug, PartialEq)]
pub enum Shown {
    Notice(Notice),
    Icon(Icon, Vec<Vec<u8>>, u8),
}

/// `t` as this desktop shows it (pure): an icon's frames decoded.
pub fn shown(t: &aio::tell::Tell) -> io::Result<Shown> {
    use aio::tell::Tell;
    use base64::Engine;
    Ok(match t {
        Tell::Notify { title, body, tag, silent, route } => Shown::Notice(Notice {
            title: title.clone(),
            body: body.clone(),
            tag: tag.clone(),
            silent: *silent,
            route: route.clone(),
        }),
        Tell::Icon { tray, frames, fps } => {
            let b64 = base64::engine::general_purpose::STANDARD;
            let frames = frames.iter().map(|f| b64.decode(f.trim_start_matches("data:image/png;base64,")));
            let frames =
                frames.collect::<Result<_, _>>().map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
            Shown::Icon(if *tray { Icon::Tray } else { Icon::Window }, frames, *fps)
        }
    })
}

/// Show `t` here (how `run` shows what cells tell): a notice through the
/// runtime, on `rt`; an icon at once. What can't be shown is said in the log.
pub fn show(rt: &tokio::runtime::Handle, t: &aio::tell::Tell) {
    match shown(t) {
        Ok(Shown::Notice(n)) => {
            _ = rt.spawn(async move {
                if let Err(e) = notify(&n).await {
                    eprintln!("[aio-shell] notify: {e}");
                }
            })
        }
        Ok(Shown::Icon(which, frames, fps)) => {
            let frames: Vec<&[u8]> = frames.iter().map(Vec::as_slice).collect();
            if let Err(e) = icon(which, &frames, fps) {
                eprintln!("[aio-shell] icon: {e}");
            }
        }
        Err(e) => eprintln!("[aio-shell] icon: {e}"),
    }
}

/// The control line for [`icon`]: each frame a PNG `data:` URL.
fn icon_line(which: Icon, frames: &[&[u8]], fps: u8) -> io::Result<String> {
    use base64::Engine;
    let bad = |m: String| io::Error::new(io::ErrorKind::InvalidInput, m);
    if frames.len() > ICON_FRAMES {
        return Err(bad(format!("an icon of {} frames: at most {ICON_FRAMES}", frames.len())));
    }
    for (i, f) in frames.iter().enumerate() {
        if crate::place::icon_size(f).is_none() {
            return Err(bad(format!("icon frame {i} is not a PNG")));
        }
        if f.len() > ICON_BYTES {
            return Err(bad(format!("icon frame {i} is {} KB: at most {} KB", f.len() >> 10, ICON_BYTES >> 10)));
        }
    }
    let b64 = base64::engine::general_purpose::STANDARD;
    let urls: Vec<String> = frames.iter().map(|f| format!("data:image/png;base64,{}", b64.encode(f))).collect();
    let target = match which {
        Icon::Window => "window",
        Icon::Tray => "tray",
    };
    Ok(json!({ "t": "icon", "target": target, "frames": urls, "fps": fps.clamp(1, ICON_FPS) }).to_string())
}

/// The tray as the runtime reads it (`tray.json`): labels and indices; the
/// functions stay here.
pub(crate) fn tray_json(t: Option<&Tray>, title: &str) -> String {
    let Some(t) = t else { return "null".into() };
    let menu: Vec<Value> = (t.menu.iter().enumerate())
        .map(|(i, it)| match it.label {
            "" => json!("-"),
            l => json!({ "label": l, "i": i, "show": it.route.is_some() }),
        })
        .collect();
    json!({ "tooltip": t.tooltip.unwrap_or(title), "closeToTray": t.close_to_tray, "menu": menu }).to_string()
}

/// What a click asks of the app.
#[derive(Clone, Debug)]
pub enum Ask {
    Run(fn()),
    Go(String),
}

/// The asks in the runtime's `m`: a clicked notice (`{"t":"clicked"}`) or
/// tray item (`{"t":"tray","i":0}`).
pub(crate) fn asks(m: &Value, tray: Option<&Tray>) -> Vec<Ask> {
    let go = |r: Option<&str>| r.filter(|r| !r.is_empty()).map(|r| Ask::Go(r.into()));
    match m["t"].as_str() {
        Some("clicked") => go(m["route"].as_str()).into_iter().collect(),
        Some("tray") => {
            let it = m["i"].as_u64().and_then(|i| tray?.menu.get(usize::try_from(i).ok()?));
            it.filter(|it| !it.label.is_empty())
                .map(|it| it.run.map(Ask::Run).into_iter().chain(go(it.route)).collect())
                .unwrap_or_default()
        }
        _ => vec![],
    }
}

pub(crate) fn channel() -> &'static broadcast::Sender<Ask> {
    static TX: std::sync::OnceLock<broadcast::Sender<Ask>> = std::sync::OnceLock::new();
    TX.get_or_init(|| broadcast::channel(16).0)
}

/// Hand `a` to the views; said when none takes it.
pub(crate) fn deliver(a: Ask) {
    if let Err(e) = channel().send(a) {
        eprintln!("[aio] a click asked {:?}, but no view takes it: call aio_shell::desktop::use_asks in the root", e.0);
    }
}

/// Take the clicks' asks while this component lives (the app's root):
/// items run here, routes go through `go` (the app's `router!` `go`).
pub fn use_asks(go: fn(&str) -> bool) {
    dioxus_core::use_hook(|| {
        let mut rx = channel().subscribe();
        dioxus_core::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(Ask::Run(f)) => f(),
                    Ok(Ask::Go(r)) => _ = go(&r),
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        })
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notice_is_one_line_and_a_blank_title_is_refused_where_it_is_written() {
        // aio tests/notify.test.ts: the effect's shape, "non-empty".
        let n = Notice {
            title: "Done".into(),
            body: "3 files".into(),
            route: Some("/exports".into()),
            ..Default::default()
        };
        assert_eq!(
            notice_line(&n).unwrap(),
            r#"{"body":"3 files","route":"/exports","silent":false,"t":"notify","tag":null,"title":"Done"}"#
        );
        let e = notice_line(&Notice { title: "  ".into(), ..Default::default() }).unwrap_err();
        assert_eq!((e.kind(), e.to_string().contains("non-empty")), (io::ErrorKind::InvalidInput, true));
    }

    #[test]
    fn what_a_cell_tells_is_shown_as_a_notice_or_an_icon() {
        use aio::tell::Tell;
        let n = shown(&Tell::notify("Done", "3 files")).unwrap();
        assert_eq!(n, Shown::Notice(Notice { title: "Done".into(), body: "3 files".into(), ..Default::default() }));
        let png = base64_of(b"\x89PNG");
        let t = Tell::Icon { tray: true, frames: vec![png.clone(), format!("data:image/png;base64,{png}")], fps: 4 };
        assert_eq!(shown(&t).unwrap(), Shown::Icon(Icon::Tray, vec![b"\x89PNG".to_vec(); 2], 4));
        let bad = Tell::Icon { tray: false, frames: vec!["not base64!".into()], fps: 1 };
        assert_eq!(shown(&bad).unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn an_icon_is_png_frames_within_the_caps_and_is_kept_for_a_later_window() {
        // aio tests/app-icon.test.ts: the caps, refused where passed; fps clamped.
        let png = |w: u32| {
            let mut p = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
            p.extend(w.to_be_bytes());
            p.extend(w.to_be_bytes());
            p
        };
        let (a, b) = (png(16), png(32));
        let v: Value = aio::serde_json::from_str(&icon_line(Icon::Tray, &[&a, &b], 30).unwrap()).unwrap();
        assert_eq!((v["t"].as_str(), v["target"].as_str(), v["fps"].as_u64()), (Some("icon"), Some("tray"), Some(12)));
        assert!(v["frames"][1].as_str().unwrap().starts_with("data:image/png;base64,iVBORw0KGgo"));
        let back: Value = aio::serde_json::from_str(&icon_line(Icon::Window, &[], 0).unwrap()).unwrap();
        assert_eq!(
            (back["frames"].as_array().map(Vec::len), back["fps"].as_u64()),
            (Some(0), Some(1)),
            "none: the app's own"
        );
        assert!(icon_line(Icon::Window, &[b"GIF89a"], 8).unwrap_err().to_string().contains("not a PNG"));
        let many: Vec<&[u8]> = vec![&a; ICON_FRAMES + 1];
        assert!(icon_line(Icon::Tray, &many, 8).unwrap_err().to_string().contains("at most 64"));
        let big = [a.clone(), vec![0; ICON_BYTES]].concat();
        assert!(icon_line(Icon::Tray, &[&big], 8).unwrap_err().to_string().contains("at most 512 KB"));
        // The newest of each kind is kept, to say again.
        icon(Icon::Tray, &[&a], 8).unwrap();
        icon(Icon::Tray, &[&b], 8).unwrap();
        icon(Icon::Window, &[&a], 8).unwrap();
        let kept = icon_lines();
        assert_eq!(kept.len(), 2);
        assert!(kept[0].contains(r#""target":"tray""#) && kept[0].contains(&base64_of(&b)));
    }

    fn base64_of(b: &[u8]) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(b)
    }

    #[tokio::test]
    async fn without_a_window_a_notice_is_an_error_never_silence() {
        let e = notify(&Notice { title: "x".into(), ..Default::default() }).await.unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::NotConnected);
    }

    fn pause() {}
    static MENU: [Item; 3] = [Item::run("Pause", pause), Item::SEPARATOR, Item::go("Library", "/lib")];

    #[test]
    fn the_runtime_gets_labels_and_indices_and_the_title_as_tooltip() {
        // aio tests/electron-tray.test.ts: the menu, closeToTray, no tray at all.
        let t = Tray { close_to_tray: true, menu: &MENU, ..Default::default() };
        let v: Value = aio::serde_json::from_str(&tray_json(Some(&t), "Player")).unwrap();
        assert_eq!(
            v,
            json!({ "tooltip": "Player", "closeToTray": true, "menu": [
                { "label": "Pause", "i": 0, "show": false }, "-", { "label": "Library", "i": 2, "show": true } ] })
        );
        assert!(tray_json(Some(&Tray { tooltip: Some("Hi"), ..t }), "P").contains(r#""tooltip":"Hi""#));
        assert_eq!(tray_json(None, "P"), "null");
    }

    /// The asks, readable: `run pause`, `go /lib`.
    fn said(a: Vec<Ask>) -> Vec<String> {
        let name = |f| if std::ptr::fn_addr_eq(f, pause as fn()) { "run pause".into() } else { "run ?".into() };
        a.into_iter()
            .map(|a| match a {
                Ask::Run(f) => name(f),
                Ask::Go(r) => format!("go {r}"),
            })
            .collect()
    }

    #[test]
    fn a_click_runs_its_item_or_goes_to_its_route_and_junk_asks_nothing() {
        // aio tests/desktop-notify-client.test.ts: a method is dispatched, a route navigates, junk is ignored.
        let t = Tray { menu: &MENU, ..Default::default() };
        let a = |m| said(asks(&m, Some(&t)));
        assert_eq!(a(json!({ "t": "tray", "i": 0 })), ["run pause"]);
        assert_eq!(a(json!({ "t": "tray", "i": 2 })), ["go /lib"]);
        assert_eq!(a(json!({ "t": "clicked", "route": "/exports" })), ["go /exports"]);
        for junk in [json!({ "t": "tray", "i": 1 }), json!({ "t": "tray", "i": 9 }), json!({ "t": "tray", "i": "0" })] {
            assert!(a(junk).is_empty());
        }
        assert!(a(json!({ "t": "clicked" })).is_empty(), "a notice with no route only comes forward");
        assert!(a(json!({ "t": "clicked", "route": "" })).is_empty());
        assert!(asks(&json!({ "t": "tray", "i": 0 }), None).is_empty(), "no tray, no items");
        static BOTH: [Item; 1] = [Item { label: "Both", run: Some(pause), route: Some("/b") }];
        let t = Tray { menu: &BOTH, ..Default::default() };
        assert_eq!(said(asks(&json!({ "t": "tray", "i": 0 }), Some(&t))), ["run pause", "go /b"]);
    }

    #[tokio::test]
    async fn the_root_takes_the_asks_in_its_scope() {
        use std::sync::Mutex;
        static WENT: Mutex<Vec<String>> = Mutex::new(Vec::new());
        use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
        static RAN: AtomicUsize = AtomicUsize::new(0);
        fn go(r: &str) -> bool {
            // In the view's scope: what the router's `go` needs.
            assert!(dioxus_core::Runtime::current().try_current_scope_id().is_some());
            WENT.lock().unwrap().push(r.into());
            true
        }
        fn ran() {
            RAN.fetch_add(1, Relaxed);
        }
        fn root() -> dioxus_core::Element {
            use_asks(go);
            dioxus_core::VNode::empty()
        }
        let mut dom = dioxus_core::VirtualDom::new(root);
        dom.rebuild_in_place();
        deliver(Ask::Go("/root-test".into()));
        deliver(Ask::Run(ran));
        let done = || WENT.lock().unwrap().contains(&"/root-test".to_string()) && RAN.load(Relaxed) > 0;
        for _ in 0..100 {
            if done() {
                break;
            }
            _ = tokio::time::timeout(std::time::Duration::from_millis(50), dom.wait_for_work()).await;
        }
        assert!(done(), "the route went through go and the item ran");
    }
}

//! The splash (`[build.splash]`, aio's `build.splash`): the app's image, and
//! its text under it, from the app's start until its window has loaded.
//! The desktop's is a frameless window that Electron's main process opens
//! before anything else (main.js `openSplash`): the app's window shows once
//! it has painted and the splash has been up `min`, and the splash never
//! stays past `max`. While Electron itself is downloaded, the download
//! window stands in for it, in the same colours (`firstrun`). The web's is
//! laid over the page until the app has mounted ([`markup`], `web`);
//! Android's is the starting window (`ax build --android`).
use aio::config::{Align, Fit, Splash, TextPosition};

/// The app's colours outside its page: `[light, dark]`, each `[background,
/// text, bar, track]` (`aio::colors::identity`), the background `bg` when
/// one is given (the window's, the splash's).
pub fn colors(hue: u16, bg: Option<[&str; 2]>) -> [[String; 4]; 2] {
    let mut c = aio::colors::identity(hue);
    if let Some(bg) = bg {
        c[0][0] = bg[0].into();
        c[1][0] = bg[1].into();
    }
    c
}

/// The image's size on screen in CSS pixels: its own, halved for `@2x`.
pub fn size(s: &Splash) -> Option<(u32, u32)> {
    let (w, h) = crate::place::png_size(s.png)?;
    Some(if s.retina() { (w.div_ceil(2), h.div_ceil(2)) } else { (w, h) })
}

/// The text strip under the image, in CSS pixels (none without text).
pub const TEXT_STRIP: u32 = 44;

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The web page's script that takes the splash away: once the app has drawn
/// its view (`aio:mounted`) and the splash has been up its minimum (from
/// the page's start), at its cap anyway.
pub const PAGE_JS: &str = "(() => {
  const s = document.getElementById('aio-splash')
  if (!s) return
  let done = false
  const go = (wait) => { if (!done) { done = true; setTimeout(() => s.remove(), wait) } }
  document.addEventListener('aio:mounted', () => go(Math.max(0, (+s.dataset.min || 0) - performance.now())), { once: true })
  setTimeout(() => go(0), +s.dataset.max || 15000)
})()";

/// Is the splash up at every start, and does a first start's download draw
/// it (pure)? `first-start`: only the download; `never`: neither (the
/// download window is then the plain one).
pub fn shown(show: aio::config::SplashShow) -> (bool, bool) {
    use aio::config::SplashShow::*;
    (show == Always, show != Never)
}

/// The image box's size in CSS pixels: `[build.splash] size`, else the
/// image's own ([`size`]); `None`: the image is not a PNG.
pub fn image_box(s: &Splash) -> Option<(u32, u32)> {
    let own = size(s)?;
    Some(s.size.map_or(own, |[w, h]| (w, h)))
}

/// The splash window's size: its image box, and the text strip below it
/// when the text is there (`text_position = "below"`).
pub fn window_size(s: &Splash) -> Option<(u32, u32)> {
    let (w, h) = image_box(s)?;
    let strip = if s.text.is_some() && s.text_position == TextPosition::Below { TEXT_STRIP } else { 0 };
    Some((w, h + strip))
}

/// `base` (`[light, dark]`, each background, text, bar, track) with the
/// splash's own `text_color`, `bar` and `track` (pure): the download
/// window's colours while it draws the splash.
pub fn look_colors(mut base: [[String; 4]; 2], s: &Splash) -> [[String; 4]; 2] {
    for (i, pair) in [(1, &s.text_color), (2, &s.bar), (3, &s.track)] {
        if let Some([l, d]) = pair {
            base[0][i] = l.clone();
            base[1][i] = d.clone();
        }
    }
    base
}

/// The splash as `<style>` and a `#aio-splash` block, its image at `src`,
/// light or dark as the OS is (`colors` as [`colors`] makes them, the
/// splash's `text_color` over them; `title` fills the text's `{title}`),
/// laid out by its look keys. `over`: laid over a page (the web's), else
/// the whole window.
pub fn markup(s: &Splash, title: &str, colors: &[[String; 4]; 2], src: &str, over: bool) -> String {
    let (w, h) = image_box(s).unwrap_or((320, 200));
    let [l, d] = look_colors(colors.clone(), s);
    let text = s.text.as_deref().map(|t| format!("<p>{}</p>", esc(&t.replace("{title}", title)))).unwrap_or_default();
    let place = match over {
        true => "position:fixed;inset:0;z-index:2147483647;display:flex;align-items:center;justify-content:center",
        false => "position:fixed;inset:0;display:flex;align-items:center;justify-content:center",
    };
    let fit = match s.fit {
        Fit::Contain => "contain",
        Fit::Cover => "cover",
    };
    let align = match s.align {
        Align::Left => "left",
        Align::Center => "center",
        Align::Right => "right",
    };
    let (side, bottom) = s.margin.map_or((24, 18), |m| (m, m));
    // The text's shadow: the OS's on the desktop (macOS, Linux on; Windows
    // off), only when asked on the web.
    let shadow = match over {
        true => s.shadow == Some(true),
        false => s.shadow.unwrap_or(!cfg!(windows)),
    };
    let shadow = if shadow { "text-shadow:0 1px 2px rgba(0,0,0,.45);" } else { "" };
    let text_at = match s.text_position {
        TextPosition::Below => format!("height:{TEXT_STRIP}px;line-height:{TEXT_STRIP}px;padding:0 {side}px"),
        TextPosition::Over => format!("position:absolute;left:{side}px;right:{side}px;bottom:{bottom}px"),
    };
    // A rounded or bordered splash: the box itself, the window clear around it.
    let frame = match (s.radius, s.border.as_deref().filter(|b| aio::config::hex_color(b))) {
        (0, None) => String::new(),
        (r, b) => format!(
            "#aio-splash>div{{border-radius:{r}px;overflow:hidden;{}}}",
            b.map_or(String::new(), |b| format!("border:1px solid {b};box-sizing:border-box"))
        ),
    };
    format!(
        "<style>#aio-splash{{{place};background:{lb};color:{lt};\
         font:{weight} {font}px {family}system-ui,-apple-system,\"Segoe UI\",sans-serif;user-select:none;cursor:default}}\
         #aio-splash>div{{position:relative;display:flex;flex-direction:column;width:{w}px;max-width:100%;background:{lb}}}\
         #aio-splash img{{display:block;width:{w}px;height:{h}px;max-width:100%;object-fit:{fit}}}\
         #aio-splash p{{margin:0;{text_at};text-align:{align};{shadow}box-sizing:border-box;\
         white-space:nowrap;overflow:hidden;text-overflow:ellipsis}}{frame}\
         @media (prefers-color-scheme: dark){{#aio-splash,#aio-splash>div{{background:{db};color:{dt}}}}}{clear}</style>\
         <div id=\"aio-splash\" role=\"img\" aria-label=\"{alt}\" data-min=\"{min}\" data-max=\"{max}\">\
         <div><img src=\"{src}\" alt=\"\">{text}</div></div>",
        // A desktop splash with a frame: the window is clear around its box.
        clear = if !over && !frame.is_empty() { "html body #aio-splash{background:transparent}" } else { "" },
        weight = if s.bold { "bold" } else { "normal" },
        font = s.font_size.unwrap_or(13),
        family = family(s.font.as_deref()),
        min = s.min_ms,
        max = s.max_ms,
        lb = l[0],
        lt = l[1],
        db = d[0],
        dt = d[1],
        alt = esc(title),
        src = esc(src),
    )
}

/// `[build.splash] font` first in the CSS font list: a generic family as
/// is, a named one quoted; a name with other than letters, digits, spaces
/// and dashes is left out.
fn family(font: Option<&str>) -> String {
    match font.map(str::trim) {
        Some(f @ ("serif" | "sans-serif" | "monospace" | "cursive" | "fantasy")) => format!("{f},"),
        Some(f) if !f.is_empty() && f.chars().all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '-') => {
            format!("\"{f}\",")
        }
        _ => String::new(),
    }
}
/// The desktop splash's page, its image beside it (`splash.png`).
pub fn page(s: &Splash, title: &str, colors: &[[String; 4]; 2]) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src file:; style-src 'unsafe-inline'\">\
         <title>{}</title><style>html,body{{margin:0;height:100%;overflow:hidden}}</style></head><body>{}</body></html>",
        esc(title),
        markup(s, title, colors, "splash.png", false)
    )
}

/// What main.js reads (`splash.json`): the window's size, how long it
/// stays, its background by mode, where it opens (`position`; `target`: the
/// app's window as last placed, `[x, y, w, h]`) and its drop shadow. None:
/// no splash, or an image that is not a PNG (said).
pub fn plan(s: &Splash, colors: &[[String; 4]; 2], target: Option<[i32; 4]>) -> Option<aio::serde_json::Value> {
    let Some((w, h)) = window_size(s) else {
        eprintln!("[aio] the splash image is not a PNG: no splash");
        return None;
    };
    // Rounded or bordered: the window is clear, the page draws the box.
    let clear = s.radius > 0 || s.border.is_some();
    Some(aio::serde_json::json!({
        "width": w,
        "height": h,
        "min": s.min_ms,
        "max": s.max_ms,
        "bg": [colors[0][0], colors[1][0]],
        "clear": clear,
        "fade": s.fade_ms.unwrap_or(100),
        "mode": s.position,
        "target": target,
        "windowShadow": s.window_shadow.unwrap_or(cfg!(target_os = "macos")),
    }))
}

/// Lay the desktop splash out in the shell's dir: `splash.json` (`null`:
/// none), and its page and image beside it ([`plan`]'s `target`).
pub fn install(
    dir: &std::path::Path,
    s: Option<&Splash>,
    title: &str,
    colors: &[[String; 4]; 2],
    target: Option<[i32; 4]>,
) -> std::io::Result<()> {
    let plan = s.and_then(|s| Some((s, plan(s, colors, target)?)));
    let Some((s, plan)) = plan else { return std::fs::write(dir.join("splash.json"), "null") };
    std::fs::write(dir.join("splash.png"), s.png)?;
    std::fs::write(dir.join("splash.html"), page(s, title, colors))?;
    std::fs::write(dir.join("splash.json"), plan.to_string())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A PNG header of `w`×`h` (all `png_size` reads).
    pub(crate) fn png(w: u32, h: u32) -> &'static [u8] {
        let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        b.extend(w.to_be_bytes());
        b.extend(h.to_be_bytes());
        Box::leak(b.into_boxed_slice())
    }

    fn splash(image: &str, text: Option<&str>) -> Splash {
        Splash {
            image: image.into(),
            text: text.map(String::from),
            min_ms: 400,
            max_ms: 15_000,
            background: None,
            font: None,
            font_size: None,
            bold: false,
            fade_ms: None,
            png: png(640, 400),
            ..Default::default()
        }
    }

    /// The first line of `lines` that holds `want` (the lines before it kept in `said`).
    async fn until<R: tokio::io::AsyncBufRead + Unpin>(
        lines: &mut tokio::io::Lines<R>,
        want: &str,
        said: &mut Vec<String>,
    ) -> String {
        loop {
            // A hang guard: a loaded machine took tens of seconds to draw it.
            match tokio::time::timeout(std::time::Duration::from_secs(60), lines.next_line()).await {
                Ok(Ok(Some(l))) if l.contains(want) => return l,
                Ok(Ok(Some(l))) => said.push(l),
                _ => panic!("never said {want:?}: {said:#?}"),
            }
        }
    }

    /// The real shell (main.js) in the runtime, headless: the splash is up
    /// before the app's window, and goes when the window shows, not before
    /// its minimum.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_splash_is_up_first_and_goes_when_the_window_shows() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let rt = crate::runtime::dir();
        let exe = crate::runtime::exe(&rt);
        if !exe.exists() {
            return eprintln!("skipped: no Electron runtime in {}", rt.display());
        }
        let d = tempfile::tempdir_in(aio::paths::short_temp()).unwrap();
        let dir = d.path().join("shell");
        crate::install(&dir, "splashed", &crate::Window { title: "Splashed", ..Default::default() }).unwrap();
        // Rounded, faded: the clear window and the fade run in the real runtime too.
        // Its cap far off: a splash drawn later than it (a loaded machine) is never up.
        let s = Splash {
            min_ms: 900,
            max_ms: 300_000,
            radius: 10,
            fade_ms: Some(150),
            ..splash("assets/splash.png", Some("Starting {title}"))
        };
        // Built in: the Windows lane runs this binary away from the source tree.
        let s = Splash { png: include_bytes!("../../aio/tests/fixtures/splash.png"), ..s };
        install(&dir, Some(&s), "Splashed", &colors(210, None), None).unwrap();
        let (sock, ctl_sock) = (aio::ipc::addr(d.path(), "wire"), aio::ipc::addr(d.path(), "ctl"));
        let mut wires = aio::ipc::Listener::bind(&sock).unwrap();
        let mut ctls = aio::ipc::Listener::bind(&ctl_sock).unwrap();
        let mut child = tokio::process::Command::new(&exe)
            .arg(&dir)
            .args(crate::runtime::sandbox_args(&rt))
            .args(["--ozone-platform=headless", "--aio-title=Splashed", "--aio-width=640", "--aio-height=480"])
            // GL drawn by SwiftShader, as TestUi's: with no display the default GL
            // fails its start under load and a hidden window may never paint.
            .args(["--use-angle=swiftshader", "--enable-unsafe-swiftshader"])
            .arg(format!("--user-data-dir={}", d.path().join("profile").display()))
            .arg(format!("--aio-sock={sock}"))
            .arg(format!("--aio-ctl={ctl_sock}"))
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env("TMPDIR", d.path())
            .env("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={}", d.path().join("no-bus").display()))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut lines = tokio::io::BufReader::new(child.stderr.take().unwrap()).lines();
        let mut said = vec![];
        let ctl = tokio::time::timeout(std::time::Duration::from_secs(60), ctls.accept()).await.unwrap().unwrap();
        until(&mut lines, "[aio-shell] splash: up", &mut said).await;
        let (_r, mut ctl) = tokio::io::split(ctl);
        // The window opens only now: the splash was up before it.
        ctl.write_all(b"{\"t\":\"place\",\"width\":640,\"height\":480}\n").await.unwrap();
        let _wire = tokio::time::timeout(std::time::Duration::from_secs(60), wires.accept()).await.unwrap().unwrap();
        let gone = until(&mut lines, "[aio-shell] splash: gone after", &mut said).await;
        let ms: u64 = gone.split("gone after ").nth(1).and_then(|t| t.split(' ').next()).unwrap().parse().unwrap();
        assert!(ms >= 900 && gone.ends_with(", the window shown"), "kept its minimum, gone with the window: {gone}");
        _ = child.kill().await;
    }

    /// aio's look keys: size, fit, text over or below, alignment, margin,
    /// font, colours by mode, a rounded and bordered box on a clear window,
    /// the fade.
    #[test]
    fn the_splash_takes_its_look_keys() {
        use aio::config::{Align, Fit, TextPosition};
        let c = colors(210, None);
        let s = Splash {
            size: Some([480, 300]),
            fit: Fit::Cover,
            text_position: TextPosition::Over,
            align: Align::Left,
            margin: Some(20),
            radius: 12,
            border: Some("#ccc".into()),
            fade_ms: Some(250),
            font_size: Some(15),
            font: Some("Iosevka Term".into()),
            bold: true,
            text_color: Some(["#111".into(), "#eee".into()]),
            ..splash("assets/splash.png", Some("Starting {title}"))
        };
        let html = page(&s, "Notes", &c);
        for want in [
            "width:480px;height:300px;max-width:100%;object-fit:cover",
            "position:absolute;left:20px;right:20px;bottom:20px;text-align:left",
            "font:bold 15px \"Iosevka Term\",system-ui",
            &format!("background:{};color:#111", c[0][0]),
            &format!("background:{};color:#eee", c[1][0]),
            "#aio-splash>div{border-radius:12px;overflow:hidden;border:1px solid #ccc",
            "html body #aio-splash{background:transparent}",
        ] {
            assert!(html.contains(want), "{want}: {html}");
        }
        let p = plan(&s, &c, None).unwrap();
        // Text over the image: no strip under it.
        assert_eq!((p["width"].as_u64(), p["height"].as_u64()), (Some(480), Some(300)));
        assert_eq!((p["clear"].as_bool(), p["fade"].as_u64()), (Some(true), Some(250)));
        // The defaults: as before the keys existed.
        let plain = splash("assets/splash.png", Some("x"));
        let html = page(&plain, "Notes", &c);
        assert!(
            html.contains("object-fit:contain")
                && html.contains("text-align:center")
                && html.contains("font:normal 13px")
        );
        assert!(!html.contains("border-radius") && !html.contains("transparent"));
        // A generic family as is; a name that could leave the CSS, left out.
        assert_eq!(
            (family(Some("serif")), family(Some("a;}b")), family(None)),
            ("serif,".into(), String::new(), String::new())
        );
        assert_eq!(window_size(&plain), Some((640, 400 + TEXT_STRIP)));
        assert_eq!(
            (plan(&plain, &c, None).unwrap()["clear"].as_bool(), plan(&plain, &c, None).unwrap()["fade"].as_u64()),
            (Some(false), Some(100))
        );
        // The download window drawing the splash takes its bar and text colours.
        let bar =
            Splash { bar: Some(["#0f0".into(), "#0a0".into()]), track: Some(["#ddd".into(), "#333".into()]), ..plain };
        let dl = look_colors(c.clone(), &bar);
        assert_eq!(
            (dl[0][2].as_str(), dl[1][2].as_str(), dl[1][3].as_str(), &dl[0][0]),
            ("#0f0", "#0a0", "#333", &c[0][0])
        );
        let js = include_str!("main.js");
        assert!(js.contains("transparent: !!SPLASH.clear") && js.contains("s.setOpacity(1 - i / steps)"));
        use aio::config::SplashShow::*;
        assert_eq!([shown(Always), shown(FirstStart), shown(Never)], [(true, true), (false, true), (false, false)]);
    }

    #[test]
    fn the_splash_is_its_image_its_text_and_the_app_s_colours() {
        let c = colors(210, None);
        let s = splash("assets/splash.png", Some("Starting {title} <beta>"));
        assert_eq!(size(&s), Some((640, 400)));
        assert_eq!(size(&splash("assets/splash@2x.png", None)), Some((320, 200)), "@2x: half its pixels");
        let html = page(&s, "Notes & Co", &c);
        assert!(html.contains("<p>Starting Notes &amp; Co &lt;beta&gt;</p>"), "{html}");
        assert!(html.contains(&format!("background:{};color:{}", c[0][0], c[0][1])));
        assert!(
            html.contains(&format!("prefers-color-scheme: dark){{#aio-splash,#aio-splash>div{{background:{}", c[1][0]))
        );
        assert!(html.contains("default-src 'none'"), "the splash page runs nothing");
        let p = plan(&s, &c, None).unwrap();
        assert_eq!((p["width"].as_u64(), p["height"].as_u64()), (Some(640), Some(400 + u64::from(TEXT_STRIP))));
        assert_eq!((p["min"].as_u64(), p["max"].as_u64()), (Some(400), Some(15000)));
        let own = colors(210, Some(["#fff", "#000"]));
        assert_eq!((own[0][0].as_str(), own[1][0].as_str(), own[0][2].as_str()), ("#fff", "#000", c[0][2].as_str()));
        assert!(plan(&Splash { png: b"GIF89a", ..s.clone() }, &c, None).is_none(), "not a PNG: no splash");
        let d = tempfile::tempdir().unwrap();
        install(d.path(), None, "T", &c, None).unwrap();
        assert_eq!(std::fs::read_to_string(d.path().join("splash.json")).unwrap(), "null");
        install(d.path(), Some(&s), "T", &c, None).unwrap();
        assert_eq!(std::fs::read(d.path().join("splash.png")).unwrap(), s.png);
        assert!(d.path().join("splash.html").is_file());
    }
}

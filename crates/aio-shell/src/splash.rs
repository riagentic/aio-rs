//! The splash (`[build.splash]`, aio's `build.splash`): the app's image, and
//! its text under it, from the app's start until its window has loaded.
//! The desktop's is a frameless window that Electron's main process opens
//! before anything else (main.js `openSplash`): the app's window shows once
//! it has painted and the splash has been up `min`, and the splash never
//! stays past `max`. While Electron itself is downloaded, the download
//! window stands in for it, in the same colours (`firstrun`). The web's is
//! laid over the page until the app has mounted ([`markup`], `web`);
//! Android's is the starting window (`ax build --android`).
use aio::config::Splash;

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

/// The splash as `<style>` and a `#aio-splash` block, its image at `src`,
/// light or dark as the OS is (`colors` as [`colors`] makes them; `title`
/// fills the text's `{title}`). `over`: laid over a page (the web's), else
/// the whole window.
pub fn markup(s: &Splash, title: &str, colors: &[[String; 4]; 2], src: &str, over: bool) -> String {
    let (w, h) = size(s).unwrap_or((320, 200));
    let [l, d] = colors;
    let text = s.text.as_deref().map(|t| format!("<p>{}</p>", esc(&t.replace("{title}", title)))).unwrap_or_default();
    let place = match over {
        true => "position:fixed;inset:0;z-index:2147483647;display:flex;align-items:center;justify-content:center",
        false => "position:fixed;inset:0;display:flex;align-items:center;justify-content:center",
    };
    format!(
        "<style>#aio-splash{{{place};flex-direction:column;background:{lb};color:{lt};\
         font:13px system-ui,-apple-system,\"Segoe UI\",sans-serif;user-select:none;cursor:default}}\
         #aio-splash img{{display:block;width:{w}px;height:{h}px;max-width:100%;object-fit:contain}}\
         #aio-splash p{{margin:0;height:{TEXT_STRIP}px;line-height:{TEXT_STRIP}px;max-width:{w}px;\
         white-space:nowrap;overflow:hidden;text-overflow:ellipsis}}\
         @media (prefers-color-scheme: dark){{#aio-splash{{background:{db};color:{dt}}}}}</style>\
         <div id=\"aio-splash\" role=\"img\" aria-label=\"{alt}\" data-min=\"{min}\" data-max=\"{max}\">\
         <img src=\"{src}\" alt=\"\">{text}</div>",
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
/// stays, its background by mode. None: no splash, or an image that is not
/// a PNG (said).
pub fn plan(s: &Splash, colors: &[[String; 4]; 2]) -> Option<aio::serde_json::Value> {
    let Some((w, h)) = size(s) else {
        eprintln!("[aio] the splash image is not a PNG: no splash");
        return None;
    };
    let strip = if s.text.is_some() { TEXT_STRIP } else { 0 };
    Some(aio::serde_json::json!({
        "width": w,
        "height": h + strip,
        "min": s.min_ms,
        "max": s.max_ms,
        "bg": [colors[0][0], colors[1][0]],
    }))
}

/// Lay the desktop splash out in the shell's dir: `splash.json` (`null`:
/// none), and its page and image beside it.
pub fn install(
    dir: &std::path::Path,
    s: Option<&Splash>,
    title: &str,
    colors: &[[String; 4]; 2],
) -> std::io::Result<()> {
    let plan = s.and_then(|s| Some((s, plan(s, colors)?)));
    let Some((s, plan)) = plan else { return std::fs::write(dir.join("splash.json"), "null") };
    std::fs::write(dir.join("splash.png"), s.png)?;
    std::fs::write(dir.join("splash.html"), page(s, title, colors))?;
    std::fs::write(dir.join("splash.json"), plan.to_string())
}

#[cfg(test)]
mod tests {
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
            png: png(640, 400),
        }
    }

    /// The first line of `lines` that holds `want` (the lines before it kept in `said`).
    async fn until<R: tokio::io::AsyncBufRead + Unpin>(
        lines: &mut tokio::io::Lines<R>,
        want: &str,
        said: &mut Vec<String>,
    ) -> String {
        loop {
            match tokio::time::timeout(std::time::Duration::from_secs(30), lines.next_line()).await {
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
        let s = Splash { min_ms: 900, ..splash("assets/splash.png", Some("Starting {title}")) };
        // Built in: the Windows lane runs this binary away from the source tree.
        let s = Splash { png: include_bytes!("../../aio/tests/fixtures/splash.png"), ..s };
        install(&dir, Some(&s), "Splashed", &colors(210, None)).unwrap();
        let (sock, ctl_sock) = (aio::ipc::addr(d.path(), "wire"), aio::ipc::addr(d.path(), "ctl"));
        let mut wires = aio::ipc::Listener::bind(&sock).unwrap();
        let mut ctls = aio::ipc::Listener::bind(&ctl_sock).unwrap();
        let mut child = tokio::process::Command::new(&exe)
            .arg(&dir)
            .args(crate::runtime::sandbox_args(&rt))
            .args(["--ozone-platform=headless", "--aio-title=Splashed", "--aio-width=640", "--aio-height=480"])
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
        let ctl = tokio::time::timeout(std::time::Duration::from_secs(20), ctls.accept()).await.unwrap().unwrap();
        until(&mut lines, "[aio-shell] splash: up", &mut said).await;
        let (_r, mut ctl) = tokio::io::split(ctl);
        // The window opens only now: the splash was up before it.
        ctl.write_all(b"{\"t\":\"place\",\"width\":640,\"height\":480}\n").await.unwrap();
        let _wire = tokio::time::timeout(std::time::Duration::from_secs(20), wires.accept()).await.unwrap().unwrap();
        let gone = until(&mut lines, "[aio-shell] splash: gone after", &mut said).await;
        let ms: u64 = gone.split("gone after ").nth(1).and_then(|t| t.split(' ').next()).unwrap().parse().unwrap();
        assert!(ms >= 900 && gone.ends_with(", the window shown"), "kept its minimum, gone with the window: {gone}");
        _ = child.kill().await;
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
        assert!(html.contains(&format!("prefers-color-scheme: dark){{#aio-splash{{background:{}", c[1][0])));
        assert!(html.contains("default-src 'none'"), "the splash page runs nothing");
        let p = plan(&s, &c).unwrap();
        assert_eq!((p["width"].as_u64(), p["height"].as_u64()), (Some(640), Some(400 + u64::from(TEXT_STRIP))));
        assert_eq!((p["min"].as_u64(), p["max"].as_u64()), (Some(400), Some(15000)));
        let own = colors(210, Some(["#fff", "#000"]));
        assert_eq!((own[0][0].as_str(), own[1][0].as_str(), own[0][2].as_str()), ("#fff", "#000", c[0][2].as_str()));
        assert!(plan(&Splash { png: b"GIF89a", ..s.clone() }, &c).is_none(), "not a PNG: no splash");
        let d = tempfile::tempdir().unwrap();
        install(d.path(), None, "T", &c).unwrap();
        assert_eq!(std::fs::read_to_string(d.path().join("splash.json")).unwrap(), "null");
        install(d.path(), Some(&s), "T", &c).unwrap();
        assert_eq!(std::fs::read(d.path().join("splash.png")).unwrap(), s.png);
        assert!(d.path().join("splash.html").is_file());
    }
}

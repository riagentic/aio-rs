//! `ax build <app> --web`: the app as a static site its users install (aio
//! `build-web`, a PWA): the app's lib built for `wasm32` (its cells run in
//! the page: its own, or synced copies of a server's, `aio::web`), bound by
//! `wasm-bindgen`, with a web manifest, the Apple Home Screen tags, icons
//! and an offline service worker. In `dist/<id>-web/`: put it on any HTTPS
//! host, or serve it from the app's own server (`Http::shell`).
use crate::build::{App, app_dir, app_of};
use aio::sha256::Sha256;
use std::path::{Path, PathBuf};
use std::process::Command;

mod hot;
mod split;

/// The page's bundle, as `wasm-bindgen --out-name` names it.
const BUNDLE: &str = "app";

/// The version of `name` in a `Cargo.lock` (pure).
pub fn locked(lock: &str, name: &str) -> Option<String> {
    let mut lines = lock.lines();
    while let Some(l) = lines.next() {
        if l == format!("name = \"{name}\"") {
            return lines.next()?.strip_prefix("version = \"")?.strip_suffix('"').map(String::from);
        }
    }
    None
}

/// `#rrggbb` of an HSL colour, as the app's icon and theme make it from its hue (pure).
fn hex(h: f64, s: f64, l: f64) -> String {
    let a = s * l.min(1.0 - l);
    [0.0, 8.0, 4.0]
        .map(|n| {
            let k = (n + h / 30.0) % 12.0;
            ((l - a * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0)) * 255.0).round() as u8
        })
        .iter()
        .fold("#".to_string(), |s, c| s + &format!("{c:02x}"))
}

/// The page's background and theme colours, from the app's hue (aio's).
pub fn colors(hue: u16) -> (String, String) {
    let h = f64::from(hue);
    (hex(h, 0.68, 0.96), hex(h, 0.62, 0.68))
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The web manifest: its name, colours and icons (pure).
pub fn manifest(title: &str, hue: u16, icons: &[(String, u32)]) -> String {
    let (background, theme) = colors(hue);
    let icons: Vec<serde_json::Value> = icons
        .iter()
        .map(|(src, n)| serde_json::json!({ "src": src, "sizes": format!("{n}x{n}"), "type": "image/png" }))
        .collect();
    let m = serde_json::json!({
        "name": title, "short_name": title, "id": "./", "start_url": "./", "scope": "./",
        "display": "standalone", "background_color": background, "theme_color": theme, "icons": icons,
    });
    serde_json::to_string_pretty(&m).unwrap_or_default() + "\n"
}

/// The `<head>` lines that make the page installable: the manifest and the
/// Apple Home Screen tags (iOS reads these, not the manifest, for the title
/// and icon) (pure).
pub fn head(title: &str, hue: u16, icon: &str, touch: &str) -> String {
    let (t, (_, theme)) = (esc(title), colors(hue));
    [
        r#"<link rel="manifest" href="./manifest.webmanifest">"#.to_string(),
        format!(r#"<meta name="theme-color" content="{theme}">"#),
        format!(r#"<link rel="icon" href="./{icon}">"#),
        format!(r#"<link rel="apple-touch-icon" href="./{touch}">"#),
        r#"<meta name="apple-mobile-web-app-capable" content="yes">"#.into(),
        r#"<meta name="mobile-web-app-capable" content="yes">"#.into(),
        format!(r#"<meta name="apple-mobile-web-app-title" content="{t}">"#),
        r#"<meta name="apple-mobile-web-app-status-bar-style" content="default">"#.into(),
    ]
    .join("\n")
}

/// What starts the page (`start.js`, a module): where the site's files
/// are (lazy views' chunks, the worker), the offline cache once the app
/// has drawn its view (its download then takes nothing from the page's),
/// which says why when the browser refuses it (`file://`, plain http off
/// localhost), then the bundle, which starts the app. A dev build has no
/// offline cache: each reload is the newest build (pure).
pub fn start_js(dev: bool) -> String {
    let cache = "if ('serviceWorker' in navigator) document.addEventListener('aio:mounted', () =>
  navigator.serviceWorker.register('./sw.js').catch((e) =>
    console.error('[aio] the offline cache (service worker) is not registered: the app works online only. It needs https (or localhost):', e)), { once: true });
";
    format!("{BASE}{}import start from './app.js';\nstart();\n", if dev { "" } else { cache })
}

/// Where the site's files are, beside the script that says it.
const BASE: &str = "globalThis.__aioBase = new URL('./', import.meta.url).href;\n";

/// The worker of `aio::effect::off_thread` (`worker.js`, a module): the
/// app's own module, handed compiled by the page, not started as the app
/// (`__aioWorker`), runs each job it is sent: its steps in turn (the
/// page's lazy chunks, then the function on its input) through the
/// module's one export; it answers the last one's output or why it failed
/// (a panic's message).
pub const WORKER_JS: &str = "globalThis.__aioWorker = true;
let ready;
onmessage = async ({ data: [module, steps] }) => {
  try {
    ready ??= import('./app.js').then(async (m) => (await m.default({ module_or_path: module }), m));
    const m = await ready;
    let out;
    for (const [step, arg] of steps) out = await m.__aio_work(step, arg);
    postMessage([true, out], [out.buffer]);
  } catch (e) {
    postMessage([false, globalThis.__aioPanic || String(e)]);
  }
};
";

/// The bundle's glue `js` with its start held in a worker: there the
/// module runs work, not the app (pure).
pub fn held(js: &str) -> Result<String, String> {
    const START: &str = "wasm.__wbindgen_start();";
    match (js.contains("__wbindgen_start"), js.matches(START).count()) {
        (false, _) => Ok(js.into()),
        (true, 1) => Ok(js.replacen(START, &format!("if (!globalThis.__aioWorker) {START}"), 1)),
        _ => Err("the wasm-bindgen glue starts the app in a way this ax does not know".into()),
    }
}

/// The window's page scripts (`page.js`): keys, focus, drag data and
/// dialogs as on the desktop (`aio::page`), and in a dev build the overlay
/// that shows the page's errors on it and the live reload ([`RELOAD`]);
/// the console sent on ([`CONSOLE`]) where something takes it (`logged`:
/// a dev build, or an app with a server and `[server] page_log`) (pure).
pub fn page_js(dev: bool, logged: bool) -> String {
    use aio::page::{DIALOG, DRAG, ENTER, FOCUS, KEYS, OVERLAY, SWAP, WHEEL};
    // `;` after each piece: an `(() => …)()` piece followed by another
    // would otherwise call its result
    let pieces: &[&str] = match dev {
        true => &[OVERLAY, KEYS, WHEEL, FOCUS, DRAG, DIALOG, ENTER, SWAP, aio_shell::splash::PAGE_JS, RELOAD],
        false => &[KEYS, WHEEL, FOCUS, DRAG, DIALOG, ENTER, SWAP, aio_shell::splash::PAGE_JS],
    };
    // The console to `ax dev`'s log, or in a release to the app server's (`[server] page_log`).
    let console = CONSOLE.replace("ENDPOINT", if dev { "/aio-dev/log" } else { "/__aio/log" });
    let console = logged.then_some(console.as_str());
    console.into_iter().chain(pieces.iter().copied()).map(|p| format!("{};\n", p.trim_end())).collect()
}

/// The page: its language and direction (`[app] lang`, `dir`), viewport,
/// policy (`aio::page::WEB`: no inline script, no `eval`) and head,
/// `body` (the first paint, if rendered), then the page scripts and what
/// starts the app (pure).
pub fn index_html(title: &str, app: &aio::config::App, head: &str, body: &str) -> String {
    format!(
        "<!doctype html>\n{}\n<head>\n<meta charset=\"utf-8\">\n\
         <meta http-equiv=\"Content-Security-Policy\" content=\"{}\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1, viewport-fit=cover\">\n\
         <title>{}</title>\n{head}\n</head>\n<body>\n<div id=\"main\">{body}</div>\n\
         <script src=\"./page.js\"></script>\n<script type=\"module\" src=\"./start.js\"></script>\n</body>\n</html>\n",
        aio_shell::html_open(app.lang.as_deref(), app.dir),
        aio::page::WEB,
        esc(title),
    )
}

/// Why the app's own icon (`w`×`h`) can't be the web app's: Chrome installs
/// a web app only with a square icon of at least 144 px (aio's `webIconProblem`).
pub fn icon_problem(w: u32, h: u32) -> Option<String> {
    let why = match () {
        () if w.min(h) < 144 => "an icon of at least 144×144",
        () if w != h => "a square icon",
        () => return None,
    };
    Some(format!(
        "icon.png is {w}×{h}: Chrome offers to install a web app only with {why}, so this one is not installable \
         on Android. Ship a 512×512 icon.png"
    ))
}

/// Whether the worker pins `f`'s digest: code only (a host may rewrite a
/// page or an image, and a pinned digest would then refuse every install).
fn pinned(f: &str) -> bool {
    [".js", ".mjs", ".css", ".wasm"].iter().any(|e| f.ends_with(e))
}

/// The build's version: SHA-256 over every file's path and bytes (pure).
pub fn content_hash(files: &[(String, Vec<u8>)]) -> String {
    let mut h = Sha256::new();
    for (f, b) in files {
        h.update(f.as_bytes());
        h.update([0]);
        h.update(b);
    }
    h.finalize()[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// The offline cache of this build (aio's `serviceWorker`): every file
/// precached under a cache named by the app, the build and its scope
/// (one origin hosts many apps and paths), fetched past the HTTP cache and
/// checked against its digest (code only); same-origin GETs answered
/// cache-first, but `served` (the app's own server serves the page, and may
/// paint each one: `Http::painted`) a navigation asks the server first; a
/// navigation the host can't answer (offline, or a client route it has no
/// file for) given the page; the app's own server (its
/// socket and `/__aio/`) never cached; older builds' caches dropped when
/// it takes over. A new build is a new `sw.js`, which installs it (pure).
pub fn service_worker(id: &str, files: &[(String, Vec<u8>)], served: bool) -> String {
    let url = |f: &str| format!("./{}", f.replace('%', "%25").replace('#', "%23").replace('?', "%3F"));
    let list: Vec<String> = std::iter::once("./".to_string()).chain(files.iter().map(|(f, _)| url(f))).collect();
    let sri: serde_json::Map<String, serde_json::Value> = files
        .iter()
        .filter(|(f, _)| pinned(f))
        .map(|(f, b)| {
            use base64::Engine;
            let d = base64::engine::general_purpose::STANDARD.encode(Sha256::digest(b));
            (url(f), format!("sha256-{d}").into())
        })
        .collect();
    let prefix = serde_json::Value::from(format!("aio-{id}-"));
    let version = serde_json::Value::from(format!("aio-{id}-{}@", content_hash(files)));
    format!(
        "// Made by aio: the offline cache of this build.\n\
const PREFIX = {prefix};\n\
const V = {version} + self.registration.scope;\n\
const FILES = {};\n\
const SRI = {};\n\
const SERVED = {served};\n\
const req = (f) => new Request(f, {{ cache: 'reload', integrity: SRI[f] || '' }});\n\
self.addEventListener('install', (e) => {{\n\
  e.waitUntil(caches.open(V).then((c) => c.addAll(FILES.map(req))).then(() => self.skipWaiting(), (err) => {{\n\
    console.error('[aio] the offline cache of this build was not installed: ' + err + '; the previous build keeps serving');\n\
    throw err;\n\
  }}));\n\
}});\n\
self.addEventListener('activate', (e) => {{\n\
  const scope = self.registration.scope;\n\
  const older = (k) => k !== V && k.startsWith(PREFIX) && k.endsWith('@' + scope);\n\
  e.waitUntil(caches.keys().then((ks) => Promise.all(ks.filter(older).map((k) => caches.delete(k))))\n\
    .then(() => self.clients.claim()));\n\
}});\n\
self.addEventListener('notificationclick', (e) => {{\n\
  e.notification.close();\n\
  e.waitUntil(self.clients.matchAll({{ type: 'window' }}).then((cs) => (cs[0] ? cs[0].focus() : self.clients.openWindow(self.registration.scope))));\n\
}});\n\
const own = (u) => u.pathname === '/ws' || u.pathname.startsWith('/__aio/');\n\
self.addEventListener('fetch', (e) => {{\n\
  const r = e.request, u = new URL(r.url);\n\
  if (r.method !== 'GET' || u.origin !== location.origin || own(u)) return;\n\
  const page = (c) => c.match('./');\n\
  const nav = r.mode === 'navigate';\n\
  e.respondWith(caches.open(V).then((c) => ((SERVED && nav) ? Promise.resolve() : c.match(r, {{ ignoreSearch: true }})).then((hit) => hit || fetch(r).then(\n\
    (res) => (nav && res.status === 404 ? page(c).then((p) => p || res) : res),\n\
    (err) => {{ if (nav) return page(c); throw err; }}))));\n\
}});\n",
        serde_json::Value::from(list),
        serde_json::Value::Object(sri)
    )
}

/// An icon file and its size in px.
type Icon = (String, u32);

/// The icons: the app's own scaled, else its generated
/// one; and why its own can't be installed, if so.
fn icons(app: &App, hue: u16, out: &Path) -> Result<(Vec<Icon>, Option<String>), String> {
    let own = app.icon.as_ref().map(std::fs::read).transpose().map_err(|e| e.to_string())?;
    let img = own
        .map(|b| image::load_from_memory_with_format(&b, image::ImageFormat::Png).map(|i| i.to_rgba8()))
        .transpose()
        .map_err(|e| format!("icon.png: {e}"))?;
    let problem = img.as_ref().and_then(|i| icon_problem(i.width(), i.height()));
    let mut made = Vec::new();
    for n in [180, 192, 512] {
        let png = match &img {
            Some(i) => {
                let s = image::imageops::resize(i, n, n, image::imageops::FilterType::Lanczos3);
                aio_config::icon::png(s.as_raw(), n, n)
            }
            None => aio_config::icon::icon_png(crate::build::label(app), hue, n),
        };
        let f = format!("icon-{n}.png");
        std::fs::write(out.join(&f), png).map_err(|e| e.to_string())?;
        made.push((f, n));
    }
    Ok((made, problem))
}

/// The portrait screens iOS shows a Home Screen app's launch image on, as
/// CSS width, height and pixel ratio (aio's `IOS_SCREENS`): iOS takes the
/// one whose media query matches the screen exactly; any other shows the
/// manifest's background colour.
pub const IOS_SCREENS: [(u32, u32, u32); 21] = [
    (320, 568, 2),
    (375, 667, 2),
    (414, 736, 3),
    (375, 812, 3),
    (414, 896, 2),
    (414, 896, 3),
    (390, 844, 3),
    (428, 926, 3),
    (393, 852, 3),
    (430, 932, 3),
    (402, 874, 3),
    (440, 956, 3),
    (768, 1024, 2),
    (744, 1133, 2),
    (810, 1080, 2),
    (820, 1180, 2),
    (834, 1112, 2),
    (834, 1194, 2),
    (834, 1210, 2),
    (1024, 1366, 2),
    (1032, 1376, 2),
];

/// `#rgb` or `#rrggbb` as RGB; black otherwise.
fn rgb(hex: &str) -> [u8; 3] {
    let h = hex.trim_start_matches('#');
    let h: String = if h.len() == 3 { h.chars().flat_map(|c| [c, c]).collect() } else { h.into() };
    let at = |i: usize| h.get(i..i + 2).and_then(|x| u8::from_str_radix(x, 16).ok()).unwrap_or(0);
    [at(0), at(2), at(4)]
}

/// iOS launch images (aio's `iosLaunchImages`): per [`IOS_SCREENS`] the
/// splash image `png` on its light background `bg`, the size the page
/// draws it (its box `w`×`h` in CSS px, no wider than the screen), so the
/// launch image and the page's splash are one picture. Each file's name,
/// its `<link>` and its PNG.
pub fn launch_images(png: &[u8], (w, h): (u32, u32), bg: &str) -> Result<Vec<(String, String, Vec<u8>)>, String> {
    use image::{ImageEncoder, codecs::png};
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .map_err(|e| format!("splash: {e}"))?
        .to_rgba8();
    let [r, g, b] = rgb(bg);
    IOS_SCREENS
        .iter()
        .map(|&(cw, ch, ratio)| {
            let (sw, sh) = (cw * ratio, ch * ratio);
            let dw = w.min(cw) * ratio;
            let dh = (u64::from(dw) * u64::from(h) / u64::from(w.max(1))) as u32;
            // Contained in its box, as `object-fit` draws it by default.
            let fit = (f64::from(dw) / f64::from(img.width())).min(f64::from(dh) / f64::from(img.height()));
            let (iw, ih) = ((f64::from(img.width()) * fit) as u32, (f64::from(img.height()) * fit) as u32);
            let mut screen = image::RgbaImage::from_pixel(sw, sh, image::Rgba([r, g, b, 255]));
            let scaled = image::imageops::resize(&img, iw.max(1), ih.max(1), image::imageops::FilterType::Triangle);
            image::imageops::overlay(
                &mut screen,
                &scaled,
                i64::from((sw - iw) / 2),
                i64::from(sh.saturating_sub(ih) / 2),
            );
            let rgb = image::DynamicImage::ImageRgba8(screen).to_rgb8();
            let mut out = Vec::new();
            png::PngEncoder::new_with_quality(&mut out, png::CompressionType::Fast, png::FilterType::Adaptive)
                .write_image(&rgb, sw, sh, image::ExtendedColorType::Rgb8)
                .map_err(|e| e.to_string())?;
            let file = format!("apple-splash-{sw}x{sh}.png");
            let link = format!(
                "<link rel=\"apple-touch-startup-image\" media=\"screen and (device-width: {cw}px) and \
                 (device-height: {ch}px) and (-webkit-device-pixel-ratio: {ratio}) and (orientation: portrait)\" \
                 href=\"./{file}\">"
            );
            Ok((file, link, out))
        })
        .collect()
}

/// cargo's arguments for [`bind`]'s build of `pkg` (pure): a release, or
/// a dev build with `dev`'s feature (aio-rs's dev tooling), its libraries
/// optimized and no debug info linked (wasm-bindgen drops it anyway): half
/// the module, so wasm-bindgen and the page's load take half the time of
/// each Rust edit; the app's own crates built as written.
fn cargo_args<'a>(pkg: &'a str, dev: Option<&'a str>) -> Vec<&'a str> {
    let mut args = vec!["rustc", "--target", "wasm32-unknown-unknown", "-p", pkg, "--lib", "--crate-type", "cdylib"];
    match dev {
        Some(tooling) => args.extend([
            "--features",
            tooling,
            "--config",
            "profile.dev.package.\"*\".opt-level=1",
            "--config",
            "profile.dev.strip=\"debuginfo\"",
        ]),
        None => args.push("--release"),
    }
    args
}

/// What a page needs beside the bundle `wasm-bindgen` made in `out`: the
/// icons, the manifest, the page (with its splash, from the app's folder
/// `dir`) and its service worker. `body`: the page's first paint. What was
/// said (an icon that can't be installed).
pub fn pack(
    app: &App,
    cfg: &aio::config::Config,
    dir: &Path,
    out: &Path,
    body: &str,
    dev: bool,
) -> Result<Vec<String>, String> {
    let hue = cfg.hue();
    let (icons, problem) = icons(app, hue, out)?;
    let made: Vec<Icon> = icons.iter().filter(|(_, n)| *n != 180).cloned().collect();
    std::fs::write(out.join("manifest.webmanifest"), manifest(&app.name, hue, &made)).map_err(|e| e.to_string())?;
    let mut head = head(&app.name, hue, "icon-192.png", "icon-180.png");
    // The splash, over the page until the app has drawn its view, and as
    // iOS's launch images (a release build: a Home Screen app starts on it).
    let splash = match &cfg.build.splash {
        Some(s) => {
            let png = dir.join(&s.image);
            let bytes = std::fs::read(&png).map_err(|e| format!("{}: {e}", png.display()))?;
            std::fs::write(out.join("splash.png"), &bytes).map_err(|e| e.to_string())?;
            let s = aio::config::Splash { png: Box::leak(bytes.into_boxed_slice()), ..s.clone() };
            let bg = s.background.as_ref().map(|[l, d]| [l.as_str(), d.as_str()]);
            let colors = aio_shell::splash::colors(hue, bg);
            if let (false, Some(b)) = (dev, aio_shell::splash::image_box(&s)) {
                for (file, link, png) in launch_images(s.png, b, &colors[0][0])? {
                    std::fs::write(out.join(file), png).map_err(|e| e.to_string())?;
                    head = format!("{head}\n{link}");
                }
            }
            aio_shell::splash::markup(&s, &app.name, &app.version, &colors, "./splash.png", true)
        }
        None => String::new(),
    };
    // `[build] assets`: at their URL paths, held offline by the worker.
    for (url, file) in aio::config::asset_files(dir, &cfg.build.assets)? {
        let to = out.join(aio::config::asset_path(&url));
        std::fs::create_dir_all(to.parent().unwrap_or(out)).map_err(|e| e.to_string())?;
        std::fs::copy(&file, &to).map_err(|e| format!("{}: {e}", file.display()))?;
    }
    let page = index_html(&app.name, &cfg.app, &head, body).replacen("<body>\n", &format!("<body>\n{splash}"), 1);
    std::fs::write(out.join("index.html"), page).map_err(|e| e.to_string())?;
    // A standalone app has no server to take its console: none is sent.
    let logged = dev || (cfg.app.mode != aio::config::Mode::Standalone && cfg.server.page_log);
    std::fs::write(out.join("page.js"), page_js(dev, logged)).map_err(|e| e.to_string())?;
    std::fs::write(out.join("start.js"), start_js(dev)).map_err(|e| e.to_string())?;
    std::fs::write(out.join("worker.js"), format!("{BASE}{WORKER_JS}")).map_err(|e| e.to_string())?;
    let mut files = site_files(out, out).map_err(|e| e.to_string())?;
    // iOS fetches its launch images itself: the worker needn't hold them.
    files.retain(|(f, _)| f != "sw.js" && !f.starts_with("apple-splash-"));
    files.sort();
    let served = cfg.app.mode != aio::config::Mode::Standalone;
    std::fs::write(out.join("sw.js"), service_worker(&app.id, &files, served)).map_err(|e| e.to_string())?;
    Ok(problem.into_iter().collect())
}

/// Every file under `at`, as its `/` path from `root` and its bytes (the
/// bundle's `snippets/` too: the page needs them offline).
fn site_files(root: &Path, at: &Path) -> std::io::Result<Vec<(String, Vec<u8>)>> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(at)? {
        let p = e?.path();
        if p.is_dir() {
            out.extend(site_files(root, &p)?);
        } else if let Ok(rel) = p.strip_prefix(root) {
            let rel: Vec<String> = rel.iter().map(|c| c.to_string_lossy().into_owned()).collect();
            out.push((rel.join("/"), std::fs::read(&p)?));
        }
    }
    Ok(out)
}

/// Build package `pkg`'s lib `lib` (in workspace `root`, built into
/// `target`) for the browser, as a cdylib (natively it stays a plain
/// library), bound into `out`: its `.wasm` and `app.js`; `dev`: a debug
/// build with aio-rs's `dev` (its dev tooling in the page: the audits, the
/// Redux DevTools bridge, live CSS), aio-rs being `aio` to `pkg`.
/// `wasm-bindgen` must be the version the lock has, as they talk to each
/// other; `fetch`: aio's is fetched when none is here. `hot` (a dev build's:
/// this ax and a folder): the module set up for hot-patching, its lib's
/// rustc call recorded in the folder; what a patch is linked against.
#[allow(clippy::too_many_arguments)]
pub fn bind(
    root: &Path,
    target: &Path,
    (pkg, lib, aio): (&str, &str, &str),
    app: &App,
    out: &Path,
    dev: bool,
    fetch: bool,
    hot: Option<(&Path, &Path)>,
) -> Result<Option<hot::Base>, String> {
    // A new app has no lock before its first build: it names the wasm-bindgen.
    if !root.join("Cargo.lock").is_file() {
        let made = Command::new("cargo").arg("generate-lockfile").current_dir(root).status();
        if !made.is_ok_and(|s| s.success()) {
            return Err("cargo generate-lockfile failed".into());
        }
    }
    let lock = std::fs::read_to_string(root.join("Cargo.lock")).map_err(|e| format!("Cargo.lock: {e}"))?;
    let want = locked(&lock, "wasm-bindgen").ok_or("Cargo.lock: no wasm-bindgen (the app's lib uses aio::web)")?;
    let bindgen = match fetch {
        true => crate::setup::wasm_bindgen(&want)?,
        false => crate::setup::wasm_bindgen_at(&want)
            .map_err(|found| format!("the web build needs wasm-bindgen {want} ({found}): ax setup fetches it"))?,
    };
    let tooling = format!("{aio}/dev");
    let mut cargo = crate::freeze::cargo("the web build", &cargo_args(pkg, dev.then_some(tooling.as_str())))?;
    if !dev {
        crate::build::remap(&mut cargo, root);
        // The linker's relocations kept: the split reads them (`split`).
        cargo.args(["--", "-C", "link-arg=--emit-relocs"]);
    }
    // Hot-patching: the lib's rustc call recorded (ax as cargo's workspace
    // wrapper), the linker's symbols kept.
    if let (true, Some((ax, record))) = (dev, hot) {
        cargo.env("RUSTC_WORKSPACE_WRAPPER", ax).env(crate::hotpatch::RUSTC_ENV, record);
        cargo.args(["--", "-C", "link-arg=--emit-relocs"]);
    }
    // Cargo's errors are said here and kept for the dev page (`ax dev --web`).
    // As it comes (it was silent until the end: 25 s of nothing on a first build).
    cargo.current_dir(root).env("AIORT_APP_ID", &app.id).env("AIORT_APP_VERSION", &app.version);
    let o = crate::hotpatch::cargo_out::run(&mut cargo, false, &Default::default()).map_err(|e| e.to_string())?;
    let said = o.said;
    if !o.status.success() {
        let why = "the web build failed (rustup target add wasm32-unknown-unknown, if it is missing)";
        return Err(match fatal(&said) {
            e if e.is_empty() => why.into(),
            e => format!("{why}\n{e}"),
        });
    }
    let profile = if dev { "debug" } else { "release" };
    let linked = target.join("wasm32-unknown-unknown").join(profile).join(format!("{lib}.wasm"));
    // A release's module without what the page never reads, bound; the linker's kept for the split.
    let pre = (!dev).then(|| std::fs::read(&linked)).transpose().map_err(|e| e.to_string())?;
    let mut base = None;
    let wasm = match &pre {
        Some(b) => {
            let to = linked.with_extension("stripped.wasm");
            std::fs::write(&to, split::strip(b, true)?).map_err(|e| e.to_string())?;
            to
        }
        None if hot.is_some() => match hot::prepare(&std::fs::read(&linked).map_err(|e| e.to_string())?) {
            Ok((wasm, b)) => {
                let to = linked.with_extension("hot.wasm");
                std::fs::write(&to, wasm).map_err(|e| e.to_string())?;
                base = Some(b);
                to
            }
            Err(e) => {
                eprintln!("ax dev: no hot-patching of this build ({e}): a Rust edit rebuilds");
                linked
            }
        },
        None => linked,
    };
    _ = std::fs::remove_dir_all(out);
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    // A release's names as the linker has them: the split matches them to its relocations.
    let ok = Command::new(&bindgen)
        .args(["--target", "web", "--no-typescript", "--out-name", BUNDLE, "--out-dir"])
        .arg(out)
        .args((!dev).then_some("--no-demangle"))
        .arg(&wasm)
        .status()
        .map_err(|e| e.to_string())?;
    if !ok.success() {
        return Err(format!("wasm-bindgen could not bind {}", wasm.display()));
    }
    if let Some(pre) = pre {
        let bound = out.join(format!("{BUNDLE}_bg.wasm"));
        let post = std::fs::read(&bound).map_err(|e| e.to_string())?;
        let main = match split::split(&pre, &post) {
            Ok(Some(s)) => {
                for (f, b) in &s.chunks {
                    std::fs::write(out.join(f), b).map_err(|e| e.to_string())?;
                }
                s.main
            }
            Ok(None) => split::strip(&post, false)?,
            Err(e) => {
                eprintln!("ax build: the lazy views stay in the one module ({e})");
                split::strip(&post, false)?
            }
        };
        std::fs::write(&bound, main).map_err(|e| e.to_string())?;
    }
    // The glue held in a worker; a release's without its indentation and
    // comment lines: ~20% less to fetch.
    let glue = out.join(format!("{BUNDLE}.js"));
    let js = held(&std::fs::read_to_string(&glue).map_err(|e| e.to_string())?)?;
    let js = if dev { js } else { trimmed(&js).unwrap_or(js) };
    std::fs::write(&glue, js).map_err(|e| e.to_string())?;
    Ok(base)
}

/// `js` through [`aio::shrink::js`]; None: it keeps its text as written.
fn trimmed(js: &str) -> Option<String> {
    let mut out = vec![0; aio::shrink::js(js.as_bytes(), &mut [])?];
    aio::shrink::js(js.as_bytes(), &mut out)?;
    String::from_utf8(out).ok()
}

/// What a site's build said, and what a patch of it is linked against.
type Made = Result<(Vec<String>, Option<hot::Base>), String>;

/// An app's web build: its package, its cdylib lib, and where cargo builds.
struct Site {
    pkg: crate::dev::Pkg,
    app: App,
    cfg: aio::config::Config,
    lib: String,
    /// The package with that lib: the app's own, or the library its launcher uses.
    lib_pkg: String,
    /// What that package calls aio-rs.
    aio: String,
    target: PathBuf,
    root: PathBuf,
}

/// The package and name of the library a web build binds (pure): the
/// first of `dirs` (the app's package, then the library its launcher uses,
/// as `ax create` lays them out) whose package has a library and uses
/// wasm-bindgen (its `#[wasm_bindgen(start)]`).
pub(crate) fn web_lib(meta: &serde_json::Value, dirs: &[PathBuf]) -> Option<(String, String)> {
    let pkgs = meta["packages"].as_array()?;
    let lib_kinds = ["lib", "rlib", "cdylib"];
    let is_lib = |t: &&serde_json::Value| {
        t["kind"].as_array().is_some_and(|k| k.iter().any(|k| lib_kinds.iter().any(|l| k == l)))
    };
    let binds = |p: &serde_json::Value| {
        p["dependencies"].as_array().is_some_and(|d| d.iter().any(|d| d["name"] == "wasm-bindgen"))
    };
    dirs.iter().find_map(|d| {
        let p = pkgs.iter().find(|p| p["manifest_path"].as_str().and_then(|m| Path::new(m).parent()) == Some(d))?;
        let t = p["targets"].as_array()?.iter().find(is_lib).filter(|_| binds(p))?;
        Some((p["name"].as_str()?.to_string(), t["name"].as_str()?.replace('-', "_")))
    })
}

/// What package `pkg` calls aio-rs: its dependency's rename (`aio`, as
/// `ax create` writes it), else `aio-rs` (pure).
fn aio_named(meta: &serde_json::Value, pkg: &str) -> String {
    let pkgs = meta["packages"].as_array().into_iter().flatten();
    let deps = pkgs.filter(|p| p["name"] == pkg).flat_map(|p| p["dependencies"].as_array().into_iter().flatten());
    let aio = deps.filter(|d| d["name"] == "aio-rs" && d["kind"].is_null()).find_map(|d| d["rename"].as_str());
    aio.unwrap_or("aio-rs").to_string()
}

impl Site {
    fn of(name: &str) -> Result<Self, String> {
        let meta = crate::dev::metadata()?;
        let pkg = crate::dev::web_pkg(&meta, name)?;
        let (app, cfg) = app_of(&pkg)?;
        let (lib_pkg, lib) = web_lib(&meta, &pkg.dirs).ok_or_else(|| {
            format!(
                "{}: no library for the web: the web build needs the app's library with, for wasm32, \
                     aio's `web` feature and wasm-bindgen, and a #[wasm_bindgen(start)] fn that sets SOURCE \
                     and calls aio::web::launch(App) (`ax create` writes them)",
                pkg.name
            )
        })?;
        let dir = |k: &str| meta[k].as_str().map(PathBuf::from).ok_or(format!("cargo metadata: no {k}"));
        let aio = aio_named(&meta, &lib_pkg);
        Ok(Self { lib, lib_pkg, aio, target: dir("target_directory")?, root: dir("workspace_root")?, pkg, app, cfg })
    }

    /// Its CSS step, the bundle and the page into `out` (`hot`: [`bind`]'s);
    /// what was said, and what a patch is linked against.
    fn make(&self, out: &Path, dev: bool, hot: Option<(&Path, &Path)>) -> Made {
        if let Some(did) = crate::css::step(&self.cfg, &app_dir(&self.pkg)?)? {
            eprintln!("ax build: {did}");
        }
        let lib = (self.lib_pkg.as_str(), self.lib.as_str(), self.aio.as_str());
        let base = bind(&self.root, &self.target, lib, &self.app, out, dev, true, hot)?;
        Ok((pack(&self.app, &self.cfg, &app_dir(&self.pkg)?, out, "", dev)?, base))
    }
}

/// The page's wasm in `out`: its bytes, and gzipped (what a browser fetches).
pub fn wasm_size(out: &Path) -> std::io::Result<(u64, u64)> {
    use std::io::Write;
    let b = std::fs::read(out.join(format!("{BUNDLE}_bg.wasm")))?;
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    gz.write_all(&b)?;
    Ok((b.len() as u64, gz.finish()?.len() as u64))
}

/// `ax build <name> --web [--dev]`: the site in `dist/<id>-web/`, and
/// what was said, its wasm's size too. `dev`: a debug build whose page
/// shows its errors on it.
pub fn build(name: &str, dev: bool) -> Result<(String, PathBuf), String> {
    let site = Site::of(name)?;
    let profile = if dev { "dev" } else { "release" };
    eprintln!("ax build: {} {} for the web ({profile})…", site.app.name, site.app.version);
    let out = crate::build::dist(&site.root, &site.cfg).join(format!("{}-web", site.app.id));
    let (said, _) = site.make(&out, dev, None)?;
    let (raw, gz) = wasm_size(&out).map_err(|e| e.to_string())?;
    let mut size = format!("wasm: {} KB, {} KB gzipped", raw / 1000, gz / 1000);
    // The lazy views' chunks (`split`): fetched when their views first show.
    let chunks: Vec<u64> = std::fs::read_dir(&out)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name().to_str().is_some_and(|f| f.starts_with(&format!("{BUNDLE}.")) && f.ends_with(".wasm"))
        })
        .filter_map(|e| e.metadata().ok().map(|m| m.len()))
        .collect();
    if !chunks.is_empty() {
        size += &format!("; lazy views: {} KB in {} chunks", chunks.iter().sum::<u64>() / 1000, chunks.len());
    }
    let lines: Vec<String> =
        said.into_iter().chain([size, format!("{}: put it on any HTTPS host", out.display())]).collect();
    Ok((lines.join("\n"), out))
}

/// Cargo's errors in what it said (`stderr`), each with its source lines,
/// without its warnings or progress (aio's diagnostic page: the fatal ones) (pure).
pub fn fatal(stderr: &str) -> String {
    let error = |l: &&str| l.starts_with("error") && !l.starts_with("error: could not compile");
    let blocks = stderr.split("\n\n").map(|b| b.lines().skip_while(|l| !error(l)).collect::<Vec<_>>().join("\n"));
    blocks.filter(|b| !b.is_empty()).collect::<Vec<_>>().join("\n\n")
}

/// The dev page's live reload (`page.js` in a dev build), asked of `ax
/// dev --web` and held there until something changes (`/aio-dev/build`):
/// markup edited since the build goes to the page's views (`__aioHot`:
/// built again in place, their state kept); a patch of its Rust is loaded
/// into the page's module (its memory and table, `__aioWasm`; each call
/// into the page by the table slot it imports, from `s`; wasm-bindgen's
/// descriptors, dead code there, as no-ops from `d`) and its map
/// handed in (`__aioPatched`), one patch at a time, one that is no longer
/// the newest skipped; a new build reloads the page (its cells come back
/// from the browser's storage); a failed build's errors show on the page,
/// open (aio's diagnostic page), until a good one reloads it. With no dev
/// server (a static `--dev` build) it stops asking.
pub const RELOAD: &str = "(() => {
  let v = null, n = null, failed = null, hot = 0, patch = 0, patching = Promise.resolve();
  const wasm = () => window.__aioWasm || new Promise((r) => addEventListener('aio:wasm', () => r(window.__aioWasm), { once: true }));
  const load = async (p) => {
    const w = await wasm(), t = w.__aio_table;
    const r = await fetch('/aio-dev/patch?g=' + p.g, { cache: 'no-store' });
    if (r.status === 409 || p.g !== patch) return;
    if (!r.ok) throw new Error('/aio-dev/patch: ' + r.status);
    if (t.length < p.table) t.grow(p.table - t.length);
    const env = { memory: w.memory, __indirect_function_table: t, __stack_pointer: w.__aio_sp };
    const d = new Proxy({}, { get: () => () => {} });
    await WebAssembly.instantiate(await r.arrayBuffer(), { env, d, s: new Proxy({}, { get: (_, k) => t.get(+k) }) });
    window.__aioPatched(p.map.join(','));
  };
  const tick = () => fetch('/aio-dev/build' + (v === null ? '' : '?after=' + v), { cache: 'no-store' }).then((r) => (r.ok ? r.json() : null)).then((b) => {
    if (!b) return;
    if (n !== null && b.n !== n) return location.reload();
    n = b.n;
    v = b.v;
    if (b.patch && b.patch.g !== patch) {
      const p = b.patch;
      patch = p.g;
      patching = patching.then(() => load(p)).catch((e) => {
        console.error('[aio] the patch did not load:', e);
        if (window.__aioDiag) window.__aioDiag('hot-patch failed', String(e), true);
      });
    }
    if (b.hot && b.hot.g !== hot && window.__aioHot) {
      hot = b.hot.g;
      window.__aioHot(JSON.stringify(b.hot));
    }
    if (b.error && b.error !== failed && window.__aioDiag) window.__aioDiag('build failed', b.error, true);
    failed = b.error || null;
    tick();
  }, () => setTimeout(tick, 1000));
  tick();
})();";

/// The page's console in a log (`page.js`, aio's console intercept):
/// every level, uncaught errors and refused promises, each line cut at
/// 4 KB, sent in batches to `ENDPOINT`: `ax dev --web`'s `/aio-dev/log` in
/// a dev build, the app server's `/__aio/log` in a release (at most 200
/// waiting: past that they are counted, and the count said). The page's own
/// console still shows them. Where nothing takes them (a static host, a
/// server with `[server] page_log = false`: a 404) it stops sending.
pub const CONSOLE: &str = "(() => {
  const MAX = 4096, CAP = 200
  let queue = [], dropped = 0, timer = 0, off = false
  const text = (a) => {
    if (typeof a === 'string') return a
    if (a instanceof Error) return a.stack || String(a)
    try { return JSON.stringify(a) ?? String(a) } catch (_) { return String(a) }
  }
  const flush = () => {
    timer = 0
    if (off || !queue.length) return
    if (dropped) { queue.push({ level: 'warn', msg: `[aio] ${dropped} console lines were dropped (over ${CAP} waiting)` }); dropped = 0 }
    const body = JSON.stringify(queue)
    queue = []
    fetch('ENDPOINT', { method: 'POST', body, keepalive: body.length < 60000, headers: { 'content-type': 'application/json' } })
      .then((r) => { if (r.status === 404) off = true }, () => {})
  }
  const send = (level, args) => {
    if (off) return
    if (queue.length >= CAP) { dropped++; return }
    queue.push({ level, msg: Array.from(args, text).join(' ').slice(0, MAX) })
    if (!timer) timer = setTimeout(flush, 100)
  }
  for (const [name, level] of [['log', 'info'], ['info', 'info'], ['warn', 'warn'], ['error', 'error'], ['debug', 'debug']]) {
    const own = console[name].bind(console)
    console[name] = (...args) => { own(...args); send(level, args) }
  }
  addEventListener('error', (e) => send('error', ['[uncaught]', e.error || e.message]))
  addEventListener('unhandledrejection', (e) => send('error', ['[unhandled rejection]', e.reason]))
  addEventListener('pagehide', flush)
})();";

/// What the dev page's console sent (`CONSOLE`): one line per entry, its
/// level and text, terminal escapes taken out (pure).
pub fn page_lines(body: &[u8]) -> Vec<String> {
    let entries: Vec<serde_json::Value> = serde_json::from_slice(body).unwrap_or_default();
    entries
        .iter()
        .map(|e| {
            let level =
                e["level"].as_str().filter(|l| ["error", "warn", "info", "debug"].contains(l)).unwrap_or("info");
            let msg: String = (e["msg"].as_str().unwrap_or_default().chars().take(4096))
                .map(|c| if c.is_control() && c != '\n' && c != '\t' { '\u{fffd}' } else { c })
                .collect();
            format!("[page] {level}: {msg}")
        })
        .collect()
}

/// Whether a change to `changed` needs a new bundle: a stylesheet's needs
/// none (the page reads it again, `aio::use_css!`) (pure).
pub fn rebuilds(changed: &[PathBuf]) -> bool {
    changed.iter().any(|p| p.extension().is_none_or(|e| e != "css"))
}

/// Where `ax dev --web` serves: `port` (0: a free one); `expose`: to the
/// network (`0.0.0.0`) over TLS, behind a fresh key (aio's `--expose`),
/// else on loopback, open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Serve {
    pub port: u16,
    pub expose: bool,
}

impl Serve {
    /// The words after the app in `ax dev <app> --web …`, in any order (pure).
    pub fn parse(words: &[&str]) -> Result<Self, String> {
        let mut s = Self::default();
        let mut it = words.iter();
        while let Some(w) = it.next() {
            match w.split_once('=').map_or((*w, None), |(f, v)| (f, Some(v))) {
                ("--web", None) => {}
                ("--expose", None) => s.expose = true,
                ("--port", v) => {
                    let v = v.or_else(|| it.next().copied()).ok_or("--port needs a number")?;
                    s.port = v.parse().map_err(|_| format!("--port {v}: a port number, 0 to 65535 (0: a free one)"))?;
                }
                _ => return Err(format!("{w}: not a flag of ax dev <app> --web [--port N] [--expose]")),
            }
        }
        Ok(s)
    }
}

/// `ax dev <name> --web`: the web build (debug) served as `serve` says,
/// built again on each saved change to its sources: a stylesheet shows at
/// once (`/aio-dev/sheet`, the page asks), any other edit (`rsx!`, Rust)
/// builds the bundle again and the open tabs reload (`/aio-dev/build`). A
/// failed build keeps the last page. Runs until Ctrl-C.
pub fn dev(name: &str, serve: Serve) -> Result<String, String> {
    dev_then(name, serve, |_| Ok(()))
}

/// Whether app `name` has a web build ([`dev`] can serve it).
pub fn has_web(name: &str) -> bool {
    Site::of(name).is_ok()
}

/// [`dev`], with `ready` called on its port once it serves (`ax dev
/// --android` opens the page on the phone then); its error ends the run.
pub fn dev_then(
    name: &str,
    serve: Serve,
    ready: impl FnOnce(u16) -> Result<(), String> + Send + 'static,
) -> Result<String, String> {
    let site = Site::of(name)?;
    let out = site.target.join("aio").join("web-dev").join(&site.app.id);
    eprintln!("ax dev: {} for the web (dev)…", site.app.name);
    let mut built = crate::reload::Built::snapshot(&site.pkg.dirs);
    let ax = std::env::current_exe().map_err(|e| e.to_string())?;
    let hot_dir = site.target.join("aio").join("web-dev").join(format!("{}-hot", site.app.id));
    let dev = DevBuild { out: out.clone(), ax, hot_dir, site };
    let mut hot = dev.session(dev.make()?.1);
    let live = std::sync::Arc::new(Live::default());
    let dirs: Vec<PathBuf> = dev.site.pkg.dirs.iter().filter_map(|d| d.canonicalize().ok()).collect();
    let paths = crate::dev::watched(&dev.site.pkg, Path::new(""), &crate::dev::Watching::All);
    let (w, rx) = crate::watch::Watch::new(&paths)?;
    // What the wasm build read beside the app's folders (`aio.toml`, an aio-rs checkout).
    let deps = dev.site.target.join("wasm32-unknown-unknown").join("debug").join(format!("{}.wasm", dev.site.lib));
    w.add_files(crate::dev::deps_of(&deps));
    let cwd = std::env::current_dir().unwrap_or_default();
    let said_live = live.clone();
    let http = aio::http::Http::default()
        .shell(&out)
        // The page's live reload: held until something changed since `?after=`.
        .route(aio::http::Route::get("/aio-dev/build", move |r| {
            let (live, after) = (said_live.clone(), r.query("after").and_then(|v| v.parse::<u64>().ok()));
            async move {
                let b = live.after(after, std::time::Duration::from_secs(20)).await;
                aio::http::Reply::json(&b).header("cache-control", "no-store")
            }
        }))
        // The page's console, in this log (`CONSOLE`).
        .route(aio::http::Route::post("/aio-dev/log", |r| {
            for l in page_lines(r.body()) {
                eprintln!("ax dev: {l}");
            }
            async { aio::http::Reply::text("").status(204) }
        }))
        .route(aio::http::Route::get("/aio-dev/sheet", move |r| {
            let file = r.query("path").and_then(|p| Path::new(p).canonicalize().ok());
            let ours =
                file.filter(|f| f.extension().is_some_and(|e| e == "css") && dirs.iter().any(|d| f.starts_with(d)));
            async move {
                match ours.map(std::fs::read_to_string) {
                    Some(Ok(text)) => {
                        aio::http::Reply::json(&serde_json::json!({ "text": text })).header("cache-control", "no-store")
                    }
                    _ => aio::http::Reply::text("not a stylesheet of the app").status(404),
                }
            }
        }));
    // Exposed: over TLS (aio-rs's own root, as an exposed app), behind a key
    // made for this run, which the printed link carries once (a cookie then).
    let tls = match serve.expose {
        true => {
            Some(aio::tls::server(&aio::paths::tool_home().join("tls"), &dev.site.app.id).map_err(|e| e.to_string())?)
        }
        false => None,
    };
    let token = match serve.expose {
        true => format!("{}{}", aio::uuid_v4().simple(), aio::uuid_v4().simple()),
        false => String::new(),
    };
    aio::rt().block_on(async move {
        let host = if serve.expose { "0.0.0.0" } else { "127.0.0.1" };
        let l = tokio::net::TcpListener::bind((host, serve.port))
            .await
            .map_err(|e| format!("{host}:{}: {e}", serve.port))?;
        let port = l.local_addr().map_err(|e| e.to_string())?.port();
        match serve.expose {
            true => {
                let at = aio::pairing::reached_at(Some(host), &aio::tls::local_addrs());
                eprintln!(
                    "ax dev: https://{at}:{port}/?token={token} (open to the network over TLS, behind this run's key; \
                     the browser asks to trust aio-rs's own certificate; Ctrl-C ends it)"
                );
            }
            false => eprintln!("ax dev: http://127.0.0.1:{port}/ (Ctrl-C ends it)"),
        }
        let patch_live = live.clone();
        // The newest patch's module; 409: the page asked for an older one.
        let http = http.route(aio::http::Route::get("/aio-dev/patch", move |r| {
            let (g, wasm) = patch_live.wasm.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let asked = r.query("g").and_then(|v| v.parse::<u64>().ok());
            async move {
                match asked == Some(g) {
                    true => {
                        aio::http::Reply::stream("application/wasm", futures_util::stream::once(async move { wasm }))
                    }
                    false => aio::http::Reply::text("not the newest patch").status(409),
                }
            }
        }));
        let opts = aio::sync::Options { open: !serve.expose, token, tls, http, ..Default::default() };
        tokio::spawn(aio::sync::serve_with(aio::Handle::<DevSite>::memory(), l, opts));
        tokio::task::spawn_blocking(move || ready(port)).await.map_err(|e| e.to_string())??;
        tokio::task::spawn_blocking(move || {
            let site = &dev.site;
            while let Ok(first) = rx.recv() {
                // Markup alone: to the open pages at the first save, their views
                // built again in place, nothing built; again once the burst settled.
                let early = built.edits(&first, &dev.site.root);
                if let Some(edits) = &early {
                    live.hot(crate::reload::params(edits));
                }
                // A burst of saves is one change. Rust: built from its first
                // saves while it settles, kept if the burst was just these
                // (each file as the build found it): the quiet costs no time.
                // A Rust edit of the page's crate is patched into the open
                // pages ([`DevBuild::remake`]).
                let (changed, ahead) = std::thread::scope(|s| {
                    let (hot, dev, edited) = (&mut hot, &dev, first.clone());
                    let begin = move || {
                        let read = crate::dev::contents(&edited);
                        let snapshot = crate::reload::Built::snapshot(&dev.site.pkg.dirs);
                        (read, snapshot, std::time::Instant::now(), s.spawn(move || dev.remake(hot, &edited)))
                    };
                    let job = (early.is_none() && rebuilds(&first)).then(begin);
                    let changed = crate::watch::settle(&rx, first);
                    (changed, job.map(|(read, snap, at, h)| (read, snap, at, h.join())))
                });
                let said = crate::watch::describe(&changed, &cwd);
                if !rebuilds(&changed) {
                    eprintln!("ax dev: {said} changed: restyled");
                    continue;
                }
                if let Some(edits) = built.edits(&changed, &site.root) {
                    if early.as_ref() != Some(&edits) {
                        live.hot(crate::reload::params(&edits));
                    }
                    let n: usize = edits.iter().map(|(_, e)| e.len()).sum();
                    eprintln!(
                        "ax dev: {said} changed: {n} template{} to the page, state kept",
                        if n == 1 { "" } else { "s" }
                    );
                    continue;
                }
                let (made, snapshot, started) = match ahead {
                    Some((read, snapshot, at, Ok(made))) if crate::dev::contents(&changed) == read => {
                        (made, snapshot, at)
                    }
                    _ => {
                        eprintln!("ax dev: {said} changed — building…");
                        let started = std::time::Instant::now();
                        (dev.remake(&mut hot, &changed), crate::reload::Built::snapshot(&site.pkg.dirs), started)
                    }
                };
                match made {
                    Remade::Patched(p) => {
                        built = snapshot;
                        let ms = |d: std::time::Duration| d.as_millis();
                        if live.patched(p.wasm, p.table, &p.map) {
                            eprintln!("ax dev: fixed: the build is good again");
                        }
                        eprintln!(
                            "ax dev: {said} changed: hot-patched in {} ms (rustc {} ms, link {} ms), state kept",
                            ms(started.elapsed()),
                            ms(p.compile),
                            ms(p.link)
                        );
                    }
                    Remade::Built(Ok((_, base))) => {
                        w.add_files(crate::dev::deps_of(&deps));
                        built = snapshot;
                        hot = dev.session(base);
                        // Red to green: said; the page drops its errors as it reloads.
                        if live.built() {
                            eprintln!("ax dev: fixed: the build is good again");
                        }
                        eprintln!("ax dev: built in {} ms; the page reloads", started.elapsed().as_millis());
                    }
                    Remade::Built(Err(e)) => {
                        let first = e.lines().next().unwrap_or_default();
                        eprintln!("ax dev: {first}; the page keeps the last build and shows why");
                        live.failed(e);
                    }
                }
            }
            drop(w);
        })
        .await
        .map_err(|e| e.to_string())?;
        Ok(String::new())
    })
}

/// `ax dev --web`'s builds of `site` into `out`, this `ax` recording its
/// lib's rustc call and linking patches in `hot_dir`.
struct DevBuild {
    site: Site,
    out: PathBuf,
    ax: PathBuf,
    hot_dir: PathBuf,
}

impl DevBuild {
    fn make(&self) -> Made {
        self.site.make(&self.out, true, Some((&self.ax, &self.hot_dir.join("rustc"))))
    }

    /// Hot-patching of the build that gave `base`, if it can.
    fn session(&self, base: Option<hot::Base>) -> Option<hot::Hot> {
        let call = self.hot_dir.join("rustc").join(format!("{}.cdylib.json", self.site.lib));
        let h = base.map(|b| hot::Hot::new(b, &call, self.hot_dir.join("patch"), self.ax.clone()));
        h.transpose().map_err(|e| eprintln!("ax dev: no hot-patching ({e}): a Rust edit rebuilds")).ok().flatten()
    }

    /// A Rust edit: a patch the open pages load, else (when the patch
    /// can't take it) a new bundle they reload; rustc's errors as a
    /// failed build's.
    fn remake(&self, hot: &mut Option<hot::Hot>, changed: &[PathBuf]) -> Remade {
        if let Some(h) = hot.as_mut() {
            match h.patch(changed) {
                Ok(p) => return Remade::Patched(p),
                Err(crate::hotpatch::Refused::Code(e)) => return Remade::Built(Err(e)),
                Err(e) => eprintln!("ax dev: a full build: {e}"),
            }
        }
        Remade::Built(self.make())
    }
}

/// What a Rust edit made: a patch, or a build (or rustc's errors).
enum Remade {
    Patched(hot::Patch),
    Built(Made),
}

/// What `ax dev --web`'s page is told (`/aio-dev/build`): the build it
/// runs (`n`, a new one reloads it), the last build's errors, the markup
/// edited since (`hot`, by generation `g`), the patch of its Rust since
/// (`patch`: its generation `g`, the table size it needs, its map of slots,
/// flat; its module at `/aio-dev/patch`), each change a new version `v`.
#[derive(Default)]
struct Live {
    state: std::sync::Mutex<serde_json::Value>,
    changed: tokio::sync::Notify,
    /// The newest patch's module, by its generation.
    wasm: std::sync::Mutex<(u64, Vec<u8>)>,
}

impl Live {
    fn change(&self, f: impl FnOnce(&mut serde_json::Value)) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if s.is_null() {
            *s = serde_json::json!({ "v": 0, "n": 1, "error": null, "hot": null });
        }
        f(&mut s);
        s["v"] = (s["v"].as_u64().unwrap_or_default() + 1).into();
        self.changed.notify_waiters();
    }

    /// A good build: the page reloads on it. Whether the last one failed.
    fn built(&self) -> bool {
        let mut was = false;
        self.change(|s| {
            was = !s["error"].is_null();
            s["n"] = (s["n"].as_u64().unwrap_or_default() + 1).into();
            (s["error"], s["hot"], s["patch"]) = Default::default();
        });
        was
    }

    /// A patch of the page's Rust (each patch holds all edits since the
    /// build): the pages load it, the markup sent before is in it. Whether
    /// the last build failed.
    fn patched(&self, wasm: Vec<u8>, table: u32, map: &[(u32, u32)]) -> bool {
        let mut was = false;
        self.change(|s| {
            was = !s["error"].is_null();
            let g = s["patch"]["g"].as_u64().unwrap_or_default() + 1;
            *self.wasm.lock().unwrap_or_else(|e| e.into_inner()) = (g, wasm);
            let map: Vec<u32> = map.iter().flat_map(|&(a, b)| [a, b]).collect();
            s["patch"] = serde_json::json!({ "g": g, "table": table, "map": map });
            (s["error"], s["hot"]) = Default::default();
        });
        was
    }

    fn failed(&self, errors: String) {
        self.change(|s| s["error"] = errors.into());
    }

    /// Markup edited since the build (`crate::reload::params`'s).
    fn hot(&self, mut params: serde_json::Value) {
        self.change(|s| {
            let g = s["hot"]["g"].as_u64().unwrap_or_default() + 1;
            params["g"] = g.into();
            s["hot"] = params;
        });
    }

    /// The state, once its version is past `after` (at once without one),
    /// or as it is after `wait`.
    async fn after(&self, after: Option<u64>, wait: std::time::Duration) -> serde_json::Value {
        let until = tokio::time::Instant::now() + wait;
        loop {
            let notified = self.changed.notified();
            let now = self.now();
            if after.is_none_or(|a| now["v"].as_u64() != Some(a)) {
                return now;
            }
            if tokio::time::timeout_at(until, notified).await.is_err() {
                return self.now();
            }
        }
    }

    fn now(&self) -> serde_json::Value {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner()).clone();
        match s.is_null() {
            true => serde_json::json!({ "v": 0, "n": 1, "error": null, "hot": null }),
            false => s,
        }
    }
}

/// What `ax dev --web`'s server serves besides the page: nothing.
#[aio::cell(shared)]
pub struct DevSite {}

#[aio::methods]
impl DevSite {}

/// A test that can't run here says why and ends; where these tests
/// must run (`AIO_CI_WEB` set: CI, the tools fetched) it fails instead.
#[cfg(test)]
pub(crate) fn skipped(why: &str) {
    assert!(std::env::var_os("AIO_CI_WEB").is_none(), "{why}: AIO_CI_WEB is set, so this test must run");
    eprintln!("{why}: skipped");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_release_glue_loses_indentation_and_comment_lines() {
        let glue =
            "/**\n * The app.\n */\nexport function web() {\n    // Cast intrinsic for `x`.\n    wasm.web();\n}\n";
        assert_eq!(trimmed(glue).unwrap(), "/**\n* The app.\n*/\nexport function web() {\nwasm.web();\n}\n");
        assert_eq!(trimmed("const s = `a\n  b`;\n"), None, "a template literal over lines stays as it is");
    }

    #[test]
    fn the_glue_starts_the_app_only_off_a_worker() {
        let glue = "function fin(i) {\n    wasm = i.exports;\n    wasm.__wbindgen_start();\n    return wasm;\n}";
        let held = held(glue).unwrap();
        assert!(held.contains("    if (!globalThis.__aioWorker) wasm.__wbindgen_start();\n"), "{held}");
        assert_eq!(super::held("no start here").unwrap(), "no start here");
        let twice = format!("{glue}{glue}");
        assert!(super::held(&twice).is_err() && super::held("wasm.__wbindgen_start(x);").is_err());
        // The worker is not the app: it holds the start, and answers each job.
        assert!(WORKER_JS.starts_with("globalThis.__aioWorker = true;"));
        assert!(WORKER_JS.contains("await m.__aio_work(step, arg)"));
    }

    #[test]
    fn dev_web_takes_its_flags_in_any_order_and_refuses_others() {
        let p = |w: &str| Serve::parse(&w.split(' ').collect::<Vec<_>>());
        assert_eq!(p("--web"), Ok(Serve::default()));
        assert_eq!(p("--port 8080 --web --expose"), Ok(Serve { port: 8080, expose: true }));
        assert_eq!(p("--web --port=0"), Ok(Serve { port: 0, expose: false }));
        assert!(p("--web --port").unwrap_err().contains("needs a number"));
        assert!(p("--web --port 70000").unwrap_err().contains("0 to 65535"));
        assert!(p("--web --fresh").unwrap_err().starts_with("--fresh: not a flag of ax dev <app> --web"));
        assert!(p("--web --expose=yes").is_err());
    }

    #[test]
    fn the_lock_names_the_bindgen_the_build_needs() {
        let lock = "[[package]]\nname = \"wasm-bindgen\"\nversion = \"0.2.129\"\nsource = \"x\"\n\n\
                    [[package]]\nname = \"wasm-bindgen-futures\"\nversion = \"0.4.79\"\n";
        assert_eq!(locked(lock, "wasm-bindgen").as_deref(), Some("0.2.129"));
        assert_eq!(locked(lock, "wasm-bindgen-futures").as_deref(), Some("0.4.79"));
        assert_eq!(locked(lock, "serde"), None);
    }

    #[test]
    fn the_web_lib_is_the_apps_own_else_its_launchers_library() {
        let bindgen = serde_json::json!([{ "name": "wasm-bindgen", "target": "cfg(target_arch = \"wasm32\")" }]);
        let meta = serde_json::json!({ "packages": [
            // The launcher's library is the Android app: no wasm-bindgen.
            { "name": "hello-app", "manifest_path": "/w/app/Cargo.toml", "dependencies": [{ "name": "aio-rs-shell" }],
              "targets": [{ "name": "hello", "kind": ["bin"] }, { "name": "hello_app", "kind": ["lib"] }] },
            { "name": "hello", "manifest_path": "/w/Cargo.toml", "dependencies": bindgen,
              "targets": [{ "name": "hello", "kind": ["lib"] }] },
            { "name": "my-page", "manifest_path": "/p/Cargo.toml", "dependencies": bindgen,
              "targets": [{ "name": "my-page", "kind": ["cdylib"] }] },
            { "name": "plain", "manifest_path": "/q/Cargo.toml", "targets": [{ "name": "plain", "kind": ["lib"] }] },
        ]});
        let dirs = [PathBuf::from("/w/app"), PathBuf::from("/w")];
        assert_eq!(web_lib(&meta, &dirs), Some(("hello".into(), "hello".into())), "ax create's layout");
        assert_eq!(web_lib(&meta, &[PathBuf::from("/p")]), Some(("my-page".into(), "my_page".into())));
        assert_eq!(web_lib(&meta, &dirs[..1]), None, "a launcher alone has none");
        assert_eq!(web_lib(&meta, &[PathBuf::from("/q")]), None, "a library that never binds");
    }

    #[test]
    fn a_dev_web_build_optimizes_the_libraries_and_links_no_debug_info_a_release_is_a_release() {
        let dev = cargo_args("hello", Some("aio/dev")).join(" ");
        assert!(dev.contains("-p hello --lib --crate-type cdylib --features aio/dev"), "{dev}");
        assert!(dev.contains("--config profile.dev.package.\"*\".opt-level=1"), "{dev}");
        assert!(dev.contains("--config profile.dev.strip=\"debuginfo\""), "{dev}");
        let release = cargo_args("hello", None).join(" ");
        assert!(release.ends_with("--release") && !release.contains("--config"), "{release}");
    }

    #[test]
    fn a_dev_web_build_turns_on_aio_rs_dev_by_the_name_the_package_gives_it() {
        let meta = serde_json::json!({ "packages": [
            { "name": "hello", "dependencies": [
                { "name": "aio-rs", "rename": "aio", "kind": "dev" },
                { "name": "aio-rs", "rename": "core", "kind": null },
            ] },
            { "name": "plain", "dependencies": [{ "name": "aio-rs", "kind": null }] },
        ]});
        assert_eq!(aio_named(&meta, "hello"), "core", "the normal dependency's name, not the tests'");
        assert_eq!(aio_named(&meta, "plain"), "aio-rs");
    }

    #[test]
    fn the_manifest_installs_the_app_in_its_own_colours() {
        let m: serde_json::Value =
            serde_json::from_str(&manifest("Notes \"1\"", 210, &[("icon-192.png".into(), 192)])).unwrap();
        assert_eq!(m["name"], "Notes \"1\"");
        assert_eq!((&m["start_url"], &m["scope"], &m["display"]), (&"./".into(), &"./".into(), &"standalone".into()));
        assert_eq!(
            m["icons"][0],
            serde_json::json!({ "src": "icon-192.png", "sizes": "192x192", "type": "image/png" })
        );
        // aio's colours from the hue: a pale background, the accent as theme.
        let (bg, theme) = colors(210);
        assert_eq!((m["background_color"].as_str(), m["theme_color"].as_str()), (Some(&*bg), Some(&*theme)));
        assert_eq!(
            colors(0),
            ("#fceeee".to_string(), "#e07b7b".to_string()),
            "aio hsl(0, .68, .96) and hsl(0, .62, .68)"
        );
    }

    #[test]
    fn the_page_has_the_apple_tags_its_scripts_as_files_and_no_inline_script_or_eval() {
        let h = head("A <b>", 10, "icon-192.png", "icon-180.png");
        for want in [
            r#"<link rel="manifest" href="./manifest.webmanifest">"#,
            r#"<link rel="apple-touch-icon" href="./icon-180.png">"#,
            r#"<meta name="apple-mobile-web-app-capable" content="yes">"#,
            r#"<meta name="apple-mobile-web-app-title" content="A &lt;b&gt;">"#,
            r#"<meta name="apple-mobile-web-app-status-bar-style" content="default">"#,
        ] {
            assert!(h.contains(want), "{want}: {h}");
        }
        // The worker registers from start.js once the view is drawn (its
        // download takes nothing from the page's), saying why when it can't;
        // a dev build's has none. Both say where the site's files are.
        let start = start_js(false);
        assert!(start.contains("register('./sw.js')") && start.contains("It needs https (or localhost)"));
        assert!(start.contains("addEventListener('aio:mounted', () =>\n  navigator.serviceWorker.register("));
        assert!(start.find("register(").unwrap() < start.find("start();").unwrap());
        assert!(!start_js(true).contains("serviceWorker") && start_js(true).contains("start();"));
        assert!(start.starts_with(BASE) && start_js(true).starts_with(BASE));
        let app = |toml: &str| aio::config::Config::parse(&format!("[app]\nid = \"t\"\n{toml}")).unwrap().app;
        let p = index_html("A <b>", &app("lang = \"cs\""), &h, "<p>first</p>");
        assert!(p.contains(r#"<html lang="cs">"#) && p.contains("<title>A &lt;b&gt;</title>"), "{p}");
        assert!(p.contains(r#"<div id="main"><p>first</p></div>"#) && p.contains(r#"src="./start.js""#));
        // The policy before anything runs; the page scripts after the root they watch, before the app.
        let at = |t: &str| p.find(t).unwrap();
        assert!(at("Content-Security-Policy") < at("<title>"));
        assert!(at("id=\"main\"") < at("./page.js") && at("./page.js") < at("./start.js"));
        assert_eq!(p.matches("<script").count(), p.matches("<script src=").count() + 1, "no inline script: {p}");
        assert!(!aio::page::WEB.contains("'unsafe-eval'") && !aio::page::WEB.contains("script-src 'unsafe-inline'"));
        assert!(
            aio::page::WEB.contains("script-src 'self' blob: 'wasm-unsafe-eval' 'sha256-"),
            "and the sandboxed islands' boot"
        );
        // aio htmlOpen: en unless set; a direction only when asked, never from the language.
        assert!(index_html("t", &app(""), "", "").contains("<html lang=\"en\">\n"));
        let p = index_html("t", &app("lang = \"ar\"\ndir = \"rtl\""), "", "");
        assert!(p.contains(r#"<html lang="ar" dir="rtl">"#), "{p}");
        assert!(index_html("t", &app("dir = \"auto\""), "", "").contains(r#"<html lang="en" dir="auto">"#));
        // The window's page scripts; a dev build's shows its errors on the page, a release's doesn't.
        assert!(page_js(false, true).contains("aria-modal") && page_js(false, true).contains("autofocus"));
        assert!(!page_js(false, true).contains("__aioDiag") && page_js(true, true).contains("window.__aioDiag = add"));
        assert!(!page_js(false, true).contains("/aio-dev/build") && page_js(true, true).contains("/aio-dev/build"));
        // no piece runs into the next: `})()\n(() =>` calls the first's result
        for dev in [false, true] {
            assert!(!page_js(dev, true).contains(")\n("), "{}", page_js(dev, true));
        }
    }

    #[test]
    fn ios_launch_images_draw_the_splash_on_its_background_for_each_screen() {
        // A 40×20 red image, its box 200×100 CSS px.
        let red = image::RgbaImage::from_pixel(40, 20, image::Rgba([255, 0, 0, 255]));
        let png = aio_config::icon::png(red.as_raw(), 40, 20);
        let made = launch_images(&png, (200, 100), "#08f").unwrap();
        assert_eq!(made.len(), IOS_SCREENS.len());
        let (file, link, bytes) = &made[0];
        assert_eq!(file, "apple-splash-640x1136.png");
        assert_eq!(
            link,
            "<link rel=\"apple-touch-startup-image\" media=\"screen and (device-width: 320px) and \
             (device-height: 568px) and (-webkit-device-pixel-ratio: 2) and (orientation: portrait)\" \
             href=\"./apple-splash-640x1136.png\">"
        );
        let img = image::load_from_memory(bytes).unwrap().to_rgb8();
        assert_eq!((img.width(), img.height()), (640, 1136));
        assert_eq!(img.get_pixel(0, 0).0, [0, 0x88, 0xff], "the splash's background");
        assert_eq!(img.get_pixel(320, 568).0, [255, 0, 0], "the image, in the middle");
        // Drawn as the page draws it: 200 CSS px wide at 2x, 400 px.
        assert_eq!(img.get_pixel(320 - 199, 568).0, [255, 0, 0]);
        assert_eq!(img.get_pixel(320 - 201, 568).0, [0, 0x88, 0xff]);
        // No wider than a screen narrower than its box.
        let narrow = launch_images(&png, (500, 250), "#fff").unwrap();
        let img = image::load_from_memory(&narrow[0].2).unwrap().to_rgb8();
        assert_eq!(img.get_pixel(0, 568).0, [255, 0, 0], "edge to edge");
        assert!(launch_images(b"not a png", (1, 1), "#fff").unwrap_err().starts_with("splash:"));
    }

    #[test]
    fn the_dev_pages_console_reaches_the_log_every_level_capped_without_escapes() {
        let js = page_js(true, true);
        assert!(js.contains("fetch('/aio-dev/log'") && js.find("CAP = 200").is_some(), "{js}");
        assert!(js.starts_with("(() => {\n  const MAX = 4096"), "first: it hears the other pieces' lines");
        let release = page_js(false, true);
        assert!(
            !release.contains("/aio-dev/log") && release.contains("fetch('/__aio/log'"),
            "a release page: to its server"
        );
        assert!(!page_js(false, false).contains("/__aio/log"), "a standalone app's page: no server, nothing sent");
        let body = br#"[{"level":"warn","msg":"low disk"},{"level":"log","msg":"a\u001b[2Jb\nc"},{"msg":7}]"#;
        assert_eq!(page_lines(body), ["[page] warn: low disk", "[page] info: a\u{fffd}[2Jb\nc", "[page] info: "]);
        assert!(page_lines(b"not json").is_empty());
        let long = serde_json::json!([{ "level": "error", "msg": "x".repeat(10_000) }]).to_string();
        assert_eq!(page_lines(long.as_bytes())[0].len(), "[page] error: ".len() + 4096);
    }

    /// aio's diagnostic page: a failed build's errors (not its warnings or
    /// progress) on the dev page, open, until a good build reloads it.
    #[test]
    fn a_failed_dev_build_shows_its_errors_on_the_page_until_a_good_one_reloads_it() {
        let said = "   Compiling notes v0.1.0\nwarning: unused import: `x`\n --> src/lib.rs:1:5\n\n\
                    error[E0425]: cannot find value `y` in this scope\n --> src/ui.rs:3:9\n  |\n3 |     y\n  |     ^\n\n\
                    error: could not compile `notes` (lib) due to 1 previous error\n";
        let e = fatal(said);
        assert_eq!(
            e,
            "error[E0425]: cannot find value `y` in this scope\n --> src/ui.rs:3:9\n  |\n3 |     y\n  |     ^"
        );
        assert_eq!(fatal("   Compiling notes\nwarning: x\n"), "", "no error: nothing on the page");
        // Each answer is held by the server until something changed: the
        // page asks again at once, past the version it has.
        let js = format!(
            "const on = {{}}, asked = [], hot = [], answers = [{{ v: 0, n: 1, error: null, hot: null }},
               {{ v: 1, n: 1, error: {e:?}, hot: null }}, {{ v: 2, n: 1, error: {e:?}, hot: null }},
               {{ v: 3, n: 1, error: {e:?}, hot: {{ g: 1, files: ['src/lib.rs'], templates: [] }} }},
               {{ v: 4, n: 1, error: {e:?}, hot: {{ g: 1, files: ['src/lib.rs'], templates: [] }} }},
               {{ v: 5, n: 2, error: null, hot: null }}];
             let reloaded = false;
             globalThis.window = {{ addEventListener: (k, f) => on[k] = f, __aioHot: (j) => hot.push(JSON.parse(j).g) }};
             globalThis.location = {{ reload: () => reloaded = true }};
             globalThis.setTimeout = () => {{}};
             globalThis.fetch = (url) => (asked.push(url), Promise.resolve({{ ok: true, json: () => answers.shift() }}));
             const el = (tag) => ({{ tag, kids: [], style: {{}}, attrs: {{}}, textContent: '',
               setAttribute(k, v) {{ this.attrs[k] = v; }},
               attachShadow() {{ return this; }},
               appendChild(c) {{ this.kids.push(c); }} }});
             globalThis.document = {{ body: el('body'), createElement: el }};
             {}\n{RELOAD}
             setImmediate(() => setImmediate(() => setImmediate(() => {{
               const [list, badge] = document.body.kids[0].kids;
               const shown = [list.kids.length, list.kids[0].textContent, list.style.display, badge.textContent];
               console.log(JSON.stringify([...shown, hot, reloaded, asked]));
             }})));",
            aio::page::OVERLAY
        );
        let Ok(out) = std::process::Command::new("node").args(["-e", &js]).output() else {
            return eprintln!("skipped: no node");
        };
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let got: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        // The error once (told twice), open; the markup once (told twice); a good build reloads.
        let asked: Vec<String> = ["".into()].into_iter().chain((0..5).map(|v| format!("?after={v}"))).collect();
        let asked: Vec<String> = asked.iter().map(|q| format!("/aio-dev/build{q}")).collect();
        assert_eq!(got, serde_json::json!([2, format!("build failed — {e}"), "block", "1 problem", [1], true, asked]));
    }

    #[test]
    fn a_dev_page_is_told_each_change_as_it_comes_its_markup_until_a_build() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        let live = std::sync::Arc::new(Live::default());
        let wait = std::time::Duration::from_secs(5);
        rt.block_on(async {
            let first = live.after(None, wait).await;
            assert_eq!(first, serde_json::json!({ "v": 0, "n": 1, "error": null, "hot": null }), "at once");
            // Held until a change: then answered with it.
            let l = live.clone();
            let held = tokio::spawn(async move { l.after(Some(0), wait).await });
            tokio::task::yield_now().await;
            live.hot(serde_json::json!({ "files": ["src/lib.rs"], "templates": [] }));
            let got = held.await.unwrap();
            assert_eq!(
                (&got["v"], &got["hot"]["g"], &got["hot"]["files"][0]),
                (&1.into(), &1.into(), &"src/lib.rs".into())
            );
            live.hot(serde_json::json!({ "files": ["src/lib.rs"], "templates": [] }));
            assert_eq!(live.after(Some(1), wait).await["hot"]["g"], 2, "each edit a new generation");
            live.failed("error: x".into());
            assert!(live.built(), "red to green");
            let now = live.after(Some(3), wait).await;
            assert_eq!(
                (&now["n"], &now["error"], &now["hot"]),
                (&2.into(), &serde_json::Value::Null, &serde_json::Value::Null)
            );
            // Nothing new: the same state once the wait is up.
            let quiet = live.after(Some(4), std::time::Duration::from_millis(20)).await;
            assert_eq!(quiet["v"], 4);
        });
    }

    #[test]
    fn only_a_stylesheet_edit_needs_no_new_bundle() {
        let p = |s: &str| PathBuf::from(s);
        assert!(!rebuilds(&[p("/a/assets/style.css"), p("/a/assets/kit.css")]));
        assert!(rebuilds(&[p("/a/assets/style.css"), p("/a/src/ui.rs")]));
        assert!(rebuilds(&[p("/a/Cargo.toml")]) && rebuilds(&[p("/a/assets/logo")]));
    }

    #[test]
    fn an_icon_too_small_or_not_square_is_said_as_not_installable() {
        // aio webIconProblem.
        assert!(icon_problem(64, 64).unwrap().contains("at least 144×144"));
        assert!(icon_problem(512, 256).unwrap().contains("square"));
        assert_eq!(icon_problem(512, 512), None);
        assert_eq!(icon_problem(144, 144), None);
    }

    #[test]
    fn the_worker_is_named_by_the_build_and_pins_only_code() {
        let files = vec![("app.js".to_string(), b"js".to_vec()), ("icon 1#.png".into(), b"png".to_vec())];
        let sw = service_worker("notes", &files, false);
        assert!(sw.contains(r#"const FILES = ["./","./app.js","./icon 1%23.png"];"#), "{sw}");
        assert!(sw.contains(r#""./app.js":"sha256-"#) && !sw.contains(r#""./icon 1%23.png":"sha256"#), "{sw}");
        assert!(sw.contains(&format!("aio-notes-{}@", content_hash(&files))));
        // Another build is another worker; the same build the same.
        let other = vec![("app.js".to_string(), b"js2".to_vec())];
        assert_ne!(content_hash(&files), content_hash(&other));
        assert_eq!(sw, service_worker("notes", &files, false));
        // The app's own server is never cached.
        assert!(sw.contains("u.pathname === '/ws' || u.pathname.startsWith('/__aio/')"));
        // A static host's page from the cache at once; a server's asked first (it paints it fresh).
        assert!(
            sw.contains("const SERVED = false;")
                && service_worker("notes", &files, true).contains("const SERVED = true;")
        );
    }
}

/// The web build in a real browser (a headless Chromium, the wasm32 target
/// and the lock's wasm-bindgen; skipped without them): its page runs a
/// synced copy of its server's cell, offline too, twin tabs mirrored.
#[cfg(all(test, unix))]
pub(crate) mod browser {
    pub(crate) use super::skipped;
    use super::*;
    use aio::prelude::*;
    use aio::serde_json::{Value, json};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    include!("web/board.rs");

    const KEY: &str = "a-long-random-app-key-for-tests";

    /// The test page's site, built and bound into `dir/site`; `None` (said)
    /// without the wasm32 target or the right wasm-bindgen.
    fn site(dir: &Path) -> Option<PathBuf> {
        let sysroot = Command::new("rustc").args(["--print", "sysroot"]).output().ok()?;
        let sysroot = PathBuf::from(String::from_utf8_lossy(&sysroot.stdout).trim());
        if !sysroot.join("lib/rustlib/wasm32-unknown-unknown").is_dir() {
            skipped("no wasm32 target");
            return None;
        }
        let page = dir.join("page");
        std::fs::create_dir_all(page.join("src")).unwrap();
        // aio-rs as this checkout has it, at the versions its lock has.
        let aio = Path::new(env!("CARGO_MANIFEST_DIR")).join("../aio");
        let manifest = format!(
            "[package]\nname = \"webpage\"\nversion = \"0.1.0\"\nedition = \"2024\"\npublish = false\n\n\
             [lib]\ncrate-type = [\"cdylib\"]\n\n[dependencies]\n\
             aio = {{ package = \"aio-rs\", path = {:?}, default-features = false, features = [\"web\", \"kit\"] }}\n\
             wasm-bindgen = \"0.2\"\nwasm-bindgen-futures = \"0.4\"\njs-sys = \"0.3\"\n\
             web-sys = {{ version = \"0.3\", features = [\"Location\", \"Window\"] }}\n\n[workspace]\n",
            aio.display().to_string()
        );
        for (f, text) in [
            ("Cargo.toml", manifest.as_str()),
            ("Cargo.lock", include_str!("../../../Cargo.lock")),
            ("rust-toolchain.toml", include_str!("../../../rust-toolchain.toml")),
            ("src/lib.rs", include_str!("web/page.rs")),
            ("src/board.rs", include_str!("web/board.rs")),
            ("src/tally.rs", include_str!("web/tally.rs")),
            ("src/view.rs", include_str!("web/view.rs")),
        ] {
            std::fs::write(page.join(f), text).unwrap();
        }
        let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| page.join("target"), PathBuf::from);
        let app = App { id: "web-test".into(), name: "Web test".into(), version: "1.0.0".into(), icon: None };
        let out = dir.join("site");
        match bind(&page, &target, ("webpage", "webpage", "aio"), &app, &out, false, false, None) {
            Err(e) if e.contains("needs wasm-bindgen") => {
                skipped(&e);
                return None;
            }
            r => _ = r.unwrap(),
        }
        let cfg = aio::config::Config::parse(
            "[app]\nid = \"web-test\"\nname = \"Web test\"\nlang = \"cs\"\ndir = \"auto\"\n\
             [build]\nassets = { \"/media\" = \"media\" }\n\
             [build.splash]\nimage = \"splash.png\"\ntext = \"Starting {title}\"\nmin_ms = 300",
        )
        .unwrap();
        std::fs::write(page.join("splash.png"), include_bytes!("../../aio/tests/fixtures/splash.png")).unwrap();
        std::fs::create_dir_all(page.join("media")).unwrap();
        std::fs::write(page.join("media/hi there.txt"), "hi").unwrap();
        assert_eq!(pack(&app, &cfg, &page, &out, "", false).unwrap(), Vec::<String>::new());
        let index = std::fs::read_to_string(out.join("index.html")).unwrap();
        assert!(index.contains("<body>\n<style>#aio-splash{") && index.contains("<p>Starting Web test</p>"));
        assert!(out.join("splash.png").is_file());
        // `[build] assets` at their paths, held offline by the worker; iOS's launch images beside.
        assert_eq!(std::fs::read_to_string(out.join("media/hi there.txt")).unwrap(), "hi");
        let sw = std::fs::read_to_string(out.join("sw.js")).unwrap();
        assert!(sw.contains("./media/hi there.txt") && !sw.contains("apple-splash-"), "{sw}");
        assert!(index.contains("apple-touch-startup-image") && out.join("apple-splash-1290x2796.png").is_file());
        Some(out)
    }

    /// The network between the browser and the server: up while its task
    /// runs; `sockets` counts the sync sockets (`GET /ws`) it carries.
    fn link(l: tokio::net::TcpListener, to: String, sockets: Arc<AtomicUsize>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut links = tokio::task::JoinSet::new();
            while let Ok((mut a, _)) = l.accept().await {
                let (to, sockets) = (to.clone(), sockets.clone());
                links.spawn(async move {
                    let mut head = [0u8; 8];
                    while matches!(a.peek(&mut head).await, Ok(n) if n > 0 && n < head.len()) {
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                    if &head == b"GET /ws?" {
                        sockets.fetch_add(1, Ordering::SeqCst);
                    }
                    let mut b = tokio::net::TcpStream::connect(to).await.unwrap();
                    _ = tokio::io::copy_bidirectional(&mut a, &mut b).await;
                });
            }
        })
    }

    /// A headless Chromium on `url` (`aio::testing::TestBrowser`: a profile
    /// of its own, off the person's display and bus), ended when dropped.
    pub(crate) struct Chromium(aio::testing::TestBrowser);

    impl Chromium {
        pub(crate) fn open(url: &str) -> Option<Self> {
            aio::testing::TestBrowser::open(url).map(Self).map_err(|e| eprintln!("{e}")).ok()
        }

        /// The sessions of its tabs on `url` once there are `n`.
        pub(crate) async fn tabs(&self, url: &str, n: usize) -> Vec<crate::cdp::Session> {
            for _ in 0..200 {
                let ts = crate::cdp::targets(self.0.port()).unwrap_or_default();
                let pages: Vec<&str> = ts
                    .iter()
                    .filter(|t| t["type"] == "page" && t["url"].as_str().is_some_and(|u| u.starts_with(url)))
                    .filter_map(|t| t["webSocketDebuggerUrl"].as_str())
                    .collect();
                if pages.len() == n {
                    let mut out = Vec::new();
                    for p in pages {
                        out.push(crate::cdp::Session::connect(p).await.unwrap());
                    }
                    return out;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            panic!("Chromium shows no {n} tabs on {url}");
        }
    }

    /// Waits until `expr` in the tab is `want` (20 s at most).
    pub(crate) async fn until(t: &mut crate::cdp::Session, expr: &str, want: Value) {
        let mut last = Value::Null;
        for _ in 0..400 {
            last = t.eval(expr).await.unwrap_or(Value::Null);
            if last == want {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("{expr}: {last} (wanted {want})");
    }

    /// Waits until the server's board has `want` (5 s at most).
    async fn served(h: &Handle<Board>, want: &[&str]) {
        for _ in 0..200 {
            if h.state().items == want {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert_eq!(h.state().items, want);
    }

    /// Collects what the page's policy refuses into `window.refused`.
    const REFUSED: &str = "window.refused = []; document.addEventListener('securitypolicyviolation', \
                           (e) => window.refused.push(e.violatedDirective + ' ' + e.blockedURI))";

    /// The browser's notifications, granted, kept in `window.notices`
    /// (a headless browser shows none).
    const NOTICES: &str = "window.notices = []; window.Notification = class { static permission = 'granted'; \
                           static requestPermission() { return Promise.resolve('granted') } \
                           constructor(t, o) { notices.push([t, o.body, o.tag, o.icon, o.silent]) } }";

    /// What tab `t` shows: its items, calls waiting, link.
    const SHOWN: &str = "import('./app.js').then((m) => JSON.parse(m.shown()))";
    const APP: &str = "import('./app.js')";
    /// The page's own cell.
    const TALLY: &str = "import('./app.js').then((m) => JSON.parse(m.tally()))";

    #[tokio::test(flavor = "multi_thread")]
    async fn the_page_runs_its_copy_offline_keeps_its_calls_and_mirrors_its_twin() {
        let dir = tempfile::tempdir().unwrap();
        let Some(site) = tokio::task::spawn_blocking({
            let d = dir.path().to_path_buf();
            move || site(&d)
        })
        .await
        .unwrap() else {
            return;
        };
        let (h, votes) = (Handle::<Board>::memory(), Handle::<Votes>::memory());
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server = l.local_addr().unwrap().to_string();
        let http = aio::http::Http::default().shell(&site).cell(votes.clone());
        let opts = aio::sync::Options { token: KEY.into(), http, ..Default::default() };
        tokio::spawn(aio::sync::serve_with(h.clone(), l, opts));
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let sockets = Arc::new(AtomicUsize::new(0));
        let up = link(l, server.clone(), sockets.clone());
        let url = format!("http://{addr}/?token={KEY}");
        let Some(c) = Chromium::open(&url) else {
            return skipped("no Chromium");
        };
        let mut a = c.tabs(&url, 1).await.remove(0);
        // Its twin: another tab of the app in the same browser.
        a.call("Target.createTarget", json!({ "url": url })).await.unwrap();
        let mut tabs = c.tabs(&url, 2).await;
        let (mut a, mut b) = (tabs.remove(0), tabs.remove(0));
        // What the page's policy refuses, counted from now on (it reloads).
        a.call("Page.enable", json!({})).await.unwrap();
        a.call("Page.addScriptToEvaluateOnNewDocument", json!({ "source": REFUSED })).await.unwrap();
        a.call("Page.addScriptToEvaluateOnNewDocument", json!({ "source": NOTICES })).await.unwrap();
        a.eval("location.reload()").await.ok();
        for t in [&mut a, &mut b] {
            until(t, "window.ready", json!("yes")).await;
            // The splash ([build.splash]) went once the view was drawn.
            until(t, "!document.getElementById('aio-splash')", json!(true)).await;
            // Installed: the worker serves the page from now on.
            until(t, "!!navigator.serviceWorker.controller", json!(true)).await;
        }
        // Both synced cells on the page's one socket: a reload opens one.
        const VOTES: &str = "import('./app.js').then((m) => JSON.parse(m.votes()))";
        until(&mut b, VOTES, json!({ "n": 0, "online": true })).await;
        let before = sockets.load(Ordering::SeqCst);
        b.eval("window.stale = true; location.reload()").await.ok();
        until(&mut b, "!window.stale && window.ready", json!("yes")).await;
        until(&mut b, VOTES, json!({ "n": 0, "online": true })).await;
        until(&mut b, &format!("{SHOWN}.then((s) => s.online)"), json!(true)).await;
        assert_eq!(sockets.load(Ordering::SeqCst) - before, 1, "one socket for the board and the votes");
        assert_eq!(b.eval(&format!("{APP}.then((m) => m.vote())")).await.unwrap(), json!(true));
        until(&mut a, VOTES, json!({ "n": 1, "online": true })).await;
        for _ in 0..200 {
            if votes.state().n == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert_eq!(votes.state().n, 1);
        // The page's language and direction, as aio.toml says them.
        let html = "[document.documentElement.lang, document.documentElement.dir]";
        assert_eq!(a.eval(html).await.unwrap(), json!(["cs", "auto"]));
        // Online: a call reaches the server, and both tabs show it.
        assert_eq!(a.eval("import('./app.js').then((m) => m.add('a'))").await.unwrap(), json!(true));
        served(&h, &["a"]).await;
        for t in [&mut a, &mut b] {
            until(t, SHOWN, json!({ "items": ["a"], "pending": 0, "online": true })).await;
        }
        // The network goes: both tabs are offline.
        up.abort();
        _ = up.await;
        for t in [&mut a, &mut b] {
            until(t, &format!("{SHOWN}.then((s) => s.online)"), json!(false)).await;
        }
        // A call made offline shows at once, waits in the browser's storage,
        // and the twin tab shows it too.
        assert_eq!(a.eval("import('./app.js').then((m) => m.add('b'))").await.unwrap(), json!(true));
        until(&mut a, SHOWN, json!({ "items": ["a", "b"], "pending": 1, "online": false })).await;
        let stored = "JSON.parse(localStorage.getItem('aio.web-test:board')).pending.map((c) => c.method)";
        until(&mut a, stored, json!(["add"])).await;
        until(&mut b, SHOWN, json!({ "items": ["a", "b"], "pending": 1, "online": false })).await;
        // The page's own cell: atomic calls, kept in the browser.
        for _ in 0..2 {
            assert_eq!(a.eval(&format!("{APP}.then((m) => m.bump())")).await.unwrap(), json!(true));
        }
        until(&mut a, TALLY, json!({ "n": 2, "busy": true, "running": 0 })).await;
        // A reload while offline: the worker's page, the saved copy and its
        // call, the page's cell as it was kept (its transient field fresh).
        a.eval("location.reload()").await.ok();
        until(&mut a, "window.ready", json!("yes")).await;
        until(&mut a, SHOWN, json!({ "items": ["a", "b"], "pending": 1, "online": false })).await;
        until(&mut a, TALLY, json!({ "n": 2, "busy": false, "running": 0 })).await;
        let bump = format!("{APP}.then((m) => m.bump())");
        assert_eq!(a.eval(&bump).await.unwrap(), json!(true));
        assert_eq!(a.eval(&bump).await.unwrap(), json!(false), "refused: it changed nothing");
        until(&mut a, TALLY, json!({ "n": 3, "busy": true, "running": 0 })).await;
        let kept = "JSON.parse(localStorage.getItem('aio.web-test:tally')).view";
        until(&mut a, kept, json!({ "n": 3 })).await;
        // A built-in file at its URL path.
        let hi = "fetch('./media/hi%20there.txt').then((r) => r.text())";
        assert_eq!(a.eval(hi).await.unwrap(), json!("hi"));
        // An async method: its effect runs on the page's timer, counted while it does.
        assert_eq!(a.eval(&format!("{APP}.then((m) => m.later())")).await.unwrap(), json!(true));
        assert_eq!(a.eval(TALLY).await.unwrap()["running"], json!(1));
        until(&mut a, TALLY, json!({ "n": 13, "busy": true, "running": 0 })).await;
        // A tell of the page's cell: the page's own notification, the app's icon on it.
        assert_eq!(a.eval(&format!("{APP}.then((m) => m.finish())")).await.unwrap(), json!(true));
        until(&mut a, "window.notices", json!([["Done", "the tally", "tally", "./icon-192.png", true]])).await;
        // The network is back on its port: the call reaches the server once,
        // sent by both tabs (the server applies a call id once).
        let l = tokio::net::TcpListener::bind(&addr).await.unwrap();
        let _up = link(l, server, sockets.clone());
        for t in [&mut a, &mut b] {
            until(t, SHOWN, json!({ "items": ["a", "b"], "pending": 0, "online": true })).await;
        }
        served(&h, &["a", "b"]).await;
        until(&mut a, stored, json!([])).await;
        // The view in the page: the real DOM, its clicks the cell's calls.
        let items = "[...document.querySelectorAll('#items li')].map((l) => l.textContent)";
        until(&mut a, items, json!(["a", "b"])).await;
        until(&mut a, "document.getElementById('link').textContent", json!("online")).await;
        b.eval("document.getElementById('add').click()").await.unwrap();
        until(&mut a, items, json!(["a", "b", "ui"])).await;
        served(&h, &["a", "b", "ui"]).await;
        h.call("add", json!(["server"])).unwrap();
        until(&mut b, items, json!(["a", "b", "ui", "server"])).await;
        // One main, nothing left unseen.
        let one = "document.querySelectorAll('#main').length === 1 && !document.querySelector('[hidden]')";
        until(&mut a, one, json!(true)).await;
        // The window's page scripts and kit in the page: the field that
        // asks for focus has it (it came after the page loaded), and an
        // island ran its script and sent its event back, all under the
        // page's policy (no eval), which refused nothing.
        until(&mut a, "document.activeElement && document.activeElement.id", json!("auto")).await;
        until(&mut a, "document.getElementById('island').textContent", json!("island 4")).await;
        until(&mut a, "document.getElementById('doubled').textContent", json!("8")).await;
        assert_eq!(a.eval("window.refused").await.unwrap(), json!([]));
        // The page as the server serves it: the web build's, its view drawn
        // by the page itself (no server paint on air).
        let page = {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut s = tokio::net::TcpStream::connect(&addr).await.unwrap();
            let ask = format!("GET /?token={KEY} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
            s.write_all(ask.as_bytes()).await.unwrap();
            let mut got = Vec::new();
            s.read_to_end(&mut got).await.unwrap();
            String::from_utf8_lossy(&got).into_owned()
        };
        assert!(page.contains("cache-control: no-store"), "{page}");
        assert!(page.contains(r#"<div id="main"></div>"#) && page.contains("./start.js"), "{page}");
        // Its head as the view asks for it, written into the page by the view.
        until(&mut a, "document.title", json!("4 items")).await;
        let tags = "document.querySelectorAll('meta[name=description][data-aio-head]').length";
        assert_eq!(a.eval(tags).await.unwrap(), json!(1));
        // What the worker doesn't hold goes to the server as asked.
        let asked = "fetch('./missing.txt').then((r) => r.status, (e) => 'failed: ' + e)";
        assert_eq!(b.eval(asked).await.unwrap(), json!(404));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_dev_pages_console_and_its_uncaught_errors_reach_the_dev_log() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("index.html"),
            "<!doctype html><html><body><div id=\"main\"></div><script src=\"./page.js\"></script></body></html>",
        )
        .unwrap();
        std::fs::write(dir.path().join("page.js"), page_js(true, true)).unwrap();
        let said = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let heard = said.clone();
        let http =
            aio::http::Http::default().shell(dir.path()).route(aio::http::Route::post("/aio-dev/log", move |r| {
                heard.lock().unwrap().extend(page_lines(r.body()));
                async { aio::http::Reply::text("").status(204) }
            }));
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", l.local_addr().unwrap());
        let opts = aio::sync::Options { open: true, http, ..Default::default() };
        tokio::spawn(aio::sync::serve_with(Handle::<DevSite>::memory(), l, opts));
        let Some(c) = Chromium::open(&url) else {
            return skipped("no Chromium");
        };
        let mut t = c.tabs(&url, 1).await.remove(0);
        until(&mut t, "location.protocol + document.readyState", json!("http:complete")).await;
        let did = "console.log('hello', { n: 1 }); console.debug('deep'); console.warn('careful'); \
                   Promise.reject(new Error('no answer')); setTimeout(() => { throw new Error('boom') }); true";
        assert_eq!(t.eval(did).await.unwrap(), json!(true));
        let want = |l: &str| said.lock().unwrap().iter().any(|s| s.starts_with(l));
        for _ in 0..200 {
            if want("[page] error: [uncaught] Error: boom")
                && want("[page] error: [unhandled rejection] Error: no answer")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let got = said.lock().unwrap().clone();
        for l in ["[page] info: hello {\"n\":1}", "[page] debug: deep", "[page] warn: careful"] {
            assert!(got.iter().any(|s| s == l), "{l} in {got:?}");
        }
        assert!(want("[page] error: [uncaught] Error: boom"), "{got:?}");
        assert!(want("[page] error: [unhandled rejection] Error: no answer"), "{got:?}");
    }

    /// The board's view, natively: what the server paints the page with.
    mod shown {
        use super::{Board, BoardCalls};
        include!("web/view.rs");

        pub(crate) fn app() -> View {
            view()
        }
    }

    /// Notes, from the page's first script, when `#items` first shows a row
    /// (`window.shownAt`, ms) and what the page held when air's interpreter
    /// took its first frame (`window.painted`: `#main`'s elements, the rows).
    const WATCH: &str = "new MutationObserver((_, o) => { if (document.querySelector('#items li')) \
                         { window.shownAt = performance.now(); o.disconnect() } }).observe(document, { childList: true, subtree: true }); \
                         Object.defineProperty(window, '__airApply', { configurable: true, set(f) { \
                         Object.defineProperty(window, '__airApply', { configurable: true, writable: true, value: (fr) => { \
                         window.painted ||= { nodes: [...document.querySelectorAll('#main *')], \
                         rows: [...document.querySelectorAll('#items li')].map((l) => l.textContent) }; return f(fr) } }) } })";

    /// A board of `a` and `b` served with the site, painted on the server or
    /// not; its address and the board.
    async fn board_site(site: &Path, painted: bool) -> (String, Handle<Board>) {
        let h = Handle::<Board>::memory();
        for i in ["a", "b"] {
            h.call("add", json!([i])).unwrap();
        }
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let http = aio::http::Http::default().shell(site);
        let http = if painted { http.painted(shown::app) } else { http };
        let opts = aio::sync::Options { token: KEY.into(), http, ..Default::default() };
        tokio::spawn(aio::sync::serve_with(h.clone(), l, opts));
        (addr, h)
    }

    /// Tab `t` (its first script [`WATCH`]ing) gone to `url`: when its rows showed.
    async fn shown_at(t: &mut crate::cdp::Session, url: &str) -> f64 {
        t.call("Page.navigate", json!({ "url": url })).await.unwrap();
        until(t, "window.ready", json!("yes")).await;
        until(t, "typeof window.shownAt", json!("number")).await;
        t.eval("window.shownAt").await.unwrap().as_f64().unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_served_page_is_painted_on_the_server_then_hydrated_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let Some(site) = tokio::task::spawn_blocking({
            let d = dir.path().to_path_buf();
            move || site(&d)
        })
        .await
        .unwrap() else {
            return;
        };
        let (addr, h) = board_site(&site, true).await;
        let Some(c) = Chromium::open("about:blank") else {
            return skipped("no Chromium");
        };
        let mut t = c.tabs("about:blank", 1).await.remove(0);
        t.call("Page.enable", json!({})).await.unwrap();
        t.call("Page.addScriptToEvaluateOnNewDocument", json!({ "source": WATCH })).await.unwrap();
        // Each navigation reaches the server (a standalone site's worker would
        // answer it with the page as built); the files come from the HTTP cache.
        t.call("Network.enable", json!({})).await.unwrap();
        t.call("Network.setBypassServiceWorker", json!({ "bypass": true })).await.unwrap();
        let painted_at = shown_at(&mut t, &format!("http://{addr}/?token={KEY}")).await;
        // The rows were in the page before the view ran, and the view took
        // its nodes over: none replaced, every element the server's.
        assert_eq!(t.eval("window.painted.rows").await.unwrap(), json!(["a", "b"]));
        until(&mut t, "!!window.__airHydrated", json!(true)).await;
        assert_eq!(t.eval("window.__airHydrated.replaced").await.unwrap(), json!(0));
        let kept = "window.painted.nodes.every((n) => n.isConnected) && \
                    window.painted.nodes.length === document.querySelectorAll('#main *').length";
        assert_eq!(t.eval(kept).await.unwrap(), json!(true));
        // The head the view asked for, from the server, written once.
        assert_eq!(t.eval("document.title").await.unwrap(), json!("2 items"));
        let tags = "document.querySelectorAll('meta[name=description][data-aio-head]').length";
        assert_eq!(t.eval(tags).await.unwrap(), json!(1));
        // Wired: a click on the server's button is the cell's call, its row
        // drawn in the server's list; the copy links and stays in sync.
        until(&mut t, "document.getElementById('link').textContent", json!("online")).await;
        t.eval("document.getElementById('add').click()").await.unwrap();
        served(&h, &["a", "b", "ui"]).await;
        let rows = "[...document.querySelectorAll('#items li')].map((l) => l.textContent)";
        until(&mut t, rows, json!(["a", "b", "ui"])).await;
        assert_eq!(t.eval("window.painted.nodes[0] === document.getElementById('items')").await.unwrap(), json!(true));
        h.call("add", json!(["server"])).unwrap();
        until(&mut t, rows, json!(["a", "b", "ui", "server"])).await;
        // The same site unpainted: its rows show only once the module ran and
        // linked. Each warmed once, then taken in turn; the medians compared.
        let (other, _) = board_site(&site, false).await;
        let urls = [format!("http://{addr}/?token={KEY}"), format!("http://{other}/?token={KEY}")];
        shown_at(&mut t, &urls[1]).await;
        assert_eq!(t.eval("window.painted.rows").await.unwrap(), json!([]));
        let mut at = [vec![painted_at], vec![]];
        for _ in 0..5 {
            for (i, url) in urls.iter().enumerate() {
                at[i].push(shown_at(&mut t, url).await);
                let painted = t.eval("window.painted.rows.length > 0").await.unwrap();
                assert_eq!(painted, json!(i == 0), "{url}");
            }
        }
        let median = |v: &mut Vec<f64>| {
            v.sort_by(f64::total_cmp);
            v[v.len() / 2]
        };
        let (painted, drawn) = (median(&mut at[0]), median(&mut at[1]));
        eprintln!("rows shown, median ms from navigation: {painted:.1} painted, {drawn:.1} drawn by the page ({at:?})");
        assert!(painted < drawn, "painted {painted} ms, drawn by the page {drawn} ms");
    }

    /// Notes, from the page's first script, when `#slow` first shows and
    /// when it shows its count (`window.slow`: ms, and the texts it showed).
    const SLOW: &str = "window.slow = { texts: [] }; new MutationObserver(() => { const s = document.getElementById('slow'); \
                        const t = s && s.textContent; if (t && t !== window.slow.texts.at(-1)) { window.slow.texts.push(t); \
                        window.slow[t] = performance.now() } }).observe(document, { childList: true, subtree: true, characterData: true })";

    #[tokio::test(flavor = "multi_thread")]
    async fn a_streamed_page_shows_at_once_and_fills_in_what_waited_before_its_view_hydrates() {
        let dir = tempfile::tempdir().unwrap();
        let Some(site) = tokio::task::spawn_blocking({
            let d = dir.path().to_path_buf();
            move || site(&d)
        })
        .await
        .unwrap() else {
            return;
        };
        let (addr, _h) = board_site(&site, true).await;
        let Some(c) = Chromium::open("about:blank") else {
            return skipped("no Chromium");
        };
        let mut t = c.tabs("about:blank", 1).await.remove(0);
        t.call("Page.enable", json!({})).await.unwrap();
        t.call("Page.addScriptToEvaluateOnNewDocument", json!({ "source": SLOW })).await.unwrap();
        t.call("Network.enable", json!({})).await.unwrap();
        t.call("Network.setBypassServiceWorker", json!({ "bypass": true })).await.unwrap();
        let url = format!("http://{addr}/?token={KEY}");
        let timing = "(() => { const n = performance.getEntriesByType('navigation')[0], \
                      p = performance.getEntriesByName('first-contentful-paint')[0]; \
                      return [n.responseStart, n.responseEnd, p ? p.startTime : -1, window.slow.counting ?? -1, \
                      window.slow['7 counted'] ?? -1] })()";
        let mut runs = vec![];
        for _ in 0..5 {
            t.call("Page.navigate", json!({ "url": url })).await.unwrap();
            until(&mut t, "window.ready", json!("yes")).await;
            until(&mut t, "!!window.__airHydrated", json!(true)).await;
            // The fallback came with the page, the count in place of it
            // before the view ran, which took both over as they were.
            assert_eq!(t.eval("window.slow.texts").await.unwrap(), json!(["counting", "7 counted"]));
            assert_eq!(t.eval("window.__airHydrated.replaced").await.unwrap(), json!(0));
            let at: Vec<f64> = aio::serde_json::from_value(t.eval(timing).await.unwrap()).unwrap();
            runs.push(at);
        }
        // The page took the server's value: its own load (8) never ran.
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        assert_eq!(t.eval("document.getElementById('slow').textContent").await.unwrap(), json!("7 counted"));
        let median = |i: usize| {
            let mut v: Vec<f64> = runs.iter().map(|r| r[i]).collect();
            v.sort_by(f64::total_cmp);
            v[v.len() / 2]
        };
        let (first_byte, last_byte, first_paint, fallback, filled) =
            (median(0), median(1), median(2), median(3), median(4));
        eprintln!(
            "streamed, median ms from navigation: first byte {first_byte:.1}, first paint {first_paint:.1}, \
             fallback {fallback:.1}, filled in {filled:.1}, last byte {last_byte:.1} ({runs:?})"
        );
        // A page rendered whole would send its first byte after its last.
        assert!(first_byte + 100.0 < last_byte, "first byte {first_byte} ms, last {last_byte} ms");
        assert!(fallback < filled && filled <= last_byte + 50.0, "fallback {fallback} ms, filled {filled} ms");
    }

    /// Types `text` into the field `t` names, as a person does.
    fn typed(t: &str, text: &str) -> String {
        format!(
            "(() => {{ const i = document.querySelector('[data-t={t}]'); i.value = {}; \
             i.dispatchEvent(new Event('input', {{ bubbles: true }})); return true; }})()",
            Value::from(text)
        )
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_page_signs_in_with_the_kits_form_its_session_only_in_an_httponly_cookie() {
        let dir = tempfile::tempdir().unwrap();
        let Some(site) = tokio::task::spawn_blocking({
            let d = dir.path().to_path_buf();
            move || site(&d)
        })
        .await
        .unwrap() else {
            return;
        };
        const PW: &str = "correct horse battery";
        let auth = std::sync::Arc::new(aio::auth::Auth::memory(aio::auth::Policy::default()).unwrap());
        auth.create("ann", PW, None).unwrap();
        let h = Handle::<Board>::memory();
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let http = aio::http::Http::default().shell(&site);
        let opts = aio::sync::Options { token: KEY.into(), auth: Some(auth.clone()), http, ..Default::default() };
        tokio::spawn(aio::sync::serve_with(h.clone(), l, opts));
        let url = format!("http://{addr}/signin");
        let Some(c) = Chromium::open(&url) else {
            return skipped("no Chromium");
        };
        let mut t = c.tabs(&url, 1).await.remove(0);
        // Signed out: the page loads (it is the sign-in), its copy can't
        // link, and aio useUser says nobody.
        until(&mut t, "window.ready", json!("unlinked")).await;
        let who = "document.getElementById('who') && document.getElementById('who').textContent";
        until(&mut t, who, json!("nobody")).await;
        // A wrong password is said on the form.
        let submit = "document.querySelector('[data-t=signinSubmit]').click() || true";
        for (f, v) in [("signinId", "ann"), ("signinPassword", "wrong password")] {
            assert_eq!(t.eval(&typed(f, v)).await.unwrap(), json!(true));
        }
        t.eval(submit).await.unwrap();
        let error = "document.querySelector('[data-t=signinError]') && \
                     document.querySelector('[data-t=signinError]').textContent";
        until(&mut t, error, json!("Incorrect username or password.")).await;
        // Signed in through the kit's form: the page starts again as ann,
        // its copy linked by the cookie alone.
        assert_eq!(t.eval(&typed("signinPassword", PW)).await.unwrap(), json!(true));
        t.eval(submit).await.unwrap();
        until(&mut t, "window.ready", json!("linked")).await;
        until(&mut t, who, json!("ann user")).await;
        assert_eq!(t.eval("import('./app.js').then((m) => m.add('as ann'))").await.unwrap(), json!(true));
        served(&h, &["as ann"]).await;
        // The session is nowhere the page's script reads: not its cookies
        // (HttpOnly), its storage or its address.
        assert_eq!(auth.sessions().unwrap(), 1, "one session: the form's");
        let seen = "JSON.stringify([location.search, ...document.cookie.split('; ').map((c) => c.split('=')[1] || ''), \
                    ...Object.values(localStorage), ...Object.values(sessionStorage)])";
        let seen: Vec<String> = aio::serde_json::from_str(t.eval(seen).await.unwrap().as_str().unwrap()).unwrap();
        assert_eq!(seen[0], "", "no ?token= in its address");
        for s in &seen {
            assert!(!s.contains("aio_session") && auth.session(s).unwrap().is_none(), "{s}");
        }
        // A door asked the page's way answers no token.
        let login = format!(
            "fetch('/__aio/auth/login', {{ method: 'POST', headers: {{ 'content-type': 'application/json', \
             'x-aio-session': 'cookie' }}, body: JSON.stringify({{ id: 'ann', password: {} }}) }}).then((r) => r.json())",
            Value::from(PW)
        );
        let said = t.eval(&login).await.unwrap();
        assert!(said["user"]["id"] == "ann" && said.get("token").is_none(), "{said}");
        // aio signOut: the session ends, and the page starts again signed out.
        t.eval("document.getElementById('out').click() || true").await.unwrap();
        until(&mut t, "window.ready", json!("unlinked")).await;
        until(&mut t, who, json!("nobody")).await;
    }
}

/// The web build on air in a real browser (a headless Chromium, the wasm32
/// target and the lock's wasm-bindgen; skipped without them): the page's
/// view drawn by air's interpreter talking to the module, air alone in it.
#[cfg(all(test, unix))]
mod browser_air {
    use super::browser::{Chromium, skipped, until};
    use super::*;
    use aio::serde_json::json;
    use std::time::Duration;

    /// The test page's site on air, built and bound into `dir/site`; `None`
    /// (said) without the wasm32 target or the right wasm-bindgen.
    fn site(dir: &Path) -> Option<PathBuf> {
        let sysroot = Command::new("rustc").args(["--print", "sysroot"]).output().ok()?;
        let sysroot = PathBuf::from(String::from_utf8_lossy(&sysroot.stdout).trim());
        if !sysroot.join("lib/rustlib/wasm32-unknown-unknown").is_dir() {
            skipped("no wasm32 target");
            return None;
        }
        let page = dir.join("page");
        std::fs::create_dir_all(page.join("src")).unwrap();
        let aio = Path::new(env!("CARGO_MANIFEST_DIR")).join("../aio");
        let manifest = format!(
            "[package]\nname = \"airpage\"\nversion = \"0.1.0\"\nedition = \"2024\"\npublish = false\n\n\
             [lib]\ncrate-type = [\"cdylib\"]\n\n[dependencies]\n\
             aio = {{ package = \"aio-rs\", path = {:?}, default-features = false, features = [\"web\", \"air\"] }}\n\
             wasm-bindgen = \"0.2\"\n\n[workspace]\n",
            aio.display().to_string()
        );
        for (f, text) in [
            ("Cargo.toml", manifest.as_str()),
            ("Cargo.lock", include_str!("../../../Cargo.lock")),
            ("rust-toolchain.toml", include_str!("../../../rust-toolchain.toml")),
            ("src/lib.rs", include_str!("web/air.rs")),
        ] {
            std::fs::write(page.join(f), text).unwrap();
        }
        let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| page.join("target"), PathBuf::from);
        let app = App { id: "air-web-test".into(), name: "Air web test".into(), version: "1.0.0".into(), icon: None };
        let out = dir.join("site");
        match bind(&page, &target, ("airpage", "airpage", "aio"), &app, &out, true, false, None) {
            Err(e) if e.contains("needs wasm-bindgen") => {
                skipped(&e);
                return None;
            }
            r => _ = r.unwrap(),
        }
        let cfg = aio::config::Config::parse("[app]\nid = \"air-web-test\"\nname = \"Air web test\"").unwrap();
        assert_eq!(pack(&app, &cfg, &page, &out, "", true).unwrap(), Vec::<String>::new());
        // air alone in the page's build.
        let tree = Command::new("cargo")
            .args(["tree", "--offline", "-e", "normal", "--prefix", "none", "--target", "wasm32-unknown-unknown"])
            .current_dir(&page)
            .output()
            .unwrap();
        let tree = String::from_utf8_lossy(&tree.stdout);
        assert!(tree.lines().any(|l| l.starts_with("aio-rs-air ")), "air draws it: {tree}");
        assert!(!tree.lines().any(|l| l.starts_with(crate::lint::RENDERER_0_1)), "{tree}");
        Some(out)
    }

    /// The site's files over HTTP, by path (a static host), until dropped.
    pub(super) fn serve(site: PathBuf, l: tokio::net::TcpListener) -> tokio::task::JoinHandle<()> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                let site = site.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = s.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let path = req.split_whitespace().nth(1).unwrap_or("/").split('?').next().unwrap_or("/");
                    let path = if path == "/" { "/index.html" } else { path };
                    let file = site.join(path.trim_start_matches('/'));
                    let (status, body) = match std::fs::read(&file) {
                        Ok(b) if !path.contains("..") => ("200 OK", b),
                        _ => ("404 Not Found", vec![]),
                    };
                    let kind = match file.extension().and_then(|e| e.to_str()) {
                        Some("html") => "text/html",
                        Some("js") => "text/javascript",
                        Some("wasm") => "application/wasm",
                        Some("png") => "image/png",
                        _ => "application/octet-stream",
                    };
                    let head = format!(
                        "HTTP/1.1 {status}\r\ncontent-type: {kind}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    );
                    _ = s.write_all(head.as_bytes()).await;
                    _ = s.write_all(&body).await;
                });
            }
        })
    }

    /// The Redux DevTools extension's page API, as a stand-in that keeps
    /// what it is told; and the console's errors.
    const EXTENSION: &str = "window.__rd = { sent: [], subs: [] };
        window.__REDUX_DEVTOOLS_EXTENSION__ = { connect: () => ({
          init: (s) => window.__rd.sent.push(['init', s]), send: (a, s) => window.__rd.sent.push([a, s]),
          subscribe: (f) => window.__rd.subs.push(f), unsubscribe: () => { window.__rd.subs = [] } }) };
        window.__errs = []; const err = console.error.bind(console);
        console.error = (...a) => { window.__errs.push(a.join(' ')); err(...a) };";

    const REFUSED: &str = "window.refused = []; document.addEventListener('securitypolicyviolation', \
                           (e) => window.refused.push(e.violatedDirective + ' ' + e.blockedURI))";

    #[tokio::test(flavor = "multi_thread")]
    async fn the_page_draws_its_view_on_air_and_keeps_its_cell() {
        let dir = tempfile::tempdir().unwrap();
        let Some(site) = tokio::task::spawn_blocking({
            let d = dir.path().to_path_buf();
            move || site(&d)
        })
        .await
        .unwrap() else {
            return;
        };
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", l.local_addr().unwrap());
        let _server = serve(site, l);
        let Some(c) = Chromium::open(&url) else {
            return skipped("no Chromium");
        };
        let mut t = c.tabs(&url, 1).await.remove(0);
        t.call("Page.enable", json!({})).await.unwrap();
        t.call("Page.addScriptToEvaluateOnNewDocument", json!({ "source": REFUSED })).await.unwrap();
        t.call("Page.addScriptToEvaluateOnNewDocument", json!({ "source": EXTENSION })).await.unwrap();
        t.eval("location.reload()").await.ok();
        let text = |id: &str| format!("document.querySelector('[data-t={id}]')?.textContent ?? null");
        until(&mut t, &text("n"), json!("0")).await;
        assert_eq!(t.eval("typeof window.__airApply").await.unwrap(), json!("function"), "air's interpreter");
        // A click: the page's event to the module, the cell's call, its hole redrawn.
        for _ in 0..2 {
            t.eval("document.querySelector('[data-t=add]').click()").await.unwrap();
        }
        until(&mut t, &text("n"), json!("2")).await;
        // A dev page's cell in Redux DevTools: its calls, its states; a jump
        // puts the page's own cell back to the state the extension holds.
        let add = |n: u64| json!([{ "type": "add", "args": [1] }, { "n": n }]);
        until(&mut t, "window.__rd.sent", json!([["init", { "n": 0 }], add(1), add(2)])).await;
        let jump = |id: u64, n: u64| {
            let state = format!("{{\"n\":{n}}}");
            let m =
                json!({ "type": "DISPATCH", "payload": { "type": "JUMP_TO_STATE", "actionId": id }, "state": state });
            format!("window.__rd.subs.forEach((f) => f({m})), true")
        };
        t.eval(&jump(1, 1)).await.unwrap();
        until(&mut t, &text("n"), json!("1")).await;
        t.eval(&jump(2, 2)).await.unwrap();
        until(&mut t, &text("n"), json!("2")).await;
        assert_eq!(t.eval("window.__rd.sent.length").await.unwrap(), json!(3), "a jump is no action");
        // Typing: the value follows, and what shows it.
        let typed = "{ const i = document.querySelector('[data-t=name]'); i.value = 'hey'; \
                     i.dispatchEvent(new Event('input', { bubbles: true })); }";
        t.eval(typed).await.unwrap();
        until(&mut t, &text("typed"), json!("hey")).await;
        // A keyed list: each row its own state, kept as the rows move.
        t.eval("document.querySelector('[data-t=b]').click()").await.unwrap();
        until(&mut t, &text("b"), json!("b 1")).await;
        t.eval("document.querySelector('[data-t=flip]').click()").await.unwrap();
        let order = "[...document.querySelectorAll('li')].map((l) => l.textContent)";
        until(&mut t, order, json!(["b 1", "a 0"])).await;
        // The page's policy refused nothing (the interpreter is a script of the page's own making).
        assert_eq!(t.eval("window.refused").await.unwrap(), json!([]));
        // Reloaded: the cell as the browser kept it.
        // (`stale`: the old page, until the new one replaced it.)
        tokio::time::sleep(Duration::from_millis(300)).await;
        t.eval("window.stale = true; location.reload()").await.ok();
        until(&mut t, &format!("window.stale ? null : {}", text("n")), json!("2")).await;
        // A panic of the page's Rust: its place on the console, open in the overlay.
        t.eval("document.querySelector('[data-t=boom]').click()").await.ok();
        let said = "window.__errs.some((e) => e.startsWith('[aio] panicked at src/lib.rs:') && e.endsWith('boom in the page'))";
        until(&mut t, said, json!(true)).await;
        let shown = "document.querySelector('[data-aio-dev=overlay]').shadowRoot.textContent";
        until(&mut t, &format!("{shown}.includes('panic — panicked at src/lib.rs:')"), json!(true)).await;
    }
}

/// A release web build split (a real headless Chromium, the wasm32 target
/// and the lock's wasm-bindgen; skipped without them): a lazy view's code
/// in its own chunk, fetched only when the view first shows, and work off
/// the page's thread in workers running the page's own module.
#[cfg(all(test, unix))]
mod browser_split {
    use super::browser::{Chromium, skipped, until};
    use super::*;
    use aio::serde_json::json;
    use std::time::Duration;

    /// The test page's site, a release, built and bound into `dir/site`;
    /// `None` (said) without the wasm32 target or the right wasm-bindgen.
    fn site(dir: &Path) -> Option<PathBuf> {
        let sysroot = Command::new("rustc").args(["--print", "sysroot"]).output().ok()?;
        let sysroot = PathBuf::from(String::from_utf8_lossy(&sysroot.stdout).trim());
        if !sysroot.join("lib/rustlib/wasm32-unknown-unknown").is_dir() {
            skipped("no wasm32 target");
            return None;
        }
        let page = dir.join("page");
        std::fs::create_dir_all(page.join("src")).unwrap();
        let aio = Path::new(env!("CARGO_MANIFEST_DIR")).join("../aio");
        let manifest = format!(
            "[package]\nname = \"splitpage\"\nversion = \"0.1.0\"\nedition = \"2024\"\npublish = false\n\n\
             [lib]\ncrate-type = [\"cdylib\"]\n\n[dependencies]\n\
             aio = {{ package = \"aio-rs\", path = {:?}, default-features = false, features = [\"web\", \"air\"] }}\n\
             wasm-bindgen = \"0.2\"\njs-sys = \"0.3\"\n\n[workspace]\n",
            aio.display().to_string()
        );
        for (f, text) in [
            ("Cargo.toml", manifest.as_str()),
            ("Cargo.lock", include_str!("../../../Cargo.lock")),
            ("rust-toolchain.toml", include_str!("../../../rust-toolchain.toml")),
            ("src/lib.rs", include_str!("web/lazy.rs")),
        ] {
            std::fs::write(page.join(f), text).unwrap();
        }
        let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| page.join("target"), PathBuf::from);
        let app = App { id: "split-test".into(), name: "Split test".into(), version: "1.0.0".into(), icon: None };
        let out = dir.join("site");
        match bind(&page, &target, ("splitpage", "splitpage", "aio"), &app, &out, false, false, None) {
            Err(e) if e.contains("needs wasm-bindgen") => {
                skipped(&e);
                return None;
            }
            r => _ = r.unwrap(),
        }
        let cfg = aio::config::Config::parse("[app]\nid = \"split-test\"\nname = \"Split test\"").unwrap();
        assert_eq!(pack(&app, &cfg, &page, &out, "", false).unwrap(), Vec::<String>::new());
        Some(out)
    }

    /// The page's errors, kept.
    const ERRORS: &str = "window.__errs = []; const err = console.error.bind(console);
        console.error = (...a) => { window.__errs.push(a.join(' ')); err(...a) };
        addEventListener('error', (e) => window.__errs.push(String(e.message)));
        addEventListener('unhandledrejection', (e) => window.__errs.push(String(e.reason)));";

    /// The page's fetches: each file's name.
    const FETCHED: &str = "performance.getEntriesByType('resource').map((e) => e.name.split('/').pop())";

    #[tokio::test(flavor = "multi_thread")]
    async fn a_lazy_views_code_is_fetched_when_it_shows_and_work_runs_off_the_page() {
        let dir = tempfile::tempdir().unwrap();
        let Some(site) = tokio::task::spawn_blocking({
            let d = dir.path().to_path_buf();
            move || site(&d)
        })
        .await
        .unwrap() else {
            return;
        };
        // One chunk, for the one lazy view; the main module without names.
        let files: Vec<String> =
            std::fs::read_dir(&site).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        let chunks: Vec<&String> = files.iter().filter(|f| f.starts_with("app.") && f.ends_with(".wasm")).collect();
        assert_eq!(chunks.len(), 1, "{files:?}");
        let chunk = chunks[0].clone();
        // The view's code moved out: the kit's markdown parser and more.
        let (main, moved) = (wasm_size(&site).unwrap(), std::fs::metadata(site.join(&chunk)).unwrap().len());
        eprintln!(
            "the first module: {} KB ({} KB gzipped); the lazy view's chunk: {} KB",
            main.0 / 1000,
            main.1 / 1000,
            moved / 1000
        );
        // ~49 KB: air's hooks and effects keep their type-free part in the
        // first module (one copy), so less of each view is its own.
        assert!(moved > 45_000, "{moved} bytes moved");
        let sw = std::fs::read_to_string(site.join("sw.js")).unwrap();
        assert!(sw.contains(&format!("./{chunk}")) && sw.contains("./worker.js"), "held offline: {sw}");
        // The chunk not there at first: the host is behind (or the page offline).
        let held = site.join(format!("{chunk}.held"));
        std::fs::rename(site.join(&chunk), &held).unwrap();
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", l.local_addr().unwrap());
        let _server = super::browser_air::serve(site.clone(), l);
        let Some(c) = Chromium::open(&url) else {
            return skipped("no Chromium");
        };
        let mut t = c.tabs(&url, 1).await.remove(0);
        t.call("Page.enable", json!({})).await.unwrap();
        t.call("Page.addScriptToEvaluateOnNewDocument", json!({ "source": ERRORS })).await.unwrap();
        t.eval("location.reload()").await.ok();
        let text = |id: &str| format!("document.querySelector('[data-t={id}]')?.textContent ?? null");
        let click = |id: &str| format!("document.querySelector('[data-t={id}]').click()");
        until(&mut t, &text("tick"), json!("ticks 0")).await;
        let fetched = |f: &str| format!("{FETCHED}.includes('{f}')");
        assert_eq!(t.eval(&fetched("app_bg.wasm")).await.unwrap(), json!(true));
        assert_eq!(t.eval(&fetched(&chunk)).await.unwrap(), json!(false), "not shown: not fetched");
        // Work in a worker: its answer, counted off the page.
        t.eval(&click("count")).await.unwrap();
        until(&mut t, &text("counted"), json!("(348513, true)")).await;
        // Work that never ends: the page goes on; stopped, its worker ends.
        t.eval(&click("spin")).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        t.eval(&click("tick")).await.unwrap();
        until(&mut t, &text("tick"), json!("ticks 1")).await;
        assert_eq!(t.eval(&text("spun")).await.unwrap(), json!(""));
        t.eval(&click("stop")).await.unwrap();
        until(&mut t, &text("spun"), json!("refused: blocking `spin` was cancelled")).await;
        // A panic in the worker: refused, with its message.
        t.eval(&click("boom")).await.unwrap();
        let spun = "document.querySelector('[data-t=spun]').textContent";
        let said = format!("{spun}.includes('panicked') && {spun}.includes('boom in the worker')");
        until(&mut t, &said, json!(true)).await;
        assert_eq!(t.eval(&fetched(&chunk)).await.unwrap(), json!(false), "the worker's fetches are its own");
        // Shown while its chunk can't be fetched: `error` says so; shown
        // again, it is fetched again.
        t.eval(&click("open")).await.unwrap();
        let failed =
            "(document.querySelector('[data-t=failed]')?.textContent ?? '').includes('could not be loaded: HTTP 404')";
        until(&mut t, failed, json!(true)).await;
        std::fs::rename(&held, site.join(&chunk)).unwrap();
        t.eval(&click("close")).await.unwrap();
        // Shown: its chunk fetched, once more; its markdown, clicks and work run.
        t.eval(&click("open")).await.unwrap();
        until(&mut t, "document.querySelector('[data-t=heavy] h1')?.textContent ?? null", json!("Heavy")).await;
        let fetches = format!("{FETCHED}.filter((f) => f === '{chunk}').length");
        assert_eq!(t.eval(&fetches).await.unwrap(), json!(2));
        t.eval(&click("hit")).await.unwrap();
        until(&mut t, &text("hit"), json!("hits 1")).await;
        t.eval(&click("sum")).await.unwrap();
        until(&mut t, &text("summed"), json!("(385, true)")).await;
        assert_eq!(t.eval("window.__errs").await.unwrap(), json!([]));
    }
}

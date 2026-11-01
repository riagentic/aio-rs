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

/// What starts the page (`start.js`, a module): the offline cache, which
/// says why when the browser refuses it (`file://`, plain http off
/// localhost), then the bundle, which starts the app. A dev build has no
/// offline cache: each reload is the newest build (pure).
pub fn start_js(dev: bool) -> String {
    let cache = "if ('serviceWorker' in navigator) navigator.serviceWorker.register('./sw.js').catch((e) =>
  console.error('[aio] the offline cache (service worker) is not registered: the app works online only. It needs https (or localhost):', e));
";
    format!("{}import start from './app.js';\nstart();\n", if dev { "" } else { cache })
}

/// The window's page scripts (`page.js`): keys, focus, drag data and
/// dialogs as on the desktop (`aio::page`), the paint's state handed to the
/// first view (`ADOPT`), and in a dev build the overlay
/// that shows the page's errors on it and the live reload ([`RELOAD`]) (pure).
pub fn page_js(dev: bool) -> String {
    use aio::page::{ADOPT, DIALOG, DRAG, ENTER, FOCUS, KEYS, OVERLAY, WHEEL};
    // `;` after each piece: an `(() => …)()` piece followed by another
    // would otherwise call its result
    let pieces: &[&str] = match dev {
        true => &[OVERLAY, KEYS, WHEEL, FOCUS, DRAG, DIALOG, ENTER, ADOPT, aio_shell::splash::PAGE_JS, RELOAD],
        false => &[KEYS, WHEEL, FOCUS, DRAG, DIALOG, ENTER, ADOPT, aio_shell::splash::PAGE_JS],
    };
    pieces.iter().map(|p| format!("{};\n", p.trim_end())).collect()
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
/// cache-first, a navigation the host can't answer (offline, or a client
/// route it has no file for) given the page; the app's own server (its
/// socket and `/__aio/`) never cached; older builds' caches dropped when
/// it takes over. A new build is a new `sw.js`, which installs it (pure).
pub fn service_worker(id: &str, files: &[(String, Vec<u8>)]) -> String {
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
const own = (u) => u.pathname === '/ws' || u.pathname.startsWith('/__aio/');\n\
self.addEventListener('fetch', (e) => {{\n\
  const r = e.request, u = new URL(r.url);\n\
  if (r.method !== 'GET' || u.origin !== location.origin || own(u)) return;\n\
  const page = (c) => c.match('./');\n\
  e.respondWith(caches.open(V).then((c) => c.match(r, {{ ignoreSearch: true }}).then((hit) => hit || fetch(r).then(\n\
    (res) => (r.mode === 'navigate' && res.status === 404 ? page(c).then((p) => p || res) : res),\n\
    (err) => {{ if (r.mode === 'navigate') return page(c); throw err; }}))));\n\
}});\n",
        serde_json::Value::from(list),
        serde_json::Value::Object(sri)
    )
}

/// An icon file and its size in px.
type Icon = (String, u32);

/// The icons: the app's own scaled, else its generated
/// one; and why its own can't be installed, if so.
fn icons(app: &App, out: &Path) -> Result<(Vec<Icon>, Option<String>), String> {
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
                crate::icon::png(s.as_raw(), n, n)
            }
            None => crate::icon::icon_png(&app.name, &app.id, n),
        };
        let f = format!("icon-{n}.png");
        std::fs::write(out.join(&f), png).map_err(|e| e.to_string())?;
        made.push((f, n));
    }
    Ok((made, problem))
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
    let hue = cfg.ui.hue.unwrap_or_else(|| aio::kit::theme::hue(&app.id));
    let (icons, problem) = icons(app, out)?;
    let made: Vec<Icon> = icons.iter().filter(|(_, n)| *n != 180).cloned().collect();
    std::fs::write(out.join("manifest.webmanifest"), manifest(&app.name, hue, &made)).map_err(|e| e.to_string())?;
    let head = head(&app.name, hue, "icon-192.png", "icon-180.png");
    // The splash, over the page until the app has drawn its view.
    let splash = match &cfg.build.splash {
        Some(s) => {
            let png = dir.join(&s.image);
            let bytes = std::fs::read(&png).map_err(|e| format!("{}: {e}", png.display()))?;
            std::fs::write(out.join("splash.png"), &bytes).map_err(|e| e.to_string())?;
            let s = aio::config::Splash { png: Box::leak(bytes.into_boxed_slice()), ..s.clone() };
            let bg = s.background.as_ref().map(|[l, d]| [l.as_str(), d.as_str()]);
            aio_shell::splash::markup(&s, &app.name, &aio_shell::splash::colors(hue, bg), "./splash.png", true)
        }
        None => String::new(),
    };
    let page = index_html(&app.name, &cfg.app, &head, body).replacen("<body>\n", &format!("<body>\n{splash}"), 1);
    std::fs::write(out.join("index.html"), page).map_err(|e| e.to_string())?;
    std::fs::write(out.join("page.js"), page_js(dev)).map_err(|e| e.to_string())?;
    std::fs::write(out.join("start.js"), start_js(dev)).map_err(|e| e.to_string())?;
    let mut files = site_files(out, out).map_err(|e| e.to_string())?;
    files.retain(|(f, _)| f != "sw.js");
    files.sort();
    std::fs::write(out.join("sw.js"), service_worker(&app.id, &files)).map_err(|e| e.to_string())?;
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
/// build. `wasm-bindgen` must be the version the lock has, as they talk to
/// each other; `fetch`: aio's is fetched when none is here.
pub fn bind(
    root: &Path,
    target: &Path,
    (pkg, lib): (&str, &str),
    app: &App,
    out: &Path,
    dev: bool,
    fetch: bool,
) -> Result<(), String> {
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
    let mut args = vec!["rustc", "--target", "wasm32-unknown-unknown", "-p", pkg, "--lib", "--crate-type", "cdylib"];
    if !dev {
        args.push("--release");
    }
    let mut cargo = crate::freeze::cargo("the web build", &args)?;
    if !dev {
        crate::build::remap(&mut cargo, root);
    }
    // Cargo's errors are said here and kept for the dev page (`ax dev --web`).
    let o = cargo
        .current_dir(root)
        .env("AIORT_APP_ID", &app.id)
        .env("AIORT_APP_VERSION", &app.version)
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|e| e.to_string())?;
    let said = String::from_utf8_lossy(&o.stderr);
    eprint!("{said}");
    if !o.status.success() {
        let why = "the web build failed (rustup target add wasm32-unknown-unknown, if it is missing)";
        return Err(match fatal(&said) {
            e if e.is_empty() => why.into(),
            e => format!("{why}\n{e}"),
        });
    }
    let profile = if dev { "debug" } else { "release" };
    let wasm = target.join("wasm32-unknown-unknown").join(profile).join(format!("{lib}.wasm"));
    _ = std::fs::remove_dir_all(out);
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let ok = Command::new(&bindgen)
        .args(["--target", "web", "--no-typescript", "--out-name", BUNDLE, "--out-dir"])
        .arg(out)
        .arg(&wasm)
        .status()
        .map_err(|e| e.to_string())?;
    if !ok.success() {
        return Err(format!("wasm-bindgen could not bind {}", wasm.display()));
    }
    Ok(())
}

/// An app's web build: its package, its cdylib lib, and where cargo builds.
struct Site {
    pkg: crate::dev::Pkg,
    app: App,
    cfg: aio::config::Config,
    lib: String,
    /// The package with that lib: the app's own, or the library its launcher uses.
    lib_pkg: String,
    target: PathBuf,
    root: PathBuf,
}

/// The package and name of the library a web build binds (pure): the
/// first of `dirs` (the app's package, then the library its launcher uses,
/// as `ax create` lays them out) whose package has a library and uses
/// wasm-bindgen (its `#[wasm_bindgen(start)]`).
fn web_lib(meta: &serde_json::Value, dirs: &[PathBuf]) -> Option<(String, String)> {
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
        Ok(Self { lib, lib_pkg, target: dir("target_directory")?, root: dir("workspace_root")?, pkg, app, cfg })
    }

    /// Its CSS step, the bundle and the page into `out`.
    fn make(&self, out: &Path, dev: bool) -> Result<Vec<String>, String> {
        if let Some(did) = crate::css::step(&self.cfg, &app_dir(&self.pkg)?)? {
            eprintln!("ax build: {did}");
        }
        bind(&self.root, &self.target, (&self.lib_pkg, &self.lib), &self.app, out, dev, true)?;
        pack(&self.app, &self.cfg, &app_dir(&self.pkg)?, out, "", dev)
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
    let out = site.root.join("dist").join(format!("{}-web", site.app.id));
    let said = site.make(&out, dev)?;
    let (raw, gz) = wasm_size(&out).map_err(|e| e.to_string())?;
    let size = format!("wasm: {} KB, {} KB gzipped", raw / 1000, gz / 1000);
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

/// The dev page's live reload (`page.js` in a dev build): the page starts
/// again once `ax dev --web` built anew (`/aio-dev/build`); a failed
/// build's errors show on the page, open (aio's diagnostic page), until a
/// good one reloads it. With no dev server (a static `--dev` build) it
/// stops asking.
pub const RELOAD: &str = "(() => {
  let seen = null, failed = null;
  const tick = () => fetch('/aio-dev/build', { cache: 'no-store' }).then((r) => (r.ok ? r.json() : null)).then((b) => {
    if (!b) return;
    if (seen !== null && b.n !== seen) return location.reload();
    seen = b.n;
    if (b.error && b.error !== failed && window.__aioDiag) window.__aioDiag('build failed', b.error, true);
    failed = b.error || null;
    setTimeout(tick, 250);
  }, () => setTimeout(tick, 1000));
  tick();
})();";

/// Whether a change to `changed` needs a new bundle: a stylesheet's needs
/// none (the page reads it again, `aio::use_css!`) (pure).
pub fn rebuilds(changed: &[PathBuf]) -> bool {
    changed.iter().any(|p| p.extension().is_none_or(|e| e != "css"))
}

/// `ax dev <name> --web`: the web build (debug) served on loopback, built
/// again on each saved change to its sources: a stylesheet shows at once
/// (`/aio-dev/sheet`, the page asks), any other edit (`rsx!`, Rust) builds
/// the bundle again and the open tabs reload (`/aio-dev/build`). A failed
/// build keeps the last page. Runs until Ctrl-C.
pub fn dev(name: &str) -> Result<String, String> {
    dev_then(name, |_| Ok(()))
}

/// Whether app `name` has a web build ([`dev`] can serve it).
pub fn has_web(name: &str) -> bool {
    Site::of(name).is_ok()
}

/// [`dev`], with `ready` called on its port once it serves (`ax dev
/// --android` opens the page on the phone then); its error ends the run.
pub fn dev_then(name: &str, ready: impl FnOnce(u16) -> Result<(), String> + Send + 'static) -> Result<String, String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    let site = Site::of(name)?;
    let out = site.target.join("aio").join("web-dev").join(&site.app.id);
    eprintln!("ax dev: {} for the web (dev)…", site.app.name);
    site.make(&out, true)?;
    let built = std::sync::Arc::new(AtomicU64::new(1));
    let dirs: Vec<PathBuf> = site.pkg.dirs.iter().filter_map(|d| d.canonicalize().ok()).collect();
    let paths = crate::dev::watched(&site.pkg, Path::new(""), &crate::dev::Watching::All);
    let (tx, rx) = std::sync::mpsc::channel::<Vec<PathBuf>>();
    let mut w = notify::recommended_watcher(move |e: notify::Result<notify::Event>| {
        if let Ok(e) = e
            && crate::dev::changes(&e.kind)
        {
            let ps: Vec<PathBuf> = e.paths.into_iter().filter(|p| crate::dev::relevant(p)).collect();
            if !ps.is_empty() {
                _ = tx.send(ps);
            }
        }
    })
    .map_err(|e| e.to_string())?;
    for p in paths.iter().filter(|p| p.exists()) {
        notify::Watcher::watch(&mut w, p, notify::RecursiveMode::Recursive).map_err(|e| e.to_string())?;
    }
    // The last build's errors, until a good one.
    let failed = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    let (n, error) = (built.clone(), failed.clone());
    let http = aio::http::Http::default()
        .shell(&out)
        .route(aio::http::Route::get("/aio-dev/build", move |_| {
            let b = serde_json::json!({ "n": n.load(Ordering::SeqCst), "error": *error.lock().unwrap() });
            async move { aio::http::Reply::json(&b).header("cache-control", "no-store") }
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
    aio::rt().block_on(async move {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.map_err(|e| e.to_string())?;
        let port = l.local_addr().map_err(|e| e.to_string())?.port();
        eprintln!("ax dev: http://127.0.0.1:{port}/ (Ctrl-C ends it)");
        let opts = aio::sync::Options { open: true, http, ..Default::default() };
        tokio::spawn(aio::sync::serve_with(aio::Handle::<DevSite>::memory(), l, opts));
        tokio::task::spawn_blocking(move || ready(port)).await.map_err(|e| e.to_string())??;
        tokio::task::spawn_blocking(move || {
            while let Ok(mut changed) = rx.recv() {
                // A burst of saves is one change.
                while let Ok(more) = rx.recv_timeout(std::time::Duration::from_millis(300)) {
                    changed.extend(more);
                }
                if !rebuilds(&changed) {
                    continue;
                }
                let started = std::time::Instant::now();
                match site.make(&out, true) {
                    Ok(_) => {
                        *failed.lock().unwrap() = None;
                        built.fetch_add(1, Ordering::SeqCst);
                        eprintln!("ax dev: built in {} ms; the page reloads", started.elapsed().as_millis());
                    }
                    Err(e) => {
                        let first = e.lines().next().unwrap_or_default();
                        eprintln!("ax dev: {first}; the page keeps the last build and shows why");
                        *failed.lock().unwrap() = Some(e);
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

/// What `ax dev --web`'s server serves besides the page: nothing.
#[aio::cell(shared)]
pub struct DevSite {}

#[aio::methods]
impl DevSite {}

#[cfg(test)]
mod tests {
    use super::*;

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
        // The worker registers from start.js, saying why when it can't; a dev build's has none.
        let start = start_js(false);
        assert!(start.contains("register('./sw.js')") && start.contains("It needs https (or localhost)"));
        assert!(start.find("register(").unwrap() < start.find("start();").unwrap());
        assert!(!start_js(true).contains("serviceWorker") && start_js(true).contains("start();"));
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
        assert!(page_js(false).contains("aria-modal") && page_js(false).contains("autofocus"));
        assert!(!page_js(false).contains("__aioDiag") && page_js(true).contains("window.__aioDiag = add"));
        assert!(!page_js(false).contains("/aio-dev/build") && page_js(true).contains("/aio-dev/build"));
        // no piece runs into the next: `})()\n(() =>` calls the first's result
        for dev in [false, true] {
            assert!(!page_js(dev).contains(")\n("), "{}", page_js(dev));
        }
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
        let js = format!(
            "const on = {{}}, ticks = [], answers = [{{ n: 1, error: null }}, {{ n: 1, error: {e:?} }}, {{ n: 1, error: {e:?} }}, {{ n: 2, error: null }}];
             let reloaded = false;
             globalThis.window = {{ addEventListener: (k, f) => on[k] = f }};
             globalThis.location = {{ reload: () => reloaded = true }};
             globalThis.setTimeout = (f) => ticks.push(f);
             globalThis.fetch = () => Promise.resolve({{ ok: true, json: () => answers.shift() }});
             const el = (tag) => ({{ tag, kids: [], style: {{}}, attrs: {{}}, textContent: '',
               setAttribute(k, v) {{ this.attrs[k] = v; }},
               appendChild(c) {{ this.kids.push(c); }} }});
             globalThis.document = {{ body: el('body'), createElement: el }};
             {}\n{RELOAD}
             const flush = () => new Promise((r) => setImmediate(r));
             const step = () => {{ ticks.shift()(); return flush(); }};
             (async () => {{
               await flush(); const before = document.body.kids.length;
               await step(); await step();
               const [list, badge] = document.body.kids[0].kids;
               const shown = [list.kids.length, list.kids[0].textContent, list.style.display, badge.textContent];
               await step();
               console.log(JSON.stringify([before, ...shown, reloaded]));
             }})();",
            aio::page::OVERLAY
        );
        let Ok(out) = std::process::Command::new("node").args(["-e", &js]).output() else {
            return eprintln!("skipped: no node");
        };
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let got: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        // Nothing while it builds; the error once (asked twice), open; a good build reloads.
        assert_eq!(got, serde_json::json!([0, 1, format!("build failed — {e}"), "block", "1 page error", true]));
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
        let sw = service_worker("notes", &files);
        assert!(sw.contains(r#"const FILES = ["./","./app.js","./icon 1%23.png"];"#), "{sw}");
        assert!(sw.contains(r#""./app.js":"sha256-"#) && !sw.contains(r#""./icon 1%23.png":"sha256"#), "{sw}");
        assert!(sw.contains(&format!("aio-notes-{}@", content_hash(&files))));
        // Another build is another worker; the same build the same.
        let other = vec![("app.js".to_string(), b"js2".to_vec())];
        assert_ne!(content_hash(&files), content_hash(&other));
        assert_eq!(sw, service_worker("notes", &files));
        // The app's own server is never cached.
        assert!(sw.contains("u.pathname === '/ws' || u.pathname.startsWith('/__aio/')"));
    }

    /// What a person did to the paint before the view was drawn is done to
    /// the view as it takes the paint's place (aio's hydration kept it by
    /// keeping the nodes), in a real Chromium (skipped without one).
    #[cfg(unix)]
    #[test]
    fn the_first_view_takes_the_paints_place_with_what_was_done_to_it() {
        use aio::serde_json::json;
        let tree = |id: &str| {
            format!(
                "<form><input id=t{id} value=a><textarea id=x{id}>b</textarea><input type=checkbox id=c{id}>\
                 <select id=s{id}><option>1</option><option>2</option></select></form>\
                 <div id=box{id} style='height:40px;overflow:auto'><p style='height:400px'>tall</p></div>\
                 <details id=d{id}><summary>more</summary>in</details><audio id=a{id}></audio>"
            )
        };
        // The view draws one part otherwise: there it is the view's.
        let view = tree("v").replace("<details id=dv><summary>more</summary>in</details>", "<p id=dv>other</p>");
        let html = format!(
            "<div id=main>{}</div><div hidden>{view}</div><p style='height:3000px'>page</p><script>{}</script>",
            tree("p"),
            aio::page::ADOPT
        );
        let Some(page) = crate::cdp::tests::Chromium::open(&html) else { return };
        aio::rt().block_on(async {
            let mut s = page.session().await;
            // Two seconds of silence, playable in both trees.
            let audio = "var $ = (i) => document.getElementById(i);
                 const n = 16000, b = new DataView(new ArrayBuffer(44 + n));
                 [[0, 0x52494646], [8, 0x57415645], [12, 0x666d7420], [36, 0x64617461]].forEach(([o, v]) => b.setUint32(o, v));
                 [[4, 36 + n], [16, 16], [24, 8000], [28, 8000], [40, n]].forEach(([o, v]) => b.setUint32(o, v, true));
                 [[20, 1], [22, 1], [32, 1], [34, 8]].forEach(([o, v]) => b.setUint16(o, v, true));
                 const url = URL.createObjectURL(new Blob([b.buffer], { type: 'audio/wav' }));
                 const ready = (a) => new Promise((ok) => { a.onloadedmetadata = ok; a.src = url; });
                 Promise.all([ready($('ap')), ready($('av'))]).then(() => { $('ap').currentTime = 1.25; return $('ap').duration; })";
            assert_eq!(s.eval(audio).await.unwrap(), json!(2));
            let did = "var $ = (i) => document.getElementById(i);
                 $('tp').value = 'typed'; $('tp').focus(); $('tp').setSelectionRange(1, 3);
                 $('xp').value = 'more text'; $('cp').checked = true; $('sp').selectedIndex = 1;
                 $('boxp').scrollTop = 120; $('dp').open = true; scrollTo(0, 500);
                 window.heard = [];
                 for (const i of ['tv', 'xv', 'cv', 'sv']) for (const t of ['input', 'change'])
                   $(i).addEventListener(t, () => heard.push(i + ' ' + t));
                 __aioAdopt($('main'), document.querySelector('[hidden]'));
                 [document.querySelectorAll('#main').length, document.querySelector('[hidden]'), $('tv').value,
                  document.activeElement.id, $('tv').selectionStart, $('tv').selectionEnd, $('xv').value,
                  $('cv').checked, $('sv').selectedIndex, $('boxv').scrollTop, scrollY, $('dv').tagName, heard,
                  $('av').currentTime]";
            let got = s.eval(did).await.unwrap();
            let heard = json!(["tv input", "xv input", "cv change", "sv change"]);
            assert_eq!(
                got,
                json!([1, null, "typed", "tv", 1, 3, "more text", true, 1, 120, 500, "P", heard, 1.25]),
                "the paint is gone, the view shows and has what was done"
            );
        });
    }
}

/// The web build in a real browser (a headless Chromium, the wasm32 target
/// and the lock's wasm-bindgen; skipped without them): its page runs a
/// synced copy of its server's cell, offline too, twin tabs mirrored.
#[cfg(all(test, unix))]
pub(crate) mod browser {
    use super::*;
    use aio::prelude::*;
    use aio::serde_json::{Value, json};
    use std::time::Duration;

    include!("web/board.rs");
    include!("web/view.rs");

    const KEY: &str = "a-long-random-app-key-for-tests";

    /// The test page's site, built and bound into `dir/site`; `None` (said)
    /// without the wasm32 target or the right wasm-bindgen.
    fn site(dir: &Path) -> Option<PathBuf> {
        let sysroot = Command::new("rustc").args(["--print", "sysroot"]).output().ok()?;
        let sysroot = PathBuf::from(String::from_utf8_lossy(&sysroot.stdout).trim());
        if !sysroot.join("lib/rustlib/wasm32-unknown-unknown").is_dir() {
            eprintln!("no wasm32 target: skipped");
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
             web-sys = {{ version = \"0.3\", features = [\"Location\", \"Window\"] }}\n\
             dioxus = {{ version = \"=0.7.10\", default-features = false, features = [\"macro\", \"html\", \"signals\", \"hooks\"] }}\n\n[workspace]\n",
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
        match bind(&page, &target, ("webpage", "webpage"), &app, &out, false, false) {
            Err(e) if e.contains("needs wasm-bindgen") => {
                eprintln!("{e}: skipped");
                return None;
            }
            r => r.unwrap(),
        }
        let cfg = aio::config::Config::parse(
            "[app]\nid = \"web-test\"\nname = \"Web test\"\nlang = \"cs\"\ndir = \"auto\"\n\
             [build.splash]\nimage = \"splash.png\"\ntext = \"Starting {title}\"\nmin_ms = 300",
        )
        .unwrap();
        std::fs::write(page.join("splash.png"), include_bytes!("../../aio/tests/fixtures/splash.png")).unwrap();
        assert_eq!(pack(&app, &cfg, &page, &out, "", false).unwrap(), Vec::<String>::new());
        let index = std::fs::read_to_string(out.join("index.html")).unwrap();
        assert!(index.contains("<body>\n<style>#aio-splash{") && index.contains("<p>Starting Web test</p>"));
        assert!(out.join("splash.png").is_file());
        Some(out)
    }

    /// The network between the browser and the server: up while its task runs.
    fn link(l: tokio::net::TcpListener, to: String) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut links = tokio::task::JoinSet::new();
            while let Ok((mut a, _)) = l.accept().await {
                let to = to.clone();
                links.spawn(async move {
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
        let h = Handle::<Board>::memory();
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server = l.local_addr().unwrap().to_string();
        let http = aio_shell::web::painted(aio::http::Http::default(), h.clone(), view, &site);
        let opts = aio::sync::Options { token: KEY.into(), http, ..Default::default() };
        tokio::spawn(aio::sync::serve_with(h.clone(), l, opts));
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let up = link(l, server.clone());
        let url = format!("http://{addr}/?token={KEY}");
        let Some(c) = Chromium::open(&url) else {
            return eprintln!("no Chromium: skipped");
        };
        let mut a = c.tabs(&url, 1).await.remove(0);
        // Its twin: another tab of the app in the same browser.
        a.call("Target.createTarget", json!({ "url": url })).await.unwrap();
        let mut tabs = c.tabs(&url, 2).await;
        let (mut a, mut b) = (tabs.remove(0), tabs.remove(0));
        // What the page's policy refuses, counted from now on (it reloads).
        a.call("Page.enable", json!({})).await.unwrap();
        a.call("Page.addScriptToEvaluateOnNewDocument", json!({ "source": REFUSED })).await.unwrap();
        a.eval("location.reload()").await.ok();
        for t in [&mut a, &mut b] {
            until(t, "window.ready", json!("yes")).await;
            // The splash ([build.splash]) went once the view was drawn.
            until(t, "!document.getElementById('aio-splash')", json!(true)).await;
            // Installed: the worker serves the page from now on.
            until(t, "!!navigator.serviceWorker.controller", json!(true)).await;
        }
        // The page's language and direction, as aio.toml says them.
        let html = "[document.documentElement.lang, document.documentElement.dir]";
        assert_eq!(a.eval(html).await.unwrap(), json!(["cs", "auto"]));
        // Online: a call reaches the server, and both tabs show it.
        assert_eq!(a.eval("import('./app.js').then((m) => m.add('a'))").await.unwrap(), json!(true));
        served(&h, &["a"]).await;
        until(&mut b, SHOWN, json!({ "items": ["a"], "pending": 0, "online": true })).await;
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
        // An async method: its effect runs on the page's timer, counted while it does.
        assert_eq!(a.eval(&format!("{APP}.then((m) => m.later())")).await.unwrap(), json!(true));
        assert_eq!(a.eval(TALLY).await.unwrap()["running"], json!(1));
        until(&mut a, TALLY, json!({ "n": 13, "busy": true, "running": 0 })).await;
        // The network is back on its port: the call reaches the server once,
        // sent by both tabs (the server applies a call id once).
        let l = tokio::net::TcpListener::bind(&addr).await.unwrap();
        let _up = link(l, server);
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
        // The view took the paint's place, through the page's `__aioAdopt`
        // (what was done to the paint goes with it): one main, nothing left unseen.
        let one = "typeof __aioAdopt === 'function' && document.querySelectorAll('#main').length === 1 && !document.querySelector('[hidden]')";
        until(&mut a, one, json!(true)).await;
        // The window's page scripts and kit in the page: the field that
        // asks for focus has it (it came after the page loaded), and an
        // island ran its script and sent its event back, all under the
        // page's policy (no eval), which refused nothing.
        until(&mut a, "document.activeElement && document.activeElement.id", json!("auto")).await;
        until(&mut a, "document.getElementById('island').textContent", json!("island 4")).await;
        until(&mut a, "document.getElementById('doubled').textContent", json!("8")).await;
        assert_eq!(a.eval("window.refused").await.unwrap(), json!([]));
        // The page as the server paints it now: the view over its state, for this viewer.
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
        let painted = r#"<div id="main"><ul id="items"><li>a</li><li>b</li><li>ui</li><li>server</li></ul>"#;
        assert!(page.contains(painted) && page.contains("./start.js"), "{page}");
        // Its head as the view asks for it, collected on the server and kept in the page.
        assert!(page.contains("<title>4 items</title>"), "{page}");
        assert!(page.contains(r#"<meta name="description" content="board" data-aio-head></head>"#), "{page}");
        until(&mut a, "document.title", json!("4 items")).await;
        let tags = "document.querySelectorAll('meta[name=description][data-aio-head]').length";
        assert_eq!(a.eval(tags).await.unwrap(), json!(1));
        // What the worker doesn't hold goes to the server as asked.
        let asked = "fetch('./missing.txt').then((r) => r.status, (e) => 'failed: ' + e)";
        assert_eq!(b.eval(asked).await.unwrap(), json!(404));
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
            return eprintln!("no Chromium: skipped");
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

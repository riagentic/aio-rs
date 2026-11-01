//! `ax build <app> --android`: the app's library for each CPU type (the
//! NDK), in a small Gradle project around the system WebView, signed with the
//! app's own key: `dist/<id>-<version>-android-<abi>.apk`, each within budget.
//!
//! The library is the app's `lib` target (it calls `aio_shell::android_app!`)
//! built as a `cdylib`. It is signed with the release keystore the app names
//! (`[build.android] keystore`, or `AIO_ANDROID_KEYSTORE` in CI), else with a
//! key made on first use in `~/.config/aio-rt/keys/<id>.android.p12` (keep
//! it: Android only updates an app signed by the same key). With
//! `[build.android] aab`, also `dist/<id>-<version>-android.aab` for Play.
use crate::build::{App, BUDGET_MB, over_budget};
use aio::config::{AndroidBuild, ColorScheme, Splash};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The NDK's API level for the libraries: the APK's minSdk.
const API: u32 = 29;

/// The library's link: relocations packed (Android 9+ reads RELR) and
/// identical functions folded, a twentieth off the library.
const LINK: [&str; 6] = [
    "-C",
    "link-arg=-Wl,--pack-dyn-relocs=android+relr",
    "-C",
    "link-arg=-Wl,--use-android-relr-tags",
    "-C",
    "link-arg=-Wl,--icf=all",
];

/// Each CPU type: Android's name, Rust's target. 32-bit ARM too, as aio
/// ships it: phones sold with a 64-bit CPU and a 32-bit Android still exist.
pub const ABIS: [(&str, &str); 3] = [
    ("arm64-v8a", "aarch64-linux-android"),
    ("armeabi-v7a", "armv7-linux-androideabi"),
    ("x86_64", "x86_64-linux-android"),
];

/// The NDK's clang for Rust target `target` (`armv7` is `armv7a` there), on
/// a host running `os` (a `.cmd` on Windows).
pub fn clang(target: &str, os: &str) -> String {
    let t = target.replace("armv7-", "armv7a-");
    format!("{t}{API}-clang{}", if os == "windows" { ".cmd" } else { "" })
}

/// The NDK's prebuilt toolchain folder for a host running `os` (an ARM Mac
/// runs the x86_64 one, as Google ships it).
pub fn prebuilt(os: &str) -> &'static str {
    match os {
        "macos" => "darwin-x86_64",
        "windows" => "windows-x86_64",
        _ => "linux-x86_64",
    }
}

/// The dev APK's activity (`ax dev --android`): the page `ax dev --web`
/// serves, over `adb reverse`; no library in it.
/// Which APK a project makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Apk {
    /// The app.
    App,
    /// A dev copy beside the app (`<application id>.dev`, "<name> (dev)"):
    /// `ax dev --android --native`, the app's library in it.
    Dev,
    /// The dev page (`ax dev --android`): beside the app as [`Apk::Dev`],
    /// no library, its [`DEV`] activity showing the page `ax dev --web`
    /// serves, over `adb reverse`.
    Page,
}

/// The dev APK activity's class.
pub const DEV: &str = "aio.rt.DevActivity";

const DEV_ACTIVITY: (&str, &str) =
    ("app/src/main/java/aio/rt/DevActivity.java", include_str!("../android/app/src/main/java/aio/rt/DevActivity.java"));

const TEMPLATE: [(&str, &str); 7] = [
    ("settings.gradle.kts", include_str!("../android/settings.gradle.kts")),
    ("build.gradle.kts", include_str!("../android/build.gradle.kts")),
    ("gradle.properties", include_str!("../android/gradle.properties")),
    ("app/build.gradle.kts", include_str!("../android/app/build.gradle.kts")),
    ("app/src/main/AndroidManifest.xml", include_str!("../android/app/src/main/AndroidManifest.xml")),
    ("app/src/main/java/aio/rt/Host.java", include_str!("../android/app/src/main/java/aio/rt/Host.java")),
    (
        "app/src/main/java/aio/rt/MainActivity.java",
        include_str!("../android/app/src/main/java/aio/rt/MainActivity.java"),
    ),
];

/// The Android application id: `aio.rt.<id>` (letters, digits, `_`).
pub fn application_id(id: &str) -> String {
    let id: String = id.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    let id = if id.starts_with(|c: char| c.is_ascii_digit()) { format!("_{id}") } else { id };
    format!("aio.rt.{id}")
}

/// The id aio's APK of aio app `label` has (`app.aio.<label sans non-[a-z0-9]>`,
/// aio's `androidApplicationId`): a port installs over it. None when aio
/// could not have made one (it then built no APK).
pub fn aio_application_id(label: &str) -> Option<String> {
    let s: String = label.chars().filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit()).collect();
    s.starts_with(|c: char| c.is_ascii_lowercase()).then(|| format!("app.aio.{s}"))
}

/// The app's application id: the declared one, else a port's aio one
/// (`[app] port_of`: the aio APK it replaces), else [`application_id`].
pub fn application_id_of(id: &str, port_of: Option<&str>, cfg: &AndroidBuild) -> String {
    cfg.application_id.clone().or_else(|| port_of.and_then(aio_application_id)).unwrap_or_else(|| application_id(id))
}

/// Where a keystore password is: an environment variable (CI) or a file
/// (aio's own key), never on a command line; or a public one (Android's
/// debug key: `android`).
#[derive(Clone, Debug, PartialEq)]
pub enum Pass {
    Env(&'static str),
    File(PathBuf),
    Plain(&'static str),
}

impl Pass {
    /// apksigner's form: `env:<VAR>`, `file:<path>`, `pass:<public>`.
    fn apksigner(&self) -> String {
        match self {
            Pass::Env(v) => format!("env:{v}"),
            Pass::File(p) => format!("file:{}", p.display()),
            Pass::Plain(p) => format!("pass:{p}"),
        }
    }

    /// jarsigner's: `-storepass:env <VAR>`, `-storepass:file <path>`.
    fn jarsigner(&self, flag: &str) -> [String; 2] {
        match self {
            Pass::Env(v) => [format!("{flag}:env"), v.to_string()],
            Pass::File(p) => [format!("{flag}:file"), p.display().to_string()],
            Pass::Plain(p) => [flag.to_string(), p.to_string()],
        }
    }
}

/// The key a build signs with.
#[derive(Clone, Debug, PartialEq)]
pub struct Signing {
    pub keystore: PathBuf,
    pub alias: String,
    pub pass: Pass,
    /// The key's own password, when it has one.
    pub key_pass: Option<Pass>,
}

pub const KEYSTORE_ENV: &str = "AIO_ANDROID_KEYSTORE";
const KEYSTORE_PASSWORD_ENV: &str = "AIO_ANDROID_KEYSTORE_PASSWORD";
const KEY_ALIAS_ENV: &str = "AIO_ANDROID_KEY_ALIAS";
const KEY_PASSWORD_ENV: &str = "AIO_ANDROID_KEY_PASSWORD";

/// The release keystore the environment (first) or `[build.android]`
/// names, relative to `app_dir`: None when none is named (aio's own key
/// signs), refused before any build when it can't be used.
pub fn signing(
    env: impl Fn(&str) -> Option<String>,
    cfg: &AndroidBuild,
    app_dir: &Path,
) -> Result<Option<Signing>, String> {
    let env = |k: &str| env(k).filter(|v| !v.is_empty());
    let Some(named) = env(KEYSTORE_ENV).or_else(|| cfg.keystore.clone()) else { return Ok(None) };
    let from = if env(KEYSTORE_ENV).is_some() {
        format!("{KEYSTORE_ENV}={named}")
    } else {
        format!("[build.android] keystore {named:?}")
    };
    let keystore = app_dir.join(&named);
    if !keystore.is_file() {
        return Err(format!("{from}: no keystore file at {}", keystore.display()));
    }
    let alias = env(KEY_ALIAS_ENV).or_else(|| cfg.key_alias.clone());
    let missing: Vec<&str> = [
        (env(KEYSTORE_PASSWORD_ENV).is_none(), KEYSTORE_PASSWORD_ENV),
        (alias.is_none(), "AIO_ANDROID_KEY_ALIAS (or [build.android] key_alias)"),
    ]
    .into_iter()
    .filter_map(|(gone, what)| gone.then_some(what))
    .collect();
    if !missing.is_empty() {
        return Err(format!(
            "{from} names a keystore, but {} not set: a release is signed with the keystore, its password and the \
             key's alias ({KEY_PASSWORD_ENV} when the key has its own password)",
            missing.join(" and ") + if missing.len() > 1 { " are" } else { " is" }
        ));
    }
    Ok(Some(Signing {
        keystore,
        alias: alias.unwrap_or_default(),
        pass: Pass::Env(KEYSTORE_PASSWORD_ENV),
        key_pass: env(KEY_PASSWORD_ENV).map(|_| Pass::Env(KEY_PASSWORD_ENV)),
    }))
}

/// `1.2.345` → 102000345, aio's formula (`major·10⁸ + minor·10⁶ + build`,
/// aio's third number being its build count): a port's APK installs over
/// the aio APK it replaces, never as a refused downgrade. Grows with every
/// release (a prerelease shares its release's code, so it must not ship to
/// the same users); out of range is refused, never wrapped onto a newer code.
pub fn version_code(v: &str) -> Result<u32, String> {
    let core = v.split(['-', '+']).next().unwrap_or(v);
    let n: Vec<u32> =
        core.split('.').map(|p| p.parse().map_err(|_| format!("version `{v}`"))).collect::<Result<_, _>>()?;
    let [major, minor, patch] = n[..] else { return Err(format!("version `{v}`: major.minor.patch")) };
    if major > 20 || minor > 99 || patch > 999_999 {
        return Err(format!(
            "version `{v}` does not fit an Android version code: major ≤ 20, minor ≤ 99, patch ≤ 999999 (aio's rule)"
        ));
    }
    Ok(major * 100_000_000 + minor * 1_000_000 + patch)
}

/// The APK's label: the app's name (its id when it has none), as aapt2
/// reads an attribute (aio's `_appNameXml`): control characters dropped, a
/// backslash and a leading `?` or `@` (a resource reference) escaped, then
/// XML's escapes.
pub fn label_xml(title: &str) -> String {
    let t: String = title.chars().filter(|c| !c.is_control() && !matches!(c, '\u{fffe}' | '\u{ffff}')).collect();
    let t = t.replace('\\', "\\\\");
    let t = if t.starts_with(['?', '@']) { format!("\\{t}") } else { t };
    t.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The `{{UPPER_CASE}}` tokens left in `text` once filled, each once
/// (aio's `_leftoverTokens`): a typo in an overlay, or another aio's key.
pub fn leftover_tokens(text: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let mut rest = text;
    while let Some(at) = rest.find("{{") {
        rest = &rest[at + 2..];
        let name: String = rest.chars().take_while(|c| c.is_ascii_uppercase() || *c == '_').collect();
        if !name.is_empty() && rest[name.len()..].starts_with("}}") {
            let t = format!("{{{{{name}}}}}");
            if !out.contains(&t) {
                out.push(t);
            }
        }
    }
    out
}

/// Where the launcher icon goes in the project.
pub const ICON: &str = "app/src/main/res/drawable/ic_launcher.png";

/// The splash's theme (`[build.splash]`, [`splash_files`]).
const SPLASH_THEME: &str = "@style/AioSplash";

/// The system theme for `[build.android] color_scheme`.
fn system_theme(scheme: ColorScheme) -> &'static str {
    match scheme {
        ColorScheme::System => "@android:style/Theme.DeviceDefault.DayNight",
        ColorScheme::Light => "@android:style/Theme.DeviceDefault.Light",
        ColorScheme::Dark => "@android:style/Theme.DeviceDefault",
    }
}

/// An APK's splash files (`[build.splash]`, aio's `_splashFiles`): its image and
/// what the activity reads of it (`assets/aio-splash.png`, `.json`: its
/// minimum and cap, its text with `{title}` filled, its colours by mode and
/// the mode `scheme` fixes), and the theme whose window background is the
/// splash's, per mode where the mode follows the phone: the starting
/// window is already its colour.
pub fn splash_files(
    s: &Splash,
    png: &[u8],
    title: &str,
    colors: &[[String; 4]; 2],
    scheme: ColorScheme,
) -> Vec<(String, Vec<u8>)> {
    let six = |c: &str| match c.strip_prefix('#') {
        Some(h) if h.len() == 3 => h.chars().fold("#".to_string(), |s, x| format!("{s}{x}{x}")),
        _ => c.to_string(),
    };
    let [l, d] = colors;
    let json = aio::serde_json::json!({
        "min": s.min_ms,
        "max": s.max_ms,
        "scale": if s.retina() { 2 } else { 1 },
        "text": s.text.as_ref().map(|t| t.replace("{title}", title)),
        "font": s.font,
        "size": s.font_size.unwrap_or(14),
        "bold": s.bold,
        "fade": s.fade_ms.unwrap_or(200),
        "night": match scheme { ColorScheme::System => None, ColorScheme::Light => Some(false), ColorScheme::Dark => Some(true) },
        "light": [six(&l[0]), six(&l[1])],
        "dark": [six(&d[0]), six(&d[1])],
    });
    let color = |c: &str| {
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<resources>\n  <color name=\"aio_splash\">{}</color>\n</resources>\n",
            six(c)
        )
    };
    let theme = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<resources>\n  <style name=\"AioSplash\" parent=\"{}\">\n    \
         <item name=\"android:windowBackground\">@color/aio_splash</item>\n  </style>\n</resources>\n",
        system_theme(scheme)
    );
    let res = "app/src/main/res/";
    let mut files = vec![
        ("app/src/main/assets/aio-splash.png".to_string(), png.to_vec()),
        ("app/src/main/assets/aio-splash.json".into(), json.to_string().into_bytes()),
        (format!("{res}values/aio_splash_theme.xml"), theme.into_bytes()),
        (
            format!("{res}values/aio_splash_color.xml"),
            color(if scheme == ColorScheme::Dark { &d[0] } else { &l[0] }).into_bytes(),
        ),
    ];
    if scheme == ColorScheme::System {
        files.push((format!("{res}values-night/aio_splash_color.xml"), color(&d[0]).into_bytes()));
    }
    files
}

/// The manifest's lines for `cfg`: the camera's permission (its hardware
/// optional: the app still installs where there is none), the launch mode
/// and one VIEW filter per scheme (a link reaches the copy that is open),
/// and the theme that fixes light or dark (`splashed`: the splash's).
fn manifest_parts(cfg: &AndroidBuild, splashed: bool) -> [(&'static str, String); 4] {
    let camera = if cfg.camera {
        "\n  <uses-permission android:name=\"android.permission.CAMERA\" />\n  \
         <uses-feature android:name=\"android.hardware.camera\" android:required=\"false\" />"
    } else {
        ""
    };
    let mode = if cfg.schemes.is_empty() { "" } else { "\n      android:launchMode=\"singleTask\"" };
    let links: String = cfg
        .schemes
        .iter()
        .map(|s| {
            format!(
                "\n      <intent-filter>\n        <action android:name=\"android.intent.action.VIEW\" />\n        \
                 <category android:name=\"android.intent.category.DEFAULT\" />\n        \
                 <category android:name=\"android.intent.category.BROWSABLE\" />\n        \
                 <data android:scheme=\"{s}\" />\n      </intent-filter>"
            )
        })
        .collect();
    let theme = if splashed { SPLASH_THEME } else { system_theme(cfg.color_scheme) };
    [("{{CAMERA}}", camera.into()), ("{{LAUNCH_MODE}}", mode.into()), ("{{LINKS}}", links), ("{{THEME}}", theme.into())]
}

/// The template, filled in for `app` and its `[build.android]` (`cfg`), its
/// launcher icon (`icon`, PNG), its splash's files ([`splash_files`], or
/// none) and the app's `android/` files (`overlay`: path, bytes) over them:
/// each replaces a file whole or adds one, its placeholders filled too (text).
/// `kind`: the app, or a dev copy of it ([`Apk`]).
pub fn project(
    app: &App,
    port_of: Option<&str>,
    cfg: &AndroidBuild,
    icon: &[u8],
    splash: &[(String, Vec<u8>)],
    overlay: &[(String, Vec<u8>)],
    kind: Apk,
) -> Result<Vec<(String, Vec<u8>)>, String> {
    let page = kind == Apk::Page;
    let abis = ABIS.iter().map(|(a, _)| format!("\"{a}\"")).collect::<Vec<_>>().join(", ");
    let code = version_code(&app.version)?.to_string();
    let parts = manifest_parts(cfg, !splash.is_empty());
    let (suffix, label) = match kind != Apk::App {
        true => (".dev", format!("{} (dev)", crate::build::label(app))),
        false => ("", crate::build::label(app).to_string()),
    };
    let id = application_id_of(&app.id, port_of, cfg) + suffix;
    let shape = [
        ("{{ACTIVITY}}", if page { DEV } else { "aio.rt.MainActivity" }),
        // The dev page is http://localhost (a secure context, over adb).
        ("{{CLEARTEXT}}", if page { "true" } else { "false" }),
        ("{{SPLITS}}", if page { "false" } else { "true" }),
        ("{{UNIVERSAL}}", if cfg.universal { "true" } else { "false" }),
    ];
    let fill = |text: &str| {
        let text = text
            .replace("{{ID}}", &app.id)
            .replace("{{APPLICATION_ID}}", &id)
            .replace("{{VERSION_CODE}}", &code)
            .replace("{{VERSION}}", &app.version)
            .replace("{{ABIS}}", &abis)
            .replace("{{NAME}}", &label_xml(&label));
        let text = shape.iter().fold(text, |t, (k, v)| t.replace(k, v));
        parts.iter().fold(text, |t, (k, v)| t.replace(k, v))
    };
    let mut files: Vec<(String, Vec<u8>)> = TEMPLATE
        .iter()
        .chain(page.then_some(&DEV_ACTIVITY))
        .map(|(p, t)| (p.to_string(), fill(t).into_bytes()))
        .chain([(ICON.to_string(), icon.to_vec())])
        .chain(splash.iter().cloned())
        .filter(|(p, _)| !overlay.iter().any(|(o, _)| o == p))
        .collect();
    files.extend(
        overlay
            .iter()
            .map(|(p, b)| (p.clone(), std::str::from_utf8(b).map_or_else(|_| b.clone(), |t| fill(t).into_bytes()))),
    );
    Ok(files)
}

/// The app's `android/` files under `dir`: relative path (`/`), bytes.
pub fn overlay_of(dir: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    fn walk(root: &Path, rel: &str, out: &mut Vec<(String, Vec<u8>)>) -> std::io::Result<()> {
        for e in std::fs::read_dir(root.join(rel))? {
            let e = e?;
            let path = if rel.is_empty() {
                e.file_name().to_string_lossy().into_owned()
            } else {
                format!("{rel}/{}", e.file_name().to_string_lossy())
            };
            let kind = e.file_type()?;
            if kind.is_dir() {
                walk(root, &path, out)?;
            } else if kind.is_file() {
                out.push((path, std::fs::read(e.path())?));
            }
        }
        Ok(())
    }
    let mut out = vec![];
    if dir.is_dir() {
        walk(dir, "", &mut out).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    out.sort();
    Ok(out)
}

/// What the build knew of an overlaid template file: the template's hash,
/// the aio that shipped it, the overlay's hash then.
#[derive(Clone, Debug, PartialEq)]
pub struct Seen {
    pub template: String,
    pub aio: String,
    pub overlay: String,
}

/// The next record and the warnings, for overlay files that replace a
/// template file (`now`: path → template hash, overlay hash). A file seen
/// the first time is recorded; a template changed since warns, until the
/// overlay itself changes (it was re-based), which records the pair afresh.
pub fn drift(
    record: &BTreeMap<String, Seen>,
    now: &BTreeMap<String, (String, String)>,
    aio: &str,
) -> (BTreeMap<String, Seen>, Vec<String>) {
    let (mut next, mut warnings) = (BTreeMap::new(), vec![]);
    for (path, (template, overlay)) in now {
        let was = record.get(path);
        if let Some(w) = was.filter(|w| w.template != *template && w.overlay == *overlay) {
            warnings.push(format!(
                "android/{path} overlays a template that changed since it was copied (aio {} → {aio}): what aio \
                 fixed in that file since does not reach this APK. Re-base it on aio's template (this warning \
                 stops once the overlay changes), or delete it to use the template.",
                w.aio
            ));
            next.insert(path.clone(), w.clone());
            continue;
        }
        let aio = was.filter(|w| w.template == *template).map_or(aio, |w| w.aio.as_str());
        next.insert(path.clone(), Seen { template: template.clone(), aio: aio.into(), overlay: overlay.clone() });
    }
    (next, warnings)
}

/// Where the build records what overlays replaced, in the app's dir.
pub const OVERLAY_RECORD: &str = ".aio/android-overlay.json";

/// Compare `overlay` with the record in `app_dir`, write it back; the warnings.
fn check_drift(app_dir: &Path, overlay: &[(String, Vec<u8>)]) -> Vec<String> {
    let hash = |b: &[u8]| aio::sha256::hex(&aio::sha256::Sha256::digest(b));
    let now: BTreeMap<String, (String, String)> = overlay
        .iter()
        .filter_map(|(p, b)| {
            TEMPLATE.iter().find(|(t, _)| t == p).map(|(_, t)| (p.clone(), (hash(t.as_bytes()), hash(b))))
        })
        .collect();
    let file = app_dir.join(OVERLAY_RECORD);
    let old: serde_json::Value =
        std::fs::read(&file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let s = |v: &serde_json::Value, k: &str| v[k].as_str().unwrap_or_default().to_string();
    let record: BTreeMap<String, Seen> = old
        .as_object()
        .into_iter()
        .flatten()
        .map(|(p, v)| (p.clone(), Seen { template: s(v, "template"), aio: s(v, "aio"), overlay: s(v, "overlay") }))
        .collect();
    let (next, warnings) = drift(&record, &now, env!("CARGO_PKG_VERSION"));
    if next != record {
        let json: serde_json::Map<String, serde_json::Value> = next
            .iter()
            .map(|(p, w)| {
                (p.clone(), serde_json::json!({ "template": w.template, "aio": w.aio, "overlay": w.overlay }))
            })
            .collect();
        _ = std::fs::create_dir_all(app_dir.join(".aio"));
        _ = std::fs::write(&file, serde_json::to_string_pretty(&json).unwrap_or_default() + "\n");
    }
    warnings
}

fn env_dir(k: &str) -> Option<PathBuf> {
    std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The user's home: `HOME`, else `USERPROFILE` (Windows).
fn home() -> PathBuf {
    env_dir("HOME").or_else(|| env_dir("USERPROFILE")).unwrap_or_default()
}

/// `name` as a host running `os` starts it (`.exe`).
fn exe(name: &str, os: &str) -> String {
    if os == "windows" { format!("{name}.exe") } else { name.to_string() }
}

/// A build-tools script (`apksigner`) as a host running `os` starts it (`.bat`).
fn script(name: &str, os: &str) -> String {
    if os == "windows" { format!("{name}.bat") } else { name.to_string() }
}

/// The newest dir in `d` (by name).
fn newest(d: &Path) -> Option<PathBuf> {
    let mut all: Vec<PathBuf> = std::fs::read_dir(d).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    all.sort();
    all.pop()
}

/// Where the SDK may be, in order (aio's `resolveSdk`): `ANDROID_HOME`,
/// `ANDROID_SDK_ROOT` (each the SDK or its parent), then each OS's default
/// (pure).
pub fn sdk_places(named: &[Option<PathBuf>], home: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> =
        named.iter().flatten().flat_map(|d| [d.clone(), d.join("Sdk"), d.join("sdk")]).collect();
    out.extend([
        home.join("Android").join("Sdk"),
        home.join("Android").join("sdk"),
        home.join("Library").join("Android").join("sdk"),
        home.join("AppData").join("Local").join("Android").join("Sdk"),
    ]);
    out
}

/// The SDK: the first of [`sdk_places`] with its platforms.
fn sdk() -> Result<PathBuf, String> {
    sdk_places(&[env_dir("ANDROID_HOME"), env_dir("ANDROID_SDK_ROOT")], &home())
        .into_iter()
        .find(|d| d.join("platforms").is_dir())
        .ok_or_else(|| "no Android SDK: install it and set ANDROID_HOME".into())
}

/// The SDK `adb` and the emulator are in (its `platform-tools`).
pub fn sdk_dir() -> Option<PathBuf> {
    sdk_places(&[env_dir("ANDROID_HOME"), env_dir("ANDROID_SDK_ROOT")], &home())
        .into_iter()
        .find(|d| d.join("platform-tools").is_dir())
}

/// The JDKs Gradle runs on (aio's `GRADLE_MIN_JDK`, `GRADLE_MAX_JDK`) and
/// the newest preferred (AGP's LTS range).
const JDK_MIN: u32 = 17;
const JDK_MAX: u32 = 23;
const JDK_PREFERRED: u32 = 21;

/// Where JDKs live on a host running `os` (aio's `jdkPlaces`): roots of one
/// JDK per folder, and homes that are JDKs, Android Studio's own included
/// (pure).
pub fn jdk_places(os: &str, home: &Path, env: impl Fn(&str) -> Option<PathBuf>) -> (Vec<PathBuf>, Vec<PathBuf>) {
    if os == "windows" {
        let mut pf: Vec<PathBuf> = ["ProgramFiles", "ProgramW6432"].iter().filter_map(|k| env(k)).collect();
        pf.dedup();
        let vendors = ["Java", "Eclipse Adoptium", "Microsoft", "Zulu", "Amazon Corretto", "BellSoft"];
        let roots = pf.iter().flat_map(|p| vendors.map(|v| p.join(v))).collect();
        let mut homes: Vec<PathBuf> = pf.iter().map(|p| p.join("Android").join("Android Studio").join("jbr")).collect();
        homes.extend(env("LOCALAPPDATA").map(|l| l.join("Programs").join("Android Studio").join("jbr")));
        return (roots, homes);
    }
    let roots =
        ["/usr/lib/jvm", "/usr/java", "/Library/Java/JavaVirtualMachines", "/opt/homebrew/opt", "/usr/local/opt"]
            .map(PathBuf::from)
            .into_iter()
            .chain([home.join(".sdkman/candidates/java")])
            .collect();
    let homes = [
        PathBuf::from("/opt/android-studio/jbr"),
        home.join("android-studio/jbr"),
        PathBuf::from("/Applications/Android Studio.app/Contents/jbr/Contents/Home"),
        home.join(".local/share/JetBrains/Toolbox/apps/AndroidStudio/ch-0/jbr"),
    ]
    .into();
    (roots, homes)
}

/// The Java major `javac -version` says (`javac 21.0.4` → 21, `javac
/// 1.8.0_402` → 8), pure.
pub fn javac_major(said: &str) -> Option<u32> {
    let v = said.lines().find_map(|l| l.trim().strip_prefix("javac "))?;
    let mut n = v.split(['.', '_', '-', '+']).map(|p| p.parse::<u32>());
    match n.next()?.ok()? {
        1 => n.next()?.ok(),
        m => Some(m),
    }
}

/// The JDK to hand Gradle among `found` (home, major), each a working
/// `javac` (aio's `findJdk`): `JAVA_HOME` when it is one Gradle runs on,
/// else the newest up to 21, else the newest Gradle runs on (pure).
pub fn pick_jdk(found: &[(PathBuf, u32)], java_home: Option<&Path>) -> Option<(PathBuf, u32)> {
    let usable: Vec<&(PathBuf, u32)> = found.iter().filter(|(_, m)| (JDK_MIN..=JDK_MAX).contains(m)).collect();
    if let Some(j) = usable.iter().find(|(h, _)| Some(h.as_path()) == java_home) {
        return Some((*j).clone());
    }
    let best = |max: u32| usable.iter().filter(|(_, m)| *m <= max).max_by_key(|(_, m)| *m).map(|j| (*j).clone());
    best(JDK_PREFERRED).or_else(|| best(JDK_MAX))
}

/// A JDK Gradle runs on: each candidate's `javac` found (symlinks followed
/// to the real JDK) and asked its version, so a JRE or a broken install is
/// never picked.
fn jdk() -> Result<PathBuf, String> {
    let os = host_os();
    let javac = exe("javac", os);
    let (roots, homes) = jdk_places(os, &home(), env_dir);
    let mut dirs: Vec<PathBuf> = env_dir("JAVA_HOME").into_iter().collect();
    for r in roots {
        for e in std::fs::read_dir(r).into_iter().flatten().flatten() {
            let p = e.path();
            dirs.extend([p.join("Contents/Home"), p.join("libexec/openjdk.jdk/Contents/Home"), p]);
        }
    }
    dirs.extend(homes);
    let mut found: Vec<(PathBuf, u32)> = vec![];
    for d in dirs {
        let Ok(real) = std::fs::canonicalize(d.join("bin").join(&javac)) else { continue };
        let Some(home) = real.parent().and_then(Path::parent).map(Path::to_path_buf) else { continue };
        if found.iter().any(|(h, _)| *h == home) {
            continue;
        }
        let Ok(o) = Command::new(&real).arg("-version").output() else { continue };
        let said = String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr);
        if let Some(m) = javac_major(&said).filter(|_| o.status.success()) {
            found.push((home, m));
        }
    }
    let java_home = env_dir("JAVA_HOME").and_then(|j| std::fs::canonicalize(j).ok());
    pick_jdk(&found, java_home.as_deref()).map(|(h, _)| h).ok_or_else(|| {
        let seen: Vec<String> = found.iter().map(|(h, m)| format!("{} ({m})", h.display())).collect();
        format!(
            "no JDK {JDK_MIN} to {JDK_MAX} (Gradle runs on those): install one or set JAVA_HOME{}",
            if seen.is_empty() { String::new() } else { format!("; found {}", seen.join(", ")) }
        )
    })
}

/// The Gradle the build runs: the wrapper pinned to this version (aio's
/// pin: reproducible, and AGP 8.7's), written once by any Gradle 8.9+.
const GRADLE: &str = "8.14.3";

/// A Gradle that writes the wrapper: 8.9 or newer (9 too), pure.
pub fn gradle_ok(v: &str) -> bool {
    let mut n = v.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    (n.next().unwrap_or(0), n.next().unwrap_or(0)) >= (8, 9)
}

/// A Gradle 8.9+: on `PATH` (`gradle.bat` on Windows), else one a wrapper
/// fetched into `~/.gradle`.
fn gradle(os: &str) -> Result<PathBuf, String> {
    let names: &[&str] = if os == "windows" { &["gradle.bat", "gradle"] } else { &["gradle"] };
    for n in names {
        let Ok(o) = Command::new(n).arg("--version").output() else { continue };
        let text = String::from_utf8_lossy(&o.stdout).into_owned();
        if text.lines().find_map(|l| l.strip_prefix("Gradle ")).is_some_and(|v| gradle_ok(v.trim())) {
            return Ok(PathBuf::from(n));
        }
    }
    let dists = env_dir("GRADLE_USER_HOME").unwrap_or_else(|| home().join(".gradle")).join("wrapper/dists");
    let mut found: Vec<(Vec<u32>, PathBuf)> = std::fs::read_dir(&dists)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let v = name.strip_prefix("gradle-")?;
            let v = v.strip_suffix("-bin").or_else(|| v.strip_suffix("-all"))?.to_string();
            let bin = newest(&e.path())?.join(format!("gradle-{v}")).join("bin").join(script("gradle", os));
            (gradle_ok(&v) && bin.exists()).then(|| (v.split('.').map(|p| p.parse().unwrap_or(0)).collect(), bin))
        })
        .collect();
    found.sort();
    found.pop().map(|(_, b)| b).ok_or_else(|| "no Gradle 8.9+ (on PATH or in ~/.gradle/wrapper): install it".into())
}

/// The Gradle wrapper pinned to [`GRADLE`] (`gradlew`, `gradlew.bat`,
/// `gradle/wrapper/`), written once into `<target>/aio/gradle-wrapper-…`
/// in a folder of its own (no build there for a newer Gradle to refuse).
fn wrapper(target_dir: &Path, jdk: &Path, os: &str) -> Result<PathBuf, String> {
    let dir = target_dir.join("aio").join(format!("gradle-wrapper-{GRADLE}"));
    if dir.join("gradle/wrapper/gradle-wrapper.jar").is_file() {
        return Ok(dir);
    }
    let tmp = dir.with_extension("making");
    _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::write(tmp.join("settings.gradle.kts"), "").map_err(|e| e.to_string())?;
    crate::freeze::ensure("gradle")?;
    run(
        Command::new(gradle(os)?)
            .args(["-q", "--no-daemon", "wrapper", "--gradle-version", GRADLE, "-p"])
            .arg(&tmp)
            .env("JAVA_HOME", jdk),
        "gradle wrapper",
    )?;
    _ = std::fs::remove_dir_all(&dir);
    std::fs::rename(&tmp, &dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(dir)
}

/// Gradle's run of `tasks` on project `proj`, by the pinned wrapper.
fn gradle_run(proj: &Path, target_dir: &Path, jdk: &Path, tasks: &[&str]) -> Result<(), String> {
    let os = host_os();
    let w = wrapper(target_dir, jdk, os)?;
    for f in ["gradlew", "gradlew.bat", "gradle/wrapper/gradle-wrapper.jar", "gradle/wrapper/gradle-wrapper.properties"]
    {
        let to = proj.join(f);
        std::fs::create_dir_all(to.parent().ok_or("bad path")?).map_err(|e| e.to_string())?;
        std::fs::copy(w.join(f), &to).map_err(|e| format!("{}: {e}", w.join(f).display()))?;
    }
    let gradlew = proj.join(if os == "windows" { "gradlew.bat" } else { "gradlew" });
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        _ = std::fs::set_permissions(&gradlew, std::fs::Permissions::from_mode(0o755));
    }
    run(
        Command::new(gradlew)
            .args(["-q", "--no-daemon", &format!("--max-workers={}", crate::freeze::max_jobs()), "-p"])
            .arg(proj)
            .args(tasks)
            .env("JAVA_HOME", jdk),
        "gradle",
    )
}

/// This machine's OS as the Android tools name their folders.
fn host_os() -> &'static str {
    std::env::consts::OS
}

/// aio's signing key for app `id`: made once (keytool), kept in the keys dir.
fn key(id: &str, jdk: &Path) -> Result<Signing, String> {
    let dir = crate::release::keys_dir();
    aio::paths::private_dir(&dir).map_err(|e| e.to_string())?;
    let ks = dir.join(format!("{id}.android.p12"));
    let pass_file = dir.join(format!("{id}.android.pass"));
    if !ks.exists() {
        let pass = format!("{:032x}", u128::from_le_bytes(aio::random_bytes::<16>()));
        aio::paths::write_atomic(&pass_file, pass.as_bytes(), true).map_err(|e| e.to_string())?;
        let st = Command::new(jdk.join("bin").join(exe("keytool", host_os())))
            .args([
                "-genkeypair",
                "-storetype",
                "PKCS12",
                "-keyalg",
                "EC",
                "-groupname",
                "secp256r1",
                "-validity",
                "36500",
            ])
            .args(["-alias", id, "-dname", &format!("CN={id}"), "-storepass", &pass, "-keystore"])
            .arg(&ks)
            .output()
            .map_err(|e| format!("keytool: {e}"))?;
        if !st.status.success() {
            return Err(format!("keytool: {}", String::from_utf8_lossy(&st.stderr).trim()));
        }
        eprintln!("ax build: made the Android signing key {} (back it up: updates need it)", ks.display());
    }
    Ok(Signing { keystore: ks, alias: id.to_string(), pass: Pass::File(pass_file), key_pass: None })
}

/// The key aio's APKs are signed with when no keystore is named (aio's
/// default build is a debug one): Android's debug key of this machine,
/// `~/.android/debug.keystore` (`ANDROID_USER_HOME` moves it). A port signs
/// with it so that it installs over the aio APK (Android updates an app
/// only from the same key).
pub fn aio_key(user_home: Option<PathBuf>, home: &Path) -> Option<Signing> {
    let ks = user_home.unwrap_or_else(|| home.join(".android")).join("debug.keystore");
    ks.is_file().then(|| Signing {
        keystore: ks,
        alias: "androiddebugkey".into(),
        pass: Pass::Plain("android"),
        key_pass: None,
    })
}

fn run(cmd: &mut Command, what: &str) -> Result<(), String> {
    let st = cmd.status().map_err(|e| format!("{what}: {e}"))?;
    if st.success() { Ok(()) } else { Err(format!("{what} failed")) }
}

/// Which APKs a build makes: a release (signed with the app's key, made
/// small) or a debug one (`--debug`: debuggable, signed with Android's debug
/// key, its page in `chrome://inspect`), for each CPU type of `abis`.
#[derive(Clone, Copy, Debug)]
pub struct Ask<'a> {
    pub debug: bool,
    pub abis: &'a [(&'a str, &'a str)],
    /// The app, or its dev copy beside it ([`Apk::Dev`]).
    pub kind: Apk,
}

impl Ask<'_> {
    /// The app's release APKs, one per CPU type.
    pub const RELEASE: Ask<'static> = Ask { debug: false, abis: &ABIS, kind: Apk::App };
}

/// The app's library: its package, its lib target, the target dir.
pub struct Lib<'a> {
    pub pkg: &'a str,
    pub lib: &'a str,
    pub target_dir: &'a Path,
}

/// What an app's own `MainActivity.java` (an `android/` file) lacks of the
/// template's, each with what it costs (aio's `ownActivityWarnings`), pure.
pub fn activity_losses(own: &str, splash: bool) -> Vec<String> {
    let mut out = vec![];
    let mut lost =
        |what: &str, cost: &str| out.push(format!("android/{}: it drops the template's {what}: {cost}", TEMPLATE[6].0));
    if !own.contains("setOnApplyWindowInsetsListener") {
        lost("insets frame", "on Android 15 the page draws under the status and navigation bars");
    } else if own.contains("WindowInsets.Type") && !own.contains("SDK_INT >= 30") {
        lost("Android 10 insets path", "WindowInsets.Type is API 30: the app crashes at start on Android 10");
    }
    if !own.contains("onRenderProcessGone") {
        lost("renderer-loss recovery", "a page renderer the system takes back for memory aborts the app");
    }
    if splash && !own.contains("aio-splash.json") {
        lost("splash", "[build.splash] is not shown");
    }
    out
}

/// The app's icon, splash and `android/` files, for its project.
fn parts(
    app: &App,
    full: &aio::config::Config,
    app_dir: &Path,
) -> Result<(Vec<u8>, Vec<(String, Vec<u8>)>, Vec<(String, Vec<u8>)>), String> {
    let cfg = &full.build.android;
    let splash = match &full.build.splash {
        Some(s) => {
            let p = app_dir.join(&s.image);
            let png = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            let hue = full.ui.hue.unwrap_or_else(|| aio::colors::hue(&app.id));
            let bg = s.background.as_ref().map(|[l, d]| [l.as_str(), d.as_str()]);
            let colors = aio_shell::splash::colors(hue, bg);
            splash_files(s, &png, crate::build::label(app), &colors, cfg.color_scheme)
        }
        None => vec![],
    };
    let overlay = overlay_of(&app_dir.join("android"))?;
    let icon = match &app.icon {
        Some(p) => std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?,
        None => crate::icon::icon_png(crate::build::label(app), &app.id, 192),
    };
    Ok((icon, splash, overlay))
}

/// Write `files` under `proj` (made afresh) with the SDK's place, warning
/// of each placeholder left unfilled.
fn write_project(proj: &Path, files: &[(String, Vec<u8>)], sdk: &Path) -> Result<(), String> {
    _ = std::fs::remove_dir_all(proj);
    for (path, bytes) in files {
        if let Ok(text) = std::str::from_utf8(bytes) {
            for t in leftover_tokens(text) {
                eprintln!("ax build: {path}: {t} is not a placeholder aio fills: it is left as written (a typo?)");
            }
        }
        let p = proj.join(path);
        std::fs::create_dir_all(p.parent().ok_or("bad path")?).map_err(|e| e.to_string())?;
        std::fs::write(&p, bytes).map_err(|e| e.to_string())?;
    }
    // Gradle reads a backslash as an escape: the path with `/` on Windows too.
    let dir = sdk.display().to_string().replace('\\', "/");
    std::fs::write(proj.join("local.properties"), format!("sdk.dir={dir}\n")).map_err(|e| e.to_string())
}

/// Build the APKs of the app's library `lib` into `dist` as `ask` says, and
/// the bundle when its `[build.android]` asks (a release); `app_dir` holds
/// its `aio.toml`.
pub fn build(
    app: &App,
    full: &aio::config::Config,
    app_dir: &Path,
    lib: &Lib,
    dist: &Path,
    ask: Ask,
) -> Result<Vec<PathBuf>, String> {
    let cfg = &full.build.android;
    let port_of = full.app.port_of.as_deref();
    let given = signing(|k| std::env::var(k).ok(), cfg, app_dir)?;
    let (icon, splash, overlay) = parts(app, full, app_dir)?;
    let missing = crate::build::missing_targets(&ask.abis.iter().map(|(_, t)| *t).collect::<Vec<_>>());
    if !missing.is_empty() {
        return Err(format!("Rust's Android targets are not installed: rustup target add {}", missing.join(" ")));
    }
    if !overlay.is_empty() {
        let paths: Vec<&str> = overlay.iter().map(|(p, _)| p.as_str()).collect();
        eprintln!("ax build: {} files from android/ over the template: {}", paths.len(), paths.join(", "));
    }
    let mut warnings = check_drift(app_dir, &overlay);
    if let Some((_, own)) = overlay.iter().find(|(p, _)| p == TEMPLATE[6].0) {
        warnings.extend(activity_losses(&String::from_utf8_lossy(own), !splash.is_empty()));
    }
    if cfg.strict && !warnings.is_empty() {
        return Err(format!("[build.android] strict: {}", warnings.join("\n  ")));
    }
    for w in &warnings {
        eprintln!("ax build: {w}");
    }
    let os = host_os();
    let sdk = sdk()?;
    let ndk = newest(&sdk.join("ndk")).ok_or("no Android NDK in the SDK: install one (sdkmanager \"ndk;…\")")?;
    let bin = ndk.join("toolchains/llvm/prebuilt").join(prebuilt(os)).join("bin");
    let proj = dist.join(format!(".{}-android", app.id));
    let made = (|| {
        write_project(&proj, &project(app, port_of, cfg, &icon, &splash, &overlay, ask.kind)?, &sdk)?;
        for (abi, target) in ask.abis {
            eprintln!("ax build: {} for {abi}…", app.name);
            let clang = bin.join(clang(target, os));
            let env = target.replace('-', "_");
            let step = format!("the {abi} library");
            let mut cargo = crate::freeze::cargo(
                &step,
                &["rustc", "--release", "--lib", "--crate-type", "cdylib", "-p", lib.pkg, "--target", target],
            )?;
            crate::build::remap(&mut cargo, app_dir);
            run(
                cargo
                    // The app's version (`aio::app_version!()`), the APK's as on desktop.
                    .env("AIORT_APP_VERSION", &app.version)
                    .arg("--")
                    .args(LINK)
                    .env(format!("CC_{env}"), &clang)
                    .env(format!("AR_{env}"), bin.join(exe("llvm-ar", os)))
                    .env(format!("CARGO_TARGET_{}_LINKER", env.to_uppercase()), &clang),
                &step,
            )?;
            let so = lib.target_dir.join(target).join("release").join(format!("lib{}.so", lib.lib));
            crate::build::no_home_paths(&std::fs::read(&so).map_err(|e| format!("{}: {e}", so.display()))?, &so)?;
            let to = proj.join("app/src/main/jniLibs").join(abi).join("libapp.so");
            std::fs::create_dir_all(to.parent().ok_or("bad path")?).map_err(|e| e.to_string())?;
            std::fs::copy(&so, &to).map_err(|e| format!("{}: {e}", so.display()))?;
        }
        let jdk = jdk()?;
        let aab = cfg.aab && !ask.debug;
        let task = if ask.debug { "assembleDebug" } else { "assembleRelease" };
        gradle_run(
            &proj,
            lib.target_dir,
            &jdk,
            &[task].into_iter().chain(aab.then_some("bundleRelease")).collect::<Vec<_>>(),
        )?;
        let base = format!("{}-{}-android", app.id, app.version);
        let mut outs: Vec<(String, String)> =
            ask.abis.iter().map(|(abi, _)| (abi.to_string(), format!("-{abi}"))).collect();
        if cfg.universal {
            outs.push(("universal".into(), "-universal".into()));
        }
        let mut apks = vec![];
        if ask.debug {
            // Signed by Gradle with Android's debug key (aio's debug build).
            let out_dir = proj.join("app/build/outputs/apk/debug");
            for (split, tail) in &outs {
                let dev = if ask.kind == Apk::Dev { "-dev" } else { "" };
                let apk = dist.join(format!("{base}{tail}{dev}-debug.apk"));
                let from = out_dir.join(format!("app-{split}-debug.apk"));
                std::fs::copy(&from, &apk).map_err(|e| format!("{}: {e}", from.display()))?;
                apks.push(apk);
            }
            return Ok(apks);
        }
        let tools = newest(&sdk.join("build-tools")).ok_or("no Android build-tools in the SDK")?;
        let sign = match (given, port_of) {
            (Some(s), _) => s,
            (None, Some(aio)) => match aio_key(env_dir("ANDROID_USER_HOME"), &home()) {
                Some(s) => {
                    eprintln!(
                        "ax build: signed with this machine's Android debug key, as aio signs {aio}'s APK: it installs over it"
                    );
                    s
                }
                None => {
                    eprintln!(
                        "ax build: no ~/.android/debug.keystore here (aio's key for {aio}'s APK): signed with aio-rs's own; \
                         a phone with the aio APK refuses it until that one is uninstalled (its data with it)"
                    );
                    key(&app.id, &jdk)?
                }
            },
            (None, None) => key(&app.id, &jdk)?,
        };
        let out_dir = proj.join("app/build/outputs/apk/release");
        let path = std::env::join_paths(
            std::iter::once(jdk.join("bin")).chain(std::env::var_os("PATH").iter().flat_map(std::env::split_paths)),
        )
        .map_err(|e| e.to_string())?;
        for (split, tail) in &outs {
            let unsigned = out_dir.join(format!("app-{split}-release-unsigned.apk"));
            let aligned = out_dir.join(format!("app-{split}-aligned.apk"));
            run(
                Command::new(tools.join(exe("zipalign", os))).args(["-f", "-p", "4"]).arg(&unsigned).arg(&aligned),
                "zipalign",
            )?;
            let apk = dist.join(format!("{base}{tail}.apk"));
            run(
                Command::new(tools.join(script("apksigner", os)))
                    .args(["sign", "--ks"])
                    .arg(&sign.keystore)
                    .args(["--ks-key-alias", &sign.alias, "--ks-pass", &sign.pass.apksigner()])
                    .args(sign.key_pass.iter().flat_map(|p| ["--key-pass".to_string(), p.apksigner()]))
                    .arg("--out")
                    .arg(&apk)
                    .arg(&aligned)
                    .env("JAVA_HOME", &jdk)
                    .env("PATH", &path),
                "apksigner",
            )?;
            _ = std::fs::remove_file(apk.with_extension("apk.idsig"));
            let bytes = std::fs::metadata(&apk).map_err(|e| e.to_string())?.len();
            // The budget is each CPU's; the universal APK is all of them.
            if let Some(why) = over_budget(bytes, BUDGET_MB).filter(|_| split != "universal") {
                return Err(format!("{}: {why}", apk.display()));
            }
            apks.push(apk);
        }
        if aab {
            let aab = dist.join(format!("{base}.aab"));
            sign_aab(&jdk, &sign, &proj.join("app/build/outputs/bundle/release/app-release.aab"), &aab)?;
            apks.push(aab);
        }
        Ok(apks)
    })();
    _ = std::fs::remove_dir_all(&proj);
    made
}

/// The dev APK of `app` (`ax dev --android`, [`project`]'s `dev`): made
/// once per template and app look (kept in `<target>/aio/android-dev/`),
/// as the app's edits reach it through the page. Its path and its hash.
pub fn dev_apk(
    app: &App,
    full: &aio::config::Config,
    app_dir: &Path,
    target_dir: &Path,
) -> Result<(PathBuf, String), String> {
    let (icon, _, overlay) = parts(app, full, app_dir)?;
    let files = project(app, full.app.port_of.as_deref(), &full.build.android, &icon, &[], &overlay, Apk::Page)?;
    let mut all = vec![];
    for (p, b) in &files {
        all.extend_from_slice(p.as_bytes());
        all.extend_from_slice(b);
    }
    let hash = aio::sha256::hex(&aio::sha256::Sha256::digest(&all))[..16].to_string();
    let dir = target_dir.join("aio").join("android-dev");
    let apk = dir.join(format!("{}-{hash}.apk", app.id));
    if apk.is_file() {
        return Ok((apk, hash));
    }
    let sdk = sdk()?;
    let proj = dir.join(&app.id);
    eprintln!("ax dev: the dev APK of {} (once: edits reach it through the page)…", app.name);
    write_project(&proj, &files, &sdk)?;
    let jdk = jdk()?;
    let made = gradle_run(&proj, target_dir, &jdk, &["assembleDebug"]).and_then(|()| {
        let from = proj.join("app/build/outputs/apk/debug/app-debug.apk");
        std::fs::copy(&from, &apk).map_err(|e| format!("{}: {e}", from.display()))
    });
    _ = std::fs::remove_dir_all(&proj);
    made.map(|_| (apk, hash))
}

/// Gradle's `unsigned` App Bundle signed with the release key into `out`,
/// as Play wants it (jarsigner: an `.aab` is a signed jar).
fn sign_aab(jdk: &Path, sign: &Signing, unsigned: &Path, out: &Path) -> Result<(), String> {
    run(
        Command::new(jdk.join("bin").join(exe("jarsigner", host_os())))
            .args(["-keystore"])
            .arg(&sign.keystore)
            .args(sign.pass.jarsigner("-storepass"))
            .args(sign.key_pass.iter().flat_map(|p| p.jarsigner("-keypass")))
            .arg("-signedjar")
            .arg(out)
            .arg(unsigned)
            .arg(&sign.alias),
        "jarsigner",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn the_app_bundle_is_signed_with_the_release_key_and_verifies() {
        let d = tempfile::tempdir().unwrap();
        let jdk = jdk().unwrap();
        let (ks, pass) = (d.path().join("release.p12"), d.path().join("pass"));
        std::fs::write(&pass, "throwaway-pass").unwrap();
        let st = Command::new(jdk.join("bin/keytool"))
            .args(["-genkeypair", "-storetype", "PKCS12", "-keyalg", "EC", "-groupname", "secp256r1"])
            .args(["-alias", "up", "-dname", "CN=aio throwaway", "-validity", "2", "-storepass", "throwaway-pass"])
            .arg("-keystore")
            .arg(&ks)
            .output()
            .unwrap();
        assert!(st.status.success(), "{}", String::from_utf8_lossy(&st.stderr));
        // What Gradle's bundleRelease writes: a zip with the bundle's parts.
        let unsigned = d.path().join("app-release.aab");
        let mut z = zip::ZipWriter::new(std::fs::File::create(&unsigned).unwrap());
        for (f, b) in [("BundleConfig.pb", &b"\x0a\x00"[..]), ("base/manifest/AndroidManifest.xml", b"<manifest/>")] {
            z.start_file(f, zip::write::SimpleFileOptions::default()).unwrap();
            std::io::Write::write_all(&mut z, b).unwrap();
        }
        z.finish().unwrap();
        let sign = Signing { keystore: ks, alias: "up".into(), pass: Pass::File(pass), key_pass: None };
        let aab = d.path().join("notes-1.2.0-android.aab");
        sign_aab(&jdk, &sign, &unsigned, &aab).unwrap();
        let out = |tool: &str, args: &[&str]| {
            let o = Command::new(jdk.join("bin").join(tool)).args(args).arg(&aab).output().unwrap();
            String::from_utf8_lossy(&o.stdout).into_owned()
        };
        assert!(out("jarsigner", &["-verify"]).contains("jar verified"));
        assert!(out("keytool", &["-printcert", "-jarfile"]).contains("CN=aio throwaway"));
        assert!(!out("jarsigner", &["-verify"]).contains("unsigned"), "every entry is signed");
        let e = sign_aab(&jdk, &Signing { alias: "nope".into(), ..sign }, &unsigned, &aab).unwrap_err();
        assert!(e.starts_with("jarsigner"), "a wrong key fails the build: {e}");
    }

    #[test]
    fn every_native_the_activity_calls_is_in_the_apps_library_and_back_asks_it_first() {
        let (host, natives) = (TEMPLATE[5].1, include_str!("../../aio-shell/src/android.rs"));
        let calls: Vec<&str> = host
            .lines()
            .filter_map(|l| l.trim().strip_prefix("public static native ")?.split(['(', ' ']).nth(1))
            .collect();
        assert_eq!(calls, ["start", "connect", "fromPage", "back", "link", "allows", "answer", "stop"]);
        for c in calls {
            assert!(natives.contains(&format!("fn Java_aio_rt_Host_{c}<")), "{c} has no entry point");
        }
        let activity = TEMPLATE[6].1;
        let back = &activity[activity.find("public void onBackPressed()").expect("Back is handled")..];
        assert!(back[..back.find("\n    }").unwrap()].contains("Host.back();"), "the app is asked first");
        assert!(natives.contains("\"notTaken\", \"()V\"") && host.contains("static void notTaken()"));
    }

    /// The body of the Java method that starts at `head`, to its closing brace.
    fn method<'a>(java: &'a str, head: &str) -> &'a str {
        let at = java.find(head).unwrap_or_else(|| panic!("no {head}"));
        let indent = java[..at].rsplit('\n').next().unwrap_or("");
        let end = java[at..].find(&format!("\n{indent}}}")).unwrap_or_else(|| panic!("{head} never ends"));
        &java[at..at + end]
    }

    #[test]
    fn the_manifest_opens_the_apps_schemes_declares_the_camera_and_fixes_the_theme_only_when_asked() {
        // aio tests/android-deep-links.test.ts, build-android-camera.test.ts.
        let app = App { id: "notes".into(), name: "Notes".into(), version: "1.2.3".into(), icon: None };
        let manifest = |cfg: &AndroidBuild| {
            let files = project(&app, None, cfg, b"png", &[], &[], Apk::App).unwrap();
            let m = files.into_iter().find(|(p, _)| p == "app/src/main/AndroidManifest.xml").unwrap().1;
            String::from_utf8(m).unwrap()
        };
        let none = manifest(&AndroidBuild::default());
        for absent in ["action.VIEW", "launchMode", "CAMERA", "uses-feature", "{{"] {
            assert!(!none.contains(absent), "{absent} in {none}");
        }
        assert!(
            none.contains("android:theme=\"@android:style/Theme.DeviceDefault.DayNight\""),
            "the phone's light or dark"
        );
        let cfg = AndroidBuild {
            schemes: vec!["myapp".into(), "other".into()],
            camera: true,
            color_scheme: ColorScheme::Dark,
            ..Default::default()
        };
        let all = manifest(&cfg);
        assert_eq!(all.matches("android.intent.action.VIEW").count(), 2, "{all}");
        assert!(all.contains("<data android:scheme=\"myapp\" />") && all.contains("<data android:scheme=\"other\" />"));
        assert_eq!(all.matches("android.intent.category.BROWSABLE").count(), 2, "a link in a browser opens it");
        assert!(all.contains("android:launchMode=\"singleTask\""), "a link reaches the copy that is open");
        assert!(all.contains("<uses-permission android:name=\"android.permission.CAMERA\" />"));
        assert!(
            all.contains("android:name=\"android.hardware.camera\" android:required=\"false\""),
            "it still installs where there is no camera"
        );
        assert!(all.contains("@android:style/Theme.DeviceDefault\""), "dark whatever the phone says");
        let light = manifest(&AndroidBuild { color_scheme: ColorScheme::Light, ..Default::default() });
        assert!(light.contains("@android:style/Theme.DeviceDefault.Light\""));
        assert!(!all.contains("{{") && !light.contains("{{"));
    }

    #[test]
    fn the_activity_answers_every_webview_gap_out_loud() {
        // aio tests/android-webview-gaps.test.ts, android-config-changes-font-scale.test.ts.
        let java = TEMPLATE[6].1;
        // A lost renderer rebuilds the activity once; a loss again at once stops the app.
        let gone = method(java, "public boolean onRenderProcessGone(");
        let (guard, destroy, rebuild) = (
            gone.find("if (loop) return false").unwrap(),
            gone.find("v.destroy()").unwrap(),
            gone.find("recreate()").unwrap(),
        );
        assert!(guard < destroy && destroy < rebuild && gone[rebuild..].contains("return true"), "{gone}");
        assert!(gone.contains("now - lastRendererLoss < RENDERER_LOSS_LOOP_MS") && gone.contains("d.didCrash()"));
        assert!(java.contains("private static long lastRendererLoss;"), "its clock outlives the activity");
        // The page's file input: answered exactly once on every path.
        let chooser = method(java, "public boolean onShowFileChooser(");
        assert!(
            chooser.find("pendingFiles.onReceiveValue(null)").unwrap()
                < chooser.find("pendingFiles = callback").unwrap()
        );
        let none = &chooser[chooser.find("catch (ActivityNotFoundException e)").unwrap()..];
        assert!(none.contains("callback.onReceiveValue(null)") && none.contains("Log.e(\"aio\""));
        let result = method(java, "protected void onActivityResult(");
        assert!(
            result.contains("pendingFiles.onReceiveValue(uris.isEmpty() ? null")
                && result.contains("pendingFiles = null")
        );
        assert!(result.contains("getClipData()"), "several files");
        // The camera: anything more is denied whole before any prompt; the
        // window and the manifest decide, each named when it says no.
        let camera = method(java, "public void onPermissionRequest(");
        let deny = camera.find("r.deny()").unwrap();
        assert!(deny < camera.find("requestPermissions(").unwrap() && deny < camera.find("r.grant(").unwrap());
        assert!(camera.contains("refuses a request it cannot grant whole"));
        assert!(camera.contains("Host.allows(\"media\")") && camera.contains("declared(Manifest.permission.CAMERA)"));
        assert!(camera.contains("[build.android] camera = true") && camera.contains("Window { permissions }"));
        // A font size change keeps the page and its text follows.
        let config = method(java, "public void onConfigurationChanged(");
        assert!(config.contains("setTextZoom(Math.round(c.fontScale * 100))"));
        // A download is said, never dropped in silence.
        assert!(java.contains("download NOT saved: "));
    }

    #[test]
    fn a_link_reaches_the_app_cold_and_warm_and_the_splash_waits_for_the_first_frame() {
        let java = TEMPLATE[6].1;
        let create = method(java, "protected void onCreate(");
        assert!(create.contains("if (state == null) link(getIntent());"), "cold, once (a recreate carries a state)");
        assert!(method(java, "protected void onNewIntent(").contains("link(i);"), "warm");
        let link = method(java, "private void link(");
        assert!(
            link.contains("Intent.ACTION_VIEW.equals(i.getAction())") && link.contains("Host.link(i.getDataString())")
        );
        // The system's splash stays until the page says its first frame is
        // drawn, at most SPLASH_CAP_MS.
        let draw = method(create, "public boolean onPreDraw()");
        assert!(draw.contains("if (!painted && splash == null) return false;"), "{draw}");
        assert!(create.contains("}, SPLASH_CAP_MS);"));
        assert!(method(java, "public void onMessage(").contains("if (PAINTED.equals(m.getData())) painted();"));
        let natives = include_str!("../../aio-shell/src/android.rs");
        assert!(natives.contains("port.postMessage('aio-painted')"), "the page says it");
        // base64 is 4n long: a frame is never the word.
        assert_ne!("aio-painted".len() % 4, 0);
    }

    #[test]
    fn the_splash_is_the_starting_window_s_colour_and_the_activity_s_image_until_the_first_frame() {
        let app = App { id: "notes".into(), name: "Notes".into(), version: "1.2.3".into(), icon: None };
        let s = Splash {
            image: "assets/splash@2x.png".into(),
            text: Some("Starting {title}".into()),
            min_ms: 700,
            max_ms: 9000,
            background: None,
            font: None,
            font_size: None,
            bold: false,
            fade_ms: None,
            png: &[],
        };
        let colors = [
            ["#fff", "#111", "#00f", "#ccc"].map(String::from),
            ["#0e1015", "#eceef3", "#00f", "#333"].map(String::from),
        ];
        let files = splash_files(&s, b"PNG", "Notes", &colors, ColorScheme::System);
        let get =
            |p: &str| files.iter().find(|(f, _)| f.ends_with(p)).map(|(_, b)| String::from_utf8_lossy(b).into_owned());
        let json: aio::serde_json::Value = aio::serde_json::from_str(&get("aio-splash.json").unwrap()).unwrap();
        assert_eq!(json["text"], "Starting Notes");
        assert_eq!(
            (json["min"].as_u64(), json["max"].as_u64(), json["scale"].as_u64()),
            (Some(700), Some(9000), Some(2))
        );
        assert_eq!(
            (json["light"][0].as_str(), json["dark"][1].as_str()),
            (Some("#ffffff"), Some("#eceef3")),
            "six digits"
        );
        assert!(json["night"].is_null(), "the phone's mode");
        assert!(get("values/aio_splash_color.xml").unwrap().contains(">#ffffff<"));
        assert!(get("values-night/aio_splash_color.xml").unwrap().contains(">#0e1015<"));
        assert!(
            get("aio_splash_theme.xml").unwrap().contains("parent=\"@android:style/Theme.DeviceDefault.DayNight\"")
        );
        let dark = splash_files(&s, b"PNG", "Notes", &colors, ColorScheme::Dark);
        assert!(!dark.iter().any(|(f, _)| f.contains("values-night")) && dark.iter().any(|(_, b)| b == b"PNG"));
        let all = project(&app, None, &AndroidBuild::default(), b"png", &files, &[], Apk::App).unwrap();
        let manifest = all.iter().find(|(p, _)| p.ends_with("AndroidManifest.xml")).unwrap();
        assert!(String::from_utf8_lossy(&manifest.1).contains("android:theme=\"@style/AioSplash\""));
        // The activity: its splash at once (no system hold), gone at the
        // first frame once up its minimum, or at its cap.
        let java = TEMPLATE[6].1;
        let create = method(java, "protected void onCreate(");
        assert!(
            create.contains("splash = splash(frame);")
                && create.contains("if (!painted && splash == null) return false;")
        );
        let painted = method(java, "private void painted(");
        assert!(painted.contains("splashMin - (SystemClock.uptimeMillis() - splashShown)"));
        assert!(method(java, "private View splash(").contains("dropSplash(\"its cap\")"));
    }

    #[test]
    fn ids_and_version_codes_suit_android() {
        assert_eq!(application_id("my-notes"), "aio.rt.my_notes");
        assert_eq!(application_id("2do"), "aio.rt._2do");
        // aio build-android.ts `androidVersion`: major·10⁸ + minor·10⁶ + build.
        assert_eq!(version_code("1.2.3"), Ok(102_000_003));
        assert_eq!(version_code("1.2.345"), Ok(102_000_345), "aio's 1.2.345 is the same code");
        assert!(
            version_code("1.2.346").unwrap() > version_code("1.2.345").unwrap(),
            "a port's next build installs over it"
        );
        assert!(version_code("0.10.0").unwrap() > version_code("0.9.999999").unwrap());
        assert_eq!(version_code("1.2.3-rc.1"), Ok(102_000_003));
        assert_eq!(version_code("20.99.999999"), Ok(2_099_999_999), "the largest fits an i32");
        for bad in ["1.2", "1.100.0", "21.0.0", "1.0.1000000"] {
            assert!(version_code(bad).is_err(), "{bad}");
        }
        // aio-rs 0.1's codes (M·10⁶ + m·10³ + p) are all lower: its APKs update too.
        assert!(version_code("0.1.0").unwrap() > 1_000);
    }

    #[test]
    fn the_project_is_filled_in() {
        let app = App { id: "notes".into(), name: "A & B".into(), version: "1.2.3".into(), icon: None };
        let get = |files: &[(String, Vec<u8>)], p: &str| {
            files.iter().find(|(f, _)| f == p).map(|(_, t)| String::from_utf8_lossy(t).into_owned()).unwrap()
        };
        let files = project(&app, None, &AndroidBuild::default(), b"png", &[], &[], Apk::App).unwrap();
        let gradle = get(&files, "app/build.gradle.kts");
        assert!(gradle.contains("applicationId = \"aio.rt.notes\"") && gradle.contains("versionCode = 102000003"));
        assert!(gradle.contains("include(\"arm64-v8a\", \"armeabi-v7a\", \"x86_64\")"));
        assert!(gradle.contains("useLegacyPackaging = true"), "the library deflated: half the APK");
        let manifest = get(&files, "app/src/main/AndroidManifest.xml");
        assert!(
            manifest.contains("android:label=\"A &amp; B\"")
                && manifest.contains("android:icon=\"@drawable/ic_launcher\"")
        );
        assert_eq!(get(&files, ICON), "png", "the launcher icon");
        assert!(files.iter().all(|(_, t)| !String::from_utf8_lossy(t).contains("{{")), "every placeholder filled");
        let own = AndroidBuild { application_id: Some("com.example.notes".into()), ..Default::default() };
        let gradle = get(&project(&app, None, &own, b"png", &[], &[], Apk::App).unwrap(), "app/build.gradle.kts");
        assert!(gradle.contains("applicationId = \"com.example.notes\""), "{gradle}");
        assert_eq!(application_id_of("notes", None, &AndroidBuild::default()), "aio.rt.notes");
    }

    #[test]
    fn the_apps_android_files_replace_or_add_to_the_template_placeholders_filled() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("android");
        std::fs::create_dir_all(dir.join("app/src/main/res/drawable")).unwrap();
        std::fs::write(dir.join("app/src/main/AndroidManifest.xml"), "<manifest label=\"{{NAME}}\"/>").unwrap();
        std::fs::write(dir.join("app/src/main/res/drawable/logo.png"), [0x89, 0xff, 0x00]).unwrap();
        let overlay = overlay_of(&dir).unwrap();
        let paths: Vec<&str> = overlay.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(paths, ["app/src/main/AndroidManifest.xml", "app/src/main/res/drawable/logo.png"]);
        assert_eq!(overlay_of(&d.path().join("none")).unwrap(), vec![], "no android/: the template as is");
        let app = App { id: "notes".into(), name: "Notes".into(), version: "1.2.3".into(), icon: None };
        let files = project(&app, None, &AndroidBuild::default(), b"png", &[], &overlay, Apk::App).unwrap();
        let of = |p: &str| files.iter().filter(|(f, _)| f == p).map(|(_, b)| b.clone()).collect::<Vec<_>>();
        assert_eq!(of("app/src/main/AndroidManifest.xml"), [b"<manifest label=\"Notes\"/>".to_vec()], "replaced, once");
        assert_eq!(of("app/src/main/res/drawable/logo.png"), [vec![0x89, 0xff, 0x00]], "bytes as they are");
        assert_eq!(files.len(), TEMPLATE.len() + 2, "the template, its icon, the added file");
        let own_icon = [("app/src/main/res/drawable/ic_launcher.png".to_string(), b"mine".to_vec())];
        let files = project(&app, None, &AndroidBuild::default(), b"png", &[], &own_icon, Apk::App).unwrap();
        assert_eq!(files.iter().filter(|(p, _)| p == ICON).map(|(_, b)| b.as_slice()).collect::<Vec<_>>(), [b"mine"]);
    }

    #[test]
    fn a_template_changed_under_an_overlay_warns_until_the_overlay_is_rebased() {
        let now =
            |t: &str, o: &str| BTreeMap::from([("app/build.gradle.kts".to_string(), (t.to_string(), o.to_string()))]);
        // First sight: recorded, nothing said.
        let (rec, w) = drift(&BTreeMap::new(), &now("t1", "o1"), "2.0.0");
        assert!(w.is_empty());
        assert_eq!(
            rec["app/build.gradle.kts"],
            Seen { template: "t1".into(), aio: "2.0.0".into(), overlay: "o1".into() }
        );
        // The same again: quiet, and the aio that shipped it is kept.
        assert_eq!(drift(&rec, &now("t1", "o1"), "2.1.0"), (rec.clone(), vec![]));
        // aio changed the template: warned, the record kept, on every build.
        let (kept, w) = drift(&rec, &now("t2", "o1"), "2.1.0");
        assert_eq!(kept, rec);
        assert!(
            w[0].starts_with("android/app/build.gradle.kts overlays a template that changed")
                && w[0].contains("2.0.0 → 2.1.0"),
            "{w:?}"
        );
        // Re-based: the overlay changed, so the pair is recorded afresh.
        let (rec2, w) = drift(&kept, &now("t2", "o2"), "2.1.0");
        assert!(w.is_empty());
        assert_eq!(rec2["app/build.gradle.kts"].aio, "2.1.0");
    }

    #[test]
    fn the_drift_record_lives_in_the_apps_dir_and_only_for_replaced_files() {
        let d = tempfile::tempdir().unwrap();
        let overlay =
            vec![("build.gradle.kts".to_string(), b"mine".to_vec()), ("extra.txt".to_string(), b"x".to_vec())];
        assert!(check_drift(d.path(), &overlay).is_empty());
        let rec: serde_json::Value =
            serde_json::from_slice(&std::fs::read(d.path().join(OVERLAY_RECORD)).unwrap()).unwrap();
        let keys: Vec<&String> = rec.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["build.gradle.kts"], "an added file replaces nothing");
        assert_eq!(rec["build.gradle.kts"]["aio"], env!("CARGO_PKG_VERSION"));
        // A record from an older template: warned.
        let mut old = rec.clone();
        old["build.gradle.kts"]["template"] = "0".repeat(64).into();
        std::fs::write(d.path().join(OVERLAY_RECORD), old.to_string()).unwrap();
        assert_eq!(check_drift(d.path(), &overlay).len(), 1);
    }

    #[test]
    fn a_release_keystore_comes_from_the_environment_or_aio_toml_and_is_checked_first() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("keys")).unwrap();
        std::fs::write(d.path().join("keys/up.jks"), b"ks").unwrap();
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
        };
        let none = AndroidBuild::default();
        assert_eq!(signing(env(&[]), &none, d.path()), Ok(None), "none named: aio's own key");
        let declared =
            AndroidBuild { keystore: Some("keys/up.jks".into()), key_alias: Some("up".into()), ..none.clone() };
        let s = signing(env(&[("AIO_ANDROID_KEYSTORE_PASSWORD", "x")]), &declared, d.path()).unwrap().unwrap();
        assert_eq!(
            s,
            Signing {
                keystore: d.path().join("keys/up.jks"),
                alias: "up".into(),
                pass: Pass::Env("AIO_ANDROID_KEYSTORE_PASSWORD"),
                key_pass: None,
            }
        );
        assert_eq!(s.pass.apksigner(), "env:AIO_ANDROID_KEYSTORE_PASSWORD", "the password never on a command line");
        assert_eq!(Pass::File("/k/p".into()).jarsigner("-storepass"), ["-storepass:file", "/k/p"]);
        // CI: the environment names it, alias and key password too.
        let ci = env(&[
            ("AIO_ANDROID_KEYSTORE", "keys/up.jks"),
            ("AIO_ANDROID_KEYSTORE_PASSWORD", "x"),
            ("AIO_ANDROID_KEY_ALIAS", "ci"),
            ("AIO_ANDROID_KEY_PASSWORD", "y"),
        ]);
        let s = signing(ci, &none, d.path()).unwrap().unwrap();
        assert_eq!((s.alias.as_str(), s.key_pass), ("ci", Some(Pass::Env("AIO_ANDROID_KEY_PASSWORD"))));
        // Refused, by name, before anything is built.
        let e = signing(env(&[("AIO_ANDROID_KEYSTORE", "gone.jks")]), &none, d.path()).unwrap_err();
        assert!(e.starts_with("AIO_ANDROID_KEYSTORE=gone.jks: no keystore file at"), "{e}");
        let e = signing(env(&[]), &AndroidBuild { key_alias: None, ..declared }, d.path()).unwrap_err();
        assert!(
            e.contains(
                "AIO_ANDROID_KEYSTORE_PASSWORD and AIO_ANDROID_KEY_ALIAS (or [build.android] key_alias) are not set"
            ),
            "{e}"
        );
    }

    #[test]
    fn a_port_has_the_aio_apks_id_and_signs_with_its_key_so_it_installs_over_it() {
        // aio build-android.ts `androidApplicationId`.
        assert_eq!(aio_application_id("notes"), Some("app.aio.notes".into()));
        assert_eq!(aio_application_id("my-notes_2"), Some("app.aio.mynotes2".into()));
        assert_eq!(aio_application_id("3dview"), None, "aio made no APK of it");
        let none = AndroidBuild::default();
        assert_eq!(application_id_of("notes-rs", Some("notes"), &none), "app.aio.notes");
        assert_eq!(application_id_of("x", Some("3d"), &none), "aio.rt.x");
        let own = AndroidBuild { application_id: Some("com.example.notes".into()), ..none.clone() };
        assert_eq!(application_id_of("notes-rs", Some("notes"), &own), "com.example.notes", "declared wins");
        let app = App { id: "notes-rs".into(), name: "Notes".into(), version: "1.3.0".into(), icon: None };
        let files = project(&app, Some("notes"), &none, b"png", &[], &[], Apk::App).unwrap();
        let gradle =
            String::from_utf8_lossy(&files.iter().find(|(p, _)| p == "app/build.gradle.kts").unwrap().1).into_owned();
        assert!(gradle.contains("applicationId = \"app.aio.notes\""), "{gradle}");
        // aio's default APK is a debug one: Android's debug key of the machine.
        let d = tempfile::tempdir().unwrap();
        assert_eq!(aio_key(None, d.path()), None);
        std::fs::create_dir_all(d.path().join(".android")).unwrap();
        std::fs::write(d.path().join(".android/debug.keystore"), b"ks").unwrap();
        let k = aio_key(None, d.path()).unwrap();
        assert_eq!((k.alias.as_str(), k.pass.apksigner()), ("androiddebugkey", "pass:android".into()));
        assert_eq!(k.pass.jarsigner("-storepass"), ["-storepass", "android"]);
        assert_eq!(aio_key(Some(d.path().join("elsewhere")), d.path()), None, "ANDROID_USER_HOME moves it");
    }

    #[test]
    fn the_label_is_escaped_as_aapt_reads_it_and_never_empty() {
        // aio build-android.ts `_appNameXml`.
        assert_eq!(label_xml("A & B <x> \"q\""), "A &amp; B &lt;x&gt; &quot;q&quot;");
        assert_eq!(label_xml("back\\slash"), "back\\\\slash");
        assert_eq!(label_xml("?quest"), "\\?quest");
        assert_eq!(label_xml("@home"), "\\@home");
        assert_eq!(label_xml("tab\there\u{7f}"), "tabhere");
        let app = App { id: "notes".into(), name: String::new(), version: "1.0.0".into(), icon: None };
        let files = project(&app, None, &AndroidBuild::default(), b"png", &[], &[], Apk::App).unwrap();
        let m = String::from_utf8_lossy(&files.iter().find(|(p, _)| p.ends_with("AndroidManifest.xml")).unwrap().1)
            .into_owned();
        assert!(m.contains("android:label=\"notes\""), "no name: the id: {m}");
    }

    #[test]
    fn a_placeholder_left_unfilled_is_named_once() {
        assert_eq!(leftover_tokens("a {{NAME}} b {{TYPO_X}} {{TYPO_X}} {{lower}} {{ X}}"), ["{{NAME}}", "{{TYPO_X}}"]);
        assert!(leftover_tokens("{ {not} }").is_empty());
    }

    #[test]
    fn an_own_activity_is_checked_for_what_the_templates_does_and_strict_refuses() {
        let java = TEMPLATE[6].1;
        assert!(activity_losses(java, true).is_empty(), "the template itself lacks nothing");
        let old = "frame.setOnApplyWindowInsetsListener((v, i) -> i.getInsets(WindowInsets.Type.ime()));";
        let lost = activity_losses(old, true);
        assert_eq!(lost.len(), 3, "{lost:?}");
        assert!(lost[0].contains("Android 10") && lost[1].contains("renderer") && lost[2].contains("splash"));
        assert!(activity_losses("", false)[0].contains("insets frame"));
    }

    #[test]
    fn the_insets_never_touch_api_30_on_android_10() {
        let java = TEMPLATE[6].1;
        let listener = &java[java.find("frame.setOnApplyWindowInsetsListener").unwrap()..];
        let listener = &listener[..listener.find(";").unwrap()];
        assert!(listener.contains("SDK_INT >= 30") && listener.contains("Api30.pad") && listener.contains("legacyPad"));
        // Every API-30 name lives in the nested class, resolved only when called.
        let api30 = method(java, "private static final class Api30");
        let outside = java.replace(api30, "");
        for name in ["WindowInsets.Type", "WindowInsets.CONSUMED", "android.graphics.Insets"] {
            assert!(api30.contains(name) && !outside.contains(name), "{name} outside Api30");
        }
        let legacy = method(java, "private static android.view.WindowInsets legacyPad(");
        assert!(legacy.contains("getSystemWindowInsetTop()") && legacy.contains("getDisplayCutout()"));
        assert!(legacy.contains("consumeSystemWindowInsets()"));
        // minSdk is aio's: Android 10.
        assert!(TEMPLATE[3].1.contains("minSdk = 29"));
    }

    #[test]
    fn the_dev_page_apk_is_beside_the_app_without_its_library() {
        let app = App { id: "notes".into(), name: "Notes".into(), version: "1.2.3".into(), icon: None };
        let files = project(&app, None, &AndroidBuild::default(), b"png", &[], &[], Apk::Page).unwrap();
        let get =
            |p: &str| files.iter().find(|(f, _)| f.ends_with(p)).map(|(_, b)| String::from_utf8_lossy(b).into_owned());
        let (gradle, manifest) = (get("app/build.gradle.kts").unwrap(), get("AndroidManifest.xml").unwrap());
        assert!(gradle.contains("applicationId = \"aio.rt.notes.dev\"") && gradle.contains("isEnable = false"));
        assert!(
            manifest.contains("android:label=\"Notes (dev)\"")
                && manifest.contains("android:name=\"aio.rt.DevActivity\"")
        );
        assert!(manifest.contains("usesCleartextTraffic=\"true\""), "http://127.0.0.1 over adb");
        let dev = get("DevActivity.java").unwrap();
        assert!(dev.contains("getStringExtra(\"url\")") && !dev.contains("Host."), "no library in it");
        let real = project(&app, None, &AndroidBuild::default(), b"png", &[], &[], Apk::App).unwrap();
        assert!(!real.iter().any(|(p, _)| p.ends_with("DevActivity.java")), "the app has no dev activity");
        let m = String::from_utf8_lossy(&real.iter().find(|(p, _)| p.ends_with("AndroidManifest.xml")).unwrap().1)
            .into_owned();
        assert!(m.contains("usesCleartextTraffic=\"false\"") && m.contains("aio.rt.MainActivity"));
        // The native dev copy: the app's activity, beside the app.
        let copy = project(&app, None, &AndroidBuild::default(), b"png", &[], &[], Apk::Dev).unwrap();
        let g =
            String::from_utf8_lossy(&copy.iter().find(|(p, _)| p == "app/build.gradle.kts").unwrap().1).into_owned();
        assert!(g.contains("aio.rt.notes.dev") && g.contains("isEnable = true"));
        let universal = AndroidBuild { universal: true, ..Default::default() };
        let u = project(&app, None, &universal, b"png", &[], &[], Apk::App).unwrap();
        assert!(
            String::from_utf8_lossy(&u.iter().find(|(p, _)| p == "app/build.gradle.kts").unwrap().1)
                .contains("isUniversalApk = true")
        );
    }

    #[test]
    fn the_host_tools_are_found_on_linux_macos_and_windows() {
        assert_eq!(clang("aarch64-linux-android", "linux"), "aarch64-linux-android29-clang");
        assert_eq!(clang("armv7-linux-androideabi", "macos"), "armv7a-linux-androideabi29-clang");
        assert_eq!(clang("x86_64-linux-android", "windows"), "x86_64-linux-android29-clang.cmd");
        assert_eq!(
            [prebuilt("linux"), prebuilt("macos"), prebuilt("windows")],
            ["linux-x86_64", "darwin-x86_64", "windows-x86_64"]
        );
        assert_eq!(
            (exe("zipalign", "windows"), script("apksigner", "windows")),
            ("zipalign.exe".into(), "apksigner.bat".into())
        );
        assert_eq!((exe("zipalign", "linux"), script("apksigner", "macos")), ("zipalign".into(), "apksigner".into()));
        let home = Path::new("/h");
        let places = sdk_places(&[Some("/opt/Android".into()), None], home);
        assert_eq!(&places[..3], [PathBuf::from("/opt/Android"), "/opt/Android/Sdk".into(), "/opt/Android/sdk".into()]);
        assert!(
            places.contains(&home.join("Library/Android/sdk"))
                && places.contains(&home.join("AppData/Local/Android/Sdk"))
        );
        let env = |k: &str| (k == "ProgramFiles").then(|| PathBuf::from("C:/PF"));
        let (roots, homes) = jdk_places("windows", home, env);
        assert!(roots.contains(&PathBuf::from("C:/PF/Eclipse Adoptium")));
        assert!(homes.contains(&PathBuf::from("C:/PF/Android/Android Studio/jbr")), "Android Studio's own JDK");
        assert!(jdk_places("macos", home, |_| None).0.contains(&PathBuf::from("/Library/Java/JavaVirtualMachines")));
    }

    #[test]
    fn a_jdk_is_one_whose_javac_says_a_version_gradle_runs_on() {
        // aio build-helpers.ts `findJdk`.
        assert_eq!(javac_major("javac 21.0.4\n"), Some(21));
        assert_eq!(javac_major("javac 1.8.0_402"), Some(8));
        assert_eq!(javac_major("javac 25-ea"), Some(25));
        assert_eq!(javac_major("openjdk 21"), None, "a JRE has no javac");
        let j = |h: &str, m: u32| (PathBuf::from(h), m);
        let found = [j("/j8", 8), j("/j17", 17), j("/j21", 21), j("/j23", 23), j("/j25", 25)];
        assert_eq!(pick_jdk(&found, None), Some(j("/j21", 21)), "the newest LTS AGP is made for");
        assert_eq!(pick_jdk(&found, Some(Path::new("/j17"))), Some(j("/j17", 17)), "JAVA_HOME when it fits");
        assert_eq!(pick_jdk(&found, Some(Path::new("/j25"))), Some(j("/j21", 21)), "not one Gradle can't run on");
        assert_eq!(pick_jdk(&[j("/j23", 23), j("/j25", 25)], None), Some(j("/j23", 23)));
        assert_eq!(pick_jdk(&[j("/j8", 8)], None), None);
        // Gradle 9 writes the wrapper too; the build runs the pinned one.
        assert!(gradle_ok("8.9") && gradle_ok("8.14.3") && gradle_ok("9.1.0"));
        assert!(!gradle_ok("8.8") && !gradle_ok("4.4.1"));
    }

    #[test]
    fn the_splash_text_takes_its_font_size_weight_and_fade() {
        let s = Splash {
            image: "splash.png".into(),
            text: Some("{title}".into()),
            min_ms: 400,
            max_ms: 9000,
            background: None,
            font: Some("serif".into()),
            font_size: Some(18),
            bold: true,
            fade_ms: Some(350),
            png: &[],
        };
        let colors =
            [["#fff", "#111", "#00f", "#ccc"].map(String::from), ["#000", "#eee", "#00f", "#333"].map(String::from)];
        let files = splash_files(&s, b"PNG", "Notes", &colors, ColorScheme::System);
        let json: aio::serde_json::Value =
            aio::serde_json::from_slice(&files.iter().find(|(p, _)| p.ends_with(".json")).unwrap().1).unwrap();
        assert_eq!(
            (json["font"].as_str(), json["size"].as_u64(), json["bold"].as_bool(), json["fade"].as_u64()),
            (Some("serif"), Some(18), Some(true), Some(350))
        );
        let plain = splash_files(
            &Splash { font: None, font_size: None, bold: false, fade_ms: None, ..s },
            b"PNG",
            "N",
            &colors,
            ColorScheme::System,
        );
        let json: aio::serde_json::Value =
            aio::serde_json::from_slice(&plain.iter().find(|(p, _)| p.ends_with(".json")).unwrap().1).unwrap();
        assert_eq!(
            (json["font"].is_null(), json["size"].as_u64(), json["fade"].as_u64()),
            (true, Some(14), Some(200)),
            "aio's look"
        );
        let java = TEMPLATE[6].1;
        let splash = method(java, "private View splash(");
        assert!(
            splash.contains("optInt(\"size\", 14)")
                && splash.contains("Typeface.BOLD")
                && splash.contains("optLong(\"fade\", 200)")
        );
        assert!(method(java, "private void dropSplash(").contains("setDuration(splashFade)"));
    }

    #[test]
    fn a_download_reaches_downloads_or_the_users_pick_and_images_are_served_from_disk() {
        let java = TEMPLATE[6].1;
        let d = method(java, "private void download(");
        assert!(d.contains("DownloadManager.Request(uri)") && d.contains("DIRECTORY_DOWNLOADS"), "http(s): Downloads");
        assert!(d.contains("ACTION_CREATE_DOCUMENT") && d.contains("Base64.decode"), "data: where the user picks");
        assert!(d.contains("download NOT saved"), "blob: said, not dropped");
        assert!(method(java, "protected void onActivityResult(").contains("saved("));
        let serve = method(java, "public android.webkit.WebResourceResponse shouldInterceptRequest(");
        assert!(serve.contains("\"/file/\"") && serve.contains("IMAGE.matcher") && serve.contains("404"));
        assert!(
            java.contains("FLAG_DEBUGGABLE") && java.contains("setWebContentsDebuggingEnabled(true)"),
            "--debug: chrome://inspect"
        );
    }
}

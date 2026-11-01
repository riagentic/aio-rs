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

/// Each CPU type: Android's name, Rust's target.
pub const ABIS: [(&str, &str); 2] = [("arm64-v8a", "aarch64-linux-android"), ("x86_64", "x86_64-linux-android")];

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

/// The app's application id: the declared one, else [`application_id`].
pub fn application_id_of(id: &str, cfg: &AndroidBuild) -> String {
    cfg.application_id.clone().unwrap_or_else(|| application_id(id))
}

/// Where a keystore password is: an environment variable (CI) or a file
/// (aio's own key), never on a command line.
#[derive(Clone, Debug, PartialEq)]
pub enum Pass {
    Env(&'static str),
    File(PathBuf),
}

impl Pass {
    /// apksigner's form: `env:<VAR>`, `file:<path>`.
    fn apksigner(&self) -> String {
        match self {
            Pass::Env(v) => format!("env:{v}"),
            Pass::File(p) => format!("file:{}", p.display()),
        }
    }

    /// jarsigner's: `-storepass:env <VAR>`, `-storepass:file <path>`.
    fn jarsigner(&self, flag: &str) -> [String; 2] {
        match self {
            Pass::Env(v) => [format!("{flag}:env"), v.to_string()],
            Pass::File(p) => [format!("{flag}:file"), p.display().to_string()],
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

/// `1.2.3` → 1002003: grows with every release (a prerelease shares its
/// release's code, so it must not ship to the same users).
pub fn version_code(v: &str) -> Result<u32, String> {
    let core = v.split(['-', '+']).next().unwrap_or(v);
    let n: Vec<u32> =
        core.split('.').map(|p| p.parse().map_err(|_| format!("version `{v}`"))).collect::<Result<_, _>>()?;
    let [major, minor, patch] = n[..] else { return Err(format!("version `{v}`: major.minor.patch")) };
    if minor > 999 || patch > 999 || major > 2000 {
        return Err(format!("version `{v}` does not fit an Android version code"));
    }
    Ok(major * 1_000_000 + minor * 1_000 + patch)
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
pub fn project(
    app: &App,
    cfg: &AndroidBuild,
    icon: &[u8],
    splash: &[(String, Vec<u8>)],
    overlay: &[(String, Vec<u8>)],
) -> Result<Vec<(String, Vec<u8>)>, String> {
    let abis = ABIS.iter().map(|(a, _)| format!("\"{a}\"")).collect::<Vec<_>>().join(", ");
    let code = version_code(&app.version)?.to_string();
    let xml = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('"', "&quot;");
    let parts = manifest_parts(cfg, !splash.is_empty());
    let fill = |text: &str| {
        let text = text
            .replace("{{ID}}", &app.id)
            .replace("{{APPLICATION_ID}}", &application_id_of(&app.id, cfg))
            .replace("{{VERSION_CODE}}", &code)
            .replace("{{VERSION}}", &app.version)
            .replace("{{ABIS}}", &abis)
            .replace("{{NAME}}", &xml(&app.name));
        parts.iter().fold(text, |t, (k, v)| t.replace(k, v))
    };
    let mut files: Vec<(String, Vec<u8>)> = TEMPLATE
        .iter()
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

fn home() -> PathBuf {
    env_dir("HOME").unwrap_or_default()
}

/// The newest dir in `d` (by name).
fn newest(d: &Path) -> Option<PathBuf> {
    let mut all: Vec<PathBuf> = std::fs::read_dir(d).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    all.sort();
    all.pop()
}

/// The SDK: `ANDROID_HOME`, `ANDROID_SDK_ROOT`, or `~/Android/Sdk`.
fn sdk() -> Result<PathBuf, String> {
    env_dir("ANDROID_HOME")
        .or_else(|| env_dir("ANDROID_SDK_ROOT"))
        .or_else(|| Some(home().join("Android/Sdk")))
        .filter(|d| d.join("platforms").is_dir())
        .ok_or_else(|| "no Android SDK: install it and set ANDROID_HOME".into())
}

/// A JDK Gradle 8 runs on (17 to 21): `JAVA_HOME` if it is one, else one in
/// `/usr/lib/jvm`.
fn jdk() -> Result<PathBuf, String> {
    let fits = |d: &Path| {
        let name = d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        ["-17", "-21"].iter().any(|v| name.contains(v)) && d.join("bin/java").exists()
    };
    if let Some(j) = env_dir("JAVA_HOME").filter(|d| fits(d)) {
        return Ok(j);
    }
    let mut found: Vec<PathBuf> = std::fs::read_dir("/usr/lib/jvm")
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| fits(p)).collect())
        .unwrap_or_default();
    found.sort();
    found.pop().ok_or_else(|| "no JDK 17 or 21 (Gradle 8 needs one): install it or set JAVA_HOME".into())
}

/// Gradle 8.9 or newer: on `PATH`, else one the Gradle wrapper fetched.
fn gradle() -> Result<PathBuf, String> {
    let ok = |v: &str| {
        let mut n = v.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
        let (major, minor) = (n.next().unwrap_or(0), n.next().unwrap_or(0));
        major == 8 && minor >= 9
    };
    let on_path = Command::new("gradle").arg("--version").output().ok().and_then(|o| {
        let text = String::from_utf8_lossy(&o.stdout).into_owned();
        let v = text.lines().find_map(|l| l.strip_prefix("Gradle "))?.trim().to_string();
        ok(&v).then(|| PathBuf::from("gradle"))
    });
    if let Some(g) = on_path {
        return Ok(g);
    }
    let dists = home().join(".gradle/wrapper/dists");
    let mut found: Vec<(String, PathBuf)> = std::fs::read_dir(&dists)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let v = name.strip_prefix("gradle-")?;
            let v = v.strip_suffix("-bin").or_else(|| v.strip_suffix("-all"))?.to_string();
            let bin = newest(&e.path())?.join(format!("gradle-{v}/bin/gradle"));
            (ok(&v) && bin.exists()).then_some((v, bin))
        })
        .collect();
    found.sort_by_key(|(v, _)| v.split('.').map(|p| p.parse::<u32>().unwrap_or(0)).collect::<Vec<_>>());
    found.pop().map(|(_, b)| b).ok_or_else(|| "no Gradle 8.9+ (on PATH or in ~/.gradle/wrapper): install it".into())
}

/// aio's signing key for app `id`: made once (keytool), kept in the keys dir.
fn key(id: &str) -> Result<Signing, String> {
    let dir = crate::release::keys_dir();
    aio::paths::private_dir(&dir).map_err(|e| e.to_string())?;
    let ks = dir.join(format!("{id}.android.p12"));
    let pass_file = dir.join(format!("{id}.android.pass"));
    if !ks.exists() {
        let pass = format!("{:032x}", rand_u128()?);
        aio::paths::write_atomic(&pass_file, pass.as_bytes(), true).map_err(|e| e.to_string())?;
        let st = Command::new(jdk()?.join("bin/keytool"))
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

fn rand_u128() -> Result<u128, String> {
    let mut b = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut b))
        .map_err(|e| format!("/dev/urandom: {e}"))?;
    Ok(u128::from_le_bytes(b))
}

fn run(cmd: &mut Command, what: &str) -> Result<(), String> {
    let st = cmd.status().map_err(|e| format!("{what}: {e}"))?;
    if st.success() { Ok(()) } else { Err(format!("{what} failed")) }
}

/// Build the APKs of `pkg` (with lib target `lib`) into `dist`, and the
/// bundle when `cfg` asks; `app_dir` holds its `aio.toml`.
pub fn build(
    app: &App,
    full: &aio::config::Config,
    app_dir: &Path,
    pkg: &str,
    lib: &str,
    target_dir: &Path,
    dist: &Path,
) -> Result<Vec<PathBuf>, String> {
    let cfg = &full.build.android;
    let given = signing(|k| std::env::var(k).ok(), cfg, app_dir)?;
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
    let missing = crate::build::missing_targets(&ABIS.map(|(_, t)| t));
    if !missing.is_empty() {
        return Err(format!("Rust's Android targets are not installed: rustup target add {}", missing.join(" ")));
    }
    let sdk = sdk()?;
    let ndk = newest(&sdk.join("ndk")).ok_or("no Android NDK in the SDK: install one (sdkmanager \"ndk;…\")")?;
    let bin = ndk.join("toolchains/llvm/prebuilt/linux-x86_64/bin");
    let proj = dist.join(format!(".{}-android", app.id));
    _ = std::fs::remove_dir_all(&proj);
    let made = (|| {
        if !overlay.is_empty() {
            let paths: Vec<&str> = overlay.iter().map(|(p, _)| p.as_str()).collect();
            eprintln!("ax build: {} files from android/ over the template: {}", paths.len(), paths.join(", "));
        }
        for w in check_drift(app_dir, &overlay) {
            eprintln!("ax build: {w}");
        }
        for (path, text) in project(app, cfg, &icon, &splash, &overlay)? {
            let p = proj.join(path);
            std::fs::create_dir_all(p.parent().ok_or("bad path")?).map_err(|e| e.to_string())?;
            std::fs::write(&p, text).map_err(|e| e.to_string())?;
        }
        std::fs::write(proj.join("local.properties"), format!("sdk.dir={}\n", sdk.display()))
            .map_err(|e| e.to_string())?;
        for (abi, target) in ABIS {
            eprintln!("ax build: {} for {abi}…", app.name);
            let clang = bin.join(format!("{target}{API}-clang"));
            let env = target.replace('-', "_");
            let step = format!("the {abi} library");
            let mut cargo = crate::freeze::cargo(
                &step,
                &["rustc", "--release", "--lib", "--crate-type", "cdylib", "-p", pkg, "--target", target],
            )?;
            crate::build::remap(&mut cargo, app_dir);
            run(
                cargo
                    // The app's version (`aio::app_version!()`), the APK's as on desktop.
                    .env("AIORT_APP_VERSION", &app.version)
                    .arg("--")
                    .args(LINK)
                    .env(format!("CC_{env}"), &clang)
                    .env(format!("AR_{env}"), bin.join("llvm-ar"))
                    .env(format!("CARGO_TARGET_{}_LINKER", env.to_uppercase()), &clang),
                &step,
            )?;
            let so = target_dir.join(target).join("release").join(format!("lib{lib}.so"));
            crate::build::no_home_paths(&std::fs::read(&so).map_err(|e| format!("{}: {e}", so.display()))?, &so)?;
            let to = proj.join("app/src/main/jniLibs").join(abi).join("libapp.so");
            std::fs::create_dir_all(to.parent().ok_or("bad path")?).map_err(|e| e.to_string())?;
            std::fs::copy(&so, &to).map_err(|e| format!("{}: {e}", so.display()))?;
        }
        let jdk = jdk()?;
        crate::freeze::ensure("gradle")?;
        run(
            Command::new(gradle()?)
                .args(["-q", "--no-daemon", &format!("--max-workers={}", crate::freeze::max_jobs()), "-p"])
                .arg(&proj)
                .arg("assembleRelease")
                .args(cfg.aab.then_some("bundleRelease"))
                .env("JAVA_HOME", &jdk),
            "gradle",
        )?;
        let tools = newest(&sdk.join("build-tools")).ok_or("no Android build-tools in the SDK")?;
        let sign = match given {
            Some(s) => s,
            None => key(&app.id)?,
        };
        let out_dir = proj.join("app/build/outputs/apk/release");
        let mut apks = vec![];
        for (abi, _) in ABIS {
            let unsigned = out_dir.join(format!("app-{abi}-release-unsigned.apk"));
            let aligned = out_dir.join(format!("app-{abi}-aligned.apk"));
            run(Command::new(tools.join("zipalign")).args(["-f", "-p", "4"]).arg(&unsigned).arg(&aligned), "zipalign")?;
            let apk = dist.join(format!("{}-{}-android-{abi}.apk", app.id, app.version));
            run(
                Command::new(tools.join("apksigner"))
                    .args(["sign", "--ks"])
                    .arg(&sign.keystore)
                    .args(["--ks-key-alias", &sign.alias, "--ks-pass", &sign.pass.apksigner()])
                    .args(sign.key_pass.iter().flat_map(|p| ["--key-pass".to_string(), p.apksigner()]))
                    .arg("--out")
                    .arg(&apk)
                    .arg(&aligned)
                    .env("JAVA_HOME", &jdk)
                    .env("PATH", format!("{}:/usr/bin:/bin", jdk.join("bin").display())),
                "apksigner",
            )?;
            _ = std::fs::remove_file(dist.join(format!("{}-{}-android-{abi}.apk.idsig", app.id, app.version)));
            let bytes = std::fs::metadata(&apk).map_err(|e| e.to_string())?.len();
            if let Some(why) = over_budget(bytes, BUDGET_MB) {
                return Err(format!("{}: {why}", apk.display()));
            }
            apks.push(apk);
        }
        if cfg.aab {
            let aab = dist.join(format!("{}-{}-android.aab", app.id, app.version));
            sign_aab(&jdk, &sign, &proj.join("app/build/outputs/bundle/release/app-release.aab"), &aab)?;
            apks.push(aab);
        }
        Ok(apks)
    })();
    _ = std::fs::remove_dir_all(&proj);
    made
}

/// Gradle's `unsigned` App Bundle signed with the release key into `out`,
/// as Play wants it (jarsigner: an `.aab` is a signed jar).
fn sign_aab(jdk: &Path, sign: &Signing, unsigned: &Path, out: &Path) -> Result<(), String> {
    run(
        Command::new(jdk.join("bin/jarsigner"))
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
            let files = project(&app, cfg, b"png", &[], &[]).unwrap();
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
        let all = project(&app, &AndroidBuild::default(), b"png", &files, &[]).unwrap();
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
        assert_eq!(version_code("1.2.3"), Ok(1_002_003));
        assert!(version_code("0.10.0").unwrap() > version_code("0.9.9").unwrap());
        assert_eq!(version_code("1.2.3-rc.1"), Ok(1_002_003));
        assert!(version_code("1.2").is_err() && version_code("1.1000.0").is_err());
    }

    #[test]
    fn the_project_is_filled_in() {
        let app = App { id: "notes".into(), name: "A & B".into(), version: "1.2.3".into(), icon: None };
        let get = |files: &[(String, Vec<u8>)], p: &str| {
            files.iter().find(|(f, _)| f == p).map(|(_, t)| String::from_utf8_lossy(t).into_owned()).unwrap()
        };
        let files = project(&app, &AndroidBuild::default(), b"png", &[], &[]).unwrap();
        let gradle = get(&files, "app/build.gradle.kts");
        assert!(gradle.contains("applicationId = \"aio.rt.notes\"") && gradle.contains("versionCode = 1002003"));
        assert!(gradle.contains("include(\"arm64-v8a\", \"x86_64\")"));
        assert!(gradle.contains("useLegacyPackaging = true"), "the library deflated: half the APK");
        let manifest = get(&files, "app/src/main/AndroidManifest.xml");
        assert!(
            manifest.contains("android:label=\"A &amp; B\"")
                && manifest.contains("android:icon=\"@drawable/ic_launcher\"")
        );
        assert_eq!(get(&files, ICON), "png", "the launcher icon");
        assert!(files.iter().all(|(_, t)| !String::from_utf8_lossy(t).contains("{{")), "every placeholder filled");
        let own = AndroidBuild { application_id: Some("com.example.notes".into()), ..Default::default() };
        let gradle = get(&project(&app, &own, b"png", &[], &[]).unwrap(), "app/build.gradle.kts");
        assert!(gradle.contains("applicationId = \"com.example.notes\""), "{gradle}");
        assert_eq!(application_id_of("notes", &AndroidBuild::default()), "aio.rt.notes");
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
        let files = project(&app, &AndroidBuild::default(), b"png", &[], &overlay).unwrap();
        let of = |p: &str| files.iter().filter(|(f, _)| f == p).map(|(_, b)| b.clone()).collect::<Vec<_>>();
        assert_eq!(of("app/src/main/AndroidManifest.xml"), [b"<manifest label=\"Notes\"/>".to_vec()], "replaced, once");
        assert_eq!(of("app/src/main/res/drawable/logo.png"), [vec![0x89, 0xff, 0x00]], "bytes as they are");
        assert_eq!(files.len(), TEMPLATE.len() + 2, "the template, its icon, the added file");
        let own_icon = [("app/src/main/res/drawable/ic_launcher.png".to_string(), b"mine".to_vec())];
        let files = project(&app, &AndroidBuild::default(), b"png", &[], &own_icon).unwrap();
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
}

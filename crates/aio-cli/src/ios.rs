//! `ax build <app> --ios` (aio's `ios-client`): the Xcode project of a thin
//! client for iPhone and iPad, `dist/<id>-ios/`. Its one screen is a
//! WKWebView that opens a bundled page asking for the server (the app's
//! `[client] server` first, then the last ones used) and goes there; the
//! server's own page pairs the device ([`aio::http`]'s pairing page) and
//! serves the app. No Rust runs on the phone: the app's server runs
//! elsewhere. The project is written on any machine; building and signing
//! it needs Xcode (an Apple ID for a device). On a Mac with Xcode a
//! simulator build is made too. Without any of it, a phone uses the same
//! server page in Safari, added to its Home Screen.
use crate::build::App;
use std::path::{Path, PathBuf};

/// The line every generated `ViewController.swift` starts its doc with.
pub const SIGN: &str = "/// The aio-rs iOS client";

const APP_DELEGATE: &str = r#"import UIKit

@main
class AppDelegate: UIResponder, UIApplicationDelegate {
    var window: UIWindow?

    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil
    ) -> Bool {
        let window = UIWindow(frame: UIScreen.main.bounds)
        window.rootViewController = ViewController()
        window.makeKeyAndVisible()
        self.window = window
        return true
    }
}
"#;

const VIEW_CONTROLLER: &str = r##"import UIKit
import WebKit

/// The aio-rs iOS client: a WKWebView that opens the bundled page
/// (`www/index.html`), which asks for the app's server and goes there. The
/// server's own page pairs this device; its cookie keeps it paired. No Rust
/// runs here: the app's server runs elsewhere.
class ViewController: UIViewController, WKNavigationDelegate, WKUIDelegate {
    private var webView: WKWebView!
    private var index: URL!

    override func viewDidLoad() {
        super.viewDidLoad()
        let config = WKWebViewConfiguration()
        config.allowsInlineMediaPlayback = true
        // The page's storage (the servers used) and the server's cookie (the pairing) are kept.
        config.websiteDataStore = WKWebsiteDataStore.default()
        webView = WKWebView(frame: view.bounds, configuration: config)
        webView.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        webView.navigationDelegate = self
        webView.uiDelegate = self
        webView.allowsBackForwardNavigationGestures = true
        if #available(iOS 16.4, *) {
            #if DEBUG
            webView.isInspectable = true
            #endif
        }
        view.addSubview(webView)
        guard let index = Bundle.main.url(forResource: "index", withExtension: "html", subdirectory: "www") else {
            fatalError("www/index.html is missing from the bundle: write the project again with ax build --ios")
        }
        self.index = index
        webView.loadFileURL(index, allowingReadAccessTo: index.deletingLastPathComponent())
    }

    /// Back to the bundled page, saying why in its hash.
    private func showForm(_ why: String) {
        guard let url = URL(string: "#" + why, relativeTo: index) else { return }
        webView.loadFileURL(url.absoluteURL, allowingReadAccessTo: index.deletingLastPathComponent())
    }

    // A server that can't be reached: the form, saying so. A load this view
    // cancelled itself (a newer navigation) is not one.
    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        let e = error as NSError
        guard e.domain == NSURLErrorDomain, e.code != NSURLErrorCancelled else { return }
        guard let failed = e.userInfo[NSURLErrorFailingURLErrorKey] as? URL, !failed.isFileURL else { return }
        showForm("unreachable")
    }

    // Camera and microphone for a page that asks (a code scanner): iOS asks the person.
    @available(iOS 15.0, *)
    func webView(
        _ webView: WKWebView,
        requestMediaCapturePermissionFor origin: WKSecurityOrigin,
        initiatedByFrame frame: WKFrameInfo,
        type: WKMediaCaptureType,
        decisionHandler: @escaping (WKPermissionDecision) -> Void
    ) {
        decisionHandler(.prompt)
    }

    // A link the page opens in a new window stays in this view.
    func webView(
        _ webView: WKWebView,
        createWebViewWith configuration: WKWebViewConfiguration,
        for navigationAction: WKNavigationAction,
        windowFeatures: WKWindowFeatures
    ) -> WKWebView? {
        if navigationAction.targetFrame == nil {
            webView.load(navigationAction.request)
        }
        return nil
    }
}
"##;

const INFO_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>en</string>
	<key>CFBundleDisplayName</key>
	<string>{NAME}</string>
	<key>CFBundleExecutable</key>
	<string>$(EXECUTABLE_NAME)</string>
	<key>CFBundleIdentifier</key>
	<string>$(PRODUCT_BUNDLE_IDENTIFIER)</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>{NAME}</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>$(MARKETING_VERSION)</string>
	<key>CFBundleVersion</key>
	<string>$(CURRENT_PROJECT_VERSION)</string>
	<key>LSRequiresIPhoneOS</key>
	<true/>
	<key>UILaunchScreen</key>
	<dict/>
	<key>UISupportedInterfaceOrientations</key>
	<array>
		<string>UIInterfaceOrientationPortrait</string>
		<string>UIInterfaceOrientationLandscapeLeft</string>
		<string>UIInterfaceOrientationLandscapeRight</string>
	</array>
	<key>UISupportedInterfaceOrientations~ipad</key>
	<array>
		<string>UIInterfaceOrientationPortrait</string>
		<string>UIInterfaceOrientationPortraitUpsideDown</string>
		<string>UIInterfaceOrientationLandscapeLeft</string>
		<string>UIInterfaceOrientationLandscapeRight</string>
	</array>
	<key>NSCameraUsageDescription</key>
	<string>Used only when a page of this app asks to scan a code.</string>
	<key>NSMicrophoneUsageDescription</key>
	<string>Used only when a page of this app asks to record.</string>
	<key>NSLocalNetworkUsageDescription</key>
	<string>Connects to the app's server, which may be on your local network.</string>
	<key>NSAppTransportSecurity</key>
	<dict>
		<key>NSAllowsArbitraryLoads</key>
		<{ARBITRARY}/>
		<key>NSAllowsLocalNetworking</key>
		<true/>
	</dict>
</dict>
</plist>
"#;

const ASSETS: &str = "{\n  \"info\" : {\n    \"author\" : \"xcode\",\n    \"version\" : 1\n  }\n}\n";

const APP_ICON: &str = "{\n  \"images\" : [\n    {\n      \"filename\" : \"icon-1024.png\",\n      \"idiom\" : \"universal\",\n      \"platform\" : \"ios\",\n      \"size\" : \"1024x1024\"\n    }\n  ],\n  \"info\" : {\n    \"author\" : \"xcode\",\n    \"version\" : 1\n  }\n}\n";

/// The bundled page: the server to open (the baked one, the last used
/// first), and why it is shown again (its hash).
const INDEX: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
<meta name="aio-server" content="{SERVER}">
<meta name="color-scheme" content="light dark">
<title>{NAME}</title>
<style>
body { font: 17px/1.4 -apple-system, system-ui, sans-serif; margin: 0; min-height: 100vh; display: grid; place-items: center; }
main { width: min(22rem, 100% - 2rem); }
input, button { font: inherit; width: 100%; box-sizing: border-box; padding: .7rem; margin-top: .5rem; border-radius: 10px; }
input { border: 1px solid #8888; }
button { border: 0; background: #0a64d8; color: #fff; }
#why { color: #c4321f; min-height: 1.4em; }
li button { background: none; color: inherit; text-align: left; padding: .4rem 0; }
ul { list-style: none; padding: 0; }
</style>
</head>
<body>
<main>
<h1>{NAME}</h1>
<form id="f">
<label for="a">Server</label>
<input id="a" placeholder="192.168.1.50:8100" autocapitalize="off" autocorrect="off" spellcheck="false" inputmode="url" required>
<button type="submit">Connect</button>
</form>
<p id="why" role="alert"></p>
<ul id="recent"></ul>
</main>
<script>
"use strict";
const baked = document.querySelector("meta[name=aio-server]").content;
const saved = JSON.parse(localStorage.getItem("aio.servers") || "[]");
const say = (t) => { document.getElementById("why").textContent = t; };
const origin = (a) => {
  a = a.trim();
  if (!/^[a-z]+:\/\//i.test(a)) a = "http://" + a;
  return new URL(a.replace(/^ws(s?):/i, "http$1:")).origin;
};
function go(a) {
  let u;
  try { u = origin(a); } catch { return say("Not an address: " + a); }
  localStorage.setItem("aio.servers", JSON.stringify([u, ...saved.filter((s) => s !== u)].slice(0, 5)));
  location.href = u + "/";
}
document.getElementById("f").addEventListener("submit", (e) => { e.preventDefault(); go(document.getElementById("a").value); });
for (const s of saved) {
  const b = Object.assign(document.createElement("button"), { type: "button", textContent: s });
  b.addEventListener("click", () => go(s));
  const li = document.createElement("li");
  li.append(b);
  document.getElementById("recent").append(li);
}
document.getElementById("a").value = saved[0] || baked;
const why = { unreachable: "Can't reach that server: is it running, and this device on its network?" }[location.hash.slice(1)];
if (why) say(why);
else if (saved[0] || baked) go(saved[0] || baked);
</script>
</body>
</html>
"#;

/// The Xcode project (one target, `App`; Xcode 15's format).
const PBXPROJ: &str = include_str!("ios.pbxproj");

/// `MARKETING_VERSION` for build version `v`: Apple takes one to three
/// numbers and nothing else (`1.2.3-beta` → `1.2.3`) (pure).
pub fn marketing(v: &str) -> String {
    let nums: Vec<&str> =
        v.split(['-', '+']).next().unwrap_or("").split('.').take(3).take_while(|n| n.parse::<u32>().is_ok()).collect();
    if nums.is_empty() { "0".into() } else { nums.join(".") }
}

/// Text inside a plist `<string>` or the page's HTML (pure).
fn xml(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() || *c == '\t' || *c == '\n')
        .collect::<String>()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The project's files for `app` as `bundle_id`, its build number `code`
/// and its baked server `server` (none: the person types one) (pure). A
/// baked `https://` server keeps App Transport Security strict; else any
/// address the person types must load (a LAN server is plain http).
pub fn files(app: &App, bundle_id: &str, code: u32, server: Option<&str>) -> Vec<(String, String)> {
    let name = xml(&app.name);
    let arbitrary = if server.is_some_and(|s| s.starts_with("https://")) { "false" } else { "true" };
    let project = PBXPROJ
        .replace("{BUNDLE_ID}", bundle_id)
        .replace("{VERSION}", &marketing(&app.version))
        .replace("{CODE}", &code.to_string());
    [
        ("App.xcodeproj/project.pbxproj", project),
        ("App/AppDelegate.swift", APP_DELEGATE.into()),
        ("App/ViewController.swift", VIEW_CONTROLLER.into()),
        ("App/Info.plist", INFO_PLIST.replace("{NAME}", &name).replace("{ARBITRARY}", arbitrary)),
        ("App/Assets.xcassets/Contents.json", ASSETS.into()),
        ("App/Assets.xcassets/AppIcon.appiconset/Contents.json", APP_ICON.into()),
        ("App/www/index.html", INDEX.replace("{NAME}", &name).replace("{SERVER}", &xml(server.unwrap_or("")))),
    ]
    .into_iter()
    .map(|(p, t)| (p.to_string(), t))
    .collect()
}

/// `ax build <app> --ios`: the project in `dist/<id>-ios/` (one an earlier
/// build wrote is replaced, never another folder); on a Mac with Xcode, a
/// simulator build of it too. What was made, and the folder.
pub fn build(name: &str) -> Result<(String, PathBuf), String> {
    let meta = crate::dev::metadata()?;
    let pkg = crate::dev::locate(&meta, name)?;
    let (app, cfg) = crate::build::app_of(&pkg)?;
    let bundle_id = cfg.mac_bundle_id();
    if !aio::config::bundle_id_ok(&bundle_id) {
        return Err(format!(
            "{bundle_id} is no bundle id (letters, digits and -, two or more dot-separated parts): set [build.macos] bundle_id"
        ));
    }
    let root = meta["workspace_root"].as_str().ok_or("cargo metadata: no workspace_root")?;
    let out = crate::build::dist(Path::new(root), &cfg).join(format!("{}-ios", app.id));
    let mark = out.join("App/ViewController.swift");
    if out.exists() && !std::fs::read_to_string(&mark).is_ok_and(|s| s.contains(SIGN)) {
        return Err(format!("{}: a folder ax build --ios didn't write: move it away", out.display()));
    }
    _ = std::fs::remove_dir_all(&out);
    let code = crate::android::version_code(&app.version)?;
    let e = |p: &Path, e: std::io::Error| format!("{}: {e}", p.display());
    for (rel, text) in files(&app, &bundle_id, code, cfg.client.server.as_deref()) {
        let to = out.join(rel);
        std::fs::create_dir_all(to.parent().ok_or("no folder")?).map_err(|x| e(&to, x))?;
        std::fs::write(&to, text).map_err(|x| e(&to, x))?;
    }
    // The App Store's 1024 px icon: the app's own, else its monogram, opaque.
    let own = app.icon.as_ref().map(|p| std::fs::read(p).map_err(|x| e(p, x))).transpose()?;
    let icon = crate::icon::ios_icon(own.as_deref(), crate::build::label(&app), cfg.hue())?;
    let to = out.join("App/Assets.xcassets/AppIcon.appiconset/icon-1024.png");
    std::fs::write(&to, icon).map_err(|x| e(&to, x))?;
    let said = format!(
        "{} (Xcode project, {bundle_id} {}): open App.xcodeproj in Xcode on a Mac, pick your team, run it on a \
         phone or Archive it; without Xcode, open the server's address in Safari and add it to the Home Screen",
        out.display(),
        marketing(&app.version)
    );
    Ok((simulator(&out).map_or(said.clone(), |s| format!("{said}\n{s}")), out))
}

/// On a Mac with Xcode's iOS simulator SDK: the project built for it,
/// unsigned (a device build is signed by its developer's team in Xcode).
fn simulator(project: &Path) -> Option<String> {
    let sdk = std::process::Command::new("xcrun").args(["--sdk", "iphonesimulator", "--show-sdk-path"]).output();
    if !cfg!(target_os = "macos") || !sdk.is_ok_and(|o| o.status.success()) {
        return None;
    }
    let derived = project.with_extension("derived");
    let o = std::process::Command::new("xcodebuild")
        .arg("-project")
        .arg(project.join("App.xcodeproj"))
        .args(["-scheme", "App", "-configuration", "Release", "-sdk", "iphonesimulator", "-derivedDataPath"])
        .arg(&derived)
        .args(["CODE_SIGNING_ALLOWED=NO", "build"])
        .output();
    Some(match o {
        Ok(o) if o.status.success() => {
            format!(
                "{} (simulator, unsigned)",
                derived.join("Build/Products/Release-iphonesimulator/App.app").display()
            )
        }
        Ok(o) => format!("xcodebuild failed: {}", String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or("")),
        Err(e) => format!("xcodebuild: {e}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        App { id: "notes".into(), name: "Notes & <Co>".into(), version: "1.2.3-beta.4".into(), icon: None }
    }

    #[test]
    fn the_project_is_complete_named_and_versioned_for_xcode() {
        let f = files(&app(), "aio.rt.notes", 102000003, None);
        let get = |p: &str| f.iter().find(|(n, _)| n == p).map(|(_, t)| t.as_str()).unwrap();
        let names: Vec<&str> = f.iter().map(|(n, _)| n.as_str()).collect();
        for p in
            ["App.xcodeproj/project.pbxproj", "App/AppDelegate.swift", "App/ViewController.swift", "App/www/index.html"]
        {
            assert!(names.contains(&p), "{p}");
        }
        let pbx = get("App.xcodeproj/project.pbxproj");
        assert!(
            pbx.contains("PRODUCT_BUNDLE_IDENTIFIER = \"aio.rt.notes\";")
                && pbx.contains("MARKETING_VERSION = \"1.2.3\";")
        );
        assert!(pbx.contains("CURRENT_PROJECT_VERSION = 102000003;"), "{pbx}");
        assert!(!pbx.contains("{BUNDLE_ID}") && !pbx.contains("{VERSION}") && !pbx.contains("{CODE}"));
        let plist = get("App/Info.plist");
        assert!(
            plist.contains("<string>Notes &amp; &lt;Co&gt;</string>")
                && plist.contains("<true/>\n\t\t<key>NSAllowsLocal")
        );
        assert!(get("App/ViewController.swift").contains(SIGN));
        let page = get("App/www/index.html");
        assert!(
            page.contains(r#"<meta name="aio-server" content="">"#)
                && page.contains("<title>Notes &amp; &lt;Co&gt;</title>")
        );
        // A baked https server: App Transport Security stays strict.
        let f = files(&app(), "aio.rt.notes", 1, Some("https://notes.example.org"));
        let plist = &f.iter().find(|(n, _)| n == "App/Info.plist").unwrap().1;
        assert!(plist.contains("<key>NSAllowsArbitraryLoads</key>\n\t\t<false/>"), "{plist}");
    }

    #[test]
    fn the_version_xcode_shows_is_its_numbers_only() {
        assert_eq!(marketing("1.2.3-beta.4"), "1.2.3");
        assert_eq!(marketing("0.1.0+git.9f3ac2b1"), "0.1.0");
        assert_eq!(marketing("2.5"), "2.5");
        assert_eq!(marketing("1.2.3.4"), "1.2.3");
        assert_eq!(marketing("dev"), "0");
    }

    /// On a Mac, the Swift is type-checked against UIKit and WebKit as Mac
    /// Catalyst has them (in the Command Line Tools' SDK: no Xcode needed).
    #[cfg(target_os = "macos")]
    #[test]
    fn the_swift_sources_typecheck_against_uikit_and_webkit() {
        let d = tempfile::tempdir().unwrap();
        let swift: Vec<PathBuf> = files(&app(), "aio.rt.notes", 1, None)
            .into_iter()
            .filter(|(n, _)| n.ends_with(".swift"))
            .map(|(n, t)| {
                let p = d.path().join(n.rsplit('/').next().unwrap());
                std::fs::write(&p, t).unwrap();
                p
            })
            .collect();
        let sdk = std::process::Command::new("xcrun").arg("--show-sdk-path").output().unwrap();
        let sdk = PathBuf::from(String::from_utf8(sdk.stdout).unwrap().trim());
        let ios = sdk.join("System/iOSSupport");
        let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "x86_64" };
        let o = std::process::Command::new("swiftc")
            .args(["-typecheck", "-target", &format!("{arch}-apple-ios15.4-macabi"), "-sdk"])
            .arg(&sdk)
            .arg("-Fsystem")
            .arg(ios.join("System/Library/Frameworks"))
            .arg("-I")
            .arg(ios.join("usr/lib/swift"))
            .args(&swift)
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }
}

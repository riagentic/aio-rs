// A headless Chromium a test owns (aio's `testBrowser`, `testing/chromium`):
// found (`$CHROMIUM_BIN`, `$CHROME_BIN`, else the well-known installs of
// Chromium, Chrome, Edge and Brave on this OS), launched on a profile of its
// own with a DevTools port, and when dropped killed and its profile removed.
// Drive its page over the DevTools protocol at `port()`; this only owns the
// process. It gets a HOME of its own too (Linux, Windows), inside the
// profile, so nothing it writes (NSS's `~/.pki`, caches, crash settings)
// lands in the person's home; the person's fonts still show. On Linux it never reaches the
// person's session: no display and no session D-Bus, so no desktop portal
// (file pickers, print, settings) can answer it or draw on their screen.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long a browser may take to open its DevTools port (a cold start on a
/// loaded machine took more than 10 s in aio).
const START: Duration = Duration::from_secs(30);

/// A launched headless Chromium; dropped, it is killed and its profile goes.
pub struct TestBrowser {
    child: std::process::Child,
    port: u16,
    profile: PathBuf,
}

impl TestBrowser {
    /// A headless Chromium on `url` ([`TestBrowser::find`]'s), once its
    /// DevTools port is open.
    pub fn open(url: &str) -> Result<Self, String> {
        Self::launch(&Self::find()?, &[url])
    }

    /// The browser to launch: `$CHROMIUM_BIN`, `$CHROME_BIN`, else the
    /// first installed one; refused naming the setting that points nowhere,
    /// or what to do when there is none.
    pub fn find() -> Result<String, String> {
        let file = |p: &Path| p.is_file();
        chosen(&|k| std::env::var(k).ok(), std::env::consts::OS, &file)
    }

    /// `bin` headless with `args` (the URL last) on a fresh profile.
    pub fn launch(bin: &str, args: &[&str]) -> Result<Self, String> {
        // Chromium binds its singleton socket in `TMPDIR`: the profile, in a
        // short temp dir, so a long `TMPDIR` can't fail it and it goes too.
        let id = crate::uuid_v4().simple().to_string();
        let profile = crate::paths::short_temp().join(format!("aio-test-browser-{}", &id[..12]));
        std::fs::create_dir_all(&profile).map_err(|e| format!("{}: {e}", profile.display()))?;
        let mut cmd = std::process::Command::new(bin);
        cmd.args(flags(&profile, std::env::consts::OS)).args(args);
        cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        for (k, v) in isolated(std::env::consts::OS) {
            match v {
                Some(v) => cmd.env(k, v),
                None => cmd.env_remove(k),
            };
        }
        cmd.envs(own_home(&profile, std::env::consts::OS, &|k| std::env::var(k).ok())?).env("TMPDIR", &profile);
        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                _ = std::fs::remove_dir_all(&profile);
                return Err(format!("[testBrowser] {bin} did not start: {e}"));
            }
        };
        let mut b = Self { child, port: 0, profile };
        let end = Instant::now() + START;
        let file = b.profile.join("DevToolsActivePort");
        while Instant::now() < end {
            if let Some(p) = std::fs::read_to_string(&file).ok().and_then(|t| t.lines().next()?.parse().ok()) {
                b.port = p;
                return Ok(b);
            }
            if let Ok(Some(s)) = b.child.try_wait() {
                return Err(format!("[testBrowser] {bin} exited ({s}) before it opened its DevTools port"));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        Err(format!("[testBrowser] {bin} opened no DevTools port in {START:?}"))
    }

    /// Its DevTools port on 127.0.0.1 (`/json/list`: its pages).
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Its profile folder (removed when it is dropped).
    pub fn profile(&self) -> &Path {
        &self.profile
    }
}

impl Drop for TestBrowser {
    fn drop(&mut self) {
        _ = self.child.kill();
        _ = self.child.wait();
        // A helper that outlives the browser can write into the profile
        // after it went (aio measured it): gone only once it stays gone.
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            _ = std::fs::remove_dir_all(&self.profile);
            std::thread::sleep(Duration::from_millis(100));
            if !self.profile.exists() || Instant::now() > end {
                break;
            }
        }
    }
}

/// Where a Chromium-family browser is installed on `os`, in the order they
/// are tried (pure; `env` gives Windows its install roots).
fn candidates(os: &str, env: &dyn Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    match os {
        "windows" => {
            let roots: Vec<String> =
                ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"].iter().filter_map(|r| env(r)).collect();
            [
                r"Chromium\Application\chrome.exe",
                r"Google\Chrome\Application\chrome.exe",
                r"Microsoft\Edge\Application\msedge.exe",
                r"BraveSoftware\Brave-Browser\Application\brave.exe",
            ]
            .iter()
            .flat_map(|rel| roots.iter().map(move |r| PathBuf::from(format!(r"{r}\{rel}"))))
            .collect()
        }
        "macos" => [
            "Google Chrome.app/Contents/MacOS/Google Chrome",
            "Chromium.app/Contents/MacOS/Chromium",
            "Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "Brave Browser.app/Contents/MacOS/Brave Browser",
        ]
        .iter()
        .map(|rel| Path::new("/Applications").join(rel))
        .collect(),
        _ => [
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
            "/usr/bin/google-chrome",
            "/usr/bin/google-chrome-stable",
            "/snap/bin/chromium",
            "/usr/bin/microsoft-edge",
            "/usr/bin/microsoft-edge-stable",
            "/usr/bin/brave-browser",
        ]
        .iter()
        .map(PathBuf::from)
        .collect(),
    }
}

/// The browser to launch (pure but for `is_file`): a setting first, checked
/// now (a typo named at the call, not as a failed spawn later); a bare
/// command name is looked up on `PATH`.
fn chosen(env: &dyn Fn(&str) -> Option<String>, os: &str, is_file: &dyn Fn(&Path) -> bool) -> Result<String, String> {
    let fix = "install one or set $CHROMIUM_BIN";
    let set = ["CHROMIUM_BIN", "CHROME_BIN"].into_iter().find_map(|k| env(k).filter(|v| !v.is_empty()).map(|v| (k, v)));
    let Some((key, bin)) = set else {
        return candidates(os, env)
            .into_iter()
            .find(|c| is_file(c))
            .map(|c| c.display().to_string())
            .ok_or_else(|| format!("[testBrowser] no headless Chromium/Chrome found: {fix}"));
    };
    let found = if bin.contains(['/', '\\']) {
        is_file(Path::new(&bin))
    } else {
        let exts: &[&str] = if os == "windows" { &["", ".exe"] } else { &[""] };
        let sep = if os == "windows" { ';' } else { ':' };
        let path = env("PATH").unwrap_or_default();
        path.split(sep)
            .filter(|d| !d.is_empty())
            .any(|d| exts.iter().any(|x| is_file(&Path::new(d).join(format!("{bin}{x}")))))
    };
    let missing = if bin.contains(['/', '\\']) { "does not exist" } else { "is not found on PATH" };
    if found {
        Ok(bin)
    } else {
        Err(format!("[testBrowser] no headless Chromium/Chrome at {bin}: ${key}={bin} {missing}: {fix}"))
    }
}

/// The variables a test browser on `os` gets (`None`: removed), pure: on
/// Linux no display and a session bus that leads nowhere, whatever the test
/// runs under (a person's desktop, an agent's shell, CI).
fn isolated(os: &str) -> Vec<(&'static str, Option<&'static str>)> {
    match os {
        "linux" => {
            vec![("DISPLAY", None), ("WAYLAND_DISPLAY", None), ("DBUS_SESSION_BUS_ADDRESS", Some("disabled:"))]
        }
        _ => vec![],
    }
}

/// The flags every test browser is launched with, before the caller's own
/// (pure). Windows: `--guest`, or each launch of Edge on a fresh profile
/// tries a logon as the Windows user that fails, and ten lock the account;
/// `--do-not-de-elevate`, or from an elevated shell it relaunches itself
/// and the process started exits at once (both measured in aio).
fn flags(profile: &Path, os: &str) -> Vec<String> {
    let own: &[&str] = match os {
        "windows" => &["--guest", "--do-not-de-elevate"],
        "linux" => &["--ozone-platform=headless"],
        _ => &[],
    };
    let all = [
        "--headless=new",
        "--no-sandbox",
        "--disable-gpu",
        "--disable-dev-shm-usage",
        "--no-first-run",
        "--no-default-browser-check",
        "--password-store=basic",
        "--use-mock-keychain",
        "--remote-debugging-port=0",
    ];
    own.iter()
        .chain(&all)
        .map(|f| f.to_string())
        .chain([format!("--user-data-dir={}", profile.display())])
        .collect()
}

/// A HOME of its own inside `profile` (made here), for `os`; the user's
/// fonts still found (Linux: a fontconfig file that includes the one in
/// force and the real user's font folders). None on macOS, where a browser
/// writes Crashpad settings and its plist in the account's home whatever
/// HOME says (moving HOME there only loses the fonts).
pub(super) fn own_home(
    profile: &Path,
    os: &str,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Vec<(String, String)>, String> {
    let home = profile.join("aio-home");
    let mk = |p: &Path| std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()));
    match os {
        "windows" => {
            let vars = windows_home(&home);
            mk(Path::new(&vars[1].1))?;
            mk(Path::new(&vars[3].1))?;
            Ok(vars)
        }
        "linux" => {
            mk(&home)?;
            let (vars, conf) = linux_home(&home, env);
            std::fs::write(home.join("fonts.conf"), conf).map_err(|e| e.to_string())?;
            Ok(vars)
        }
        _ => Ok(vec![]),
    }
}

/// Windows' per-user folders moved into `home` (pure).
fn windows_home(home: &Path) -> Vec<(String, String)> {
    let h = home.display();
    let local = format!(r"{h}\AppData\Local");
    [
        ("USERPROFILE", h.to_string()),
        ("APPDATA", format!(r"{h}\AppData\Roaming")),
        ("LOCALAPPDATA", local.clone()),
        ("TEMP", format!(r"{local}\Temp")),
        ("TMP", format!(r"{local}\Temp")),
    ]
    .map(|(k, v)| (k.to_string(), v))
    .to_vec()
}

/// Linux: HOME and the XDG folders in `home`, and the fontconfig file that
/// keeps the real user's fonts (pure).
fn linux_home(home: &Path, env: &dyn Fn(&str) -> Option<String>) -> (Vec<(String, String)>, String) {
    let h = home.display();
    let real = env("HOME").unwrap_or_default();
    let or = |k: &str, d: String| env(k).filter(|v| !v.is_empty()).unwrap_or(d);
    let (config, data) =
        (or("XDG_CONFIG_HOME", format!("{real}/.config")), or("XDG_DATA_HOME", format!("{real}/.local/share")));
    let xml = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let include = |p: &str| format!("  <include ignore_missing=\"yes\">{}</include>\n", xml(p));
    let mut conf = format!(
        "<?xml version=\"1.0\"?>\n<fontconfig>\n{}",
        include(&or("FONTCONFIG_FILE", "/etc/fonts/fonts.conf".into()))
    );
    if !real.is_empty() {
        conf += &include(&format!("{config}/fontconfig/conf.d"));
        conf += &include(&format!("{config}/fontconfig/fonts.conf"));
        conf += &format!(
            "  <dir>{}</dir>\n  <dir>{}</dir>\n",
            xml(&format!("{data}/fonts")),
            xml(&format!("{real}/.fonts"))
        );
    }
    conf += "</fontconfig>\n";
    let mut vars: Vec<(String, String)> = [
        ("HOME", h.to_string()),
        ("XDG_CONFIG_HOME", format!("{h}/.config")),
        ("XDG_DATA_HOME", format!("{h}/.local/share")),
        ("XDG_CACHE_HOME", format!("{h}/.cache")),
        ("FONTCONFIG_FILE", format!("{h}/fonts.conf")),
    ]
    .map(|(k, v)| (k.to_string(), v))
    .to_vec();
    // Xlib finds its cookie through HOME when XAUTHORITY isn't set.
    if !real.is_empty() && env("XAUTHORITY").is_none() {
        vars.push(("XAUTHORITY".into(), format!("{real}/.Xauthority")));
    }
    (vars, conf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn windows_is_probed_under_all_three_roots_edge_included() {
        let e = env(&[("ProgramFiles", r"C:\PF"), ("ProgramFiles(x86)", r"C:\PF86"), ("LOCALAPPDATA", r"C:\U\L")]);
        let c: Vec<String> = candidates("windows", &e).iter().map(|p| p.display().to_string()).collect();
        assert_eq!(c.len(), 12);
        assert_eq!(c[0], r"C:\PF\Chromium\Application\chrome.exe");
        assert!(c.contains(&r"C:\U\L\Microsoft\Edge\Application\msedge.exe".to_string()));
        assert!(c.contains(&r"C:\PF86\BraveSoftware\Brave-Browser\Application\brave.exe".to_string()));
        let mac = candidates("macos", &e);
        assert!(mac.iter().any(|p| p.ends_with("Microsoft Edge.app/Contents/MacOS/Microsoft Edge")));
        let linux = candidates("linux", &e);
        assert_eq!(linux[0], Path::new("/usr/bin/chromium"));
        assert!(
            linux.iter().any(|p| p.ends_with("brave-browser")) && linux.iter().any(|p| p.ends_with("microsoft-edge"))
        );
    }

    #[test]
    fn a_setting_wins_and_one_that_points_nowhere_is_named() {
        let all = |_: &Path| true;
        let none = |_: &Path| false;
        let set = env(&[("CHROMIUM_BIN", "/opt/c/chrome"), ("CHROME_BIN", "/x")]);
        assert_eq!(chosen(&set, "linux", &all), Ok("/opt/c/chrome".into()), "it wins over every probed path");
        let e = chosen(&set, "linux", &none).unwrap_err();
        assert!(e.contains("$CHROMIUM_BIN=/opt/c/chrome does not exist"), "{e}");
        let chrome = env(&[("CHROME_BIN", "/nope/chrome")]);
        assert!(chosen(&chrome, "linux", &none).unwrap_err().contains("$CHROME_BIN=/nope/chrome"));
        assert!(chosen(&env(&[]), "linux", &none).unwrap_err().contains("no headless Chromium/Chrome found"));
        let first = |p: &Path| p == Path::new("/usr/bin/google-chrome");
        assert_eq!(chosen(&env(&[]), "linux", &first), Ok("/usr/bin/google-chrome".into()));
    }

    #[test]
    fn a_bare_name_is_looked_up_on_path() {
        let e = env(&[("CHROMIUM_BIN", "mychrome"), ("PATH", "/a:/b")]);
        let on_b = |p: &Path| p == Path::new("/b/mychrome");
        assert_eq!(chosen(&e, "linux", &on_b), Ok("mychrome".into()));
        let in_cwd = |p: &Path| p == Path::new("mychrome");
        assert!(chosen(&e, "linux", &in_cwd).unwrap_err().contains("is not found on PATH"), "not the cwd's");
        let w = env(&[("CHROMIUM_BIN", "msedge"), ("PATH", r"C:\E;C:\F")]);
        let exe = |p: &Path| p == Path::new(r"C:\F").join("msedge.exe");
        assert_eq!(chosen(&w, "windows", &exe), Ok("msedge".into()));
    }

    #[test]
    fn windows_launches_a_guest_kept_elevated_other_hosts_do_not() {
        let p = Path::new("/tmp/p");
        let w = flags(p, "windows");
        assert_eq!(w[..2], ["--guest", "--do-not-de-elevate"]);
        let l = flags(p, "linux");
        assert!(!l.iter().any(|f| f == "--guest" || f == "--do-not-de-elevate"));
        assert!(l.contains(&"--ozone-platform=headless".into()), "no display server at all");
        assert_eq!(
            isolated("linux"),
            [("DISPLAY", None), ("WAYLAND_DISPLAY", None), ("DBUS_SESSION_BUS_ADDRESS", Some("disabled:"))],
            "never the person's screen or bus (no portal)"
        );
        assert!(isolated("windows").is_empty() && isolated("macos").is_empty());
        for f in [&w, &l] {
            assert!(f.contains(&"--headless=new".into()) && f.contains(&"--remote-debugging-port=0".into()));
            assert_eq!(f.last().unwrap(), "--user-data-dir=/tmp/p");
        }
    }

    #[test]
    fn its_own_home_keeps_the_users_fonts_and_moves_windows_folders() {
        let e = env(&[("HOME", "/home/u"), ("FONTCONFIG_FILE", "/etc/my.conf")]);
        let (vars, conf) = linux_home(Path::new("/p/aio-home"), &e);
        let get = |k: &str| vars.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        assert_eq!(get("HOME"), Some("/p/aio-home"));
        assert_eq!(get("FONTCONFIG_FILE"), Some("/p/aio-home/fonts.conf"));
        assert_eq!(get("XAUTHORITY"), Some("/home/u/.Xauthority"), "Xlib's cookie still found");
        assert!(conf.contains("<include ignore_missing=\"yes\">/etc/my.conf</include>"), "the one in force: {conf}");
        assert!(
            conf.contains("/home/u/.config/fontconfig/conf.d")
                && conf.contains("<dir>/home/u/.local/share/fonts</dir>")
        );
        let (vars, _) = linux_home(Path::new("/p"), &env(&[("HOME", "/h"), ("XAUTHORITY", "/x")]));
        assert!(!vars.iter().any(|(k, _)| k == "XAUTHORITY"), "one already in force wins");
        let w = windows_home(Path::new(r"C:\p\aio-home"));
        assert_eq!(w[0], ("USERPROFILE".into(), r"C:\p\aio-home".into()));
        assert_eq!(w[3], ("TEMP".into(), r"C:\p\aio-home\AppData\Local\Temp".into()));
        assert_eq!(own_home(Path::new("/nowhere"), "macos", &e), Ok(vec![]), "macOS keeps the account's");
    }

    /// A real one (skipped without one): its port answers, and dropped, the
    /// browser and its profile are gone.
    #[test]
    fn a_launched_browser_has_a_port_and_leaves_nothing() {
        let Ok(bin) = TestBrowser::find() else { return };
        assert!(TestBrowser::launch("/nonexistent/chrome", &[]).err().unwrap().contains("did not start"));
        let b = TestBrowser::launch(&bin, &["about:blank"]).unwrap_or_else(|e| panic!("{e}"));
        assert!(std::net::TcpStream::connect(("127.0.0.1", b.port())).is_ok());
        let profile = b.profile().to_path_buf();
        assert!(profile.is_dir());
        drop(b);
        assert!(!profile.exists(), "its profile went with it");
    }
}

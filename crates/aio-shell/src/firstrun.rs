//! The first-run runtime download on screen: "Getting `<title>` ready"
//! with a bar, what comes down and a Cancel while Electron downloads, and a
//! Retry / Quit question when that fails (aio's `runtime-dialog.ts`). Electron
//! can't draw it (it is what is coming), so each OS draws it with what it
//! always has: Linux zenity, else yad; macOS JavaScript for Automation over
//! Cocoa; Windows PowerShell's Windows Forms; a Linux desktop with neither
//! zenity nor yad, a page in the default browser (`firstpage`,
//! `AIORT_DIALOG=browser` forces it). With no desktop (a service, an
//! ssh session, `AIORT_DIALOG=none`) the log alone carries it and a failure
//! ends the start.
//!
//! Every window is in the app's colours ([`Look`], light or dark as the OS
//! is; zenity and yad through GTK's own stylesheet), and the Windows and
//! macOS ones open over the app's window as it was last placed, sharp at
//! any DPI. On the first start after an update, while the version it
//! replaced is still installed, the question also offers to start that one
//! ([`Answer::Previous`], aio's "Start the previous version").
//!
//! zenity and yad are fed on stdin; the macOS and Windows helpers poll a
//! progress file (`<pct>\t<text>`, or `close`) and write `shown` once on
//! screen. A helper that exits before it is told to is the user closing it,
//! a cancel, if its window showed; else a window that never came up, and the
//! download goes on without one. Each helper also closes itself once the
//! app's process is gone. Text reaches them as argv or environment, never
//! spliced into script source.
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// The failure question's answers. `Previous`: the version an update
/// replaced, started in this one's place (offered on a first boot only).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Answer {
    Retry,
    Quit,
    Previous,
}

/// The third answer's words (aio's).
pub(crate) const PREVIOUS: &str = "Start the previous version";

/// How the windows look: the app's colours (`splash::colors`: `[light,
/// dark]`, each background, text, bar, track) and the middle of its window
/// as last placed, in Electron's screen points, to open over it. Its `icon`:
/// the app's PNG, on each window (title bar, taskbar, Dock).
pub(crate) struct Look<'a> {
    pub(crate) colors: &'a [[String; 4]; 2],
    pub(crate) at: Option<(i32, i32)>,
    /// The splash shown while Electron downloads (`[build.splash]`): its PNG
    /// and its size on screen in px; unset: the plain download window.
    pub(crate) image: Option<(&'a [u8], (u32, u32))>,
    pub(crate) icon: Option<&'a Path>,
}

/// `#rgb` as `#rrggbb` (the helpers read six digits).
fn hex6(c: &str) -> String {
    match c.strip_prefix('#') {
        Some(h) if h.len() == 3 => h.chars().fold("#".into(), |s, x| format!("{s}{x}{x}")),
        _ => c.into(),
    }
}

impl Look<'_> {
    /// `AIO_DLG_LIGHT` / `AIO_DLG_DARK`: `"#bg #text #bar #track"`.
    pub(crate) fn env(&self) -> [String; 2] {
        self.colors.clone().map(|c| c.iter().map(|x| hex6(x)).collect::<Vec<_>>().join(" "))
    }
    /// `AIO_DLG_ICON`: the icon's path, or empty (the helper's own).
    pub(crate) fn icon(&self) -> &std::ffi::OsStr {
        self.icon.map_or("".as_ref(), Path::as_os_str)
    }
    /// `AIO_DLG_AT`: `"x y"`, or empty (the screen's middle).
    pub(crate) fn at(&self) -> String {
        self.at.map_or(String::new(), |(x, y)| format!("{x} {y}"))
    }
}

/// GTK's own stylesheet, for zenity and yad: the window, its text and the
/// bar in one mode's colours (background, text, bar, track). A button not
/// yet usable (zenity's OK until the download is done) keeps looking so.
pub(crate) fn gtk_css([bg, text, bar, track]: &[String; 4]) -> String {
    format!(
        "window, dialog, .background, box {{ background-color: {bg}; color: {text}; }}\n\
         label {{ color: {text}; }}\n\
         button:disabled label {{ color: alpha({text}, 0.4); }}\n\
         progressbar trough {{ background-color: {track}; border-color: {track}; }}\n\
         progressbar progress {{ background-color: {bar}; border-color: {bar}; }}\n"
    )
}

/// The Linux desktop is dark: `GTK_THEME` ends in `:dark`, or GNOME's
/// colour scheme (`scheme`, as `gsettings` says it) prefers dark.
pub(crate) fn linux_dark(gtk_theme: Option<&str>, scheme: &str) -> bool {
    gtk_theme.is_some_and(|t| t.ends_with(":dark")) || scheme.contains("prefer-dark")
}

/// A config dir that holds only GTK's stylesheet in the app's colours, for
/// zenity or yad (`XDG_CONFIG_HOME`); GTK 3 and 4 both read it.
fn gtk_config(look: &Look) -> io::Result<PathBuf> {
    let scheme = Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "color-scheme"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let dark = linux_dark(std::env::var("GTK_THEME").ok().as_deref(), &scheme);
    let dir = helper_dir()?;
    for v in ["gtk-3.0", "gtk-4.0"] {
        std::fs::create_dir_all(dir.join(v))?;
        std::fs::write(dir.join(v).join("gtk.css"), gtk_css(&look.colors[dark as usize]))?;
    }
    Ok(dir)
}

/// How this machine draws the windows.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Backend {
    /// zenity, or yad (`yad`), at `bin`.
    Zenity {
        bin: PathBuf,
        yad: bool,
    },
    Cocoa,
    Forms,
    /// A page in the browser `opener` (`xdg-open`) opens.
    Browser {
        opener: PathBuf,
    },
}

/// The backend for `os`, or `None`: no desktop to draw on, or Linux with
/// neither zenity, yad nor `xdg-open` (`find`: a program on PATH).
pub(crate) fn here(
    os: &str,
    env: &dyn Fn(&str) -> Option<String>,
    find: &dyn Fn(&str) -> Option<PathBuf>,
) -> Option<Backend> {
    if env("AIORT_DIALOG").is_some_and(|v| v == "none") || crate::os::desktop(os, env).is_err() {
        return None;
    }
    match os {
        "macos" => Some(Backend::Cocoa),
        "windows" => Some(Backend::Forms),
        _ => {
            let browser = || Some(Backend::Browser { opener: find("xdg-open")? });
            if env("AIORT_DIALOG").is_some_and(|v| v == "browser") {
                return browser();
            }
            ["zenity", "yad"]
                .iter()
                .find_map(|b| Some(Backend::Zenity { bin: find(b)?, yad: *b == "yad" }))
                .or_else(browser)
        }
    }
}

/// `bin` on `PATH`.
pub(crate) fn on_path(bin: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(bin)).find(|p| p.is_file())
}

/// The line a progress file holds: the percent (-1: unknown) and the text.
pub(crate) fn progress_line(pct: Option<u64>, detail: &str) -> String {
    let pct = pct.map_or(-1, |p| p.min(100) as i64);
    format!("{pct}\t{}", detail.replace(['\r', '\n', '\t'], " "))
}

/// The app's `[build.download_window]` (`instance::start` sets it).
pub(crate) static WORDS: std::sync::OnceLock<aio::config::DownloadWindow> = std::sync::OnceLock::new();

/// The download window's words and colours: the app's, else aio's.
pub(crate) fn words() -> &'static aio::config::DownloadWindow {
    WORDS.get_or_init(Default::default)
}

/// The download window's words for app `cfg` (pure): its
/// `[build.download_window]`, the line under the bar the splash's
/// `download_text` when the splash draws the download.
pub(crate) fn words_of(cfg: &aio::config::Config) -> aio::config::DownloadWindow {
    let mut w = cfg.build.download_window.clone();
    let splash = cfg.build.splash.as_ref().filter(|s| s.show != aio::config::SplashShow::Never);
    if let Some(t) = splash.and_then(|s| s.download_text.clone()) {
        w.text = Some(t);
    }
    w
}

/// `t` with `{title}`, `{version}` and each of `more` filled (pure).
fn fill(t: &str, title: &str, version: &str, more: &[(&str, &str)]) -> String {
    let t = t.replace("{title}", title).replace("{version}", version);
    more.iter().fold(t, |t, (k, v)| t.replace(&format!("{{{k}}}"), v))
}

/// The download window's heading: `w.heading`, else "Getting <title> ready".
pub(crate) fn heading(w: &aio::config::DownloadWindow, title: &str, version: &str) -> String {
    fill(w.heading.as_deref().unwrap_or("Getting {title} ready"), title, version, &[])
}

/// `colors` (`[light, dark]`, each background, text, bar, track) with the
/// ones `w` sets, in both modes (pure).
pub(crate) fn recolored(mut colors: [[String; 4]; 2], w: &aio::config::DownloadWindow) -> [[String; 4]; 2] {
    for (i, c) in [&w.background, &w.color, &w.bar, &w.track].into_iter().enumerate() {
        if let Some(c) = c {
            colors[0][i] = c.clone();
            colors[1][i] = c.clone();
        }
    }
    colors
}

/// "Downloading Electron 44.9.3 — first start only · 45 / 110 MB", or the
/// app's `text` with `{got}` and `{total}` (`?` while unknown).
pub(crate) fn download_detail(
    w: &aio::config::DownloadWindow,
    title: &str,
    version: &str,
    got: u64,
    total: Option<u64>,
) -> String {
    let mb = |n: u64| format!("{:.0}", n as f64 / 1e6);
    let total = total.filter(|t| *t > 0);
    if let Some(t) = &w.text {
        let pct = total.map_or("?".into(), |t| (got.min(t) * 100 / t).to_string());
        return fill(t, title, version, &[("got", &mb(got)), ("total", &total.map_or("?".into(), mb)), ("pct", &pct)]);
    }
    match total {
        Some(t) => format!("Downloading Electron {version} — first start only · {} / {} MB", mb(got), mb(t)),
        None => format!("Downloading Electron {version} — first start only · {} MB", mb(got)),
    }
}

/// The question asked when the runtime could not be had. `download`: it
/// failed coming down (then the connection is the advice), not on disk.
/// The app's `question` takes `{reason}`.
pub(crate) fn failure_message(
    w: &aio::config::DownloadWindow,
    title: &str,
    version: &str,
    why: &str,
    download: bool,
) -> String {
    let why = why.lines().next().unwrap_or("").trim();
    if let Some(q) = &w.question {
        return fill(q, title, version, &[("reason", why)]);
    }
    match download {
        true => format!(
            "{title} needs Electron {version} to open its window, and it could not be downloaded.\n\n{why}\n\n\
             Check the internet connection, then Retry."
        ),
        false => format!(
            "{title} needs Electron {version} to open its window, and it could not be prepared.\n\n{why}\n\n\
             Fix the cause above, then Retry."
        ),
    }
}
/// `attempt` until it works, the user cancels (`Interrupted`) or, asked
/// with the failure (`ask`), quits or takes the previous version (`back`
/// starts it, then this start ends). An attempt's error says if it was the
/// download that failed.
pub(crate) fn retrying(
    title: &str,
    version: &str,
    mut ask: impl FnMut(&str) -> Answer,
    back: &dyn Fn() -> io::Result<()>,
    mut attempt: impl FnMut() -> Result<(), (io::Error, bool)>,
) -> io::Result<()> {
    loop {
        let Err((e, download)) = attempt() else { return Ok(()) };
        if e.kind() == io::ErrorKind::Interrupted {
            return Err(e);
        }
        eprintln!("[aio] runtime: could not get Electron {version}: {e}");
        match ask(&failure_message(words(), title, version, &e.to_string(), download)) {
            Answer::Retry => eprintln!("[aio] runtime: retrying"),
            Answer::Quit => return Err(e),
            Answer::Previous => {
                back()?;
                eprintln!("[aio] runtime: the previous version starts in this one's place");
                return Err(io::Error::other(format!("{e}; the previous version starts instead")));
            }
        }
    }
}

/// zenity and yad `--text` is Pango markup: `&`, `<`, `>` are text.
pub(crate) fn pango(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The zenity / yad download window. yad: Cancel is its one button;
/// `cancel` false: none (closing the window still cancels).
pub(crate) fn zenity_progress_args(yad: bool, title: &str, heading: &str, cancel: bool) -> Vec<String> {
    let mut a = vec![
        "--progress".into(),
        format!("--title={title}"),
        format!("--text={}", pango(heading)),
        "--percentage=0".into(),
        "--width=460".into(),
        "--auto-close".into(),
    ];
    a.extend(
        match (yad, cancel) {
            (true, true) => Some("--button=Cancel:1"),
            (true, false) => Some("--no-buttons"),
            (false, true) => None,
            (false, false) => Some("--no-cancel"),
        }
        .map(String::from),
    );
    a
}

/// The zenity / yad question: Retry (exit 0) or Quit, and with `previous`
/// [`PREVIOUS`] (zenity prints it, yad exits 2).
pub(crate) fn zenity_ask_args(yad: bool, title: &str, message: &str, previous: bool) -> Vec<String> {
    let (title, text) = (format!("--title={title}"), format!("--text={}", pango(message)));
    let mut a = match yad {
        false => vec!["--question".into(), title, text, "--ok-label=Retry".into(), "--cancel-label=Quit".into()],
        true => vec![title, text, "--button=Quit:1".into(), "--button=Retry:0".into()],
    };
    if previous {
        a.push(if yad { format!("--button={PREVIOUS}:2") } else { format!("--extra-button={PREVIOUS}") });
    }
    a.push("--width=460".into());
    a
}

/// What a zenity / yad question's ending (exit code, stdout) answered.
pub(crate) fn zenity_answer(yad: bool, code: Option<i32>, stdout: &str) -> Answer {
    match code {
        Some(0) => Answer::Retry,
        Some(2) if yad => Answer::Previous,
        _ if !yad && stdout.trim() == PREVIOUS => Answer::Previous,
        _ => Answer::Quit,
    }
}

/// What the macOS question printed (its button's name), or what the
/// Windows one wrote (`retry`, `previous`, `quit`), answered.
pub(crate) fn said_answer(said: &str) -> Answer {
    match said.trim() {
        "Retry" | "retry" => Answer::Retry,
        PREVIOUS | "previous" => Answer::Previous,
        _ => Answer::Quit,
    }
}

/// How a zenity / yad window ended when it wasn't told to: exit 0 or 1 is
/// its Cancel or close, unless its display could not be opened (GTK warns on
/// stderr either way, so stderr alone says nothing). `Err`: why it never showed.
pub(crate) fn zenity_ending(code: Option<i32>, stderr: &str) -> Result<(), String> {
    let display = |l: &str| {
        let l = l.to_lowercase();
        ["cannot open display", "failed to open display", "unable to init server", "cannot connect to"]
            .iter()
            .any(|p| l.contains(p))
    };
    if matches!(code, Some(0 | 1)) && !stderr.lines().any(display) {
        return Ok(());
    }
    let line = stderr.lines().find(|l| display(l)).or_else(|| stderr.trim().lines().next()).unwrap_or("");
    Err(if line.is_empty() { format!("exited {code:?}") } else { line.trim().to_string() })
}

/// What the window says after an update.
#[derive(Debug, PartialEq)]
pub(crate) enum Shown {
    On,
    /// The user closed it: the start is cancelled.
    Cancelled,
    /// It could not be shown (why): the download goes on without it.
    Lost(String),
}

/// A download window that is showing; it closes when dropped.
pub(crate) struct Progress {
    /// The program drawing it; none for a page.
    child: Option<Child>,
    /// zenity / yad: the heading (each update says it again); else the
    /// helper's dir, with `progress` and `shown` in it.
    feed: Feed,
    last: String,
    /// zenity / yad: their GTK stylesheet's dir ([`gtk_config`]).
    css: Option<PathBuf>,
}

enum Feed {
    Stdin(String),
    Files(PathBuf),
    Page(crate::firstpage::Page),
}

impl Progress {
    /// Show `pct` (None: unknown) and `detail`, unless the window is gone.
    pub(crate) fn show(&mut self, pct: Option<u64>, detail: &str) -> Shown {
        if let Feed::Page(p) = &self.feed {
            if p.closed() {
                return Shown::Cancelled;
            }
            if let Some(why) = p.lost(crate::firstpage::OPEN_WITHIN) {
                return Shown::Lost(why);
            }
            p.show(|s| (s.fraction, s.detail) = (pct.map(|p| p.min(100) as f64 / 100.0), detail.into()));
            return Shown::On;
        }
        if let Some(child) = self.child.as_mut()
            && let Ok(Some(st)) = child.try_wait()
        {
            return match &self.feed {
                Feed::Stdin(_) => {
                    let mut err = String::new();
                    _ = child.stderr.take().map(|mut e| e.read_to_string(&mut err));
                    zenity_ending(st.code(), &err).map_or_else(Shown::Lost, |()| Shown::Cancelled)
                }
                Feed::Files(dir) if dir.join("shown").exists() => Shown::Cancelled,
                _ => Shown::Lost("the dialog helper exited before its window showed".into()),
            };
        }
        let line = progress_line(pct, detail);
        if line != self.last {
            match &self.feed {
                Feed::Stdin(heading) => {
                    let bar = pct.map_or(String::new(), |p| format!("{}\n", p.min(100) * 99 / 100));
                    let text = format!("{bar}# {}\\n{}\n", pango(heading), pango(&detail.replace(['\r', '\n'], " ")));
                    _ = self.child.as_mut().and_then(|c| c.stdin.as_mut()).map(|i| i.write_all(text.as_bytes()));
                }
                // A skipped frame is all a failed write costs: the next one writes again.
                Feed::Files(dir) => _ = std::fs::write(dir.join("progress"), &line),
                Feed::Page(_) => {}
            }
            self.last = line;
        }
        Shown::On
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        // A page closes as it drops.
        let Some(child) = self.child.as_mut() else { return };
        match &self.feed {
            Feed::Stdin(_) => {
                _ = child.stdin.as_mut().map(|i| i.write_all(b"100\n"));
                drop(child.stdin.take());
            }
            Feed::Files(dir) => _ = std::fs::write(dir.join("progress"), "close"),
            Feed::Page(_) => {}
        }
        let until = Instant::now() + Duration::from_secs(2);
        while matches!(child.try_wait(), Ok(None)) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(50));
        }
        _ = child.kill();
        _ = child.wait();
        if let Feed::Files(dir) = &self.feed {
            _ = std::fs::remove_dir_all(dir);
        }
        if let Some(dir) = &self.css {
            _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// A fresh dir for one helper's files.
fn helper_dir() -> io::Result<PathBuf> {
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("aio-dialog-{}-{at}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub(crate) fn quiet(mut cmd: Command) -> Command {
    aio::lifecycle::scrub(&mut cmd, false);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut cmd, 0x0800_0000); // CREATE_NO_WINDOW
    cmd
}

fn osascript(script: &Path) -> Command {
    let mut cmd = quiet(Command::new("/usr/bin/osascript"));
    cmd.args(["-l", "JavaScript"]).arg(script);
    cmd
}

/// PowerShell, running the script `$env:AIO_PS_SCRIPT` names: read as a
/// script block, so the execution policy treats it as a typed command.
fn powershell(script: &Path) -> Command {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let mut cmd = quiet(Command::new(Path::new(&root).join("System32/WindowsPowerShell/v1.0/powershell.exe")));
    cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-WindowStyle", "Hidden"])
        .arg("-EncodedCommand")
        .arg(encoded_command("& ([ScriptBlock]::Create([IO.File]::ReadAllText($env:AIO_PS_SCRIPT)))"))
        .env("AIO_PS_SCRIPT", script);
    cmd
}

/// PowerShell's `-EncodedCommand` form: UTF-16LE, base64.
pub(crate) fn encoded_command(script: &str) -> String {
    use base64::Engine;
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

impl Backend {
    /// Open the download window.
    pub(crate) fn progress(&self, title: &str, heading: &str, look: &Look) -> io::Result<Progress> {
        let mut css = None;
        let (child, feed) = match self {
            Backend::Zenity { bin, yad } => {
                let mut cmd = quiet(Command::new(bin));
                if let Ok(d) = gtk_config(look) {
                    // yad draws the splash above its bar; zenity has no image.
                    if let Some((png, _)) = look.image.filter(|_| *yad)
                        && std::fs::write(d.join("splash.png"), png).is_ok()
                    {
                        cmd.arg(format!("--image={}", d.join("splash.png").display()));
                    }
                    cmd.env("XDG_CONFIG_HOME", &d);
                    css = Some(d);
                }
                let child = cmd
                    .args(zenity_progress_args(*yad, title, heading, words().cancel))
                    .args(look.icon.map(|p| format!("--window-icon={}", p.display())))
                    .stdin(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn();
                let child = child.inspect_err(|_| _ = css.as_ref().map(std::fs::remove_dir_all))?;
                (Some(child), Feed::Stdin(heading.into()))
            }
            Backend::Browser { opener } => {
                let image = look.image.map(|(png, _)| png);
                (None, Feed::Page(crate::firstpage::Page::open(title, heading, opener, look.colors, image)?))
            }
            helper => {
                let dir = helper_dir()?;
                let (file, ack, pid) = (dir.join("progress"), dir.join("shown"), std::process::id().to_string());
                std::fs::write(&file, progress_line(None, ""))?;
                let [light, dark] = look.env();
                // The splash above the bar: its file, and its size on screen.
                let (image, image_size) = match look.image {
                    Some((png, (w, h))) => {
                        std::fs::write(dir.join("splash.png"), png)?;
                        (dir.join("splash.png").display().to_string(), format!("{w} {h}"))
                    }
                    None => (String::new(), String::new()),
                };
                let spawned = if *helper == Backend::Cocoa {
                    std::fs::write(dir.join("progress.js"), [MAC_LOOK_JXA, MAC_PROGRESS_JXA].concat())?;
                    osascript(&dir.join("progress.js"))
                        .env("AIO_DLG_ICON", look.icon())
                        .args([title, heading])
                        .arg(&file)
                        .arg(&ack)
                        .args([&pid, &light, &dark, &look.at()])
                        .arg(if words().cancel { "1" } else { "0" })
                        .args([&image, &image_size])
                        .spawn()
                } else {
                    std::fs::write(dir.join("progress.ps1"), [WINDOWS_LOOK_PS, WINDOWS_PROGRESS_PS].concat())?;
                    powershell(&dir.join("progress.ps1"))
                        .env("AIO_DLG_TITLE", title)
                        .env("AIO_DLG_HEADING", heading)
                        .env("AIO_DLG_FILE", &file)
                        .env("AIO_DLG_ACK", &ack)
                        .env("AIO_DLG_PARENT", &pid)
                        .env("AIO_DLG_LIGHT", &light)
                        .env("AIO_DLG_DARK", &dark)
                        .env("AIO_DLG_AT", look.at())
                        .env("AIO_DLG_CANCEL", if words().cancel { "1" } else { "0" })
                        .env("AIO_DLG_IMAGE", &image)
                        .env("AIO_DLG_IMAGE_SIZE", &image_size)
                        .env("AIO_DLG_ICON", look.icon())
                        .spawn()
                };
                match spawned {
                    Ok(c) => (Some(c), Feed::Files(dir)),
                    Err(e) => {
                        _ = std::fs::remove_dir_all(&dir);
                        return Err(e);
                    }
                }
            }
        };
        Ok(Progress { child, feed, last: String::new(), css })
    }

    /// Ask Retry or Quit, with `message`, and with `previous` the version
    /// an update replaced too. A question that can't be asked is a Quit.
    pub(crate) fn ask(&self, title: &str, message: &str, previous: bool, look: &Look) -> Answer {
        let [light, dark] = look.env();
        let asked = match self {
            Backend::Zenity { bin, yad } => {
                let mut cmd = quiet(Command::new(bin));
                let css = gtk_config(look).ok();
                if let Some(d) = &css {
                    cmd.env("XDG_CONFIG_HOME", d);
                }
                let icon = look.icon.map(|p| format!("--window-icon={}", p.display()));
                let out = cmd
                    .args(zenity_ask_args(*yad, title, message, previous))
                    .args(icon)
                    .stdout(Stdio::piped())
                    .output();
                _ = css.map(std::fs::remove_dir_all);
                out.map(|o| zenity_answer(*yad, o.status.code(), &String::from_utf8_lossy(&o.stdout)))
            }
            Backend::Cocoa => helper_dir().and_then(|dir| {
                let script = dir.join("ask.js");
                let out = std::fs::write(&script, [MAC_LOOK_JXA, MAC_ASK_JXA].concat()).and_then(|()| {
                    osascript(&script)
                        .env("AIO_DLG_ICON", look.icon())
                        .args([title, message, if previous { PREVIOUS } else { "" }, &light, &dark, &look.at()])
                        .stdout(Stdio::piped())
                        .output()
                });
                _ = std::fs::remove_dir_all(&dir);
                Ok(said_answer(&String::from_utf8_lossy(&out?.stdout)))
            }),
            Backend::Forms => helper_dir().and_then(|dir| {
                let (script, file) = (dir.join("ask.ps1"), dir.join("answer"));
                let ran = std::fs::write(&script, [WINDOWS_LOOK_PS, WINDOWS_ASK_PS].concat()).and_then(|()| {
                    powershell(&script)
                        .env("AIO_DLG_TITLE", title)
                        .env("AIO_DLG_TEXT", message)
                        .env("AIO_DLG_FILE", &file)
                        .env("AIO_DLG_PREVIOUS", if previous { PREVIOUS } else { "" })
                        .env("AIO_DLG_LIGHT", &light)
                        .env("AIO_DLG_DARK", &dark)
                        .env("AIO_DLG_AT", look.at())
                        .env("AIO_DLG_ICON", look.icon())
                        .status()
                });
                let answer = std::fs::read_to_string(&file).unwrap_or_default();
                _ = std::fs::remove_dir_all(&dir);
                ran.map(|_| said_answer(&answer))
            }),
            Backend::Browser { opener } => crate::firstpage::Page::open(title, title, opener, look.colors, None)
                .map(|p| p.ask(title, message, previous, crate::firstpage::OPEN_WITHIN)),
        };
        asked.unwrap_or_else(|e| {
            eprintln!("[aio] runtime: the Retry / Quit question could not be asked ({e}): quitting");
            Answer::Quit
        })
    }
}

/// Before each Windows helper: Forms loaded, the process DPI aware (sharp
/// text, sizes scaled by `AutoScaleMode`), and `Look $form` giving a form
/// the app's colours (`AIO_DLG_LIGHT` / `AIO_DLG_DARK`, as Windows' app
/// mode is light or dark) and its place over the app's window
/// (`AIO_DLG_AT`, Electron's points, scaled to this screen's pixels) and its
/// icon (`AIO_DLG_ICON`, a PNG).
pub(crate) const WINDOWS_LOOK_PS: &str = r#"
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
try {
  Add-Type -Namespace AioDpi -Name U -MemberDefinition '[DllImport("user32.dll")] public static extern bool SetProcessDPIAware();'
  [void][AioDpi.U]::SetProcessDPIAware()
} catch {}
[System.Windows.Forms.Application]::EnableVisualStyles()
$dark = $false
try { $dark = (Get-ItemPropertyValue 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize' 'AppsUseLightTheme') -eq 0 } catch {}
$cc = $(if ($dark) { $env:AIO_DLG_DARK } else { $env:AIO_DLG_LIGHT })
$bg = $null
if ($cc) {
  $cc = $cc.Split(' ') | ForEach-Object { [System.Drawing.ColorTranslator]::FromHtml($_) }
  $bg, $fg, $barc, $trk = $cc
}
function Look($form) {
  $form.AutoScaleDimensions = New-Object System.Drawing.SizeF(96, 96)
  $form.AutoScaleMode = 'Dpi'
  if ($bg) { $form.BackColor = $bg; $form.ForeColor = $fg }
  if ($env:AIO_DLG_ICON) {
    try { $form.Icon = [System.Drawing.Icon]::FromHandle((New-Object System.Drawing.Bitmap $env:AIO_DLG_ICON).GetHicon()) } catch {}
  }
  if ($env:AIO_DLG_AT) {
    $form.StartPosition = 'Manual'
    $at = $env:AIO_DLG_AT.Split(' ')
    $form.Add_Load({
      $s = $this.DeviceDpi / 96
      $this.Location = New-Object System.Drawing.Point([int]([int]$at[0] * $s - $this.Width / 2), [int]([int]$at[1] * $s - $this.Height / 2))
    }.GetNewClosure())
  }
}
"#;

/// The download window over Windows Forms. Reads `AIO_DLG_*`; polls the
/// progress file, writes the ack once shown, closes itself when the app's
/// process (`AIO_DLG_PARENT`) is gone. Exit 3: the user closed it.
pub(crate) const WINDOWS_PROGRESS_PS: &str = r#"
$f = New-Object System.Windows.Forms.Form
$f.Text = $env:AIO_DLG_TITLE
$f.ClientSize = New-Object System.Drawing.Size(460, 138)
$f.FormBorderStyle = 'FixedDialog'
$f.MaximizeBox = $false
$f.MinimizeBox = $false
$f.ShowInTaskbar = $true
$f.StartPosition = 'CenterScreen'
$h = New-Object System.Windows.Forms.Label
$h.Text = $env:AIO_DLG_HEADING
$h.Location = New-Object System.Drawing.Point(18, 14)
$h.AutoSize = $true
$h.Font = New-Object System.Drawing.Font($h.Font.FontFamily, 10, [System.Drawing.FontStyle]::Bold)
$d = New-Object System.Windows.Forms.Label
$d.Location = New-Object System.Drawing.Point(18, 42)
$d.Size = New-Object System.Drawing.Size(424, 20)
# The bar: a track and its fill.
$tr = New-Object System.Windows.Forms.Panel
$tr.Location = New-Object System.Drawing.Point(18, 74)
$tr.Size = New-Object System.Drawing.Size(424, 6)
$tr.BackColor = $(if ($bg) { $trk } else { [System.Drawing.SystemColors]::ControlLight })
$fl = New-Object System.Windows.Forms.Panel
$fl.Size = New-Object System.Drawing.Size(127, 6)
$fl.BackColor = $(if ($bg) { $barc } else { [System.Drawing.SystemColors]::Highlight })
$tr.Controls.Add($fl)
# Cancel, the one button: it ends the download, as the window's close does.
$f.Controls.AddRange(@($h, $d, $tr))
if ($env:AIO_DLG_CANCEL -ne '0') {
  $c = New-Object System.Windows.Forms.Button
  $c.Text = 'Cancel'
  $c.Location = New-Object System.Drawing.Point(354, 96)
  $c.Size = New-Object System.Drawing.Size(88, 26)
  $c.Add_Click({ $f.Close() })
  $f.CancelButton = $c
  $f.Controls.Add($c)
}
# The splash ([build.splash]) above it all: the window grows to hold it.
if ($env:AIO_DLG_IMAGE) {
  $iw, $ih = $env:AIO_DLG_IMAGE_SIZE.Split(' ') | ForEach-Object { [int]$_ }
  $W = [Math]::Max(460, $iw)
  foreach ($x in $f.Controls) { $x.Top += $ih }
  $pic = New-Object System.Windows.Forms.PictureBox
  $pic.SizeMode = 'Zoom'
  $pic.Location = New-Object System.Drawing.Point([int](($W - $iw) / 2), 0)
  $pic.Size = New-Object System.Drawing.Size($iw, $ih)
  $pic.Image = [System.Drawing.Image]::FromFile($env:AIO_DLG_IMAGE)
  $f.Controls.Add($pic)
  $f.ClientSize = New-Object System.Drawing.Size($W, (138 + $ih))
}
Look $f
$TW = $tr.Width
$script:pulse = $true
$t = New-Object System.Windows.Forms.Timer
$t.Interval = 50
$script:n = 0
$t.Add_Tick({
  $script:n++
  # Unknown size: a segment runs along the track.
  if ($script:pulse) { $fl.Left = (($script:n * 12) % ($TW + $fl.Width)) - $fl.Width }
  if ($script:n % 4 -ne 0) { return }
  # The app that opened this window is gone: nobody will close it.
  if ($script:n % 40 -eq 0 -and -not (Get-Process -Id ([int]$env:AIO_DLG_PARENT) -ErrorAction SilentlyContinue)) {
    $t.Stop(); $f.Tag = 'done'; $f.Close(); return
  }
  $s = $null
  try { $s = [IO.File]::ReadAllText($env:AIO_DLG_FILE) } catch { return }
  if ($s -eq 'close') { $t.Stop(); $f.Tag = 'done'; $f.Close(); return }
  $p = $s.Split([char]9, 2)
  if ($p.Length -lt 2) { return }
  $k = 0
  if ([int]::TryParse($p[0], [ref]$k) -and $k -ge 0) {
    $script:pulse = $false
    $fl.Left = 0
    $fl.Width = [int]($TW * [Math]::Min(100, $k) / 100)
  } elseif (-not $script:pulse) { $script:pulse = $true; $fl.Width = [int]($TW * 0.3) }
  $d.Text = $p[1]
})
$t.Start()
$f.Add_Shown({
  $f.Activate()
  try { [IO.File]::WriteAllText($env:AIO_DLG_ACK, 'shown') } catch {}
})
[void]$f.ShowDialog()
if ($f.Tag -eq 'done') { exit 0 } else { exit 3 }
"#;

/// The Retry / Quit question over Windows Forms: `AIO_DLG_TEXT` and its
/// buttons (`AIO_DLG_PREVIOUS`: a third); writes `retry`, `previous` or
/// `quit` (also for its close) to `AIO_DLG_FILE`.
pub(crate) const WINDOWS_ASK_PS: &str = r#"
$f = New-Object System.Windows.Forms.Form
$f.Text = $env:AIO_DLG_TITLE
$f.FormBorderStyle = 'FixedDialog'
$f.MaximizeBox = $false
$f.MinimizeBox = $false
$f.ShowInTaskbar = $true
$f.StartPosition = 'CenterScreen'
# Laid out at 96 DPI, scaled once at load (`Look`); text measures at the
# screen's DPI, so a measure is taken back to 96 first.
$s = [System.Drawing.Graphics]::FromHwnd([IntPtr]::Zero).DpiX / 96
$m = New-Object System.Windows.Forms.Label
$m.Text = $env:AIO_DLG_TEXT
$m.Location = New-Object System.Drawing.Point(18, 18)
$m.MaximumSize = New-Object System.Drawing.Size([int](424 * $s), 0)
$m.AutoSize = $true
$by = $m.Location.Y + [Math]::Ceiling($m.PreferredSize.Height / $s) + 18
$m.MaximumSize = New-Object System.Drawing.Size(424, 0)
$f.Controls.Add($m)
$x = 442
# The unary comma keeps a pair one item (@(@(a, b)) flattens to a, b).
$names = @(,@('Retry', 'retry'))
if ($env:AIO_DLG_PREVIOUS) { $names += ,@($env:AIO_DLG_PREVIOUS, 'previous') }
$names += ,@('Quit', 'quit')
foreach ($n in $names) {
  $b = New-Object System.Windows.Forms.Button
  $b.Text = $n[0]
  $b.Tag = $n[1]
  $bw = [Math]::Max(88, [Math]::Ceiling($b.PreferredSize.Width / $s) + 16)
  $b.Size = New-Object System.Drawing.Size($bw, 26)
  $x -= $bw
  $b.Location = New-Object System.Drawing.Point($x, $by)
  $x -= 8
  $b.Add_Click({ $f.Tag = $this.Tag; $f.Close() })
  $f.Controls.Add($b)
  if ($b.Tag -eq 'retry') { $f.AcceptButton = $b } else { $f.CancelButton = $b }
}
$f.ClientSize = New-Object System.Drawing.Size(460, ($by + 26 + 18))
Look $f
$f.Add_Shown({ $f.Activate() })
[void]$f.ShowDialog()
$a = $(if ($f.Tag) { [string]$f.Tag } else { 'quit' })
[IO.File]::WriteAllText($env:AIO_DLG_FILE, $a)
"#;

/// Before each macOS helper: `col('#rrggbb')`, `look(argv, i)` (the
/// colours at argv i and i+1, light and dark, picked as the app's
/// appearance is: `[bg, text, bar, track]` or null) and `place(win, at,
/// W, H)` (over the app's window: `"x y"` in Electron's points, top-left
/// origin; else the screen's middle) and `icon(app)` (the app's, `AIO_DLG_ICON`,
/// in the Dock).
pub(crate) const MAC_LOOK_JXA: &str = r#"
ObjC.import('Cocoa');
ObjC.import('stdlib');
function col(h) {
  const n = parseInt(h.slice(1), 16);
  return $.NSColor.colorWithSRGBRedGreenBlueAlpha(((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255, 1);
}
function look(argv, i) {
  const app = $.NSApplication.sharedApplication;
  const name = ObjC.unwrap(app.effectiveAppearance.name) || '';
  const s = (name.indexOf('Dark') >= 0 ? argv[i + 1] : argv[i]) || '';
  const c = s.split(' ');
  return c.length === 4 ? c.map(col) : null;
}
function icon(app) {
  const p = ObjC.unwrap($.NSProcessInfo.processInfo.environment.objectForKey('AIO_DLG_ICON'));
  if (!p) return;
  const img = $.NSImage.alloc.initWithContentsOfFile(p);
  if (img && !img.isNil()) app.applicationIconImage = img;
}
function place(win, at, W, H) {
  const p = (at || '').split(' ').map(Number);
  if (p.length !== 2 || p.some(isNaN)) return win.center;
  const top = $.NSScreen.screens.objectAtIndex(0).frame.size.height;
  win.setFrameOrigin($.NSMakePoint(p[0] - W / 2, top - p[1] - H / 2));
}
"#;

/// The download window over Cocoa. argv: title, heading, progress file,
/// ack file, the app's pid, its colours (light, dark) and place
/// ([`MAC_LOOK_JXA`]), then `0` for no Cancel button, then the splash image
/// and its size (`"w h"`), both empty for none. Exit 3: the user closed it. The event loop is
/// run by hand: an NSTimer needs a target from ObjC.registerSubclass, which
/// never returned on macOS 14 (aio, measured).
pub(crate) const MAC_PROGRESS_JXA: &str = r#"
function run(argv) {
  const title = argv[0], heading = argv[1], file = argv[2], ack = argv[3], parent = argv[4];
  let ticks = 0;
  const app = $.NSApplication.sharedApplication;
  app.setActivationPolicy($.NSApplicationActivationPolicyRegular);
  icon(app);
  // argv[9], argv[10]: the splash ([build.splash]) and its size, drawn above the rest.
  const img = argv[9] || '', is = (argv[10] || '').split(' ').map(Number);
  const IH = img && is.length === 2 ? is[1] : 0, IW = IH ? is[0] : 0;
  const W = Math.max(460, IW), H0 = 150, H = H0 + IH, BW = W - 40;
  const win = $.NSWindow.alloc.initWithContentRectStyleMaskBackingDefer(
    $.NSMakeRect(0, 0, W, H),
    $.NSWindowStyleMaskTitled | $.NSWindowStyleMaskClosable,
    $.NSBackingStoreBuffered, false);
  win.title = title;
  win.releasedWhenClosed = false;
  const head = $.NSTextField.labelWithString(heading);
  head.font = $.NSFont.boldSystemFontOfSize(13);
  head.frame = $.NSMakeRect(20, H0 - 40, BW, 20);
  const det = $.NSTextField.labelWithString('');
  det.frame = $.NSMakeRect(20, H0 - 66, BW, 18);
  function box(x, y, w, h, col) {
    const b = $.NSBox.alloc.initWithFrame($.NSMakeRect(x, y, w, h));
    b.boxType = 4; // NSBoxCustom
    b.titlePosition = 0; // NSNoTitle
    b.borderWidth = 0;
    b.cornerRadius = h / 2;
    b.fillColor = col;
    return b;
  }
  const c = look(argv, 5);
  if (c) {
    win.backgroundColor = c[0];
    head.textColor = c[1];
    det.textColor = c[1];
  }
  const track = box(20, 64, BW, 6, c ? c[3] : $.NSColor.quaternaryLabelColor);
  const fill = box(20, 64, BW * 0.3, 6, c ? c[2] : $.NSColor.controlAccentColor);
  // Cancel, the one button: it ends the download, as the window's close does.
  const cancel = $.NSButton.alloc.initWithFrame($.NSMakeRect(W - 20 - 88, 14, 88, 32));
  cancel.title = 'Cancel';
  cancel.bezelStyle = $.NSBezelStyleRounded;
  cancel.setButtonType($.NSButtonTypePushOnPushOff);
  cancel.keyEquivalent = '\u001b';
  // argv[8] '0' ([build.download_window] cancel = false): no button; the close still cancels.
  for (const v of [head, det, track, fill, ...(argv[8] === '0' ? [] : [cancel])]) win.contentView.addSubview(v);
  if (IH) {
    const iv = $.NSImageView.imageViewWithImage($.NSImage.alloc.initWithContentsOfFile(img));
    iv.imageScaling = $.NSImageScaleProportionallyUpOrDown;
    iv.frame = $.NSMakeRect((W - IW) / 2, H0, IW, IH);
    win.contentView.addSubview(iv);
  }
  place(win, argv[7], W, H);
  win.makeKeyAndOrderFront(null);
  app.activateIgnoringOtherApps(true);
  $('shown').writeToFileAtomicallyEncodingError(ack, true, $.NSUTF8StringEncoding, null);
  let pulse = true;
  function tick() {
    // Minimized or hidden (Cmd+H) is not closed. Cancel is the close.
    if (!win.isVisible && !win.isMiniaturized && !app.isHidden) $.exit(3);
    // JXA hands NSInteger back as a string ("1"), measured on macOS 14.
    if (Number(cancel.state) === 1) $.exit(3);
    ticks++;
    // Unknown size: a segment runs along the track.
    if (pulse) fill.frame = $.NSMakeRect(20 + ((ticks * 9) % (BW + BW * 0.3)) - BW * 0.3, 64, BW * 0.3, 6);
    if (ticks % 4 !== 0) return;
    // The app that opened this window is gone: nobody will close it.
    if (ticks % 100 === 0 && parent) {
      const k = $.NSTask.launchedTaskWithLaunchPathArguments('/bin/kill', ['-0', parent]);
      k.waitUntilExit;
      if (k.terminationStatus !== 0) $.exit(0);
    }
    const s = $.NSString.stringWithContentsOfFileEncodingError(file, $.NSUTF8StringEncoding, null);
    if (!s || s.isNil()) return;
    const str = s.js;
    if (str === 'close') $.exit(0);
    const i = str.indexOf('\t');
    if (i < 0) return;
    const n = parseInt(str.slice(0, i), 10);
    pulse = !(n >= 0);
    if (!pulse) fill.frame = $.NSMakeRect(20, 64, Math.max(1, BW * Math.min(100, n) / 100), 6);
    det.stringValue = str.slice(i + 1);
  }
  app.finishLaunching;
  for (;;) {
    const ev = app.nextEventMatchingMaskUntilDateInModeDequeue(
      $.NSEventMaskAny, $.NSDate.dateWithTimeIntervalSinceNow(0.05),
      $.NSDefaultRunLoopMode, true);
    if (ev && !ev.isNil()) app.sendEvent(ev);
    tick();
  }
}
"#;

/// The Retry / Quit question over Cocoa. argv: title, message, a third
/// button's words (empty: none), the colours and place ([`MAC_LOOK_JXA`]).
/// Prints the button; its close is Quit. The buttons toggle and the loop reads which
/// one is on (a button action needs a target, see `MAC_PROGRESS_JXA`).
pub(crate) const MAC_ASK_JXA: &str = r#"
function run(argv) {
  const title = argv[0], message = argv[1], previous = argv[2] || '';
  const app = $.NSApplication.sharedApplication;
  app.setActivationPolicy($.NSApplicationActivationPolicyRegular);
  icon(app);
  const W = 460;
  const msg = $.NSTextField.wrappingLabelWithString(message);
  msg.preferredMaxLayoutWidth = W - 40;
  const mh = Math.ceil(msg.fittingSize.height);
  const H = 20 + mh + 20 + 32 + 14;
  const win = $.NSWindow.alloc.initWithContentRectStyleMaskBackingDefer(
    $.NSMakeRect(0, 0, W, H),
    $.NSWindowStyleMaskTitled | $.NSWindowStyleMaskClosable,
    $.NSBackingStoreBuffered, false);
  win.title = title;
  win.releasedWhenClosed = false;
  msg.frame = $.NSMakeRect(20, H - 20 - mh, W - 40, mh);
  const c = look(argv, 3);
  if (c) {
    win.backgroundColor = c[0];
    msg.textColor = c[1];
  }
  win.contentView.addSubview(msg);
  const buttons = [];
  let x = W - 20;
  for (const name of ['Retry'].concat(previous ? [previous] : [], ['Quit'])) {
    const b = $.NSButton.alloc.initWithFrame($.NSMakeRect(0, 0, 88, 32));
    b.title = name;
    b.bezelStyle = $.NSBezelStyleRounded;
    b.setButtonType($.NSButtonTypePushOnPushOff);
    if (name === 'Retry') b.keyEquivalent = '\r';
    b.sizeToFit;
    const bw = Math.max(88, b.frame.size.width + 8);
    x -= bw;
    b.frame = $.NSMakeRect(x, 14, bw, 32);
    x -= 8;
    win.contentView.addSubview(b);
    buttons.push([name, b]);
  }
  place(win, argv[5], W, H);
  win.makeKeyAndOrderFront(null);
  app.activateIgnoringOtherApps(true);
  app.finishLaunching;
  for (;;) {
    const ev = app.nextEventMatchingMaskUntilDateInModeDequeue(
      $.NSEventMaskAny, $.NSDate.dateWithTimeIntervalSinceNow(0.1),
      $.NSDefaultRunLoopMode, true);
    if (ev && !ev.isNil()) app.sendEvent(ev);
    // JXA hands NSInteger back as a string ("1"), measured on macOS 14.
    for (const [n, b] of buttons) if (Number(b.state) === 1) return n;
    if (!win.isVisible && !win.isMiniaturized) return 'Quit';
  }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_backend_follows_the_os_and_the_desktop() {
        let env = |display: bool, dialog: &'static str| {
            move |k: &str| match k {
                "DISPLAY" if display => Some(":0".to_string()),
                "AIORT_DIALOG" if !dialog.is_empty() => Some(dialog.to_string()),
                _ => None,
            }
        };
        let yad_only = |b: &str| (b == "yad").then(|| PathBuf::from("/usr/bin/yad"));
        let zenity = Backend::Zenity { bin: "/usr/bin/zenity".into(), yad: false };
        assert_eq!(here("linux", &env(true, ""), &|b| Some(PathBuf::from("/usr/bin").join(b))), Some(zenity));
        assert_eq!(
            here("linux", &env(true, ""), &yad_only),
            Some(Backend::Zenity { bin: "/usr/bin/yad".into(), yad: true })
        );
        // Neither: a page in the browser; asked for: the page even so.
        let opener = |b: &str| (b == "xdg-open").then(|| PathBuf::from("/usr/bin/xdg-open"));
        let page = Some(Backend::Browser { opener: "/usr/bin/xdg-open".into() });
        assert_eq!(here("linux", &env(true, ""), &opener), page);
        assert_eq!(here("linux", &env(true, "browser"), &|b| Some(PathBuf::from("/usr/bin").join(b))), page);
        // No display, no helper, or turned off: the log alone.
        assert_eq!(here("linux", &env(false, ""), &yad_only), None);
        assert_eq!(here("linux", &env(true, ""), &|_| None), None);
        assert_eq!(here("macos", &env(false, "none"), &|_| None), None);
        assert_eq!(here("macos", &env(false, ""), &|_| None), Some(Backend::Cocoa));
        assert_eq!(here("windows", &env(false, ""), &|_| None), Some(Backend::Forms));
    }

    #[test]
    fn progress_says_how_far_in_one_line() {
        assert_eq!(progress_line(Some(42), "a\tb\nc"), "42\ta b c");
        assert_eq!(progress_line(Some(140), ""), "100\t");
        assert_eq!(progress_line(None, "x"), "-1\tx");
        assert_eq!(
            download_detail(&Default::default(), "Notes", "44.9.3", 45_000_000, Some(110_000_000)),
            "Downloading Electron 44.9.3 — first start only · 45 / 110 MB"
        );
        let none = aio::config::DownloadWindow::default;
        assert_eq!(
            download_detail(&none(), "N", "1", 3_000_000, None),
            "Downloading Electron 1 — first start only · 3 MB"
        );
        // aio's downloadWindow.text: {got} / {total} MB, {title}, {version}.
        let own =
            aio::config::DownloadWindow { text: Some("{title}: {got} of {total} MB ({version})".into()), ..none() };
        assert_eq!(download_detail(&own, "Notes", "44", 3_000_000, Some(9_000_000)), "Notes: 3 of 9 MB (44)");
        assert_eq!(download_detail(&own, "Notes", "44", 3_000_000, None), "Notes: 3 of ? MB (44)");
        let h = aio::config::DownloadWindow { heading: Some("Setting up {title}".into()), ..none() };
        assert_eq!(heading(&none(), "Notes", "1"), "Getting Notes ready");
        assert_eq!(heading(&h, "Notes", "1"), "Setting up Notes");
        // A colour set is the window's in both modes; the rest stay the app's.
        let c = [["a", "b", "c", "d"].map(String::from), ["e", "f", "g", "h"].map(String::from)];
        let bar = aio::config::DownloadWindow { bar: Some("#14f195".into()), ..none() };
        assert_eq!(recolored(c.clone(), &bar), [["a", "b", "#14f195", "d"], ["e", "f", "#14f195", "h"]]);
        assert_eq!(recolored(c.clone(), &none()), c);
    }

    #[test]
    fn the_question_names_the_cause_and_what_to_do() {
        let w = aio::config::DownloadWindow::default();
        let m = failure_message(&w, "Notes", "44.9.3", "connection refused\nhttps://x (for developers)", true);
        assert!(m.starts_with("Notes needs Electron 44.9.3") && m.contains("\n\nconnection refused\n\n"), "{m}");
        assert!(m.ends_with("Check the internet connection, then Retry.") && !m.contains("developers"));
        assert!(failure_message(&w, "Notes", "1", "disk full", false).ends_with("Fix the cause above, then Retry."));
        // aio's downloadWindow.question, {reason} the first line of what failed.
        let q = aio::config::DownloadWindow { question: Some("{title} could not start: {reason}".into()), ..w };
        assert_eq!(failure_message(&q, "Notes", "1", "disk full\nmore", true), "Notes could not start: disk full");
    }

    #[test]
    fn retry_tries_again_quit_and_cancel_end_the_start() {
        let fail = |n: &mut u32| -> Result<(), (io::Error, bool)> {
            *n += 1;
            if *n < 3 { Err((io::Error::other("offline"), true)) } else { Ok(()) }
        };
        let (mut n, mut asked) = (0, vec![]);
        let no_back = || -> io::Result<()> { panic!("not asked for") };
        retrying(
            "Notes",
            "1",
            |m| {
                asked.push(m.to_string());
                Answer::Retry
            },
            &no_back,
            || fail(&mut n),
        )
        .unwrap();
        assert_eq!((n, asked.len()), (3, 2));
        assert!(asked[0].contains("could not be downloaded.\n\noffline"), "{}", asked[0]);
        let mut n = 0;
        let e = retrying("Notes", "1", |_| Answer::Quit, &no_back, || fail(&mut n)).unwrap_err();
        assert_eq!((e.to_string().as_str(), n), ("offline", 1));
        // The previous version: started, and this start ends.
        let (mut n, went) = (0, std::cell::Cell::new(false));
        let back = || {
            went.set(true);
            Ok(())
        };
        let e = retrying("Notes", "1", |_| Answer::Previous, &back, || fail(&mut n)).unwrap_err();
        assert!(went.get() && e.to_string().ends_with("the previous version starts instead"), "{e}");
        // A cancel is the user's answer already: nothing is asked.
        let cancel = || Err((io::Error::new(io::ErrorKind::Interrupted, "cancelled"), true));
        let e = retrying("Notes", "1", |_| panic!("asked after a cancel"), &no_back, cancel).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::Interrupted);
    }

    #[test]
    fn zenity_and_yad_get_text_not_markup_and_say_how_they_ended() {
        let a = zenity_progress_args(false, "Notes", "Getting A&B <ready>", true);
        assert!(a.contains(&"--text=Getting A&amp;B &lt;ready&gt;".to_string()) && a.contains(&"--auto-close".into()));
        assert!(!a.iter().any(|x| x.starts_with("--button") || x == "--no-cancel"));
        assert!(zenity_progress_args(true, "Notes", "h", true).contains(&"--button=Cancel:1".into()));
        // [build.download_window] cancel = false: no button (the close still cancels).
        assert!(zenity_progress_args(false, "Notes", "h", false).contains(&"--no-cancel".into()));
        let yad = zenity_progress_args(true, "Notes", "h", false);
        assert!(yad.contains(&"--no-buttons".into()) && !yad.iter().any(|x| x.starts_with("--button")));
        let q = zenity_ask_args(false, "Notes", "why", false);
        assert!(q.contains(&"--ok-label=Retry".into()) && q.contains(&"--cancel-label=Quit".into()), "{q:?}");
        assert!(!q.iter().any(|a| a.contains(PREVIOUS)));
        assert!(zenity_ask_args(true, "Notes", "why", false).contains(&"--button=Retry:0".into()));
        assert!(zenity_ask_args(false, "N", "w", true).contains(&format!("--extra-button={PREVIOUS}")));
        assert!(zenity_ask_args(true, "N", "w", true).contains(&format!("--button={PREVIOUS}:2")));
        assert_eq!(zenity_answer(false, Some(0), ""), Answer::Retry);
        assert_eq!(zenity_answer(false, Some(1), &format!("{PREVIOUS}\n")), Answer::Previous);
        assert_eq!(zenity_answer(false, Some(1), ""), Answer::Quit);
        assert_eq!(zenity_answer(true, Some(2), ""), Answer::Previous);
        assert_eq!(zenity_answer(true, Some(252), ""), Answer::Quit, "closed");
        assert_eq!(said_answer("previous\r\n"), Answer::Previous);
        assert_eq!(said_answer("Retry\n"), Answer::Retry);
        assert_eq!(said_answer(""), Answer::Quit);
        // Cancel or close, GTK noise or not.
        assert_eq!(zenity_ending(Some(1), "Gtk-WARNING: theme parsing\n"), Ok(()));
        assert_eq!(
            zenity_ending(Some(1), "Gtk-WARNING **: cannot open display: :9\n").unwrap_err(),
            "Gtk-WARNING **: cannot open display: :9"
        );
        assert_eq!(zenity_ending(Some(255), "").unwrap_err(), "exited Some(255)");
        assert!(zenity_ending(None, "").is_err());
    }

    #[test]
    fn the_windows_are_in_the_app_s_colours_light_or_dark() {
        let c = [["#fff".to_string(), "#111111".into(), "#0a84ff".into(), "#ddd".into()], Default::default()];
        let look = Look { colors: &c, at: Some((640, -12)), icon: None, image: None };
        assert_eq!(look.env()[0], "#ffffff #111111 #0a84ff #dddddd", "six digits for the helpers");
        assert_eq!(look.at(), "640 -12");
        assert_eq!(Look { colors: &c, at: None, icon: None, image: None }.at(), "");
        let css = gtk_css(&c[0]);
        assert!(
            css.contains("background-color: #fff; color: #111111;")
                && css.contains("progress { background-color: #0a84ff")
                && css.contains("button:disabled label { color: alpha(#111111, 0.4); }")
        );
        assert!(linux_dark(Some("Adwaita:dark"), "") && linux_dark(None, "'prefer-dark'\n"));
        assert!(!linux_dark(Some("Adwaita"), "'default'") && !linux_dark(None, ""));
        assert!(WINDOWS_LOOK_PS.contains("$env:AIO_DLG_DARK") && WINDOWS_LOOK_PS.contains("SetProcessDPIAware"));
        assert!(WINDOWS_PROGRESS_PS.contains("Look $f") && WINDOWS_ASK_PS.contains("Look $f"));
        assert!(
            WINDOWS_ASK_PS.contains("PreferredSize.Height / $s") && WINDOWS_ASK_PS.contains("PreferredSize.Width / $s")
        );
        assert!(WINDOWS_ASK_PS.contains("$env:AIO_DLG_PREVIOUS") && MAC_ASK_JXA.contains("argv[2]"));
        assert!(MAC_PROGRESS_JXA.contains("look(argv, 5)") && MAC_ASK_JXA.contains("look(argv, 3)"));
        // Each window wears the app's icon: Windows' title bar and taskbar, the Dock.
        assert!(WINDOWS_LOOK_PS.contains("$form.Icon = ") && WINDOWS_LOOK_PS.contains("$env:AIO_DLG_ICON"));
        assert!(MAC_LOOK_JXA.contains("'AIO_DLG_ICON'") && MAC_LOOK_JXA.contains("app.applicationIconImage = img"));
        assert!(MAC_PROGRESS_JXA.contains("icon(app);") && MAC_ASK_JXA.contains("icon(app);"));
        let icon = Path::new("/a/icon.png");
        assert_eq!(Look { colors: &c, at: None, icon: Some(icon), image: None }.icon(), icon.as_os_str());
        assert_eq!(Look { colors: &c, at: None, icon: None, image: None }.icon(), "");
    }

    /// The macOS helpers are JavaScript: a syntax error is a download with
    /// no window, found here rather than on a Mac.
    #[test]
    fn the_macos_helpers_parse() {
        let d = tempfile::tempdir().unwrap();
        for (name, js) in [("progress.js", MAC_PROGRESS_JXA), ("ask.js", MAC_ASK_JXA)] {
            std::fs::write(d.path().join(name), [MAC_LOOK_JXA, js].concat()).unwrap();
            let Ok(out) = Command::new("node").arg("--check").arg(d.path().join(name)).output() else {
                return eprintln!("skipped: no node");
            };
            assert!(out.status.success(), "{name}: {}", String::from_utf8_lossy(&out.stderr));
        }
        // The splash above the bar, and no Cancel when the app says so.
        assert!(MAC_PROGRESS_JXA.contains("argv[9]") && MAC_PROGRESS_JXA.contains("argv[8] === '0'"));
        assert!(
            WINDOWS_PROGRESS_PS.contains("$env:AIO_DLG_IMAGE_SIZE")
                && WINDOWS_PROGRESS_PS.contains("$env:AIO_DLG_CANCEL -ne '0'")
        );
    }

    #[test]
    fn the_splash_s_download_text_is_the_download_window_s() {
        let parse = |t: &str| aio::config::Config::parse(&format!("[app]\nid = \"n\"\n{t}")).unwrap();
        let own = "[build.splash]\nimage = \"s.png\"\ndownload_text = \"{pct}%\"";
        assert_eq!(words_of(&parse(own)).text.as_deref(), Some("{pct}%"));
        let never = format!("{own}\nshow = \"never\"\n[build.download_window]\ntext = \"t\"");
        assert_eq!(words_of(&parse(&never)).text.as_deref(), Some("t"), "no splash in the download: the window's own");
        let w = aio::config::DownloadWindow { text: Some("{pct}% of {total} MB".into()), ..Default::default() };
        assert_eq!(download_detail(&w, "N", "1", 25_000_000, Some(100_000_000)), "25% of 100 MB");
    }

    #[test]
    fn powershell_gets_its_script_by_path_never_in_its_command_line() {
        assert_eq!(encoded_command("ab"), "YQBiAA==");
        let cmd = powershell(Path::new("C:/t/progress.ps1"));
        let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(args.len(), 8);
        assert!(!args.iter().any(|a| a.contains("progress.ps1")));
        // Text is read from the environment, never spliced into the script.
        assert!(WINDOWS_PROGRESS_PS.contains("$env:AIO_DLG_TITLE") && WINDOWS_ASK_PS.contains("$env:AIO_DLG_TEXT"));
        assert!(MAC_PROGRESS_JXA.contains("argv[0]") && MAC_ASK_JXA.contains("argv[1]"));
    }

    /// A stand-in zenity that logs what it is fed and, like a Cancel, exits 1
    /// once it reads `stop_at`.
    #[cfg(unix)]
    fn fake_zenity(dir: &Path, stop_at: &str, code: i32) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let (bin, src) = (dir.join("zenity"), dir.join("zenity.sh"));
        let log = dir.join("fed");
        std::fs::write(
            &src,
            format!(
                "#!/bin/sh\necho \"$@\" > '{}.args'\nwhile read -r l; do printf '%s\\n' \"$l\" >> '{}'; \
                 [ \"$l\" = '{stop_at}' ] && exit 1; done\nexit {code}\n",
                log.display(),
                log.display()
            ),
        )
        .unwrap();
        // Copied by `cp`, never written here: a fork by another test thread
        // would inherit our write handle and make running it fail (ETXTBSY).
        assert!(Command::new("cp").arg(&src).arg(&bin).status().unwrap().success());
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        bin
    }

    #[cfg(unix)]
    #[test]
    fn a_closed_zenity_window_is_a_cancel_and_a_question_answers() {
        let d = tempfile::tempdir().unwrap();
        let b = Backend::Zenity { bin: fake_zenity(d.path(), "49", 0), yad: false };
        let c: [[String; 4]; 2] = Default::default();
        let icon = d.path().join("icon.png");
        let look = Look { colors: &c, at: None, icon: Some(&icon), image: None };
        let wears = || std::fs::read_to_string(d.path().join("fed.args")).unwrap().contains("--window-icon=");
        let mut w = b.progress("Notes", "Getting Notes ready", &look).unwrap();
        assert_eq!(w.show(Some(10), "10 MB"), Shown::On);
        assert_eq!(w.show(Some(10), "10 MB"), Shown::On, "the same line is not sent again");
        assert_eq!(w.show(Some(50), "50 MB"), Shown::On);
        let until = Instant::now() + Duration::from_secs(10);
        let shown = loop {
            match w.show(Some(60), "60 MB") {
                Shown::On if Instant::now() < until => std::thread::sleep(Duration::from_millis(20)),
                s => break s,
            }
        };
        assert_eq!(shown, Shown::Cancelled);
        drop(w);
        assert!(wears(), "the download window wears the app's icon");
        let fed = std::fs::read_to_string(d.path().join("fed")).unwrap();
        assert_eq!(fed.lines().filter(|l| l.starts_with("# Getting Notes ready")).count(), 1, "{fed}");
        assert!(fed.starts_with("9\n# Getting Notes ready\\n10 MB\n49\n"), "{fed}");
        // The question: exit 0 is Retry, 1 is Quit.
        assert_eq!(b.ask("Notes", "why", false, &look), Answer::Retry);
        assert!(wears(), "and so does the question");
        let quit = Backend::Zenity { bin: fake_zenity(d.path(), "x", 1), yad: false };
        assert_eq!(quit.ask("Notes", "why", true, &look), Answer::Quit);
        let missing = Backend::Zenity { bin: d.path().join("none"), yad: false };
        assert_eq!(missing.ask("Notes", "why", false, &look), Answer::Quit, "a question that can't be asked is a Quit");
    }

    #[cfg(unix)]
    #[test]
    fn a_window_told_to_close_is_not_a_cancel() {
        let d = tempfile::tempdir().unwrap();
        let b = Backend::Zenity { bin: fake_zenity(d.path(), "never", 0), yad: false };
        let c: [[String; 4]; 2] = Default::default();
        let mut w = b.progress("Notes", "h", &Look { colors: &c, at: None, icon: None, image: None }).unwrap();
        assert_eq!(w.show(None, "Looking"), Shown::On);
        drop(w);
        let fed = std::fs::read_to_string(d.path().join("fed")).unwrap();
        assert!(fed.ends_with("100\n"), "closed with 100 (auto-close): {fed}");
    }
}

//! The OS's own apps, for what the app's code hands them: a web or mail link
//! in the default browser or mail app, a file or folder in its default app, a
//! file shown in its folder. Each answers whether the OS took it (never a
//! silent nothing). The page can't reach these: its own links open only
//! http(s), in main.js.
//!
//! The target is always one argument, never a command line: aio's Windows
//! `cmd /c start` ran `b=2` from `https://h/?a=1&b=2`. Windows gets the shell
//! call itself (no program between), Linux `xdg-open`, macOS `open`.
use std::io;
use std::path::{Path, PathBuf};

/// `url` in the OS's handler for it: http, https and mailto only (any other
/// scheme reaches custom protocol handlers, which can run anything; a file
/// goes through [`open_path`]).
pub async fn open_url(url: &str) -> io::Result<()> {
    let url = checked_url(url)?;
    may_open(&run_env, &url)?;
    launch(What::Open(url.clone()), &url).await
}

/// The file or folder at `path` (relative: to the working directory) in the
/// OS's default app for it: a folder in the file manager.
pub async fn open_path(path: impl AsRef<Path>) -> io::Result<()> {
    let p = existing(path.as_ref())?;
    let shown = p.display().to_string();
    may_open(&run_env, &shown)?;
    launch(What::Open(shown.clone()), &shown).await
}

/// The file manager on `path`'s folder, with `path` selected.
pub async fn reveal(path: impl AsRef<Path>) -> io::Result<()> {
    let p = existing(path.as_ref())?;
    let shown = p.display().to_string();
    may_open(&run_env, &shown)?;
    launch(What::Reveal(p), &shown).await
}

/// The process's variables, `AIORT_NO_OPEN` set in a test's sandbox
/// (`aio::testing`): a test's link or file never reaches the person's apps.
fn run_env(k: &str) -> Option<String> {
    let test = k == "AIORT_NO_OPEN" && aio::paths::in_sandbox();
    if test { Some("1".into()) } else { std::env::var(k).ok() }
}

/// A contained run (`AIORT_NO_OPEN`, as `ax dev` sets for a caller with no
/// terminal, aio's `AIO_NO_OPEN`) hands nothing to the person's browser or file
/// manager: refused, naming what it would have opened.
fn may_open(env: &dyn Fn(&str) -> Option<String>, shown: &str) -> io::Result<()> {
    match env("AIORT_NO_OPEN").filter(|v| !v.is_empty()) {
        Some(_) => Err(io::Error::other(format!("not opened (AIORT_NO_OPEN, a contained run): {shown}"))),
        None => Ok(()),
    }
}

/// `url`, if its scheme may leave the app.
fn checked_url(url: &str) -> io::Result<String> {
    crate::urls::link(url).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
}

/// `path` made absolute (the launcher runs elsewhere), if it is there.
fn existing(path: &Path) -> io::Result<PathBuf> {
    let p = std::path::absolute(path)?;
    std::fs::metadata(&p).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", p.display())))?;
    Ok(p)
}

#[derive(Debug, PartialEq)]
enum What {
    /// A URL or an absolute path.
    Open(String),
    /// An absolute path.
    Reveal(PathBuf),
}

/// One way to hand a target over: a program and its arguments.
#[cfg_attr(windows, allow(dead_code))]
#[derive(Debug, PartialEq)]
struct Launcher {
    cmd: &'static str,
    args: Vec<String>,
}

#[cfg_attr(windows, allow(dead_code))]
/// The launchers to try in turn for `what` on `os` (Unix; Windows calls the
/// shell itself).
fn launchers(os: &str, what: &What) -> Vec<Launcher> {
    let l = |cmd, args: &[&str]| Launcher { cmd, args: args.iter().map(|a| a.to_string()).collect() };
    match (os, what) {
        ("macos", What::Open(t)) => vec![l("open", &[t])],
        ("macos", What::Reveal(p)) => vec![l("open", &["-R", &p.to_string_lossy()])],
        (_, What::Open(t)) => vec![l("xdg-open", &[t])],
        // The freedesktop file manager call (it selects the file); without
        // one, the folder alone.
        (_, What::Reveal(p)) => {
            let folder = p.parent().unwrap_or(p).to_string_lossy();
            vec![
                l(
                    "dbus-send",
                    &[
                        "--session",
                        "--print-reply",
                        "--dest=org.freedesktop.FileManager1",
                        "/org/freedesktop/FileManager1",
                        "org.freedesktop.FileManager1.ShowItems",
                        &format!("array:string:{}", file_uri(p)),
                        "string:",
                    ],
                ),
                l("xdg-open", &[&folder]),
            ]
        }
    }
}

#[cfg_attr(windows, allow(dead_code))]
/// `p` as a `file:` URI, as one item of a `dbus-send` array (where a comma
/// would split it).
fn file_uri(p: &Path) -> String {
    crate::urls::file_uri(p.as_os_str().as_encoded_bytes(), cfg!(windows)).replace(',', "%2C")
}

/// Linux: a window needs a desktop session to appear on; with none, a
/// launcher fails (or opens a text browser nobody sees), so it is refused
/// first. macOS and Windows have one whenever someone is logged in.
pub(crate) fn desktop(os: &str, env: &dyn Fn(&str) -> Option<String>) -> io::Result<()> {
    let set = |k: &str| env(k).is_some_and(|v| !v.is_empty());
    if os != "linux" || set("DISPLAY") || set("WAYLAND_DISPLAY") {
        return Ok(());
    }
    Err(io::Error::other("no desktop session (no DISPLAY or WAYLAND_DISPLAY): nothing can open here"))
}

#[cfg(not(windows))]
async fn launch(what: What, shown: &str) -> io::Result<()> {
    let os = std::env::consts::OS;
    desktop(os, &|k| std::env::var(k).ok())?;
    run(launchers(os, &what), shown).await
}

#[cfg_attr(windows, allow(dead_code))]
/// Try each launcher until one takes the target; the error says what each did.
async fn run(tries: Vec<Launcher>, shown: &str) -> io::Result<()> {
    use std::process::Stdio;
    let mut why = Vec::new();
    for l in tries {
        let mut cmd = tokio::process::Command::new(l.cmd);
        // What opens starts in a neutral folder, with no AppImage or aio
        // settings.
        cmd.args(&l.args).current_dir("/").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        aio::lifecycle::scrub(cmd.as_std_mut(), false);
        match cmd.status().await {
            Ok(s) if s.success() => return Ok(()),
            Ok(s) => why.push(format!("{} {s}", l.cmd)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => why.push(format!("no {}", l.cmd)),
            Err(e) => why.push(format!("{}: {e}", l.cmd)),
        }
    }
    Err(io::Error::other(format!("could not open {shown}: {}", why.join("; "))))
}

#[cfg(windows)]
async fn launch(what: What, shown: &str) -> io::Result<()> {
    let shown = shown.to_string();
    tokio::task::spawn_blocking(move || win::launch(&what, &shown)).await.map_err(io::Error::other)?
}

#[cfg(windows)]
mod win {
    use super::What;
    use std::io;
    use windows_sys::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
    };
    use windows_sys::Win32::UI::Shell::{ILCreateFromPathW, ILFree, SHOpenFolderAndSelectItems, ShellExecuteW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    /// The shell's own calls (COM on this thread, as they ask). What opens
    /// starts in the Windows folder, never the install (an update must be
    /// able to move it).
    pub(super) fn launch(what: &What, shown: &str) -> io::Result<()> {
        // SAFETY: plain Win32 calls on NUL-terminated buffers that outlive them.
        unsafe {
            let com = CoInitializeEx(std::ptr::null(), (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32);
            let r = match what {
                What::Open(t) => {
                    let dir = wide(&std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()));
                    let (op, t) = (wide("open"), wide(t));
                    let h = ShellExecuteW(
                        std::ptr::null_mut(),
                        op.as_ptr(),
                        t.as_ptr(),
                        std::ptr::null(),
                        dir.as_ptr(),
                        SW_SHOWNORMAL,
                    );
                    // Above 32 is success; below, the reason.
                    match h as isize {
                        n if n > 32 => Ok(()),
                        n => Err(io::Error::other(format!("could not open {shown}: Windows said {n}"))),
                    }
                }
                What::Reveal(p) => {
                    let p = wide(&p.to_string_lossy());
                    let item = ILCreateFromPathW(p.as_ptr());
                    if item.is_null() {
                        Err(io::Error::other(format!("could not show {shown}")))
                    } else {
                        let hr = SHOpenFolderAndSelectItems(item, 0, std::ptr::null(), 0);
                        ILFree(item);
                        if hr < 0 { Err(io::Error::other(format!("could not show {shown}: {hr:#x}"))) } else { Ok(()) }
                    }
                }
            };
            if com >= 0 {
                CoUninitialize();
            }
            r
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_web_and_mail_links_leave_the_app() {
        assert_eq!(checked_url("https://h/?a=1&b=2").unwrap(), "https://h/?a=1&b=2");
        assert_eq!(checked_url("mailto:a@b.c").unwrap(), "mailto:a@b.c");
        assert_eq!(checked_url("HTTP://H/x y").unwrap(), "http://h/x%20y");
        for u in ["file:///etc/passwd", "javascript:alert(1)", "ms-settings:", "smb://h/s", "steam://run/1"] {
            let e = checked_url(u).unwrap_err();
            assert_eq!(e.kind(), io::ErrorKind::InvalidInput, "{u}");
            assert!(e.to_string().starts_with("refused"), "{u}: {e}");
        }
        assert_eq!(checked_url("not a url").unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn the_target_is_one_argument_on_every_os() {
        let t = What::Open("a&b; $(x) %PATH%".into());
        assert_eq!(launchers("linux", &t), [Launcher { cmd: "xdg-open", args: vec!["a&b; $(x) %PATH%".into()] }]);
        assert_eq!(launchers("macos", &t), [Launcher { cmd: "open", args: vec!["a&b; $(x) %PATH%".into()] }]);
        assert_eq!(launchers("freebsd", &t)[0].cmd, "xdg-open");
    }

    // The Linux launcher is built on Linux: a `/home/…` path isn't absolute on Windows.
    #[cfg(unix)]
    #[test]
    fn reveal_selects_the_file_and_falls_back_to_its_folder() {
        let p = Path::new("/home/u/a,b c.txt");
        let mac = launchers("macos", &What::Reveal(p.into()));
        assert_eq!(mac, [Launcher { cmd: "open", args: vec!["-R".into(), "/home/u/a,b c.txt".into()] }]);
        let linux = launchers("linux", &What::Reveal(p.into()));
        assert_eq!(linux.len(), 2);
        assert_eq!(linux[0].cmd, "dbus-send");
        // One array item: the comma is escaped, the space too.
        assert!(linux[0].args.contains(&"array:string:file:///home/u/a%2Cb%20c.txt".into()), "{:?}", linux[0]);
        assert_eq!(linux[1], Launcher { cmd: "xdg-open", args: vec!["/home/u".into()] });
    }

    #[test]
    fn a_contained_run_opens_nothing_and_says_what() {
        let e = may_open(&|k| (k == "AIORT_NO_OPEN").then(|| "1".into()), "https://h/").unwrap_err();
        assert_eq!(e.to_string(), "not opened (AIORT_NO_OPEN, a contained run): https://h/");
        assert!(may_open(&|_| Some(String::new()), "x").is_ok() && may_open(&|_| None, "x").is_ok());
    }

    #[test]
    fn linux_without_a_desktop_refuses_up_front() {
        let none = |_: &str| None;
        let empty = |_: &str| Some(String::new());
        assert!(desktop("linux", &none).unwrap_err().to_string().contains("no desktop session"));
        assert!(desktop("linux", &empty).is_err());
        assert!(desktop("linux", &|k| (k == "WAYLAND_DISPLAY").then(|| "wayland-0".into())).is_ok());
        assert!(desktop("macos", &none).is_ok() && desktop("windows", &none).is_ok());
    }

    #[test]
    fn a_missing_file_is_an_error_naming_it() {
        let e = existing(Path::new("no-such-file-here.md")).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
        assert!(e.to_string().contains("no-such-file-here.md"));
        let d = tempfile::tempdir().unwrap();
        assert_eq!(existing(d.path()).unwrap(), d.path());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn each_launcher_is_tried_until_one_takes_it_and_failures_are_told() {
        let l = |cmd: &'static str| Launcher { cmd, args: vec![] };
        assert!(run(vec![l("aio-no-such-launcher"), l("false"), l("true")], "x").await.is_ok());
        let e = run(vec![l("aio-no-such-launcher"), l("false")], "/a b").await.unwrap_err();
        assert_eq!(e.to_string(), "could not open /a b: no aio-no-such-launcher; false exit status: 1");
    }
}

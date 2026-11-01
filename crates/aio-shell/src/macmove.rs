//! macOS: offer to move the app into Applications (aio macos-move). A `.app`
//! opened from its disk image or from Downloads runs where an update can't
//! replace it: a read-only image, or the read-only copy macOS makes of a
//! quarantined app (App Translocation). So once its window is up the app
//! asks, once: "Move" copies the bundle into Applications (whole, with
//! `ditto`), clears the quarantine mark from the copy, and the app restarts
//! from there with the same arguments; "Not Now" is remembered. An update
//! that can't replace the app is refused before a byte is downloaded,
//! saying why ([`blocker`]). Every decision is a pure function here, so it
//! is tested on any OS; only `run` touches a Mac.
use std::io;
use std::path::Path;

/// `AIORT_MOVE_TO_APPLICATIONS`: `never` (or off, 0, no): no offer; `move`:
/// move without asking (an unattended install); else ask.
pub const ENV: &str = "AIORT_MOVE_TO_APPLICATIONS";
/// In the app's data: the user said "Not Now".
pub const DECLINED: &str = "macos-move-declined";

/// The `.app` an executable runs in (`…/X.app/Contents/MacOS/x`).
pub fn app_dir(exe: &str) -> Option<&str> {
    let at = exe.find(".app/Contents/MacOS/")?;
    Some(&exe[..at + 4])
}

fn parent(p: &str) -> &str {
    p.rfind('/').map_or("", |i| if i == 0 { "/" } else { &p[..i] })
}

fn name(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// What a move would be: the bundle, and where it goes.
#[derive(Debug, PartialEq, Eq)]
pub struct Offer {
    pub app: String,
    pub dest: String,
}

/// The facts an offer is decided on (handed in: every case is a test).
pub struct Facts<'a> {
    pub os: &'a str,
    pub exe: &'a str,
    pub home: Option<&'a str>,
    /// [`ENV`].
    pub env: Option<&'a str>,
    pub declined: bool,
    pub exists: &'a dyn Fn(&str) -> bool,
    pub can_write: &'a dyn Fn(&str) -> bool,
}

/// Where the running bundle would move, or `None`: not macOS, not a bundle,
/// in an Applications folder already, a bundle of that name there already
/// (nothing here replaces one), installed by `ax` (its `versions/`
/// layout), switched off, or declined (unless [`ENV`] says `move`).
pub fn offer(f: &Facts) -> Option<Offer> {
    let mode = f.env.unwrap_or("").trim().to_lowercase();
    if f.os != "macos" || ["never", "0", "false", "off", "no"].contains(&mode.as_str()) {
        return None;
    }
    let app = app_dir(f.exe)?;
    let user_apps = f.home.map(|h| format!("{}/Applications", h.trim_end_matches('/')));
    if app.starts_with("/Applications/") || user_apps.as_ref().is_some_and(|u| app.starts_with(&format!("{u}/"))) {
        return None;
    }
    let n = name(app);
    let root = match parent(parent(app)) {
        r if name(r) == "versions" => parent(r),
        _ => parent(app),
    };
    let installed = (f.exists)(&format!("{root}/versions")) && (f.exists)(&format!("{root}/{n}"));
    let taken = (f.exists)(&format!("/Applications/{n}"))
        || user_apps.as_ref().is_some_and(|u| (f.exists)(&format!("{u}/{n}")));
    if installed || taken || (f.declined && mode != "move") {
        return None;
    }
    let dir = if (f.can_write)("/Applications") { "/Applications".to_string() } else { user_apps? };
    Some(Offer { app: app.into(), dest: format!("{dir}/{n}") })
}

/// Why the bundle `app` can't update itself where it runs, if it can't:
/// translocated, or in a folder this user can't write.
pub fn blocker(app: &str, can_write: &dyn Fn(&str) -> bool) -> Option<String> {
    let n = name(app);
    let why = if app.contains("/AppTranslocation/") {
        "macOS is running it from a temporary read-only copy (App Translocation: it was opened from the disk \
         image or from Downloads)"
            .to_string()
    } else if !can_write(parent(app)) {
        format!("its folder ({}) is not writable by this user", parent(app))
    } else {
        return None;
    };
    Some(if parent(app) == "/Applications" {
        format!(
            "{n} can't update itself: {why}. Update it from an administrator account, or install it in ~/Applications"
        )
    } else {
        format!("{n} can't update itself: {why}. Move {n} to /Applications, open it from there, then update")
    })
}

/// The question as `osascript` arguments: the text travels as arguments,
/// never inside the script, so a title with a quote is a title.
/// `updates_here`: the bundle can update where it is (the reason differs).
pub fn dialog_args(title: &str, icon: Option<&str>, updates_here: bool) -> Vec<String> {
    let with_icon = if icon.is_some() { " with icon POSIX file (item 3 of argv)" } else { "" };
    let why = if updates_here {
        "It is not in Applications, where macOS and Launchpad look for apps. "
    } else {
        "It was opened from where it was downloaded. There it cannot update itself. "
    };
    let mut a: Vec<String> = [
        "on run argv",
        "tell me to activate",
        &format!(
            "set r to display dialog (item 1 of argv) with title (item 2 of argv) buttons {{\"Not Now\", \"Move to \
             Applications\"}} default button 2 cancel button 1{with_icon} giving up after 600"
        ),
        "if gave up of r then return \"gave up\"",
        "return button returned of r",
        "end run",
    ]
    .iter()
    .flat_map(|l| ["-e".to_string(), l.to_string()])
    .collect();
    a.push(format!(
        "Move {title} to your Applications folder?\n\n{why}Moving copies it into Applications and reopens it from there."
    ));
    a.push(title.into());
    a.extend(icon.map(String::from));
    a
}

/// What the user said.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Move,
    Later,
    /// Not shown, left alone, failed: nothing is remembered.
    Unanswered,
}

/// `osascript`'s exit as the answer: "Not Now" is the cancel button (exit
/// 1, "User canceled", -128).
pub fn answer(code: Option<i32>, stdout: &str, stderr: &str) -> Answer {
    match code {
        Some(0) if stdout.trim() == "Move to Applications" => Answer::Move,
        Some(0) => Answer::Unanswered,
        _ if stderr.contains("-128") || stderr.to_lowercase().contains("user canceled") => Answer::Later,
        _ => Answer::Unanswered,
    }
}

/// What the moved app is opened with: through LaunchServices, as a Dock
/// click, waiting for this process to be gone, with the same arguments.
pub fn open_args(dest: &str, args: &[String], pid: u32) -> Vec<String> {
    let mut a = vec!["-n".into(), dest.into(), "--env".into(), format!("{}={pid}", crate::update::RELAUNCH)];
    if !args.is_empty() {
        a.push("--args".into());
        a.extend(args.iter().cloned());
    }
    a
}

/// What happened, for the log and the tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    None,
    Moved,
    Later,
    Unanswered,
    Failed,
}

/// A command's (exit code, stdout, stderr).
pub type Ran = io::Result<(Option<i32>, String, String)>;

/// The I/O the offer needs (a Mac's, or a test's).
pub struct Deps<'a> {
    pub facts: Facts<'a>,
    pub title: &'a str,
    pub data: &'a Path,
    pub args: &'a [String],
    pub pid: u32,
    pub run: &'a dyn Fn(&str, &[String]) -> Ran,
    /// Start the moved app (detached), then stop this one.
    pub hand_over: &'a dyn Fn(&str, &[String]) -> io::Result<()>,
}

/// Ask, and on "Move" copy, then hand over to the copy. Never fails the
/// app: what went wrong is said, and it runs on where it is.
pub fn ask(d: &Deps) -> Outcome {
    match offer(&d.facts) {
        Some(o) => ask_to(d, o),
        None => Outcome::None,
    }
}

fn ask_to(d: &Deps, o: Offer) -> Outcome {
    let unasked = d.facts.env.is_some_and(|e| e.trim().eq_ignore_ascii_case("move"));
    let said = if unasked {
        Answer::Move
    } else {
        let icon = format!("{}/Contents/Resources/AppIcon.icns", o.app);
        let icon = (d.facts.exists)(&icon).then_some(icon.as_str());
        let here = blocker(&o.app, d.facts.can_write).is_none();
        match (d.run)("/usr/bin/osascript", &dialog_args(d.title, icon, here)) {
            Ok((code, out, err)) => answer(code, &out, &err),
            Err(_) => Answer::Unanswered,
        }
    };
    match said {
        Answer::Unanswered => Outcome::Unanswered,
        Answer::Later => {
            let f = d.data.join(DECLINED);
            _ = std::fs::write(&f, "");
            eprintln!(
                "[aio] not moved to Applications (\"Not Now\"): not asked again; delete {} to be asked",
                f.display()
            );
            Outcome::Later
        }
        Answer::Move => match copy(&o, d) {
            Err(e) => {
                eprintln!("[aio] could not move the app to Applications ({e}): it runs on from where it is");
                Outcome::Failed
            }
            Ok(()) => match (d.hand_over)("/usr/bin/open", &open_args(&o.dest, d.args, d.pid)) {
                Ok(()) => {
                    eprintln!("[aio] copied {} to {}: this copy closes, that one opens", o.app, o.dest);
                    Outcome::Moved
                }
                Err(e) => {
                    eprintln!("[aio] the app is in {}, but it could not be opened ({e}): open it from there", o.dest);
                    Outcome::Failed
                }
            },
        },
    }
}

/// `ditto` the bundle beside its destination, clear the quarantine mark of
/// the copy, and rename it into place: a copy cut short is never a bundle
/// named like the app, and is removed.
fn copy(o: &Offer, d: &Deps) -> io::Result<()> {
    let staged = format!("{}.moving-{}", o.dest, d.pid);
    let made = (|| {
        std::fs::create_dir_all(parent(&o.dest))?;
        match (d.run)("/usr/bin/ditto", &[o.app.clone(), staged.clone()])? {
            (Some(0), _, _) => {}
            (code, _, err) => return Err(io::Error::other(format!("ditto exited {code:?}: {}", err.trim()))),
        }
        // Not fatal: macOS may still translocate the copy, and an update then says so.
        if let Ok((code, _, err)) =
            (d.run)("/usr/bin/xattr", &["-dr".into(), "com.apple.quarantine".into(), staged.clone()])
            && code != Some(0)
        {
            eprintln!("[aio] the quarantine mark stays on {staged} ({}): it may not update itself", err.trim());
        }
        std::fs::rename(&staged, &o.dest)
    })();
    if made.is_err() {
        _ = std::fs::remove_dir_all(&staged);
    }
    made
}

/// The offer's dialog while it is up (its pid): it closes with the app.
#[cfg(target_os = "macos")]
static ASKING: std::sync::Mutex<Option<u32>> = std::sync::Mutex::new(None);

/// The app's window closed: an offer still up goes with it (else the dialog
/// outlives the app, asking about an app that is gone).
#[cfg(target_os = "macos")]
pub fn close() {
    if let Some(pid) = ASKING.lock().unwrap_or_else(|e| e.into_inner()).take() {
        // SAFETY: a signal to the dialog this process started and still waits on.
        unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    }
}

/// This Mac's offer, once the window is up (a dialog before it takes the
/// app's place in the Dock).
#[cfg(target_os = "macos")]
pub fn run(title: &str, data: &Path) {
    let exe = std::env::current_exe().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    let (home, env) = (std::env::var("HOME").ok(), std::env::var(ENV).ok());
    let exists = |p: &str| std::fs::symlink_metadata(p).is_ok();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let run = |cmd: &str, a: &[String]| -> Ran {
        let child = std::process::Command::new(cmd)
            .args(a)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;
        *ASKING.lock().unwrap_or_else(|e| e.into_inner()) = Some(child.id());
        let o = child.wait_with_output();
        *ASKING.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let o = o?;
        Ok((o.status.code(), String::from_utf8_lossy(&o.stdout).into(), String::from_utf8_lossy(&o.stderr).into()))
    };
    let hand_over = |cmd: &str, a: &[String]| -> io::Result<()> {
        std::process::Command::new(cmd).args(a).spawn()?;
        aio::lifecycle::ask_stop();
        Ok(())
    };
    let d = Deps {
        facts: Facts {
            os: "macos",
            exe: &exe,
            home: home.as_deref(),
            env: env.as_deref(),
            declined: data.join(DECLINED).exists(),
            exists: &exists,
            can_write: &can_write,
        },
        title,
        data,
        args: &args,
        pid: std::process::id(),
        run: &run,
        hand_over: &hand_over,
    };
    ask(&d);
}

/// Can this user make a file in `dir`?
pub fn can_write(dir: &str) -> bool {
    let probe = Path::new(dir).join(format!(".aio-probe-{}", std::process::id()));
    std::fs::create_dir(&probe).and_then(|()| std::fs::remove_dir(&probe)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const EXE: &str = "/Volumes/Counter/Counter.app/Contents/MacOS/counter";

    fn no(_: &str) -> bool {
        false
    }
    fn yes(_: &str) -> bool {
        true
    }

    fn facts<'a>() -> Facts<'a> {
        Facts {
            os: "macos",
            exe: EXE,
            home: Some("/Users/u"),
            env: None,
            declined: false,
            exists: &no,
            can_write: &yes,
        }
    }

    #[test]
    fn an_app_outside_applications_is_offered_applications() {
        // aio tests/macos-move.test.ts "moveOffer".
        let o = offer(&facts()).unwrap();
        assert_eq!((o.app.as_str(), o.dest.as_str()), ("/Volumes/Counter/Counter.app", "/Applications/Counter.app"));
        let tl = "/private/var/folders/x/T/AppTranslocation/ABC/d/Counter.app/Contents/MacOS/counter";
        assert_eq!(offer(&Facts { exe: tl, ..facts() }).unwrap().dest, "/Applications/Counter.app");
        let not_apps = |d: &str| d != "/Applications";
        assert_eq!(
            offer(&Facts { can_write: &not_apps, ..facts() }).unwrap().dest,
            "/Users/u/Applications/Counter.app"
        );
        assert_eq!(offer(&Facts { can_write: &no, home: None, ..facts() }), None);
    }

    #[test]
    fn nothing_is_offered_where_nothing_should_move() {
        let at_apps = |p: &str| p == "/Applications/Counter.app";
        let at_home = |p: &str| p == "/Users/u/Applications/Counter.app";
        let aio_install = |p: &str| p == "/Users/u/app/counter/versions" || p == "/Users/u/app/counter/Counter.app";
        let cases: Vec<(&str, Facts)> = vec![
            ("not macOS", Facts { os: "linux", ..facts() }),
            ("not a bundle", Facts { exe: "/usr/local/bin/counter", ..facts() }),
            ("in /Applications", Facts { exe: "/Applications/Counter.app/Contents/MacOS/counter", ..facts() }),
            ("in a folder of it", Facts { exe: "/Applications/Tools/Counter.app/Contents/MacOS/counter", ..facts() }),
            ("in ~/Applications", Facts { exe: "/Users/u/Applications/Counter.app/Contents/MacOS/counter", ..facts() }),
            ("a copy is installed", Facts { exists: &at_apps, ..facts() }),
            ("a copy in ~/Applications", Facts { exists: &at_home, ..facts() }),
            (
                "installed by aio (versioned)",
                Facts {
                    exe: "/Users/u/app/counter/versions/1.0.0/Counter.app/Contents/MacOS/counter",
                    exists: &aio_install,
                    ..facts()
                },
            ),
            (
                "installed by aio (the stable link)",
                Facts {
                    exe: "/Users/u/app/counter/Counter.app/Contents/MacOS/counter",
                    exists: &aio_install,
                    ..facts()
                },
            ),
            ("Not Now before", Facts { declined: true, ..facts() }),
            ("switched off", Facts { env: Some("never"), ..facts() }),
            ("switched off (0)", Facts { env: Some("0"), ..facts() }),
        ];
        for (why, f) in cases {
            assert_eq!(offer(&f), None, "{why}");
        }
        assert!(offer(&Facts { declined: true, env: Some("move"), ..facts() }).is_some(), "move outranks Not Now");
        let only = |p: &str| p == "/Users/u/versions";
        let unpacked = "/Users/u/versions/1.0.0/Counter.app/Contents/MacOS/counter";
        assert!(offer(&Facts { exe: unpacked, exists: &only, ..facts() }).is_some(), "a folder named versions");
        assert!(offer(&Facts { exe: "/Applications2/Counter.app/Contents/MacOS/counter", ..facts() }).is_some());
    }

    #[test]
    fn an_update_is_refused_where_the_app_cant_replace_itself_saying_how() {
        let tl = "/private/var/folders/x/AppTranslocation/A/d/Counter.app";
        assert!(
            blocker(tl, &yes).unwrap().contains("App Translocation") && blocker(tl, &yes).unwrap().contains("Move")
        );
        assert!(blocker("/Volumes/C/Counter.app", &no).unwrap().contains("(/Volumes/C) is not writable"));
        assert!(blocker("/Applications/Counter.app", &no).unwrap().contains("administrator"));
        assert_eq!(blocker("/Users/u/Applications/Counter.app", &yes), None);
    }

    #[test]
    fn the_dialog_says_its_text_as_arguments_and_its_answer_is_read() {
        let title = r#"Dan’s "App" \ end"#;
        let a = dialog_args(title, Some("/x/AppIcon.icns"), false);
        let script: Vec<&String> = a.iter().skip(1).step_by(2).take(6).collect();
        assert!(
            script.iter().all(|l| !l.contains("Dan")) && script[2].contains("with icon POSIX file (item 3 of argv)")
        );
        assert_eq!((a[a.len() - 2].as_str(), a[a.len() - 1].as_str()), (title, "/x/AppIcon.icns"));
        assert!(a[a.len() - 3].contains("cannot update itself"));
        let plain = dialog_args(title, None, true);
        assert!(plain.last() == Some(&title.to_string()) && !plain.join("\n").contains("with icon"));
        assert!(!plain.join("\n").contains("cannot update itself"), "the reason is true of where it runs");
        assert_eq!(answer(Some(0), "Move to Applications\n", ""), Answer::Move);
        assert_eq!(answer(Some(1), "", "execution error: User canceled. (-128)"), Answer::Later);
        assert_eq!(answer(Some(0), "gave up\n", ""), Answer::Unanswered);
        assert_eq!(answer(Some(1), "", "no user interaction allowed (-1713)"), Answer::Unanswered);
        assert_eq!(
            open_args("/Applications/C.app", &["--port=1".into()], 42),
            ["-n", "/Applications/C.app", "--env", "AIORT_RELAUNCH_AFTER=42", "--args", "--port=1"]
        );
    }

    /// The offer driven with a fake Mac in `t`: what ran, what was opened.
    fn drive(
        t: &Path,
        env: Option<&str>,
        says: &str,
        ditto_ok: bool,
        open_ok: bool,
    ) -> (Outcome, Vec<String>, Vec<String>) {
        let calls = RefCell::new(vec![]);
        let opened = RefCell::new(vec![]);
        let dest_dir = t.join("Applications");
        let run = |cmd: &str, a: &[String]| -> Ran {
            calls.borrow_mut().push(name(cmd).to_string());
            match name(cmd) {
                "osascript" if says == "move" => Ok((Some(0), "Move to Applications\n".into(), String::new())),
                "osascript" if says == "later" => Ok((Some(1), String::new(), "User canceled. (-128)".into())),
                "osascript" => Ok((Some(0), "gave up".into(), String::new())),
                "ditto" if ditto_ok => std::fs::create_dir_all(&a[1]).map(|()| (Some(0), String::new(), String::new())),
                "ditto" => {
                    std::fs::create_dir_all(&a[1])?;
                    Ok((Some(1), String::new(), "No space left".into()))
                }
                _ => Ok((Some(0), String::new(), String::new())),
            }
        };
        let hand_over = |cmd: &str, a: &[String]| -> io::Result<()> {
            opened.borrow_mut().push(format!("{} {}", name(cmd), a.join(" ")));
            if open_ok { Ok(()) } else { Err(io::Error::other("no LaunchServices")) }
        };
        let dest = dest_dir.to_string_lossy().into_owned();
        let writable = move |d: &str| d == "/Applications";
        let exists = |_: &str| false;
        let out = {
            let d = Deps {
                facts: Facts { env, can_write: &writable, exists: &exists, ..facts() },
                title: "Counter",
                data: t,
                args: &[],
                pid: 7,
                run: &run,
                hand_over: &hand_over,
            };
            // Applications is the temp dir here: the real one is never touched.
            let o = offer(&d.facts).unwrap();
            let fake = Offer { app: o.app, dest: format!("{dest}/Counter.app") };
            ask_to(&d, fake)
        };
        (out, calls.into_inner(), opened.into_inner())
    }

    #[test]
    fn move_copies_clears_the_mark_and_hands_over_and_not_now_is_remembered() {
        // aio tests/macos-move.test.ts "offer: …".
        let t = tempfile::tempdir().unwrap();
        let (o, calls, opened) = drive(t.path(), None, "move", true, true);
        assert_eq!((o, calls), (Outcome::Moved, ["osascript", "ditto", "xattr"].map(String::from).to_vec()));
        assert!(t.path().join("Applications/Counter.app").is_dir());
        assert!(opened[0].starts_with("open -n ") && opened[0].contains("--env AIORT_RELAUNCH_AFTER=7"));
        assert!(!t.path().join(DECLINED).exists());
        let t = tempfile::tempdir().unwrap();
        let (o, calls, opened) = drive(t.path(), None, "later", true, true);
        assert_eq!((o, calls.len(), opened.len()), (Outcome::Later, 1, 0));
        assert!(t.path().join(DECLINED).exists());
        let (o, _, _) = drive(t.path(), Some("move"), "", true, true);
        assert_eq!(o, Outcome::Moved, "move: without asking, past Not Now");
        let t = tempfile::tempdir().unwrap();
        assert_eq!(drive(t.path(), None, "", true, true).0, Outcome::Unanswered);
        assert!(!t.path().join(DECLINED).exists(), "nobody answered: nothing remembered");
        // A copy that fails leaves nothing behind, and nothing is opened.
        let (o, _, opened) = drive(t.path(), None, "move", false, true);
        assert_eq!((o, opened.len()), (Outcome::Failed, 0));
        assert_eq!(std::fs::read_dir(t.path().join("Applications")).unwrap().count(), 0);
        // Copied, but not opened: the app runs on (it is a whole copy there).
        let t = tempfile::tempdir().unwrap();
        assert_eq!(drive(t.path(), None, "move", true, false).0, Outcome::Failed);
        assert!(t.path().join("Applications/Counter.app").is_dir());
    }

    /// On a Mac, with its own `ditto` and `xattr`: the bundle is copied
    /// whole (its link kept), the copy loses the quarantine mark the
    /// original keeps, and a folder this user can't write blocks an update.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_mac_copies_the_bundle_whole_and_unquarantined() {
        use std::process::Command;
        let t = tempfile::tempdir().unwrap();
        let app = t.path().join("Downloads/Counter.app");
        let macos = app.join("Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        std::fs::write(macos.join("counter"), "#!/bin/sh\n").unwrap();
        std::os::unix::fs::symlink("counter", macos.join("link")).unwrap();
        let mark = |p: &Path, set: bool| {
            let a: &[&str] = if set {
                &["-w", "com.apple.quarantine", "0081;00000000;aio;"]
            } else {
                &["-p", "com.apple.quarantine"]
            };
            Command::new("/usr/bin/xattr").args(a).arg(p).output().unwrap().status.success()
        };
        assert!(mark(&app, true));
        let run = |cmd: &str, a: &[String]| -> Ran {
            let o = Command::new(cmd).args(a).output()?;
            Ok((o.status.code(), String::from_utf8_lossy(&o.stdout).into(), String::from_utf8_lossy(&o.stderr).into()))
        };
        let hand_over = |_: &str, _: &[String]| Ok(());
        let exe = macos.join("counter").display().to_string();
        let d = Deps {
            facts: Facts { exe: &exe, ..facts() },
            title: "Counter",
            data: t.path(),
            args: &[],
            pid: 7,
            run: &run,
            hand_over: &hand_over,
        };
        let dest = t.path().join("Applications/Counter.app");
        let o = Offer { app: app.display().to_string(), dest: dest.display().to_string() };
        copy(&o, &d).unwrap();
        assert_eq!(std::fs::read_link(dest.join("Contents/MacOS/link")).unwrap(), Path::new("counter"));
        assert!(mark(&app, false) && !mark(&dest, false), "only the copy is unmarked");
        assert!(!t.path().join("Applications/Counter.app.moving-7").exists());
        assert!(can_write(&t.path().display().to_string()) && !can_write("/System"));
        assert!(
            blocker("/System/Applications/Counter.app", &can_write).unwrap().contains("is not writable by this user")
        );
        assert_eq!(blocker(&dest.display().to_string(), &can_write), None);
    }
}

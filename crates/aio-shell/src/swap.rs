//! The update swap on macOS, Windows and of a Linux AppImage, as the
//! Linux helper does with `current`: run from the staged new version (never
//! from the place it
//! fills), it waits for the old app to exit, copies the new version into the
//! app's place (the old one set aside beside it), starts it and waits for it
//! to claim its first boot. A version that exits before that is started once
//! more ([`BOOTS`]); if it still doesn't claim it, the helper stops it, puts
//! the old version back and starts that. A swap that could not be made (the
//! place held) says so in [`SWAP_FAILED`] instead, so the release is tried
//! again. Some version always ends up running.
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// The argument that makes an app binary the swap helper.
pub const FLAG: &str = "--aio-update-swap";
/// How many starts a new version gets to claim its first boot (aio's two
/// boot attempts: one failure can be a port taken or a machine still busy).
pub const BOOTS: u32 = 2;
/// Beside `failed`: the swap was never made, the old version runs as it was.
pub const SWAP_FAILED: &str = "swap-failed";
/// Beside `first_boot`: the new version gave its first boot up (the user
/// asked for the version before it, `firstrun`), so it is not started again.
pub const BACK: &str = "first-boot.back";

/// What the helper does (its one other argument, as JSON).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub id: String,
    /// The old app: the swap starts once it is gone.
    pub parent: u32,
    /// The app's place: its `.app` bundle, its `.exe`, or its AppImage.
    pub live: PathBuf,
    /// The staged new version, copied into `live`.
    pub new: PathBuf,
    /// What to start, inside `live`.
    pub launch: PathBuf,
    pub first_boot: PathBuf,
    pub failed: PathBuf,
    pub wait: Duration,
    /// The app's arguments, for the version that starts (a plan from an
    /// older app has none).
    #[serde(default)]
    pub args: Vec<String>,
    /// A supervisor starts each version (`AIO_SUPERVISED`): the helper only
    /// swaps, waits for the first boot and puts the old one back.
    #[serde(default)]
    pub by_supervisor: bool,
}

/// Where the old version waits beside `live` until the new one is kept.
pub fn aside(live: &Path) -> PathBuf {
    let name = live.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    live.with_file_name(format!(".{name}.aio-old"))
}

fn remove(p: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(p),
        Ok(_) => std::fs::remove_file(p),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// `from` copied into `to`, folder by folder (Windows has no `cp`).
pub(crate) fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        match e.file_type()?.is_dir() {
            true => copy_dir(&e.path(), &to.join(e.file_name()))?,
            false => std::fs::copy(e.path(), to.join(e.file_name())).map(drop)?,
        }
    }
    Ok(())
}

/// Copy a version into place: a file, or a bundle with its links and
/// signature intact (`ditto` on macOS), or on Windows a folder.
fn copy(from: &Path, to: &Path) -> io::Result<()> {
    if !from.is_dir() {
        return std::fs::copy(from, to).map(drop);
    }
    if cfg!(windows) {
        return copy_dir(from, to);
    }
    let tool = if cfg!(target_os = "macos") { "/usr/bin/ditto" } else { "cp" };
    let mut cmd = Command::new(tool);
    if !cfg!(target_os = "macos") {
        cmd.args(["-R", "-P"]);
    }
    match cmd.arg(from).arg(to).status()? {
        s if s.success() => Ok(()),
        s => Err(io::Error::other(format!("{tool} {}: {s}", from.display()))),
    }
}

/// Linux: `cmd`'s process gets no descriptor of this one but its three. An
/// AppImage's runtime keeps its mount (and itself) alive while any process
/// holds one of its descriptors, so a version started from an AppImage held
/// the one before it up for as long as it ran.
pub(crate) fn inherit_none(#[cfg_attr(not(target_os = "linux"), allow(unused))] cmd: &mut Command) {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: one syscall in the child before exec. Close on exec, not
        // now: std's own exec-error pipe must stay open until then.
        unsafe {
            cmd.pre_exec(|| {
                libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, libc::CLOSE_RANGE_CLOEXEC);
                Ok(())
            });
        }
    }
}

/// Start `p` with `args`. A file just written can be "busy" for a moment on
/// Linux (another process's fork still holds it open for writing): try again.
fn start(p: &Path, args: &[String]) -> io::Result<std::process::Child> {
    let mut tries = 0;
    loop {
        let mut cmd = Command::new(p);
        inherit_none(&mut cmd);
        let r = cmd
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        match r {
            Err(e) if e.kind() == io::ErrorKind::ExecutableFileBusy && tries < 20 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(50));
            }
            r => return r,
        }
    }
}

/// Do the swap. Its own log lines go to stderr (the update log).
pub fn run(plan: &Plan) -> io::Result<()> {
    let start = || match plan.by_supervisor {
        true => Ok(None),
        false => start(&plan.launch, &plan.args).map(Some),
    };
    aio::lifecycle::wait_gone(plan.parent, Duration::from_secs(60));
    let old = aside(&plan.live);
    remove(&old)?;
    let unmade = plan.failed.with_file_name(SWAP_FAILED);
    // Nothing moved yet: the old version stays, the swap marked unmade.
    if let Err(e) = std::fs::rename(&plan.live, &old) {
        _ = std::fs::rename(&plan.first_boot, &unmade);
        start()?;
        return Err(io::Error::new(e.kind(), format!("could not set {} aside: {e}", plan.live.display())));
    }
    let back = || -> io::Result<()> {
        remove(&plan.live)?;
        std::fs::rename(&old, &plan.live)?;
        start().map(drop)
    };
    if let Err(e) = copy(&plan.new, &plan.live) {
        eprintln!("ax update: could not put {} in place: {e}", plan.new.display());
        _ = std::fs::rename(&plan.first_boot, &unmade);
        return back();
    }
    let end = Instant::now() + plan.wait;
    // Supervised, a new version that dies is started again until the wait ends.
    let mut child = start()?;
    let exited =
        |c: &mut Option<std::process::Child>| c.as_mut().map_or(Ok(None), |c| c.try_wait()).map(|e| e.is_some());
    for boot in 1..=BOOTS {
        while plan.first_boot.exists() && Instant::now() < end && !exited(&mut child)? {
            std::thread::sleep(Duration::from_millis(200));
        }
        if plan.first_boot.with_file_name(BACK).exists() {
            eprintln!("ax update: the new version asked for the version before it");
            break;
        }
        if boot == BOOTS || !plan.first_boot.exists() || Instant::now() >= end || !exited(&mut child)? {
            break;
        }
        eprintln!("ax update: the new version exited before its first boot; starting it once more");
        child = start()?;
    }
    // Claiming `first-boot` first decides it: the new version, or this.
    if std::fs::rename(&plan.first_boot, &plan.failed).is_ok() {
        eprintln!("ax update: the new version did not start; back to the old one");
        _ = crate::instance::quit(&plan.id);
        let stop = Instant::now() + Duration::from_secs(10);
        if let Some(child) = child.as_mut() {
            while child.try_wait()?.is_none() && Instant::now() < stop {
                std::thread::sleep(Duration::from_millis(200));
            }
            _ = child.kill();
            _ = child.wait();
        }
        return back();
    }
    remove(&old)
}

/// The helper's command line, from the staged binary `helper`.
pub fn command(helper: &Path, plan: &Plan) -> io::Result<Command> {
    let mut cmd = Command::new(helper);
    cmd.arg(FLAG).arg(aio::serde_json::to_string(plan)?);
    Ok(cmd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn a_started_version_holds_none_of_the_starters_descriptors() {
        let f = std::fs::File::open("/proc/self/stat").unwrap();
        // SAFETY: dup of a descriptor this test owns; dup's copy is inherited.
        let fd = unsafe { libc::dup(std::os::fd::AsRawFd::as_raw_fd(&f)) };
        let seen = |none: bool| {
            let mut cmd = Command::new("/bin/sh");
            cmd.args(["-c", &format!("test -e /proc/self/fd/{fd}")]);
            if none {
                inherit_none(&mut cmd);
            }
            cmd.status().unwrap().success()
        };
        assert!(seen(false), "a plain child holds it");
        assert!(!seen(true));
        // SAFETY: closing the dup made above.
        unsafe { libc::close(fd) };
    }

    /// A fake version: a script that claims its first boot (or not) and says
    /// which version ran, with its first argument. On Windows a `.cmd`.
    fn version(dir: &Path, name: &str, claims: bool, mark: &Path, fb: &Path) -> PathBuf {
        let (p, claim) = (script(dir, name), if claims { remove_cmd(fb) } else { exit_cmd(3) });
        write_script(&p, &format!("{}\n{claim}\n", echo_cmd(&format!("{name} {}", ARG1), mark)));
        p
    }

    #[cfg(unix)]
    const ARG1: &str = "\"$1\"";
    #[cfg(windows)]
    const ARG1: &str = "%~1";

    fn script(dir: &Path, name: &str) -> PathBuf {
        dir.join(if cfg!(windows) { format!("{name}.cmd") } else { name.into() })
    }

    #[cfg(unix)]
    fn write_script(p: &Path, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(p, format!("#!/bin/sh\n{body}")).unwrap();
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    #[cfg(windows)]
    fn write_script(p: &Path, body: &str) {
        std::fs::write(p, format!("@echo off\r\n{}", body.replace('\n', "\r\n"))).unwrap();
    }

    #[cfg(unix)]
    fn echo_cmd(text: &str, to: &Path) -> String {
        format!("echo {text} >> '{}'", to.display())
    }
    #[cfg(windows)]
    fn echo_cmd(text: &str, to: &Path) -> String {
        format!("echo {text}>>\"{}\"", to.display())
    }

    #[cfg(unix)]
    fn remove_cmd(p: &Path) -> String {
        format!("rm -f '{}'", p.display())
    }
    #[cfg(windows)]
    fn remove_cmd(p: &Path) -> String {
        format!("del /q \"{}\"", p.display())
    }

    #[cfg(unix)]
    fn exit_cmd(n: u8) -> String {
        format!("exit {n}")
    }
    #[cfg(windows)]
    fn exit_cmd(n: u8) -> String {
        format!("exit /b {n}")
    }

    /// What ran, once `n` versions have said so (they start detached).
    fn ran(mark: &Path, n: usize) -> String {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            let got = std::fs::read_to_string(mark).unwrap_or_default().replace("\r\n", "\n");
            if got.lines().count() >= n || Instant::now() > end {
                return got;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Does the version in place say `name`?
    fn in_place(d: &Path, name: &str) -> bool {
        std::fs::read_to_string(script(d, "app")).is_ok_and(|t| t.contains(&format!("echo {name} ")))
    }

    fn plan(d: &Path, new: PathBuf) -> Plan {
        let live = script(d, "app");
        Plan {
            id: "swap-test".into(),
            parent: u32::MAX - 1,
            launch: live.clone(),
            live,
            new,
            first_boot: d.join("first-boot"),
            failed: d.join("update-failed"),
            wait: Duration::from_secs(5),
            args: vec!["a file.md".into()],
            by_supervisor: false,
        }
    }

    #[test]
    fn a_new_version_that_starts_takes_the_place_and_the_old_one_goes() {
        let t = tempfile::tempdir().unwrap();
        let (d, mark) = (t.path(), t.path().join("ran"));
        let fb = d.join("first-boot");
        std::fs::create_dir(d.join("staged")).unwrap();
        version(d, "app", true, &mark, &fb);
        let new = version(&d.join("staged"), "new", true, &mark, &fb);
        std::fs::write(&fb, "2").unwrap();
        run(&plan(d, new)).unwrap();
        assert_eq!(ran(&mark, 1), "new a file.md\n", "with the app's arguments");
        assert!(!aside(&script(d, "app")).exists() && !d.join("update-failed").exists());
    }

    #[test]
    fn a_new_version_that_never_starts_is_undone() {
        let t = tempfile::tempdir().unwrap();
        let (d, mark) = (t.path(), t.path().join("ran"));
        let fb = d.join("first-boot");
        std::fs::create_dir(d.join("staged")).unwrap();
        version(d, "app", true, &mark, &fb);
        let new = version(&d.join("staged"), "new", false, &mark, &fb);
        std::fs::write(&fb, "2").unwrap();
        run(&plan(d, new)).unwrap();
        assert_eq!(
            ran(&mark, 3),
            "new a file.md\nnew a file.md\napp a file.md\n",
            "two starts, then the old one again"
        );
        assert!(d.join("update-failed").exists() && !fb.exists());
        assert!(in_place(d, "app"), "the old one is back");
    }

    #[test]
    fn supervised_the_helper_swaps_and_waits_but_starts_nothing() {
        let t = tempfile::tempdir().unwrap();
        let (d, mark) = (t.path(), t.path().join("ran"));
        let fb = d.join("first-boot");
        std::fs::create_dir(d.join("staged")).unwrap();
        version(d, "app", true, &mark, &fb);
        // The supervisor starts what is in place now, and it boots.
        let new = version(&d.join("staged"), "new", true, &mark, &fb);
        std::fs::write(&fb, "2").unwrap();
        let p = Plan { by_supervisor: true, ..plan(d, new.clone()) };
        let booted = std::thread::spawn({
            let (d, fb) = (d.to_path_buf(), fb.clone());
            move || {
                while !in_place(&d, "new") {
                    std::thread::sleep(Duration::from_millis(20));
                }
                std::fs::remove_file(fb).unwrap();
            }
        });
        run(&p).unwrap();
        booted.join().unwrap();
        assert!(!mark.exists(), "the helper started no version");
        assert!(!aside(&script(d, "app")).exists() && !d.join("update-failed").exists());
        // One that never boots: the old one is put back, for the supervisor.
        std::fs::write(&fb, "3").unwrap();
        let p = Plan { by_supervisor: true, wait: Duration::from_millis(300), ..plan(d, new) };
        version(d, "app", true, &mark, &fb);
        run(&p).unwrap();
        assert!(!mark.exists(), "nor the old one");
        assert!(d.join("update-failed").exists() && !fb.exists());
        assert!(in_place(d, "app"), "the old one is back");
    }

    #[test]
    fn a_new_version_that_exits_before_its_first_boot_is_started_once_more() {
        let t = tempfile::tempdir().unwrap();
        let (d, mark) = (t.path(), t.path().join("ran"));
        let fb = d.join("first-boot");
        std::fs::create_dir(d.join("staged")).unwrap();
        version(d, "app", true, &mark, &fb);
        // Crashes the first time, claims the second.
        let (new, tried) = (script(&d.join("staged"), "new"), d.join("tried"));
        let body = if cfg!(windows) {
            format!(
                "if exist \"{t}\" (\n{}\n{}\n) else (\ntype nul > \"{t}\"\n{}\n{}\n)\n",
                remove_cmd(&fb),
                echo_cmd("new", &mark),
                echo_cmd("crash", &mark),
                exit_cmd(1),
                t = tried.display()
            )
        } else {
            format!(
                "if [ -e '{}' ]; then {}; {}; else touch '{0}'; {}; {}; fi\n",
                tried.display(),
                remove_cmd(&fb),
                echo_cmd("new", &mark),
                echo_cmd("crash", &mark),
                exit_cmd(1)
            )
        };
        write_script(&new, &body);
        std::fs::write(&fb, "2").unwrap();
        run(&plan(d, new)).unwrap();
        assert_eq!(ran(&mark, 2), "crash\nnew\n");
        assert!(!d.join("update-failed").exists() && !aside(&script(d, "app")).exists());
    }

    #[test]
    fn a_new_version_that_asks_for_the_one_before_it_is_undone_at_once() {
        let t = tempfile::tempdir().unwrap();
        let (d, mark) = (t.path(), t.path().join("ran"));
        let fb = d.join("first-boot");
        std::fs::create_dir(d.join("staged")).unwrap();
        version(d, "app", true, &mark, &fb);
        let new = script(&d.join("staged"), "new");
        let back = if cfg!(windows) {
            format!("type nul > \"{}\"", d.join(BACK).display())
        } else {
            format!("touch '{}'", d.join(BACK).display())
        };
        write_script(&new, &format!("{}\n{back}\n{}\n", echo_cmd("new", &mark), exit_cmd(1)));
        std::fs::write(&fb, "2").unwrap();
        run(&plan(d, new)).unwrap();
        assert_eq!(ran(&mark, 2), "new\napp a file.md\n", "started once, then the old one");
        assert!(d.join("update-failed").exists() && in_place(d, "app"));
    }

    #[test]
    fn a_swap_that_cannot_be_made_runs_the_old_version_and_says_so() {
        let t = tempfile::tempdir().unwrap();
        let (d, mark) = (t.path(), t.path().join("ran"));
        let fb = d.join("first-boot");
        // The new version is missing: nothing is put in place.
        version(d, "app", true, &mark, &fb);
        std::fs::write(&fb, "2").unwrap();
        run(&plan(d, d.join("staged/gone"))).unwrap();
        assert_eq!(ran(&mark, 1), "app a file.md\n");
        assert!(d.join(SWAP_FAILED).exists() && !d.join("update-failed").exists());
        assert!(in_place(d, "app"), "the old one stays");
    }

    /// A macOS `.app` (a folder) swaps whole: `ditto` there, `cp -R -P`
    /// elsewhere, its links kept.
    #[cfg(unix)]
    #[test]
    fn a_bundle_swaps_whole_with_its_links() {
        let t = tempfile::tempdir().unwrap();
        let (d, mark) = (t.path(), t.path().join("ran"));
        let fb = d.join("first-boot");
        let bundle = |at: &Path, name: &str| {
            let exe = at.join("Counter.app/Contents/MacOS");
            std::fs::create_dir_all(&exe).unwrap();
            version(&exe, "counter", true, &mark, &fb);
            std::fs::write(exe.join(name), "").unwrap();
            std::os::unix::fs::symlink("counter", exe.join("link")).unwrap();
            at.join("Counter.app")
        };
        let (live, new) = (bundle(d, "old"), bundle(&d.join("staged"), "new"));
        std::fs::write(&fb, "2").unwrap();
        let launch = live.join("Contents/MacOS/counter");
        run(&Plan { launch, live: live.clone(), new, ..plan(d, PathBuf::new()) }).unwrap();
        assert_eq!(ran(&mark, 1), "counter a file.md\n");
        let exe = live.join("Contents/MacOS");
        assert!(exe.join("new").exists() && !exe.join("old").exists(), "the new bundle, nothing of the old");
        assert_eq!(std::fs::read_link(exe.join("link")).unwrap(), Path::new("counter"));
        assert!(!aside(&live).exists() && !d.join("update-failed").exists());
    }

    #[test]
    fn the_plan_round_trips_and_the_old_version_waits_beside() {
        let p = Plan {
            id: "notes".into(),
            parent: 7,
            live: "/Applications/Notes.app".into(),
            new: "/x/versions/2.0.0/Notes.app".into(),
            launch: "/Applications/Notes.app/Contents/MacOS/notes".into(),
            first_boot: "/x/first-boot".into(),
            failed: "/x/update-failed".into(),
            wait: Duration::from_secs(120),
            args: vec!["a file.md".into()],
            by_supervisor: true,
        };
        let back: Plan = aio::serde_json::from_str(&aio::serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back, p);
        assert_eq!(aside(&p.live), Path::new("/Applications/.Notes.app.aio-old"));
        // An older app's plan (no args) still runs.
        let mut old = aio::serde_json::to_value(&p).unwrap();
        old.as_object_mut().unwrap().remove("args");
        old.as_object_mut().unwrap().remove("by_supervisor");
        let old = aio::serde_json::from_value::<Plan>(old).unwrap();
        assert!(old.args.is_empty() && !old.by_supervisor, "it starts the versions itself");
    }
}

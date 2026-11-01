//! Where a window `ax dev` opens goes (aio: `am-display.ts`). A person at a
//! terminal gets their own desktop, as always. A caller with no terminal (an
//! agent, a script) gets a nested display (Xephyr, else Xvfb) so the window
//! never lands on, or takes focus from, the desktop the person is using.
//! `--display current` (or `AIORT_DISPLAY=current`) turns that off,
//! `isolated` always nests, `:N` names a display you run yourself. Linux
//! with no display at all nests too (else the window can't open). With
//! nothing to nest in, `auto` lets the window go where it always went, and
//! says so; `isolated` refuses to start. A contained run opens nothing in the person's browser
//! (`AIORT_NO_OPEN`); on a nested display it also gets that display's access
//! cookie (no other local account gets in), software WebGL and a private
//! D-Bus (no notification or portal dialog reaches the person's desktop).
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Where the window goes.
#[derive(Debug, PartialEq)]
pub enum Choice {
    /// The caller's display, as is.
    Current,
    /// A nested display started for this run.
    Nested,
    /// A display named by the caller.
    Named(String),
}

/// The choice for `asked` (`--display`, else `AIORT_DISPLAY`), a caller
/// that is (or isn't) at a terminal, and a desktop there is (or isn't).
pub fn choose(asked: Option<&str>, interactive: bool, desktop: bool) -> Result<Choice, String> {
    match asked.unwrap_or("auto") {
        // No screen at all (a Linux box with no display): headless, else it can't open.
        "auto" if !desktop && cfg!(all(unix, not(target_os = "macos"))) => Ok(Choice::Nested),
        "auto" if interactive || !desktop => Ok(Choice::Current),
        "auto" | "isolated" => Ok(Choice::Nested),
        "current" => Ok(Choice::Current),
        d if d.starts_with(':') && d[1..].split('.').next().is_some_and(|n| n.parse::<u32>().is_ok()) => {
            Ok(Choice::Named(d.into()))
        }
        d => Err(format!("--display {d}: auto, isolated, current or :N")),
    }
}

fn on_path(exe: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(exe).is_file()))
}

/// aio's `NESTED_GPU_ARGS`: a nested display has no GPU and Chromium
/// blocklists WebGL there (a WebGL view draws black): SwiftShader draws it.
pub const NESTED_GPU_ARGS: &str = "--use-angle=swiftshader --enable-unsafe-swiftshader";

/// One `.Xauthority` record for display number `n` (pure): every length a
/// big-endian u16; family `0xffff` (FamilyWild, any host: a hostname that
/// changes strands nothing), no address, the number, MIT-MAGIC-COOKIE-1.
pub fn xauthority(n: u32, cookie: &[u8; 16]) -> Vec<u8> {
    let n = n.to_string();
    let mut out = 0xffffu16.to_be_bytes().to_vec();
    for f in [&b""[..], n.as_bytes(), b"MIT-MAGIC-COOKIE-1", cookie] {
        out.extend((f.len() as u16).to_be_bytes());
        out.extend(f);
    }
    out
}

/// A private D-Bus session: stopped when dropped, with everything it
/// started (portals, at-spi, gvfsd: its own process group). Linux: it ends
/// with its holder too, even a killed one.
pub struct Bus {
    pub address: String,
    child: Child,
}

impl Drop for Bus {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            // SAFETY: kill(2) on the bus's own process group (it leads it).
            let group = -(self.child.id() as libc::pid_t);
            unsafe { libc::kill(group, libc::SIGTERM) };
            let until = Instant::now() + Duration::from_secs(2);
            // A helper still in the group after the bus is gone gets no second ask.
            while unsafe { libc::kill(group, 0) } == 0 && Instant::now() < until {
                _ = self.child.try_wait();
                std::thread::sleep(Duration::from_millis(20));
            }
            unsafe { libc::kill(group, libc::SIGKILL) };
        }
        _ = self.child.kill();
        _ = self.child.wait();
        // Killed, it leaves its socket (`unix:path=…,guid=…`).
        let path = self.address.split([':', ',']).find_map(|kv| kv.strip_prefix("path="));
        if let Some(p) = path {
            _ = std::fs::remove_file(p);
        }
    }
}

/// Start a private D-Bus session (none without `dbus-daemon`). What it
/// starts on demand (a portal asked for the colour scheme, at-spi) sees
/// `display` (a nested display and its cookie) or no display at all, never
/// the person's: a helper there would hold one of its X clients.
pub fn bus(display: Option<(&str, &Path)>) -> Option<Bus> {
    bus_with(display, |_| {})
}

fn bus_with(display: Option<(&str, &Path)>, more: impl FnOnce(&mut Command)) -> Option<Bus> {
    let mut cmd = Command::new("dbus-daemon");
    cmd.args(["--session", "--nofork", "--nopidfile", "--print-address=1"])
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DBUS_SESSION_BUS_ADDRESS");
    match display {
        Some((name, auth)) => cmd.env("DISPLAY", name).env("XAUTHORITY", auth),
        None => cmd.env_remove("DISPLAY").env_remove("XAUTHORITY"),
    };
    more(&mut cmd);
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    #[cfg(target_os = "linux")]
    // SAFETY: only prctl between fork and exec (async-signal-safe).
    unsafe {
        std::os::unix::process::CommandExt::pre_exec(&mut cmd, || {
            match libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) {
                0 => Ok(()),
                _ => Err(std::io::Error::last_os_error()),
            }
        });
    }
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut line = String::new();
    _ = std::io::BufReader::new(child.stdout.take()?).read_line(&mut line);
    let bus = Bus { address: line.trim().into(), child };
    (!bus.address.is_empty()).then_some(bus)
}

/// A session bus no one listens on: unset, the person's would be found
/// (`$XDG_RUNTIME_DIR/bus`).
const NO_BUS: &str = "unix:path=/nonexistent/aio-no-bus";

/// Put `cmd` on the private bus `bus` (none: on no session bus at all,
/// rather than the person's).
pub fn on_bus(cmd: &mut Command, bus: Option<&Bus>) {
    match bus {
        Some(b) => cmd.env("DBUS_SESSION_BUS_ADDRESS", &b.address),
        None => cmd.env("DBUS_SESSION_BUS_ADDRESS", NO_BUS),
    };
}

/// A contained run: nothing it opens reaches the person's browser.
pub fn contain(cmd: &mut Command) {
    cmd.env("AIORT_NO_OPEN", "1");
}

/// A display started here, with its own access cookie and private D-Bus:
/// all stopped and removed when dropped. Handed to the app (`ax start`), the
/// display ends by itself once its last window is gone (`-terminate`); held
/// for a whole run (`ax dev`, which restarts the app on it), it ends with
/// its holder (Linux: even when that is killed).
pub struct Nested {
    pub name: String,
    n: u32,
    pub auth: PathBuf,
    pub bus: Option<Bus>,
    child: Child,
}

impl Nested {
    /// Put `cmd`'s windows here, contained (`inherited`: the caller's own
    /// `AIORT_ELECTRON_ARGS`, kept after the software GPU so they still win).
    pub fn onto(&self, cmd: &mut Command, inherited: Option<&str>) {
        onto(cmd, Some(&self.name));
        contain(cmd);
        on_bus(cmd, self.bus.as_ref());
        let args = [Some(NESTED_GPU_ARGS), inherited.map(str::trim).filter(|a| !a.is_empty())];
        cmd.env("XAUTHORITY", &self.auth)
            .env("AIORT_ELECTRON_ARGS", args.into_iter().flatten().collect::<Vec<_>>().join(" "));
    }
}

impl Drop for Nested {
    fn drop(&mut self) {
        // SIGTERM first: an X server removes its socket and lock only then.
        _ = Command::new("kill").arg("-TERM").arg(self.child.id().to_string()).status();
        let until = Instant::now() + Duration::from_secs(5);
        while self.child.try_wait().ok().flatten().is_none() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(100));
        }
        _ = self.child.kill();
        _ = self.child.wait();
        _ = std::fs::remove_file(format!("/tmp/.X11-unix/X{}", self.n));
        _ = std::fs::remove_file(format!("/tmp/.X{}-lock", self.n));
        _ = std::fs::remove_file(&self.auth);
    }
}

/// A fresh cookie for display `n`, in a file only this user can read.
fn cookie_file(n: u32) -> Result<PathBuf, String> {
    let mut cookie = [0u8; 16];
    std::io::Read::read_exact(&mut std::fs::File::open("/dev/urandom").map_err(|e| e.to_string())?, &mut cookie)
        .map_err(|e| e.to_string())?;
    let file = std::env::temp_dir().join(format!("aio-display-{n}-{}.auth", std::process::id()));
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    std::io::Write::write_all(
        &mut o.open(&file).map_err(|e| format!("{}: {e}", file.display()))?,
        &xauthority(n, &cookie),
    )
    .map_err(|e| e.to_string())?;
    Ok(file)
}

/// The display servers to try, in order (pure): `visible` with a desktop
/// to show it on prefers Xephyr (a window there that holds the app's),
/// else Xvfb (seen by no one); Xephyr needs that desktop, Xvfb nothing.
pub fn servers(visible: bool, desktop: bool, xephyr: bool, xvfb: bool) -> Vec<&'static str> {
    let mut v = Vec::new();
    if visible && desktop && xephyr {
        v.push("Xephyr");
    }
    if xvfb {
        v.push("Xvfb");
    }
    v
}

/// Start a nested display: the first of [`servers`] that comes up, `held`
/// by this process (see [`Nested`]).
pub fn start(visible: bool, held: bool) -> Result<Nested, String> {
    let all = servers(visible, desktop(), on_path("Xephyr"), on_path("Xvfb"));
    let mut why = Vec::new();
    for s in &all {
        match start_one(s, held) {
            Ok(n) => return Ok(n),
            Err(e) => why.push(format!("{s}: {e}")),
        }
    }
    Err(match (all.is_empty(), visible && on_path("Xephyr")) {
        (true, true) => "Xephyr needs a desktop (DISPLAY) to show on and there is none; no Xvfb either".into(),
        (true, false) => "no Xvfb (or Xephyr) to nest in".into(),
        (false, _) => why.join("; "),
    })
}

fn start_one(server: &str, held: bool) -> Result<Nested, String> {
    let n = crate::smoke::free_display(|n| {
        Path::new(&format!("/tmp/.X11-unix/X{n}")).exists() || Path::new(&format!("/tmp/.X{n}-lock")).exists()
    })
    .ok_or("no free display number")?;
    let name = format!(":{n}");
    let mut cmd = Command::new(server);
    match server {
        "Xephyr" => cmd.args([&name, "-screen", "1280x800", "-no-host-grab"]),
        _ => cmd.args([&name, "-screen", "0", "1280x800x24"]),
    };
    if !held {
        cmd.arg("-terminate");
    }
    #[cfg(target_os = "linux")]
    if held {
        // SAFETY: only prctl between fork and exec (async-signal-safe).
        unsafe {
            use std::os::unix::process::CommandExt;
            cmd.pre_exec(|| match libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) {
                0 => Ok(()),
                _ => Err(std::io::Error::last_os_error()),
            });
        }
    }
    let auth = cookie_file(n)?;
    cmd.args(["-nolisten", "tcp", "-auth"]).arg(&auth);
    let child = match cmd.stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
        Ok(c) => c,
        Err(e) => {
            _ = std::fs::remove_file(&auth);
            return Err(e.to_string());
        }
    };
    // A handed display has no bus: it would outlive the app.
    let bus = if held { bus(Some((&name, &auth))) } else { None };
    let mut nested = Nested { name: name.clone(), n, auth, bus, child };
    let sock = format!("/tmp/.X11-unix/X{n}");
    let until = Instant::now() + Duration::from_secs(10);
    while !Path::new(&sock).exists() {
        if let Ok(Some(s)) = nested.child.try_wait() {
            return Err(format!("display {name} ended at once ({s})"));
        }
        if Instant::now() > until {
            return Err(format!("display {name} did not start"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(nested)
}

/// The nested display a run that chose [`Choice::Nested`] puts its window
/// on (`who` says it). `isolated` asked for: none is an error, never the
/// person's screen; `auto`: none says so and the window opens where it
/// always did. `held`: see [`Nested`].
pub fn nest(asked: Option<&str>, who: &str, held: bool) -> Result<Option<Nested>, String> {
    match start(true, held) {
        Ok(n) => {
            eprintln!("{who}: the app's window is on the nested display {} (--display current: yours)", n.name);
            Ok(Some(n))
        }
        Err(e) if asked == Some("isolated") => Err(format!(
            "--display isolated: no nested display ({e}); install Xvfb (ax setup --check says how), \
             or name one you run with --display :N"
        )),
        Err(e) if !desktop() => {
            eprintln!(
                "{who}: no nested display ({e}) and no desktop: the window can't open; install Xvfb (ax setup --check says how)"
            );
            Ok(None)
        }
        Err(e) => {
            eprintln!("{who}: no nested display ({e}): the window opens on yours");
            Ok(None)
        }
    }
}

/// Whether there is a person's desktop here to keep windows off.
pub fn desktop() -> bool {
    cfg!(all(unix, not(target_os = "macos")))
        && ["DISPLAY", "WAYLAND_DISPLAY"].iter().any(|v| std::env::var_os(v).is_some_and(|v| !v.is_empty()))
}

/// Put `cmd`'s windows on `display` (none: leave it as it is).
pub fn onto(cmd: &mut Command, display: Option<&str>) {
    if let Some(d) = display {
        cmd.env("DISPLAY", d).env_remove("WAYLAND_DISPLAY");
    }
}

#[cfg(test)]
mod tests {
    /// One display started at a time: two would pick the same free number.
    #[cfg(target_os = "linux")]
    static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());

    use super::*;

    #[test]
    fn only_a_caller_with_no_terminal_is_nested_by_itself() {
        // A person at a terminal keeps their desktop; an agent is contained.
        assert_eq!(choose(None, true, true), Ok(Choice::Current));
        assert_eq!(choose(None, false, true), Ok(Choice::Nested));
        // No desktop to protect: nothing to nest, but on Linux nothing to show on either.
        let none = || if cfg!(all(unix, not(target_os = "macos"))) { Choice::Nested } else { Choice::Current };
        assert_eq!(choose(None, false, false), Ok(none()));
        assert_eq!(choose(None, true, false), Ok(none()), "a terminal with no display (ssh) too");
        assert_eq!(choose(Some("isolated"), true, true), Ok(Choice::Nested));
        assert_eq!(choose(Some("current"), false, true), Ok(Choice::Current));
        assert_eq!(choose(Some(":7"), false, true), Ok(Choice::Named(":7".into())));
        assert_eq!(choose(Some(":7.0"), true, true), Ok(Choice::Named(":7.0".into())));
        assert!(choose(Some("elsewhere"), true, true).unwrap_err().contains("auto, isolated, current or :N"));
        assert!(choose(Some(":x"), true, true).is_err());
    }

    #[test]
    fn xephyr_only_with_a_desktop_to_show_it_on_else_xvfb() {
        assert_eq!(servers(true, true, true, true), ["Xephyr", "Xvfb"], "Xvfb when Xephyr can't start");
        assert_eq!(servers(true, false, true, true), ["Xvfb"], "no DISPLAY: Xephyr has nothing to show on");
        assert!(servers(true, false, true, false).is_empty());
        assert_eq!(servers(false, true, true, true), ["Xvfb"], "seen by no one");
    }

    /// Headless only (Xvfb, never a window on a screen); skipped without it.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_nested_display_starts_and_goes_away_with_its_holder() {
        if !on_path("Xvfb") {
            return;
        }
        let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
        let d = start(false, false).unwrap();
        let sock = format!("/tmp/.X11-unix/X{}", &d.name[1..]);
        assert!(Path::new(&sock).exists());
        drop(d);
        assert!(!Path::new(&sock).exists(), "nothing left behind");
    }

    /// `ax dev` restarts the app on its display: a held one outlives its
    /// last window, a handed one doesn't. Headless only; skipped without Xvfb.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_held_display_outlives_its_last_window() {
        if !on_path("Xvfb") || !on_path("xdpyinfo") {
            return;
        }
        for held in [true, false] {
            let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
            let mut d = start(false, held).unwrap();
            let window = Command::new("xdpyinfo").env("DISPLAY", &d.name).env("XAUTHORITY", &d.auth).output();
            assert!(window.unwrap().status.success());
            std::thread::sleep(Duration::from_millis(500));
            assert_eq!(d.child.try_wait().unwrap().is_none(), held, "held {held}");
        }
    }

    #[test]
    fn a_cookie_record_is_what_libxau_reads() {
        let c = [7u8; 16];
        let r = xauthority(91, &c);
        let mut want = vec![0xff, 0xff, 0, 0, 0, 2, b'9', b'1', 0, 18];
        want.extend(b"MIT-MAGIC-COOKIE-1");
        want.extend([0, 16]);
        want.extend(c);
        assert_eq!(r, want);
    }

    /// Headless only (Xvfb), and its cookie works: `xdpyinfo` with it gets
    /// in, without it is refused. Skipped without the tools.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_nested_display_lets_in_only_its_cookie_and_has_its_own_bus() {
        if !on_path("Xvfb") || !on_path("xdpyinfo") {
            return;
        }
        let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
        let d = start(false, false).unwrap();
        let reach = |auth: Option<&Path>| {
            let mut c = Command::new("xdpyinfo");
            c.env("DISPLAY", &d.name).env_remove("XAUTHORITY").stdout(Stdio::null()).stderr(Stdio::null());
            if let Some(a) = auth {
                c.env("XAUTHORITY", a);
            }
            c.status().is_ok_and(|s| s.success())
        };
        assert!(reach(Some(&d.auth)), "the cookie gets in");
        assert!(!reach(Some(Path::new("/nonexistent"))), "nothing else does");
        let mut c = Command::new("true");
        d.onto(&mut c, Some(" --lang=de "));
        let env: std::collections::BTreeMap<_, _> = c
            .get_envs()
            .map(|(k, v)| (k.to_string_lossy().into_owned(), v.map(|v| v.to_string_lossy().into_owned())))
            .collect();
        assert_eq!(
            env["AIORT_ELECTRON_ARGS"].as_deref(),
            Some(&*format!("{NESTED_GPU_ARGS} --lang=de")),
            "the caller's win"
        );
        assert_eq!(env["AIORT_NO_OPEN"].as_deref(), Some("1"));
        if let Some(b) = &d.bus {
            assert_eq!(env["DBUS_SESSION_BUS_ADDRESS"].as_deref(), Some(b.address.as_str()));
            assert_ne!(
                std::env::var("DBUS_SESSION_BUS_ADDRESS").ok().as_deref(),
                Some(b.address.as_str()),
                "not the person's"
            );
        }
        let auth = d.auth.clone();
        drop(d);
        assert!(!auth.exists(), "the cookie goes with it");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_private_bus_answers_and_goes_away_with_its_holder() {
        if !on_path("dbus-daemon") || !on_path("dbus-send") {
            return;
        }
        let b = bus(None).unwrap();
        assert_ne!(std::env::var("DBUS_SESSION_BUS_ADDRESS").ok(), Some(b.address.clone()), "not the person's");
        let names = Command::new("dbus-send")
            .arg(format!("--bus={}", b.address))
            .args(["--print-reply", "--dest=org.freedesktop.DBus", "/", "org.freedesktop.DBus.ListNames"])
            .output()
            .unwrap();
        assert!(names.status.success(), "{}", String::from_utf8_lossy(&names.stderr));
        let pid = b.child.id();
        let sock = b.address.split([':', ',']).find_map(|kv| kv.strip_prefix("path=")).map(PathBuf::from);
        drop(b);
        assert!(!Path::new(&format!("/proc/{pid}")).exists(), "nothing left behind");
        assert!(sock.is_none_or(|s| !s.exists()), "its socket neither");
    }

    /// What the bus starts on demand goes with it, and sees no person's
    /// display: a service of its own (`sleep`), activated, then the bus dropped.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_private_bus_takes_what_it_started_along_and_never_shows_it_the_persons_display() {
        if !on_path("dbus-daemon") || !on_path("dbus-send") {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let services = d.path().join("dbus-1/services");
        std::fs::create_dir_all(&services).unwrap();
        let svc = "[D-BUS Service]\nName=org.aio.Sleeper\nExec=/bin/sleep 600\n";
        std::fs::write(services.join("org.aio.Sleeper.service"), svc).unwrap();
        let b = bus_with(Some((":95", Path::new("/nonexistent/auth"))), |c| {
            c.env("XDG_DATA_DIRS", d.path());
        })
        .unwrap();
        // Activation times out (`sleep` never takes the name), but it runs.
        _ = Command::new("dbus-send")
            .arg(format!("--bus={}", b.address))
            .args(["--reply-timeout=300", "--print-reply", "--dest=org.freedesktop.DBus", "/"])
            .args(["org.freedesktop.DBus.StartServiceByName", "string:org.aio.Sleeper", "uint32:0"])
            .output();
        let group = b.child.id().to_string();
        let started: Vec<String> = std::fs::read_dir("/proc")
            .unwrap()
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|p| p.bytes().all(|c| c.is_ascii_digit()) && *p != group)
            .filter(|p| {
                let stat = std::fs::read_to_string(format!("/proc/{p}/stat")).unwrap_or_default();
                stat.rsplit_once(") ").and_then(|(_, r)| r.split(' ').nth(2)) == Some(group.as_str())
            })
            .collect();
        let comm = |p: &String| std::fs::read_to_string(format!("/proc/{p}/comm")).unwrap_or_default();
        let sleeper = started.iter().find(|p| comm(p).trim() == "sleep").expect("the service it started, in its group");
        let env = std::fs::read(format!("/proc/{sleeper}/environ")).unwrap();
        let env: Vec<&[u8]> = env.split(|c| *c == 0).collect();
        assert!(env.contains(&&b"DISPLAY=:95"[..]), "the nested display, never the person's");
        assert!(!env.iter().any(|v| v.starts_with(b"WAYLAND_DISPLAY=")));
        drop(b);
        for p in &started {
            let st = std::fs::read_to_string(format!("/proc/{p}/stat")).unwrap_or_default();
            assert!(st.is_empty() || st.contains(") Z "), "{p} outlived its bus");
        }
    }

    #[test]
    fn a_contained_run_is_on_a_private_bus_or_none() {
        let mut c = Command::new("true");
        on_bus(&mut c, None);
        contain(&mut c);
        let env: Vec<_> = c.get_envs().map(|(k, v)| (k.to_owned(), v.map(|v| v.to_owned()))).collect();
        assert!(env.contains(&("DBUS_SESSION_BUS_ADDRESS".into(), Some(NO_BUS.into()))), "never the person's");
        assert!(env.contains(&("AIORT_NO_OPEN".into(), Some("1".into()))));
    }

    #[test]
    fn a_window_goes_onto_the_display_chosen() {
        let mut c = Command::new("true");
        onto(&mut c, Some(":91"));
        let envs: Vec<_> = c.get_envs().map(|(k, v)| (k.to_owned(), v.map(|v| v.to_owned()))).collect();
        assert!(envs.contains(&("DISPLAY".into(), Some(":91".into()))));
        assert!(envs.contains(&("WAYLAND_DISPLAY".into(), None)), "no way around it through Wayland");
        let mut c = Command::new("true");
        onto(&mut c, None);
        assert_eq!(c.get_envs().count(), 0);
    }
}

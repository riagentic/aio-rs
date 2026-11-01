//! `ax install <app> --android`: the built APK onto a real, attached phone,
//! then started (aio's `install:android`). The APK is the one for the phone's
//! CPU; an emulator only when asked (`--emulator`); never an APK older than
//! the app's sources (that would put the previous build on the phone under
//! the same version and say done): `--build` builds first, `--apk <file>`
//! installs a named one regardless. The device: `--device`, else adb's own
//! `ANDROID_SERIAL`, else the one attached. `ax dev --android` and `ax test
//! --android` boot an emulator when none is attached ([`ensure_device`]).
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

/// One attached device, as `adb devices -l` says.
#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub serial: String,
    pub state: String,
    /// `emulator-5554` is an emulator; anything else is hardware.
    pub emulator: bool,
    pub model: String,
}

impl Device {
    /// Its model, else its serial.
    pub fn label(&self) -> String {
        if self.model.is_empty() { self.serial.clone() } else { self.model.clone() }
    }
}

/// `adb devices -l`, read.
pub fn parse_devices(out: &str) -> Vec<Device> {
    out.lines()
        .skip(1)
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            let (serial, state) = (w.next()?, w.next()?);
            let model = w.find_map(|f| f.strip_prefix("model:")).unwrap_or_default().replace('_', " ");
            Some(Device {
                serial: serial.into(),
                state: state.into(),
                emulator: serial.starts_with("emulator-"),
                model,
            })
        })
        .collect()
}

fn state_hint(d: &Device) -> String {
    match d.state.as_str() {
        "unauthorized" => {
            format!("unlock {} and tap \"Allow USB debugging\": the phone is waiting for you, not broken", d.serial)
        }
        "offline" => format!("{} is offline: unplug and plug it in again, or adb kill-server", d.serial),
        s => format!("{} is in state \"{s}\"", d.serial),
    }
}

/// Which devices a pick may take: phones only (`ax install`), any
/// (`--emulator`), or any with a ready emulator first (the dev loop, aio's
/// `devTarget`: a phone plugged in beside it is not an error).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    Phone,
    Any,
    EmulatorFirst,
}

/// The device to install on (`serial`: the one named), or why there is none
/// and what to do: each refusal says what is attached.
pub fn pick_device(devices: &[Device], serial: Option<&str>, want: Want) -> Result<Device, String> {
    if let Some(s) = serial {
        let d = devices.iter().find(|d| d.serial == s).ok_or_else(|| {
            let all: Vec<&str> = devices.iter().map(|d| d.serial.as_str()).collect();
            let attached = if all.is_empty() { "nothing is attached".into() } else { all.join(", ") };
            format!("no device {s} is attached (attached: {attached})")
        })?;
        return if d.state == "device" {
            Ok(d.clone())
        } else {
            Err(format!("device {} is \"{}\": {}", d.serial, d.state, state_hint(d)))
        };
    }
    let emulator = want != Want::Phone;
    let usable: Vec<&Device> = devices.iter().filter(|d| emulator || !d.emulator).collect();
    let mut ready: Vec<&&Device> = usable.iter().filter(|d| d.state == "device").collect();
    if want == Want::EmulatorFirst && ready.iter().any(|d| d.emulator) {
        ready.retain(|d| d.emulator);
    }
    match ready[..] {
        [d] => return Ok((*d).clone()),
        [_, _, ..] => {
            let flags: Vec<String> = ready
                .iter()
                .map(|d| match d.model.as_str() {
                    "" => format!("--device {}", d.serial),
                    m => format!("--device {} ({m})", d.serial),
                })
                .collect();
            return Err(format!(
                "{} devices are attached: name the one you mean (or ANDROID_SERIAL=<serial>)\n  {}",
                ready.len(),
                flags.join("\n  ")
            ));
        }
        [] => {}
    }
    if let Some(d) = usable.first() {
        return Err(format!("the only attached device is \"{}\": {}", d.state, state_hint(d)));
    }
    if !emulator && devices.iter().any(|d| d.emulator) {
        return Err("only an emulator is attached, and this installs to a phone (--emulator: to it)".into());
    }
    Err(if emulator {
        "no Android device is attached: start an emulator first (emulator -avd <name>)".into()
    } else {
        "no Android device is attached: turn on Developer options → USB debugging, plug the phone in, and check \
         that adb devices lists it"
            .into()
    })
}

/// The first of the device's CPU types (`ro.product.cpu.abilist`) an APK is
/// built for.
pub fn abi_of(abilist: &str) -> Option<&'static str> {
    abilist.trim().split(',').find_map(|a| crate::android::ABIS.iter().map(|(b, _)| *b).find(|b| *b == a.trim()))
}

/// The version in `name` when it is app `id`'s APK for `split` (an ABI, or
/// `universal`) exactly: `<id>-<major.minor.patch[-pre]>-android-<split>.apk`
/// (`notes` is not `notes-pro`), pure.
pub fn apk_version<'a>(name: &'a str, id: &str, split: &str) -> Option<&'a str> {
    let v = name.strip_prefix(id)?.strip_prefix('-')?.strip_suffix(&format!("-android-{split}.apk"))?;
    let core = v.split(['-', '+']).next()?;
    let parts: Vec<&str> = core.split('.').collect();
    (parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))).then_some(v)
}

/// The newest APK of app `id` for `abi` among `files` (name, modified),
/// else its newest universal one.
pub fn pick_apk<'a>(files: &'a [(String, SystemTime)], id: &str, abi: &str) -> Result<&'a str, String> {
    let newest = |split: &str| {
        files
            .iter()
            .filter(|(n, _)| apk_version(n, id, split).is_some())
            .max_by_key(|(_, t)| *t)
            .map(|(n, _)| n.as_str())
    };
    newest(abi)
        .or_else(|| newest("universal"))
        .ok_or_else(|| format!("no {id} APK for {abi} in dist/: ax build {id} --android (or add --build)"))
}

/// The newest file the build reads under the app's `dirs` (sources,
/// assets, manifests), never its output.
pub fn newest_source(dirs: &[PathBuf]) -> Option<(PathBuf, SystemTime)> {
    fn walk(p: &Path, best: &mut Option<(PathBuf, SystemTime)>) {
        let Ok(meta) = std::fs::metadata(p) else { return };
        if meta.is_dir() {
            for e in std::fs::read_dir(p).into_iter().flatten().flatten() {
                if !e.file_name().to_string_lossy().starts_with('.') {
                    walk(&e.path(), best);
                }
            }
        } else if let Ok(t) = meta.modified()
            && best.as_ref().is_none_or(|(_, b)| t > *b)
        {
            *best = Some((p.to_path_buf(), t));
        }
    }
    let mut best = None;
    for d in dirs {
        for sub in ["src", "assets", "android", "build.rs", "Cargo.toml", "aio.toml"] {
            walk(&d.join(sub), &mut best);
        }
    }
    best
}

/// Why installing `apk` (modified `built`) would put old code on the phone:
/// a source changed after it was built.
pub fn stale(apk: &str, built: SystemTime, newest: Option<&(PathBuf, SystemTime)>) -> Option<String> {
    let (path, t) = newest?;
    let ago = t.duration_since(built).ok()?.as_secs();
    if ago == 0 {
        return None; // the same second: the build wrote it after reading
    }
    let age = match ago {
        0..90 => format!("{ago}s"),
        90..5400 => format!("{}min", (ago as f64 / 60.0).round()),
        _ => format!("{}h", (ago as f64 / 3600.0).round()),
    };
    Some(format!(
        "{apk} is older than your sources: {} changed {age} after it was built.\n  \
         Installing it would put the previous build on the phone, under the same version, and report success.\n  \
         fix: add --build (builds, then installs). To install this exact APK anyway: --apk {apk}",
        path.display()
    ))
}

/// A tool in the SDK (`platform-tools/adb`, `emulator/emulator`), else on
/// the PATH.
fn sdk_tool(dir: &str, name: &str) -> PathBuf {
    let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    crate::android::sdk_dir().map(|s| s.join(dir).join(&exe)).filter(|p| p.exists()).unwrap_or_else(|| exe.into())
}

fn adb() -> PathBuf {
    sdk_tool("platform-tools", "adb")
}

pub fn adb_out(args: &[&str]) -> Result<String, String> {
    let out =
        Command::new(adb()).args(args).output().map_err(|e| format!("adb: {e} (install the SDK's platform-tools)"))?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    if out.status.success() { Ok(text) } else { Err(format!("adb {}: {}", args.join(" "), text.trim())) }
}

/// adb's own `ANDROID_SERIAL`, when set.
fn serial_env() -> Option<String> {
    std::env::var("ANDROID_SERIAL").ok().filter(|s| !s.is_empty())
}

/// The emulator (AVD) to boot (aio's): `AIO_AVD` when it is one of `avds`,
/// else the first; why none (pure).
pub fn pick_avd(avds: &str, asked: Option<&str>) -> Result<String, String> {
    let all: Vec<&str> = avds.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("INFO")).collect();
    match asked {
        Some(a) if all.contains(&a) => Ok(a.into()),
        Some(a) => Err(format!(
            "AVD \"{a}\" (AIO_AVD) not found: available: {}",
            if all.is_empty() { "none".into() } else { all.join(", ") }
        )),
        None => all.first().map(|a| a.to_string()).ok_or_else(|| {
            "no Android emulator (AVD) found: create one (Android Studio → Device Manager, or avdmanager create avd), \
             or attach a phone"
                .into()
        }),
    }
}

/// The emulator's command line for `avd` (aio's): no boot animation, no
/// snapshot saved (a dev emulator boots fresh), on console `port`
/// (`AIO_AVD_PORT`) when given, without a window where there is no display.
pub fn emulator_args(avd: &str, port: Option<&str>, headless: bool) -> Vec<String> {
    let mut a: Vec<String> = ["-avd", avd, "-no-boot-anim", "-no-snapshot-save"].map(String::from).into();
    if let Some(p) = port {
        a.extend(["-port".into(), p.into()]);
    }
    if headless {
        a.push("-no-window".into());
    }
    a
}

/// The device the dev loop or the emulator lane runs on (`serial`, else
/// `ANDROID_SERIAL`, else a ready emulator before a phone): when none is
/// attached, `AIO_AVD` (or the first AVD) is booted and waited for, up to
/// 180 s (aio's `dev:android`); a booted emulator is left running for the
/// next run.
pub fn ensure_device(serial: Option<&str>) -> Result<Device, String> {
    let serial = serial.map(String::from).or_else(serial_env);
    let attached = || -> Result<Vec<Device>, String> { Ok(parse_devices(&adb_out(&["devices", "-l"])?)) };
    let booted = |d: &Device| {
        adb_out(&["-s", &d.serial, "shell", "getprop", "sys.boot_completed"]).is_ok_and(|o| o.trim() == "1")
    };
    _ = adb_out(&["start-server"]);
    if let Ok(d) = pick_device(&attached()?, serial.as_deref(), Want::EmulatorFirst) {
        eprintln!(
            "ax: on the attached {} {} ({})",
            if d.emulator { "emulator" } else { "device" },
            d.label(),
            d.serial
        );
        return wait_booted(d, booted, None);
    }
    // A phone named and not there: nothing to boot. An emulator named
    // (`emulator-<port>`) is booted on that port, as aio boots on a miss.
    let named_port = serial.as_deref().and_then(|s| s.strip_prefix("emulator-")).map(String::from);
    if serial.is_some() && named_port.is_none() {
        return pick_device(&attached()?, serial.as_deref(), Want::Any);
    }
    let emulator = sdk_tool("emulator", "emulator");
    let list =
        Command::new(&emulator).arg("-list-avds").output().map_err(|e| format!("{}: {e}", emulator.display()))?;
    let avd = pick_avd(&String::from_utf8_lossy(&list.stdout), std::env::var("AIO_AVD").ok().as_deref())?;
    let port = named_port.or_else(|| std::env::var("AIO_AVD_PORT").ok().filter(|p| !p.is_empty()));
    let headless = cfg!(target_os = "linux")
        && std::env::var_os("DISPLAY").is_none()
        && std::env::var_os("WAYLAND_DISPLAY").is_none();
    eprintln!("ax: booting the emulator \"{avd}\" (the first boot can take a minute)…");
    let log = std::env::temp_dir().join(format!("ax-emulator-{avd}.log"));
    let out = std::fs::File::create(&log).map_err(|e| format!("{}: {e}", log.display()))?;
    let err = out.try_clone().map_err(|e| e.to_string())?;
    let child = Command::new(&emulator)
        .args(emulator_args(&avd, port.as_deref(), headless))
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err)
        .spawn()
        .map_err(|e| format!("{}: {e}", emulator.display()))?;
    let deadline = Instant::now() + Duration::from_secs(180);
    let mut child = child;
    loop {
        if let Ok(d) =
            pick_device(&attached()?, port.as_ref().map(|p| format!("emulator-{p}")).as_deref(), Want::EmulatorFirst)
            && d.emulator
        {
            return wait_booted(d, booted, Some((&mut child, &log, deadline)));
        }
        if let Some(st) = child.try_wait().map_err(|e| e.to_string())? {
            return Err(format!("the emulator \"{avd}\" ended ({st}) while booting: {}", tail(&log)));
        }
        if Instant::now() > deadline {
            return Err(format!("the emulator \"{avd}\" did not come up in 180 s: {}", tail(&log)));
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

/// The last lines of `log`, to say why an emulator didn't boot.
fn tail(log: &Path) -> String {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let lines: Vec<&str> = text.lines().rev().take(15).collect();
    lines.into_iter().rev().collect::<Vec<_>>().join("\n  ")
}

/// `d` once Android on it has booted (`sys.boot_completed`).
fn wait_booted(
    d: Device,
    booted: impl Fn(&Device) -> bool,
    mut booting: Option<(&mut Child, &Path, Instant)>,
) -> Result<Device, String> {
    let deadline = booting.as_ref().map_or(Instant::now() + Duration::from_secs(180), |(_, _, t)| *t);
    while !booted(&d) {
        if let Some((child, log, _)) = booting.as_mut()
            && let Some(st) = child.try_wait().map_err(|e| e.to_string())?
        {
            return Err(format!("the emulator ended ({st}) while booting: {}", tail(log)));
        }
        if Instant::now() > deadline {
            return Err(format!("{} did not finish booting in 180 s", d.serial));
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    Ok(d)
}

/// The app's lines on `serial` from now on (its log, its page's console,
/// a crash), into the terminal as `[device] …` until the child is stopped
/// (aio's dev:android console).
pub fn logcat(serial: &str) -> Result<Child, String> {
    let mut child = Command::new(adb())
        .args(["-s", serial, "logcat", "-v", "brief", "-T", "1", "-s", "aio:V", "AndroidRuntime:E", "chromium:W"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("adb logcat: {e}"))?;
    let out = child.stdout.take().ok_or("adb logcat: no output")?;
    std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
            if !line.starts_with("--------- beginning") {
                eprintln!("[device] {line}");
            }
        }
    });
    Ok(child)
}

/// What `ax install` was asked.
pub struct Opts<'a> {
    pub build: bool,
    pub emulator: bool,
    pub apk: Option<&'a str>,
    pub device: Option<&'a str>,
    pub launch: bool,
}

/// Install `apk` on `dev`, saying what refused it and how to get past it.
pub fn install_on(dev: &Device, apk: &Path) -> Result<(), String> {
    let out = adb_out(&["-s", &dev.serial, "install", "-r", &apk.to_string_lossy()])?;
    if !out.contains("Failure") {
        return Ok(());
    }
    let why = if out.contains("INSTALL_FAILED_UPDATE_INCOMPATIBLE") {
        "a build signed with another key is installed: uninstall it on the phone first (its data with it)"
    } else if out.contains("INSTALL_FAILED_VERSION_DOWNGRADE") {
        "a newer version is installed: raise [app] version, or uninstall it first"
    } else {
        "see adb's answer"
    };
    Err(format!("adb install failed on {}: {}\n  {why}", dev.label(), out.trim()))
}

/// Start `component` on `dev` (`extra`: `am start` flags after it); what
/// went wrong, if it did not start (`am start` exits 0 on a missing class).
pub fn start_on(dev: &Device, component: &str, extra: &[&str]) -> Result<(), String> {
    let mut args = vec!["-s", &dev.serial, "shell", "am", "start", "-S", "-n", component];
    args.extend(extra);
    match adb_out(&args) {
        Ok(out) if !out.contains("Error") => Ok(()),
        Ok(out) | Err(out) => Err(out.trim().lines().last().unwrap_or_default().to_string()),
    }
}

/// `ax install <app> --android`.
pub fn android(name: &str, o: &Opts) -> Result<String, String> {
    let meta = crate::dev::metadata()?;
    let pkg = crate::dev::locate(&meta, name)?;
    let (app, cfg) = crate::build::app_of(&pkg)?;
    if o.build {
        eprintln!("{}", crate::build::run(name, Some("android"), None, false)?);
    }
    let want = if o.emulator { Want::Any } else { Want::Phone };
    let serial = o.device.map(String::from).or_else(serial_env);
    let dev = pick_device(&parse_devices(&adb_out(&["devices", "-l"])?), serial.as_deref(), want)?;
    let label = dev.label();
    let apk = match o.apk {
        Some(a) => PathBuf::from(a),
        None => {
            let list = adb_out(&["-s", &dev.serial, "shell", "getprop", "ro.product.cpu.abilist"])?;
            let abi = abi_of(&list).ok_or_else(|| format!("{label} runs {}: ax builds no APK for it", list.trim()))?;
            let dist =
                Path::new(meta["workspace_root"].as_str().ok_or("cargo metadata: no workspace_root")?).join("dist");
            let files: Vec<(String, SystemTime)> = std::fs::read_dir(&dist)
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|e| {
                    Some((e.file_name().to_string_lossy().into_owned(), e.metadata().ok()?.modified().ok()?))
                })
                .collect();
            let file = pick_apk(&files, &app.id, abi)?;
            let built = files.iter().find(|(n, _)| n == file).map(|(_, t)| *t).unwrap_or(SystemTime::UNIX_EPOCH);
            if !o.build
                && let Some(why) = stale(file, built, newest_source(&pkg.dirs).as_ref())
            {
                return Err(why);
            }
            dist.join(file)
        }
    };
    eprintln!("ax install: {} → {label} ({})", apk.display(), dev.serial);
    install_on(&dev, &apk)?;
    if !o.launch {
        return Ok(format!("{} is installed on {label}", app.name));
    }
    let id = crate::android::application_id_of(&app.id, cfg.app.port_of.as_deref(), &cfg.build.android);
    // A debug APK of the dev copy is `<id>.dev`.
    let dev_copy = apk.file_name().is_some_and(|n| n.to_string_lossy().ends_with("-dev-debug.apk"));
    let component = format!("{id}{}/aio.rt.MainActivity", if dev_copy { ".dev" } else { "" });
    match start_on(&dev, &component, &[]) {
        Ok(()) => Ok(format!("{} is installed and started on {label}", app.name)),
        Err(why) => {
            Ok(format!("{} is installed on {label}, but did not start ({why}): open it from the launcher", app.name))
        }
    }
}

/// What the app's log on the device says of a start (aio's emulator e2e
/// lane): ready (`[aio] window ready`), crashed (the line why), or nothing
/// yet (pure).
pub fn start_said(log: &str) -> Option<Result<(), String>> {
    let crash = [
        "FATAL EXCEPTION",
        "the app did not start",
        "UnsatisfiedLinkError",
        "NoSuchMethodError",
        "NoClassDefFoundError",
        "too old for the app's page",
    ];
    if let Some(l) = log.lines().find(|l| crash.iter().any(|c| l.contains(c))) {
        let at = log.find(l).unwrap_or(0);
        let why: Vec<&str> = log[at..].lines().take(8).collect();
        return Some(Err(why.join("\n  ")));
    }
    log.contains("[aio] window ready").then_some(Ok(()))
}

/// `ax test <app> --android` (aio's `AIO_AVD` emulator lane): the app's
/// APK for the device's CPU built, installed and started on the emulator
/// (`AIO_AVD` booted when none is attached; a phone named by `--device` or
/// `ANDROID_SERIAL`), passed when its window is ready within 60 s and it is
/// still running 3 s later, failed with the device's own lines on a crash.
pub fn lane(name: &str, device: Option<&str>) -> Result<String, String> {
    let meta = crate::dev::metadata()?;
    let (app, cfg) = crate::build::app_of(&crate::dev::locate(&meta, name)?)?;
    let dev = ensure_device(device)?;
    let sh = |args: &[&str]| adb_out(&[&["-s", dev.serial.as_str(), "shell"], args].concat());
    let abi = abi_of(&sh(&["getprop", "ro.product.cpu.abilist"])?)
        .and_then(|a| crate::android::ABIS.iter().find(|(b, _)| *b == a))
        .ok_or_else(|| format!("{} runs a CPU ax builds no APK for", dev.label()))?;
    let api = sh(&["getprop", "ro.build.version.sdk"])?.trim().to_string();
    let ask = crate::android::Ask { abis: std::slice::from_ref(abi), ..crate::android::Ask::RELEASE };
    let (_, apks) = crate::build::android(name, ask)?;
    let apk = apks.first().ok_or("the build made no APK")?;
    install_on(&dev, apk)?;
    let id = crate::android::application_id_of(&app.id, cfg.app.port_of.as_deref(), &cfg.build.android);
    adb_out(&["-s", &dev.serial, "logcat", "-c"])?;
    let started = Instant::now();
    start_on(&dev, &format!("{id}/aio.rt.MainActivity"), &[]).map_err(|why| format!("{id} did not start: {why}"))?;
    let log = || adb_out(&["-s", &dev.serial, "logcat", "-d", "-v", "brief", "-s", "aio:V", "AndroidRuntime:E"]);
    loop {
        match start_said(&log()?) {
            Some(Ok(())) => break,
            Some(Err(why)) => return Err(format!("{} failed on {} (API {api}):\n  {why}", app.name, dev.label())),
            None if started.elapsed() > Duration::from_secs(60) => {
                return Err(format!(
                    "{}: no window in 60 s on {} (API {api}); its log:\n{}",
                    app.name,
                    dev.label(),
                    log()?
                ));
            }
            None => std::thread::sleep(Duration::from_millis(500)),
        }
    }
    let ready = started.elapsed().as_millis();
    std::thread::sleep(Duration::from_secs(3));
    if let Some(Err(why)) = start_said(&log()?) {
        return Err(format!("{} crashed after its start on {} (API {api}):\n  {why}", app.name, dev.label()));
    }
    if sh(&["pidof", &id]).map_or(true, |p| p.trim().is_empty()) {
        return Err(format!("{} is not running 3 s after its start on {} (API {api})", app.name, dev.label()));
    }
    _ = sh(&["am", "force-stop", &id]);
    Ok(format!(
        "{} on {} ({}, API {api}): window ready in {ready} ms, still running 3 s later",
        app.name,
        dev.label(),
        dev.serial
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const LIST: &str = "List of devices attached
R5CT12ABCDE            device usb:1-3 product:a52qxx model:SM_A525F device:a52q
emulator-5554          device product:sdk_gphone64_x86_64 model:sdk_gphone64_x86_64
";

    fn devices(s: &str) -> Vec<Device> {
        parse_devices(&format!("List of devices attached\n{s}"))
    }

    #[test]
    fn devices_are_read_telling_a_phone_from_an_emulator_and_a_phone_wins() {
        let d = parse_devices(LIST);
        assert_eq!(d.len(), 2);
        assert_eq!((d[0].serial.as_str(), d[0].emulator, d[0].model.as_str()), ("R5CT12ABCDE", false, "SM A525F"));
        assert!(d[1].emulator);
        assert_eq!(pick_device(&d, None, Want::Phone).unwrap().serial, "R5CT12ABCDE");
    }

    #[test]
    fn an_emulator_alone_is_refused_unless_asked() {
        let only = devices("emulator-5554 device model:sdk_gphone64\n");
        assert!(pick_device(&only, None, Want::Phone).unwrap_err().contains("--emulator"));
        assert_eq!(pick_device(&only, None, Want::Any).unwrap().serial, "emulator-5554");
        assert!(pick_device(&[], None, Want::Any).unwrap_err().contains("emulator -avd"));
    }

    #[test]
    fn an_unauthorized_phone_is_the_dialog_and_nothing_says_what_to_do() {
        let e = pick_device(&devices("R5CT12ABCDE unauthorized usb:1-3\n"), None, Want::Phone).unwrap_err();
        assert!(e.contains("unauthorized") && e.contains("Allow USB debugging"), "{e}");
        let e = pick_device(&devices("R5CT12ABCDE offline\n"), Some("R5CT12ABCDE"), Want::Phone).unwrap_err();
        assert!(e.contains("adb kill-server"), "{e}");
        assert!(pick_device(&[], None, Want::Phone).unwrap_err().contains("USB debugging"));
    }

    #[test]
    fn two_phones_are_refused_with_the_exact_flags_and_naming_one_resolves_it() {
        let two = devices("R5CT12ABCDE device model:SM_A525F\n9A281FFAZ004TZ device model:Pixel_7\n");
        let e = pick_device(&two, None, Want::Phone).unwrap_err();
        assert!(
            e.contains("--device R5CT12ABCDE (SM A525F)") && e.contains("--device 9A281FFAZ004TZ (Pixel 7)"),
            "{e}"
        );
        assert_eq!(pick_device(&two, Some("9A281FFAZ004TZ"), Want::Phone).unwrap().model, "Pixel 7");
        assert!(
            pick_device(&two, Some("nope"), Want::Phone).unwrap_err().contains("attached: R5CT12ABCDE, 9A281FFAZ004TZ")
        );
    }

    #[test]
    fn the_newest_apk_for_the_phones_cpu_is_the_one_meant() {
        assert_eq!(abi_of("arm64-v8a,armeabi-v7a,armeabi\n"), Some("arm64-v8a"));
        assert_eq!(abi_of("x86_64,x86"), Some("x86_64"));
        assert_eq!(abi_of("armeabi-v7a"), Some("armeabi-v7a"), "32-bit ARM too");
        assert_eq!(abi_of("mips"), None);
        let t = |s: u64| SystemTime::UNIX_EPOCH + Duration::from_secs(s);
        let files = [
            ("notes-1.0.0-android-arm64-v8a.apk".to_string(), t(1000)),
            ("notes-1.1.0-android-arm64-v8a.apk".to_string(), t(9000)),
            ("notes-1.1.0-android-x86_64.apk".to_string(), t(9500)),
            ("notes-pro-2.0.0-android-arm64-v8a.apk".to_string(), t(9900)),
            ("notes-1.1.0-linux-x64.tar.gz".to_string(), t(9999)),
        ];
        // `notes` is not `notes-pro`, however new that one is.
        assert_eq!(pick_apk(&files, "notes", "arm64-v8a"), Ok("notes-1.1.0-android-arm64-v8a.apk"));
        assert_eq!(pick_apk(&files, "notes-pro", "arm64-v8a"), Ok("notes-pro-2.0.0-android-arm64-v8a.apk"));
        assert!(pick_apk(&files[4..], "notes", "x86_64").unwrap_err().contains("ax build notes --android"));
    }

    #[test]
    fn an_apk_older_than_the_sources_is_refused_saying_what_and_how() {
        let t = |s: u64| SystemTime::UNIX_EPOCH + Duration::from_secs(s);
        let src = (PathBuf::from("src/cells.rs"), t(1120));
        let why = stale("notes.apk", t(1000), Some(&src)).unwrap();
        assert!(why.contains("notes.apk") && why.contains("src/cells.rs") && why.contains("2min"), "{why}");
        assert!(why.contains("previous build") && why.contains("--build") && why.contains("--apk notes.apk"));
        assert_eq!(stale("notes.apk", t(1120), Some(&src)), None, "the same second is fresh");
        assert_eq!(stale("notes.apk", t(2000), Some(&src)), None);
        assert_eq!(stale("notes.apk", t(1000), None), None, "no sources: nothing to compare");
    }

    #[test]
    fn the_newest_source_is_what_the_build_reads_never_its_output() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().to_path_buf();
        std::fs::create_dir_all(root.join("src/ui")).unwrap();
        std::fs::create_dir_all(root.join("dist")).unwrap();
        let at = |p: &str, s: u64| {
            let f = std::fs::File::options().create(true).append(true).open(root.join(p)).unwrap();
            f.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(s)).unwrap();
        };
        at("Cargo.toml", 1_700_000_000);
        at("src/lib.rs", 1_700_000_000);
        at("src/ui/view.rs", 1_700_000_600);
        at("dist/notes.apk", 1_800_000_000);
        at("src/.view.rs.swp", 1_800_000_000);
        let (p, t) = newest_source(std::slice::from_ref(&root)).unwrap();
        assert_eq!((p, t), (root.join("src/ui/view.rs"), SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_600)));
        assert_eq!(newest_source(&[root.join("nothing")]), None);
    }

    #[test]
    fn an_apk_is_the_apps_own_exactly_and_a_universal_one_serves_any_cpu() {
        assert_eq!(apk_version("notes-1.2.3-android-x86_64.apk", "notes", "x86_64"), Some("1.2.3"));
        assert_eq!(apk_version("notes-1.2.3-rc.1-android-x86_64.apk", "notes", "x86_64"), Some("1.2.3-rc.1"));
        for not in [
            "notes-pro-1.2.3-android-x86_64.apk",
            "notes-1.2-android-x86_64.apk",
            "notes-1.2.3-android-x86_64-debug.apk",
        ] {
            assert_eq!(apk_version(not, "notes", "x86_64"), None, "{not}");
        }
        let t = |s: u64| SystemTime::UNIX_EPOCH + Duration::from_secs(s);
        let files = [("notes-1.0.0-android-universal.apk".to_string(), t(5))];
        assert_eq!(pick_apk(&files, "notes", "armeabi-v7a"), Ok("notes-1.0.0-android-universal.apk"));
    }

    #[test]
    fn the_dev_loop_takes_the_emulator_beside_a_phone_and_android_serial_names_one() {
        // aio android-devices.ts `devTarget`.
        let both = devices("R5CT12ABCDE device model:SM_A525F\nemulator-5600 device model:sdk_gphone64\n");
        assert_eq!(pick_device(&both, None, Want::EmulatorFirst).unwrap().serial, "emulator-5600");
        assert_eq!(pick_device(&both, None, Want::Phone).unwrap().serial, "R5CT12ABCDE", "install: the phone");
        assert!(pick_device(&both, None, Want::Any).unwrap_err().contains("ANDROID_SERIAL=<serial>"));
        assert_eq!(pick_device(&both, Some("R5CT12ABCDE"), Want::EmulatorFirst).unwrap().model, "SM A525F");
        let phone = devices("R5CT12ABCDE device model:SM_A525F\n");
        assert_eq!(pick_device(&phone, None, Want::EmulatorFirst).unwrap().serial, "R5CT12ABCDE", "a lone phone");
    }

    #[test]
    fn an_emulator_is_booted_from_aio_avd_or_the_first_and_says_why_not() {
        let list = "INFO    | Storing crashdata\nPixel_7_API_35\nsmall_29\n";
        assert_eq!(pick_avd(list, None), Ok("Pixel_7_API_35".into()));
        assert_eq!(pick_avd(list, Some("small_29")), Ok("small_29".into()));
        let e = pick_avd(list, Some("gone")).unwrap_err();
        assert!(e.contains("AIO_AVD") && e.contains("Pixel_7_API_35, small_29"), "{e}");
        assert!(pick_avd("", None).unwrap_err().contains("avdmanager create avd"));
        assert_eq!(
            emulator_args("small_29", Some("5600"), true),
            ["-avd", "small_29", "-no-boot-anim", "-no-snapshot-save", "-port", "5600", "-no-window"]
        );
        assert_eq!(emulator_args("a", None, false).len(), 4, "a window where there is a display");
    }

    #[test]
    fn the_emulator_lane_passes_on_a_ready_window_and_fails_on_a_crash_with_its_lines() {
        assert_eq!(start_said("I/aio ( 812): splash: up\n"), None, "not yet");
        assert_eq!(
            start_said("I/aio ( 812): [aio] notes started on Android\nI/aio ( 812): [aio] window ready\n"),
            Some(Ok(()))
        );
        let crash = "E/AndroidRuntime( 812): FATAL EXCEPTION: main\nE/AndroidRuntime( 812): java.lang.NoClassDefFoundError: android.view.WindowInsets$Type\n";
        let why = start_said(crash).unwrap().unwrap_err();
        assert!(why.contains("FATAL EXCEPTION") && why.contains("WindowInsets$Type"), "{why}");
        assert!(start_said("E/aio ( 812): the app did not start\n").unwrap().is_err());
        let old = "I/aio ( 812): [aio] window ready\nE/aio ( 812): this WebView (74.0) is too old for the app's page: update\n";
        assert!(start_said(old).unwrap().is_err(), "a page that cannot run is no pass");
    }
}

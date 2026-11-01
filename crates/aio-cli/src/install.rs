//! `ax install <app> --android`: the built APK onto a real, attached phone,
//! then started (aio's `install:android`). The APK is the one for the phone's
//! CPU; an emulator only when asked (`--emulator`); never an APK older than
//! the app's sources (that would put the previous build on the phone under
//! the same version and say done): `--build` builds first, `--apk <file>`
//! installs a named one regardless.
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

/// One attached device, as `adb devices -l` says.
#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub serial: String,
    pub state: String,
    /// `emulator-5554` is an emulator; anything else is hardware.
    pub emulator: bool,
    pub model: String,
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

/// The device to install on (`serial`: the one named), or why there is none
/// and what to do: each refusal says what is attached.
pub fn pick_device(devices: &[Device], serial: Option<&str>, emulator: bool) -> Result<Device, String> {
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
    let usable: Vec<&Device> = devices.iter().filter(|d| emulator || !d.emulator).collect();
    let ready: Vec<&&Device> = usable.iter().filter(|d| d.state == "device").collect();
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
                "{} devices are attached: name the one you mean\n  {}",
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

/// The newest APK of app `id` for `abi` among `files` (name, modified).
pub fn pick_apk<'a>(files: &'a [(String, SystemTime)], id: &str, abi: &str) -> Result<&'a str, String> {
    let suffix = format!("-android-{abi}.apk");
    files
        .iter()
        .filter(|(n, _)| n.starts_with(&format!("{id}-")) && n.ends_with(&suffix))
        .max_by_key(|(_, t)| *t)
        .map(|(n, _)| n.as_str())
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

/// `adb`: in the SDK's platform-tools, else on the PATH.
fn adb() -> PathBuf {
    let exe = if cfg!(windows) { "adb.exe" } else { "adb" };
    ["ANDROID_HOME", "ANDROID_SDK_ROOT"]
        .iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .chain(std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Android/Sdk")))
        .map(|sdk| sdk.join("platform-tools").join(exe))
        .find(|p| p.exists())
        .unwrap_or_else(|| exe.into())
}

fn adb_out(args: &[&str]) -> Result<String, String> {
    let out =
        Command::new(adb()).args(args).output().map_err(|e| format!("adb: {e} (install the SDK's platform-tools)"))?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    if out.status.success() { Ok(text) } else { Err(format!("adb {}: {}", args.join(" "), text.trim())) }
}

/// What `ax install` was asked.
pub struct Opts<'a> {
    pub build: bool,
    pub emulator: bool,
    pub apk: Option<&'a str>,
    pub device: Option<&'a str>,
    pub launch: bool,
}

/// `ax install <app> --android`.
pub fn android(name: &str, o: &Opts) -> Result<String, String> {
    let meta = crate::dev::metadata()?;
    let pkg = crate::dev::locate(&meta, name)?;
    let (app, cfg) = crate::build::app_of(&pkg)?;
    if o.build {
        eprintln!("{}", crate::build::run(name, Some("android"), None, false)?);
    }
    let dev = pick_device(&parse_devices(&adb_out(&["devices", "-l"])?), o.device, o.emulator)?;
    let label = if dev.model.is_empty() { dev.serial.clone() } else { dev.model.clone() };
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
    let out = adb_out(&["-s", &dev.serial, "install", "-r", &apk.to_string_lossy()])?;
    if out.contains("Failure") {
        return Err(format!(
            "adb install failed on {label}: {}\n  INSTALL_FAILED_UPDATE_INCOMPATIBLE: a build signed with another key \
             is installed; uninstall it on the phone first",
            out.trim()
        ));
    }
    if !o.launch {
        return Ok(format!("{} is installed on {label}", app.name));
    }
    let component = format!("{}/aio.rt.MainActivity", crate::android::application_id_of(&app.id, &cfg.build.android));
    match adb_out(&["-s", &dev.serial, "shell", "am", "start", "-n", &component]) {
        Ok(out) if !out.contains("Error") => Ok(format!("{} is installed and started on {label}", app.name)),
        Ok(out) | Err(out) => Ok(format!(
            "{} is installed on {label}, but did not start ({}): open it from the launcher",
            app.name,
            out.trim().lines().next().unwrap_or_default()
        )),
    }
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
        assert_eq!(pick_device(&d, None, false).unwrap().serial, "R5CT12ABCDE");
    }

    #[test]
    fn an_emulator_alone_is_refused_unless_asked() {
        let only = devices("emulator-5554 device model:sdk_gphone64\n");
        assert!(pick_device(&only, None, false).unwrap_err().contains("--emulator"));
        assert_eq!(pick_device(&only, None, true).unwrap().serial, "emulator-5554");
        assert!(pick_device(&[], None, true).unwrap_err().contains("emulator -avd"));
    }

    #[test]
    fn an_unauthorized_phone_is_the_dialog_and_nothing_says_what_to_do() {
        let e = pick_device(&devices("R5CT12ABCDE unauthorized usb:1-3\n"), None, false).unwrap_err();
        assert!(e.contains("unauthorized") && e.contains("Allow USB debugging"), "{e}");
        let e = pick_device(&devices("R5CT12ABCDE offline\n"), Some("R5CT12ABCDE"), false).unwrap_err();
        assert!(e.contains("adb kill-server"), "{e}");
        assert!(pick_device(&[], None, false).unwrap_err().contains("USB debugging"));
    }

    #[test]
    fn two_phones_are_refused_with_the_exact_flags_and_naming_one_resolves_it() {
        let two = devices("R5CT12ABCDE device model:SM_A525F\n9A281FFAZ004TZ device model:Pixel_7\n");
        let e = pick_device(&two, None, false).unwrap_err();
        assert!(
            e.contains("--device R5CT12ABCDE (SM A525F)") && e.contains("--device 9A281FFAZ004TZ (Pixel 7)"),
            "{e}"
        );
        assert_eq!(pick_device(&two, Some("9A281FFAZ004TZ"), false).unwrap().model, "Pixel 7");
        assert!(pick_device(&two, Some("nope"), false).unwrap_err().contains("attached: R5CT12ABCDE, 9A281FFAZ004TZ"));
    }

    #[test]
    fn the_newest_apk_for_the_phones_cpu_is_the_one_meant() {
        assert_eq!(abi_of("arm64-v8a,armeabi-v7a,armeabi\n"), Some("arm64-v8a"));
        assert_eq!(abi_of("x86_64,x86"), Some("x86_64"));
        assert_eq!(abi_of("armeabi-v7a"), None);
        let t = |s: u64| SystemTime::UNIX_EPOCH + Duration::from_secs(s);
        let files = [
            ("notes-1.0.0-android-arm64-v8a.apk".to_string(), t(1000)),
            ("notes-1.1.0-android-arm64-v8a.apk".to_string(), t(9000)),
            ("notes-1.1.0-android-x86_64.apk".to_string(), t(9500)),
            ("notes-pro-2.0.0-android-arm64-v8a.apk".to_string(), t(9900)),
            ("notes-1.1.0-linux-x64.tar.gz".to_string(), t(9999)),
        ];
        assert_eq!(pick_apk(&files, "notes", "arm64-v8a"), Ok("notes-pro-2.0.0-android-arm64-v8a.apk"));
        assert_eq!(pick_apk(&files[..3], "notes", "arm64-v8a"), Ok("notes-1.1.0-android-arm64-v8a.apk"));
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
}

//! `ax lab <app> <os> --boot` (aio's `am lab`): a real Windows, macOS or
//! Linux desktop, or the Android emulator, booted in a container on this
//! machine for a person to drive from a browser, the app's `dist/` mounted
//! in it at `/shared`. The images are aio's (`dockurr/windows`,
//! `dockurr/macos`: a QEMU VM with a disk and a viewer; `linuxserver/webtop`:
//! an XFCE desktop; `budtmo/docker-android`: the emulator and noVNC), and
//! so are the container names and the disk folder, so `am lab` and `ax lab`
//! work on one lab. Every difference is a row of [`SPECS`]; the argv is
//! pure. Manual: no gate runs it (a first Windows run downloads ~8.5 GB).
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

type Out = Result<String, String>;

/// One lab: a VM has a disk, a container is the guest.
#[derive(Debug, PartialEq)]
pub struct Spec {
    pub os: &'static str,
    pub vm: bool,
    pub image: &'static str,
    pub container: &'static str,
    /// The viewer's port inside the container.
    pub viewer: u16,
    pub kvm: bool,
    pub tun: bool,
    pub extra: &'static [&'static str],
    pub env: &'static [(&'static str, &'static str)],
    /// `PUID`/`PGID` of this user (linuxserver's), so `/shared` stays theirs.
    pub owner: bool,
    /// A VM's defaults: version, RAM, CPUs, disk.
    pub defaults: (&'static str, &'static str, u32, &'static str),
    /// Where the guest's browser or shell finds `/shared` over HTTP.
    pub share: &'static str,
}

pub const SPECS: [Spec; 4] = [
    Spec {
        os: "windows",
        vm: true,
        image: "dockurr/windows:latest",
        container: "aio-lab-windows",
        viewer: 8006,
        kvm: true,
        tun: true,
        extra: &[],
        env: &[],
        owner: false,
        defaults: ("11", "8G", 4, "64G"),
        share: "http://host.lan:8007/ (also \\\\host.lan\\Data)",
    },
    Spec {
        os: "macos",
        vm: true,
        image: "dockurr/macos:latest",
        container: "aio-lab-macos",
        viewer: 8006,
        kvm: true,
        tun: true,
        extra: &[],
        env: &[],
        owner: false,
        defaults: ("14", "8G", 4, "64G"),
        share: "http://host.lan:8007/",
    },
    Spec {
        os: "linux",
        vm: false,
        image: "lscr.io/linuxserver/webtop:ubuntu-xfce",
        container: "aio-lab-linux",
        viewer: 3000,
        kvm: false,
        tun: false,
        extra: &[
            "--shm-size=1g",
            "--device=/dev/fuse",
            "--cap-add=SYS_ADMIN",
            "--security-opt=apparmor:unconfined",
            "--security-opt=seccomp=unconfined",
        ],
        env: &[("TITLE", "aio lab")],
        owner: true,
        defaults: ("", "", 0, ""),
        share: "/shared (the host's dist/, live)",
    },
    Spec {
        os: "android",
        vm: false,
        image: "budtmo/docker-android:emulator_14.0",
        container: "aio-lab-android",
        viewer: 6080,
        kvm: true,
        tun: false,
        extra: &["--shm-size=2g"],
        env: &[("EMULATOR_DEVICE", "Samsung Galaxy S10"), ("WEB_VNC", "true")],
        owner: false,
        defaults: ("", "", 0, ""),
        share: "the APK, installed by ax over adb",
    },
];

/// The lab of `os`, or what the choices are.
pub fn spec(os: &str) -> Result<&'static Spec, String> {
    let os = match os {
        "win" => "windows",
        "mac" | "osx" => "macos",
        "ubuntu" => "linux",
        "apk" => "android",
        o => o,
    };
    SPECS.iter().find(|s| s.os == os).ok_or_else(|| {
        format!("--boot: no lab {os} (labs: {})", SPECS.iter().map(|s| s.os).collect::<Vec<_>>().join(", "))
    })
}

/// What `--boot` was asked: a VM's knobs (refused for a container), the
/// viewer's port on loopback.
#[derive(Debug, Default)]
pub struct Ask<'a> {
    pub ram: Option<&'a str>,
    pub cpus: Option<&'a str>,
    pub disk: Option<&'a str>,
    pub version: Option<&'a str>,
    pub port: Option<u16>,
    pub apk: Option<&'a str>,
}

/// How long a VM is given to shut its disk down cleanly.
const STOP_SECS: u32 = 120;

/// The `docker run` of lab `s` (pure): its viewer on `127.0.0.1:port`, a
/// VM's disk in `storage`, `share` at `/shared`; `owner`: this user's ids.
pub fn run_argv(s: &Spec, a: &Ask, port: u16, storage: &Path, share: &Path, owner: Option<(u32, u32)>) -> Vec<String> {
    let mut v: Vec<String> = ["run", "--detach", "--name", s.container].map(String::from).to_vec();
    if s.kvm {
        v.push("--device=/dev/kvm".into());
    }
    if s.tun {
        v.extend(["--device=/dev/net/tun", "--cap-add", "NET_ADMIN"].map(String::from));
    }
    v.extend(["--stop-timeout".into(), STOP_SECS.to_string()]);
    v.extend(s.extra.iter().map(|x| x.to_string()));
    let mut env = |k: &str, val: &str| v.extend(["-e".into(), format!("{k}={val}")]);
    if s.vm {
        let (version, ram, cpus, disk) = s.defaults;
        env("VERSION", a.version.unwrap_or(version));
        env("RAM_SIZE", a.ram.unwrap_or(ram));
        env("CPU_CORES", &a.cpus.map_or(cpus.to_string(), String::from));
        env("DISK_SIZE", a.disk.unwrap_or(disk));
    }
    for (k, val) in s.env {
        env(k, val);
    }
    if let (true, Some((uid, gid))) = (s.owner, owner) {
        env("PUID", &uid.to_string());
        env("PGID", &gid.to_string());
    }
    v.extend(["-p".into(), format!("127.0.0.1:{port}:{}", s.viewer)]);
    if s.vm {
        v.extend(["-v".into(), format!("{}:/storage", storage.display())]);
    }
    v.extend(["-v".into(), format!("{}:/shared", share.display()), s.image.into()]);
    v
}

/// Where a lab keeps its VM disk (aio's place, so both see one lab).
pub fn storage(os: &str) -> PathBuf {
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache"));
    cache.join("aio").join("labs").join(os).join("storage")
}

/// The APK to install from `names` (in `dist/`, pure): the one asked, else the
/// newest-named signed one, a `-client` one first when there are several.
pub fn pick_apk(names: &[String], asked: Option<&str>) -> Result<String, String> {
    let mut apks: Vec<&String> =
        names.iter().filter(|n| n.ends_with(".apk") && !n.ends_with("-unsigned.apk")).collect();
    apks.sort();
    if let Some(a) = asked {
        let a = Path::new(a).file_name().and_then(|f| f.to_str()).unwrap_or(a);
        return apks.iter().find(|n| **n == a).map(|n| n.to_string()).ok_or_else(|| {
            format!(
                "--apk {a}: not in dist/ (there: {})",
                apks.iter().map(|n| n.as_str()).collect::<Vec<_>>().join(", ")
            )
        });
    }
    let clients: Vec<&&String> = apks.iter().filter(|n| n.ends_with("-client.apk")).collect();
    match (clients.last(), apks.last()) {
        (Some(c), _) => Ok(c.to_string()),
        (None, Some(n)) => Ok(n.to_string()),
        _ => Err("no APK in dist/: ax build <app> --android first".into()),
    }
}

fn docker(args: &[&str]) -> Result<String, String> {
    let o = Command::new("docker").args(args).output().map_err(|e| format!("docker: {e} (is Docker installed?)"))?;
    let out = String::from_utf8_lossy(&o.stdout).trim().to_string();
    if o.status.success() {
        Ok(out)
    } else {
        Err(format!("docker {}: {}", args.first().unwrap_or(&""), String::from_utf8_lossy(&o.stderr).trim()))
    }
}

/// `running|startedAt|status` of the lab's container; `None`: there is none.
fn state(s: &Spec) -> Option<(bool, String)> {
    let line = docker(&["inspect", s.container, "--format", "{{.State.Running}}|{{.State.Status}}"]).ok()?;
    let (running, status) = line.split_once('|')?;
    Some((running == "true", status.into()))
}

/// The viewer's published port of the running lab.
fn port_of(s: &Spec) -> Option<u16> {
    let out = docker(&["port", s.container, &format!("{}/tcp", s.viewer)]).ok()?;
    out.lines().find_map(|l| l.rsplit_once(':')?.1.trim().parse().ok())
}

/// A free loopback port for the viewer.
fn free_port() -> Result<u16, String> {
    std::net::TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()).map(|a| a.port()).map_err(|e| e.to_string())
}

/// What docker and the machine lack for lab `s`, each with its fix.
fn preflight(s: &Spec) -> Result<(), String> {
    docker(&["info", "--format", "{{.ServerVersion}}"])
        .map_err(|e| format!("{e}\n  a lab is a container: Docker must run, and this user reach it (docker info)"))?;
    if s.kvm && !Path::new("/dev/kvm").exists() {
        return Err(format!("the {} lab needs /dev/kvm (hardware virtualization, on and reachable)", s.os));
    }
    Ok(())
}

/// `ax lab <app> <os> --boot|--status|--stop|--reset`: `dist` is the
/// app's `dist/` (mounted at `/shared`).
pub fn run(os: &str, what: &str, a: &Ask, dist: &Path) -> Out {
    let s = spec(os)?;
    if !s.vm && (a.ram.is_some() || a.cpus.is_some() || a.disk.is_some() || a.version.is_some()) {
        return Err(format!("--ram, --cpus, --disk and --os-version are a VM's: the {} lab is a container", s.os));
    }
    if a.apk.is_some() && s.os != "android" {
        return Err("--apk is the android lab's".into());
    }
    match what {
        "status" => Ok(match state(s) {
            None => format!("{}: not booted (ax lab <app> {} --boot)", s.os, s.os),
            Some((true, _)) => {
                let at = port_of(s).map_or(String::new(), |p| format!(": the viewer at http://127.0.0.1:{p}/"));
                format!("{}: up ({}){at}", s.os, s.container)
            }
            Some((false, st)) => format!("{}: {st} (--boot starts it again; --stop removes it)", s.os),
        }),
        "stop" | "reset" => {
            let mut said = vec![];
            if state(s).is_some() {
                _ = docker(&["stop", "--time", &STOP_SECS.to_string(), s.container]);
                docker(&["rm", "--force", s.container])?;
                said.push(format!("{}: stopped and removed {}", s.os, s.container));
            } else {
                said.push(format!("{}: no lab running", s.os));
            }
            let disk = storage(s.os);
            if what == "reset" && s.vm && disk.exists() {
                std::fs::remove_dir_all(&disk).map_err(|e| format!("{}: {e}", disk.display()))?;
                said.push(format!("its disk {} deleted: the next --boot installs anew", disk.display()));
            } else if s.vm && disk.exists() {
                said.push(format!("its disk stays in {} (--reset deletes it)", disk.display()));
            }
            Ok(said.join("\n"))
        }
        _ => boot(s, a, dist),
    }
}

fn boot(s: &Spec, a: &Ask, dist: &Path) -> Out {
    preflight(s)?;
    std::fs::create_dir_all(dist).map_err(|e| format!("{}: {e}", dist.display()))?;
    let port = match state(s) {
        Some((true, _)) => port_of(s).ok_or("the lab runs, its viewer's port unpublished: --stop, then --boot")?,
        Some((false, _)) => {
            docker(&["start", s.container])?;
            port_of(s).ok_or("its viewer's port unpublished: --stop, then --boot")?
        }
        None => {
            let port = match a.port {
                Some(p) => p,
                None => free_port()?,
            };
            let disk = storage(s.os);
            if s.vm {
                std::fs::create_dir_all(&disk).map_err(|e| format!("{}: {e}", disk.display()))?;
            }
            let owner = owner_ids();
            let argv = run_argv(s, a, port, &disk, dist, owner);
            docker(&argv.iter().map(String::as_str).collect::<Vec<_>>())?;
            port
        }
    };
    let viewer = format!("http://127.0.0.1:{port}/");
    let up = wait(Duration::from_secs(120), || std::net::TcpStream::connect(("127.0.0.1", port)).is_ok());
    let mut said = vec![format!(
        "{}: {} (the viewer: {viewer}{})",
        s.os,
        if up { "up" } else { "booting" },
        if up { "" } else { ", soon; `docker logs -f` it to watch" }
    )];
    if s.vm {
        said.push(share(s));
    }
    if s.os == "android" {
        said.push(install(s, a, dist)?);
    } else {
        said.push(format!("the app's dist/ is there: {}", s.share));
    }
    said.push(format!("ax lab <app> {} --stop ends it", s.os));
    Ok(said.join("\n"))
}

/// This user's uid and gid (linuxserver's `PUID`/`PGID`).
fn owner_ids() -> Option<(u32, u32)> {
    let id = |f: &str| {
        Command::new("id").arg(f).output().ok().and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
    };
    Some((id("-u")?, id("-g")?))
}

fn wait(budget: Duration, done: impl Fn() -> bool) -> bool {
    let end = Instant::now() + budget;
    while Instant::now() < end {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    false
}

/// A VM guest reaches `/shared` at `http://host.lan:8007/`: a read-only file
/// server in the container, on the guest's side (aio's share).
fn share(s: &Spec) -> String {
    let Ok(ip) = docker(&["exec", s.container, "cat", "/run/shm/qemu.gw"]) else {
        return format!("the guest's share starts once its network is up: run --boot again then ({})", s.share);
    };
    let ip = ip.lines().next().unwrap_or_default().trim().to_string();
    let serve =
        format!("exec python3 -m http.server 8007 --directory /shared --bind {ip} >> /run/shm/aio-share.log 2>&1");
    match docker(&["exec", "--detach", s.container, "sh", "-c", &serve]) {
        Ok(_) => format!("the app's dist/ is at {} in the guest", s.share),
        Err(e) => format!("no share in the guest: {e}"),
    }
}

/// The Android lab: once it has booted, the app's APK installed over adb.
fn install(s: &Spec, a: &Ask, dist: &Path) -> Out {
    let names: Vec<String> = std::fs::read_dir(dist)
        .map_err(|e| format!("{}: {e}", dist.display()))?
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(String::from))
        .collect();
    let apk = pick_apk(&names, a.apk)?;
    let booted = || {
        docker(&["exec", s.container, "adb", "shell", "getprop", "sys.boot_completed"]).is_ok_and(|o| o.trim() == "1")
    };
    if !wait(Duration::from_secs(300), booted) {
        return Err(format!(
            "the emulator hasn't booted within 5 minutes: `docker logs -f {}`, then --boot again to install {apk}",
            s.container
        ));
    }
    docker(&["exec", s.container, "adb", "install", "-r", &format!("/shared/{apk}")])
        .map(|_| format!("installed {apk} (adb install -r)"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lab_is_a_row_and_its_run_is_pure() {
        assert_eq!(spec("win").unwrap().os, "windows");
        assert!(spec("beos").unwrap_err().contains("labs: windows, macos, linux, android"));
        let win = run_argv(
            spec("windows").unwrap(),
            &Ask { ram: Some("16G"), ..Default::default() },
            8100,
            Path::new("/c/w"),
            Path::new("/a/dist"),
            None,
        );
        let w = win.join(" ");
        assert!(w.starts_with("run --detach --name aio-lab-windows --device=/dev/kvm --device=/dev/net/tun --cap-add NET_ADMIN --stop-timeout 120"), "{w}");
        assert!(w.contains("-e VERSION=11 -e RAM_SIZE=16G -e CPU_CORES=4 -e DISK_SIZE=64G"), "{w}");
        assert!(
            w.ends_with("-p 127.0.0.1:8100:8006 -v /c/w:/storage -v /a/dist:/shared dockurr/windows:latest"),
            "{w}"
        );
        let linux = run_argv(
            spec("linux").unwrap(),
            &Ask::default(),
            9000,
            Path::new("/c"),
            Path::new("/d"),
            Some((1000, 1000)),
        );
        let l = linux.join(" ");
        assert!(!l.contains("/dev/kvm") && !l.contains("/storage") && l.contains("--device=/dev/fuse"), "{l}");
        assert!(l.contains("-e TITLE=aio lab -e PUID=1000 -e PGID=1000 -p 127.0.0.1:9000:3000"), "{l}");
        let droid =
            run_argv(spec("android").unwrap(), &Ask::default(), 6100, Path::new("/c"), Path::new("/d"), Some((1, 1)));
        assert!(
            droid
                .join(" ")
                .contains("--device=/dev/kvm --stop-timeout 120 --shm-size=2g -e EMULATOR_DEVICE=Samsung Galaxy S10")
        );
        assert!(!droid.iter().any(|a| a.starts_with("PUID")), "only linuxserver's image takes the owner");
        // A container has no VM knobs.
        let e =
            run("linux", "boot", &Ask { ram: Some("8G"), ..Default::default() }, Path::new("/nowhere")).unwrap_err();
        assert!(e.contains("are a VM's"), "{e}");
        assert!(
            run("linux", "boot", &Ask { apk: Some("x.apk"), ..Default::default() }, Path::new("/n"))
                .unwrap_err()
                .contains("android")
        );
    }

    #[test]
    fn the_apk_is_the_one_asked_else_the_client_else_the_newest() {
        let n = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let dist = n(&["notes-1.0.0-arm64.apk", "notes-1.1.0-arm64.apk", "notes-1.1.0-unsigned.apk", "notes.tar.gz"]);
        assert_eq!(pick_apk(&dist, None).unwrap(), "notes-1.1.0-arm64.apk");
        assert_eq!(pick_apk(&dist, Some("dist/notes-1.0.0-arm64.apk")).unwrap(), "notes-1.0.0-arm64.apk");
        assert!(pick_apk(&dist, Some("x.apk")).unwrap_err().contains("not in dist/"));
        let both = n(&["notes-1.1.0-arm64.apk", "notes-1.1.0-client.apk"]);
        assert_eq!(pick_apk(&both, None).unwrap(), "notes-1.1.0-client.apk");
        assert!(pick_apk(&[], None).unwrap_err().contains("--android first"));
    }
}

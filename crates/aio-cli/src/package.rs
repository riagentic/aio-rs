//! System packages of a built app: `--deb` (installed system-wide by the
//! package manager, which then updates it: the app's own updater stays off)
//! and `--appimage` (one portable file). Like the bundle, neither carries the
//! shared runtime: the app fetches it once per machine.
use crate::build::{App, Built, desktop_entry};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The AppImage runtime (AppImage/type2-runtime, release 20251108): pinned
/// by the SHA-256 GitHub publishes for it.
const APPIMAGE_RUNTIME: &str = "https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-";

fn appimage_runtime_sha(arch: &str) -> Option<&'static str> {
    match arch {
        "x86_64" => Some("2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d"),
        "aarch64" => Some("00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444"),
        _ => None,
    }
}

/// A Debian package name: lowercase letters, digits, `+ - .`.
pub fn deb_name(id: &str) -> String {
    id.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() || "+-.".contains(c) { c } else { '-' }).collect()
}

/// A Debian version that sorts as the app's: `1.2.0-rc.1` → `1.2.0~rc.1`
/// (before `1.2.0`); `+build` dropped.
pub fn deb_version(v: &str) -> String {
    let v = v.split('+').next().unwrap_or(v);
    v.replacen('-', "~", 1)
}

/// Debian's word for CPU `arch` (`std::env::consts::ARCH`'s).
pub fn deb_arch(arch: &str) -> &str {
    match arch {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        a => a,
    }
}

/// What the shared runtime (Electron) needs from the system.
const DEPENDS: &str = "libgtk-3-0t64 | libgtk-3-0, libnss3, libgbm1, libasound2t64 | libasound2, libxss1, libxtst6, \
                       libatspi2.0-0t64 | libatspi2.0-0, libsecret-1-0, xdg-utils";

/// `DEBIAN/control`.
pub fn deb_control(app: &App, arch: &str, maintainer: &str, installed_kb: u64) -> String {
    format!(
        "Package: {}\nVersion: {}\nArchitecture: {}\nMaintainer: {maintainer}\nInstalled-Size: {installed_kb}\n\
         Depends: {DEPENDS}\nSection: utils\nPriority: optional\nDescription: {}\n \
         {} is an aio-rs app. Its runtime (Electron) is fetched once per machine, on first start.\n",
        deb_name(&app.id),
        deb_version(&app.version),
        deb_arch(arch),
        app.name.replace('\n', " "),
        app.name.replace('\n', " "),
    )
}

fn sh(cmd: &mut Command) -> Result<(), String> {
    let out = cmd.output().map_err(|e| format!("{:?}: {e}", cmd.get_program()))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("{:?} failed: {}", cmd.get_program(), String::from_utf8_lossy(&out.stderr).trim()))
    }
}

fn put(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path.parent().ok_or("no parent")?).map_err(|e| e.to_string())?;
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(|e| e.to_string())
}

/// Every dir under `root` (and it) readable by all, as packages need,
/// whatever the umask made them.
fn open_dirs(root: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    for e in std::fs::read_dir(root).map_err(|e| e.to_string())?.flatten() {
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            open_dirs(&e.path())?;
        }
    }
    Ok(())
}

fn dir_kb(dir: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_kb(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map_or(0, |m| m.len().div_ceil(1024)),
            _ => 0,
        })
        .sum()
}

/// `dist/<pkg>_<version>_<arch>.deb`.
pub fn deb(b: &Built) -> Result<PathBuf, String> {
    let app = &b.app;
    let maintainer =
        b.cfg.app.maintainer.as_deref().ok_or(
            "--deb needs a maintainer: add maintainer = \"Your Name <you@example.org>\" under [app] in aio.toml",
        )?;
    let dist = &b.dist;
    std::fs::create_dir_all(dist).map_err(|e| e.to_string())?;
    let root = dist.join(format!(".{}-deb", app.id));
    _ = std::fs::remove_dir_all(&root);
    let made = (|| {
        let id = &app.id;
        put(&root.join(format!("usr/lib/{id}/{id}")), &std::fs::read(&b.binary).map_err(|e| e.to_string())?, 0o755)?;
        crate::build::put_runtime(b, &root.join(format!("usr/lib/{id}")))?;
        std::fs::create_dir_all(root.join("usr/bin")).map_err(|e| e.to_string())?;
        std::os::unix::fs::symlink(format!("../lib/{id}/{id}"), root.join(format!("usr/bin/{id}")))
            .map_err(|e| e.to_string())?;
        put(
            &root.join(format!("usr/share/applications/{id}.desktop")),
            desktop_entry(app, &format!("/usr/bin/{id}")).as_bytes(),
            0o644,
        )?;
        if let Some(icon) = &app.icon {
            let png = std::fs::read(icon).map_err(|e| e.to_string())?;
            put(&root.join(format!("usr/share/icons/hicolor/256x256/apps/{id}.png")), &png, 0o644)?;
        }
        let control = deb_control(app, b.platform.arch, maintainer, dir_kb(&root));
        put(&root.join("DEBIAN/control"), control.as_bytes(), 0o644)?;
        open_dirs(&root)?;
        let out =
            dist.join(format!("{}_{}_{}.deb", deb_name(id), deb_version(&app.version), deb_arch(b.platform.arch)));
        sh(Command::new("dpkg-deb").args(["--root-owner-group", "-Zxz", "--build"]).arg(&root).arg(&out))?;
        Ok(out)
    })();
    _ = std::fs::remove_dir_all(&root);
    made
}

/// The pinned AppImage runtime for CPU `arch`, fetched once and checked
/// every time.
fn appimage_runtime(arch: &str) -> Result<Vec<u8>, String> {
    let want = appimage_runtime_sha(arch).ok_or_else(|| format!("no AppImage runtime for {arch}"))?;
    let cache = crate::build::cache_dir().join(format!("appimage/runtime-{arch}-{want}"));
    let ok = |b: &[u8]| aio::sha256::hex(&aio::sha256::Sha256::digest(b)) == want;
    if let Ok(b) = std::fs::read(&cache)
        && ok(&b)
    {
        return Ok(b);
    }
    let url = format!("{APPIMAGE_RUNTIME}{arch}");
    eprintln!("ax build: fetching the AppImage runtime (once): {url}");
    std::fs::create_dir_all(cache.parent().ok_or("no cache dir")?).map_err(|e| e.to_string())?;
    let tmp = cache.with_extension(format!("partial-{}", std::process::id()));
    let got = aio_shell::runtime::download(&url, &tmp, &mut |_, _| true).map_err(|e| e.to_string());
    let r = match got {
        Ok(sha) if sha == want => std::fs::rename(&tmp, &cache).map_err(|e| e.to_string()),
        Ok(sha) => Err(format!("{url}: SHA-256 {sha}, expected {want}: not the pinned runtime")),
        Err(e) => Err(e),
    };
    _ = std::fs::remove_file(&tmp);
    r?;
    std::fs::read(&cache).map_err(|e| e.to_string())
}

/// `dist/<id>-<version>-<arch>.AppImage`.
pub fn appimage(b: &Built) -> Result<PathBuf, String> {
    let app = &b.app;
    let id = &app.id;
    let dist = &b.dist;
    std::fs::create_dir_all(dist).map_err(|e| e.to_string())?;
    let appdir = dist.join(format!(".{id}-AppDir"));
    let squash = dist.join(format!(".{id}.squashfs"));
    _ = std::fs::remove_dir_all(&appdir);
    _ = std::fs::remove_file(&squash);
    let made = (|| {
        let runtime = appimage_runtime(b.platform.arch)?;
        put(&appdir.join(format!("usr/bin/{id}")), &std::fs::read(&b.binary).map_err(|e| e.to_string())?, 0o755)?;
        crate::build::put_runtime(b, &appdir.join("usr/bin"))?;
        std::os::unix::fs::symlink(format!("usr/bin/{id}"), appdir.join("AppRun")).map_err(|e| e.to_string())?;
        let entry = desktop_entry(app, id).replace(&format!("Exec=\"{id}\""), &format!("Exec={id}"));
        put(&appdir.join(format!("{id}.desktop")), entry.as_bytes(), 0o644)?;
        if let Some(icon) = &app.icon {
            put(&appdir.join(format!("{id}.png")), &std::fs::read(icon).map_err(|e| e.to_string())?, 0o644)?;
            std::os::unix::fs::symlink(format!("{id}.png"), appdir.join(".DirIcon")).map_err(|e| e.to_string())?;
        }
        open_dirs(&appdir)?;
        sh(Command::new("mksquashfs").arg(&appdir).arg(&squash).args([
            "-root-owned",
            "-noappend",
            "-no-xattrs",
            "-comp",
            "zstd",
            "-quiet",
        ]))?;
        let out = dist.join(format!("{id}-{}-{}.AppImage", app.version, b.platform.arch));
        let mut bytes = runtime;
        bytes.extend(std::fs::read(&squash).map_err(|e| e.to_string())?);
        put(&out, &bytes, 0o755)?;
        Ok(out)
    })();
    _ = std::fs::remove_dir_all(&appdir);
    _ = std::fs::remove_file(&squash);
    made
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debian_names_and_versions_sort_like_the_app() {
        assert_eq!(deb_name("My_Notes"), "my-notes");
        assert_eq!(deb_version("1.2.0"), "1.2.0");
        assert_eq!(deb_version("1.2.0-rc.1+b7"), "1.2.0~rc.1", "a prerelease sorts before its release");
        let app = App { id: "notes".into(), name: "Notes".into(), version: "1.2.0".into(), icon: None };
        let c = deb_control(&app, "aarch64", "Ann <ann@example.org>", 9000);
        assert!(c.starts_with("Package: notes\nVersion: 1.2.0\nArchitecture: arm64\n"), "{c}");
        assert_eq!(deb_arch("x86_64"), "amd64");
        assert!(
            c.contains("\nMaintainer: Ann <ann@example.org>\nInstalled-Size: 9000\n") && c.ends_with("first start.\n")
        );
    }

    #[test]
    fn the_appimage_runtime_is_pinned_per_arch() {
        assert_eq!(appimage_runtime_sha("x86_64").map(str::len), Some(64));
        assert_eq!(appimage_runtime_sha("riscv64"), None);
    }
}

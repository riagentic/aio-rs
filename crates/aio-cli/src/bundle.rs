//! The macOS and Windows bundles `ax build` makes (Linux's is in `build`):
//! `<id>-<version>-macos-<arch>.zip` holding `<Name>.app`, signed ad hoc
//! (unzip it, drag it to Applications; made away from a Mac, its binary
//! alone is signed, by zig's linker), and `<id>-<version>-windows-<arch>.zip`
//! holding `<id>.exe` (the update artifact), beside the same exe as the
//! one-click download `<id>-<version>-windows-<arch>.exe`: double-clicked,
//! it installs itself (`aio_shell::oneclick`; `--install` too). As on
//! Linux, the shared runtime is not inside; an offline build's own is (the
//! one-click exe carries it appended, and puts it beside the installed exe).
use crate::build::App;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The `.app`'s `Info.plist` (pure). The bundle only launches the app, so it
/// is no Dock entry of its own (`LSUIElement`): its window's bundle is.
pub fn info_plist(app: &App, bundle_id: &str) -> String {
    let x = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let icon = if app.icon.is_some() { "\t<key>CFBundleIconFile</key>\n\t<string>AppIcon</string>\n" } else { "" };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n\
         \t<key>CFBundleExecutable</key>\n\t<string>{id}</string>\n\
         \t<key>CFBundleIdentifier</key>\n\t<string>{bundle_id}</string>\n\
         \t<key>CFBundleName</key>\n\t<string>{name}</string>\n\
         \t<key>CFBundleDisplayName</key>\n\t<string>{name}</string>\n\
         \t<key>CFBundlePackageType</key>\n\t<string>APPL</string>\n\
         \t<key>CFBundleShortVersionString</key>\n\t<string>{v}</string>\n\
         \t<key>CFBundleVersion</key>\n\t<string>{v}</string>\n\
         {icon}\
         \t<key>LSMinimumSystemVersion</key>\n\t<string>11.0</string>\n\
         \t<key>LSUIElement</key>\n\t<true/>\n\
         \t<key>NSHighResolutionCapable</key>\n\t<true/>\n\
         </dict>\n</plist>\n",
        id = x(&app.id),
        bundle_id = x(bundle_id),
        name = x(&app.name),
        v = x(&app.version),
    )
}

fn run(cmd: &mut Command, what: &str) -> Result<(), String> {
    let out = cmd.output().map_err(|e| format!("{what}: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    Err(format!("{what}: {}", String::from_utf8_lossy(&out.stderr).trim()))
}

/// `<Name>.app` from `binary`, as `bundle_id`, in `staging`: on a Mac
/// signed ad hoc whole (`codesign`); elsewhere unsealed, its binary signed
/// by its linker. Its own runtime (`runtime`, an offline build) goes in as
/// `Contents/MacOS/electron/Electron.app`, where the binary looks, and only
/// that: `codesign` takes nothing but code there, so Electron's licences go
/// in `Resources`.
pub fn mac_app(
    app: &App,
    bundle_id: &str,
    binary: &Path,
    runtime: Option<&Path>,
    staging: &Path,
) -> Result<PathBuf, String> {
    let bundle = staging.join(format!("{}.app", app.name.replace('/', "-")));
    let contents = bundle.join("Contents");
    let e = |e: std::io::Error| e.to_string();
    std::fs::create_dir_all(contents.join("MacOS")).map_err(e)?;
    std::fs::create_dir_all(contents.join("Resources")).map_err(e)?;
    std::fs::write(contents.join("Info.plist"), info_plist(app, bundle_id)).map_err(e)?;
    std::fs::write(contents.join("PkgInfo"), "APPL????").map_err(e)?;
    std::fs::copy(binary, contents.join("MacOS").join(&app.id)).map_err(e)?;
    if let Some(icon) = &app.icon {
        // Built here, every slot from 16 to 1024 px: the monogram's drawn
        // beside it at each size, an `icon.icns` of the app's own as it is,
        // else its `icon.png` scaled (no `sips`, so any host can make it).
        let icns = icon.with_extension("icns");
        let icns = match icns.is_file() {
            true => std::fs::read(&icns).map_err(e)?,
            false => crate::icon::icns_from_png(&std::fs::read(icon).map_err(e)?)?,
        };
        std::fs::write(contents.join("Resources/AppIcon.icns"), icns).map_err(e)?;
    }
    if let Some(rt) = runtime {
        let into = contents.join("MacOS").join(aio_shell::runtime::BUNDLED).join("Electron.app");
        // Its links and modes kept (ditto on a Mac, cp elsewhere).
        std::fs::create_dir_all(into.parent().ok_or("no folder")?).map_err(e)?;
        let copy = match cfg!(target_os = "macos") {
            true => Command::new("/usr/bin/ditto").arg(rt.join("Electron.app")).arg(&into).output(),
            false => Command::new("cp").arg("-R").arg("-P").arg(rt.join("Electron.app")).arg(&into).output(),
        };
        match copy {
            Ok(o) if o.status.success() => {}
            _ => return Err(format!("could not copy the runtime into {}", bundle.display())),
        }
        for f in ["LICENSE", "LICENSES.chromium.html"].map(|f| rt.join(f)).iter().filter(|f| f.is_file()) {
            std::fs::copy(f, contents.join("Resources").join(f.file_name().unwrap_or_default())).map_err(e)?;
        }
    }
    // Ad hoc: Apple Silicon runs nothing unsigned. A Developer ID and
    // notarization replace it for downloads (Gatekeeper).
    if cfg!(target_os = "macos") {
        run(Command::new("/usr/bin/codesign").args(["--force", "--sign", "-"]).arg(&bundle), "codesign")?;
    }
    Ok(bundle)
}

/// `bundle` (a `.app`) zipped into `out`, its links, modes and signature
/// kept (`ditto` on a Mac).
pub fn mac_zip(bundle: &Path, out: &Path) -> Result<(), String> {
    if cfg!(target_os = "macos") {
        return run(Command::new("/usr/bin/ditto").args(["-c", "-k", "--keepParent"]).arg(bundle).arg(out), "ditto");
    }
    let e = |e: std::io::Error| e.to_string();
    let parent = bundle.parent().ok_or("the bundle has no folder")?;
    let mut all = vec![];
    walk(bundle, &mut all).map_err(|e| format!("{}: {e}", bundle.display()))?;
    all.sort();
    let mut z = zip::ZipWriter::new(std::fs::File::create(out).map_err(e)?);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(zip_now());
    for f in all {
        let name = f.strip_prefix(parent).map_err(|e| e.to_string())?.to_string_lossy().replace('\\', "/");
        let meta = std::fs::symlink_metadata(&f).map_err(e)?;
        if meta.is_symlink() {
            let to = std::fs::read_link(&f).map_err(e)?;
            z.add_symlink(name, to.to_string_lossy().replace('\\', "/"), opts).map_err(|e| e.to_string())?;
            continue;
        }
        z.start_file(name, opts.unix_permissions(mode(&f, &meta))).map_err(|e| e.to_string())?;
        z.write_all(&std::fs::read(&f).map_err(e)?).map_err(e)?;
    }
    z.finish().map(drop).map_err(|e| e.to_string())
}

/// Every file and link under `dir`.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for e in std::fs::read_dir(dir)? {
        let p = e?.path();
        match std::fs::symlink_metadata(&p)?.is_dir() {
            true => walk(&p, out)?,
            false => out.push(p),
        }
    }
    Ok(())
}

/// A file's mode in the zip: 0755 when it runs, else 0644, whatever this
/// machine's umask; on Windows, which has no modes, what `Contents/MacOS`
/// holds runs.
fn mode(f: &Path, meta: &std::fs::Metadata) -> u32 {
    #[cfg(unix)]
    let runs = std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o111 != 0 && f.is_file();
    #[cfg(not(unix))]
    let runs = meta.is_file() && f.parent().is_some_and(|d| d.ends_with("Contents/MacOS"));
    if runs { 0o755 } else { 0o644 }
}

/// Now, as a zip's time (its UTC fields; a zip has no zone).
fn zip_now() -> zip::DateTime {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
    let s = aio::log::stamp(ms);
    let n = |a: usize, b: usize| s.get(a..b).and_then(|v| v.parse().ok()).unwrap_or(0);
    zip::DateTime::from_date_and_time(
        n(0, 4),
        n(5, 7) as u8,
        n(8, 10) as u8,
        n(11, 13) as u8,
        n(14, 16) as u8,
        n(17, 19) as u8,
    )
    .unwrap_or_default()
}

/// `hdiutil`'s arguments for the disk image `out` of folder `stage` (the
/// `.app` and a link to Applications to drag it onto), named `volume`:
/// compressed (UDZO, what every shipped `.dmg` is), a stale one replaced.
pub fn dmg_args(volume: &str, stage: &Path, out: &Path) -> Vec<String> {
    let mut a: Vec<String> =
        ["create", "-volname", volume, "-fs", "HFS+", "-format", "UDZO", "-ov", "-srcfolder"].map(String::from).into();
    a.push(stage.display().to_string());
    a.push(out.display().to_string());
    a
}

/// `bundle` (a `.app` alone in its folder) as the disk image `out`, with
/// the Applications link beside it (made on a Mac: `hdiutil`).
pub fn mac_dmg(app: &App, bundle: &Path, out: &Path) -> Result<(), String> {
    let stage = bundle.parent().ok_or("no staging dir")?;
    #[cfg(unix)]
    std::os::unix::fs::symlink("/Applications", stage.join("Applications")).map_err(|e| e.to_string())?;
    run(Command::new("/usr/bin/hdiutil").args(dmg_args(&app.name, stage, out)), "hdiutil")
}

/// A zip holding `<id>.exe`, and its own runtime (`runtime`, an offline
/// build) as `electron/` beside it.
pub fn windows_zip(app: &App, binary: &Path, runtime: Option<&Path>, out: &Path) -> Result<(), String> {
    let e = |e: std::io::Error| e.to_string();
    let file = std::fs::File::create(out).map_err(e)?;
    let mut z = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    z.start_file(format!("{}.exe", app.id), opts).map_err(|e| e.to_string())?;
    z.write_all(&std::fs::read(binary).map_err(e)?).map_err(e)?;
    if let Some(rt) = runtime {
        zip_dir(&mut z, rt)?;
    }
    z.finish().map(drop).map_err(|e| e.to_string())
}

/// The runtime in `rt` zipped as `electron/`: what the one-click download
/// carries (`aio_shell::oneclick::with_runtime`).
pub fn runtime_zip(rt: &Path) -> Result<Vec<u8>, String> {
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip_dir(&mut z, rt)?;
    z.finish().map(|c| c.into_inner()).map_err(|e| e.to_string())
}

/// Every file under `rt` into `z`, under `electron/`, in name order.
fn zip_dir<W: Write + std::io::Seek>(z: &mut zip::ZipWriter<W>, rt: &Path) -> Result<(), String> {
    fn files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
        for e in std::fs::read_dir(dir)? {
            let p = e?.path();
            if p.is_dir() { files(&p, out)? } else { out.push(p) }
        }
        Ok(())
    }
    let mut all = vec![];
    files(rt, &mut all).map_err(|e| format!("{}: {e}", rt.display()))?;
    all.sort();
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for f in all {
        let rel = f.strip_prefix(rt).map_err(|e| e.to_string())?.to_string_lossy().replace('\\', "/");
        z.start_file(format!("{}/{rel}", aio_shell::runtime::BUNDLED), opts).map_err(|e| e.to_string())?;
        z.write_all(&std::fs::read(&f).map_err(|e| format!("{}: {e}", f.display()))?).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// What a user does with the bundle at `out`.
pub fn how_to_install(app: &App, os: &str, out: &Path) -> String {
    let file = out.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    match os {
        "macos" if file.ends_with(".dmg") => {
            format!("install: open {file}, then drag {}.app to Applications", app.name)
        }
        "macos" => format!("install: unzip {file}, then drag {}.app to Applications", app.name),
        "windows" => format!(
            "install: double-click {} (no admin rights needed); {file} is the same exe, for updates",
            out.with_extension("exe").file_name().unwrap_or_default().to_string_lossy()
        ),
        // aio's default Linux artifact was the AppImage: say where it is.
        _ => format!("install: tar -xzf {file}, then run its install.sh (one portable file instead: --appimage)"),
    }
}

/// Where cargo puts `name`'s release binary for `triple` (None: the host).
pub fn binary(target_dir: &Path, triple: Option<&str>, name: &str, os: &str) -> PathBuf {
    let dir = match triple {
        Some(t) => target_dir.join(t).join("release"),
        None => target_dir.join("release"),
    };
    dir.join(if os == "windows" { format!("{name}.exe") } else { name.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(icon: bool) -> App {
        App {
            id: "notes".into(),
            name: "Notes & Co".into(),
            version: "1.2.0".into(),
            icon: icon.then(|| "i.png".into()),
        }
    }

    #[test]
    fn the_app_bundle_names_its_binary_and_hides_its_launcher() {
        let p = info_plist(&app(true), "aio.rt.notes");
        assert!(p.contains("<key>CFBundleExecutable</key>\n\t<string>notes</string>"));
        assert!(p.contains("<string>aio.rt.notes</string>") && p.contains("<string>Notes &amp; Co</string>"));
        assert!(p.contains("<key>LSUIElement</key>\n\t<true/>") && p.contains("AppIcon"));
        assert!(!info_plist(&app(false), "x.y").contains("CFBundleIconFile"));
        assert!(info_plist(&app(false), "com.example.notes").contains("<string>com.example.notes</string>"));
    }

    #[test]
    fn the_disk_image_is_compressed_named_after_the_app_and_says_how_to_install() {
        let a = dmg_args("Notes & Co", Path::new("/s/stage"), Path::new("/d/notes-1.2.0-macos-arm64.dmg"));
        assert_eq!(
            a,
            [
                "create",
                "-volname",
                "Notes & Co",
                "-fs",
                "HFS+",
                "-format",
                "UDZO",
                "-ov",
                "-srcfolder",
                "/s/stage",
                "/d/notes-1.2.0-macos-arm64.dmg"
            ]
        );
        let how = how_to_install(&app(false), "macos", Path::new("/d/notes-1.2.0-macos-arm64.dmg"));
        assert_eq!(how, "install: open notes-1.2.0-macos-arm64.dmg, then drag Notes & Co.app to Applications");
        assert!(how_to_install(&app(false), "macos", Path::new("/d/n.zip")).starts_with("install: unzip n.zip"));
        // Linux: the AppImage (aio's default artifact) is named beside the bundle.
        let how = how_to_install(&app(false), "linux", Path::new("/d/notes-1.2.0-linux-x64.tar.gz"));
        assert!(how.starts_with("install: tar -xzf notes-1.2.0-linux-x64.tar.gz") && how.contains("--appimage"));
    }

    /// Away from a Mac (no codesign, no ditto): the bundle is assembled,
    /// zipped with its binary executable and the runtime's links kept.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn a_mac_app_is_made_and_zipped_off_a_mac_its_modes_and_links_kept() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let bin = d.path().join("notes");
        std::fs::write(&bin, b"\xcf\xfa\xed\xfe fake").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
        let fw = d.path().join("rt/Electron.app/Contents/Frameworks/E.framework");
        std::fs::create_dir_all(fw.join("Versions/A")).unwrap();
        std::fs::write(fw.join("Versions/A/E"), b"lib").unwrap();
        std::os::unix::fs::symlink("Versions/A/E", fw.join("E")).unwrap();
        let staging = d.path().join("stage");
        std::fs::create_dir_all(&staging).unwrap();
        let made = mac_app(&app(false), "aio.rt.notes", &bin, Some(&d.path().join("rt")), &staging).unwrap();
        assert_eq!(made, staging.join("Notes & Co.app"));
        let out = d.path().join("n.zip");
        mac_zip(&made, &out).unwrap();
        let mut z = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
        let mode = |z: &mut zip::ZipArchive<std::fs::File>, n: &str| z.by_name(n).unwrap().unix_mode().unwrap();
        assert_eq!(mode(&mut z, "Notes & Co.app/Contents/MacOS/notes") & 0o777, 0o755);
        assert_eq!(mode(&mut z, "Notes & Co.app/Contents/Info.plist") & 0o777, 0o644, "data stays data");
        assert!(z.by_name("Notes & Co.app/Contents/PkgInfo").unwrap().last_modified().unwrap().year() >= 2026);
        let link = "Notes & Co.app/Contents/MacOS/electron/Electron.app/Contents/Frameworks/E.framework/E";
        let mut f = z.by_name(link).unwrap();
        assert!(f.is_symlink());
        let mut to = String::new();
        std::io::Read::read_to_string(&mut f, &mut to).unwrap();
        assert_eq!(to, "Versions/A/E");
    }

    #[test]
    fn a_windows_zip_holds_the_exe() {
        let d = tempfile::tempdir().unwrap();
        let (bin, out) = (d.path().join("notes.exe"), d.path().join("n.zip"));
        std::fs::write(&bin, b"MZ fake").unwrap();
        windows_zip(&app(false), &bin, None, &out).unwrap();
        let mut z = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
        assert_eq!(z.len(), 1);
        let mut f = z.by_name("notes.exe").unwrap();
        let mut got = vec![];
        std::io::Read::read_to_end(&mut f, &mut got).unwrap();
        assert_eq!(got, b"MZ fake");
        drop(f);
        // An offline build: its runtime beside the exe, as the app looks for it.
        let rt = d.path().join("rt");
        std::fs::create_dir_all(rt.join("locales")).unwrap();
        std::fs::write(rt.join("electron.exe"), b"MZ electron").unwrap();
        std::fs::write(rt.join("locales/en-US.pak"), b"us").unwrap();
        windows_zip(&app(false), &bin, Some(&rt), &out).unwrap();
        let names = |bytes: Vec<u8>| {
            let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
            (0..z.len()).map(|i| z.by_index(i).unwrap().name().unwrap().to_string()).collect::<Vec<_>>()
        };
        let all = names(std::fs::read(&out).unwrap());
        assert_eq!(all, ["notes.exe", "electron/electron.exe", "electron/locales/en-US.pak"]);
        assert_eq!(names(runtime_zip(&rt).unwrap()), ["electron/electron.exe", "electron/locales/en-US.pak"]);
        let how = how_to_install(&app(false), "windows", Path::new("/d/notes-1.2.0-windows-x64.zip"));
        assert!(how.starts_with("install: double-click notes-1.2.0-windows-x64.exe (no admin"), "{how}");
        assert_eq!(
            binary(Path::new("/t"), Some("x86_64-pc-windows-gnu"), "notes", "windows"),
            Path::new("/t/x86_64-pc-windows-gnu/release/notes.exe")
        );
    }
}

//! `ax installed` and `ax remove <app> [--data]` (aio: `am installed`,
//! `am remove`): the apps a bundle's `install.sh` (or `ax install
//! --appimage`, [`from_appimage`]) put on this machine, and
//! taking one away again: the program, its versions, menu entry and icon,
//! and its data only when asked (`--data`), never while it runs.
use std::path::{Path, PathBuf};

/// Where an install puts things (`install.sh`'s XDG dirs, aio-rt's homes).
pub struct Places {
    /// `aio-rt/install`: one dir per app, `installed.json` in it.
    pub install: PathBuf,
    /// `aio-rt/apps`: the apps' data homes.
    pub apps: PathBuf,
    pub cache: PathBuf,
    pub bin: PathBuf,
    /// `<data home>`: `applications/` and `icons/` under it.
    pub share: PathBuf,
}

impl Places {
    /// This user's, from the environment `env` describes.
    pub fn of(env: aio::paths::Env, tool: &Path, cache: PathBuf) -> Self {
        let home = env("HOME").or_else(|| env("USERPROFILE")).unwrap_or_default();
        Places {
            install: env("AIORT_INSTALL_ROOT").unwrap_or_else(|| tool.join("install")),
            apps: env("AIORT_APPS_DIR").unwrap_or_else(|| tool.join("apps")),
            cache,
            bin: env("XDG_BIN_HOME").unwrap_or_else(|| home.join(".local/bin")),
            share: env("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local/share")),
        }
    }
}

fn record(dir: &Path) -> serde_json::Value {
    std::fs::read(dir.join("installed.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// When record `r` last changed (pure): its update, else its install. An
/// app put in place by hand (a macOS .app) has a record only once it
/// updated itself, with no `installedAt`.
fn when(r: &serde_json::Value) -> String {
    match (r["updatedAt"].as_str(), r["installedAt"].as_str()) {
        (Some(at), _) => format!("updated {at}"),
        (None, Some(at)) => format!("installed {at}"),
        (None, None) => String::new(),
    }
}

/// The installed apps: id, version, when, from which bundle, running or not.
pub fn list(p: &Places, json: bool) -> String {
    let mut ids: Vec<String> = std::fs::read_dir(&p.install)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().join("installed.json").is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    ids.sort();
    // An AppImage's dir is `<id>-appimage`: shown as `<id> (AppImage)`.
    let rows = ids.iter().map(|dir| {
        let r = record(&p.install.join(dir));
        let image = dir.strip_suffix("-appimage").filter(|id| r["id"] == *id);
        let id = image.unwrap_or(dir);
        let running = aio::paths::running(&p.apps.join(id).join("data"));
        (id.to_string(), image.is_some(), r, running)
    });
    if json {
        let rows = rows.map(|(id, image, r, running)| {
            let kind = if image { "appimage" } else { "bundle" };
            serde_json::json!({ "id": id, "kind": kind, "version": r["version"], "installedAt": r["installedAt"], "updatedAt": r["updatedAt"], "source": r["source"], "running": running })
        });
        return serde_json::Value::Array(rows.collect()).to_string();
    }
    let lines: Vec<String> = rows
        .map(|(id, image, r, running)| {
            let id = if image { format!("{id} (AppImage)") } else { id };
            format!(
                "{id:<20} {:<12} {}{}",
                r["version"].as_str().unwrap_or("?"),
                when(&r),
                if running { "  (running)" } else { "" }
            )
        })
        .collect();
    if lines.is_empty() { "no apps installed from a bundle".into() } else { lines.join("\n") }
}

/// Take app `id` away: what `uninstall.sh` takes, and with `data` its data
/// home and cache too. Never while it runs, never an app not installed.
pub fn remove(p: &Places, id: &str, data: bool) -> Result<String, String> {
    let home = p.apps.join(id);
    if aio::paths::running(&home.join("data")) {
        return Err(format!("{id} is running: ax stop {id} first"));
    }
    let installed = p.install.join(id);
    let image = p.install.join(format!("{id}-appimage"));
    if !installed.join("installed.json").is_file() && !image.join("installed.json").is_file() {
        return Err(format!("{id} is not installed from a bundle (no {})", installed.join("installed.json").display()));
    }
    let gone = |f: &Path| match std::fs::remove_file(f) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("{}: {e}", f.display())),
        _ => Ok(()),
    };
    let tree = |d: &Path| match std::fs::remove_dir_all(d) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("{}: {e}", d.display())),
        _ => Ok(()),
    };
    // The bin link only when it is the install's (a user's own file stays).
    let link = p.bin.join(id);
    if std::fs::read_link(&link).is_ok_and(|t| t.starts_with(&installed)) {
        gone(&link)?;
    }
    gone(&p.share.join("applications").join(format!("{id}.desktop")))?;
    gone(&p.share.join("icons/hicolor/256x256/apps").join(format!("{id}.png")))?;
    tree(&installed)?;
    tree(&image)?;
    if !data {
        return Ok(format!(
            "{id} is removed. Its data is kept in {} (ax remove {id} --data removes it)",
            home.display()
        ));
    }
    tree(&home)?;
    tree(&p.cache.join(id))?;
    Ok(format!("{id} and its data are removed"))
}

/// `ax install <app>` (aio: `am installed --reinstall`): the Linux bundle
/// just built, `bundle` (`<dir>/` inside a `.tar.gz`), unpacked aside and
/// its own `install.sh` run, as a user would; nothing is left behind. `env`
/// is what the script sees changed (a test's `HOME`).
#[cfg(unix)]
pub fn from_bundle(bundle: &Path, env: &[(&str, &std::ffi::OsStr)]) -> Result<String, String> {
    let name = bundle.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let dir =
        name.strip_suffix(".tar.gz").ok_or_else(|| format!("{}: not a Linux bundle (.tar.gz)", bundle.display()))?;
    let aside = aside_dir()?;
    let run = |cmd: &mut std::process::Command, what: &str| {
        let out = cmd.envs(env.iter().copied()).output().map_err(|e| format!("{what}: {e}"))?;
        match out.status.success() {
            true => Ok(String::from_utf8_lossy(&out.stdout).trim().to_string()),
            false => Err(format!("{what} failed: {}", String::from_utf8_lossy(&out.stderr).trim())),
        }
    };
    let done = run(std::process::Command::new("tar").arg("-xzf").arg(bundle).arg("-C").arg(&aside), "unpacking")
        .and_then(|_| run(std::process::Command::new("sh").arg(aside.join(dir).join("install.sh")), "install.sh"));
    _ = std::fs::remove_dir_all(&aside);
    done
}

/// The menu entry of an AppImage installed at `file` (aio's run.sh): its
/// `Exec` sets a private `TMPDIR`, `unpack` (made 0700), before the
/// AppImage runs, as the AppImage runtime unpacks into `TMPDIR` before
/// anything of the app can, and `/tmp` is shared. A path the entry can't
/// spell plainly (a quote, `$`, `%`, a backslash) is refused.
#[cfg(unix)]
pub fn appimage_entry(app: &crate::build::App, file: &Path, unpack: &Path) -> Result<String, String> {
    let plain = |p: &Path| match p.to_str() {
        Some(s) if !s.contains(['\'', '"', '\\', '$', '`', '%', '\n']) => Ok(s.to_string()),
        _ => Err(format!("{}: a menu entry can't name this path (a quote, $, % or \\ in it)", p.display())),
    };
    let (file, unpack) = (plain(file)?, plain(unpack)?);
    let script =
        format!("D=\"{unpack}\"; mkdir -p \"$D\"; chmod 700 \"$D\" 2>/dev/null; TMPDIR=\"$D\" exec \"{file}\" \"$@\"");
    // As the Desktop Entry spec quotes an argument (`"`, `` ` ``, `$`, `\`
    // escaped), then escapes the value's backslashes (aio's single quotes
    // are outside the spec: desktop-file-validate refuses them).
    let quoted: String =
        script.chars().flat_map(|c| ['\\'].into_iter().filter(move |_| "\"`$\\".contains(c)).chain([c])).collect();
    let exec = format!("Exec=sh -c \"{}\" _ %U", quoted.replace('\\', "\\\\"));
    Ok(crate::build::desktop_entry(app, "\0").replace("Exec=\"\0\" %U", &exec))
}

/// `ax install <app> --appimage` (aio: `am run`'s install): the AppImage
/// `image` just built, copied whole to `aio-rt/install/<id>-appimage/`
/// (where its updates go: an update replaces that file), with its icon and
/// its menu entry ([`appimage_entry`]: unpacked in `<cache>/<id>/tmp`).
#[cfg(unix)]
pub fn from_appimage(p: &Places, app: &crate::build::App, image: &Path) -> Result<String, String> {
    use std::os::unix::fs::PermissionsExt;
    fn e(f: &Path) -> impl FnOnce(std::io::Error) -> String + '_ {
        move |e| format!("{}: {e}", f.display())
    }
    let dir = p.install.join(format!("{}-appimage", app.id));
    let file = dir.join(format!("{}.AppImage", app.id));
    let entry = appimage_entry(app, &file, &p.cache.join(&app.id).join("tmp"))?;
    std::fs::create_dir_all(&dir).map_err(e(&dir))?;
    let part = file.with_extension("AppImage.part");
    std::fs::copy(image, &part).map_err(e(&part))?;
    std::fs::set_permissions(&part, std::fs::Permissions::from_mode(0o755)).map_err(e(&part))?;
    std::fs::rename(&part, &file).map_err(e(&file))?;
    let at = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
    let source = image.file_name().unwrap_or_default().to_string_lossy();
    let rec =
        serde_json::json!({ "id": app.id, "version": app.version, "source": source, "installedAt": at.to_string() });
    std::fs::write(dir.join("installed.json"), format!("{rec}\n")).map_err(e(&dir))?;
    if let Some(icon) = &app.icon {
        let icons = p.share.join("icons/hicolor/256x256/apps");
        std::fs::create_dir_all(&icons).map_err(e(&icons))?;
        std::fs::copy(icon, icons.join(format!("{}.png", app.id))).map_err(e(icon))?;
    }
    let apps = p.share.join("applications");
    std::fs::create_dir_all(&apps).map_err(e(&apps))?;
    let desktop = apps.join(format!("{}.desktop", app.id));
    std::fs::write(&desktop, entry).map_err(e(&desktop))?;
    // Some desktops show a new entry only after this (or the next login).
    _ = std::process::Command::new("update-desktop-database").arg(&apps).output();
    Ok(format!("{} {} is installed: {}, and in the app menu", app.name, app.version, file.display()))
}

/// A new private dir to unpack into.
#[cfg(unix)]
fn aside_dir() -> Result<PathBuf, String> {
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let d = std::env::temp_dir().join(format!("aio-install-{}-{at}", std::process::id()));
    std::fs::create_dir_all(&d).map_err(|e| format!("{}: {e}", d.display()))?;
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_apps_data_follows_aiort_apps_dir() {
        let tool = Path::new("/u/share/aio-rt");
        let none = Places::of(&|_| None, tool, PathBuf::from("/c"));
        assert_eq!(none.apps, Path::new("/u/share/aio-rt/apps"));
        let set = Places::of(&|k| (k == "AIORT_APPS_DIR").then(|| "/srv/apps".into()), tool, PathBuf::from("/c"));
        assert_eq!(set.apps, Path::new("/srv/apps"), "ax remove --data deletes the data where it is");
        assert_eq!(none.install, Path::new("/u/share/aio-rt/install"));
        let root = Places::of(&|k| (k == "AIORT_INSTALL_ROOT").then(|| "/opt/apps".into()), tool, PathBuf::from("/c"));
        assert_eq!(root.install, Path::new("/opt/apps"), "ax installed and remove look there");
    }

    fn places(root: &Path) -> Places {
        Places {
            install: root.join("aio-rt/install"),
            apps: root.join("aio-rt/apps"),
            cache: root.join("cache"),
            bin: root.join("bin"),
            share: root.join("share"),
        }
    }

    /// What `install.sh` leaves for `id`.
    fn install(p: &Places, id: &str) {
        let v = p.install.join(id).join("versions/1.0.0");
        std::fs::create_dir_all(&v).unwrap();
        std::fs::write(v.join(id), "bin").unwrap();
        let rec =
            format!(r#"{{"id":"{id}","version":"1.0.0","source":"{id}.tar.gz","installedAt":"2026-10-28T08:00:00Z"}}"#);
        std::fs::write(p.install.join(id).join("installed.json"), rec).unwrap();
        for d in [p.bin.clone(), p.share.join("applications"), p.apps.join(id).join("data"), p.cache.join(id)] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(p.share.join("applications").join(format!("{id}.desktop")), "").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(v.join(id), p.bin.join(id)).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_bundle_built_now_installs_in_one_step_and_leaves_nothing_aside() {
        let t = tempfile::tempdir().unwrap();
        let bin = t.path().join("notes-built");
        std::fs::write(&bin, "#!/bin/sh\necho hi\n").unwrap();
        let app = crate::build::App { id: "notes".into(), name: "Notes".into(), version: "1.2.0".into(), icon: None };
        let name = crate::build::bundle_name(&app, crate::platform::host().unwrap());
        let staged = crate::build::stage(&app, &name, &bin, &t.path().join("stage"), true).unwrap();
        let bundle = t.path().join(format!("{name}.tar.gz"));
        let dir = staged.file_name().unwrap();
        let mut tar = std::process::Command::new("tar");
        tar.arg("-czf").arg(&bundle).arg("-C").arg(t.path().join("stage")).arg(dir);
        assert!(tar.status().unwrap().success());
        let (home, share, binh) = (t.path().join("home"), t.path().join("share"), t.path().join("bin"));
        let env =
            [("HOME", home.as_os_str()), ("XDG_DATA_HOME", share.as_os_str()), ("XDG_BIN_HOME", binh.as_os_str())];
        let said = from_bundle(&bundle, &env).unwrap();
        assert!(said.contains("Notes 1.2.0 is installed"), "{said}");
        assert_eq!(std::process::Command::new(binh.join("notes")).output().unwrap().stdout, b"hi\n");
        let p = Places {
            install: share.join("aio-rt/install"),
            apps: share.join("aio-rt/apps"),
            cache: t.path().join("c"),
            bin: binh.clone(),
            share: share.clone(),
        };
        assert!(list(&p, false).starts_with("notes"), "{}", list(&p, false));
        let left = std::fs::read_dir(std::env::temp_dir()).unwrap().flatten();
        let mine = format!("aio-install-{}-", std::process::id());
        assert!(!left.into_iter().any(|e| e.file_name().to_string_lossy().starts_with(&mine)), "nothing left aside");
        assert!(from_bundle(&t.path().join("notes.zip"), &env).unwrap_err().contains("not a Linux bundle"));
    }

    #[cfg(unix)]
    #[test]
    fn an_appimages_menu_entry_unpacks_it_in_a_private_tmpdir_as_aios_did() {
        let app = crate::build::App { id: "notes".into(), name: "Notes".into(), version: "1.2.0".into(), icon: None };
        let e = appimage_entry(&app, Path::new("/h/i/notes.AppImage"), Path::new("/h/c/notes/tmp")).unwrap();
        let exec = r#"Exec=sh -c "D=\\"/h/c/notes/tmp\\"; mkdir -p \\"\\$D\\"; chmod 700 \\"\\$D\\" 2>/dev/null; TMPDIR=\\"\\$D\\" exec \\"/h/i/notes.AppImage\\" \\"\\$@\\"" _ %U"#;
        assert!(e.lines().any(|l| l == exec), "{e}");
        assert!(e.contains("StartupWMClass=notes\n") && e.starts_with("[Desktop Entry]\n"), "{e}");
        for bad in ["/h/it's", "/h/$x", "/h/50%", "/h/a\\b", "/h/\"q\""] {
            assert!(appimage_entry(&app, Path::new(bad), Path::new("/t")).is_err(), "{bad}");
            assert!(appimage_entry(&app, Path::new("/f"), Path::new(bad)).is_err(), "{bad}");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_appimage_installs_in_the_menu_runs_in_its_tmpdir_and_is_removed() {
        let t = tempfile::tempdir().unwrap();
        let p = places(t.path());
        let icon = t.path().join("icon.png");
        std::fs::write(&icon, b"png").unwrap();
        let app =
            crate::build::App { id: "notes".into(), name: "Notes".into(), version: "1.2.0".into(), icon: Some(icon) };
        // The AppImage: here a script saying where it would unpack, and its arguments.
        let image = t.path().join("notes-1.2.0-x86_64.AppImage");
        std::fs::write(&image, "#!/bin/sh\necho \"$TMPDIR|$*\"\n").unwrap();
        let said = from_appimage(&p, &app, &image).unwrap();
        let file = p.install.join("notes-appimage/notes.AppImage");
        assert!(said.contains("Notes 1.2.0 is installed") && said.contains(&file.display().to_string()), "{said}");
        let rec = record(&p.install.join("notes-appimage"));
        assert_eq!((rec["id"].as_str(), rec["source"].as_str()), (Some("notes"), Some("notes-1.2.0-x86_64.AppImage")));
        assert!(p.share.join("icons/hicolor/256x256/apps/notes.png").is_file());
        // The menu's Exec, run as a launcher does (%U: the files handed to it).
        let entry = std::fs::read_to_string(p.share.join("applications/notes.desktop")).unwrap();
        // Valid to the spec, where its checker is here.
        let desktop = p.share.join("applications/notes.desktop");
        if let Ok(o) = std::process::Command::new("desktop-file-validate").arg(&desktop).output() {
            assert!(o.status.success() && o.stdout.is_empty(), "{}", String::from_utf8_lossy(&o.stdout));
        }
        // The value's escapes undone (`\\\\` is `\\`), then quoted as the spec's (and sh's) double quotes are.
        let exec = entry
            .lines()
            .find_map(|l| l.strip_prefix("Exec="))
            .unwrap()
            .replace("\\\\", "\\")
            .replace("%U", "'a b.txt'");
        let out = std::process::Command::new("sh").arg("-c").arg(&exec).env("TMPDIR", "/tmp").output().unwrap();
        let tmp = p.cache.join("notes/tmp");
        assert_eq!(String::from_utf8_lossy(&out.stdout), format!("{}|a b.txt\n", tmp.display()));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&tmp).unwrap().permissions().mode() & 0o777, 0o700, "private");
        // Listed by its id, and removed with its entry and icon.
        assert!(list(&p, false).starts_with("notes (AppImage)     1.2.0"), "{}", list(&p, false));
        let rows: serde_json::Value = serde_json::from_str(&list(&p, true)).unwrap();
        assert_eq!((rows[0]["id"].as_str(), rows[0]["kind"].as_str()), (Some("notes"), Some("appimage")));
        assert!(remove(&p, "notes", false).unwrap().contains("is removed"));
        assert!(!file.exists() && !p.share.join("applications/notes.desktop").exists());
        assert!(!p.share.join("icons/hicolor/256x256/apps/notes.png").exists());
    }

    #[test]
    fn installed_apps_are_listed_and_removed_keeping_data_unless_asked() {
        let t = tempfile::tempdir().unwrap();
        let p = places(t.path());
        assert_eq!(list(&p, false), "no apps installed from a bundle");
        install(&p, "notes");
        install(&p, "mail");
        let out = list(&p, false);
        assert!(out.lines().next().unwrap().starts_with("mail ") && out.contains("1.0.0"), "{out}");
        assert!(out.contains("installed 2026-10-28T08:00:00Z") && !out.contains('?'), "{out}");
        // A macOS .app put in place by hand: the updater's record alone.
        let mac = serde_json::json!({ "id": "x", "version": "0.2.1", "updatedAt": "2026-10-31T09:00:00Z", "updatedFrom": "0.1.1" });
        assert_eq!(when(&mac), "updated 2026-10-31T09:00:00Z");
        let both = serde_json::json!({ "installedAt": "2026-10-28T08:00:00Z", "updatedAt": "2026-10-31T09:00:00Z" });
        assert_eq!(when(&both), "updated 2026-10-31T09:00:00Z", "the last change");
        assert_eq!(when(&serde_json::json!({})), "");
        let rows: serde_json::Value = serde_json::from_str(&list(&p, true)).unwrap();
        assert_eq!((rows[1]["id"].as_str(), rows[1]["running"].as_bool()), (Some("notes"), Some(false)));
        // Running: refused, nothing touched.
        {
            let _run = aio::paths::AppDirs::at(&p.apps.join("notes"), "notes").unwrap();
            assert!(remove(&p, "notes", true).unwrap_err().contains("is running"));
            assert!(p.install.join("notes").exists());
        }
        let said = remove(&p, "notes", false).unwrap();
        assert!(said.contains("Its data is kept"), "{said}");
        assert!(!p.install.join("notes").exists() && !p.share.join("applications/notes.desktop").exists());
        assert!(std::fs::symlink_metadata(p.bin.join("notes")).is_err());
        assert!(p.apps.join("notes/data").exists(), "the data stays");
        assert!(remove(&p, "notes", false).unwrap_err().contains("not installed"));
        assert_eq!(remove(&p, "mail", true), Ok("mail and its data are removed".into()));
        assert!(!p.apps.join("mail").exists() && !p.cache.join("mail").exists());
        assert!(p.apps.join("notes").exists(), "only that app's");
    }
}

//! `ax upgrade` and `ax uninstall` (aio's `am upgrade`, `am uninstall`): ax
//! itself from an aio-rs release (its install script, checked against the
//! release's `SHA256SUMS`, checks each archive's SHA-256 before anything
//! runs) or from a checkout; or gone, the apps and their data untouched.
use std::path::{Path, PathBuf};
use std::process::Command;

/// Where the installers put ax: cargo's bin folder.
pub fn bin() -> PathBuf {
    std::env::var_os("CARGO_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|h| h.join(".cargo")))
        .unwrap_or_default()
        .join("bin")
}

fn exe_in(bin: &Path, name: &str) -> PathBuf {
    bin.join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

/// Whether `sums` (`SHA256SUMS`) lists `file` with SHA-256 `sha` (pure).
pub fn listed(sums: &str, file: &str, sha: &str) -> bool {
    sums.lines().any(|l| l.split_once("  ").is_some_and(|(s, f)| s.eq_ignore_ascii_case(sha) && f.trim() == file))
}

/// `ax upgrade [version] [--aio dir]`: ax from release `to` (`latest`, the
/// default: the newest), or built from the aio-rs checkout `checkout`.
pub fn upgrade(to: Option<&str>, checkout: Option<&Path>) -> Result<String, String> {
    if let Some(dir) = checkout {
        let cli = dir.join("crates/aio-cli");
        if !cli.join("Cargo.toml").is_file() {
            return Err(format!("--aio {}: no aio-rs checkout there (crates/aio-cli)", dir.display()));
        }
        let built = Command::new("cargo").args(["install", "--locked", "--path"]).arg(&cli).status();
        return match built {
            Ok(s) if s.success() => {
                Ok(format!("ax built from {} (a checkout's; plain `ax upgrade` returns to a release)", dir.display()))
            }
            _ => Err(format!("cargo install from {} failed (cargo said why, above)", dir.display())),
        };
    }
    let tag = match to.unwrap_or("latest") {
        "latest" => crate::create::newest(&crate::create::tags()?).ok_or("aio-rs has no release yet")?,
        v => match crate::create::Aio::version(v, || Err("unused".into()))? {
            crate::create::Aio::Tag(t) => t,
            _ => {
                return Err(format!(
                    "ax upgrade {v}: a release (0.3.0) or latest; a branch builds from source: \
                     cargo install --locked --git {} --branch {v} aio-rs-cli",
                    crate::create::REPO
                ));
            }
        },
    };
    let version = tag.trim_start_matches('v');
    if to.is_none() && version == env!("CARGO_PKG_VERSION") {
        return Ok(format!("ax {version} is aio-rs's newest release (ax upgrade {version} installs it again)"));
    }
    let url = crate::release::aio_release_url(version);
    let script = if cfg!(windows) { "install.ps1" } else { "install.sh" };
    let tmp = std::env::temp_dir().join(format!("ax-upgrade-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let r = install(&url, script, &tmp);
    _ = std::fs::remove_dir_all(&tmp);
    r.map(|()| format!("ax {version} installed in {}", bin().display()))
}

/// Get `script` of the release at `url` into `tmp`, check it against the
/// release's `SHA256SUMS`, and run it.
fn install(url: &str, script: &str, tmp: &Path) -> Result<(), String> {
    let get = |f: &str| {
        aio_shell::runtime::download(&format!("{url}/{f}"), &tmp.join(f), &mut |_, _| true)
            .map_err(|e| format!("{url}/{f}: {e}"))
    };
    get("SHA256SUMS")?;
    let sha = get(script)?;
    let sums = std::fs::read_to_string(tmp.join("SHA256SUMS")).map_err(|e| format!("SHA256SUMS: {e}"))?;
    if !listed(&sums, script, &sha) {
        return Err(format!("{url}/{script}: SHA-256 {sha} is not the release's (SHA256SUMS): not run"));
    }
    // Windows keeps a running program's file: it moves aside for the new one.
    let (exe, old) = (exe_in(&bin(), "ax"), exe_in(&bin(), "ax.old"));
    if cfg!(windows) && exe.is_file() {
        _ = std::fs::remove_file(&old);
        std::fs::rename(&exe, &old).map_err(|e| format!("{}: {e}", exe.display()))?;
    }
    let mut run = match cfg!(windows) {
        true => {
            let mut c = Command::new("powershell");
            c.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]);
            c
        }
        false => Command::new("sh"),
    };
    let ok = run.arg(tmp.join(script)).env_remove("AIO_DOWNLOAD").status().is_ok_and(|s| s.success());
    if !ok && cfg!(windows) && !exe.exists() {
        _ = std::fs::rename(&old, &exe);
    }
    match ok {
        true => Ok(()),
        false => Err(format!("{script} of {url} failed (it said why, above); ax is as it was")),
    }
}

/// `ax uninstall [--tools]`: ax out of cargo's bin folder `bin` (aio's `am
/// uninstall`); `--tools`: also what ax fetched and built: `tools/` in the
/// tool home `home`, ax's shared build folders (`target/` in aio's cache
/// home `cache`, unless the person named their own with `AIORT_TARGET_DIR`
/// or `CARGO_TARGET_DIR`) and ax 0.2's (`home/target`). Apps, their data
/// and installs stay.
pub fn uninstall(tools: bool, home: &Path, cache: &Path, bin: &Path) -> Result<String, String> {
    let exe = exe_in(bin, "ax");
    if !exe.is_file() {
        return Err(format!("no ax in {}: one built elsewhere goes with its folder", bin.display()));
    }
    _ = std::fs::remove_file(exe_in(bin, "ax.old"));
    let running = std::env::current_exe().ok().and_then(|e| e.canonicalize().ok()) == exe.canonicalize().ok();
    if cfg!(windows) && running {
        // A running program's file goes once it ends.
        let mut c = Command::new("cmd");
        c.arg("/C").arg(format!("ping -n 3 127.0.0.1 >NUL & del /F /Q \"{}\"", exe.display()));
        #[cfg(windows)]
        std::os::windows::process::CommandExt::creation_flags(&mut c, 0x0800_0000);
        c.spawn().map_err(|e| format!("cmd: {e}"))?;
    } else {
        std::fs::remove_file(&exe).map_err(|e| format!("{}: {e}", exe.display()))?;
    }
    let mut gone = vec![exe.display().to_string()];
    if tools {
        // A CARGO_TARGET_DIR the person set is AIORT_TARGET_DIR by now (`dev::keep_persons_target`).
        let own_target = std::env::var_os(crate::dev::TARGET_DIR).is_none_or(|v| v.is_empty());
        let dirs = [Some(home.join("tools")), own_target.then(|| cache.join("target")), Some(legacy_target(home))];
        for d in dirs.into_iter().flatten() {
            if d.is_dir() {
                std::fs::remove_dir_all(&d).map_err(|e| format!("{}: {e}", d.display()))?;
                gone.push(d.display().to_string());
            }
        }
    }
    let keep = if tools { "" } else { "; ax uninstall --tools also takes what it fetched and built" };
    Ok(format!(
        "removed {}\nyour apps, their data and installs stay (ax remove <app> takes one){keep}",
        gone.join(", ")
    ))
}

/// ax 0.2's shared build folder, in the tool home `home` (0.3 builds in
/// aio's cache home): `ax prune --yes` and `ax uninstall --tools` take it.
pub fn legacy_target(home: &Path) -> PathBuf {
    home.join("target")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_runs_only_as_the_release_lists_it() {
        let sums = "abc123  install.sh\nDEF456  install.ps1\n";
        assert!(listed(sums, "install.sh", "abc123") && listed(sums, "install.ps1", "def456"));
        assert!(!listed(sums, "install.sh", "def456"), "another file's sum");
        assert!(!listed(sums, "install.sh", "abc12"), "a part of it");
        assert!(!listed("", "install.sh", "abc123"));
    }

    #[test]
    fn uninstall_takes_ax_and_with_tools_what_it_fetched_never_the_apps() {
        let d = tempfile::tempdir().unwrap();
        let (bin, home, cache) = (d.path().join("bin"), d.path().join("home"), d.path().join("cache"));
        assert!(uninstall(false, &home, &cache, &bin).unwrap_err().contains("no ax in"));
        let shared = cache.join("target/1.98.0/debug");
        for p in [bin.clone(), home.join("tools/dx"), home.join("target"), shared.clone(), home.join("apps/notes/data")]
        {
            std::fs::create_dir_all(p).unwrap();
        }
        std::fs::write(exe_in(&bin, "ax"), "").unwrap();
        let said = uninstall(true, &home, &cache, &bin).unwrap();
        assert!(!exe_in(&bin, "ax").exists() && !home.join("tools").exists(), "{said}");
        assert!(!home.join("target").exists(), "ax 0.2's build folder: {said}");
        let own = std::env::var_os(crate::dev::TARGET_DIR).is_none_or(|v| v.is_empty());
        assert_eq!(shared.exists(), !own, "a named AIORT_TARGET_DIR is not ax's to take: {said}");
        assert!(home.join("apps/notes/data").is_dir() && said.contains("your apps, their data"), "{said}");
    }
}

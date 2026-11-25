//! The OS's record of the app's links (`[app] schemes`) kept up with the
//! installed app. An install registers them; an update swaps the program
//! in place, so a scheme it adds (or drops) was registered only by the next
//! install. Each start of an installed app (Linux, Windows) sets what
//! differs, in the background; macOS and Android read theirs from the bundle.

/// Set the installed app's links again where they differ (Linux: its menu
/// entry; Windows: its `HKCU\Software\Classes` handlers), off this thread.
/// A dev build or an app not installed registers nothing.
pub(crate) fn on_start(cfg: &aio::config::Config) {
    if cfg!(feature = "dev") || !cfg!(any(target_os = "linux", windows)) || !crate::update::installed(&cfg.app.id) {
        return;
    }
    let (id, name, schemes) = (cfg.app.id.clone(), cfg.app.name.clone(), cfg.app.schemes.clone());
    _ = std::thread::Builder::new().name("aio-schemes".into()).spawn(move || set(&id, &name, &schemes));
}

#[cfg(target_os = "linux")]
fn set(id: &str, _: &str, schemes: &[String]) {
    let apps = aio::paths::xdg_data_home().join("applications");
    let file = apps.join(format!("{id}.desktop"));
    // No menu entry: installed without one (a CLI), nothing to keep up.
    let Ok(entry) = std::fs::read_to_string(&file) else { return };
    let Some(fixed) = with_schemes(&entry, schemes) else { return };
    if let Err(e) = aio::paths::write_atomic(&file, fixed.as_bytes(), false) {
        return eprintln!("[aio] {}: its links can't be registered: {e}", file.display());
    }
    let had = handled(&entry);
    for s in schemes.iter().filter(|s| !had.contains(&s.as_str())) {
        _ = std::process::Command::new("xdg-mime")
            .args(["default", &format!("{id}.desktop"), &format!("x-scheme-handler/{s}")])
            .output();
    }
    _ = std::process::Command::new("update-desktop-database").arg(&apps).output();
}

#[cfg(windows)]
fn set(id: &str, name: &str, schemes: &[String]) {
    crate::winstall::reregister(id, name, schemes);
}

#[cfg(not(any(target_os = "linux", windows)))]
fn set(_: &str, _: &str, _: &[String]) {}

/// The schemes a menu entry's `MimeType` hands to it (pure).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn handled(entry: &str) -> Vec<&str> {
    let mime = entry.lines().find_map(|l| l.strip_prefix("MimeType=")).unwrap_or_default();
    mime.split(';').filter_map(|m| m.strip_prefix("x-scheme-handler/")).collect()
}

/// Menu entry `entry` with its `MimeType` handing it exactly `schemes`, its
/// other types kept (pure); `None`: it already does.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn with_schemes(entry: &str, schemes: &[String]) -> Option<String> {
    let mut had = handled(entry);
    let mut want: Vec<&str> = schemes.iter().map(String::as_str).collect();
    had.sort_unstable();
    want.sort_unstable();
    if had == want {
        return None;
    }
    let mime = entry.lines().find_map(|l| l.strip_prefix("MimeType=")).unwrap_or_default();
    let others = mime.split(';').filter(|m| !m.is_empty() && !m.starts_with("x-scheme-handler/"));
    let types: String =
        others.map(|m| format!("{m};")).chain(schemes.iter().map(|s| format!("x-scheme-handler/{s};"))).collect();
    let line = (!types.is_empty()).then(|| format!("MimeType={types}"));
    let mut out = String::new();
    for l in entry.lines().filter(|l| !l.starts_with("MimeType=")) {
        out += l;
        out.push('\n');
        // In the entry's group, after its command.
        if l.starts_with("Exec=")
            && let Some(m) = &line
        {
            out += m;
            out.push('\n');
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENTRY: &str = "[Desktop Entry]\nType=Application\nName=Notes\nExec=\"/h/.local/bin/notes\" %U\nIcon=notes\n\
                         MimeType=x-scheme-handler/notes;\nStartupWMClass=notes\nTerminal=false\nCategories=Utility;\n";

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_menu_entry_takes_the_schemes_an_update_added_or_dropped() {
        assert_eq!(handled(ENTRY), ["notes"]);
        assert_eq!(with_schemes(ENTRY, &s(&["notes"])), None, "as installed: left alone");
        let added = with_schemes(ENTRY, &s(&["notes", "nt+x"])).unwrap();
        assert_eq!(
            added,
            ENTRY.replace(
                "Icon=notes\nMimeType=x-scheme-handler/notes;\n",
                "MimeType=x-scheme-handler/notes;x-scheme-handler/nt+x;\nIcon=notes\n"
            )
        );
        assert_eq!(with_schemes(&added, &s(&["nt+x", "notes"])), None, "the same schemes in another order");
        let dropped = with_schemes(ENTRY, &[]).unwrap();
        assert!(!dropped.contains("MimeType") && dropped.contains("StartupWMClass=notes\n"), "{dropped}");
        // An entry made before the app had schemes gets the line; other types stay.
        let old = ENTRY.replace("MimeType=x-scheme-handler/notes;\n", "MimeType=text/plain;\n");
        let now = with_schemes(&old, &s(&["notes"])).unwrap();
        assert!(now.contains("\nMimeType=text/plain;x-scheme-handler/notes;\n"), "{now}");
    }
}

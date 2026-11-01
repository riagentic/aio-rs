//! Windows: the one-click download (aio's SFX `.exe`). `ax build` ships the
//! app's own exe as `<id>-<version>-windows-<arch>.exe`; double-clicked
//! under that name it installs itself for this user (as `--install` does)
//! and starts the installed copy with the same arguments. Opened again, or
//! when the installed app has updated itself past it, it only starts the
//! installed one: an old download never puts its version over a newer
//! install. A Start menu entry the user removed is not put back by an
//! upgrade. Every decision is a pure function here, tested on any OS; only
//! `winstall` touches Windows.
use std::cmp::Ordering;

/// Is `file` (the running exe's name) the one-click download of app `id`,
/// not its installed `<id>.exe`? The name `ax build` gives it, perhaps with
/// the ` (1)` a browser adds to a second download.
pub fn is_download(file: &str, id: &str) -> bool {
    let f = file.to_lowercase();
    let Some(stem) = f.strip_suffix(".exe") else { return false };
    stem.strip_prefix(&format!("{}-", id.to_lowercase())).is_some_and(|rest| rest.contains("-windows-"))
}

/// What a double-click on the download does.
#[derive(Debug, PartialEq, Eq)]
pub enum Plan {
    /// Copy this exe into place; `shortcut`: add the Start menu entry too
    /// (a first install; an upgrade keeps what the user left).
    Install { shortcut: bool },
    /// Start the installed app as it is (this version, or a newer one).
    Open,
}

/// The plan for download `mine` when version `installed` is there (None:
/// nothing installed, or a copy that can't say). A version that can't be
/// ordered is installed over: the download is what the user asked for.
pub fn plan(mine: &str, installed: Option<&str>) -> Plan {
    match installed.map(|v| aio::update::compare(v, mine)) {
        None => Plan::Install { shortcut: true },
        Some(Some(Ordering::Greater | Ordering::Equal)) => Plan::Open,
        Some(_) => Plan::Install { shortcut: false },
    }
}

/// The version an installed exe's `--version` printed (`<id> <version>`),
/// if it is app `id`'s.
pub fn version_of(out: &str, id: &str) -> Option<String> {
    let (who, v) = out.trim().split_once(' ')?;
    (who == id && !v.trim().is_empty()).then(|| v.trim().to_string())
}

/// The Start menu shortcut's file name (no `.lnk`) of an app named `name`:
/// what Windows refuses in a file name becomes `_`, and spaces and dots at
/// either end go (aio's `shortcut_file_name`). Empty: name it by the id.
pub fn shortcut_name(name: &str, id: &str) -> String {
    let n: String = name.chars().map(|c| if c < ' ' || r#"\/:*?"<>|"#.contains(c) { '_' } else { c }).collect();
    match n.trim_matches([' ', '.']) {
        "" => id.to_string(),
        n => n.to_string(),
    }
}

/// What ends a one-click download that carries its runtime: after the
/// runtime's zip, its length (8 bytes, little-endian), then this.
const RUNTIME_MARK: &[u8] = b"\0aio-runtime-zip\0";

/// The one-click download of an offline build: `exe` with its runtime's
/// zip (`electron/…`) appended. Windows runs the exe as it is; what follows
/// it is data the installer unpacks.
pub fn with_runtime(exe: &[u8], zip: &[u8]) -> Vec<u8> {
    [exe, zip, &(zip.len() as u64).to_le_bytes(), RUNTIME_MARK].concat()
}

/// A download's exe and, if it carries one, its runtime's zip.
pub fn split_runtime(file: &[u8]) -> (&[u8], Option<&[u8]>) {
    let found = file.strip_suffix(RUNTIME_MARK).and_then(|rest| {
        let (head, len) = rest.split_at_checked(rest.len().checked_sub(8)?)?;
        let len = usize::try_from(u64::from_le_bytes(len.try_into().ok()?)).ok()?;
        let (exe, zip) = head.split_at_checked(head.len().checked_sub(len)?)?;
        Some((exe, Some(zip)))
    });
    found.unwrap_or((file, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_offline_download_carries_its_runtime_after_the_exe() {
        let file = with_runtime(b"MZ exe", b"PK zip");
        assert_eq!(split_runtime(&file), (b"MZ exe".as_slice(), Some(b"PK zip".as_slice())));
        assert_eq!(split_runtime(b"MZ exe"), (b"MZ exe".as_slice(), None), "a download with none");
        // A mark whose length doesn't fit is no runtime.
        let bad = [b"MZ".as_slice(), &99u64.to_le_bytes(), RUNTIME_MARK].concat();
        assert_eq!(split_runtime(&bad), (bad.as_slice(), None));
        assert_eq!(split_runtime(RUNTIME_MARK), (RUNTIME_MARK, None));
    }

    #[test]
    fn the_download_is_told_from_the_installed_exe() {
        assert!(is_download("notes-1.2.0-windows-x64.exe", "notes"));
        assert!(is_download("Notes-1.2.0-windows-x64 (1).EXE", "notes"), "a second download, any case");
        assert!(!is_download("notes.exe", "notes"), "the installed copy");
        assert!(!is_download("notes-1.2.0-windows-x64.zip", "notes"));
        assert!(!is_download("notes-cli.exe", "notes"), "another binary of the app");
        assert!(!is_download("other-1.2.0-windows-x64.exe", "notes"));
    }

    #[test]
    fn an_old_download_only_opens_a_newer_install_and_an_upgrade_keeps_the_users_menu() {
        assert_eq!(plan("1.2.0", None), Plan::Install { shortcut: true });
        assert_eq!(plan("1.2.0", Some("1.2.0")), Plan::Open);
        assert_eq!(plan("1.2.0", Some("v1.2")), Plan::Open, "equal versions");
        assert_eq!(plan("1.2.0", Some("1.3.0")), Plan::Open, "never a downgrade");
        assert_eq!(plan("1.2.0", Some("1.2.0-beta")), Plan::Install { shortcut: false });
        assert_eq!(plan("1.10.0", Some("1.9.0")), Plan::Install { shortcut: false });
        assert_eq!(plan("1.2.0", Some("??")), Plan::Install { shortcut: false }, "unorderable: the download wins");
    }

    #[test]
    fn the_installed_version_is_read_from_its_answer() {
        assert_eq!(version_of("notes 1.3.0\r\n", "notes").as_deref(), Some("1.3.0"));
        assert_eq!(version_of("other 1.3.0", "notes"), None);
        assert_eq!(version_of("", "notes"), None);
        assert_eq!(version_of("notes ", "notes"), None);
    }

    #[test]
    fn the_shortcut_is_named_as_windows_allows() {
        // aio's shortcut-names.json.
        for (title, file) in [
            ("My App", "My App"),
            ("  My App  ", "My App"),
            ("...hidden...", "hidden"),
            (" . My.App . ", "My.App"),
            ("a/b\\c:d*e?f\"g<h>i|j", "a_b_c_d_e_f_g_h_i_j"),
            ("Tab\there\nnewline\0nul\x1fend", "Tab_here_newline_nul_end"),
            ("It's $HOME `cmd` %PATH% & (x) ; 'q'", "It's $HOME `cmd` %PATH% & (x) ; 'q'"),
            ("Ünï — 日本語 ☃", "Ünï — 日本語 ☃"),
            ("Rocket 🚀 App", "Rocket 🚀 App"),
            ("\x7f del stays", "\x7f del stays"),
            ("?", "_"),
            ("a\u{a0}", "a\u{a0}"),
            ("CON", "CON"),
        ] {
            assert_eq!(shortcut_name(title, "notes"), file, "{title:?}");
        }
        for nameless in ["", "...", ". ."] {
            assert_eq!(shortcut_name(nameless, "notes"), "notes", "{nameless:?}");
        }
    }
}

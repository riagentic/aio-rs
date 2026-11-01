//! `ax trust`: this machine's root (`aio::tls`), and how to put it in the
//! OS and browser trust stores, so a browser opening an app here over HTTPS
//! knows who it talks to. It explains and never installs: a root in a trust
//! store is the user's call, and those commands need their elevation. aio-rs's
//! own clients need none of it (they pin the root at pairing).
use aio::serde_json::json;
use std::path::Path;

type Out = Result<String, String>;

/// The OS's name and the steps that trust `cert` there (pure: `os` is
/// `std::env::consts::OS`'s word, so every OS's text is tested anywhere).
pub fn steps(cert: &Path, os: &str) -> (&'static str, Vec<String>) {
    let c = cert.display();
    let firefox =
        "Firefox keeps its own store: Settings › Privacy & Security › Certificates › View Certificates › Import";
    match os {
        "linux" => (
            "Linux",
            vec![
                format!("sudo cp {c} /usr/local/share/ca-certificates/aio-rt-root.crt"),
                "sudo update-ca-certificates".into(),
                String::new(),
                "Firefox and Chrome keep their own store (NSS); for those:".into(),
                format!("  certutil -d sql:$HOME/.pki/nssdb -A -t \"C,,\" -n \"aio-rt local root\" -i {c}"),
                "  (certutil: apt install libnss3-tools)".into(),
            ],
        ),
        "macos" => (
            "macOS",
            vec![
                format!("sudo security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain {c}"),
                String::new(),
                firefox.into(),
            ],
        ),
        // One line each: PowerShell continues a line with a backtick, not `\`.
        "windows" => (
            "Windows",
            vec![
                "In PowerShell (no admin needed; Windows asks once to confirm):".into(),
                format!("  Import-Certificate -FilePath \"{c}\" -CertStoreLocation Cert:\\CurrentUser\\Root"),
                String::new(),
                "For every account on this PC instead, in an admin PowerShell:".into(),
                format!("  Import-Certificate -FilePath \"{c}\" -CertStoreLocation Cert:\\LocalMachine\\Root"),
                String::new(),
                "Chrome and Edge read both stores.".into(),
                firefox.into(),
            ],
        ),
        _ => ("this OS", vec![format!("Import {c} into the system's trust store.")]),
    }
}

/// `ax trust [path] [--json]` for the root in `dir` (made if missing).
pub fn run(dir: &Path, sub: Option<&str>, json: bool) -> Out {
    let cert = dir.join("root.pem");
    match sub {
        Some("path") if json => return Ok(json!({ "path": cert }).to_string()),
        Some("path") => return Ok(cert.display().to_string()),
        None | Some("show") => {}
        Some(other) => return Err(format!("ax trust has no `{other}`: ax trust, or ax trust path")),
    }
    let created = !cert.exists();
    aio::tls::root(dir).map_err(|e| format!("this machine's root can't be read or made in {}: {e}", dir.display()))?;
    let (os, steps) = steps(&cert, std::env::consts::OS);
    if json {
        let steps: Vec<&String> = steps.iter().filter(|s| !s.is_empty()).collect();
        return Ok(json!({ "path": cert, "created": created, "os": os, "steps": steps }).to_string());
    }
    let mut out = vec![
        format!("aio-rt local root{}", if created { " (made just now)" } else { "" }),
        cert.display().to_string(),
        String::new(),
        "Trusted once, every aio-rs app on this machine is known to your browser, apps not".into(),
        "written yet included. It was made on this machine; its key never leaves it.".into(),
        String::new(),
        "It is name-constrained: it can only vouch for localhost, .local and private".into(),
        "addresses, never for a public site, not even for whoever holds its key.".into(),
        String::new(),
        format!("{os}:"),
    ];
    out.extend(steps.into_iter().map(|s| if s.is_empty() { s } else { format!("  {s}") }));
    out.extend([String::new(), "To undo: remove \"aio-rt local root\" from the same store.".into()]);
    Ok(out.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_explains_and_makes_the_root_but_installs_nothing() {
        let d = tempfile::tempdir().unwrap();
        let first = run(d.path(), None, false).unwrap();
        assert!(first.contains("(made just now)") && first.contains("name-constrained"), "{first}");
        assert!(d.path().join("root.pem").exists());
        assert!(!run(d.path(), Some("show"), false).unwrap().contains("made just now"));
        // It prints commands; none of them ran (nothing but the root was made).
        let mut made: Vec<_> = std::fs::read_dir(d.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        made.sort();
        assert_eq!(made, ["root-key.pem", "root.pem"]);
    }

    #[test]
    fn path_is_the_one_line_a_script_wants() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(run(d.path(), Some("path"), false).unwrap(), d.path().join("root.pem").display().to_string());
        let j: aio::serde_json::Value = aio::serde_json::from_str(&run(d.path(), Some("path"), true).unwrap()).unwrap();
        assert_eq!(j["path"].as_str().map(Path::new), Some(d.path().join("root.pem").as_path()));
        let j: aio::serde_json::Value = aio::serde_json::from_str(&run(d.path(), None, true).unwrap()).unwrap();
        assert_eq!(j["created"], true);
        assert!(j["steps"].as_array().unwrap().iter().all(|s| !s.as_str().unwrap().is_empty()));
        assert!(run(d.path(), Some("install"), false).unwrap_err().contains("ax trust path"));
    }

    #[test]
    fn every_os_gets_its_steps_and_windows_lines_are_whole() {
        let cert = Path::new("/r/root.pem");
        assert_eq!(steps(cert, "linux").1[1], "sudo update-ca-certificates");
        assert!(steps(cert, "macos").1[0].contains("add-trusted-cert"));
        let (os, win) = steps(cert, "windows");
        assert_eq!(os, "Windows");
        for l in win.iter().filter(|l| l.contains("Import-Certificate")) {
            assert!(!l.ends_with('\\') && !l.ends_with('`') && l.contains("-CertStoreLocation"), "{l}");
        }
        assert!(win.iter().any(|l| l.contains("CurrentUser")));
        assert!(steps(cert, "freebsd").1[0].contains("/r/root.pem"));
    }
}

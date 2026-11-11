//! A macOS target built from Linux or Windows on a Mac over SSH (aio
//! `AIO_MACOS_SSH`): a macOS binary links against Apple's SDK, which runs on
//! a Mac. `AIO_MACOS_SSH=[ssh options] [user@]host` names the Mac; the app's
//! workspace and the aio-rs checkout it builds with are copied into
//! `AIO_MACOS_DIR` there (default `.cache/aio-build`, under the Mac's home,
//! kept so the next build is incremental), `ax build` runs there as it would
//! on the Mac (signed, `.dmg` by `hdiutil`), and what it made comes back to
//! `dist/`. Only `tar`, `ssh` and the Mac's Rust: no daemon, no shared disk.
//! The copy keeps the folders' places relative to each other, so a `path`
//! dependency is relative (an absolute one names this machine).
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const HOST_ENV: &str = "AIO_MACOS_SSH";
pub const DIR_ENV: &str = "AIO_MACOS_DIR";
const DIR: &str = ".cache/aio-build";

/// The Mac: ssh's options (`-i key`, `-p 2222`) and its `[user@]host`.
#[derive(Debug, PartialEq)]
pub struct Host {
    pub opts: Vec<String>,
    pub target: String,
}

/// The Mac `v` names (`[ssh options] [user@]host`), if any. Pure.
pub fn host_of(v: &str) -> Option<Host> {
    let mut words: Vec<String> = v.split_whitespace().map(String::from).collect();
    let target = words.pop()?;
    Some(Host { opts: words, target })
}

/// The Mac the app's `[build.macos] host` names, else `AIO_MACOS_SSH`, if any.
pub fn configured(pinned: Option<&str>) -> Option<Host> {
    host_of(&pinned.map(String::from).or_else(|| std::env::var(HOST_ENV).ok())?)
}

/// What a remote build copies: the folder all of it is under, and each
/// file or folder relative to it. The workspace's own files and members,
/// the aio-rs checkout's workspace and crates, any other local package; one
/// inside another only once. `exists` filters the optional files. Pure.
pub fn shipped(
    ws: &Path,
    members: &[PathBuf],
    aio: &Path,
    others: &[PathBuf],
    exists: &dyn Fn(&Path) -> bool,
) -> (PathBuf, Vec<PathBuf>) {
    let root_files = ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml", ".cargo"];
    let mut all: Vec<PathBuf> = root_files.iter().map(|f| ws.join(f)).filter(|p| exists(p)).collect();
    all.extend(members.iter().cloned());
    all.extend(root_files.iter().map(|f| aio.join(f)).filter(|p| exists(p)));
    all.push(aio.join("crates"));
    all.extend(others.iter().cloned());
    all.sort();
    all.dedup();
    let all: Vec<PathBuf> = all.iter().filter(|p| !all.iter().any(|q| q != *p && p.starts_with(q))).cloned().collect();
    let base = all
        .iter()
        .skip(1)
        .fold(all.first().and_then(|p| p.parent()).unwrap_or(Path::new("/")).to_path_buf(), |b, p| {
            b.ancestors().find(|a| p.starts_with(a)).unwrap_or(Path::new("/")).to_path_buf()
        });
    let rel = all.iter().filter_map(|p| p.strip_prefix(&base).ok().map(Path::to_path_buf)).collect();
    (base, rel)
}

/// `s` single-quoted for the Mac's shell.
fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// A path as the Mac writes it.
fn unix(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// The script the Mac runs (piped to `bash -s`: zsh, its login shell, would
/// abort on an unmatched glob): aio built from the copied checkout, then
/// `ax build <name> --targets <label>` (on `platform`, else the Mac's own)
/// into a fresh `dist/`: what it says, then [`LISTED`] and what is in
/// `dist/`. Pure.
pub fn script(
    dir: &str,
    aio: &Path,
    ws: &Path,
    name: &str,
    label: &str,
    platform: Option<&str>,
    smoke: bool,
) -> String {
    let (aio, ws) = (q(&unix(aio)), q(&unix(ws)));
    let on = platform.map_or(String::new(), |p| format!(" --platforms {}", q(p)));
    let smoke = if smoke { " --smoke" } else { "" };
    [
        "set -euo pipefail".to_string(),
        r#"[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env""#.to_string(),
        format!(r#"cd {} && root="$PWD""#, q(dir)),
        format!(r#"cd "$root"/{aio}"#),
        "cargo build --release -p aio-rs-cli >&2".to_string(),
        format!(r#"cd "$root"/{ws}"#),
        "rm -rf dist".to_string(),
        format!(r#""$root"/{aio}/target/release/ax build {} --targets {}{on}{smoke}"#, q(name), q(label)),
        format!("echo {LISTED}"),
        "ls -1A dist".to_string(),
    ]
    .join("\n")
}

/// The line between what the remote build says and its `dist/` listing.
const LISTED: &str = "--aio-dist--";

/// What a remote build said, less the lines naming its files (they are
/// said again here, where they land), and the files it made, from its
/// `dist/` listing: no manifest (this machine's keeps the record), no
/// staging folder. Pure.
pub fn made(out: &str) -> (Vec<String>, Vec<String>) {
    let (said, listing) = out.split_once(&format!("{LISTED}\n")).unwrap_or(("", out));
    let files: Vec<String> = listing
        .lines()
        .map(str::trim)
        .filter(|f| !f.is_empty() && !f.starts_with('.') && !f.ends_with(".manifest.json"))
        .map(String::from)
        .collect();
    let names_one = |l: &str| files.iter().any(|f| l.contains(&format!("/{f} (")));
    (said.lines().filter(|l| !l.trim().is_empty() && !names_one(l)).map(String::from).collect(), files)
}

pub(crate) fn ssh(h: &Host) -> Command {
    let mut c = Command::new("ssh");
    // A missing key fails at once, never waits on a password prompt.
    c.args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"]).args(&h.opts).arg(&h.target);
    c
}

/// What of `rel` (under `base`) is copied: a folder in a git work tree as
/// git sees it (tracked or new, never ignored: no `.data`, no build
/// output), else as it is.
fn files(base: &Path, rel: &Path) -> Vec<PathBuf> {
    let dir = base.join(rel);
    let listed = dir.is_dir().then(|| {
        Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(["ls-files", "-z", "--cached", "--others", "--exclude-standard"])
            .stderr(Stdio::null())
            .output()
    });
    match listed {
        Some(Ok(o)) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .split('\0')
            .filter(|f| !f.is_empty())
            .map(|f| rel.join(f))
            .filter(|f| base.join(f).is_file())
            .collect(),
        _ => vec![rel.to_path_buf()],
    }
}

/// `a | b` (`input`: what `a` reads), both checked.
fn pipe(mut a: Command, input: Option<String>, mut b: Command, what: &str) -> Result<(), String> {
    let stdin = if input.is_some() { Stdio::piped() } else { Stdio::null() };
    let mut first = a.stdin(stdin).stdout(Stdio::piped()).spawn().map_err(|e| format!("{what}: {e}"))?;
    let out = first.stdout.take().ok_or("no pipe")?;
    if let (Some(text), Some(mut w)) = (input, first.stdin.take()) {
        std::thread::spawn(move || std::io::Write::write_all(&mut w, text.as_bytes()));
    }
    let second = b.stdin(out).status().map_err(|e| format!("{what}: {e}"))?;
    let first = first.wait().map_err(|e| format!("{what}: {e}"))?;
    if !(first.success() && second.success()) {
        return Err(format!("{what} failed"));
    }
    Ok(())
}

/// The local aio-rs package of workspace `ws` (a `[patch]` to a checkout):
/// its folder, from the resolved metadata (a package with no source).
fn patched_aio(ws: &Path) -> Option<PathBuf> {
    let out = Command::new("cargo").args(["metadata", "--format-version", "1"]).current_dir(ws).output().ok()?;
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    meta["packages"].as_array()?.iter().find_map(|p| {
        (p["name"] == "aio-rs" && p["source"].is_null())
            .then(|| Path::new(p["manifest_path"].as_str()?).parent().map(Path::to_path_buf))
            .flatten()
    })
}

/// Build target `label` of app `name` (`meta`: its workspace) on the Mac
/// `h`, for `platform` (None: the Mac's own); how to install them, and the
/// files it made, in `dist`; `smoke`: each then smoke-tested there.
pub fn build(
    h: &Host,
    meta: &serde_json::Value,
    name: &str,
    label: &str,
    platform: Option<&str>,
    dist: &Path,
    smoke: bool,
) -> Result<(Vec<String>, Vec<PathBuf>), String> {
    // Real paths: a `path = "../aio/crates/aio"` dependency is said as written.
    let real = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let ws = real(Path::new(meta["workspace_root"].as_str().ok_or("cargo metadata: no workspace_root")?));
    let packages = meta["packages"].as_array().ok_or("cargo metadata: no packages")?;
    // The members, and the local packages they use (`--no-deps` lists only
    // the members, each with its dependencies' paths).
    let members =
        packages.iter().filter_map(|p| Some((p["name"].as_str()?, Path::new(p["manifest_path"].as_str()?).parent()?)));
    let deps = packages.iter().flat_map(|p| p["dependencies"].as_array().into_iter().flatten());
    let deps = deps.filter_map(|d| Some((d["name"].as_str()?, Path::new(d["path"].as_str()?))));
    let local: Vec<(&str, PathBuf)> = members.chain(deps).map(|(n, d)| (n, real(d))).collect();
    // A `[patch]` to a checkout is no dependency path: the full metadata names it.
    let aio = local
        .iter()
        .find(|(n, _)| *n == "aio-rs")
        .map(|(_, d)| d.clone())
        .or_else(|| patched_aio(&ws).map(|d| real(&d)))
        .and_then(|d| d.ancestors().nth(2).map(Path::to_path_buf))
        .filter(|r| r.join("crates/aio-cli/Cargo.toml").is_file())
        .ok_or("the app builds with no aio-rs checkout (crates/aio-cli beside its aio-rs) to build ax from on the Mac")?
        .to_path_buf();
    let (members, others): (Vec<PathBuf>, Vec<PathBuf>) = local
        .into_iter()
        .map(|(_, d)| d)
        .filter(|d| !d.starts_with(aio.join("crates")))
        .partition(|d| d.starts_with(&ws));
    let (base, rel) = shipped(&ws, &members, &aio, &others, &|p| p.exists());
    let rel_of = |p: &Path| p.strip_prefix(&base).map(Path::to_path_buf).map_err(|_| "outside the copy".to_string());
    let dir = std::env::var(DIR_ENV).ok().filter(|d| !d.is_empty()).unwrap_or(DIR.into());
    eprintln!("ax build: {label} on {} ({dir}), copying {} …", h.target, base.display());
    let mut tar = Command::new("tar");
    tar.arg("-czf").arg("-").arg("-C").arg(&base);
    for x in ["target", "dist", ".git", "node_modules"] {
        tar.arg(format!("--exclude={x}"));
    }
    // The names on stdin: a long list is past a command line's limit.
    tar.args(["-T", "-"]);
    let list: Vec<String> =
        rel.iter().flat_map(|r| files(&base, r)).map(|f| f.to_string_lossy().into_owned()).collect();
    let mut untar = ssh(h);
    untar.arg(format!("mkdir -p {d} && tar -xzf - -C {d}", d = q(&dir)));
    pipe(
        tar,
        Some(
            list.join(
                "
",
            ) + "
",
        ),
        untar,
        "copying to the Mac",
    )?;
    let mut run = ssh(h);
    run.arg("bash -s").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit());
    let mut child = run.spawn().map_err(|e| format!("ssh: {e}"))?;
    let text = script(&dir, &rel_of(&aio)?, &rel_of(&ws)?, name, label, platform, smoke);
    std::io::Write::write_all(&mut child.stdin.take().ok_or("no pipe")?, text.as_bytes()).map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("the build on {} failed (its output is above)", h.target));
    }
    let (how, files) = made(&String::from_utf8_lossy(&out.stdout));
    if files.is_empty() {
        return Err(format!("the build on {} made nothing", h.target));
    }
    std::fs::create_dir_all(dist).map_err(|e| e.to_string())?;
    let mut fetch = ssh(h);
    // Without macOS's `._` side files: only what was made comes back.
    let remote_dist = format!("{dir}/{}/dist", unix(&rel_of(&ws)?));
    fetch.arg(format!(
        "COPYFILE_DISABLE=1 tar --no-mac-metadata --no-xattrs -czf - -C {} {}",
        q(&remote_dist),
        files.iter().map(|f| q(f)).collect::<Vec<_>>().join(" ")
    ));
    let mut here = Command::new("tar");
    here.arg("-xzf").arg("-").arg("-C").arg(dist);
    pipe(fetch, None, here, "fetching from the Mac")?;
    Ok((how, files.iter().map(|f| dist.join(f)).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mac_is_its_ssh_target_after_any_options() {
        assert_eq!(host_of("me@mac-mini"), Some(Host { opts: vec![], target: "me@mac-mini".into() }));
        let h = host_of(" -i /k/mac -o IdentitiesOnly=yes  me@10.0.0.2 ").unwrap();
        assert_eq!(
            (h.opts, h.target.as_str()),
            (vec!["-i".into(), "/k/mac".into(), "-o".into(), "IdentitiesOnly=yes".into()], "me@10.0.0.2")
        );
        assert_eq!(host_of("  "), None);
        let pinned = configured(Some("me@studio")).unwrap();
        assert_eq!(pinned.target, "me@studio", "the app's [build.macos] host outranks AIO_MACOS_SSH");
    }

    #[test]
    fn the_copy_is_the_workspace_and_the_aio_checkout_under_their_common_folder() {
        let has = |p: &Path| !p.ends_with("rust-toolchain.toml") || p.starts_with("/w/aio");
        let (base, rel) = shipped(
            Path::new("/w/apps"),
            &["/w/apps/notes".into(), "/w/apps/notes/app".into()],
            Path::new("/w/aio"),
            &["/w/libs/chart".into()],
            &has,
        );
        assert_eq!(base, Path::new("/w"));
        let rel: Vec<String> = rel.iter().map(|p| unix(p)).collect();
        assert_eq!(
            rel,
            [
                "aio/.cargo",
                "aio/Cargo.lock",
                "aio/Cargo.toml",
                "aio/crates",
                "aio/rust-toolchain.toml",
                "apps/.cargo",
                "apps/Cargo.lock",
                "apps/Cargo.toml",
                "apps/notes",
                "libs/chart"
            ],
            "a member inside another once, an absent file left out"
        );
        // A one-package workspace is its own member: the folder, once.
        let (base, rel) =
            shipped(Path::new("/w/notes"), &["/w/notes".into()], Path::new("/w/notes/aio"), &[], &|_| true);
        assert_eq!((base.as_path(), rel), (Path::new("/w"), vec![PathBuf::from("notes")]));
    }

    #[test]
    fn the_mac_builds_aio_then_the_target_into_a_fresh_dist_and_lists_it() {
        let s = script(".cache/aio-build", Path::new("aio"), Path::new("my apps"), "notes", "dmg", None, false);
        assert_eq!(
            s.lines().collect::<Vec<_>>(),
            [
                "set -euo pipefail",
                r#"[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env""#,
                r#"cd '.cache/aio-build' && root="$PWD""#,
                r#"cd "$root"/'aio'"#,
                "cargo build --release -p aio-rs-cli >&2",
                r#"cd "$root"/'my apps'"#,
                "rm -rf dist",
                r#""$root"/'aio'/target/release/ax build 'notes' --targets 'dmg'"#,
                "echo --aio-dist--",
                "ls -1A dist",
            ]
        );
        let s = script("b", Path::new("aio"), Path::new("w"), "it's", "desktop", Some("macos"), true);
        assert!(s.contains("build 'it'\\''s' --targets 'desktop' --platforms 'macos' --smoke\n"), "{s}");
    }

    #[test]
    fn what_comes_back_is_the_artifacts_and_how_to_install_never_the_manifest_or_staging() {
        let out = "/Users/me/b/apps/dist/relay-1.0.0-macos-arm64-cli.tar.gz (3.6 MB)\ninstall: tar -xzf relay.tar.gz\n\
                   service: cp relay.plist ~/Library/LaunchAgents\n--aio-dist--\nrelay-1.0.0-macos-arm64-cli.tar.gz\n\
                   .relay-cli-staging\nrelay.manifest.json\n\nrelay-1.0.0-macos-arm64-cli.plist\n";
        let (how, files) = made(out);
        assert_eq!(files, ["relay-1.0.0-macos-arm64-cli.tar.gz", "relay-1.0.0-macos-arm64-cli.plist"]);
        assert_eq!(how, ["install: tar -xzf relay.tar.gz", "service: cp relay.plist ~/Library/LaunchAgents"]);
    }
}

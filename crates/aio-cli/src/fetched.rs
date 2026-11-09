//! `ax run <link>` (aio's `run.sh`): an app straight from its git
//! repository. The link is cloned into ax's cache (a second run fetches),
//! checked out at one commit, and what will run is shown (the link, the
//! commit, its apps) and asked at a terminal before any of its code runs:
//! building runs a repository's build scripts and macros, and the app runs
//! as the person. A commit asked once is not asked again; `--ref <commit>`
//! pins it, so a moved branch never runs unseen.
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The person's `RUSTUP_TOOLCHAIN` as ax started (ax names its folder's
/// own): the clone's build picks its toolchain by its own folder.
pub static PERSONS_TOOLCHAIN: std::sync::OnceLock<Option<OsString>> = std::sync::OnceLock::new();

/// `ax run <link> [app]`: fetched at `at`, shown, allowed (`yes`, else
/// asked), then started by `ax start` in the clone (`dev`: `ax dev`).
pub fn run(link: &str, app: Option<&str>, at: Option<&str>, yes: bool, dev: bool) -> Result<String, String> {
    let url = url_of(link)?;
    let dir = dir_of(&aio::paths::cache_home(), &url);
    let (commit, line) = fetch(&url, &dir, at)?;
    let apps = crate::dev::in_dir(dir.clone(), crate::stop::workspace_apps).unwrap_or_default();
    eprintln!("{}", shown(&url, at, &commit, &line, &dir, &apps));
    let app = pick(&apps, app)?;
    allowed(&dir, &commit, yes, &ask_terminal)?;
    let ax = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(ax);
    match dev {
        true => cmd.args(["dev", &app]),
        // Its line said, not answered: this verb answers once, as asked.
        false => cmd.args(["start", &app, "--text"]).stdout(std::io::stderr()),
    };
    cmd.current_dir(&dir);
    match PERSONS_TOOLCHAIN.get().cloned().flatten() {
        Some(t) => cmd.env("RUSTUP_TOOLCHAIN", t),
        None => cmd.env_remove("RUSTUP_TOOLCHAIN"),
    };
    let short = &commit[..12.min(commit.len())];
    match cmd.status().map_err(|e| e.to_string())?.success() {
        true => Ok(format!("{app} at {short} runs: ax stop {app} stops it")),
        false => Err(format!("{app} at {short}: did not start (its clone: {})", dir.display())),
    }
}

/// The URL git clones for `link` (pure): a URL or `git@host:path` as it is,
/// a local folder (`/`, `./`, `../`, `file://`) as it is, `github:user/repo`
/// and a bare `user/repo` on GitHub.
pub fn url_of(link: &str) -> Result<String, String> {
    let l = link.trim();
    let named = |s: &str| {
        let parts: Vec<&str> = s.trim_end_matches(".git").split('/').collect();
        let ok = |p: &&str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b));
        (parts.len() == 2 && parts.iter().all(ok) && !parts.contains(&"..")).then(|| format!("https://github.com/{s}"))
    };
    let as_is = ["https://", "http://", "ssh://", "git://", "file://", "git@", "/", "./", "../"];
    match () {
        _ if as_is.iter().any(|p| l.starts_with(p)) => Ok(l.to_string()),
        _ if cfg!(windows) && l.get(1..3) == Some(":\\") => Ok(l.to_string()),
        _ => l.strip_prefix("github:").map_or_else(|| named(l), named).ok_or_else(|| {
            format!(
                "{link}: a git URL (https://…, git@host:path), a folder (./app, /path), github:user/repo or user/repo"
            )
        }),
    }
}

/// The clone's folder in `cache` for `url` (pure): the repository's name
/// and a hash of the whole URL, so two forks never share one.
pub fn dir_of(cache: &Path, url: &str) -> PathBuf {
    let name = url.trim_end_matches('/').trim_end_matches(".git").rsplit(['/', ':', '\\']).next().unwrap_or("app");
    let name: String = name.chars().filter(|c| c.is_ascii_alphanumeric() || "-_.".contains(*c)).collect();
    let hash = aio::sha256::hex(&aio::sha256::Sha256::digest(url));
    cache.join("run").join(format!("{}-{}", name.trim_matches('.'), &hash[..8]))
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let o = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| format!("git: {e} (git is not on PATH)"))?;
    match o.status.success() {
        true => Ok(String::from_utf8_lossy(&o.stdout).trim().to_string()),
        false => Err(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&o.stderr).trim())),
    }
}

/// The commit `url` is at in clone `dir` (made if new): `at` (a branch,
/// tag or commit, 7 to 40 hex digits; None: its default branch) fetched,
/// checked out alone (nothing of an earlier run left), its id and one
/// line saying it.
pub fn fetch(url: &str, dir: &Path, at: Option<&str>) -> Result<(String, String), String> {
    if !dir.join(".git").is_dir() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        git(dir, &["init", "-q"])?;
        git(dir, &["remote", "add", "origin", url])?;
    }
    git(dir, &["remote", "set-url", "origin", url])?;
    let commit = |c: &&str| (7..=40).contains(&c.len()) && c.bytes().all(|b| b.is_ascii_hexdigit());
    let got = git(dir, &["fetch", "-q", "origin", at.unwrap_or("HEAD")]);
    let rev = match (got, at.filter(commit)) {
        (Ok(_), None) => git(dir, &["rev-parse", "--verify", "FETCH_HEAD^{commit}"])?,
        (Err(e), None) => return Err(e),
        // A commit: by its id where the server allows, else from all its branches and tags.
        (got, Some(c)) => {
            if got.is_err() {
                git(dir, &["fetch", "-q", "--tags", "origin", "+refs/heads/*:refs/remotes/origin/*"])?;
            }
            git(dir, &["rev-parse", "--verify", &format!("{c}^{{commit}}")])
                .map_err(|_| format!("{url} has no commit {c}"))?
        }
    };
    git(dir, &["checkout", "-q", "--detach", "--force", &rev])?;
    git(dir, &["clean", "-q", "-ffdx"])?;
    let line = git(dir, &["log", "-1", "--format=%cs %s", &rev])?;
    Ok((rev, line))
}

/// What a run of `url` at `commit` shows before anything of it runs (pure).
pub fn shown(url: &str, at: Option<&str>, commit: &str, line: &str, dir: &Path, apps: &[String]) -> String {
    let link = |s: &str| if s.contains(char::is_whitespace) { format!("'{s}'") } else { s.to_string() };
    format!(
        "ax run: {url}\n  commit   {commit} ({}) {line}\n  in       {}\n  apps     {}\n  \
         Building runs this repository's code (build scripts, macros), and the app runs as you, with your \
         files: run only what you trust.\n  this commit again: ax run {} --ref {commit}",
        at.unwrap_or("its default branch"),
        dir.display(),
        if apps.is_empty() { "none".to_string() } else { apps.join(", ") },
        link(url),
    )
}

/// The file in clone `dir` naming the commit last let run.
fn approved(dir: &Path) -> PathBuf {
    dir.join(".git").join("ax-run-approved")
}

/// Whether `commit` of clone `dir` may run: asked once (`yes`: not asked;
/// `ask`: the person's answer, None with nobody to ask), remembered.
pub fn allowed(dir: &Path, commit: &str, yes: bool, ask: &dyn Fn() -> Option<bool>) -> Result<(), String> {
    let before = std::fs::read_to_string(approved(dir)).is_ok_and(|c| c.trim() == commit);
    match before || yes {
        true => {}
        false => match ask() {
            None => return Err("not run: nobody here to ask; read the above, then run it again with --yes".into()),
            Some(false) => return Err("not run".into()),
            Some(true) => {}
        },
    }
    std::fs::write(approved(dir), commit).map_err(|e| e.to_string())
}

/// "Run it?" at the terminal: None when there is none.
pub fn ask_terminal() -> Option<bool> {
    use std::io::{BufRead, IsTerminal, Write};
    if !std::io::stdin().is_terminal() {
        return None;
    }
    eprint!("Run it? [y/N] ");
    _ = std::io::stderr().flush();
    let mut said = String::new();
    _ = std::io::stdin().lock().read_line(&mut said);
    Some(matches!(said.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}

/// Which app of `apps` runs: `asked`, else the only one (pure).
pub fn pick(apps: &[String], asked: Option<&str>) -> Result<String, String> {
    match (asked, apps) {
        (Some(a), _) if apps.iter().any(|x| x == a) => Ok(a.to_string()),
        (Some(a), _) => Err(format!("no app {a} in it: {}", apps.join(", "))),
        (None, [one]) => Ok(one.clone()),
        (None, []) => Err("no aio-rs app in it (no binary with an aio.toml)".into()),
        (None, many) => Err(format!("it has {} apps: name one ({})", many.len(), many.join(", "))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_is_what_git_clones_and_anything_else_is_refused() {
        assert_eq!(url_of("github:ria/notes").unwrap(), "https://github.com/ria/notes");
        assert_eq!(url_of("ria/notes.git").unwrap(), "https://github.com/ria/notes.git");
        for l in ["https://example.org/a/b.git", "git@example.org:a/b", "ssh://h/a", "./app", "/srv/app", "file:///x"] {
            assert_eq!(url_of(l).unwrap(), l);
        }
        for bad in ["notes", "a/b/c", "github:a/..", "a/b c", ""] {
            assert!(url_of(bad).is_err(), "{bad}");
        }
        assert!(url_of("--upload-pack=x/y").is_err(), "never an option to git");
    }

    #[test]
    fn each_link_has_its_own_clone_named_after_it() {
        let c = Path::new("/c");
        let a = dir_of(c, "https://github.com/ria/notes.git");
        assert!(a.starts_with("/c/run") && a.file_name().unwrap().to_str().unwrap().starts_with("notes-"), "{a:?}");
        assert_ne!(a, dir_of(c, "https://github.com/fork/notes.git"));
        assert_eq!(a, dir_of(c, "https://github.com/ria/notes.git"));
        assert!(dir_of(c, "git@h:..").file_name().unwrap().to_str().unwrap().starts_with('-'));
    }

    #[test]
    fn what_runs_is_shown_with_the_line_that_pins_it() {
        let s = shown("https://h/n", None, "abc123", "2026-11-01 first", Path::new("/c/run/n"), &["notes".into()]);
        assert!(s.contains("commit   abc123 (its default branch) 2026-11-01 first"), "{s}");
        assert!(s.contains("apps     notes") && s.contains("run only what you trust"), "{s}");
        assert!(s.ends_with("ax run https://h/n --ref abc123"), "{s}");
    }

    #[test]
    fn the_app_is_the_one_named_or_the_only_one() {
        let apps = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(pick(&apps(&["notes"]), None).unwrap(), "notes");
        assert_eq!(pick(&apps(&["a", "b"]), Some("b")).unwrap(), "b");
        assert!(pick(&apps(&["a", "b"]), None).unwrap_err().contains("name one (a, b)"));
        assert!(pick(&apps(&["a"]), Some("z")).unwrap_err().starts_with("no app z"));
        assert!(pick(&[], None).unwrap_err().starts_with("no aio-rs app"));
    }

    fn sh(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.org")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.org")
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    #[test]
    fn a_run_is_one_commit_fetched_alone_pinned_and_asked_once() {
        if Command::new("git").arg("--version").output().is_err() {
            return crate::web::skipped("no git");
        }
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        sh(&src, &["init", "-q", "-b", "main"]);
        std::fs::write(src.join("a.txt"), "one").unwrap();
        sh(&src, &["add", "."]);
        sh(&src, &["commit", "-q", "-m", "first"]);
        let url = src.display().to_string();
        let dir = dir_of(&t.path().join("cache"), &url);
        let (first, line) = fetch(&url, &dir, None).unwrap();
        assert!(line.ends_with(" first"), "{line}");
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "one");
        // The branch moves; a run of it follows, a pinned run doesn't.
        std::fs::write(src.join("a.txt"), "two").unwrap();
        sh(&src, &["commit", "-q", "-am", "second"]);
        std::fs::write(dir.join("left.txt"), "x").unwrap();
        let (second, _) = fetch(&url, &dir, Some("main")).unwrap();
        assert_ne!(first, second);
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "two");
        assert!(!dir.join("left.txt").exists(), "nothing of an earlier run is left");
        assert_eq!(fetch(&url, &dir, Some(&first)).unwrap().0, first);
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "one");
        assert!(fetch(&url, &dir, Some("0123456789abcdef")).is_err(), "a commit it hasn't is refused");
        // Asked once, then not again for that commit only.
        let never = || -> Option<bool> { panic!("asked") };
        assert_eq!(allowed(&dir, &first, false, &|| Some(false)).unwrap_err(), "not run");
        assert!(allowed(&dir, &first, false, &|| Some(true)).is_ok());
        assert!(allowed(&dir, &first, false, &never).is_ok(), "approved before");
        assert!(allowed(&dir, &second, false, &|| None).unwrap_err().contains("--yes"), "nobody to ask");
        assert!(allowed(&dir, &second, true, &never).is_ok(), "--yes asks nobody");
    }
}

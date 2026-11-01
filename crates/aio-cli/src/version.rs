//! The app's version (aio's `app-version.ts`). `[app] version` in `aio.toml`
//! is `major.minor`, and aio numbers the builds from git: `1.2.<commit
//! count>`, so every commit bumps it and two builds of one commit agree. A
//! three-part `1.2.3` is pinned, used as written. Either may end in a stage
//! (`-alpha` < `-beta` < `-rc` < none): `1.2-beta` builds `1.2.345-beta`.
//! Uncommitted edits make `-dirty.<hash8>` of them, a build outside git
//! `1.2.0-nogit.<hash8>` of its files: prereleases, so below the clean
//! build, and never released. Without `[app] version` the package's
//! (`Cargo.toml`) is used as written. The build gives the binary its
//! version as `AIORT_APP_VERSION` (`aio::app_version!()`).
use std::path::{Path, PathBuf};
use std::process::Command;

/// A declared version: `major.minor`, the patch when pinned, the stage.
#[derive(Debug, PartialEq)]
pub struct Declared {
    pub base: (u64, u64),
    pub pinned: Option<u64>,
    pub stage: Option<&'static str>,
}

/// The stages, least finished first.
pub const STAGES: [&str; 3] = ["alpha", "beta", "rc"];

/// `[app] version`, strictly (pure): anything but `M.m` or `M.m.p`, with an
/// optional stage, is refused by name.
pub fn declared(v: &str) -> Result<Declared, String> {
    let raw = v.trim();
    let (core, stage) = match raw.split_once('-') {
        Some((c, s)) => (c, Some(STAGES.into_iter().find(|t| *t == s).ok_or_else(|| refusal(raw))?)),
        None => (raw, None),
    };
    let nums: Vec<u64> = core
        .split('.')
        .map(|n| n.chars().all(|c| c.is_ascii_digit()).then(|| n.parse().ok()).flatten())
        .collect::<Option<_>>()
        .ok_or_else(|| refusal(raw))?;
    match nums[..] {
        [major, minor] => Ok(Declared { base: (major, minor), pinned: None, stage }),
        [major, minor, patch] => Ok(Declared { base: (major, minor), pinned: Some(patch), stage }),
        _ => Err(refusal(raw)),
    }
}

fn refusal(shown: &str) -> String {
    format!(
        "aio.toml: [app] version {shown:?}: an app's version is \"major.minor\" (write \"1.2\"; aio numbers builds \
         from commits: 1.2.<commit count>) or a pinned \"major.minor.patch\" (used as written), either one perhaps \
         followed by a stage, -alpha, -beta or -rc (\"1.2-beta\" builds 1.2.345-beta, ranked below 1.2.345)"
    )
}

/// What the tree says about a build.
#[derive(Debug, PartialEq)]
pub enum Tree {
    /// In git: its commit count, and the hash of its edits (None: clean).
    Git { count: u64, dirty: Option<String> },
    /// Outside git: the hash of its files.
    None { files: String },
}

/// The version `d` builds as from tree `t` (pure).
pub fn resolve(d: &Declared, t: &Tree) -> String {
    let (major, minor) = d.base;
    let tail = |mark: Option<String>| {
        let ids: Vec<String> = d.stage.map(str::to_string).into_iter().chain(mark).collect();
        if ids.is_empty() { String::new() } else { format!("-{}", ids.join(".")) }
    };
    match (d.pinned, t) {
        (Some(p), _) => format!("{major}.{minor}.{p}{}", tail(None)),
        (None, Tree::Git { count, dirty }) => {
            format!("{major}.{minor}.{count}{}", tail(dirty.as_ref().map(|h| format!("dirty.{h}"))))
        }
        (None, Tree::None { files }) => format!("{major}.{minor}.0{}", tail(Some(format!("nogit.{files}")))),
    }
}

/// Why version `v` may not be released (None: it may): a dirty or no-git
/// build can't be made again from a commit (pure).
pub fn unreleasable(v: &str) -> Option<String> {
    let pre = v.split_once('-')?.1;
    let mark = pre.split('.').find(|id| *id == "dirty" || *id == "nogit")?;
    let (what, fix) = if mark == "dirty" {
        ("uncommitted edits", "commit first")
    } else {
        ("no git repository", "git init, git add -A and git commit first")
    };
    Some(format!(
        "version {v} is a build of {what}: {fix}, a release is made again from its commit \
         (--allow-dirty releases it anyway)"
    ))
}

/// The tree of the app in `dirs` (its package's, then its workspace
/// packages'), as git sees it, else by its files.
pub fn tree(dirs: &[PathBuf]) -> Tree {
    let count = dirs.first().and_then(|d| {
        let out = Command::new("git").arg("-C").arg(d).args(["rev-list", "--count", "HEAD"]).output().ok()?;
        String::from_utf8_lossy(&out.stdout).trim().parse().ok().filter(|_| out.status.success())
    });
    match (count, crate::manifest::tree(dirs)) {
        (Some(count), Some((_, dirty))) => Tree::Git { count, dirty },
        _ => Tree::None { files: files_hash(dirs) },
    }
}

/// The hash of every file in `dirs`, but what a build makes or a tool
/// keeps (`target`, `dist`, `node_modules`, dot folders).
fn files_hash(dirs: &[PathBuf]) -> String {
    fn walk(root: &Path, at: &Path, out: &mut Vec<(String, Option<Vec<u8>>)>) {
        let Ok(rd) = std::fs::read_dir(at) else { return };
        for e in rd.flatten() {
            let (p, name) = (e.path(), e.file_name().to_string_lossy().into_owned());
            match e.file_type() {
                Ok(t) if t.is_dir() => {
                    if !name.starts_with('.') && !["target", "dist", "node_modules"].contains(&name.as_str()) {
                        walk(root, &p, out);
                    }
                }
                Ok(t) if t.is_file() => {
                    let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                    out.push((rel, std::fs::read(&p).ok()));
                }
                _ => {}
            }
        }
    }
    let mut all = vec![];
    for d in dirs {
        let root = d.parent().unwrap_or(d);
        walk(root, d, &mut all);
    }
    all.sort();
    all.dedup_by(|a, b| a.0 == b.0);
    crate::manifest::dirty_hash(&all)
}

/// The version of the app whose `aio.toml` declares `v` (None: none) and
/// whose package says `package`, built from `dirs`: resolved once per run,
/// so what a build writes (`Cargo.lock`, `dist/`) never renames its later
/// targets or its manifest.
pub fn of(v: Option<&str>, package: &str, dirs: &[PathBuf]) -> Result<String, String> {
    static RESOLVED: std::sync::Mutex<Vec<(Vec<PathBuf>, String)>> = std::sync::Mutex::new(Vec::new());
    let Some(v) = v else { return Ok(package.to_string()) };
    let d = declared(v)?;
    let mut seen = RESOLVED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, got)) = seen.iter().find(|(at, _)| at == dirs) {
        return Ok(got.clone());
    }
    let got = resolve(&d, &tree(dirs));
    seen.push((dirs.to_vec(), got.clone()));
    Ok(got)
}

/// Does `binary` carry version `v`? An app that says another one (its
/// package's) would be offered its own release again by every update check.
pub fn carried(binary: &[u8], v: &str) -> bool {
    binary.windows(v.len()).any(|w| w == v.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aio::update::compare;
    use std::cmp::Ordering::*;

    fn clean() -> Tree {
        Tree::Git { count: 345, dirty: None }
    }

    fn dirty() -> Tree {
        Tree::Git { count: 345, dirty: Some("4e1d0c77".into()) }
    }

    fn v(d: &str, t: &Tree) -> String {
        resolve(&declared(d).unwrap(), t)
    }

    #[test]
    fn a_clean_tree_is_major_minor_and_its_commit_count() {
        assert_eq!(v("1.2", &clean()), "1.2.345");
        assert_eq!(v(" 01.2 ", &clean()), "1.2.345", "one version, one spelling");
        assert_eq!(v("1.2", &Tree::Git { count: 0, dirty: None }), "1.2.0");
    }

    #[test]
    fn a_dirty_tree_says_so_and_ranks_below_the_clean_build() {
        let d = v("1.2", &dirty());
        assert_eq!(d, "1.2.345-dirty.4e1d0c77");
        assert_eq!(compare(&d, "1.2.345"), Some(Less));
        assert_eq!(compare(&d, "1.2.344"), Some(Greater), "above the commit before");
    }

    #[test]
    fn outside_git_the_build_is_zero_and_marked_by_its_files() {
        let t = Tree::None { files: "aabbccdd".into() };
        assert_eq!(v("1.2", &t), "1.2.0-nogit.aabbccdd");
        assert_eq!(v("1.2-rc", &t), "1.2.0-rc.nogit.aabbccdd");
    }

    #[test]
    fn a_three_part_version_is_pinned_as_written() {
        for t in [clean(), dirty(), Tree::None { files: "aabbccdd".into() }] {
            assert_eq!(v("1.0.0", &t), "1.0.0");
            assert_eq!(v("1.0.7-rc", &t), "1.0.7-rc");
        }
    }

    #[test]
    fn a_stage_rides_the_version_in_one_prerelease_tail() {
        for s in STAGES {
            assert_eq!(v(&format!("1.2-{s}"), &clean()), format!("1.2.345-{s}"));
        }
        assert_eq!(v("1.2-beta", &dirty()), "1.2.345-beta.dirty.4e1d0c77");
        // alpha < beta < rc < the release; a later build beats any stage.
        let order = ["1.2.345-alpha", "1.2.345-beta", "1.2.345-rc", "1.2.345", "1.2.346-alpha"];
        for w in order.windows(2) {
            assert_eq!(compare(w[0], w[1]), Some(Less), "{} < {}", w[0], w[1]);
        }
        assert_eq!(compare("1.2.345-beta.dirty.4e1d0c77", "1.2.345-rc"), Some(Less));
    }

    #[test]
    fn anything_else_is_refused_by_name() {
        for bad in ["", "1", "1.2.3.4", "v1.2", "1.x", "1.2-beta2", "1.2-Beta", "1.2-", "1.2-dirty", "1.+2", "1..2"] {
            let e = declared(bad).unwrap_err();
            assert!(e.contains(&format!("{:?}", bad.trim())) && e.contains("major.minor"), "{bad}: {e}");
        }
        assert_eq!(declared("1.2-rc").unwrap(), Declared { base: (1, 2), pinned: None, stage: Some("rc") });
    }

    #[test]
    fn a_dirty_or_no_git_build_is_never_released() {
        for bad in ["1.2.345-dirty.4e1d0c77", "1.2.345-beta.dirty.4e1d0c77", "1.2.0-nogit.aabbccdd"] {
            assert!(unreleasable(bad).is_some_and(|w| w.contains(bad) && w.contains("--allow-dirty")), "{bad}");
        }
        for ok in ["1.2.345", "1.2.345-beta", "1.0.0", "0.1.0"] {
            assert_eq!(unreleasable(ok), None, "{ok}");
        }
    }

    #[test]
    fn without_a_declared_version_the_package_one_is_used() {
        assert_eq!(of(None, "0.3.1", &[]).unwrap(), "0.3.1");
        assert!(of(Some("1.2.x"), "0.3.1", &[]).is_err());
    }

    #[test]
    fn a_tree_is_counted_in_git_and_hashed_outside_it() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("app");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::write(dir.join("src/a.rs"), "a").unwrap();
        let dirs = [dir.clone()];
        let Tree::None { files: one } = tree(&dirs) else { panic!("no repository") };
        let first = of(Some("1.2"), "0.0.1", &dirs).unwrap();
        assert_eq!(first, format!("1.2.0-nogit.{one}"));
        std::fs::write(dir.join("target/out"), "built").unwrap();
        assert_eq!(tree(&dirs), Tree::None { files: one.clone() }, "what a build makes is not the app");
        std::fs::write(dir.join("src/a.rs"), "b").unwrap();
        assert_ne!(tree(&dirs), Tree::None { files: one }, "an edit is");
        if Command::new("git").arg("--version").output().is_err() {
            eprintln!("skipped: no git on this machine");
            return;
        }
        let git = |args: &[&str]| {
            let ok = Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid", "-c", "commit.gpgsign=false"])
                .args(args)
                .output()
                .unwrap();
            assert!(ok.status.success(), "{}", String::from_utf8_lossy(&ok.stderr));
        };
        git(&["init", "-q"]);
        std::fs::write(dir.join(".gitignore"), "/target\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "one"]);
        git(&["commit", "-q", "--allow-empty", "-m", "two"]);
        assert_eq!(tree(&dirs), Tree::Git { count: 2, dirty: None });
        std::fs::write(dir.join("src/a.rs"), "c").unwrap();
        assert!(matches!(tree(&dirs), Tree::Git { count: 2, dirty: Some(h) } if h.len() == 8));
        assert_eq!(of(Some("1.2"), "0.0.1", &dirs).unwrap(), first, "one resolution per run");
    }

    #[test]
    fn a_binary_carries_the_version_it_was_given() {
        assert!(carried(b"\0x1.2.345-dirty.4e1d0c77\0", "1.2.345-dirty.4e1d0c77"));
        assert!(!carried(b"\x000.1.0\0", "1.2.345"));
    }
}

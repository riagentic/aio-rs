//! `dist/<id>.manifest.json` (aio's `dist/manifest.json`): what `ax build
//! <app>` made: the version, the commit and whether the tree was dirty (with
//! the hash of what was), when, and per target its artifacts and Android
//! id. A build reads the one before it and warns when the version went down
//! (Android refuses a lower versionCode as an update; the update check never
//! offers it) or an Android id changed (it installs as a second app).
use aio::sha256::Sha256;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

/// One target's outcome.
pub struct Target {
    pub label: String,
    pub kind: String,
    pub ok: bool,
    pub application_id: Option<String>,
    pub files: Vec<PathBuf>,
}

/// The commit and the dirty set's hash (None: clean), of the git tree
/// holding `dirs`; None outside a repository.
pub type Tree = Option<(String, Option<String>)>;

/// 8 hex of the dirty set: each path with its content (None: deleted), so
/// the same edit hashes the same and another edit does not.
pub fn dirty_hash(entries: &[(String, Option<Vec<u8>>)]) -> String {
    let mut h = Sha256::new();
    for (path, content) in entries {
        h.update(path.as_bytes());
        h.update([0]);
        match content {
            Some(c) => h.update(c),
            None => h.update(b"\x00deleted"),
        }
        h.update([0]);
    }
    h.finalize()[..4].iter().map(|b| format!("{b:02x}")).collect()
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The tree of `dirs`: its commit, and what in `dirs` differs from it.
pub fn tree(dirs: &[PathBuf]) -> Tree {
    let dir = dirs.first()?;
    let commit = git(dir, &["rev-parse", "--short=8", "HEAD"])?.trim().to_string();
    let top = PathBuf::from(git(dir, &["rev-parse", "--show-toplevel"])?.trim());
    let mut paths: Vec<String> = dirs
        .iter()
        .filter_map(|d| git(d, &["status", "--porcelain=v1", "-z", "--untracked-files=all", "--", "."]))
        .flat_map(|s| s.split('\0').filter(|e| e.len() > 3).map(|e| e[3..].to_string()).collect::<Vec<_>>())
        .collect();
    paths.sort();
    paths.dedup();
    let entries: Vec<(String, Option<Vec<u8>>)> =
        paths.into_iter().map(|p| (p.clone(), std::fs::read(top.join(&p)).ok())).collect();
    Some((commit, (!entries.is_empty()).then(|| dirty_hash(&entries))))
}

/// The manifest of this build.
pub fn manifest(id: &str, name: &str, version: &str, tree: &Tree, built_at: &str, targets: &[Target]) -> Value {
    let file = |p: &PathBuf| {
        json!({
            "file": p.file_name().map(|f| f.to_string_lossy().into_owned()),
            "bytes": std::fs::metadata(p).map(|m| m.len()).ok(),
        })
    };
    json!({
        "app": id,
        "name": name,
        "version": version,
        "commit": tree.as_ref().map(|t| &t.0),
        "dirty": tree.as_ref().and_then(|t| t.1.as_ref()),
        "builtAt": built_at,
        "targets": targets.iter().map(|t| json!({
            "target": t.label,
            "kind": t.kind,
            "ok": t.ok,
            "applicationId": t.application_id,
            "artifacts": t.files.iter().map(file).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

fn triple(v: &str) -> Option<[u64; 3]> {
    let mut n = v.split(['-', '+']).next()?.split('.').map(|p| p.parse().ok());
    Some([n.next()??, n.next()??, n.next()??])
}

/// What the build after `prev` should be told about `next`.
pub fn warnings(prev: &Value, next: &Value) -> Vec<String> {
    let mut out = vec![];
    let (was, now) = (prev["version"].as_str().unwrap_or_default(), next["version"].as_str().unwrap_or_default());
    if let (Some(a), Some(b)) = (triple(was), triple(now))
        && b < a
    {
        out.push(format!(
            "version {now} is lower than the last build's {was}: Android refuses it as an update (a lower \
             versionCode; only an uninstall gets past, and that deletes the app's data), and the update check \
             never offers it. Raise the version in Cargo.toml"
        ));
    }
    for t in next["targets"].as_array().into_iter().flatten() {
        let Some(id) = t["applicationId"].as_str() else { continue };
        let before =
            prev["targets"].as_array().into_iter().flatten().find(|p| p["target"] == t["target"] && p["ok"] != false);
        if let Some(was) = before.and_then(|p| p["applicationId"].as_str()).filter(|was| *was != id) {
            out.push(format!(
                "{}: the Android id is now {id}, the last build's was {was}: Android installs it as a second app, \
                 and the installed one keeps its data. Pin it: [build.android] application_id = \"{was}\" (or the \
                 new one, if the change is meant)",
                t["target"].as_str().unwrap_or_default()
            ));
        }
    }
    out
}

/// Where app `id`'s manifest is.
pub fn path(dist: &Path, id: &str) -> PathBuf {
    dist.join(format!("{id}.manifest.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(version: &str, id: Option<&str>, ok: bool) -> Value {
        let t = Target {
            label: "android".into(),
            kind: "android".into(),
            ok,
            application_id: id.map(String::from),
            files: vec![PathBuf::from("/nowhere/notes-1.2.0-android-x86_64.apk")],
        };
        manifest("notes", "Notes", version, &Some(("abcdef12".into(), None)), "2026-01-01T00:00:00Z", &[t])
    }

    #[test]
    fn the_manifest_says_what_was_built_from_what() {
        let v = m("1.2.0", Some("aio.rt.notes"), true);
        assert_eq!(
            (v["app"].as_str(), v["version"].as_str(), v["commit"].as_str()),
            (Some("notes"), Some("1.2.0"), Some("abcdef12"))
        );
        assert_eq!(v["dirty"], Value::Null, "a clean tree");
        assert_eq!(v["targets"][0]["artifacts"][0]["file"], "notes-1.2.0-android-x86_64.apk");
        assert_eq!(v["targets"][0]["applicationId"], "aio.rt.notes");
        let dirty = manifest("notes", "Notes", "1.2.0", &Some(("abcdef12".into(), Some("9f3ac2b1".into()))), "", &[]);
        assert_eq!(dirty["dirty"], "9f3ac2b1");
        assert_eq!(manifest("notes", "Notes", "1.2.0", &None, "", &[])["commit"], Value::Null, "no repository");
    }

    #[test]
    fn the_dirty_hash_is_the_content_the_same_edit_the_same() {
        let a = dirty_hash(&[("src/a.rs".into(), Some(b"x".to_vec()))]);
        assert_eq!(a.len(), 8);
        assert_eq!(a, dirty_hash(&[("src/a.rs".into(), Some(b"x".to_vec()))]));
        assert_ne!(a, dirty_hash(&[("src/a.rs".into(), Some(b"y".to_vec()))]));
        assert_ne!(a, dirty_hash(&[("src/a.rs".into(), None)]), "a deletion is an edit");
        assert_ne!(a, dirty_hash(&[("src/b.rs".into(), Some(b"x".to_vec()))]));
    }

    #[test]
    fn a_lower_version_or_a_changed_android_id_is_warned() {
        let prev = m("1.2.0", Some("aio.rt.notes"), true);
        assert!(warnings(&prev, &m("1.2.1", Some("aio.rt.notes"), true)).is_empty());
        assert!(warnings(&prev, &m("1.2.0", Some("aio.rt.notes"), true)).is_empty());
        let w = warnings(&prev, &m("1.1.9", Some("aio.rt.notes"), true));
        assert!(w.len() == 1 && w[0].starts_with("version 1.1.9 is lower than the last build's 1.2.0"), "{w:?}");
        assert!(warnings(&prev, &m("1.10.0", None, true)).is_empty(), "numbers, not text");
        let w = warnings(&prev, &m("1.2.1", Some("com.example.notes"), true));
        assert!(
            w.len() == 1
                && w[0].contains("now com.example.notes")
                && w[0].contains("application_id = \"aio.rt.notes\""),
            "{w:?}"
        );
        assert!(
            warnings(&m("1.2.0", Some("x.y"), false), &m("1.2.1", Some("a.b"), true)).is_empty(),
            "a failed build said nothing"
        );
        assert!(warnings(&Value::Null, &m("1.2.0", Some("a.b"), true)).is_empty(), "the first build");
    }

    #[test]
    fn a_tree_is_its_commit_and_its_edits_and_none_outside_a_repository() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().to_path_buf();
        assert_eq!(tree(std::slice::from_ref(&dir)), None);
        if Command::new("git").arg("--version").output().is_err() {
            eprintln!("skipped: no git on this machine (tree is None without it, as above)");
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
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/a.rs"), "a").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "x"]);
        let (commit, dirty) = tree(std::slice::from_ref(&dir)).unwrap();
        assert_eq!((commit.len(), dirty), (8, None));
        std::fs::write(dir.join("src/a.rs"), "b").unwrap();
        let one = tree(std::slice::from_ref(&dir)).unwrap().1.unwrap();
        assert_eq!(tree(std::slice::from_ref(&dir)).unwrap().1.unwrap(), one, "the same edit, the same hash");
        std::fs::write(dir.join("src/new.rs"), "n").unwrap();
        assert_ne!(tree(std::slice::from_ref(&dir)).unwrap().1.unwrap(), one, "an untracked file counts");
    }
}

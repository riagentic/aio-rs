//! An app's credits (its About's): every crate its binary links (normal
//! dependencies; no build script's, no proc-macro's, no other platform's),
//! with its licence and licence texts, from `cargo tree` and `cargo
//! metadata`: the sources cargo already has, no network. `ax build` writes
//! them under the build folder and hands their path to the build
//! (`AIORT_CREDITS`), which `aio::credits!()` builds in.
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Is a file a licence's (`LICENSE-MIT`, `COPYING`, `NOTICE`)?
fn is_licence(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    ["LICENSE", "LICENCE", "COPYING", "NOTICE", "UNLICENSE", "COPYRIGHT"].iter().any(|p| n.starts_with(p))
}

/// The licence files of the crate in `dir` (`license_file` too, where its
/// manifest names one elsewhere), `(name, text)`.
pub fn files(dir: &Path, license_file: Option<&str>) -> Vec<(String, String)> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| is_licence(n))
        .collect();
    if let Some(f) = license_file.filter(|f| !names.iter().any(|n| n == f)) {
        names.push(f.into());
    }
    names.into_iter().filter_map(|n| Some((n.clone(), std::fs::read_to_string(dir.join(&n)).ok()?))).collect()
}

/// The credits text (pure but for `texts`): of each crate `tree` lists
/// (`cargo tree --prefix none -f "{p}"`) but the workspace's own, by name
/// and version, its licence, repository and licence texts (`texts`: of a
/// crate's folder and `license_file`); a text met before is named, not
/// repeated. The same lock, platform and features give the same bytes.
pub fn text(meta: &Value, tree: &str, texts: &dyn Fn(&Path, Option<&str>) -> Vec<(String, String)>) -> String {
    let own: HashSet<&str> =
        meta["workspace_members"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let packages: HashMap<(&str, &str), &Value> = meta["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| !p["id"].as_str().is_some_and(|id| own.contains(id)))
        .filter_map(|p| Some(((p["name"].as_str()?, p["version"].as_str()?), p)))
        .collect();
    let linked: BTreeMap<(&str, &str), &Value> = tree
        .lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            let (name, version) = (w.next()?, w.next()?.strip_prefix('v')?);
            packages.get_key_value(&(name, version)).map(|(k, p)| (*k, *p))
        })
        .collect();
    let mut out = format!(
        "# Credits\n\nThe {} open-source crates this app is built with, their licences and licence texts.\n",
        linked.len()
    );
    let mut seen: HashMap<String, String> = HashMap::new();
    for ((name, version), p) in linked {
        let licence = p["license"].as_str().unwrap_or("no licence declared");
        out += &format!("\n## {name} {version} ({licence})\n");
        if let Some(r) = p["repository"].as_str() {
            out += &format!("{r}\n");
        }
        let Some(dir) = p["manifest_path"].as_str().and_then(|m| Path::new(m).parent()) else { continue };
        let mut files = texts(dir, p["license_file"].as_str());
        files.sort();
        for (file, body) in files {
            let body: String = body.replace("\r\n", "\n").lines().map(|l| l.trim_end().to_string() + "\n").collect();
            match seen.get(&body) {
                Some(first) => out += &format!("\n### {file}: as {first}\n"),
                None => {
                    out += &format!("\n### {file}\n\n{}", fenced(body.trim_matches('\n')));
                    seen.insert(body, format!("{name} {version}"));
                }
            }
        }
    }
    out
}

/// `text` in a code fence longer than any run of backticks in it: a
/// licence's own `#` lines never read as the credits' headings (pure).
fn fenced(text: &str) -> String {
    let run = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(run.max(2) + 1);
    format!("{fence}text\n{text}\n{fence}\n")
}

/// Write the credits of package `pkg` of the workspace at `root`, built
/// for `triple` with `features`, at `<target>/aio/<pkg>-<triple>-credits.md`
/// (only when they changed: the build takes them in again on each write),
/// and hand that path to `cargo` (`AIORT_CREDITS`).
pub fn stamp(
    cargo: &mut Command,
    root: &Path,
    target: &Path,
    pkg: &str,
    triple: &str,
    features: &[String],
) -> Result<PathBuf, String> {
    let run = |args: &[&str]| {
        let out = Command::new("cargo").args(args).current_dir(root).output().map_err(|e| format!("cargo: {e}"))?;
        match out.status.success() {
            true => Ok(out.stdout),
            false => Err(format!("credits: cargo {}: {}", args[0], String::from_utf8_lossy(&out.stderr).trim_end())),
        }
    };
    let joined = features.join(",");
    let mut args = vec!["tree", "-p", pkg, "-e", "normal,no-proc-macro", "--target", triple, "--prefix", "none"];
    args.extend(["-f", "{p}"]);
    if !features.is_empty() {
        args.extend(["--features", &joined]);
    }
    let tree = String::from_utf8_lossy(&run(&args)?).into_owned();
    let meta = run(&["metadata", "--format-version", "1", "--filter-platform", triple])?;
    let meta: Value = serde_json::from_slice(&meta).map_err(|e| format!("credits: cargo metadata: {e}"))?;
    let credits = text(&meta, &tree, &files);
    let at = target.join("aio").join(format!("{pkg}-{triple}-credits.md"));
    if std::fs::read_to_string(&at).ok().as_deref() != Some(&credits) {
        std::fs::create_dir_all(target.join("aio")).map_err(|e| e.to_string())?;
        std::fs::write(&at, &credits).map_err(|e| format!("{}: {e}", at.display()))?;
    }
    cargo.env("AIORT_CREDITS", &at);
    Ok(at)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credits_name_each_linked_crate_once_with_its_licence_texts_deterministically() {
        let meta = serde_json::json!({
            "workspace_members": ["app 0.1.0 (path+file:///w/app)"],
            "packages": [
                { "id": "app 0.1.0 (path+file:///w/app)", "name": "app", "version": "0.1.0", "license": "MIT",
                  "manifest_path": "/w/app/Cargo.toml" },
                { "id": "zeta", "name": "zeta", "version": "2.0.0", "license": "MIT OR Apache-2.0",
                  "repository": "https://example.org/zeta", "manifest_path": "/r/zeta-2.0.0/Cargo.toml" },
                { "id": "alpha", "name": "alpha", "version": "1.0.0", "license": "Apache-2.0",
                  "manifest_path": "/r/alpha-1.0.0/Cargo.toml", "license_file": "legal/LICENSE.txt" },
                { "id": "bare", "name": "bare", "version": "0.3.0", "manifest_path": "/r/bare-0.3.0/Cargo.toml" },
                { "id": "unlinked", "name": "unlinked", "version": "1.0.0", "license": "GPL-3.0",
                  "manifest_path": "/r/unlinked-1.0.0/Cargo.toml" }
            ]
        });
        // cargo tree's lines: the app itself, a crate met twice, a path dependency.
        let tree = "app v0.1.0 (/w/app)\nzeta v2.0.0\nalpha v1.0.0\nzeta v2.0.0 (*)\nbare v0.3.0 (/r/bare-0.3.0)\n";
        let apache = "Apache License\r\nVersion 2.0   \r\n";
        let texts = |dir: &Path, file: Option<&str>| -> Vec<(String, String)> {
            match (dir.to_str().unwrap(), file) {
                ("/r/zeta-2.0.0", None) => {
                    let mit = "## MIT\n``` (c) zeta\n";
                    vec![("LICENSE-MIT".into(), mit.into()), ("LICENSE-APACHE".into(), apache.into())]
                }
                ("/r/alpha-1.0.0", Some("legal/LICENSE.txt")) => vec![("legal/LICENSE.txt".into(), apache.into())],
                _ => vec![],
            }
        };
        let t = text(&meta, tree, &texts);
        assert_eq!(
            t,
            "# Credits\n\nThe 3 open-source crates this app is built with, their licences and licence texts.\n\
             \n## alpha 1.0.0 (Apache-2.0)\n\n### legal/LICENSE.txt\n\n```text\nApache License\nVersion 2.0\n```\n\
             \n## bare 0.3.0 (no licence declared)\n\
             \n## zeta 2.0.0 (MIT OR Apache-2.0)\nhttps://example.org/zeta\n\
             \n### LICENSE-APACHE: as alpha 1.0.0\n\n### LICENSE-MIT\n\n````text\n## MIT\n``` (c) zeta\n````\n"
        );
        assert_eq!(text(&meta, tree, &texts), t, "the same bytes each time");
    }

    #[test]
    fn a_build_is_handed_its_workspaces_credits_written_once() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).unwrap();
        let target = tempfile::tempdir().unwrap();
        let host = crate::platform::host().unwrap().triple;
        let mut cargo = Command::new("cargo");
        let at = stamp(&mut cargo, root, target.path(), "aio-rs-config", host, &[]).unwrap();
        let handed = cargo.get_envs().find(|(k, _)| *k == "AIORT_CREDITS").and_then(|(_, v)| v);
        assert_eq!(handed, Some(at.as_os_str()));
        let text = std::fs::read_to_string(&at).unwrap();
        assert!(text.contains("\n## serde 1.") && text.contains("(MIT OR Apache-2.0)\n"), "{text}");
        assert!(text.contains("### LICENSE-APACHE\n\n```text\n") && text.contains("### LICENSE-MIT"), "{text}");
        assert!(!text.contains("## aio-rs-"), "the workspace's own crates are the app");
        assert!(!text.contains("## syn ") && !text.contains("## serde_derive "), "a proc-macro links nothing");
        let written = std::fs::metadata(&at).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        stamp(&mut cargo, root, target.path(), "aio-rs-config", host, &[]).unwrap();
        assert_eq!(std::fs::metadata(&at).unwrap().modified().unwrap(), written, "unchanged: not rebuilt");
    }

    #[test]
    fn licence_files_are_found_by_name_and_by_the_manifest() {
        let d = tempfile::tempdir().unwrap();
        for f in ["LICENSE-MIT", "licence.txt", "COPYING", "NOTICE", "README.md", "src.rs"] {
            std::fs::write(d.path().join(f), f).unwrap();
        }
        std::fs::create_dir(d.path().join("legal")).unwrap();
        std::fs::write(d.path().join("legal/TERMS"), "terms").unwrap();
        let mut got: Vec<String> = files(d.path(), Some("legal/TERMS")).into_iter().map(|(n, _)| n).collect();
        got.sort();
        assert_eq!(got, ["COPYING", "LICENSE-MIT", "NOTICE", "legal/TERMS", "licence.txt"]);
    }
}

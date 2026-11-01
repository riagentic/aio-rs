//! An app made before aio's slim round still builds as it is; `ax lint`
//! names what is left of its old layout and `ax lint --fix` moves it to
//! today's (`ax create`'s): no launcher `build.rs` (`aio::config!()` reads
//! `aio.toml` itself), no build-dependency on aio-rs (one host build of aio,
//! SQLite, ring and tokio less), no `testing`/`video` features (empty now),
//! `aio::config!()` without `?` (it is the config, checked as it built),
//! and the paths aio renamed before its freeze ([`RENAMED`]).
//! Line edits, so a manifest keeps its comments and order.
use super::{Issue, Severity};

/// The release whose layout every rule here moves an app to (`ax lint
/// --from`: a rule newer than the version named is said).
pub const SINCE: &str = "0.1.0";

/// What the old layout's issues say to do.
const FIX: &str = "ax lint --fix";

/// A `build.rs` that only parses `aio.toml` (`aio::config::build(..)`).
pub fn old_build_rs(text: &str) -> bool {
    let code: String =
        text.lines().map(|l| l.split("//").next().unwrap_or_default()).collect::<String>().split_whitespace().collect();
    code.strip_prefix("fnmain(){aio::config::build(")
        .and_then(|r| r.strip_suffix(");}"))
        .is_some_and(|arg| !arg.contains([';', '{', '}']))
}

/// A manifest line that depends on aio-rs.
fn on_aio(line: &str) -> bool {
    line.contains("package = \"aio-rs\"") || line.trim_start().starts_with("aio-rs ")
}

const OLD_FEATURES: [&str; 2] = ["\"testing\"", "\"video\""];

/// `aio::config!()` as it was: a `Result` (it read `AIORT_CONFIG`).
const OLD_CONFIG: &str = "aio::config!()?";

/// Paths aio renamed, old → new: `--fix` rewrites them.
pub const RENAMED: [(&str, &str); 8] = [
    ("aio::hold::deciding()", "aio::deciding::start()"),
    ("aio::hold::Hold", "aio::deciding::Deciding"),
    ("aio::effect_key", "aio::effect::key"),
    ("aio::retry", "aio::effect::retry"),
    ("aio::CALL_CEILING", "aio::effect::CALL_CEILING"),
    ("aio::MaybeSend", "aio::effect::MaybeSend"),
    ("aio::testing::Call", "aio::testing::RandomCall"),
    ("aio::fns::Caller", "aio::fns::Req"),
];

/// `line` with each whole-path `old` (not a longer name's start or end)
/// made `new`; `None` when it has none (pure).
fn swap(line: &str, old: &str, new: &str) -> Option<String> {
    let name = |c: char| c.is_alphanumeric() || c == '_';
    let (mut out, mut at, mut hit) = (String::new(), 0, false);
    for (i, _) in line.match_indices(old) {
        if i < at || line[i + old.len()..].starts_with(name) || line[..i].ends_with(name) {
            continue;
        }
        out += &line[at..i];
        out += new;
        at = i + old.len();
        hit = true;
    }
    hit.then(|| out + &line[at..])
}

/// `text` with every [`RENAMED`] path rewritten (pure).
pub fn rename(text: &str) -> String {
    let lines = text
        .split_inclusive('\n')
        .map(|l| RENAMED.iter().fold(l.to_string(), |l, (old, new)| swap(&l, old, new).unwrap_or(l)));
    lines.collect()
}

/// A `type_text`/`set_value` whose text has a `\n`: once Enter, now text (pure).
fn typed_newline(line: &str) -> bool {
    [".type_text(", ".set_value("].iter().any(|c| line.split_once(c).is_some_and(|(_, args)| args.contains("\\n")))
}

/// The findings for a file of the project (pure).
pub fn check(rel: &str, text: &str, out: &mut Vec<Issue>) {
    let name = rel.rsplit('/').next().unwrap_or_default();
    if name == "build.rs" && old_build_rs(text) {
        let msg = "this build.rs only parses aio.toml, which aio::config!() now does itself".into();
        out.push(Issue::new(Severity::Warn, "migrate", msg, rel, 1).fix(FIX));
    }
    if name.ends_with(".rs") {
        for (i, _) in text.lines().enumerate().filter(|(_, l)| l.contains(OLD_CONFIG)) {
            let msg = "aio::config!() is the config now, no Result: drop the `?`".into();
            out.push(Issue::new(Severity::Error, "migrate", msg, rel, i + 1).fix(FIX));
        }
        for (i, line) in text.lines().enumerate() {
            for (old, new) in RENAMED.iter().filter(|(old, new)| swap(line, old, new).is_some()) {
                let mut msg = format!("`{old}` is `{new}` now");
                if *old == "aio::fns::Caller" {
                    msg += ": its user and role are `req.who` (`req.who.user()`, `req.who.role`), a Caller";
                }
                out.push(Issue::new(Severity::Error, "migrate", msg, rel, i + 1).fix(FIX));
            }
            if typed_newline(line) {
                let msg =
                    "TestUi types `\\n` as text now, no Enter: end with `ui.press(t, \"Enter\")` to submit".into();
                out.push(Issue::new(Severity::Warn, "migrate", msg, rel, i + 1));
            }
        }
    }
    if name != "Cargo.toml" {
        return;
    }
    let mut section = "";
    for (i, line) in text.lines().enumerate() {
        let t = line.trim();
        if t.starts_with('[') {
            section = t;
            continue;
        }
        if !on_aio(line) {
            continue;
        }
        if section == "[build-dependencies]" {
            let msg = "a build-dependency on aio-rs builds it a second time, for the host; nothing needs it now".into();
            out.push(Issue::new(Severity::Warn, "migrate", msg, rel, i + 1).fix(FIX));
        } else if let Some(f) = OLD_FEATURES.iter().find(|f| line.contains(**f)) {
            let msg = format!("aio-rs's {f} feature is empty now: TestUi and video come with `kit`");
            out.push(Issue::new(Severity::Warn, "migrate", msg, rel, i + 1).fix(FIX));
        }
    }
}

/// `text` (a manifest) in today's layout, or `None` when it already is
/// (pure): no aio-rs build-dependency, no old features, a dev-dependency
/// that only added them gone, and a section left empty gone with the
/// comment above it.
pub fn migrate_manifest(text: &str) -> Option<String> {
    let has_dep = {
        let mut s = "";
        text.lines().any(|l| {
            if l.trim().starts_with('[') {
                s = l.trim();
            }
            s == "[dependencies]" && on_aio(l)
        })
    };
    let mut out: Vec<String> = Vec::new();
    let mut section = String::new();
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            section = t.into();
            out.push(line.into());
            continue;
        }
        if !on_aio(line) {
            out.push(line.into());
            continue;
        }
        if section == "[build-dependencies]" {
            continue;
        }
        let mut l = line.to_string();
        for f in OLD_FEATURES {
            l = l.replace(&format!(", {f}"), "").replace(&format!("{f}, "), "").replace(f, "");
        }
        l = l.replace(", features = []", "");
        let adds_nothing = !l.contains(" features =") && l.contains("default-features = false");
        if section == "[dev-dependencies]" && has_dep && adds_nothing {
            continue;
        }
        out.push(l);
    }
    let out = drop_empty_sections(out);
    let mut new = out.join("\n");
    if text.ends_with('\n') {
        new.push('\n');
    }
    (new != text).then_some(new)
}

/// `lines` without a dependency section that holds nothing, nor the
/// comment right above it, nor a blank line doubled where it was.
fn drop_empty_sections(lines: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim();
        let deps = t.ends_with("dependencies]") && t.starts_with('[');
        let end = lines[i + 1..].iter().position(|l| l.trim().starts_with('[')).map_or(lines.len(), |p| i + 1 + p);
        if deps && lines[i + 1..end].iter().all(|l| l.trim().is_empty() || l.trim().starts_with('#')) {
            while out.last().is_some_and(|l| l.trim().starts_with('#')) {
                out.pop();
            }
            // The comments below it belong to the next section: keep them.
            let keep: Vec<String> = lines[i + 1..end].iter().skip_while(|l| l.trim().is_empty()).cloned().collect();
            if !keep.is_empty() && out.last().is_some_and(|l| !l.trim().is_empty()) {
                out.push(String::new());
            }
            out.extend(keep);
            i = end;
            continue;
        }
        out.push(lines[i].clone());
        i += 1;
    }
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    out
}

/// Move the project at `dir` to today's layout: what was done, one line each.
pub fn fix(dir: &std::path::Path, files: &[std::path::PathBuf]) -> Result<Vec<String>, String> {
    let mut did = vec![];
    for path in files {
        let rel = path.strip_prefix(dir).unwrap_or(path).to_string_lossy().replace('\\', "/");
        let name = rel.rsplit('/').next().unwrap_or_default();
        let err = |e: std::io::Error| format!("{rel}: {e}");
        if name == "build.rs" && std::fs::read_to_string(path).is_ok_and(|t| old_build_rs(&t)) {
            std::fs::remove_file(path).map_err(err)?;
            did.push(format!("removed {rel} (aio::config!() reads aio.toml itself)"));
        } else if name == "Cargo.toml"
            && let Some(new) = std::fs::read_to_string(path).ok().as_deref().and_then(migrate_manifest)
        {
            std::fs::write(path, new).map_err(err)?;
            did.push(format!("{rel}: dropped the aio-rs build-dependency and the empty testing/video features"));
        } else if name.ends_with(".rs")
            && let Ok(t) = std::fs::read_to_string(path)
        {
            let new = rename(&t.replace(OLD_CONFIG, "aio::config!()"));
            if new != t {
                std::fs::write(path, new).map_err(err)?;
                did.push(format!("{rel}: aio::config!() without `?`, renamed paths moved"));
            }
        }
    }
    Ok(did)
}

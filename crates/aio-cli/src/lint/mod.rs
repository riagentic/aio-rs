//! `ax lint [dir]` (aio: aiol): what the compiler and the macros can't
//! enforce. Every aio check maps to one of three:
//!
//! - **compile time**, no check needed: the cell is a typed struct (state
//!   interface, reserved and colliding keys, empty collections), methods
//!   take `&mut self` and return `aio::Result` (draft escape, thrown errors,
//!   post-await reads, mixed method styles), imports and the client/server
//!   split are resolved by rustc (import graph, browser imports, legacy
//!   paths, Node APIs, `any`), cells reach each other only through typed
//!   handles (inter-cell reads), aio::ui signals are handles (stale tuples,
//!   memo/projection), `Effect` is `#[must_use]` (a dropped effect warns,
//!   an error under `-D warnings`);
//! - **clippy**, run here with aio's lints: unused imports, deprecated
//!   spellings, `dbg!` left in, `todo!()`/`unimplemented!()` shipped;
//! - **aio checks**, below: credentials in source and assets, credential-named
//!   cell fields every client gets, impure methods (I/O, clock, randomness,
//!   threads), hooks off a component's top level, rsx! markup without
//!   accessible names, duplicate cell ids, cells no test names, `aio.toml`,
//!   `.env` files git would commit.
//!
//! `// aio-ok: <why>` (or `aio-ok(<area>): <why>`) on the line or in the
//! comment block right above silences one finding; a reason is required.
//! Exit: 0 clean (warnings and hints don't fail), 1 errors, 2 bad usage.
mod layout;
mod rust;
mod secrets;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warn,
    Hint,
}

impl Severity {
    fn name(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warn => "warn",
            Severity::Hint => "hint",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    pub severity: Severity,
    pub area: &'static str,
    pub message: String,
    pub file: Option<String>,
    pub line: Option<usize>,
    pub fix: Option<String>,
}

impl Issue {
    pub fn new(severity: Severity, area: &'static str, message: String, file: &str, line: usize) -> Self {
        let line = (line > 0).then_some(line);
        Self { severity, area, message, file: Some(file.into()), line, fix: None }
    }

    pub fn fix(mut self, fix: &str) -> Self {
        self.fix = Some(fix.into());
        self
    }
}

#[derive(Debug, Default)]
pub struct Report {
    pub issues: Vec<Issue>,
    pub passed: Vec<String>,
    pub files: usize,
    pub cells: usize,
    pub tests: usize,
}

/// `aio-ok`, an optional `(area,…)` that names this finding's area, then `:` and a reason.
fn marker(line: &str, area: &str) -> bool {
    line.match_indices("aio-ok").any(|(at, _)| {
        let rest = &line[at + 6..];
        let (scope, rest) = match rest.strip_prefix('(').and_then(|r| r.split_once(')')) {
            Some((s, r)) => (Some(s), r),
            None => (None, rest),
        };
        let word = !line[..at].ends_with(|c: char| c.is_alphanumeric() || c == '-');
        let reason = rest.trim_start().strip_prefix(':').is_some_and(|r| !r.trim().is_empty());
        word && reason && scope.is_none_or(|s| s.split(',').any(|x| x.trim() == area))
    })
}

/// Is the finding on line `idx` (0-based) justified, on the line or in the
/// comment and attribute block right above it? A blank line ends the block.
pub fn suppressed(lines: &[&str], idx: usize, area: &str) -> bool {
    if lines.get(idx).is_some_and(|l| marker(l, area)) {
        return true;
    }
    lines[..idx.min(lines.len())]
        .iter()
        .rev()
        .map(|l| l.trim())
        .take_while(|t| ["//", "#", "*", "/*", "<!--"].iter().any(|p| t.starts_with(p)))
        .any(|t| marker(t, area))
}

/// Under `tests/` or `benches/`, or a `tests.rs`: test code, not app code.
fn is_test_path(rel: &str) -> bool {
    let parts: Vec<&str> = rel.split('/').collect();
    parts[..parts.len() - 1].iter().any(|p| *p == "tests" || *p == "benches")
        || parts.last().is_some_and(|f| *f == "tests.rs" || f.ends_with("_test.rs") || f.ends_with("_tests.rs"))
}

/// Every file under `dir`, sorted; build output, VCS and dot dirs left out,
/// symlinks not followed.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        match e.file_type() {
            Ok(t) if t.is_dir() => {
                if !(name.starts_with('.') || name.starts_with("target") || name == "node_modules" || name == "dist") {
                    walk(&e.path(), out);
                }
            }
            Ok(t) if t.is_file() => out.push(e.path()),
            _ => {}
        }
    }
}

const MAX: u64 = 1 << 20;
const TEXT: [&str; 12] = ["rs", "toml", "json", "js", "mjs", "ts", "css", "html", "md", "txt", "yaml", "yml"];

/// Was `.env` at `rel` kept out of git? `git check-ignore`, else `.gitignore`.
fn env_ignored(dir: &Path, rel: &str) -> bool {
    match Command::new("git").arg("-C").arg(dir).args(["check-ignore", "-q", rel]).status().map(|s| s.code()) {
        Ok(Some(0)) => true,
        Ok(Some(1)) => false,
        _ => std::fs::read_to_string(dir.join(".gitignore")).is_ok_and(|g| secrets::ignores_env(&g)),
    }
}

/// Every aio check (no clippy) of the project at `dir`.
pub fn lint(dir: &Path) -> Report {
    let mut paths = Vec::new();
    walk(dir, &mut paths);
    let (mut facts, mut report) = (rust::Facts::default(), Report::default());
    let mut texts: HashMap<String, String> = HashMap::new();
    for path in paths {
        let rel = path.strip_prefix(dir).unwrap_or(&path).to_string_lossy().replace('\\', "/");
        let name = rel.rsplit('/').next().unwrap_or_default().to_string();
        let ext = name.rsplit_once('.').map_or("", |(_, e)| e);
        if secrets::is_env(&name) {
            if !env_ignored(dir, &rel) {
                facts.issues.push(
                    Issue::new(Severity::Warn, "security", format!("{rel} would be committed"), &rel, 0)
                        .fix("add .env to .gitignore"),
                );
            }
            continue;
        }
        if name == "Cargo.lock" {
            continue;
        }
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX) {
            if TEXT.contains(&ext) {
                facts.issues.push(Issue::new(Severity::Hint, "scan", "not checked: larger than 1 MiB".into(), &rel, 0));
            }
            continue;
        }
        let Ok(text) = std::fs::read(&path).map(String::from_utf8) else { continue };
        let Ok(text) = text else { continue }; // not text: an image, a font
        report.files += 1;
        layout::check(&rel, &text, &mut facts.issues);
        if ext == "rs" {
            rust::file(&rel, &text, is_test_path(&rel), &mut facts);
        } else if !is_test_path(&rel) {
            secrets::text(&rel, &text, &mut facts.issues);
            if name == "Cargo.toml" {
                rust::manifest(&rel, &text, &mut facts.issues);
            }
            if name == "aio.toml" {
                match aio::config::Config::parse(&text) {
                    Ok(c) => report.passed.push(format!("{rel}: app {}", c.app.id)),
                    Err(e) => facts.issues.push(Issue::new(Severity::Error, "config", e, &rel, 0)),
                }
            }
        }
        texts.insert(rel, text);
    }
    across(&mut facts, &mut report);
    // One place for every `aio-ok`.
    facts.issues.retain(|i| {
        let (Some(f), Some(l)) = (&i.file, i.line) else { return true };
        let lines: Vec<&str> = texts.get(f).map_or(Vec::new(), |t| t.lines().collect());
        !suppressed(&lines, l - 1, i.area)
    });
    facts.issues.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    report.issues = facts.issues;
    report.cells = facts.cells.len();
    report.tests = facts.tests;
    report
}

/// The checks that need every file: duplicate cell ids, untested cells.
fn across(f: &mut rust::Facts, report: &mut Report) {
    let mut by_id: BTreeMap<&str, &rust::Cell> = BTreeMap::new();
    for c in &f.cells {
        if let Some(first) = by_id.get(c.id.as_str()) {
            f.issues.push(
                Issue::new(
                    Severity::Error,
                    "cells",
                    format!(
                        "cell `{}` has the id `{}` of `{}` ({}:{}): they would share one store row",
                        c.name, c.id, first.name, first.file, first.line
                    ),
                    &c.file,
                    c.line,
                )
                .fix("give one its own: #[cell(id = \"…\")]"),
            );
        } else {
            by_id.insert(&c.id, c);
        }
    }
    let untested: Vec<&rust::Cell> = f.cells.iter().filter(|c| !f.tested.contains(&c.name)).collect();
    for c in &untested {
        f.issues.push(
            Issue::new(Severity::Hint, "testing", format!("no test names cell `{}`", c.name), &c.file, c.line)
                .fix("test its methods with Handle::<Cell>::open on a temp store, or aio::testing"),
        );
    }
    if !f.cells.is_empty() {
        let names: Vec<&str> = f.cells.iter().map(|c| c.name.as_str()).collect();
        report.passed.push(format!("{} cell(s): {}", names.len(), names.join(", ")));
        if untested.is_empty() {
            report.passed.push(format!("all {} cells are tested", names.len()));
        }
    }
}

/// The lints aio adds to the project's own clippy set: debugging and
/// unfinished code that must not ship.
const LINTS: [&str; 6] = ["-W", "clippy::dbg_macro", "-D", "clippy::todo", "-D", "clippy::unimplemented"];

/// Findings from `cargo clippy --message-format=json` output: errors and
/// warnings in the project's files, each once (all targets repeat them).
pub fn clippy_issues(stdout: &str, root: &Path, dir: &Path) -> Vec<Issue> {
    let mut out: Vec<Issue> = Vec::new();
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if v["reason"] != "compiler-message" {
            continue;
        }
        let m = &v["message"];
        let severity = match m["level"].as_str() {
            Some("error") => Severity::Error,
            Some("warning") => Severity::Warn,
            _ => continue,
        };
        let spans = m["spans"].as_array().map(Vec::as_slice).unwrap_or_default();
        let Some(span) = spans.iter().find(|s| s["is_primary"] == true) else { continue };
        let file = root.join(span["file_name"].as_str().unwrap_or_default());
        let Ok(rel) = file.strip_prefix(dir) else { continue };
        let code = m["code"]["code"].as_str().unwrap_or("");
        let issue = Issue {
            severity,
            area: if code.starts_with("clippy::") { "clippy" } else { "rustc" },
            message: m["message"].as_str().unwrap_or_default().to_string(),
            file: Some(rel.to_string_lossy().replace('\\', "/")),
            line: span["line_start"].as_u64().map(|l| l as usize),
            fix: (!code.is_empty()).then(|| format!("see {code}")),
        };
        if !out.contains(&issue) {
            out.push(issue);
        }
    }
    out
}

/// Run clippy on the project (with `--fix`: apply its machine-applicable fixes).
fn clippy(dir: &Path, fix: bool) -> Result<Vec<Issue>, String> {
    let ws = Command::new("cargo")
        .args(["locate-project", "--workspace", "--message-format", "plain"])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("cargo: {e}"))?;
    let root = PathBuf::from(String::from_utf8_lossy(&ws.stdout).trim());
    let root = root.parent().ok_or("no Cargo.toml here: clippy not run")?;
    let mut c = Command::new("cargo");
    c.current_dir(dir).arg("clippy").arg("--all-targets").args(crate::dev::layout());
    if fix {
        // Denied lints would stop the fixing: here they only warn.
        c.args(["--fix", "--allow-dirty", "--allow-staged", "--"])
            .args(LINTS.map(|a| if a == "-D" { "-W" } else { a }));
        let s = c.stdout(Stdio::inherit()).stderr(Stdio::inherit()).status().map_err(|e| e.to_string())?;
        return if s.success() { Ok(Vec::new()) } else { Err("cargo clippy --fix failed".into()) };
    }
    let out = c.arg("--message-format=json").arg("--").args(LINTS).stderr(Stdio::piped()).output();
    let out = out.map_err(|e| format!("cargo clippy: {e}"))?;
    let issues = clippy_issues(&String::from_utf8_lossy(&out.stdout), root, dir);
    if !out.status.success() && !issues.iter().any(|i| i.severity == Severity::Error) {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = err.lines().rev().take(5).collect();
        return Err(tail.into_iter().rev().collect::<Vec<_>>().join("\n"));
    }
    Ok(issues)
}

fn summary(r: &Report) -> (usize, usize, usize) {
    let n = |s| r.issues.iter().filter(|i| i.severity == s).count();
    (n(Severity::Error), n(Severity::Warn), n(Severity::Hint))
}

/// The report as JSON, in aio's shape (hints always in).
pub fn json(r: &Report) -> serde_json::Value {
    let (errors, warnings, hints) = summary(r);
    let issues: Vec<serde_json::Value> = r
        .issues
        .iter()
        .map(|i| {
            let mut v = serde_json::json!({"severity": i.severity.name(), "area": i.area, "message": i.message});
            for (k, x) in [
                ("file", i.file.clone().map(Into::into)),
                ("line", i.line.map(Into::into)),
                ("fix", i.fix.clone().map(Into::into)),
            ] {
                if let Some(x) = x {
                    v[k] = x;
                }
            }
            v
        })
        .collect();
    serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "stats": {"filesScanned": r.files, "cellsFound": r.cells, "testsFound": r.tests},
        "passed": r.passed,
        "issues": issues,
        "summary": {"errors": errors, "warnings": warnings, "hints": hints},
    })
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The report for people; `hide_hints` leaves hint lines out but counts them.
pub fn human(r: &Report, dir: &Path, hide_hints: bool) -> String {
    let mut out = vec![format!("ax lint {} · {}\n", env!("CARGO_PKG_VERSION"), dir.display())];
    out.extend(r.passed.iter().map(|p| format!("  ✓ {p}")));
    for i in r.issues.iter().filter(|i| !(hide_hints && i.severity == Severity::Hint)) {
        let mark = match i.severity {
            Severity::Error => "✗",
            Severity::Warn => "⚠",
            Severity::Hint => "·",
        };
        let at = match (&i.file, i.line) {
            (Some(f), Some(l)) => format!("{f}:{l}  "),
            (Some(f), None) => format!("{f}  "),
            _ => String::new(),
        };
        out.push(format!("  {mark} {} {at}[{}] {}", i.severity.name(), i.area, i.message));
        out.extend(i.fix.as_ref().map(|f| format!("      fix: {f}")));
    }
    let (e, w, h) = summary(r);
    out.push(format!("\n  Files: {}  Cells: {}  Tests: {}", r.files, r.cells, r.tests));
    let hidden = if hide_hints && h > 0 { " (hidden: run without --no-hints)" } else { "" };
    out.push(format!(
        "  {} · {} · {}{hidden}",
        plural(e, "error", "errors"),
        plural(w, "warning", "warnings"),
        plural(h, "hint", "hints")
    ));
    out.join("\n")
}

/// The nearest directory at or above `dir` with a Cargo.toml; `dir` when
/// none is, stopping at a repository root and below HOME.
pub fn project_root(dir: &Path) -> PathBuf {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from);
    for d in dir.ancestors() {
        if Some(d) == home.as_deref() {
            break;
        }
        if d.join("Cargo.toml").is_file() {
            return d.into();
        }
        if d.join(".git").exists() {
            break;
        }
    }
    dir.into()
}

const FLAGS: [&str; 7] = ["--json", "--no-hints", "--quiet-hints", "--no-clippy", "--safe-fix", "--fix", "--help"];

const USAGE: &str = "ax lint [dir] [--json] [--no-hints|--quiet-hints] [--no-clippy] [--safe-fix] [--fix]

  dir          the project (the nearest Cargo.toml at or above it; default: here)
  --json       one JSON document on stdout (hints always in)
  --no-hints   leave hint lines out (still counted); --quiet-hints is the same
  --no-clippy  only aio's own checks (fast: no build)
  --safe-fix   apply clippy's machine-applicable fixes first, then lint what is left
  --fix        move an app of the older layout to today's (ax create's) first: no launcher build.rs,
               no aio-rs build-dependency, no testing/video features; then lint

Exit: 0 no errors (warnings and hints don't fail), 1 errors, 2 bad usage.
Silence one finding with a reason: // aio-ok: <why>, or // aio-ok(<area>): <why>.";

pub fn main(args: &[String]) -> ExitCode {
    let (flags, pos): (Vec<&str>, Vec<&str>) = args.iter().map(String::as_str).partition(|a| a.starts_with('-'));
    let unknown: Vec<&&str> = flags.iter().filter(|f| !FLAGS.contains(f) && **f != "-h").collect();
    if !unknown.is_empty() || pos.len() > 1 {
        let what = unknown
            .iter()
            .map(|f| format!("unknown flag {f}"))
            .chain((pos.len() > 1).then(|| "one dir at most".into()));
        eprintln!("ax lint: {}\n\n{USAGE}", what.collect::<Vec<_>>().join(", "));
        return ExitCode::from(2);
    }
    let has = |f: &str| flags.contains(&f);
    if has("--help") || has("-h") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let asked = PathBuf::from(pos.first().copied().unwrap_or("."));
    let Ok(abs) = std::fs::canonicalize(&asked) else {
        eprintln!("ax lint: directory not found: {}", asked.display());
        return ExitCode::FAILURE;
    };
    if !abs.is_dir() {
        eprintln!("ax lint: {} is a file: ax lint checks a project (the directory with Cargo.toml)", asked.display());
        return ExitCode::from(2);
    }
    let dir = project_root(&abs);
    let json_out = has("--json");
    // Under --json, stdout is the one document: narration goes to stderr.
    let say = |s: &str| if json_out { eprintln!("{s}") } else { println!("{s}") };
    if dir != abs {
        say(&format!("ax lint: checking {} (the nearest Cargo.toml)", dir.display()));
    }
    if has("--fix") {
        let mut files = Vec::new();
        walk(&dir, &mut files);
        match layout::fix(&dir, &files) {
            Ok(did) if did.is_empty() => say("ax lint: the layout is today's; nothing to move"),
            Ok(did) => did.iter().for_each(|d| say(&format!("ax lint: {d}"))),
            Err(e) => {
                eprintln!("ax lint --fix: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    let clippy_on = !has("--no-clippy") && dir.join("Cargo.toml").is_file();
    if has("--safe-fix") && clippy_on {
        say("ax lint: applying clippy's safe fixes");
        if let Err(e) = clippy(&dir, true) {
            say(&format!("ax lint: {e}"));
        }
    }
    let mut report = lint(&dir);
    if clippy_on {
        match clippy(&dir, false) {
            Ok(mut i) => {
                report.passed.push(if i.is_empty() { "clippy: clean".into() } else { "clippy ran".into() });
                report.issues.append(&mut i);
            }
            Err(e) => report.issues.push(Issue {
                severity: Severity::Error,
                area: "clippy",
                message: format!("clippy didn't run, so nothing it checks was checked: {e}"),
                file: None,
                line: None,
                fix: Some("cargo clippy --all-targets says why".into()),
            }),
        }
    }
    report.issues.sort_by(|a, b| (a.severity, &a.file, a.line).cmp(&(b.severity, &b.file, b.line)));
    if json_out {
        println!("{}", json(&report));
    } else {
        println!("{}", human(&report, &dir, has("--no-hints") || has("--quiet-hints")));
    }
    if summary(&report).0 > 0 { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

#[cfg(test)]
mod tests;

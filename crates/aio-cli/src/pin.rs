//! `ax pin` (aio's `am pin`): which aio-rs an app uses, and how to change it.
//!
//! ```text
//! ax pin                  what the app's Cargo.toml files ask for, what its
//!                         Cargo.lock built, this ax, and the releases
//! ax pin 0.3.0            every aio-rs dependency to that release, Cargo.lock with it
//! ax pin main             follow the branch tip (a moving target)
//! ax pin latest           the newest release in the app's major (--major: any)
//! ax pin ../aio-rs        an aio-rs checkout on this machine (local development)
//! ```
//!
//! The pin is the app's Cargo.toml files (aio's `deno.json` `aioVersion`),
//! committed with the code; Cargo.lock holds the exact commit, so a clone
//! builds against the aio-rs it was written for.
use crate::create::{self, Aio};
use serde_json::{Value, json};
use std::path::Path;

/// Where one aio-rs dependency, or one Cargo.lock entry, takes aio-rs from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Src {
    Tag(String),
    Branch(String),
    Rev(String),
    /// Git, its default branch.
    Git,
    /// A checkout; in Cargo.toml its `crates` folder relative to the app's
    /// root (Cargo.lock names no path: empty).
    Path(String),
    /// crates.io, by version requirement (Cargo.toml) or version (Cargo.lock).
    Registry(String),
}

impl Src {
    /// The same pin (a lock entry names no path, so any path is the path).
    fn same(&self, o: &Src) -> bool {
        match (self, o) {
            (Src::Path(_), Src::Path(_)) => true,
            // Cargo.toml's requirement, Cargo.lock's version cargo picked for it.
            (Src::Registry(_), Src::Registry(_)) => true,
            _ => self == o,
        }
    }

    /// As Cargo.lock's entry says it: `tag v0.1.0`, `branch main`, `a checkout`.
    fn locked(&self) -> String {
        match self {
            Src::Git => "git".into(),
            Src::Path(_) => "a checkout".into(),
            Src::Registry(_) => "crates.io".into(),
            s => format!("{} {}", s.kind(), s.name()),
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Src::Tag(_) => "tag",
            Src::Branch(_) => "branch",
            Src::Rev(_) => "rev",
            Src::Git => "git",
            Src::Path(_) => "path",
            Src::Registry(_) => "crates.io",
        }
    }

    fn name(&self) -> &str {
        match self {
            Src::Tag(s) | Src::Branch(s) | Src::Rev(s) | Src::Path(s) | Src::Registry(s) => s,
            Src::Git => "",
        }
    }
}

impl std::fmt::Display for Src {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Src::Tag(t) => write!(f, "{t}"),
            Src::Branch(b) => write!(f, "{b} (a branch)"),
            Src::Rev(r) => write!(f, "rev {r}"),
            Src::Git => write!(f, "git, its default branch"),
            Src::Path(p) if p.is_empty() => write!(f, "a checkout"),
            Src::Path(p) => write!(f, "{p} (a checkout)"),
            Src::Registry(v) => write!(f, "{v} (crates.io)"),
        }
    }
}

/// `key = "value"` in a dependency's inline table `line` (pure).
fn value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("{key} = \"");
    let at = line.match_indices(&pat).map(|(i, _)| i).find(|&i| line[..i].ends_with([' ', '{']))? + pat.len();
    line[at..].find('"').map(|e| &line[at..at + e])
}

/// The aio-rs package a Cargo.toml line depends on and where from, `up`
/// folders below the app's root (pure): `aio = { package = "aio-rs", … }`
/// or `aio-rs = { … }`, as one line; None for any other line (and for
/// `workspace = true`: the workspace's line says it).
pub fn asked(line: &str, up: usize) -> Option<(String, Src)> {
    let t = line.trim_start();
    let key = t.split([' ', '=']).next()?;
    let package = value(t, "package").unwrap_or(key);
    if t.starts_with('#') || !package.starts_with("aio-rs") || !t.contains('{') || t.contains("workspace = true") {
        return None;
    }
    let src = if let Some(p) = value(t, "path") {
        let p = p.strip_prefix(&"../".repeat(up)).unwrap_or(p);
        Src::Path(p.rsplit_once('/').map_or(".", |(d, _)| d).to_string())
    } else if value(t, "git").is_some() {
        match (value(t, "tag"), value(t, "branch"), value(t, "rev")) {
            (Some(x), ..) => Src::Tag(x.into()),
            (_, Some(x), _) => Src::Branch(x.into()),
            (.., Some(x)) => Src::Rev(x.into()),
            _ => Src::Git,
        }
    } else {
        Src::Registry(value(t, "version")?.into())
    };
    Some((package.into(), src))
}

/// One aio-rs crate in Cargo.lock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Locked {
    pub name: String,
    pub version: String,
    pub src: Src,
    /// The commit, for git.
    pub commit: Option<String>,
}

/// Cargo.lock's aio-rs crates (pure).
pub fn locked(lock: &str) -> Vec<Locked> {
    lock.split("[[package]]")
        .skip(1)
        .filter_map(|p| {
            let field = |k: &str| {
                p.lines().find_map(|l| l.strip_prefix(&format!("{k} = \""))?.strip_suffix('"').map(String::from))
            };
            let (name, version) = (field("name")?, field("version")?);
            if !name.starts_with("aio-rs") || name == "aio-rs-cli" {
                return None;
            }
            let (src, commit) = match field("source") {
                None => (Src::Path(String::new()), None),
                Some(s) if s.starts_with("git+") => {
                    let (url, commit) = s.split_once('#').map_or((s.as_str(), None), |(u, c)| (u, Some(c.into())));
                    let q = url.split_once('?').map(|(_, q)| q).unwrap_or("");
                    let src = q.split('&').find_map(|kv| match kv.split_once('=')? {
                        ("tag", v) => Some(Src::Tag(v.into())),
                        ("branch", v) => Some(Src::Branch(v.into())),
                        ("rev", v) => Some(Src::Rev(v.into())),
                        _ => None,
                    });
                    (src.unwrap_or(Src::Git), commit)
                }
                Some(_) => (Src::Registry(version.clone()), None),
            };
            Some(Locked { name, version, src, commit })
        })
        .collect()
}

/// aio-rs's release tags in `git ls-remote --tags` text, newest first (pure).
pub fn releases(ls_remote: &str) -> Vec<String> {
    let mut tags: Vec<String> = ls_remote
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1)?.strip_prefix("refs/tags/"))
        .filter(|t| t.starts_with('v') && !t.ends_with("^{}") && aio::update::parse_version(t).is_some())
        .map(String::from)
        .collect();
    tags.sort_by(|a, b| aio::update::compare(b, a).unwrap_or(std::cmp::Ordering::Equal));
    tags.dedup();
    tags
}

fn major(v: &str) -> Option<u64> {
    aio::update::parse_version(v).map(|(n, _)| n[0])
}

/// The newest release of `tags` (newest first), in major `within` when
/// given; a prerelease only when there is nothing else (pure).
pub fn latest(tags: &[String], within: Option<u64>) -> Option<String> {
    let of: Vec<&String> = tags.iter().filter(|t| within.is_none_or(|m| major(t) == Some(m))).collect();
    let release = |t: &&&String| aio::update::parse_version(t).is_some_and(|(_, pre)| pre.is_none());
    of.iter().find(release).or(of.first()).map(|t| t.to_string())
}

/// Where an app stands: what its Cargo.toml files ask for, what its
/// Cargo.lock built, this ax, and aio-rs's releases (None: not reachable).
#[derive(Debug)]
pub struct Info {
    /// Each aio-rs dependency: its file (relative to the root), package, source.
    pub asks: Vec<(String, String, Src)>,
    pub locked: Vec<Locked>,
    pub lock_file: bool,
    pub ax: String,
    pub releases: Result<Vec<String>, String>,
}

impl Info {
    /// The one source every dependency asks for; None when they disagree.
    pub fn pinned(&self) -> Option<&Src> {
        let first = &self.asks.first()?.2;
        self.asks.iter().all(|(.., s)| s == first).then_some(first)
    }

    /// The `aio-rs` crate Cargo.lock built (else its first aio-rs crate).
    pub fn built(&self) -> Option<&Locked> {
        self.locked.iter().find(|l| l.name == "aio-rs").or(self.locked.first())
    }

    /// Cargo.lock built something the Cargo.toml files didn't ask for, or
    /// its aio-rs crates come from different places.
    pub fn drift(&self) -> bool {
        let (Some(p), Some(b)) = (self.pinned(), self.built()) else { return false };
        !p.same(&b.src) || self.locked.iter().any(|l| l.src != b.src || l.commit != b.commit)
    }

    /// Releases between the pin and the newest one; None when the pin
    /// isn't a release, or the releases aren't known.
    pub fn behind(&self) -> Option<usize> {
        let Some(Src::Tag(t)) = self.pinned() else { return None };
        let tags = self.releases.as_ref().ok()?;
        aio::update::parse_version(t)?;
        Some(tags.iter().filter(|r| aio::update::compare(r, t) == Some(std::cmp::Ordering::Greater)).count())
    }

    fn latest(&self) -> Option<String> {
        latest(self.releases.as_ref().ok()?, None)
    }

    /// Does the version Cargo.lock built match this ax's?
    pub fn matches(&self) -> Option<bool> {
        self.built().map(|b| b.version == self.ax)
    }

    pub fn json(&self) -> Value {
        let src = |s: &Src| json!({ "kind": s.kind(), "name": s.name() });
        json!({
            "pinned": self.pinned().map(src),
            "asks": self.asks.iter().map(|(f, p, s)| json!({ "file": f, "package": p, "source": src(s) })).collect::<Vec<_>>(),
            "built": self.built().map(|b| json!({ "version": b.version, "source": src(&b.src), "commit": b.commit })),
            "locked": self.locked.iter().map(|l| json!({ "name": l.name, "version": l.version, "source": src(&l.src), "commit": l.commit })).collect::<Vec<_>>(),
            "ax": self.ax,
            "matches": self.matches(),
            "drift": self.drift(),
            "latest": self.latest(),
            "behind": self.behind(),
            "releases": self.releases.as_ref().ok(),
            "releasesError": self.releases.as_ref().err(),
        })
    }

    /// What `ax pin` prints (aio's layout: the facts, the warnings, then
    /// what to type).
    pub fn render(&self) -> String {
        let short = |c: &Option<String>| c.as_deref().map_or(String::new(), |c| format!(" @ {}", &c[..c.len().min(7)]));
        let pinned = match self.pinned() {
            Some(p) => p.to_string(),
            None => "mixed (its Cargo.toml files disagree)".into(),
        };
        let files: Vec<&str> = self.asks.iter().map(|(f, ..)| f.as_str()).fold(vec![], |mut v, f| {
            if !v.contains(&f) {
                v.push(f);
            }
            v
        });
        let built = match self.built() {
            Some(b) => format!("{} ({}{})", b.version, b.src.locked(), short(&b.commit)),
            None => "nothing yet".into(),
        };
        let ax = match self.matches() {
            Some(true) => format!("{}  matches", self.ax),
            Some(false) => format!("{}  differs from what the app builds", self.ax),
            None => self.ax.clone(),
        };
        let releases = match &self.releases {
            Ok(r) if r.is_empty() => "none".to_string(),
            Ok(r) if r.len() > 4 => format!("{} +{}", r[..4].join(" "), r.len() - 4),
            Ok(r) => r.join(" "),
            Err(_) => "unknown (aio-rs's repository isn't reachable)".into(),
        };
        let latest = self.latest();
        let rows = [
            ("aio-rs", Some(format!("{pinned}  {}", files.join(", ")))),
            ("built", Some(built)),
            ("ax", Some(ax)),
            ("latest", latest.clone()),
            ("releases", Some(releases)),
        ];
        let mut out = vec![
            rows.iter().filter_map(|(k, v)| Some(format!("  {k:<8}  {}", v.as_ref()?))).collect::<Vec<_>>().join("\n"),
        ];
        let block = |mark: &str, head: String, body: &str, fix: Option<&str>| {
            let mut b = format!("  {mark}  {head}\n     {body}");
            if let Some(f) = fix {
                b.push_str(&format!("\n     fix  {f}"));
            }
            b
        };
        if self.pinned().is_none() {
            out.push(block(
                "!",
                "Mixed — the app's Cargo.toml files ask for different aio-rs.".into(),
                &self.asks.iter().map(|(f, p, s)| format!("{f}: {p} {s}")).collect::<Vec<_>>().join("; "),
                Some("ax pin <version>   (moves every one)"),
            ));
        } else if self.drift() {
            let b = self.built().expect("drift has a build");
            out.push(block(
                "!",
                "Drift — this app is built against something it did not ask for.".into(),
                &format!("Cargo.toml asks for {}; Cargo.lock built {} ({}).", pinned, b.version, b.src.locked()),
                Some("ax pin <what Cargo.toml asks for>   (re-resolves Cargo.lock)"),
            ));
        }
        if !self.lock_file || self.locked.is_empty() {
            out.push(block(
                "!",
                "Nothing is built yet.".into(),
                "There is no Cargo.lock with aio-rs in it, so nothing pins the exact commit.",
                Some("ax build <app>   (or cargo generate-lockfile), then commit Cargo.lock"),
            ));
        }
        if let Some(Src::Path(_)) = self.pinned() {
            out.push(block(
                "!",
                "Local-dev pin — this machine only.".into(),
                "Cargo.toml takes aio-rs from a checkout beside the app: a clone elsewhere doesn't build.",
                Some("ax pin latest"),
            ));
        }
        if let (Some(n @ 1..), Some(l)) = (self.behind(), &latest) {
            out.push(block(
                "i",
                format!("{n} release{} behind {l}.", if n == 1 { "" } else { "s" }),
                "This app keeps building as pinned — nothing changes until you move it.",
                Some("ax pin latest"),
            ));
        }
        if let Err(e) = &self.releases {
            out.push(block("i", "The releases aren't known.".into(), e, None));
        }
        let hints = [
            ("ax pin latest", "the newest release in this app's major"),
            ("ax pin <version>", "switch this app (Cargo.toml and Cargo.lock)"),
            ("ax pin main", "follow the branch tip (a moving target)"),
            ("ax pin <path>", "follow an aio-rs checkout on this machine"),
        ];
        out.push(hints.iter().map(|(c, d)| format!("  {c:<16}  {d}")).collect::<Vec<_>>().join("\n"));
        out.join("\n\n")
    }
}

/// Where the app at `root` stands; `tags`: `git ls-remote --tags` of aio-rs.
pub fn info(root: &Path, tags: impl FnOnce() -> Result<String, String>) -> Result<Info, String> {
    let mut asks = vec![];
    for (file, up) in create::manifests(root) {
        let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        let rel = file.strip_prefix(root).unwrap_or(&file).to_string_lossy().replace('\\', "/");
        asks.extend(text.lines().filter_map(|l| asked(l, up)).map(|(p, s)| (rel.clone(), p, s)));
    }
    if asks.is_empty() {
        return Err(format!("{}: no aio-rs dependency in its Cargo.toml files", root.display()));
    }
    let lock = std::fs::read_to_string(root.join("Cargo.lock")).ok();
    Ok(Info {
        asks,
        locked: lock.as_deref().map(locked).unwrap_or_default(),
        lock_file: lock.is_some(),
        ax: env!("CARGO_PKG_VERSION").into(),
        releases: tags().map(|t| releases(&t)),
    })
}

/// Is `word` what to pin to (a version, `main`, `latest`, an aio-rs
/// checkout), rather than an app?
pub fn is_target(word: &str) -> bool {
    matches!(word, "main" | "latest")
        || aio::update::parse_version(word).is_some()
        || [Path::new(word).join("crates/aio/Cargo.toml"), Path::new(word).join("aio/Cargo.toml")]
            .iter()
            .any(|p| p.is_file())
}

/// What `target` names for the app pinned to `current` (aio's `pinTarget`
/// and `tagForRef`): `latest` within the pin's major unless `cross`; a
/// version the known `tags` don't have is refused (pure but for a path's
/// checkout).
pub fn resolve(
    root: &Path,
    target: &str,
    current: Option<&Src>,
    cross: bool,
    tags: &Result<Vec<String>, String>,
) -> Result<Aio, String> {
    let cur = match current {
        Some(Src::Tag(t)) => major(t),
        _ => None,
    };
    match target {
        "latest" => {
            let tags = tags.as_ref().map_err(|e| format!("latest: aio-rs's releases aren't known: {e}"))?;
            let within = if cross { None } else { cur };
            latest(tags, within).map(Aio::Tag).ok_or_else(|| match within {
                Some(m) => format!(
                    "no releases in the {m}.x line. `ax pin latest --major` crosses to the newest major \
                     (a BREAKING upgrade — read the CHANGELOG)"
                ),
                None => format!("no release tags in {} — use `ax pin main`", create::REPO),
            })
        }
        "main" => Ok(Aio::Branch("main".into())),
        t if aio::update::parse_version(t).is_some() => {
            let aio = Aio::version(t, || Err(String::new()))?;
            match (&aio, tags) {
                (Aio::Tag(tag), Ok(known)) if !known.contains(tag) => Err(format!(
                    "aio-rs version {tag} not found; releases: {}",
                    if known.is_empty() { "none".into() } else { known.join(" ") }
                )),
                _ => Ok(aio),
            }
        }
        dir => Aio::checkout(root, Path::new(dir)),
    }
}

/// The note for a pin that moved back between two releases (aio's
/// `pinnedBackNote`, pure).
pub fn back_note(from: Option<&Src>, to: &Aio) -> Option<String> {
    let (Some(Src::Tag(was)), Aio::Tag(now)) = (from, to) else { return None };
    (aio::update::compare(now, was) == Some(std::cmp::Ordering::Less)).then(|| {
        format!(
            "moved BACK: {was} → {now}. Code written for {was} — by its `ax create`, or by hand since — may use \
             what {now} does not have. `cargo check` names each one: remove it, or return with `ax pin {was}`."
        )
    })
}

/// `ax pin <target>`: every aio-rs dependency of the app at `root` moved to
/// `to`, then `relock` (Cargo.lock): all or nothing. What it did, as text
/// or JSON.
pub fn pin(
    root: &Path,
    requested: &str,
    to: &Aio,
    before: Option<&Src>,
    relock: impl FnOnce(&Path, &Aio) -> Result<Option<String>, String>,
    json: bool,
) -> Result<String, String> {
    let mut was = vec![];
    for (file, up) in create::manifests(root) {
        let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        if let Some(new) = create::repoint(&text, to, up) {
            let rel = file.strip_prefix(root).unwrap_or(&file).to_string_lossy().replace('\\', "/");
            was.push((file, rel, text, new));
        }
    }
    if was.is_empty() {
        return Err(format!("{}: no aio-rs dependency in its Cargo.toml files", root.display()));
    }
    let write = |f: &Path, t: &str| std::fs::write(f, t).map_err(|e| format!("{}: {e}", f.display()));
    let note = match was.iter().try_for_each(|(f, _, _, new)| write(f, new)).and_then(|()| relock(root, to)) {
        Ok(n) => n,
        Err(e) => {
            for (f, _, old, _) in &was {
                _ = write(f, old);
            }
            return Err(format!("{e}\nnothing changed"));
        }
    };
    let lock = std::fs::read_to_string(root.join("Cargo.lock")).map(|t| locked(&t)).unwrap_or_default();
    let built = lock.iter().find(|l| l.name == "aio-rs");
    let back = back_note(before, to);
    if let Some(b) = &back {
        eprintln!("! {b}");
    }
    let files: Vec<&str> = was.iter().map(|(_, r, ..)| r.as_str()).collect();
    let resolved = to.to_string().trim_start_matches("aio-rs ").to_string();
    if json {
        return Ok(json!({
            "pinned": resolved,
            "requested": requested,
            "recordedIn": files,
            "built": built.map(|b| json!({ "version": b.version, "commit": b.commit })),
            "note": note,
            "movedBack": back,
        })
        .to_string());
    }
    let mut out = format!("pinned to {to}");
    if let Aio::Tag(t) = to
        && *t != requested
        && *t != format!("v{requested}")
    {
        out.push_str(&format!("  (resolved from \"{requested}\")"));
    }
    out.push_str(&format!("\n  recorded in {}", files.join(", ")));
    if let Some(b) = built {
        let c = b.commit.as_deref().map_or(String::new(), |c| format!(" @ {}", &c[..c.len().min(7)]));
        out.push_str(&format!("\n  Cargo.lock: aio-rs {}{c}", b.version));
    }
    if let Some(n) = note {
        out.push_str(&format!("\n  {n}"));
    }
    match to {
        Aio::Branch(_) => {
            out.push_str("\n  NOTE: Cargo.lock holds that exact commit. Re-run `ax pin main` to advance.")
        }
        Aio::Path(_) => out.push_str("\n  local-dev pin — this machine only: `ax pin latest` returns to a release"),
        Aio::Tag(_) => {}
    }
    out.push_str("\n  commit Cargo.toml and Cargo.lock so a clone builds against the same version");
    Ok(out)
}

/// Cargo.lock after `ax pin` moved aio-rs: only the aio-rs crates
/// re-resolved (`cargo update -p`); offline, from what cargo has (a
/// release or checkout it built before), said; a tag or branch that isn't
/// there fails here.
pub fn relock(root: &Path, to: &Aio) -> Result<Option<String>, String> {
    let lock = std::fs::read_to_string(root.join("Cargo.lock")).ok();
    let mut names: Vec<String> = lock.as_deref().map(locked).unwrap_or_default().into_iter().map(|l| l.name).collect();
    names.dedup();
    let cargo = |offline: bool| {
        let mut c = std::process::Command::new("cargo");
        match &lock {
            Some(_) => _ = c.args(["update", "--quiet"]).args(names.iter().flat_map(|n| ["-p", n])),
            None => _ = c.args(["generate-lockfile", "--quiet"]),
        }
        if offline {
            c.arg("--offline");
        }
        let o = c.current_dir(root).output().map_err(|e| format!("cargo: {e}"))?;
        match o.status.success() {
            true => Ok(()),
            false => Err(format!("cargo could not resolve it:\n{}", String::from_utf8_lossy(&o.stderr).trim())),
        }
    };
    match cargo(false) {
        Ok(()) => Ok(None),
        Err(online) => match cargo(true) {
            Ok(()) => Ok(Some(format!(
                "offline: {to} as cargo last fetched it, other crates from its cache; online, re-run to refresh"
            ))),
            Err(_) => Err(online),
        },
    }
}

/// The app `word` names: an app's folder, else an app of this workspace.
pub fn root_of(word: Option<&str>) -> Result<std::path::PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    match word {
        None => create::app_root(&cwd),
        Some(w) if cwd.join(w).join("aio.toml").is_file() => create::app_root(&cwd.join(w)),
        Some(w) => {
            let pkg = crate::dev::locate(&crate::dev::metadata()?, w)?;
            create::app_root(&crate::build::app_dir(&pkg)?)
        }
    }
}

/// `ax pin [app] [target] [--major]`.
pub fn run(app: Option<&str>, target: Option<&str>, cross: bool, json: bool) -> Result<String, String> {
    let root = root_of(app)?;
    let tags = || create::tags();
    let now = info(&root, tags)?;
    let Some(target) = target else {
        let text = if json { now.json().to_string() } else { now.render() };
        // Scriptable: drift is a failure (aio's exit 1).
        return if now.drift() { Err(text) } else { Ok(text) };
    };
    let to = resolve(&root, target, now.pinned(), cross, &now.releases)?;
    if let (Some(Src::Tag(was)), Aio::Tag(t)) = (now.pinned(), &to)
        && major(was) != major(t)
        && !json
    {
        eprintln!("! crossing a major: {was} → {t}. Read the CHANGELOG before shipping this.");
    }
    if let Aio::Path(_) = to {
        eprintln!(
            "! local-dev pin — this machine only: Cargo.toml names a checkout beside the app. \
             Return to a release with \"ax pin latest\"."
        );
    }
    pin(&root, target, &to, now.pinned(), relock, json)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCK: &str = r#"
[[package]]
name = "aio-rs"
version = "0.1.0"
source = "git+https://github.com/riagentic/aio-rs?tag=v0.1.0#ab0cadaf226da601cd4f2745f80125d9fa64182d"

[[package]]
name = "aio-rs-shell"
version = "0.1.0"
source = "git+https://github.com/riagentic/aio-rs?tag=v0.1.0#ab0cadaf226da601cd4f2745f80125d9fa64182d"

[[package]]
name = "notes"
version = "0.1.0"
"#;

    const TAGS: &str = "a\trefs/tags/v0.1.0\nb\trefs/tags/v0.3.0-rc1\nc\trefs/tags/v0.2.0\nd\trefs/tags/v1.0.0\nx\trefs/tags/nightly\n";

    #[test]
    fn reads_each_way_cargo_toml_names_aio_rs() {
        let g = r#"aio = { package = "aio-rs", git = "https://github.com/riagentic/aio-rs", tag = "v0.1.0", features = ["kit"] }"#;
        assert_eq!(asked(g, 0), Some(("aio-rs".into(), Src::Tag("v0.1.0".into()))));
        let b = r#"aio-shell = { package = "aio-rs-shell", git = "x", branch = "main" }"#;
        assert_eq!(asked(b, 1), Some(("aio-rs-shell".into(), Src::Branch("main".into()))));
        assert_eq!(asked(r#"aio-rs = { git = "x", rev = "abc" }"#, 0), Some(("aio-rs".into(), Src::Rev("abc".into()))));
        assert_eq!(asked(r#"aio-rs = { git = "x" }"#, 0).unwrap().1, Src::Git);
        let p = r#"aio = { package = "aio-rs", path = "../../aio-rs/crates/aio" }"#;
        assert_eq!(asked(p, 1).unwrap().1, Src::Path("../aio-rs/crates".into()), "relative to the app's root");
        assert_eq!(asked(r#"aio-rs = { version = "0.2" }"#, 0).unwrap().1, Src::Registry("0.2".into()));
        assert_eq!(asked(r#"aio = { workspace = true }"#, 1), None, "the workspace's line says it");
        assert_eq!(asked(r#"# aio = { package = "aio-rs", tag = "v1" }"#, 0), None);
        assert_eq!(asked(r#"serde = { version = "1" }"#, 0), None);
        // Every spelling moves the same way, workspace deps included.
        let to = Aio::Tag("v0.2.0".into());
        let git = r#"git = "https://github.com/riagentic/aio-rs", tag = "v0.2.0""#;
        let ws = "[workspace.dependencies]\naio = { package = \"aio-rs\", version = \"0.1\", features = [\"kit\"] }\n";
        assert_eq!(
            create::repoint(ws, &to, 0).unwrap(),
            format!("[workspace.dependencies]\naio = {{ package = \"aio-rs\", {git}, features = [\"kit\"] }}\n")
        );
        assert_eq!(
            create::repoint("aio-rs = { version = \"0.1\" }\n", &to, 0).unwrap(),
            format!("aio-rs = {{ {git} }}\n")
        );
        assert_eq!(
            create::repoint("aio-rs-shell = { path = \"../x/aio-shell\", optional = true }", &to, 1).unwrap(),
            format!("aio-rs-shell = {{ {git}, optional = true }}")
        );
        assert_eq!(create::repoint("aio = { workspace = true }\n", &to, 1), None);
    }

    #[test]
    fn reads_cargo_lock_and_the_releases() {
        let l = locked(LOCK);
        assert_eq!(l.len(), 2, "aio-rs crates only");
        assert_eq!(l[0].src, Src::Tag("v0.1.0".into()));
        assert_eq!(l[0].commit.as_deref(), Some("ab0cadaf226da601cd4f2745f80125d9fa64182d"));
        let path = locked("[[package]]\nname = \"aio-rs\"\nversion = \"0.2.0\"\n");
        assert_eq!(path[0].src, Src::Path(String::new()));
        let reg = locked(
            "[[package]]\nname = \"aio-rs\"\nversion = \"0.2.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n",
        );
        assert_eq!(reg[0].src, Src::Registry("0.2.0".into()));
        let r = releases(TAGS);
        assert_eq!(r, ["v1.0.0", "v0.3.0-rc1", "v0.2.0", "v0.1.0"], "newest first, versions only");
        assert_eq!(latest(&r, None).as_deref(), Some("v1.0.0"));
        assert_eq!(latest(&r, Some(0)).as_deref(), Some("v0.2.0"), "a prerelease only when nothing else");
        assert_eq!(latest(&r[1..2], Some(0)).as_deref(), Some("v0.3.0-rc1"));
        assert_eq!(latest(&r, Some(7)), None);
    }

    fn of(asks: &[(&str, Src)], lock: &str, releases: Result<Vec<String>, String>) -> Info {
        Info {
            asks: asks.iter().map(|(f, s)| (f.to_string(), "aio-rs".into(), s.clone())).collect(),
            locked: locked(lock),
            lock_file: !lock.is_empty(),
            ax: "0.1.0".into(),
            releases,
        }
    }

    #[test]
    fn says_the_pin_the_build_this_ax_and_the_releases() {
        let tag = Src::Tag("v0.1.0".into());
        let i = of(&[("Cargo.toml", tag.clone()), ("app/Cargo.toml", tag.clone())], LOCK, Ok(releases(TAGS)));
        assert_eq!((i.pinned(), i.drift(), i.matches(), i.behind()), (Some(&tag), false, Some(true), Some(3)));
        let text = i.render();
        assert!(text.starts_with("  aio-rs    v0.1.0  Cargo.toml, app/Cargo.toml\n  built     0.1.0 (tag v0.1.0 @ ab0cada)\n  ax        0.1.0  matches\n  latest    v1.0.0\n  releases  v1.0.0 v0.3.0-rc1 v0.2.0 v0.1.0"), "{text}");
        assert!(text.contains("i  3 releases behind v1.0.0."), "{text}");
        assert!(text.contains("  ax pin <path>     follow an aio-rs checkout on this machine"), "{text}");
        assert!(!text.contains("Drift"), "{text}");
        let j = i.json();
        assert_eq!(j["pinned"], json!({ "kind": "tag", "name": "v0.1.0" }));
        assert_eq!(
            (j["drift"].as_bool(), j["matches"].as_bool(), j["behind"].as_u64()),
            (Some(false), Some(true), Some(3))
        );
        // Cargo.toml moved by hand, Cargo.lock not: drift.
        let moved = of(&[("Cargo.toml", Src::Tag("v0.2.0".into()))], LOCK, Ok(vec![]));
        assert!(
            moved.drift() && moved.render().contains("Drift — this app is built against something it did not ask for.")
        );
        // Files that disagree, no lock, offline.
        let mixed =
            of(&[("Cargo.toml", tag), ("app/Cargo.toml", Src::Branch("main".into()))], "", Err("offline".into()));
        let text = mixed.render();
        assert_eq!((mixed.pinned(), mixed.drift(), mixed.behind()), (None, false, None));
        assert!(
            text.contains("aio-rs    mixed") && text.contains("Mixed —") && text.contains("Nothing is built yet."),
            "{text}"
        );
        assert!(text.contains("releases  unknown") && text.contains("built     nothing yet"), "{text}");
        let path = of(
            &[("Cargo.toml", Src::Path("../aio-rs/crates".into()))],
            "[[package]]\nname = \"aio-rs\"\nversion = \"0.2.0\"\n",
            Ok(vec![]),
        );
        assert!(!path.drift() && path.render().contains("Local-dev pin"));
        assert!(path.render().contains("ax        0.1.0  differs from what the app builds"));
    }

    #[test]
    fn resolves_what_to_pin_to() {
        let d = tempfile::tempdir().unwrap();
        let tags = Ok(releases(TAGS));
        let v1 = Src::Tag("v0.1.0".into());
        let r = |t: &str, cur: Option<&Src>, cross: bool| resolve(d.path(), t, cur, cross, &tags);
        assert_eq!(r("latest", Some(&v1), false), Ok(Aio::Tag("v0.2.0".into())), "within its major");
        assert_eq!(r("latest", Some(&v1), true), Ok(Aio::Tag("v1.0.0".into())), "--major crosses");
        assert_eq!(r("latest", None, false), Ok(Aio::Tag("v1.0.0".into())));
        assert_eq!(r("main", None, false), Ok(Aio::Branch("main".into())));
        assert_eq!(r("0.2", None, false), Ok(Aio::Tag("v0.2.0".into())), "a bare version is its tag");
        assert!(r("0.9.0", None, false).unwrap_err().contains("v0.9.0 not found; releases: v1.0.0"));
        let off = Err("offline".into());
        assert_eq!(resolve(d.path(), "0.9.0", None, false, &off), Ok(Aio::Tag("v0.9.0".into())), "cargo decides");
        assert!(resolve(d.path(), "latest", None, false, &off).unwrap_err().contains("aren't known"));
        assert!(r("latest", Some(&Src::Tag("v7.0.0".into())), false).unwrap_err().contains("--major"));
        assert!(r("nowhere", None, false).unwrap_err().contains("no aio-rs checkout"));
        assert!(is_target("main") && is_target("latest") && is_target("0.2.0") && is_target("v0.2.0"));
        assert!(!is_target("notes"));
        assert_eq!(
            back_note(Some(&Src::Tag("v0.2.0".into())), &Aio::Tag("v0.1.0".into()))
                .map(|n| n.starts_with("moved BACK: v0.2.0 → v0.1.0")),
            Some(true)
        );
        assert_eq!(back_note(Some(&v1), &Aio::Tag("v0.2.0".into())), None);
    }

    #[test]
    fn moves_every_aio_rs_dependency_all_or_nothing() {
        let no_lock = |_: &Path, _: &Aio| Ok(None);
        let d = tempfile::tempdir().unwrap();
        let (target, installed) = (d.path().join("target"), d.path().join("install"));
        let made = create::run(
            d.path(),
            &create::New { template: "cli", ..create::New::of("notes") },
            (&target, &installed),
            false,
        )
        .unwrap();
        let root = create::app_root(&made.at.join("app/src")).unwrap();
        assert_eq!(root, made.at, "from inside it");
        let tag = format!("v{}", env!("CARGO_PKG_VERSION"));
        let now = info(&root, || Err("offline".into())).unwrap();
        assert_eq!(now.pinned(), Some(&Src::Tag(tag.clone())));
        assert_eq!(now.asks.len(), 4, "four aio-rs lines in two files (the tests' one too)");
        let said = pin(&root, "9.9.9", &Aio::Tag("v9.9.9".into()), now.pinned(), no_lock, false).unwrap();
        assert!(said.starts_with("pinned to aio-rs v9.9.9\n  recorded in Cargo.toml, app/Cargo.toml"), "{said}");
        assert!(said.ends_with("commit Cargo.toml and Cargo.lock so a clone builds against the same version"));
        let app = std::fs::read_to_string(root.join("app/Cargo.toml")).unwrap();
        assert!(
            app.contains(r#"package = "aio-rs-shell", git = "https://github.com/riagentic/aio-rs", tag = "v9.9.9", default-features = false }"#),
            "{app}"
        );
        assert_eq!(info(&root, || Err("x".into())).unwrap().pinned(), Some(&Src::Tag("v9.9.9".into())));
        let j: Value =
            serde_json::from_str(&pin(&root, "main", &Aio::Branch("main".into()), None, no_lock, true).unwrap())
                .unwrap();
        assert_eq!((j["pinned"].as_str(), j["requested"].as_str()), (Some("main (a branch)"), Some("main")));
        assert_eq!(j["recordedIn"], json!(["Cargo.toml", "app/Cargo.toml"]));
        // Cargo refuses the move (no such tag, no network): every file as it was.
        let before = std::fs::read_to_string(root.join("app/Cargo.toml")).unwrap();
        let e =
            pin(&root, "x", &Aio::Branch("gone".into()), None, |_, _| Err("no branch gone".into()), false).unwrap_err();
        assert_eq!(e, "no branch gone\nnothing changed");
        assert_eq!(std::fs::read_to_string(root.join("app/Cargo.toml")).unwrap(), before);
        // An app's other crates are its own: a dot folder or build output is never read.
        std::fs::create_dir_all(root.join("target/x")).unwrap();
        std::fs::write(root.join("target/x/Cargo.toml"), r#"a = { package = "aio-rs", path = "/p" }"#).unwrap();
        assert_eq!(info(&root, || Err("x".into())).unwrap().asks.len(), 4);
        let bare = d.path().join("bare");
        std::fs::create_dir_all(&bare).unwrap();
        std::fs::write(bare.join("Cargo.toml"), "[package]\nname = \"bare\"\n").unwrap();
        std::fs::write(bare.join("aio.toml"), "").unwrap();
        assert!(info(&bare, || Err("x".into())).unwrap_err().contains("no aio-rs dependency"));
        assert!(
            pin(&bare, "main", &Aio::Branch("main".into()), None, no_lock, false)
                .unwrap_err()
                .contains("no aio-rs dependency")
        );
    }
}

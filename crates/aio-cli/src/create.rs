//! `ax create <name> [--template app|cli|service] [--aio-version v | --aio dir]`
//! (aio's `am create`): a new app that runs, builds and tests as it is: its
//! library (cell and window, or commands), the launcher `ax dev` and `aio
//! build` run, `aio.toml`, a README, the agent brief's pointer, its own Cargo
//! workspace and git repository. Nothing of the machine it was made on goes in
//! it: aio-rs comes from git at a release tag (or a checkout named relative
//! to the app), so a clone builds anywhere. The Rust it writes is compiled
//! and its tests run as aio's own (below), so a template can't rot.
use std::path::{Path, PathBuf};

/// The kinds of app, and what each is.
pub const TEMPLATES: &[(&str, &str)] = &[
    ("app", "a desktop window on a saved cell (a counter)"),
    ("cli", "a command-line tool on a saved cell (notes)"),
    ("service", "cells served to clients, no window (a board)"),
];

const APP_LIB: &str = include_str!("create/app_lib.rs");
/// The window app's stylesheet (here too, where the template's Rust,
/// compiled as aio's own in tests, finds it): `ax dev` shows a saved edit at once.
const STYLE_CSS: &str = include_str!("../assets/style.css");
const CLI_LIB: &str = include_str!("create/cli_lib.rs");
const SERVICE_LIB: &str = include_str!("create/service_lib.rs");

const APP_MAIN: &str = "// The launcher: the app is the library, so `ax dev` can hot-patch all of it.
// A release build on Windows is a window app: no console opens with it.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = \"windows\")]
use aio_shell::Start;

fn main() -> std::io::Result<()> {
    let cfg = aio::config!();
    let Start::Run(dirs) = aio_shell::start(&cfg, aio::app_version!())? else { return Ok(()) };
    _ = app::SOURCE.set(aio::Source::for_app(&cfg, &dirs)?);
    aio_shell::run(app::App, aio_shell::Window { width: 900, height: 640, ..aio_shell::Window::of(&cfg) }, &dirs)
}
";

/// The window app on Android (`ax build <app> --android`): the launcher's
/// library, the same window and cell as `main.rs`, in the system WebView.
const APP_ANDROID: &str = "// The app on Android (`ax build --android`): the same window and cell as
// `main.rs`, hosted by the system WebView. Other systems build none of it.
#![cfg(target_os = \"android\")]

aio_shell::android_app!(aio_shell::android::App {
    config: || aio::config!(),
    version: aio::app_version!(),
    root: app::App,
    window: aio_shell::Window::default(),
    setup: |cfg, dirs| {
        _ = app::SOURCE.set(aio::Source::for_app(cfg, dirs)?);
        Ok(())
    },
});
";

const CLI_MAIN: &str = "// The launcher: one command on the tool's cell, then out.
use aio_shell::Start;

fn main() -> std::process::ExitCode {
    let go = || -> std::io::Result<Result<String, (u8, String)>> {
        let cfg = aio::config!();
        let Start::Run(dirs) = aio_shell::start(&cfg, aio::app_version!())? else { return Ok(Ok(String::new())) };
        let notes = aio::Source::<app::Notes>::for_app(&cfg, &dirs)?;
        let args: Vec<String> = std::env::args().skip(1).collect();
        Ok(app::run(&notes, &args))
    };
    match go() {
        Ok(Ok(out)) => {
            if !out.is_empty() {
                println!(\"{out}\");
            }
            std::process::ExitCode::SUCCESS
        }
        Ok(Err((code, why))) => {
            eprintln!(\"{why}\");
            std::process::ExitCode::from(code)
        }
        Err(e) => {
            eprintln!(\"{e}\");
            std::process::ExitCode::FAILURE
        }
    }
}
";

const SERVICE_MAIN: &str = "// The launcher: the cells served until the service is asked to stop.
use aio_shell::Start;

fn main() -> std::io::Result<std::process::ExitCode> {
    let cfg = aio::config!();
    let Start::Run(dirs) = aio_shell::start(&cfg, aio::app_version!())? else {
        return Ok(std::process::ExitCode::SUCCESS);
    };
    // Never a client: the binary holds no sync client (`for_app` would).
    let _board = aio::Source::<app::Board>::for_server(&cfg, &dirs)?;
    // Served until asked to stop; the board is saved first.
    aio_shell::serve(&dirs)?;
    // `ax stop` stays down under the supervisor files `ax build` writes.
    Ok(aio::lifecycle::exit_code())
}
";

/// The build's own settings: the bundled SQLite without the extensions aio
/// never uses (full-text search, R-tree, extension loading…), 330 KB off
/// each binary, so a server fits its 6 MB budget.
const CARGO_CONFIG: &str = "# Cargo's settings for this app's builds.
[env]
# SQLite as aio uses it: no FTS3/FTS5, R-tree, dbstat, STAT4, soundex,
# extension loading, shared cache or deprecated calls (330 KB). An app that
# queries with one of them drops its flag here.
LIBSQLITE3_FLAGS = \"-USQLITE_ENABLE_FTS3 -USQLITE_ENABLE_FTS3_PARENTHESIS -USQLITE_ENABLE_FTS5 \
-USQLITE_ENABLE_RTREE -USQLITE_ENABLE_DBSTAT_VTAB -USQLITE_ENABLE_STAT4 -USQLITE_SOUNDEX \
-USQLITE_ENABLE_LOAD_EXTENSION -DSQLITE_OMIT_LOAD_EXTENSION -DSQLITE_OMIT_DEPRECATED \
-DSQLITE_OMIT_SHARED_CACHE -DSQLITE_DEFAULT_MEMSTATUS=0\"
";

/// A name a package, a binary and an app id can all be (pure): `my-app`.
pub fn name_ok(name: &str) -> Result<(), String> {
    let ok = name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.ends_with('-')
        && name != "app"
        && !name.ends_with("-app");
    match ok {
        true => Ok(()),
        false => Err(format!(
            "{name}: a name of lowercase letters, digits and - (my-app), starting with a letter, not `app` or ending in -app"
        )),
    }
}

/// aio-rs's repository: a new app's aio-rs comes from its release tags.
pub const REPO: &str = "https://github.com/riagentic/aio-rs";

/// Where an app's aio-rs comes from (aio's framework pin): in its
/// Cargo.toml, so a clone builds on any machine; Cargo.lock holds the commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Aio {
    /// A release, by its git tag (`v0.1.0`): the default, this ax's own.
    Tag(String),
    /// A branch's tip (`main`), moving; Cargo.lock pins the commit built.
    Branch(String),
    /// A checkout's `crates` folder, relative to the app's root: aio-rs
    /// co-developed on one machine (`ax framework` moves a clone elsewhere
    /// back to a release).
    Path(String),
}

impl Aio {
    /// This ax's own release.
    pub fn this() -> Aio {
        Aio::Tag(format!("v{}", env!("CARGO_PKG_VERSION")))
    }

    /// A version as `--aio-version` and `ax framework` take it: `0.2.0` or
    /// `v0.2.0` (a release), `main` (the branch), `latest` (the newest
    /// release in `tags`, `git ls-remote --tags` of aio-rs, asked only then).
    pub fn version(v: &str, tags: impl FnOnce() -> Result<String, String>) -> Result<Aio, String> {
        match v {
            "main" => Ok(Aio::Branch(v.into())),
            "latest" => newest(&tags()?).map(Aio::Tag).ok_or_else(|| format!("{REPO} has no release tag")),
            // `0.2` is the tag `v0.2.0`: release tags have three numbers.
            _ => match aio::update::parse_version(v) {
                Some((n, pre)) => Ok(Aio::Tag(format!(
                    "v{}.{}.{}{}",
                    n[0],
                    n[1],
                    n[2],
                    pre.map_or(String::new(), |p| format!("-{p}"))
                ))),
                None => Err(format!("aio-rs version {v}: a release (0.2.0), main or latest")),
            },
        }
    }

    /// The aio-rs checkout `dir` (or its `crates` folder) for the app at
    /// `root`, named relative to it: no folder of this machine goes in the app.
    pub fn checkout(root: &Path, dir: &Path) -> Result<Aio, String> {
        let crates = [dir.join("crates"), dir.to_path_buf()]
            .into_iter()
            .find(|c| c.join("aio/Cargo.toml").is_file() && c.join("aio-shell/Cargo.toml").is_file())
            .ok_or_else(|| {
                format!("--aio {}: no aio-rs checkout there (crates/aio, crates/aio-shell)", dir.display())
            })?;
        let canon = |p: &Path| std::fs::canonicalize(p).map_err(|e| format!("{}: {e}", p.display()));
        let parent = root.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let root = canon(parent)?.join(root.file_name().ok_or("the app's folder has no name")?);
        relative(&root, &canon(&crates)?).map(Aio::Path).ok_or_else(|| {
            format!(
                "--aio {}: the app and the checkout share no folder (another drive, or only the root): \
                     put both in one, a path through the root would name this machine's folders",
                dir.display()
            )
        })
    }

    /// The dependency source of aio-rs crate `krate` (`aio`, `aio-shell`)
    /// in a Cargo.toml `up` folders below the app's root (pure).
    fn source(&self, krate: &str, up: usize) -> String {
        match self {
            Aio::Tag(t) => format!(r#"git = "{REPO}", tag = "{t}""#),
            Aio::Branch(b) => format!(r#"git = "{REPO}", branch = "{b}""#),
            Aio::Path(p) => format!(r#"path = "{}{p}/{krate}""#, "../".repeat(up)),
        }
    }

    /// Where its guide is read: its release's (a checkout's: this ax's).
    fn docs(&self) -> String {
        match self {
            Aio::Tag(r) | Aio::Branch(r) => format!("{REPO}/blob/{r}/docs"),
            Aio::Path(_) => format!("{REPO}/blob/v{}/docs", env!("CARGO_PKG_VERSION")),
        }
    }
}

impl std::fmt::Display for Aio {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Aio::Tag(t) => write!(f, "aio-rs {t}"),
            Aio::Branch(b) => write!(f, "aio-rs {b} (a branch)"),
            Aio::Path(p) => write!(f, "aio-rs at {p} (a checkout)"),
        }
    }
}

/// The newest release tag in `git ls-remote --tags` text (pure); a
/// prerelease only when there is nothing else.
pub fn newest(ls_remote: &str) -> Option<String> {
    let tags: Vec<(&str, bool)> = ls_remote
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1)?.strip_prefix("refs/tags/"))
        .filter(|t| t.starts_with('v') && !t.ends_with("^{}"))
        .filter_map(|t| aio::update::parse_version(t).map(|(_, pre)| (t, pre.is_none())))
        .collect();
    let pick = |release: bool| {
        tags.iter()
            .filter(|(_, r)| *r || !release)
            .map(|(t, _)| *t)
            .max_by(|a, b| aio::update::compare(a, b).unwrap_or(std::cmp::Ordering::Equal))
    };
    pick(true).or_else(|| pick(false)).map(String::from)
}

/// aio-rs's tags, for `latest` (`git ls-remote`).
pub fn tags() -> Result<String, String> {
    let o = std::process::Command::new("git")
        .args(["ls-remote", "--tags", "--refs", REPO])
        .output()
        .map_err(|e| format!("git: {e}"))?;
    match o.status.success() {
        true => Ok(String::from_utf8_lossy(&o.stdout).into_owned()),
        false => Err(format!("git ls-remote {REPO}: {}", String::from_utf8_lossy(&o.stderr).trim())),
    }
}

/// `to` named from `from`, both absolute (pure): `../aio-rs/crates`; None
/// when they share no folder below the root (or drive): a path through it
/// would name this machine's folders (`../../home/<user>/…`).
pub fn relative(from: &Path, to: &Path) -> Option<String> {
    use std::path::Component::{Prefix, RootDir};
    let (a, b): (Vec<_>, Vec<_>) = (from.components().collect(), to.components().collect());
    let same = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    if same <= a.iter().take_while(|c| matches!(c, Prefix(_) | RootDir)).count() {
        return None;
    }
    let up = std::iter::repeat_n("..".to_string(), a.len() - same);
    let all: Vec<String> = up.chain(b[same..].iter().map(|c| c.as_os_str().to_string_lossy().into_owned())).collect();
    Some(if all.is_empty() { ".".into() } else { all.join("/") })
}

/// `key = "…"` out of a dependency's inline table `line`, with its comma.
fn drop_kv(line: &mut String, key: &str) {
    let pat = format!("{key} = \"");
    let Some(s) = line.match_indices(&pat).map(|(i, _)| i).find(|&i| line[..i].ends_with([' ', '{'])) else {
        return;
    };
    let v = s + pat.len();
    let Some(e) = line[v..].find('"').map(|e| v + e + 1) else { return };
    match (line[e..].starts_with(", "), line[..s].ends_with(", ")) {
        (true, _) => line.replace_range(s..e + 2, ""),
        (false, true) => line.replace_range(s - 2..e, ""),
        _ => line.replace_range(s..e, ""),
    }
}

/// `text`, a Cargo.toml `up` folders below the app's root, with each aio-rs
/// dependency (`package = "aio-rs…"` on one line, as `ax create` writes
/// them) taken from `aio` (pure); None: it has none.
pub fn repoint(text: &str, aio: &Aio, up: usize) -> Option<String> {
    let mut hit = false;
    let out = text
        .split_inclusive('\n')
        .map(|line| {
            let tag = "package = \"aio-rs";
            let Some(at) = line.find(tag) else { return line.to_string() };
            let Some(len) = line[at + tag.len()..].find('"') else { return line.to_string() };
            let package = &line[at + 11..at + tag.len() + len];
            let mut line = line.to_string();
            for key in ["path", "git", "tag", "branch", "rev"] {
                drop_kv(&mut line, key);
            }
            let end = line.find(&format!("package = \"{package}\"")).map_or(0, |i| i + package.len() + 12);
            let krate = format!("aio{}", &package["aio-rs".len()..]);
            line.insert_str(end, &format!(", {}", aio.source(&krate, up)));
            hit = true;
            line
        })
        .collect();
    hit.then_some(out)
}

/// The app around `here`: the nearest folder with its aio.toml and Cargo.toml.
pub fn app_root(here: &Path) -> Result<PathBuf, String> {
    here.ancestors()
        .find(|d| d.join("aio.toml").is_file() && d.join("Cargo.toml").is_file())
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("{}: in no app (no aio.toml beside a Cargo.toml here or above)", here.display()))
}

/// The app's Cargo.toml files under `root`, each with how deep it is
/// (build output and dot folders aside).
fn manifests(root: &Path) -> Vec<(PathBuf, usize)> {
    fn walk(d: &Path, up: usize, out: &mut Vec<(PathBuf, usize)>) {
        if d.join("Cargo.toml").is_file() {
            out.push((d.join("Cargo.toml"), up));
        }
        let mut subs: Vec<PathBuf> = std::fs::read_dir(d)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter(|e| {
                let n = e.file_name();
                let n = n.to_string_lossy();
                !n.starts_with('.') && !["target", "dist", "node_modules"].contains(&n.as_ref())
            })
            .map(|e| e.path())
            .collect();
        subs.sort();
        for s in subs {
            walk(&s, up + 1, out);
        }
    }
    let mut out = vec![];
    walk(root, 0, &mut out);
    out
}

/// `ax framework [version] [--aio dir]` (aio's `am pin`, and `am fix` on a
/// clone): the aio-rs of the app at `root`, or each of its Cargo.toml files'
/// aio-rs dependencies pointed at `to`, then `relock` (Cargo.lock): all
/// or nothing.
pub fn framework(
    root: &Path,
    to: Option<&Aio>,
    relock: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<String, String> {
    let (mut said, mut was) = (vec![], vec![]);
    for (file, up) in manifests(root) {
        let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        let rel = file.strip_prefix(root).unwrap_or(&file).to_string_lossy().replace('\\', "/");
        match to.map(|aio| (aio, repoint(&text, aio, up))) {
            None => said.extend(
                text.lines().filter(|l| l.contains("package = \"aio-rs")).map(|l| format!("{rel}: {}", l.trim())),
            ),
            Some((aio, Some(new))) => {
                said.push(format!("{rel}: {aio}"));
                was.push((file, text, new));
            }
            Some((_, None)) => {}
        }
    }
    if said.is_empty() {
        return Err(format!("{}: no aio-rs dependency in its Cargo.toml files", root.display()));
    }
    if to.is_none() {
        return Ok(said.join("\n"));
    }
    let write = |f: &Path, t: &str| std::fs::write(f, t).map_err(|e| format!("{}: {e}", f.display()));
    let moved = was.iter().try_for_each(|(f, _, new)| write(f, new)).and_then(|()| relock(root));
    match moved {
        Ok(()) => Ok(said.join("\n") + "\nCargo.lock: updated (commit it with the change)"),
        Err(e) => {
            for (f, old, _) in &was {
                _ = write(f, old);
            }
            Err(format!("{e}\nnothing changed"))
        }
    }
}

/// Cargo.lock after `ax framework` moved aio-rs: only aio-rs re-resolved
/// (a tag or branch that isn't there fails here).
pub fn relock(root: &Path) -> Result<(), String> {
    let o = std::process::Command::new("cargo")
        .args(["update", "--workspace", "--quiet"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cargo: {e}"))?;
    match o.status.success() {
        true => Ok(()),
        false => Err(format!("cargo could not resolve it:\n{}", String::from_utf8_lossy(&o.stderr).trim())),
    }
}

/// What `ax create` is asked to make.
pub struct New<'a> {
    pub name: &'a str,
    pub template: &'a str,
    /// Its id (`--id`): its data's home.
    pub id: &'a str,
    /// Its Debian maintainer, `Name <email>`.
    pub maintainer: &'a str,
    pub aio: Aio,
    /// `--target a,b`: its `[build] targets`.
    pub targets: Vec<&'a str>,
    /// `--css tailwind`.
    pub tailwind: bool,
}

impl<'a> New<'a> {
    /// The window app `name` on this ax's aio-rs, nothing else asked.
    pub fn of(name: &'a str) -> Self {
        New {
            name,
            template: "app",
            id: name,
            maintainer: NO_MAINTAINER,
            aio: Aio::this(),
            targets: vec![],
            tailwind: false,
        }
    }
}

/// `--css tailwind`'s source sheet: Tailwind, then the template's own styles.
const TAILWIND_SOURCE: &str = "/* The stylesheet's source: `[build] css` (Tailwind) writes assets/style.css from it
   before every build and on each saved change under ax dev. Edit this file, not that one. */
@import \"tailwindcss\";

";

/// The README of app `n` (aio's, per kind).
fn readme(n: &New, ui: bool) -> String {
    let name = n.name;
    let what = match n.template {
        "app" => "a desktop window on a saved cell",
        "cli" => "a command-line tool on a saved cell",
        _ => "a service: cells served to clients, no window",
    };
    let run = match n.template {
        "app" => format!("ax dev {name}                 # the window; a saved edit hot-patches in"),
        "cli" => format!("cargo run -q -p {name}-app -- add milk   # one command, then out (list, done <n>)"),
        _ => format!("ax dev {name}                 # served on [server] listen until stopped"),
    };
    let more = if ui {
        format!(
            "ax dev {name} --web           # the same window in a browser\n\
             ax build {name} --android     # signed APKs for a phone\n"
        )
    } else {
        String::new()
    };
    let css = match n.tailwind {
        true => {
            "\nStyles: edit `src/app.css`; `[build] css` (Tailwind's `tailwindcss` on PATH) writes\n\
                 `assets/style.css` from it before every build. Commit both: `cargo` and `ax test` read\n\
                 the written one.\n"
        }
        false => "",
    };
    format!(
        "# {name}\n\n\
         An [aio-rs]({REPO}) app ({}): {what}.\n\n\
         ```sh\n\
         {run}\n\
         ax test {name}                # its tests\n\
         ax build {name}               # its installable app (ax build {name} --list: every kind)\n\
         {more}\
         ax release {name}             # a signed release for its users' updates\n\
         ```\n\n\
         A fresh clone builds as it is: Cargo.toml takes aio-rs from git ({}), Cargo.lock\n\
         pins its commit. `ax framework` says which; `ax framework <version>` (or `main`,\n\
         `latest`) moves the app to another, `--aio <dir>` to a local aio-rs checkout.\n\
         Needs: `ax` (aio-rs's install script, or `cargo install --locked --git {REPO} aio-rs-cli`),\n\
         then `ax setup` once per machine.\n\n\
         Its version is `[app] version` (major.minor) in aio.toml; each build is\n\
         `major.minor.<commit count>` (`-dirty.<hash>` from uncommitted changes).\n\
         More targets in one `ax build`: `[build] targets` in aio.toml.\n{css}\n\
         The app is `src/lib.rs`; `app/` is its launcher; aio.toml its one config.\n\
         The guide: {}/README.md\n",
        n.template,
        n.aio,
        n.aio.docs()
    )
}

/// The agent brief's pointer, `AGENTS.md` (aio's): the rules and the loop
/// inline, the rest by `ax agent` and the guide's URL.
fn agents(n: &New, ui: bool) -> String {
    let (name, docs) = (n.name, n.aio.docs());
    let lib = match n.template {
        "app" => "its cell and its window",
        "cli" => "its cell and its commands",
        _ => "its cell and its served methods",
    };
    let surface = match ui {
        true => format!("ax surface {name}                        # what is on screen, by name\n    "),
        false => String::new(),
    };
    let platforms = match ui {
        true => format!(
            "- The same window elsewhere: `ax dev {name} --web` / `ax build {name} --web` (a browser: `src/lib.rs`'s\n  \
             `web`), `ax build {name} --android` (`app/src/lib.rs`).\n\
             - Styles live in `assets/style.css`, read by `aio::use_css!`: a saved edit shows at once under\n  \
             `ax dev`, no compile. Never keep CSS (or anything an edit should change) in `use_hook`: a\n  \
             hook's value is made once, so a hot-patched edit never shows (`ax lint` says so).\n"
        ),
        false => String::new(),
    };
    format!(
        "# {name}\n\n\
         An aio-rs app: Rust, cells and Dioxus views, not aio's TypeScript, React or Next. Code guessed\n\
         from those compiles to the wrong thing or not at all.\n\n\
         ## Read this first\n\n    ax agent\n\n\
         One page: how aio-rs works, every verb, adding state, UI and tests, debugging, shipping\n\
         (`ax agent --min` for a small context, `ax agent --list` for its sections). The guide, every\n\
         part with examples: {docs}/README.md (porting an aio app: {docs}/migration/README.md).\n\n\
         ## Rules\n\n\
         - Never end an app by process match (`pkill -f`, `killall`): `ax stop {name}`.\n\
         - Never take over the person's screen: a window an agent opens goes on a nested display\n  \
         (`ax dev` and `ax start` do it with no terminal); `ax build {name} --smoke` runs headless.\n\
         - Never script around `ax`: `ax expect` asserts, `ax state --watch` follows; every verb takes\n  \
         `--json` and exits 0 ok, 1 failed, 2 bad usage.\n\
         - check → run → test: `cargo check`, then the running app, then tests that prove it.\n\
         - Clean up only what you made: a throwaway run gets `AIORT_DATA=<dir>` (the same for every\n  \
         verb); done, `ax stop {name}` and delete that dir.\n\n\
         ## The loop\n\n    \
         ax start {name}                          # in the background (ax dev: hot-patching, foreground)\n    \
         {surface}ax call {name} <cell> <method> [arg…]  # change its state\n    \
         ax expect {name} <cell> <path> eq <value>  # assert it\n    \
         ax events {name} --diff                  # what happened, with each change\n    \
         ax test {name}                           # its tests, last\n    \
         ax stop {name}\n\n\
         ## This app\n\n\
         - `src/lib.rs` is the app: {lib}. `app/` is its launcher; `aio.toml` its one config.\n\
         - `ax build {name}` makes its installable app; ax builds into one build folder per machine\n  \
         (plain `cargo` builds its own `target/`). A missing tool: `ax setup --check` names it.\n\
         {platforms}\
         - Its aio-rs: {} (`ax framework` moves it).\n",
        n.aio
    )
}

/// The files of app `n`, as (path, text) (pure).
pub fn files(n: &New) -> Result<Vec<(String, String)>, String> {
    let (name, template, id) = (n.name, n.template, n.id);
    name_ok(name)?;
    name_ok(id).map_err(|e| format!("--id {e}"))?;
    let (lib, main, mode, server) = match template {
        "app" => (APP_LIB, APP_MAIN, "standalone", ""),
        "cli" => (CLI_LIB, CLI_MAIN, "cli", ""),
        "service" => (
            SERVICE_LIB,
            SERVICE_MAIN,
            "server",
            "\n[server]\nlisten = \"127.0.0.1:7420\" # --port, --host and --expose change it at launch\n",
        ),
        t => {
            let all: Vec<&str> = TEMPLATES.iter().map(|(t, _)| *t).collect();
            return Err(format!("--template {t}: one of {}", all.join(", ")));
        }
    };
    let ui = template == "app";
    let kinds: Vec<&str> = crate::fleet::Kind::ALL.iter().map(|k| k.name()).collect();
    if let Some(t) = n.targets.iter().find(|t| !kinds.contains(t)) {
        return Err(format!("--target {t}: one of {}", kinds.join(", ")));
    }
    if !ui && n.targets.contains(&"android") {
        return Err(format!("--target android: a window app's (--template app), not a {template}'s"));
    }
    if !ui && n.tailwind {
        return Err(format!("--css tailwind: a window app's (--template app), not a {template}'s"));
    }
    let (aio, aio1, shell1) = (n.aio.source("aio", 0), n.aio.source("aio", 1), n.aio.source("aio-shell", 1));
    let dioxus = r#"dioxus = { version = "=0.7.10", default-features = false, features = ["macro", "html", "signals", "hooks"] }"#;
    // The window in a browser: aio's page half and the `start` binding.
    let web = format!(
        r#"
# The browser build (`ax dev --web`, `ax build --web`): aio in the page.
[target.'cfg(target_arch = "wasm32")'.dependencies]
aio = {{ package = "aio-rs", {aio}, features = ["web"] }}
wasm-bindgen = "0.2"
"#
    );
    // One fixed feature set per kind (each other set is one more ax build).
    let feats = match template {
        "app" => r#", features = ["kit"]"#,
        "service" => r#", features = ["net"]"#,
        _ => "",
    };
    let root = format!(
        r#"[workspace]
resolver = "3"
members = [".", "app"]

[workspace.package]
edition = "2024"
publish = false

[package]
name = "{name}"
version = "0.1.0"
edition.workspace = true
publish.workspace = true

# aio-rs from git at a release tag: a clone builds anywhere, Cargo.lock pins
# the commit. `ax framework <version>` moves it (`--aio <dir>`: a checkout).
[dependencies]
aio = {{ package = "aio-rs", {aio}{feats} }}
{deps}
[profile.release]
lto = true
opt-level = "z"
strip = true
codegen-units = 1

# Small debug builds: line numbers for this app, no debug info for its libraries.
[profile.dev]
debug = "line-tables-only"

[profile.dev.package."*"]
debug = false

# Every debug build, aio's and dx's: one copy of the libraries (ax dev, test, lint).
[profile.desktop-dev]
inherits = "dev"
strip = false
"#,
        deps = if ui { format!("{dioxus}\n{web}") } else { String::new() }
    );
    let launcher = format!(
        r#"# The launcher alone: the app is the `{name}` library, so `ax dev` can
# hot-patch all of it (dx recompiles library packages, not a binary's own).
[package]
name = "{name}-app"
version = "0.1.0"
edition.workspace = true
publish.workspace = true

[[bin]]
name = "{name}"
path = "src/main.rs"

[dependencies]
app = {{ package = "{name}", path = ".." }}
aio = {{ package = "aio-rs", {aio1} }}
aio-shell = {{ package = "aio-rs-shell", {shell1} }}
"#
    );
    let mut build = vec![];
    if !n.targets.is_empty() {
        let list: Vec<String> = n.targets.iter().map(|t| format!("\"{t}\"")).collect();
        build.push(format!("targets = [{}] # what `ax build` makes (--list: every kind)", list.join(", ")));
    }
    if n.tailwind {
        build.push(r#"css = "tailwindcss -i src/app.css -o assets/style.css" # before every build"#.into());
    }
    let build = match build.is_empty() {
        true => String::new(),
        false => format!("\n[build]\n{}\n", build.join("\n")),
    };
    let maintainer = n.maintainer;
    let toml = format!(
        r#"# aio.toml: the app's one config file, with the settings to look at first.
# Every setting and its default: {docs}/reference/aio-toml.md
[app]
id = "{id}"           # its data's home: never change it once it ran
name = "{name}"
mode = "{mode}"         # standalone | client-server | client | server | cli
version = "0.1"         # major.minor: ax numbers builds from git (0.1.<commit count>)
lang = "en"             # the page's language, <html lang>
dir = "ltr"             # ltr | rtl | auto: the page's direction, never guessed from lang
maintainer = "{maintainer}" # "Name <email>": the Debian package's (ax build --deb)

[history]
keep = "90d"            # how long the event log keeps every step

[capabilities]
network = []            # hosts aio may reach besides this machine: "api.example.org", "*.example.org", "lan", "*"
{server}{build}"#,
        docs = n.aio.docs()
    );
    let toolchain = format!(
        "# The Rust ax builds with: one compiler for every app, so ax's shared build\n\
         # folder is reused. `ax setup` names the targets Android and the web add.\n\
         [toolchain]\nchannel = \"{}\"\n",
        crate::setup::toolchain()
    );
    let ignore = "/target\n/.data\n/dist\n\n# Secrets and release keys: never committed\n.env\n*.key\n*.p12\n*.jks\n*.keystore\n";
    let attributes = "# LF line endings in every checkout, whatever git's core.autocrlf says:\n\
                      # a Windows clone holds the same bytes as a Linux one.\n* text=auto eol=lf\n";
    let style = ui.then(|| ("assets/style.css".into(), STYLE_CSS.into()));
    let tailwind = n.tailwind.then(|| ("src/app.css".into(), format!("{TAILWIND_SOURCE}{STYLE_CSS}")));
    let android = ui.then(|| ("app/src/lib.rs".into(), APP_ANDROID.into()));
    Ok([
        ("Cargo.toml".into(), root),
        ("aio.toml".into(), toml),
        ("src/lib.rs".into(), lib.into()),
        ("app/Cargo.toml".into(), launcher),
        ("app/src/main.rs".into(), main.into()),
        (".cargo/config.toml".into(), CARGO_CONFIG.into()),
        ("rust-toolchain.toml".into(), toolchain),
        (".gitignore".into(), ignore.into()),
        (".gitattributes".into(), attributes.into()),
        ("README.md".into(), readme(n, ui)),
        ("AGENTS.md".into(), agents(n, ui)),
        ("CLAUDE.md".into(), "@AGENTS.md\n".into()),
    ]
    .into_iter()
    .chain(style)
    .chain(tailwind)
    .chain(android)
    .collect())
}

/// What `ax create` made.
pub struct Made {
    pub at: PathBuf,
    pub name: String,
    pub template: String,
    pub aio: Aio,
    pub files: Vec<String>,
    /// Said after: earlier data under its id, Cargo.lock, git.
    pub notes: Vec<String>,
}

impl Made {
    /// What to print: a line and its notes, or (`json`) one object with
    /// the files and the next steps.
    pub fn said(&self, json: bool) -> String {
        let n = &self.name;
        let next = [format!("cd {n}"), format!("ax dev {n}"), format!("ax test {n}"), format!("ax build {n}")];
        match json {
            true => serde_json::json!({
                "created": self.at, "name": n, "template": self.template, "aio": self.aio.to_string(),
                "files": self.files, "next": next, "notes": self.notes,
            })
            .to_string(),
            false => format!(
                "created {} ({}, {}): cd {n}; ax dev {n} runs it, ax test {n} tests it, ax build {n} builds it{}",
                self.at.display(),
                self.template,
                self.aio,
                self.notes.iter().map(|l| format!("\n{l}")).collect::<String>()
            ),
        }
    }
}

/// Write `all` under `at`, all or nothing (aio's ScaffoldLedger): on a
/// failure each file written over is put back, and each file and folder
/// made goes again.
fn write_all(at: &Path, all: &[(String, String)]) -> Result<(), String> {
    let (mut made, mut saved): (Vec<PathBuf>, Vec<(PathBuf, Vec<u8>)>) = (vec![], vec![]);
    let mut step = |rel: &str, text: &str| -> Result<(), String> {
        let p = at.join(rel);
        let dirs: Vec<PathBuf> = p.ancestors().skip(1).take_while(|d| !d.exists()).map(Path::to_path_buf).collect();
        made.extend(dirs.into_iter().rev());
        let parent = p.parent().unwrap_or(at);
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        match std::fs::read(&p) {
            Ok(old) => saved.push((p.clone(), old)),
            Err(_) if !p.exists() => made.push(p.clone()),
            Err(_) => {}
        }
        std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))
    };
    let Err(e) = all.iter().try_for_each(|(rel, text)| step(rel, text)) else { return Ok(()) };
    for (p, old) in &saved {
        _ = std::fs::write(p, old);
    }
    for p in made.iter().rev() {
        _ = if p.is_dir() { std::fs::remove_dir(p) } else { std::fs::remove_file(p) };
    }
    Err(format!("{e} (nothing of the new app is left)"))
}

/// Write app `n` into `dir/<name>`; a folder that holds anything is
/// refused unless `force` (then only the template's files are written), and
/// so is a name another app builds in the shared build folder `target`. An
/// id an app installed in `installed` has is refused: the two would share
/// one home and its data.
pub fn run(dir: &Path, n: &New, (target, installed): (&Path, &Path), force: bool) -> Result<Made, String> {
    let (name, id) = (n.name, n.id);
    if installed.join(id).exists() {
        return Err(format!(
            "id `{id}` is an installed app's ({}): the new app would share its data; name it apart with \
             --id <id> ({name}-dev), or `ax remove {id}` first",
            installed.join(id).display()
        ));
    }
    let all = files(n)?;
    let at = dir.join(name);
    if !force && std::fs::read_dir(&at).is_ok_and(|mut d| d.next().is_some()) {
        return Err(format!(
            "{} holds files already: another name, or --force to write the template over it",
            at.display()
        ));
    }
    crate::dev::claim(target, name, &at)?;
    write_all(&at, &all)?;
    let prior = aio::paths::locate(id).ok().and_then(|(data, ..)| prior_data(id, &data));
    Ok(Made {
        at,
        name: name.into(),
        template: n.template.into(),
        aio: n.aio.clone(),
        files: all.into_iter().map(|(f, _)| f).collect(),
        notes: prior.into_iter().collect(),
    })
}

/// Write the new app's `Cargo.lock` now, so its first commit holds it and
/// that commit's build is a release, not a dirty tree; None when written,
/// else what to do (the first build writes it then).
pub fn lock(app: &Path) -> Option<String> {
    let done = std::process::Command::new("cargo")
        .args(["generate-lockfile", "--quiet"])
        .current_dir(app)
        .status()
        .is_ok_and(|s| s.success());
    (!done).then(|| "no Cargo.lock yet: the first build writes it; commit it with the app".into())
}

/// `git init` and the first commit, so the app's builds are numbered from
/// minute one (aio's `tryGitInit`); never inside another repository, never
/// failing the create: what it did is said. `identity`: git has the
/// person's name and email, else the commit is ax's.
pub fn git_init(app: &Path, identity: bool) -> String {
    let git = |args: &[&str]| {
        let mut c = std::process::Command::new("git");
        // A hook's GIT_DIR never decides which repository.
        c.args(args).current_dir(app).env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env_remove("GIT_INDEX_FILE");
        c.output()
    };
    match git(&["rev-parse", "--show-toplevel"]) {
        Err(_) => return "git: not found, no repository made".into(),
        Ok(o) if o.status.success() => {
            return format!("git: inside {}, no repository of its own", String::from_utf8_lossy(&o.stdout).trim());
        }
        Ok(_) => {}
    }
    let who: &[&str] = if identity { &[] } else { &["-c", "user.name=ax", "-c", "user.email=ax@localhost"] };
    let commit =
        [who, &["-c", "commit.gpgsign=false", "commit", "--quiet", "-m", "Initial commit (ax create)"]].concat();
    let ok = |args: &[&str]| git(args).is_ok_and(|o| o.status.success());
    match ok(&["init", "--quiet"]) && ok(&["add", "-A"]) && ok(&commit) {
        true => "git: a repository with its first commit".into(),
        false => "git: the repository's first commit failed (git status says why)".into(),
    }
}

/// The placeholder maintainer when git has no `user.name` and `user.email`.
pub const NO_MAINTAINER: &str = "Your Name <you@example.org>";

/// `Name <email>` from git's `user.name` and `user.email` (`dir`'s config,
/// else the global one), when both are set and fit a TOML string as they are.
pub fn git_maintainer(dir: &Path) -> Option<String> {
    let get = |k: &str| {
        let out = std::process::Command::new("git").arg("-C").arg(dir).args(["config", k]).output().ok()?;
        let v = String::from_utf8(out.stdout).ok()?.trim().to_string();
        (out.status.success() && !v.is_empty()).then_some(v)
    };
    let m = format!("{} <{}>", get("user.name")?, get("user.email")?);
    (!m.chars().any(|c| c == '"' || c == '\\' || c.is_control())).then_some(m)
}

/// What to say when an app called `name` already keeps data in `data`
/// (aio's `priorAppData`): the new app will open it, so the person knows.
pub fn prior_data(name: &str, data: &Path) -> Option<String> {
    data.join("state.db").exists().then(|| {
        format!(
            "note: an app called {name} already keeps data in {}; the new one opens it (`ax data {name}` says what is there)",
            data.display()
        )
    })
}

/// The aio-rs checkout this `ax` was built from (its templates may be newer
/// than its release tag: `ax create` says so, never writes it in an app);
/// none in one `ax release --aio` made: the build machine's folders don't
/// ship, and are not on the user's machine anyway.
pub fn this_checkout() -> Option<&'static Path> {
    match option_env!("AIORT_RELEASED") {
        Some(_) => None,
        None => Some(Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).unwrap_or(Path::new("."))),
    }
}

// The templates' Rust, compiled and tested as aio's own.
#[cfg(test)]
#[path = "create/app_lib.rs"]
mod app_lib;
#[cfg(test)]
#[path = "create/cli_lib.rs"]
mod cli_lib;
#[cfg(test)]
#[path = "create/service_lib.rs"]
mod service_lib;

#[cfg(test)]
mod tests {
    use super::*;

    fn get<'a>(all: &'a [(String, String)], p: &str) -> &'a str {
        &all.iter().find(|(f, _)| f == p).unwrap_or_else(|| panic!("no {p}")).1
    }

    fn of<'a>(name: &'a str, template: &'a str) -> Vec<(String, String)> {
        files(&New { template, ..New::of(name) }).unwrap()
    }

    #[test]
    fn each_template_writes_a_whole_app_with_its_config_valid() {
        let tag = format!("v{}", env!("CARGO_PKG_VERSION"));
        for (t, _) in TEMPLATES {
            let all = files(&New { template: t, maintainer: "A <a@example.org>", ..New::of("my-tool") }).unwrap();
            let get = |p: &str| get(&all, p);
            let cfg = aio::config::Config::parse(get("aio.toml")).unwrap_or_else(|e| panic!("{t}: {e}"));
            assert_eq!((cfg.app.id.as_str(), cfg.app.name.as_str()), ("my-tool", "my-tool"));
            assert_eq!(cfg.app.maintainer.as_deref(), Some("A <a@example.org>"), "ax build --deb works as created");
            let git = format!(r#"git = "https://github.com/riagentic/aio-rs", tag = "{tag}""#);
            assert!(get("Cargo.toml").contains(&format!(r#"aio = {{ package = "aio-rs", {git}"#)), "a release");
            assert!(get("app/Cargo.toml").contains(&format!(r#"aio-shell = {{ package = "aio-rs-shell", {git} }}"#)));
            assert!(
                get("app/Cargo.toml").contains("name = \"my-tool\"\npath = \"src/main.rs\""),
                "the binary is the app's name"
            );
            assert!(get("app/Cargo.toml").contains(r#"app = { package = "my-tool", path = ".." }"#));
            assert!(get("app/src/main.rs").contains("aio::config!();"), "aio.toml is parsed as it builds");
            assert!(
                !all.iter().any(|(f, _)| f.ends_with("build.rs")),
                "no build script: aio::config!() reads aio.toml"
            );
            assert!(!get("app/Cargo.toml").contains("[build-dependencies]"), "no host build of aio");
            assert!(
                get("Cargo.toml").contains("[profile.desktop-dev]\ninherits = \"dev\"\nstrip = false\n"),
                "dx's profile"
            );
            assert_eq!(get("CLAUDE.md"), "@AGENTS.md\n", "aio: every template's CLAUDE.md imports AGENTS.md");
            let docs = format!("https://github.com/riagentic/aio-rs/blob/{tag}/docs");
            for f in ["AGENTS.md", "README.md", "aio.toml"] {
                assert!(get(f).contains(&docs), "{f}: the guide of the aio-rs it builds on, by URL");
            }
            let agents = get("AGENTS.md");
            for want in ["ax agent", "ax stop my-tool", "## Rules", "## The loop", "ax expect my-tool", "AIORT_DATA"] {
                assert!(agents.contains(want), "{want}: the rules and the loop inline\n{agents}");
            }
            assert_eq!(agents.contains("ax surface"), *t == "app", "a window to read");
            assert!(get("README.md").starts_with("# my-tool\n") && get("README.md").contains("ax test my-tool"));
            assert!(
                get(".gitignore").contains("/.data") && get(".gitignore").contains("\n.env\n*.key\n*.p12\n*.jks\n")
            );
            assert!(get(".gitattributes").contains("* text=auto eol=lf\n"));
            let tc = format!("[toolchain]\nchannel = \"{}\"\n", crate::setup::toolchain());
            assert!(get("rust-toolchain.toml").ends_with(&tc), "aio's own Rust: {}", get("rust-toolchain.toml"));
            let flags = get(".cargo/config.toml").lines().find(|l| l.starts_with("LIBSQLITE3_FLAGS = \"")).unwrap();
            assert!(flags.ends_with("MEMSTATUS=0\"") && flags.contains("FTS5 -USQLITE_ENABLE_RTREE"), "{flags}");
        }
        let main = |t| get(&of("x", t), "app/src/main.rs").to_string();
        assert!(
            main("service").contains("for_server(") && main("app").contains("for_app("),
            "a server holds no client"
        );
        let mode = |t| aio::config::Config::parse(get(&of("x", t), "aio.toml")).unwrap().app.mode;
        use aio::config::Mode;
        assert_eq!([mode("app"), mode("cli"), mode("service")], [Mode::Standalone, Mode::Cli, Mode::Server]);
        assert!(get(&of("x", "app"), "Cargo.toml").contains("dioxus"), "a window needs Dioxus");
        let android = |t| of("x", t).into_iter().find(|(f, _)| f == "app/src/lib.rs");
        let lib = android("app").expect("a window app builds for Android as it is").1;
        assert!(
            lib.contains("#![cfg(target_os = \"android\")]")
                && lib.contains("android_app!")
                && lib.contains("root: app::App")
        );
        assert!(android("cli").is_none() && android("service").is_none(), "no window, no Android app");
        // The web: the app's library binds itself in the page (ax build --web links it as a cdylib).
        let root = files(&New { aio: Aio::Path("../a".into()), ..New::of("x") }).unwrap()[0].1.clone();
        let web = "[target.'cfg(target_arch = \"wasm32\")'.dependencies]\n\
                   aio = { package = \"aio-rs\", path = \"../a/aio\", features = [\"web\"] }\nwasm-bindgen = \"0.2\"\n";
        assert!(root.contains(web) && !root.contains("crate-type"), "{root}");
        assert!(APP_LIB.contains("#[wasm_bindgen::prelude::wasm_bindgen(start)]") && APP_LIB.contains("launch(App)"));
        assert!(!get(&of("x", "cli"), "Cargo.toml").contains("wasm32"), "a tool has no page");
        assert!(!get(&of("x", "cli"), "Cargo.toml").contains("dioxus"), "a tool doesn't");
        let game = files(&New { template: "game", ..New::of("x") });
        assert!(game.unwrap_err().contains("one of app, cli, service"));
    }

    #[test]
    fn targets_and_tailwind_go_in_aio_toml_and_only_where_they_fit() {
        let all = files(&New { targets: vec!["deb", "android"], tailwind: true, ..New::of("x") }).unwrap();
        let cfg = aio::config::Config::parse(get(&all, "aio.toml")).unwrap();
        let kinds: Vec<&str> = cfg.build.targets.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(kinds, ["deb", "android"]);
        assert_eq!(cfg.build.css.unwrap().0, ["tailwindcss", "-i", "src/app.css", "-o", "assets/style.css"]);
        let source = get(&all, "src/app.css");
        assert!(
            source.contains("@import \"tailwindcss\";") && source.ends_with(STYLE_CSS),
            "the template's styles kept"
        );
        assert_eq!(get(&all, "assets/style.css"), STYLE_CSS, "cargo and ax test build before Tailwind ever ran");
        assert!(get(&all, "README.md").contains("src/app.css"));
        assert!(aio::config::Config::parse(get(&of("x", "app"), "aio.toml")).unwrap().build == Default::default());
        let e = |n: New| files(&n).unwrap_err();
        assert!(e(New { targets: vec!["apk"], ..New::of("x") }).contains("--target apk: one of desktop, windows, deb"));
        assert!(e(New { template: "cli", targets: vec!["android"], ..New::of("x") }).contains("a window app's"));
        assert!(e(New { template: "service", tailwind: true, ..New::of("x") }).contains("--css tailwind: a window"));
        let cli = files(&New { template: "cli", targets: vec!["cli", "deb"], ..New::of("x") }).unwrap();
        assert!(get(&cli, "aio.toml").contains("[build]\ntargets = [\"cli\", \"deb\"]"));
    }

    #[test]
    fn a_version_is_a_release_tag_main_or_the_newest_release() {
        let none = || -> Result<String, String> { panic!("asked for no tags") };
        assert_eq!(Aio::version("0.2.0", none), Ok(Aio::Tag("v0.2.0".into())));
        assert_eq!(Aio::version("v0.2.0", none), Ok(Aio::Tag("v0.2.0".into())));
        assert_eq!(Aio::version("0.2", none), Ok(Aio::Tag("v0.2.0".into())), "a tag has three numbers");
        assert_eq!(Aio::version("0.3.0-beta.1", none), Ok(Aio::Tag("v0.3.0-beta.1".into())));
        assert_eq!(Aio::version("main", none), Ok(Aio::Branch("main".into())));
        assert!(Aio::version("newest", none).unwrap_err().contains("a release (0.2.0), main or latest"));
        let ls = "a1\trefs/tags/v0.1.0\nb2\trefs/tags/v0.10.0-beta.1\nc3\trefs/tags/v0.2.0\nd4\trefs/tags/v0.2.0^{}\n\
                  e5\trefs/tags/nightly\nf6\trefs/tags/v0.9.0\n";
        assert_eq!(Aio::version("latest", || Ok(ls.into())), Ok(Aio::Tag("v0.9.0".into())), "a release, not a beta");
        assert_eq!(
            newest("a1\trefs/tags/v0.3.0-alpha2\nb2\trefs/tags/v0.3.0-alpha10\n").as_deref(),
            Some("v0.3.0-alpha10")
        );
        assert_eq!(newest(""), None);
        assert!(Aio::version("latest", || Ok(String::new())).unwrap_err().contains("no release tag"));
        assert_eq!(Aio::this(), Aio::Tag(format!("v{}", env!("CARGO_PKG_VERSION"))));
    }

    #[test]
    fn a_checkout_is_named_relative_to_the_app() {
        assert_eq!(relative(Path::new("/a/b/app"), Path::new("/a/c/crates")).as_deref(), Some("../../c/crates"));
        assert_eq!(relative(Path::new("/a/app"), Path::new("/a/app/aio/crates")).as_deref(), Some("aio/crates"));
        assert_eq!(relative(Path::new("/a"), Path::new("/a")).as_deref(), Some("."));
        assert_eq!(relative(Path::new("/a"), Path::new("b")), None, "nothing in common: another drive");
        let through = relative(Path::new("/tmp/w/app"), Path::new("/home/me/aio-rs/crates"));
        assert_eq!(through, None, "a path through the root names this machine's folders");
        let d = tempfile::tempdir().unwrap();
        let aio = d.path().join("src/aio-rs");
        for c in ["aio", "aio-shell"] {
            std::fs::create_dir_all(aio.join("crates").join(c)).unwrap();
            std::fs::write(aio.join("crates").join(c).join("Cargo.toml"), "").unwrap();
        }
        std::fs::create_dir_all(d.path().join("apps")).unwrap();
        let app = d.path().join("apps/notes");
        let want = Aio::Path("../../src/aio-rs/crates".into());
        assert_eq!(Aio::checkout(&app, &aio), Ok(want.clone()));
        assert_eq!(Aio::checkout(&app, &aio.join("crates")), Ok(want), "its crates folder too");
        assert!(Aio::checkout(&app, d.path()).unwrap_err().contains("no aio-rs checkout there"));
        let src = Aio::Path("../../src/aio-rs/crates".into());
        assert_eq!(src.source("aio-shell", 1), r#"path = "../../../src/aio-rs/crates/aio-shell""#, "app/ is one down");
    }

    #[test]
    fn repoint_moves_every_aio_rs_dependency_and_nothing_else() {
        for (t, _) in TEMPLATES {
            let made = of("notes", t);
            for (rel, text) in made.iter().filter(|(f, _)| f.ends_with("Cargo.toml")) {
                let up = rel.matches('/').count();
                let path = repoint(text, &Aio::Path("../aio-rs/crates".into()), up).unwrap();
                let dots = "../".repeat(up);
                assert!(
                    path.contains(&format!(r#"package = "aio-rs", path = "{dots}../aio-rs/crates/aio""#)),
                    "{path}"
                );
                assert!(!path.contains("git = "), "{path}");
                assert_eq!(path.contains("path = \"..\""), text.contains("path = \"..\""), "the app's own path stays");
                let back = repoint(&path, &Aio::this(), up).unwrap();
                assert_eq!(&back, text, "{t} {rel}: there and back, the same file");
                let main = repoint(text, &Aio::Branch("main".into()), up).unwrap();
                assert!(main.contains(r#"git = "https://github.com/riagentic/aio-rs", branch = "main""#));
                assert!(!main.contains("tag = "), "{main}");
            }
        }
        let mine = "[dependencies]\naio = { path = \"../x/aio\", package = \"aio-rs\", features = [\"kit\"] }\nserde = \"1\"\n";
        let out = repoint(mine, &Aio::Tag("v0.2.0".into()), 0).unwrap();
        let want = "aio = { package = \"aio-rs\", git = \"https://github.com/riagentic/aio-rs\", tag = \"v0.2.0\", features = [\"kit\"] }\n";
        assert!(out.contains(want) && out.ends_with("serde = \"1\"\n"), "{out}");
        let rev = "x = { package = \"aio-rs-macros\", git = \"https://example.org/aio\", rev = \"abc\" }\n";
        assert_eq!(
            repoint(rev, &Aio::Path("../c".into()), 0).unwrap(),
            "x = { package = \"aio-rs-macros\", path = \"../c/aio-macros\" }\n"
        );
        assert_eq!(repoint("[dependencies]\nserde = \"1\"\n", &Aio::this(), 0), None);
    }

    /// The bar of CR1/CR2: a new app holds nothing of the machine it was made
    /// on (the creator's home, the folder made in, an aio-rs checkout), so a
    /// clone builds anywhere and leaks no one's paths.
    #[test]
    fn a_new_app_holds_no_path_of_the_machine_it_was_made_on() {
        let d = tempfile::tempdir().unwrap();
        let dir = std::fs::canonicalize(d.path()).unwrap();
        let checkout = dir.join("code/aio-rs");
        for c in ["aio", "aio-shell"] {
            std::fs::create_dir_all(checkout.join("crates").join(c)).unwrap();
            std::fs::write(checkout.join("crates").join(c).join("Cargo.toml"), "").unwrap();
        }
        let (target, installed) = (dir.join("target"), dir.join("install"));
        let home = std::env::home_dir().filter(|h| h.components().count() > 1);
        let mut machine: Vec<String> = [Some(dir.clone()), Some(checkout.clone()), home]
            .into_iter()
            .flatten()
            .chain(this_checkout().map(Path::to_path_buf))
            .flat_map(|p| [p.display().to_string(), p.display().to_string().replace('\\', "/")])
            .collect();
        machine.push(env!("CARGO_MANIFEST_DIR").into());
        for (i, (t, _)) in TEMPLATES.iter().enumerate() {
            let (at, name) = (dir.join(format!("apps{i}")), ["win", "tool", "board"][i]);
            std::fs::create_dir_all(&at).unwrap();
            let local = Aio::checkout(&at.join(name), &checkout).unwrap();
            assert_eq!(local, Aio::Path("../../code/aio-rs/crates".into()));
            for aio in [Aio::this(), Aio::Branch("main".into()), local] {
                let n = New { template: t, tailwind: *t == "app", targets: vec!["deb"], aio, ..New::of(name) };
                let made = run(&at, &n, (&target, &installed), true).unwrap();
                let mut seen = 0;
                let mut todo = vec![made.at.clone()];
                while let Some(p) = todo.pop() {
                    if p.is_dir() {
                        todo.extend(std::fs::read_dir(&p).unwrap().map(|e| e.unwrap().path()));
                        continue;
                    }
                    seen += 1;
                    let text = std::fs::read_to_string(&p).unwrap();
                    for m in &machine {
                        assert!(!text.contains(m.as_str()), "{}: holds {m}:\n{text}", p.display());
                    }
                }
                assert_eq!(seen, made.files.len(), "every file looked at");
            }
        }
    }

    #[test]
    fn framework_says_and_moves_the_apps_aio_rs() {
        let no_lock = |_: &Path| Ok(());
        let d = tempfile::tempdir().unwrap();
        let (target, installed) = (d.path().join("target"), d.path().join("install"));
        let made = run(d.path(), &New { template: "cli", ..New::of("notes") }, (&target, &installed), false).unwrap();
        let root = app_root(&made.at.join("app/src")).unwrap();
        assert_eq!(root, made.at, "from inside it");
        let tag = format!("v{}", env!("CARGO_PKG_VERSION"));
        let said = framework(&root, None, no_lock).unwrap();
        assert!(
            said.starts_with("Cargo.toml: aio = { package = \"aio-rs\", git =")
                && said.contains("app/Cargo.toml: aio-shell"),
            "{said}"
        );
        assert!(said.contains(&format!("tag = \"{tag}\"")), "{said}");
        let said = framework(&root, Some(&Aio::Tag("v9.9.9".into())), no_lock).unwrap();
        assert_eq!(
            said,
            "Cargo.toml: aio-rs v9.9.9\napp/Cargo.toml: aio-rs v9.9.9\nCargo.lock: updated (commit it with the change)"
        );
        let app = std::fs::read_to_string(root.join("app/Cargo.toml")).unwrap();
        assert!(
            app.contains(r#"package = "aio-rs-shell", git = "https://github.com/riagentic/aio-rs", tag = "v9.9.9" }"#),
            "{app}"
        );
        assert!(!framework(&root, None, no_lock).unwrap().contains(&tag));
        // Cargo refuses the move (no such tag, no network): every file as it was.
        let before = std::fs::read_to_string(root.join("app/Cargo.toml")).unwrap();
        let e = framework(&root, Some(&Aio::Branch("gone".into())), |_| Err("no branch gone".into())).unwrap_err();
        assert_eq!(e, "no branch gone\nnothing changed");
        assert_eq!(std::fs::read_to_string(root.join("app/Cargo.toml")).unwrap(), before);
        // An app's other crates are its own: a dot folder or build output is never read.
        std::fs::create_dir_all(root.join("target/x")).unwrap();
        std::fs::write(root.join("target/x/Cargo.toml"), r#"a = { package = "aio-rs", path = "/p" }"#).unwrap();
        assert_eq!(framework(&root, None, no_lock).unwrap().lines().count(), 3, "three aio-rs lines in two files");
        let bare = d.path().join("bare");
        std::fs::create_dir_all(&bare).unwrap();
        std::fs::write(bare.join("Cargo.toml"), "[package]\nname = \"bare\"\n").unwrap();
        std::fs::write(bare.join("aio.toml"), "").unwrap();
        assert!(framework(&bare, None, no_lock).unwrap_err().contains("no aio-rs dependency"));
        assert!(app_root(d.path()).unwrap_err().contains("in no app"));
    }

    #[test]
    fn a_failed_create_leaves_nothing_and_puts_back_what_it_wrote_over() {
        let d = tempfile::tempdir().unwrap();
        let (target, installed) = (d.path().join("target"), d.path().join("install"));
        let at = d.path().join("notes");
        std::fs::create_dir_all(at.join("src/lib.rs")).unwrap();
        std::fs::write(at.join("aio.toml"), "mine").unwrap();
        let e = run(d.path(), &New { template: "cli", ..New::of("notes") }, (&target, &installed), true).err().unwrap();
        assert!(e.contains("lib.rs") && e.contains("nothing of the new app is left"), "{e}");
        assert_eq!(std::fs::read_to_string(at.join("aio.toml")).unwrap(), "mine", "put back");
        let mut left: Vec<String> =
            std::fs::read_dir(&at).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["aio.toml", "src"], "Cargo.toml made, then gone");
        assert!(at.join("src/lib.rs").is_dir(), "what was there stays");
    }

    #[test]
    fn git_init_makes_the_first_commit_never_a_repository_inside_another() {
        if std::process::Command::new("git").arg("--version").output().is_err() {
            eprintln!("no git here: skipped");
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let (target, installed) = (d.path().join("target"), d.path().join("install"));
        let made = run(d.path(), &New { template: "cli", ..New::of("notes") }, (&target, &installed), false).unwrap();
        let said = git_init(&made.at, false);
        if said.starts_with("git: inside") {
            eprintln!("the temp dir is in a repository: {said}");
            return;
        }
        assert_eq!(said, "git: a repository with its first commit");
        let git = |a: &[&str]| {
            let o = std::process::Command::new("git").args(a).current_dir(&made.at).output().unwrap();
            String::from_utf8(o.stdout).unwrap()
        };
        assert_eq!(git(&["rev-list", "--count", "HEAD"]).trim(), "1");
        assert_eq!(git(&["status", "--porcelain"]), "", "every file in it");
        assert!(git_init(&made.at.join("src"), false).starts_with("git: inside"), "never one inside another");
    }

    #[test]
    fn an_earlier_apps_data_under_the_name_is_said() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(prior_data("notes", d.path()), None);
        std::fs::write(d.path().join("state.db"), "").unwrap();
        let said = prior_data("notes", d.path()).unwrap();
        assert!(said.contains(&d.path().display().to_string()) && said.contains("ax data notes"), "{said}");
    }

    #[test]
    fn a_name_is_one_a_package_binary_and_id_can_be() {
        assert!(name_ok("notes").is_ok() && name_ok("my-tool2").is_ok());
        for bad in ["", "My", "2x", "a_b", "a.b", "x-", "app", "my-app", "../x"] {
            assert!(name_ok(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn create_writes_into_a_new_folder_and_never_over_one_that_holds_files() {
        let d = tempfile::tempdir().unwrap();
        let (target, installed) = (d.path().join("target"), d.path().join("install"));
        let cli = |name| New { template: "cli", ..New::of(name) };
        let made = run(d.path(), &cli("notes"), (&target, &installed), false).unwrap();
        assert!(made.said(false).contains("ax dev notes") && made.said(false).contains("(cli, aio-rs v"));
        let json: serde_json::Value = serde_json::from_str(&made.said(true)).unwrap();
        assert_eq!(json["next"][1], "ax dev notes");
        assert!(json["files"].as_array().unwrap().iter().any(|f| f == "app/Cargo.toml"), "{json}");
        let lib = std::fs::read_to_string(d.path().join("notes/src/lib.rs")).unwrap();
        assert_eq!(lib, CLI_LIB);
        let mine = d.path().join("notes/src/lib.rs");
        std::fs::write(&mine, "// mine").unwrap();
        let again = run(d.path(), &New::of("notes"), (&target, &installed), false);
        assert!(again.err().unwrap().contains("--force"));
        assert_eq!(std::fs::read_to_string(&mine).unwrap(), "// mine", "nothing written over");
        run(d.path(), &New::of("notes"), (&target, &installed), true).unwrap();
        assert_eq!(std::fs::read_to_string(&mine).unwrap(), APP_LIB);
        let elsewhere = d.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let e = run(&elsewhere, &cli("notes"), (&target, &installed), false).err().unwrap();
        assert!(e.contains("binary `notes` is already"), "one name, one app in the shared build folder: {e}");
        // An installed app's id is never a new app's: they would share its data.
        std::fs::create_dir_all(installed.join("todo")).unwrap();
        let e = run(d.path(), &cli("todo"), (&target, &installed), false).err().unwrap();
        assert!(e.contains("id `todo` is an installed app's") && e.contains("--id <id> (todo-dev)"), "{e}");
        assert!(!d.path().join("todo").exists(), "nothing written");
        run(d.path(), &New { id: "todo-dev", ..cli("todo") }, (&target, &installed), false).unwrap();
        let toml = std::fs::read_to_string(d.path().join("todo/aio.toml")).unwrap();
        assert!(toml.contains("id = \"todo-dev\"") && toml.contains("name = \"todo\""), "{toml}");
        let bad = run(d.path(), &New { id: "Bad", ..cli("x") }, (&target, &installed), false).err().unwrap();
        assert!(bad.starts_with("--id Bad: a name of lowercase"), "{bad}");
    }
}

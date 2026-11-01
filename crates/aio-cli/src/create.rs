//! `ax create <name> [--template app|cli|service] [--aio dir] [--force]`
//! (aio's `am create`): a new app that runs, builds and tests as it is: its
//! library (cell and window, or commands), the launcher `ax dev` and `aio
//! build` run, `aio.toml` with every default visible, the agent brief's
//! pointer, and its own Cargo workspace. The Rust it writes is compiled and
//! its tests run as aio's own (below), so a template can't rot.
use std::path::Path;

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

/// The files of app `name` from template `template`, its aio at `aio` (the
/// checkout's `crates` dir), its Debian maintainer `maintainer`, as (path, text) (pure).
pub fn files(
    name: &str,
    template: &str,
    aio: &str,
    id: &str,
    maintainer: &str,
) -> Result<Vec<(String, String)>, String> {
    name_ok(name)?;
    name_ok(id).map_err(|e| format!("--id {e}"))?;
    let (lib, main, mode, extra) = match template {
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
    let dioxus = r#"dioxus = { version = "=0.7.10", default-features = false, features = ["macro", "html", "signals", "hooks"] }"#;
    // The window in a browser: aio's page half and the `start` binding.
    let web = format!(
        r#"
# The browser build (`ax dev --web`, `ax build --web`): aio in the page.
[target.'cfg(target_arch = "wasm32")'.dependencies]
aio = {{ package = "aio-rs", path = "{aio}/aio", features = ["web"] }}
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

[dependencies]
aio = {{ package = "aio-rs", path = "{aio}/aio"{feats} }}
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
aio = {{ package = "aio-rs", path = "{aio}/aio" }}
aio-shell = {{ package = "aio-rs-shell", path = "{aio}/aio-shell" }}
"#
    );
    let toml = format!(
        r#"# aio.toml: the app's one config file. Every default is visible here.
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
{extra}"#
    );
    // The guide beside the crates of the aio-rs checkout the app builds on.
    let guide = format!("{}/docs", aio.strip_suffix("/crates").unwrap_or(aio));
    let agents = format!(
        "# {name}\n\nAn aio-rs app. Before editing, run `ax agent`: how aio-rs works, the verbs, and the rules\n\
         that protect the person (never kill by process match, never take over their screen).\n\
         The guide, every part with examples: `{guide}/README.md` (porting an aio app:\n\
         `{guide}/migration/README.md`).\n\n\
         - `ax dev {name}` runs it; `ax test {name}` tests it; `ax build {name}` makes its installable app\n  \
         (all three build into ax's one build folder per machine; plain `cargo` builds its own `target/`).\n{}\
         - A build or test that can't find a tool: `ax setup --check` names each missing one and the command\n  \
         that gets it.\n\
         - `src/lib.rs` is the app: its cell and its {}.\n{}",
        if ui {
            "- The same window on the other platforms: `ax dev {name} --web` / `ax build {name} --web` (in a\n  \
             browser, `src/lib.rs`'s `web`), `ax build {name} --android` (`app/src/lib.rs`).\n"
                .replace("{name}", name)
        } else {
            String::new()
        },
        if ui {
            "window"
        } else if template == "cli" {
            "commands"
        } else {
            "served methods"
        },
        if ui {
            "- Styles live in `assets/style.css`, read by `aio::use_css!`: a saved edit shows in the window at\n  \
             once while `ax dev` runs, no compile. Never keep CSS (or anything an edit should change) in\n  \
             `use_hook`: a hook's value is made once, so a hot-patched edit never shows (`ax lint` says so).\n"
        } else {
            ""
        }
    );
    let toolchain = format!(
        "# The Rust ax builds with: one compiler for every app, so ax's shared build\n\
         # folder is reused. `ax setup` names the targets Android and the web add.\n\
         [toolchain]\nchannel = \"{}\"\n",
        crate::setup::toolchain()
    );
    let style = ui.then(|| ("assets/style.css".into(), STYLE_CSS.into()));
    let android = ui.then(|| ("app/src/lib.rs".into(), APP_ANDROID.into()));
    Ok([
        ("Cargo.toml".into(), root),
        ("aio.toml".into(), toml),
        ("src/lib.rs".into(), lib.into()),
        ("app/Cargo.toml".into(), launcher),
        ("app/src/main.rs".into(), main.into()),
        (".cargo/config.toml".into(), CARGO_CONFIG.into()),
        ("rust-toolchain.toml".into(), toolchain),
        (".gitignore".into(), "/target\n/.data\n/dist\n".into()),
        ("AGENTS.md".into(), agents),
        ("CLAUDE.md".into(), "@AGENTS.md\n".into()),
    ]
    .into_iter()
    .chain(style)
    .chain(android)
    .collect())
}

/// Write app `name` into `dir/name`; a folder that holds anything is
/// refused unless `force` (then only the template's files are written), and
/// so is a name another app builds in the shared build folder `target`.
/// Its id is `id` (`--id`), else its name; one an app installed in
/// `installed` has is refused: the two would share one home and its data.
pub fn run(
    dir: &Path,
    (name, id): (&str, Option<&str>),
    template: &str,
    aio: &Path,
    (target, installed): (&Path, &Path),
    force: bool,
) -> Result<String, String> {
    let id = id.unwrap_or(name);
    if installed.join(id).exists() {
        return Err(format!(
            "id `{id}` is an installed app's ({}): the new app would share its data; name it apart with \
             --id <id> ({name}-dev), or `ax remove {id}` first",
            installed.join(id).display()
        ));
    }
    let crates = aio.join("crates");
    let crates = if crates.join("aio/Cargo.toml").exists() { crates } else { aio.to_path_buf() };
    if !crates.join("aio/Cargo.toml").exists() || !crates.join("aio-shell/Cargo.toml").exists() {
        return Err(format!(
            "--aio {}: no aio-rs checkout there (crates/aio, crates/aio-shell); a prebuilt ax was built elsewhere: \
             name yours with --aio <dir> (git clone https://github.com/riagentic/aio-rs first)",
            aio.display()
        ));
    }
    let crates = std::path::absolute(&crates).map_err(|e| e.to_string())?;
    let maintainer = git_maintainer(dir).unwrap_or_else(|| NO_MAINTAINER.into());
    let all = files(name, template, &crates.display().to_string().replace('\\', "/"), id, &maintainer)?;
    let at = dir.join(name);
    if !force && std::fs::read_dir(&at).is_ok_and(|mut d| d.next().is_some()) {
        return Err(format!(
            "{} holds files already: another name, or --force to write the template over it",
            at.display()
        ));
    }
    crate::dev::claim(target, name, &at)?;
    for (rel, text) in &all {
        let p = at.join(rel);
        std::fs::create_dir_all(p.parent().unwrap_or(&at)).map_err(|e| format!("{}: {e}", p.display()))?;
        std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    let prior = aio::paths::locate(id).ok().and_then(|(data, ..)| prior_data(id, &data)).unwrap_or_default();
    Ok(format!(
        "created {} ({template}): cd {name}; ax dev {name} runs it, ax test {name} tests it, ax build {name} builds it{prior}",
        at.display()
    ))
}

/// Write the new app's `Cargo.lock` now, so its first commit holds it and
/// that commit's build is a release, not a dirty tree; "" when written, else
/// what to do (the first build writes it then).
pub fn lock(app: &Path) -> String {
    let done = std::process::Command::new("cargo")
        .args(["generate-lockfile", "--quiet"])
        .current_dir(app)
        .status()
        .is_ok_and(|s| s.success());
    match done {
        true => String::new(),
        false => "\n(no Cargo.lock yet: the first build writes it; commit it with the app)".into(),
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
            "\nnote: an app called {name} already keeps data in {}; the new one opens it (`ax data {name}` says what is there)",
            data.display()
        )
    })
}

/// The aio-rs checkout this `ax` was built from (`--aio` names another);
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

    #[test]
    fn each_template_writes_a_whole_app_with_its_config_valid() {
        for (t, _) in TEMPLATES {
            let all = files("my-tool", t, "/src/aio/crates", "my-tool", "A <a@example.org>").unwrap();
            let get = |p: &str| &all.iter().find(|(f, _)| f == p).unwrap_or_else(|| panic!("{t}: no {p}")).1;
            let cfg = aio::config::Config::parse(get("aio.toml")).unwrap_or_else(|e| panic!("{t}: {e}"));
            assert_eq!((cfg.app.id.as_str(), cfg.app.name.as_str()), ("my-tool", "my-tool"));
            assert_eq!(cfg.app.maintainer.as_deref(), Some("A <a@example.org>"), "ax build --deb works as created");
            assert!(get("Cargo.toml").contains(r#"path = "/src/aio/crates/aio""#));
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
            assert!(get("AGENTS.md").contains("ax agent"));
            assert!(get("AGENTS.md").contains("`/src/aio/docs/README.md`"), "the guide of the aio-rs it builds on");
            assert!(get(".gitignore").contains("/.data"));
            let tc = format!("[toolchain]\nchannel = \"{}\"\n", crate::setup::toolchain());
            assert!(get("rust-toolchain.toml").ends_with(&tc), "aio's own Rust: {}", get("rust-toolchain.toml"));
            let flags = get(".cargo/config.toml").lines().find(|l| l.starts_with("LIBSQLITE3_FLAGS = \"")).unwrap();
            assert!(flags.ends_with("MEMSTATUS=0\"") && flags.contains("FTS5 -USQLITE_ENABLE_RTREE"), "{flags}");
        }
        let main = |t| {
            files("x", t, "/a", "x", NO_MAINTAINER)
                .unwrap()
                .into_iter()
                .find(|(f, _)| f == "app/src/main.rs")
                .unwrap()
                .1
        };
        assert!(
            main("service").contains("for_server(") && main("app").contains("for_app("),
            "a server holds no client"
        );
        let mode =
            |t| aio::config::Config::parse(&files("x", t, "/a", "x", NO_MAINTAINER).unwrap()[1].1).unwrap().app.mode;
        use aio::config::Mode;
        assert_eq!([mode("app"), mode("cli"), mode("service")], [Mode::Standalone, Mode::Cli, Mode::Server]);
        assert!(files("x", "app", "/a", "x", NO_MAINTAINER).unwrap()[0].1.contains("dioxus"), "a window needs Dioxus");
        let android =
            |t| files("x", t, "/a", "x", NO_MAINTAINER).unwrap().into_iter().find(|(f, _)| f == "app/src/lib.rs");
        let lib = android("app").expect("a window app builds for Android as it is").1;
        assert!(
            lib.contains("#![cfg(target_os = \"android\")]")
                && lib.contains("android_app!")
                && lib.contains("root: app::App")
        );
        assert!(android("cli").is_none() && android("service").is_none(), "no window, no Android app");
        // The web: the app's library binds itself in the page (ax build --web links it as a cdylib).
        let root = &files("x", "app", "/a", "x", NO_MAINTAINER).unwrap()[0].1;
        let web = "[target.'cfg(target_arch = \"wasm32\")'.dependencies]\n\
                   aio = { package = \"aio-rs\", path = \"/a/aio\", features = [\"web\"] }\nwasm-bindgen = \"0.2\"\n";
        assert!(root.contains(web) && !root.contains("crate-type"), "{root}");
        assert!(APP_LIB.contains("#[wasm_bindgen::prelude::wasm_bindgen(start)]") && APP_LIB.contains("launch(App)"));
        assert!(!files("x", "cli", "/a", "x", NO_MAINTAINER).unwrap()[0].1.contains("wasm32"), "a tool has no page");
        assert!(!files("x", "cli", "/a", "x", NO_MAINTAINER).unwrap()[0].1.contains("dioxus"), "a tool doesn't");
        assert!(files("x", "game", "/a", "x", NO_MAINTAINER).unwrap_err().contains("one of app, cli, service"));
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
        let aio = d.path().join("aio");
        for c in ["aio", "aio-shell"] {
            std::fs::create_dir_all(aio.join("crates").join(c)).unwrap();
            std::fs::write(aio.join("crates").join(c).join("Cargo.toml"), "").unwrap();
        }
        let (target, installed) = (d.path().join("target"), d.path().join("install"));
        let said = run(d.path(), ("notes", None), "cli", &aio, (&target, &installed), false).unwrap();
        let root = std::fs::read_to_string(d.path().join("notes/Cargo.toml")).unwrap();
        let at = std::path::absolute(aio.join("crates")).unwrap().display().to_string().replace('\\', "/");
        assert!(root.contains(&format!("path = \"{at}/aio\"")), "{root}");
        assert!(said.contains("ax dev notes"), "{said}");
        let lib = std::fs::read_to_string(d.path().join("notes/src/lib.rs")).unwrap();
        assert_eq!(lib, CLI_LIB);
        let mine = d.path().join("notes/src/lib.rs");
        std::fs::write(&mine, "// mine").unwrap();
        assert!(
            run(d.path(), ("notes", None), "app", &aio, (&target, &installed), false).unwrap_err().contains("--force")
        );
        assert_eq!(std::fs::read_to_string(&mine).unwrap(), "// mine", "nothing written over");
        run(d.path(), ("notes", None), "app", &aio, (&target, &installed), true).unwrap();
        assert_eq!(std::fs::read_to_string(&mine).unwrap(), APP_LIB);
        assert!(
            run(d.path(), ("other", None), "app", d.path(), (&target, &installed), false)
                .unwrap_err()
                .contains("no aio-rs checkout")
        );
        let elsewhere = d.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let e = run(&elsewhere, ("notes", None), "cli", &aio, (&target, &installed), false).unwrap_err();
        assert!(e.contains("binary `notes` is already"), "one name, one app in the shared build folder: {e}");
        // An installed app's id is never a new app's: they would share its data.
        std::fs::create_dir_all(installed.join("todo")).unwrap();
        let e = run(d.path(), ("todo", None), "cli", &aio, (&target, &installed), false).unwrap_err();
        assert!(e.contains("id `todo` is an installed app's") && e.contains("--id <id> (todo-dev)"), "{e}");
        assert!(!d.path().join("todo").exists(), "nothing written");
        run(d.path(), ("todo", Some("todo-dev")), "cli", &aio, (&target, &installed), false).unwrap();
        let toml = std::fs::read_to_string(d.path().join("todo/aio.toml")).unwrap();
        assert!(toml.contains("id = \"todo-dev\"") && toml.contains("name = \"todo\""), "{toml}");
        let made = aio::config::Config::parse(&toml).unwrap().app.maintainer;
        assert_eq!(made, Some(git_maintainer(d.path()).unwrap_or_else(|| NO_MAINTAINER.into())), "{toml}");
        let bad = run(d.path(), ("x", Some("Bad")), "cli", &aio, (&target, &installed), false).unwrap_err();
        assert!(bad.starts_with("--id Bad: a name of lowercase"), "{bad}");
    }
}

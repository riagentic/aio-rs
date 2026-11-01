//! `ax preview <app> <component> [props] [--html]` (aio's `am preview`): one
//! component of the app, mounted alone with the props given (Rust, as they
//! are written in `rsx!`: `name: "Ada".into(), times: 2`), and what it
//! shows: its surface (as `ax surface` and TestUi read it), or its HTML. An
//! empty state, an error card, a long name, without driving the app there.
//! It is a test of the app's, written for the run and removed after: the
//! props are type-checked by the compiler, the component renders in the
//! TestUi page its tests use (the pinned Electron runtime, no window shown).
use std::path::Path;

/// The test a preview runs as, `tests/aio_preview.rs` in the app.
const NAME: &str = "aio_preview";
/// What the preview prints between, to be told from the test's own output.
const BEGIN: &str = "--- ax preview ---";
const END: &str = "--- ax preview end ---";

/// The test that mounts `component` (a path in crate `krate`: `ui::Card`)
/// with `props` (none: a component without props), printing its surface or
/// `html` (pure).
pub fn test(krate: &str, component: &str, props: Option<&str>, html: bool) -> String {
    let c = format!("{krate}::{component}");
    let ui = match props {
        Some(p) => format!("aio::testing::TestUi::with({c}, {c}Props {{ {p} }})"),
        None => format!("aio::testing::TestUi::new({c})"),
    };
    let show = if html { "html" } else { "surface" };
    format!(
        "//! Written by `ax preview` for one run, then removed.\n\
         #[test]\n\
         fn aio_preview() {{\n    \
         let ui = {ui};\n    \
         println!(\"\\n{BEGIN}\\n{{}}\\n{END}\", ui.{show}());\n\
         }}\n"
    )
}

/// What the preview printed, out of the test run's output (pure).
pub fn shown(out: &str) -> Option<String> {
    between(out, BEGIN, END)
}

/// What a written test printed between the lines `begin` and `end` (pure).
pub fn between(out: &str, begin: &str, end: &str) -> Option<String> {
    let (_, rest) = out.split_once(&format!("{begin}\n"))?;
    Some(rest.split_once(&format!("\n{end}"))?.0.to_string())
}

/// Preview `component` of the app library `pkg` (its package, in `dir`).
pub fn run(pkg: &str, dir: &Path, component: &str, props: Option<&str>, html: bool) -> Result<String, String> {
    if component.is_empty() || !component.split("::").all(|p| p.chars().all(|c| c.is_alphanumeric() || c == '_')) {
        return Err(format!("{component}: a component's path in the app's crate (ui::Card)"));
    }
    if props.is_some_and(|p| p.trim().is_empty()) {
        return Err("empty props: leave them out for a component without any, else name its fields".into());
    }
    let out = run_written(pkg, dir, NAME, &test(&pkg.replace('-', "_"), component, props, html), &[])
        .map_err(|e| format!("the preview didn't render:\n{e}"))?;
    shown(&out).ok_or_else(|| format!("the preview didn't render:\n{out}"))
}

/// Run `source` as the app's test `tests/<name>.rs` (its one test is
/// `name`), written for the run and removed after (and `tests/` when the
/// run made it), with `env` set: what it printed, or why it failed.
pub fn run_written(pkg: &str, dir: &Path, name: &str, source: &str, env: &[(&str, &str)]) -> Result<String, String> {
    let file = dir.join("tests").join(format!("{name}.rs"));
    if file.exists() {
        return Err(format!("{} is there already: another run of it is going, or remove it", file.display()));
    }
    // The file goes after the run, and `tests/` too when the run made it.
    struct Gone<'a>(&'a Path, bool);
    impl Drop for Gone<'_> {
        fn drop(&mut self) {
            _ = std::fs::remove_file(self.0);
            if self.1 {
                _ = self.0.parent().map(std::fs::remove_dir);
            }
        }
    }
    let made = !dir.join("tests").exists();
    std::fs::create_dir_all(dir.join("tests")).map_err(|e| e.to_string())?;
    let _gone = Gone(&file, made);
    std::fs::write(&file, source).map_err(|e| e.to_string())?;
    let out = std::process::Command::new("cargo")
        .args(["test", "-q", "-p", pkg, "--test", name])
        .args(crate::dev::layout())
        .args(["--", "--exact", name, "--nocapture"])
        .envs(env.iter().copied())
        .output()
        .map_err(|e| format!("can't run cargo: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    if out.status.success() {
        return Ok(text);
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let why: Vec<&str> = err.lines().filter(|l| !l.trim().is_empty()).take(40).collect();
    Err(format!("{}\n{}", why.join("\n"), text.trim()))
}

/// The preview `tests::the_preview_it_writes_mounts_and_shows` makes, as a
/// test of ours: it compiles, and it renders.
#[cfg(test)]
#[path = "preview_fixture.rs"]
#[rustfmt::skip]
mod written;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use aio::ui::prelude::*;

    #[component]
    pub fn Greeting(name: String, times: u32) -> View {
        rsx! { p { "data-t": "hi", "Hello {name} ×{times}" } button { "Wave" } }
    }

    #[test]
    fn the_preview_it_writes_mounts_and_shows() {
        let t = test("crate", "preview::tests::Greeting", Some("name: \"Ada\".into(), times: 2"), false);
        assert_eq!(t, include_str!("preview_fixture.rs"));
        assert!(test("app", "ui::App", None, true).contains("TestUi::new(app::ui::App)"));
        assert!(test("app", "ui::App", None, true).contains("ui.html()"));
        let ui = aio::testing::TestUi::with(Greeting, GreetingProps { name: "Ada".into(), times: 2 });
        let out = format!("running 1 test\n\n{BEGIN}\n{}\n{END}\ntest aio_preview ... ok\n", ui.surface());
        assert_eq!(shown(&out).unwrap(), "  generic \"Hello Ada ×2\" [hi]\n  button \"Wave\"");
        assert_eq!(shown("no preview"), None);
    }

    #[test]
    fn a_component_is_a_path_and_a_preview_never_takes_an_apps_file() {
        let d = tempfile::tempdir().unwrap();
        assert!(run("x", d.path(), "ui::Card; drop", None, false).unwrap_err().contains("a component's path"));
        assert!(run("x", d.path(), "Card", Some(" "), false).unwrap_err().contains("empty props"), "never ignored");
        std::fs::create_dir_all(d.path().join("tests")).unwrap();
        std::fs::write(d.path().join("tests").join(format!("{NAME}.rs")), "mine").unwrap();
        assert!(run("x", d.path(), "Card", None, false).unwrap_err().contains("is there already"));
        assert_eq!(std::fs::read_to_string(d.path().join("tests").join(format!("{NAME}.rs"))).unwrap(), "mine");
    }
}

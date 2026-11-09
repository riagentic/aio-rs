//! `ax check <app>` (aio's `deno task check` and `deno task test` in one):
//! what CI runs on every push, in order, each step the real command, the
//! run stopping at the first that fails: `cargo fmt --all --check`, `ax
//! lint`, with a web build its library checked for the page (`cargo check
//! --target wasm32-unknown-unknown`: aio's `am check`, does the client
//! graph build), `ax test <app>`. `--release` adds the release gate: `ax build
//! <app> --sizes` (the binary within its budget) and `ax build <app>
//! --smoke` (the bundle built, started headless, stopped, its log checked).
use std::time::Instant;

/// The steps (pure): a name and the command's words; `ax` is this binary,
/// `web` the package of the app's web build, if it has one.
pub fn steps(ax: &str, app: &str, web: Option<&str>, release: bool) -> Vec<(&'static str, Vec<String>)> {
    let w = |ws: &[&str]| ws.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let mut s = vec![("format", w(&["cargo", "fmt", "--all", "--check"])), ("lint", w(&[ax, "lint"]))];
    if let Some(p) = web {
        s.push(("web", w(&["cargo", "check", "--target", "wasm32-unknown-unknown", "-p", p, "--lib"])));
    }
    s.push(("tests", w(&[ax, "test", app])));
    if release {
        s.push(("size", w(&[ax, "build", app, "--sizes"])));
        s.push(("bundle", w(&[ax, "build", app, "--smoke"])));
    }
    s
}

/// Run `steps` in order with `exec` (true: it passed), saying each; the
/// first that fails ends the run, naming it.
pub fn run_steps(
    steps: Vec<(&'static str, Vec<String>)>,
    exec: impl Fn(&[String]) -> Result<bool, String>,
) -> Result<String, String> {
    let mut said = vec![];
    for (name, words) in steps {
        eprintln!("ax check: {name}: {}", words.join(" "));
        let t = Instant::now();
        let passed = exec(&words)?;
        let line = format!("{name}: {} ({:.1} s)", if passed { "ok" } else { "failed" }, t.elapsed().as_secs_f64());
        if !passed {
            said.push(line);
            return Err(format!(
                "{}\nax check stops at the first step that fails: fix {name}, then run it again",
                said.join("\n")
            ));
        }
        said.push(line);
    }
    Ok(said.join("\n"))
}

/// `ax check <app> [--release]`.
pub fn run(app: &str, release: bool) -> Result<String, String> {
    let ax = std::env::current_exe().map_err(|e| format!("ax: {e}"))?;
    let web = crate::dev::metadata().ok().and_then(|m| {
        let p = crate::dev::web_pkg(&m, app).ok()?;
        crate::web::web_lib(&m, &p.dirs).map(|(pkg, _)| pkg)
    });
    run_steps(steps(&ax.to_string_lossy(), app, web.as_deref(), release), |words| {
        std::process::Command::new(&words[0])
            .args(&words[1..])
            .status()
            .map(|s| s.success())
            .map_err(|e| format!("{}: {e}", words[0]))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_steps_are_cis_in_order_and_release_adds_the_gate() {
        let names = |web, r| {
            steps("ax", "notes", web, r).into_iter().map(|(n, w)| format!("{n}: {}", w.join(" "))).collect::<Vec<_>>()
        };
        assert_eq!(names(None, false), ["format: cargo fmt --all --check", "lint: ax lint", "tests: ax test notes"]);
        assert_eq!(names(None, true)[3..], ["size: ax build notes --sizes", "bundle: ax build notes --smoke"]);
        assert_eq!(
            names(Some("notes"), false)[2],
            "web: cargo check --target wasm32-unknown-unknown -p notes --lib",
            "aio's am check: the page's code builds"
        );
    }

    #[test]
    fn the_first_failing_step_ends_the_run_naming_it() {
        let ran = std::cell::RefCell::new(vec![]);
        let e = run_steps(steps("ax", "notes", None, true), |w| {
            ran.borrow_mut().push(w[1].clone());
            Ok(w[1] != "lint")
        })
        .unwrap_err();
        assert_eq!(*ran.borrow(), ["fmt", "lint"], "nothing after the failure ran");
        assert!(e.starts_with("format: ok (") && e.contains("lint: failed (") && e.contains("fix lint"), "{e}");
        let ok = run_steps(steps("ax", "notes", None, false), |_| Ok(true)).unwrap();
        assert_eq!(ok.lines().count(), 3);
        assert!(
            run_steps(steps("ax", "n", None, false), |_| Err("cargo: not found".into()))
                .is_err_and(|e| e == "cargo: not found")
        );
    }
}

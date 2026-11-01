//! The app's CSS toolchain (`[build] css`, aio's `build.css`): Tailwind, Sass
//! or a script, run in the app's dir before every build, so what
//! `include_str!` embeds is fresh. It fails loud: a step that did not run
//! would ship yesterday's stylesheet with every check green. A build stops on
//! it; `ax dev` says so and keeps the running app.
use std::path::Path;
use std::process::Command;
use std::time::Instant;

/// Run `argv` in `dir`: what it did, or why it failed (its output).
pub fn run(argv: &[String], dir: &Path) -> Result<String, String> {
    let command = argv.join(" ");
    let started = Instant::now();
    let out = Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("[build] css could not run `{command}`: {e}"))?;
    let ms = started.elapsed().as_millis();
    if !out.status.success() {
        let said = String::from_utf8_lossy(&out.stderr).into_owned() + &String::from_utf8_lossy(&out.stdout);
        return Err(format!("[build] css `{command}` failed ({}):\n{}", out.status, said.trim()));
    }
    Ok(format!("css: `{command}` ({ms} ms)"))
}

/// The app's step, if it declares one.
pub fn step(cfg: &aio::config::Config, dir: &Path) -> Result<Option<String>, String> {
    cfg.build.css.as_ref().map(|c| run(&c.0, dir)).transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn absent_means_nothing_runs() {
        let cfg = aio::config::Config::parse("[app]\nid = \"t\"").unwrap();
        assert_eq!(step(&cfg, Path::new("/nowhere")), Ok(None));
    }

    #[test]
    fn a_missing_command_names_the_command_and_the_reason() {
        let e = run(&argv(&["aio-no-such-css-tool", "-o", "x.css"]), &std::env::temp_dir()).unwrap_err();
        assert!(e.starts_with("[build] css could not run `aio-no-such-css-tool -o x.css`: "), "{e}");
    }

    #[cfg(unix)]
    #[test]
    fn a_step_runs_in_the_apps_dir_and_a_failing_one_fails_with_its_output() {
        let d = tempfile::tempdir().unwrap();
        let ok = run(&argv(&["sh", "-c", "printf 'a{}' > style.css"]), d.path()).unwrap();
        assert!(ok.starts_with("css: `sh -c printf 'a{}' > style.css` ("), "{ok}");
        assert_eq!(std::fs::read_to_string(d.path().join("style.css")).unwrap(), "a{}");
        let e = run(&argv(&["sh", "-c", "echo 'unknown class' >&2; exit 3"]), d.path()).unwrap_err();
        assert!(e.contains("failed (exit status: 3)") && e.ends_with("unknown class"), "{e}");
    }
}

//! `ax test <app> [filter] [--video out] [--pace ms]`: the app's tests
//! (`cargo test`); `--video` records each UI test with no change to it (aio
//! `deno test -- --video`): a frame per step, `out` a folder (one video per
//! test) or one .mp4/.webm file, each step `--pace` ms on screen. Every
//! package of the app is tested (its library holds the tests, its launcher
//! the binary), and a run in which no test ran fails: a green run tested.
use std::io::BufRead;
use std::process::{Command, Stdio};

/// The name the app's package gives aio (`aio = { package = "aio-rs" }`).
pub fn aio_dep(pkg: &serde_json::Value) -> Option<String> {
    let deps = pkg["dependencies"].as_array()?;
    let d = deps.iter().find(|d| d["name"] == "aio-rs")?;
    Some(d["rename"].as_str().or(d["name"].as_str())?.to_string())
}

/// The workspace packages in the app's dirs (pure): its launcher and its library.
pub fn packages(meta: &serde_json::Value, dirs: &[std::path::PathBuf]) -> Vec<String> {
    let pkgs = meta["packages"].as_array().into_iter().flatten();
    pkgs.filter(|p| {
        let dir = p["manifest_path"].as_str().map(std::path::Path::new).and_then(std::path::Path::parent);
        dir.is_some_and(|d| dirs.iter().any(|x| x == d))
    })
    .filter_map(|p| p["name"].as_str().map(String::from))
    .collect()
}

/// How many tests ran (passed or failed), summed over libtest's
/// `test result:` lines (pure).
pub fn ran(out: &str) -> usize {
    let count = |rest: &str, what: &str| -> usize {
        rest.split(';').find_map(|p| p.trim().strip_suffix(what)?.trim().parse().ok()).unwrap_or(0)
    };
    out.lines()
        .filter_map(|l| l.trim_start().strip_prefix("test result: ")?.split_once(". ").map(|(_, r)| r))
        .map(|r| count(r, "passed") + count(r, "failed"))
        .sum()
}

/// cargo's arguments (pure): with a video, aio's painter is built in.
pub fn args(pkgs: &[String], aio: &str, filter: Option<&str>, video: bool) -> Vec<String> {
    let mut a: Vec<String> = vec!["test".into()];
    for p in pkgs {
        a.extend(["-p".into(), p.clone()]);
    }
    if video {
        a.extend(["--features".into(), format!("{aio}/video")]);
    }
    a.extend(filter.map(String::from));
    a
}

/// Run app `name`'s tests, a video of each UI test into `video`.
pub fn run(name: &str, filter: Option<&str>, video: Option<&str>, pace: Option<&str>) -> Result<String, String> {
    if let Some(p) = pace
        && !p.parse::<u64>().is_ok_and(|ms| ms > 0)
    {
        return Err(format!("--pace {p}: a number of ms"));
    }
    let meta = crate::dev::metadata()?;
    let pkg = crate::dev::locate(&meta, name)?;
    let found = meta["packages"].as_array().into_iter().flatten().find(|p| p["name"] == pkg.name.as_str());
    let aio = found.and_then(aio_dep).ok_or(format!("{} doesn't depend on aio-rs", pkg.name))?;
    let all = packages(&meta, &pkg.dirs);
    let mut cmd = Command::new("cargo");
    cmd.args(args(&all, &aio, filter, video.is_some())).args(crate::dev::layout()).stdout(Stdio::piped());
    if let Some(v) = video {
        cmd.env("AIO_TEST_VIDEO", std::path::absolute(v).map_err(|e| e.to_string())?);
    }
    if let Some(p) = pace {
        cmd.env("AIO_TEST_VIDEO_PACE", p);
    }
    let mut child = cmd.spawn().map_err(|e| format!("can't run cargo: {e}"))?;
    // libtest's lines shown as they come, and kept to count what ran.
    let mut out = String::new();
    for line in std::io::BufReader::new(child.stdout.take().ok_or("cargo: no output")?).lines().map_while(Result::ok) {
        println!("{line}");
        out.push_str(&line);
        out.push('\n');
    }
    let ok = child.wait().map_err(|e| format!("cargo: {e}"))?.success();
    match (ok, ran(&out)) {
        (false, _) => Err(format!("{name}: tests failed")),
        (true, 0) => Err(match filter {
            Some(f) => format!("{name}: no test matches `{f}` (in {})", all.join(", ")),
            None => format!("{name}: no test ran (in {}): add a #[test] to src/lib.rs", all.join(", ")),
        }),
        (true, n) => Ok(format!("{name}: {n} test{} passed", if n == 1 { "" } else { "s" })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aio::serde_json::json;

    #[test]
    fn a_video_builds_aios_painter_in_under_the_apps_name_for_it() {
        let pkg = json!({ "dependencies": [
            { "name": "serde" },
            { "name": "aio-rs", "rename": "aio" },
        ]});
        assert_eq!(aio_dep(&pkg).as_deref(), Some("aio"));
        assert_eq!(aio_dep(&json!({ "dependencies": [{ "name": "aio-rs" }] })).as_deref(), Some("aio-rs"));
        assert_eq!(aio_dep(&json!({ "dependencies": [] })), None);
        let todo = ["todo-app".to_string(), "todo".into()];
        assert_eq!(args(&todo[1..], "aio", None, false), ["test", "-p", "todo"]);
        assert_eq!(
            args(&todo, "aio", Some("adds"), true),
            ["test", "-p", "todo-app", "-p", "todo", "--features", "aio/video", "adds"]
        );
        assert!(run("x", None, None, Some("0")).unwrap_err().contains("--pace 0"));
    }

    #[test]
    fn the_launcher_and_the_library_are_tested_and_a_run_of_no_test_is_none() {
        let meta = json!({ "packages": [
            { "name": "notes-app", "manifest_path": "/w/notes/app/Cargo.toml" },
            { "name": "notes", "manifest_path": "/w/notes/Cargo.toml" },
            { "name": "other", "manifest_path": "/w/other/Cargo.toml" },
        ]});
        let dirs = ["/w/notes/app".into(), "/w/notes".into()];
        assert_eq!(packages(&meta, &dirs), ["notes-app", "notes"]);
        let none = "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; \
                    finished in 0.00s\n";
        assert_eq!(ran(none), 0, "the launcher's main.rs alone: nothing tested");
        let two = format!(
            "{none}running 2 tests\ntest tests::a ... ok\n\ntest result: FAILED. 1 passed; 1 failed; 3 ignored; \
             0 measured; 0 filtered out; finished in 0.01s\n"
        );
        assert_eq!(ran(&two), 2, "ignored tests didn't run");
    }
}

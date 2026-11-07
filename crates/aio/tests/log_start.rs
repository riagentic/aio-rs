// Its own test binary: `start` takes over this process's stderr.
use std::io::Write;

#[test]
fn stderr_and_children_land_in_the_log() {
    let d = tempfile::tempdir().unwrap();
    aio::log::start(d.path()).unwrap();
    std::io::stderr().write_all(b"from the app\n").unwrap();
    #[cfg(unix)]
    std::process::Command::new("sh").args(["-c", "echo from a child >&2"]).status().unwrap();
    #[cfg(windows)]
    std::process::Command::new("cmd").args(["/C", "echo from a child 1>&2"]).status().unwrap();
    let log = d.path().join("app.log");
    // `finish` writes out what's still in flight: the last line is never lost.
    std::io::stderr().write_all(b"the last words\n").unwrap();
    aio::log::finish();
    assert!(std::fs::read_to_string(&log).unwrap().ends_with("the last words\n"));
    for _ in 0..100 {
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        if text.contains("from the app") && text.contains("from a child") {
            assert!(text.lines().all(|l| l.as_bytes().get(23) == Some(&b'Z')), "stamped: {text}");
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("not logged: {}", std::fs::read_to_string(&log).unwrap_or_default());
}

/// An app whose `main` returns an error (`Error: …`, printed by Rust after
/// `main`) never reaches `finish`: the exit itself writes the last line
/// out, to the log and the terminal.
#[test]
fn an_exit_without_finish_keeps_the_last_line() {
    if let Some(logs) = std::env::var_os("AIO_TEST_EXIT_LOGS") {
        aio::log::start(std::path::Path::new(&logs)).unwrap();
        // Many lines, so the copier is still behind at the exit.
        for i in 0..2000 {
            std::io::stderr().write_all(format!("line {i}\n").as_bytes()).unwrap();
        }
        std::io::stderr().write_all(b"Error: the reason\n").unwrap();
        if std::env::var_os("AIO_TEST_EXIT_STD").is_some() {
            std::process::exit(1);
        }
        // What a return from `main` ends in (C's `exit`; on Windows
        // `std::process::exit` skips it).
        unsafe extern "C" {
            fn exit(code: std::ffi::c_int) -> !;
        }
        // SAFETY: C's exit, as after `main`.
        unsafe { exit(1) };
    }
    let run = |std_exit: bool| {
        let d = tempfile::tempdir().unwrap();
        let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
        cmd.args(["--exact", "an_exit_without_finish_keeps_the_last_line", "--nocapture"]);
        cmd.env("AIO_TEST_EXIT_LOGS", d.path());
        if std_exit {
            cmd.env("AIO_TEST_EXIT_STD", "1");
        }
        let out = cmd.output().unwrap();
        (out.status.code(), std::fs::read_to_string(d.path().join("app.log")).unwrap(), out.stderr)
    };
    let (code, log, stderr) = run(false);
    assert_eq!(code, Some(1));
    assert!(log.ends_with("Error: the reason\n"), "the log: {}", &log[log.len().saturating_sub(200)..]);
    assert!(String::from_utf8_lossy(&stderr).contains("Error: the reason"), "the terminal");
    // `std::process::exit`: C's exit on Unix, so the same; on Windows it
    // ends the process at once, and still with its own code.
    let (code, log, _) = run(true);
    assert_eq!(code, Some(1));
    #[cfg(unix)]
    assert!(log.ends_with("Error: the reason\n"), "the log: {}", &log[log.len().saturating_sub(200)..]);
    _ = log;
}

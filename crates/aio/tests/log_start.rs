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

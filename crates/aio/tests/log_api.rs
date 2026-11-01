// Its own test binary: `start` takes over this process's stderr.
use aio::log::Level;

#[test]
fn macros_write_gated_tagged_lines_into_the_log() {
    let d = tempfile::tempdir().unwrap();
    aio::log::start(d.path()).unwrap();
    let (peer, n) = ("a1", 3);
    aio::info!("saved {n} notes");
    aio::warn!(cat: "net", peer = peer, tries = n + 1, "lost {} frames", 2);
    aio::error!(peer = peer, "bad type \"\nFORGED\"");
    // Below the level: dropped, and its fields are never evaluated.
    aio::debug!(x = never(), "chatty");
    aio::trace!("chattier");
    aio::log::set_level(Level::Trace);
    aio::trace!(cat: "deep", "now on");
    for _ in 0..3 {
        aio::info!("tick");
    }
    aio::log::set_level(Level::Error);
    aio::warn!("quiet now");
    aio::log::finish();
    let text = std::fs::read_to_string(d.path().join("app.log")).unwrap();
    let lines: Vec<_> = text.lines().map(|l| &l[25..]).collect();
    assert_eq!(
        lines,
        [
            "INFO  [app] saved 3 notes",
            "WARN  [net] lost 2 frames peer=a1 tries=4",
            "ERROR [app] bad type \"\\nFORGED\" peer=a1",
            "TRACE [deep] now on",
            "INFO  [app] tick",
            "… last line repeated 2 more times",
            "[aio] log closed: 1 error, 1 warning this run",
        ],
        "{text}"
    );
}

fn never() -> u8 {
    panic!("a field below the level was evaluated")
}

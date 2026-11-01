// Its own test binary: `start` takes over this process's stderr.
use aio::prelude::*;

#[cell]
pub struct Tick {
    pub n: u32,
    pub label: String,
}

#[methods]
impl Tick {
    pub fn tick(&mut self) {
        self.n += 1;
    }

    pub fn rename(&mut self, label: String) {
        self.label = label;
    }

    pub fn save(&mut self, secret: String) -> Result {
        ensure!(!secret.is_empty(), "empty");
        Ok(())
    }
}

#[test]
fn log_config_sets_level_files_heartbeat_and_suppressed_calls() {
    let d = tempfile::tempdir().unwrap();
    let cfg = aio::config::Config::parse(
        "[app]\nid = \"tick\"\n[log]\nlevel = \"debug\"\nheartbeat = 1\nsuppress = [\"tick.tick\"]",
    )
    .unwrap();
    aio::log::configure(&cfg.log);
    aio::log::start(d.path()).unwrap();
    let mut s = Store::<Tick>::memory();
    s.call(&By::Test, "tick", serde_json::Value::Null).unwrap();
    s.call(&By::Test, "save", serde_json::json!(["hunter2"])).unwrap();
    s.call(&By::Test, "save", serde_json::json!([""])).unwrap_err();
    s.call(&By::Test, "rename", serde_json::json!(["x"])).unwrap();
    aio::error!(cat: "db", "gone");
    aio::warn!("slow");
    let read = |f: &str| std::fs::read_to_string(d.path().join(f)).unwrap_or_default();
    for _ in 0..100 {
        if read("app.log").contains("heartbeat") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    aio::log::finish();
    let app = read("app.log");
    let calls: Vec<_> = app.lines().filter(|l| l[25..].starts_with("DEBUG [aio] call ")).collect();
    assert_eq!(calls.len(), 3, "tick.tick is suppressed: {app}");
    assert!(calls[0].contains("call tick.save took=") && !calls[0].contains("refused"), "{app}");
    assert!(calls[1].ends_with("refused=empty"), "{app}");
    assert!(!app.contains("hunter2"), "a call's arguments are never logged");
    // A dev build says what each call changed; a suppressed method, nothing.
    let diffs: Vec<_> = app.lines().filter(|l| l[25..].starts_with("DEBUG [diff] ")).collect();
    let said: Vec<_> = diffs.iter().map(|l| &l[25..]).collect();
    assert_eq!(said, ["DEBUG [diff] tick.rename: label \"\"→\"x\""], "save changes nothing, tick is suppressed");
    let beat = app.lines().find(|l| l[25..].starts_with("INFO  [aio] heartbeat uptime=")).expect(&app);
    assert!(beat.ends_with("calls=4 errors=1"), "{beat}");
    assert!(read("error.log").ends_with("ERROR [db] gone\n"));
    assert_eq!(read("warning.log").lines().count(), 1);
}

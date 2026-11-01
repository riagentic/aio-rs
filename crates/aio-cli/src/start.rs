//! `aio start` and `aio restart` (aio: `am start`, `am restart`): run an app
//! in the background, detached from the caller, and wait until it is up:
//! its lock held (the OS's, so a dead start leaves no zombie to clean) and a
//! `health.json` written by this start. What it prints meanwhile goes to
//! `logs/start.log`; a start that ends at once says why with its tail. The
//! launch is recorded (`logs/start.json`: args, binary, pid, who) so
//! `aio restart` replays its flags and `aio apps` says who started it.
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

type Out = Result<String, String>;

/// The launch record and the start's output, in the app's logs dir.
pub const RECORD: &str = "start.json";
pub const LOG: &str = "start.log";

/// How long a start waits for the app to come up, unless `--wait`.
pub const WAIT: Duration = Duration::from_secs(30);

/// What a start's wait sees, once: up (its line), ended (why), or not yet (pure).
pub fn verdict(ended: Option<String>, locked: bool, health: Option<&Value>, since: i64, now: i64) -> Option<Out> {
    if let Some(code) = ended {
        return Some(Err(format!("it ended at once ({code})")));
    }
    let doc = health.filter(|d| locked && d["updatedAt"].as_i64().is_some_and(|t| t >= since))?;
    if matches!(doc["status"].as_str(), Some("stopping" | "maintenance")) {
        return None;
    }
    Some(Ok(aio::health::judge(doc, now, true).unwrap_or_else(|e| e)))
}

/// A flag's name: `--port=1` and `--port` are one flag.
fn name(a: &str) -> &str {
    a.split_once('=').map_or(a, |(n, _)| n)
}

/// A recorded launch merged with what this restart typed (aio
/// `mergeLaunchFlags`), per flag: a typed flag overrides the recorded one
/// of its name, the rest is replayed; after a bare `--` are the app's
/// words, a typed list replacing the recorded one whole (pure). Returns
/// (launch, replayed, overridden).
pub fn merge(recorded: &[String], typed: &[String]) -> (Vec<String>, Vec<String>, Vec<String>) {
    let split = |a: &[String]| match a.iter().position(|w| w == "--") {
        Some(i) => (a[..i].to_vec(), Some(a[i..].to_vec())),
        None => (a.to_vec(), None),
    };
    let ((rec, rec_tail), (typ, typ_tail)) = (split(recorded), split(typed));
    let typed_names: Vec<&str> = typ.iter().filter(|a| a.starts_with("--")).map(|a| name(a)).collect();
    // A recorded flag's value (`--x v`) goes with it.
    let mut kept = vec![];
    let mut over = vec![];
    let mut i = 0;
    while i < rec.len() {
        let a = &rec[i];
        let takes = a.starts_with("--") && !a.contains('=') && rec.get(i + 1).is_some_and(|v| !v.starts_with("--"));
        let n = 1 + usize::from(takes);
        if a.starts_with("--") && typed_names.contains(&name(a)) {
            over.push(name(a).to_string());
        } else {
            kept.extend_from_slice(&rec[i..i + n]);
        }
        i += n;
    }
    let mut replayed = kept.clone();
    if typ_tail.is_none() {
        replayed.extend(rec_tail.clone().unwrap_or_default());
    } else if rec_tail.is_some() {
        over.push("--".into());
    }
    let mut launch = kept;
    launch.extend(typ);
    launch.extend(typ_tail.or(rec_tail).unwrap_or_default());
    (launch, replayed, over)
}

/// Who a running app's start was, from its record and its `health.json`
/// (pure): `aio start` when the record names the pid and start time the
/// app itself reports (a pid alone may be reused), else `other`.
pub fn started_by(record: &Value, health: &Value) -> &'static str {
    let same = record["pid"].as_u64().is_some() && record["pid"] == health["pid"];
    let at = record.get("startedAt").is_none_or(|a| *a == health["startedAt"]);
    if same && at { "aio start" } else { "other" }
}

/// The last `n` lines of `file`.
fn tail(file: &Path, n: usize) -> String {
    let text = std::fs::read_to_string(file).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// The record of the last start in `logs`, if any.
pub fn record(logs: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(logs.join(RECORD)).ok()?).ok()
}

/// What starts `bin` detached: nothing of the caller's terminal, its own
/// process group (a Ctrl-C there doesn't reach it), output into `log`.
pub(crate) fn command(bin: &Path, args: &[String], log: &Path, display: Option<&str>) -> Result<Command, String> {
    let out = std::fs::File::create(log).map_err(|e| format!("{}: {e}", log.display()))?;
    let err = out.try_clone().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(bin);
    cmd.args(args).stdin(Stdio::null()).stdout(out).stderr(err);
    crate::display::onto(&mut cmd, display);
    aio::lifecycle::scrub(&mut cmd, true);
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    #[cfg(windows)]
    // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP.
    std::os::windows::process::CommandExt::creation_flags(&mut cmd, 0x0000_0008 | 0x0000_0200);
    Ok(cmd)
}

/// Where the window goes for `asked` (`--display`, else `AIORT_DISPLAY`),
/// as `aio dev` decides it; a nested display started for it comes along,
/// handed to the app once it is up (it ends with its last window).
fn place(asked: Option<&str>) -> Result<(Option<String>, Option<crate::display::Nested>), String> {
    use crate::display::Choice;
    use std::io::IsTerminal;
    let asked = asked.map(String::from).or_else(|| std::env::var("AIORT_DISPLAY").ok());
    Ok(match crate::display::choose(asked.as_deref(), std::io::stdin().is_terminal(), crate::display::desktop())? {
        Choice::Current => (None, None),
        Choice::Named(d) => (Some(d), None),
        Choice::Nested => match crate::display::nest(asked.as_deref(), "aio start", false)? {
            Some(n) => (Some(n.name.clone()), Some(n)),
            None => (None, None),
        },
    })
}

/// Whether app `t` opens a window (its `aio.toml`, in its workspace): a
/// start of one is up once its window is ready, not just its health.
pub fn windowed(t: &crate::target::Target) -> bool {
    use aio::config::Mode;
    let cfg = t.pkg.as_ref().and_then(|p| crate::build::app_of(p).ok()).map(|(_, c)| c.app.mode);
    matches!(cfg, Some(Mode::Standalone | Mode::Client | Mode::ClientServer))
}

/// Start `bin args` for app `t` and wait at most `wait` for it to be up
/// (healthy, and its window ready when it has one), its window where
/// `asked` says (`--display`).
pub fn run(t: &crate::target::Target, bin: &Path, args: &[String], asked: Option<&str>, wait: Duration) -> Out {
    let (name, plan, window) = (t.name.as_str(), &t.plan, windowed(t));
    let health = || std::fs::read(plan.data.join("health.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
    // Up: never started twice. Starting or stopping: waited for (aio attach).
    let end = Instant::now() + wait;
    let mut waited = false;
    while aio::paths::running(&plan.data) {
        if let Some(Ok(line)) = verdict(None, true, health().as_ref(), 0, now_ms()) {
            return match waited {
                true => Ok(format!("{name}: another start brought it up: {line}")),
                false => Err(format!(
                    "{name} is already running: `aio status {name}` says how, `aio restart {name}` starts it again"
                )),
            };
        }
        if Instant::now() >= end {
            return Err(format!(
                "{name} is still starting or stopping after {} s: `aio status {name}`",
                wait.as_secs()
            ));
        }
        if !waited {
            eprintln!("aio start: {name} is starting or stopping: waiting for it");
            waited = true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if !bin.exists() {
        return Err(format!("{name}: no binary at {}", bin.display()));
    }
    let (display, mut nested) = place(asked)?;
    let display = display.as_deref();
    aio::paths::private_dir(&plan.logs).map_err(|e| format!("{}: {e}", plan.logs.display()))?;
    let log = plan.logs.join(LOG);
    let since = now_ms();
    // `--cdp` is aio's: the app gets its port, not the flag (the record keeps it for a restart).
    let (app_args, cdp) = crate::cdp::take_flag(args)?;
    let mut cmd = command(bin, &app_args, &log, display)?;
    if let Some(n) = nested.as_mut() {
        // Its cookie and software GPU; no private bus: handed to the app, it would outlive it.
        n.bus = None;
        n.onto(&mut cmd, std::env::var("AIORT_ELECTRON_ARGS").ok().as_deref());
    }
    let cdp = crate::cdp::open(&plan.logs, cdp)?;
    if let Some(p) = cdp {
        cmd.env("AIORT_CDP", p.to_string());
    }
    let mut child = cmd.spawn().map_err(|e| format!("can't start {}: {e}", bin.display()))?;
    use std::io::IsTerminal;
    let user = ["USER", "USERNAME"].iter().find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()));
    let mut rec = json!({
        "by": "aio start", "user": user, "terminal": std::io::stdin().is_terminal(),
        "at": since, "bin": bin, "args": args, "display": asked, "pid": child.id(),
    });
    let write = |rec: &Value| aio::paths::write_atomic(&plan.logs.join(RECORD), rec.to_string().as_bytes(), false);
    write(&rec).map_err(|e| format!("{}: {e}", plan.logs.join(RECORD).display()))?;
    // `--no-wait` (`--wait 0`): spawned is all; its display goes on with it.
    if wait.is_zero() {
        std::mem::forget(nested);
        return Ok(format!(
            "{name}: spawned (pid {}), not waited for: `aio status {name}` says when it is up",
            child.id()
        ));
    }
    let end = Instant::now() + wait;
    loop {
        let ended = child.try_wait().ok().flatten().map(|s| s.to_string());
        let health = health();
        let took = (now_ms() - since) as f64 / 1000.0;
        let ready = !window || std::fs::read_to_string(&log).is_ok_and(|l| l.contains(crate::smoke::READY));
        match verdict(ended, aio::paths::running(&plan.data), health.as_ref(), since, now_ms()) {
            Some(Ok(_)) if !ready && Instant::now() >= end => {
                return Err(format!(
                    "{name}: up, but its window isn't ready after {} s (pid {}): `aio logs {name}`, {} say why",
                    wait.as_secs(),
                    child.id(),
                    log.display()
                ));
            }
            Some(Ok(_)) if !ready => std::thread::sleep(Duration::from_millis(100)),
            Some(Ok(line)) => {
                rec["startedAt"] = health.as_ref().map_or(Value::Null, |h| h["startedAt"].clone());
                _ = write(&rec);
                // The app holds it now; it ends with its last window. Its cookie was
                // read as the window connected, so it goes.
                if let Some(n) = &nested {
                    _ = std::fs::remove_file(&n.auth);
                }
                std::mem::forget(nested);
                let cdp = cdp.map_or(String::new(), |p| format!(" (DevTools protocol on 127.0.0.1:{p})"));
                return Ok(format!("{name}: started in {took:.1} s: {line}{cdp}"));
            }
            Some(Err(why)) => {
                return Err(format!("{name}: {why}; the end of {}:\n{}", log.display(), tail(&log, 20)));
            }
            None if Instant::now() >= end => {
                return Err(format!(
                    "{name}: not up after {} s (pid {}, still running): `aio status {name}`, `aio logs {name}`, \
                     {} say why; `aio stop {name}` ends it",
                    wait.as_secs(),
                    child.id(),
                    log.display()
                ));
            }
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

/// `aio start --all` and `aio restart --all` (aio: a project's components,
/// all at once): `one` for each app of the workspace, in order, a line
/// each; one already up is fine, any other failure fails the whole (the
/// rest still go).
pub fn each(names: &[String], one: &dyn Fn(&str) -> Out) -> Out {
    if names.is_empty() {
        return Ok("no aio apps in this workspace".into());
    }
    let mut failed = false;
    let lines: Vec<String> = names
        .iter()
        .map(|n| match one(n) {
            Ok(l) => l,
            Err(e) if e.contains("is already running") => format!("{n}: already running"),
            Err(e) => {
                failed = true;
                e
            }
        })
        .collect();
    if failed { Err(lines.join("\n")) } else { Ok(lines.join("\n")) }
}

/// The home flags a verb was given, as the app takes them.
pub fn home_args(ask: &aio::paths::Ask) -> Vec<String> {
    [("--home", &ask.home), ("--profile", &ask.profile), ("--instance", &ask.instance)]
        .into_iter()
        .filter_map(|(f, v)| v.as_ref().map(|v| [f.to_string(), v.clone()]))
        .flatten()
        .collect()
}

/// `aio restart`: stop the app if it runs, then start it again with the
/// flags it was started with, merged with `typed`. Notes on stderr.
pub fn restart(
    name: &str,
    t: &crate::target::Target,
    bin: &dyn Fn() -> Result<PathBuf, String>,
    typed: &[String],
    wait: Duration,
) -> Out {
    if crate::dev::session(&t.plan.logs) {
        return via_dev(t, wait);
    }
    let rec = record(&t.plan.logs);
    let recorded: Vec<String> = rec
        .as_ref()
        .and_then(|r| r["args"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|a| a.as_str().map(String::from))
        .collect();
    let (launch, replayed, over) = merge(&recorded, typed);
    if !replayed.is_empty() {
        let over = if over.is_empty() { String::new() } else { format!(" (overridden here: {})", over.join(" ")) };
        eprintln!("aio restart: replaying the start's flags: {}{over}", replayed.join(" "));
    }
    let running = aio::paths::running(&t.plan.data);
    if running && rec.is_none() {
        eprintln!("aio restart: {name} wasn't started by aio start, so its flags are unknown: starting it with these");
    }
    if running {
        eprintln!("{}", crate::stop::one(name, t, false, Duration::from_secs(15))?);
    }
    let asked = rec.as_ref().and_then(|r| r["display"].as_str().map(String::from));
    run(t, &bin()?, &launch, asked.as_deref(), wait)
}

/// How long a restart under `aio dev` waits at least: a whole build.
const DEV_WAIT: Duration = Duration::from_secs(180);

/// `aio restart` of an app `aio dev` runs: the session rebuilds it whole
/// and starts it again, with its flags on its display (`aio start`'s own
/// would be a second app, elsewhere); up once healthy with its window open.
fn via_dev(t: &crate::target::Target, wait: Duration) -> Out {
    let (name, data) = (t.name.as_str(), &t.plan.data);
    let health = || std::fs::read(data.join("health.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok());
    let before = health().map(|h| h["startedAt"].clone());
    crate::dev::ask_restart(&t.plan.logs).map_err(|e| format!("{}: {e}", t.plan.logs.display()))?;
    eprintln!("aio restart: {name} runs under `aio dev`: it rebuilds it and starts it again, with its flags");
    let (since, wait) = (now_ms(), wait.max(DEV_WAIT));
    let end = Instant::now() + wait;
    loop {
        let h = health().filter(|h| Some(&h["startedAt"]) != before.as_ref());
        if let Some(Ok(line)) = verdict(None, aio::paths::running(data), h.as_ref(), since, now_ms()) {
            if windowed(t) {
                let door = crate::cdp::door(&t.id, &t.plan).ok_or(format!("{name} ended as it started"))?;
                let left = end.saturating_duration_since(Instant::now());
                aio::rt()
                    .block_on(crate::cdp::window_within(&crate::cdp::Reach::Door(door), 0, left))
                    .map_err(|e| format!("{name}: up, but its window isn't ready: {e}"))?;
            }
            let took = (now_ms() - since) as f64 / 1000.0;
            return Ok(format!("{name}: restarted by `aio dev` in {took:.1} s: {line}"));
        }
        if Instant::now() >= end {
            return Err(format!(
                "{name}: not up again after {} s: the `aio dev` session's output, or `aio errors {name}`, says why",
                wait.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn a_typed_flag_overrides_the_recorded_one_and_the_rest_is_replayed() {
        // aio am-restart-keeps-flags.test.ts.
        let (l, r, o) = merge(&s(&["--cdp", "--port=8140"]), &s(&["--port=9000"]));
        assert_eq!((l, r, o), (s(&["--cdp", "--port=9000"]), s(&["--cdp"]), s(&["--port"])));
        // `--port` and `--port=N` are one flag; a value word goes with its flag.
        assert_eq!(merge(&s(&["--cdp=9222"]), &s(&["--cdp"])).0, s(&["--cdp"]));
        assert_eq!(
            merge(&s(&["--instance", "a1", "--x"]), &s(&["--instance", "b2"])).0,
            s(&["--x", "--instance", "b2"])
        );
        // Nothing recorded, nothing invented; nothing typed, all replayed.
        assert_eq!(merge(&[], &s(&["--cdp"])), (s(&["--cdp"]), vec![], vec![]));
        assert_eq!(merge(&s(&["--env-file=.env"]), &[]).0, s(&["--env-file=.env"]));
        assert_eq!(merge(&s(&["--env-file=.env"]), &s(&["--cdp"])).0, s(&["--env-file=.env", "--cdp"]));
    }

    #[test]
    fn the_apps_own_words_are_replaced_whole_and_never_mixed_with_flags() {
        // aio: a typed flag after a recorded `-- x` went to the app as a word.
        let (l, r, _) = merge(&s(&["--a", "--", "x.md"]), &s(&["--expose"]));
        assert_eq!((l, r), (s(&["--a", "--expose", "--", "x.md"]), s(&["--a", "--", "x.md"])));
        let (l, _, o) = merge(&s(&["--a", "--", "x.md"]), &s(&["--", "y.md"]));
        assert_eq!((l, o), (s(&["--a", "--", "y.md"]), s(&["--"])));
    }

    #[test]
    fn a_start_is_up_only_when_its_own_health_is_written_under_the_lock() {
        let health = |at: i64, status: &str| json!({ "appId": "notes", "status": status, "updatedAt": at, "pid": 7, "uptimeS": 0, "memory": { "rssBytes": 0, "machinePct": 0, "level": "ok" } });
        assert_eq!(verdict(None, false, None, 100, 100), None, "no lock yet");
        assert_eq!(verdict(None, true, None, 100, 100), None, "locked, no health yet: starting");
        assert_eq!(verdict(None, true, Some(&health(50, "healthy")), 100, 100), None, "an old run's health");
        assert_eq!(verdict(None, false, Some(&health(150, "healthy")), 100, 150), None, "health but no lock");
        assert_eq!(verdict(None, true, Some(&health(150, "stopping")), 100, 150), None);
        let up = verdict(None, true, Some(&health(150, "healthy")), 100, 150).unwrap().unwrap();
        assert!(up.starts_with("notes is healthy: pid 7"), "{up}");
        let degraded = verdict(None, true, Some(&health(150, "degraded")), 100, 150).unwrap().unwrap();
        assert!(degraded.contains("is degraded"), "up, and says how: {degraded}");
        let ended = verdict(Some("exit status: 3".into()), false, None, 100, 150).unwrap().unwrap_err();
        assert!(ended.contains("ended at once (exit status: 3)"));
    }

    #[test]
    fn who_started_it_is_the_record_only_when_it_names_the_running_process() {
        let h = json!({ "pid": 7, "startedAt": "2026-10-28T08:00:00.000Z" });
        assert_eq!(started_by(&json!({ "pid": 7, "startedAt": "2026-10-28T08:00:00.000Z" }), &h), "aio start");
        assert_eq!(started_by(&json!({ "pid": 7 }), &h), "aio start", "a start still waiting");
        assert_eq!(
            started_by(&json!({ "pid": 7, "startedAt": "2026-10-27T08:00:00.000Z" }), &h),
            "other",
            "a reused pid"
        );
        assert_eq!(started_by(&json!({ "pid": 8 }), &h), "other");
        assert_eq!(started_by(&Value::Null, &h), "other");
    }

    /// Marks a home a test hands this test binary as a stand-in app.
    const FAKE: &str = "aio-start-fake";

    /// This test binary as an app, run by the test below (its home the last
    /// arg, a filter no test matches): it takes the app's lock, writes its
    /// health and runs until ended. Run any other way, it does nothing.
    #[test]
    fn fake_app() {
        let Some(home) = std::env::args().next_back().map(PathBuf::from).filter(|h| h.join(FAKE).exists()) else {
            return;
        };
        let _dirs = aio::paths::AppDirs::at(&home, "notes").unwrap();
        let now = now_ms();
        let doc = json!({
            "appId": "notes", "status": "healthy", "pid": std::process::id(), "updatedAt": now,
            "startedAt": aio::log::stamp(now), "uptimeS": 0, "memory": { "rssBytes": 0, "machinePct": 0, "level": "ok" },
        });
        std::fs::write(home.join("data/health.json"), doc.to_string()).unwrap();
        println!("up");
        std::thread::sleep(Duration::from_secs(60));
    }

    #[cfg(unix)]
    #[test]
    fn a_real_start_waits_until_up_records_who_and_a_restart_replays_it() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join(FAKE), "").unwrap();
        let p = plan(t.path());
        let exe = std::env::current_exe().unwrap();
        let args = s(&["start::tests::fake_app", "--nocapture", "--test-threads=1", &t.path().to_string_lossy()]);
        let up = run(&target_of(&p), &exe, &args, Some("current"), WAIT).unwrap();
        assert!(up.starts_with("notes: started in") && up.contains("notes is healthy"), "{up}");
        let health = |p: &aio::paths::Plan| -> Value {
            serde_json::from_slice(&std::fs::read(p.data.join("health.json")).unwrap()).unwrap()
        };
        let (rec, h) = (record(&p.logs).unwrap(), health(&p));
        assert_eq!(started_by(&rec, &h), "aio start");
        assert_eq!((&rec["args"], rec["display"].as_str()), (&json!(args), Some("current")));
        assert!(run(&target_of(&p), &exe, &args, Some("current"), WAIT).unwrap_err().contains("already running"));
        // Restart: stopped, then started again on the recorded flags.
        let target = crate::target::Target { name: "notes".into(), id: "notes".into(), plan: p.clone(), pkg: None };
        let again = restart("notes", &target, &|| Ok(exe.clone()), &[], WAIT).unwrap();
        assert!(again.starts_with("notes: started in"), "{again}");
        let (rec2, h2) = (record(&p.logs).unwrap(), health(&p));
        assert_ne!(rec2["pid"], rec["pid"]);
        assert_eq!((rec2["args"].clone(), started_by(&rec2, &h2)), (json!(args), "aio start"));
        assert!(std::fs::read_to_string(p.logs.join(LOG)).unwrap().contains("up"));
        crate::stop::one("notes", &target, true, Duration::ZERO).unwrap();
        // A sibling test's fresh fork holds a copy of the lock until it execs.
        let end = std::time::Instant::now() + Duration::from_secs(10);
        while aio::paths::running(&p.data) {
            assert!(std::time::Instant::now() < end, "still running after the stop");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// An app `aio dev` runs is restarted by that session (its flags, its
    /// display), never started a second time elsewhere by `aio start`.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_restart_of_an_app_under_aio_dev_is_asked_of_the_session() {
        let t = tempfile::tempdir().unwrap();
        let p = plan(t.path());
        let _held = aio::paths::AppDirs::at(t.path(), "notes").unwrap();
        let doc = |at: &str| {
            json!({ "appId": "notes", "status": "healthy", "pid": 1, "updatedAt": now_ms(),
            "startedAt": at, "uptimeS": 0, "memory": { "rssBytes": 0, "machinePct": 0 } })
        };
        std::fs::write(p.data.join("health.json"), doc("old").to_string()).unwrap();
        std::fs::write(p.logs.join("dev.pid"), std::process::id().to_string()).unwrap();
        assert!(crate::dev::session(&p.logs));
        let (logs, data) = (p.logs.clone(), p.data.clone());
        let session = std::thread::spawn(move || {
            while std::fs::remove_file(logs.join("dev-restart")).is_err() {
                std::thread::sleep(Duration::from_millis(20));
            }
            std::fs::write(data.join("health.json"), doc("new").to_string()).unwrap();
        });
        let never = || -> Result<PathBuf, String> { panic!("nothing built or started by aio start") };
        let said = restart("notes", &target_of(&p), &never, &[], WAIT).unwrap();
        assert!(said.starts_with("notes: restarted by `aio dev`") && said.contains("healthy"), "{said}");
        session.join().unwrap();
        std::fs::write(p.logs.join("dev.pid"), "4194999").unwrap();
        assert!(!crate::dev::session(&p.logs), "a session that ended");
    }

    #[test]
    fn all_starts_each_app_and_one_already_up_is_fine() {
        let names = s(&["relay", "agent", "control"]);
        let one = |n: &str| match n {
            "relay" => Ok("relay: started in 0.4 s".to_string()),
            "agent" => Err("agent is already running: `aio status agent` says how".into()),
            _ => Ok(format!("{n}: started")),
        };
        assert_eq!(each(&names, &one).unwrap(), "relay: started in 0.4 s\nagent: already running\ncontrol: started");
        let broken = |n: &str| {
            if n == "agent" { Err("agent: the build failed".to_string()) } else { Ok(format!("{n}: started")) }
        };
        let e = each(&names, &broken).unwrap_err();
        assert_eq!(e, "relay: started\nagent: the build failed\ncontrol: started", "the rest still go");
        assert_eq!(each(&[], &one).unwrap(), "no aio apps in this workspace");
    }

    #[test]
    fn home_flags_reach_the_app_as_it_takes_them() {
        let ask = aio::paths::Ask { profile: Some("work".into()), instance: Some("a1".into()), ..Default::default() };
        assert_eq!(home_args(&ask), s(&["--profile", "work", "--instance", "a1"]));
        assert!(home_args(&aio::paths::Ask::default()).is_empty());
    }

    #[cfg(unix)]
    fn target_of(p: &aio::paths::Plan) -> crate::target::Target {
        crate::target::Target { name: "notes".into(), id: "notes".into(), plan: p.clone(), pkg: None }
    }

    #[cfg(unix)]
    fn plan(home: &Path) -> aio::paths::Plan {
        aio::paths::Plan {
            data: home.join("data"),
            logs: home.join("logs"),
            cache: home.join("cache"),
            store: home.join("data/state.db"),
            profile: None,
            port_of: None,
            from: "--home",
            asked: true,
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_start_that_ends_at_once_says_why_and_one_that_never_comes_up_is_left_running_and_said() {
        let t = tempfile::tempdir().unwrap();
        let p = plan(t.path());
        let sh = Path::new("/bin/sh");
        let e = run(&target_of(&p), sh, &s(&["-c", "echo no config here; exit 3"]), Some("current"), WAIT).unwrap_err();
        assert!(e.contains("ended at once (exit status: 3)") && e.ends_with("no config here"), "{e}");
        let rec = record(&p.logs).unwrap();
        assert_eq!((rec["by"].as_str(), rec["args"][0].as_str()), (Some("aio start"), Some("-c")));
        // Detached: its own process group, nothing of this terminal.
        let e = run(&target_of(&p), sh, &s(&["-c", "exec sleep 5"]), Some("current"), Duration::from_millis(300))
            .unwrap_err();
        assert!(e.contains("not up after 0 s") && e.contains("aio stop notes"), "{e}");
        let pid = record(&p.logs).unwrap()["pid"].as_u64().unwrap() as u32;
        let pgid = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        assert_eq!(pgid.split_whitespace().nth(4), Some(pid.to_string().as_str()), "its own group");
        aio::lifecycle::terminate(pid, true).unwrap();
        assert!(
            run(&target_of(&p), Path::new("/no/such/bin"), &[], Some("current"), WAIT)
                .unwrap_err()
                .contains("no binary")
        );
        // One stopping is waited for, then started.
        let held = aio::paths::AppDirs::at(t.path(), "notes").unwrap();
        std::fs::write(p.data.join("health.json"), r#"{"status":"stopping","updatedAt":1}"#).unwrap();
        let short = Duration::from_millis(300);
        assert!(
            run(&target_of(&p), sh, &[], Some("current"), short).unwrap_err().contains("still starting or stopping")
        );
        let gone = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(held);
        });
        let e = run(&target_of(&p), sh, &s(&["-c", "exit 4"]), Some("current"), WAIT).unwrap_err();
        assert!(e.contains("exit status: 4"), "started once the other was gone: {e}");
        gone.join().unwrap();
        // One that is up is never started twice.
        let _held = aio::paths::AppDirs::at(t.path(), "notes").unwrap();
        std::fs::write(p.data.join("health.json"), r#"{"appId":"notes","status":"healthy","updatedAt":1}"#).unwrap();
        assert!(run(&target_of(&p), sh, &[], Some("current"), WAIT).unwrap_err().contains("already running"));
    }
}

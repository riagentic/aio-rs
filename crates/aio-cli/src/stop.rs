//! `ax stop` and `ax kill`: end a running app and wait until it is gone
//! (its lock free: the OS drops it with the process, so "gone" is never a
//! guess). `stop` asks over the app's hand door first (any OS), then, if it
//! is still there after `--wait`, SIGTERM, then SIGKILL. `kill` ends it at
//! once. The pid comes from the app's own `health.json`, never from
//! matching process names.
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

type Out = Result<String, String>;

/// How long a SIGTERM or SIGKILL is given to end the process.
const SIGNAL_WAIT: Duration = Duration::from_secs(5);

/// How an app ended.
#[derive(Debug, PartialEq)]
pub enum How {
    Asked,
    Termed,
    Killed,
}

/// Wait until `alive` says no, at most `budget`; whether it did.
fn gone(alive: &dyn Fn() -> bool, budget: Duration) -> bool {
    let end = Instant::now() + budget;
    loop {
        if !alive() {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// End a running app: `asked` is how asking it went (`None`: don't ask,
/// end it now); then `end(pid, hard)` signals it. `wait`: its time to go
/// when asked, and after a signal.
pub fn end(
    alive: &dyn Fn() -> bool,
    asked: Option<io::Result<()>>,
    pid: Option<u32>,
    end: &dyn Fn(u32, bool) -> io::Result<()>,
    (wait, signalled): (Duration, Duration),
) -> Result<How, String> {
    let ask = asked.is_some();
    let why = match asked {
        Some(Ok(())) if gone(alive, wait) => return Ok(How::Asked),
        Some(Ok(())) => format!("it was asked to stop and is still running after {} s", wait.as_secs()),
        Some(Err(e)) => format!("it can't be asked to stop ({e})"),
        None => "it is to end at once".into(),
    };
    let Some(pid) = pid else { return Err(format!("{why}; no pid in its health.json to end it with")) };
    if ask && end(pid, false).is_ok() && gone(alive, signalled) {
        return Ok(How::Termed);
    }
    end(pid, true).map_err(|e| format!("{why}; pid {pid} can't be ended: {e}"))?;
    if gone(alive, signalled) { Ok(How::Killed) } else { Err(format!("pid {pid} is still running after SIGKILL")) }
}

/// The pid in `data/health.json`, written by the app itself.
fn pid(data: &Path) -> Option<u32> {
    let doc: aio::serde_json::Value =
        aio::serde_json::from_slice(&std::fs::read(data.join("health.json")).ok()?).ok()?;
    doc["pid"].as_u64().and_then(|p| u32::try_from(p).ok())
}

/// How a stop went, for its line or its JSON.
#[derive(Debug, PartialEq)]
pub struct Stopped {
    pub name: String,
    /// `None`: asked, not waited for (`--no-wait`).
    pub how: Option<How>,
    pub hard: bool,
    pub took: f64,
    pub wait: Duration,
    /// A `--fresh` instance's data thrown away after it.
    pub discarded: bool,
}

impl Stopped {
    pub fn said(&self) -> String {
        let (name, took) = (&self.name, self.took);
        let line = match self.how {
            None => format!("{name}: asked to stop (not waited for: `ax status {name}`)"),
            Some(How::Asked) => format!("{name}: stopped ({took:.1} s)"),
            Some(How::Termed) => {
                format!(
                    "{name}: stopped by SIGTERM: asking didn't do it within {} s ({took:.1} s)",
                    self.wait.as_secs()
                )
            }
            Some(How::Killed) if self.hard => format!("{name}: killed ({took:.1} s)"),
            Some(How::Killed) => format!("{name}: killed: neither asking nor SIGTERM stopped it ({took:.1} s)"),
        };
        if self.discarded { format!("{line}; its --fresh data thrown away") } else { line }
    }

    pub fn json(&self) -> aio::serde_json::Value {
        let how = match self.how {
            None => "asked",
            Some(How::Asked) => "stopped",
            Some(How::Termed) => "sigterm",
            Some(How::Killed) => "sigkill",
        };
        aio::serde_json::json!({
            "app": self.name, "how": how, "waited": self.how.is_some(), "seconds": self.took, "discarded": self.discarded,
        })
    }
}

/// `ax stop <app>` (`hard`: `ax kill <app>`), at its home `t`, its line.
pub fn one(name: &str, t: &crate::target::Target, hard: bool, wait: Duration) -> Out {
    stopped(name, t, hard, Some(wait)).map(|s| s.said())
}

/// [`one`], as it went; `wait` `None`: asked, not waited for.
pub fn stopped(name: &str, t: &crate::target::Target, hard: bool, wait: Option<Duration>) -> Result<Stopped, String> {
    let data = &t.plan.data;
    // An `ax dev` session would start it again on the next save: it ends first,
    // and stops the app it runs the graceful way.
    let started = Instant::now();
    let session = crate::dev::end_session(&t.plan.logs);
    if !aio::paths::running(data) && session {
        let took = started.elapsed().as_secs_f64();
        let wait = wait.unwrap_or_default();
        return Ok(Stopped { name: name.into(), how: Some(How::Asked), hard, took, wait, discarded: false });
    }
    if !aio::paths::running(data) {
        let verb = if hard { "kill" } else { "stop" };
        return Err(format!("{name} isn't running: nothing to {verb} (`ax {verb} {name} || true` when that's fine)"));
    }
    let pid = pid(data);
    let asked = (!hard).then(|| aio_shell::instance::quit_at(&t.id, data));
    let done = |how, discarded| Stopped {
        name: name.into(),
        how,
        hard,
        took: started.elapsed().as_secs_f64(),
        wait: wait.unwrap_or_default(),
        discarded,
    };
    let Some(wait) = wait else {
        return match asked {
            Some(Ok(())) => Ok(done(None, false)),
            Some(Err(e)) => {
                Err(format!("{name}: it can't be asked to stop ({e}): ax stop {name} (waits, then signals)"))
            }
            None => Err(format!("{name}: ax kill always waits")),
        };
    };
    let alive = || aio::paths::running(data);
    let how =
        end(&alive, asked, pid, &aio::lifecycle::terminate, (wait, SIGNAL_WAIT)).map_err(|e| format!("{name}: {e}"))?;
    let discarded = match crate::dev::discard_fresh(&t.plan) {
        Some(r) => r.map(|_| true).map_err(|e| format!("{name}: stopped; its --fresh data stays: {e}"))?,
        None => false,
    };
    Ok(done(Some(how), discarded))
}

/// The apps of this workspace: each binary beside an `aio.toml`.
pub fn workspace_apps() -> Result<Vec<String>, String> {
    let meta = crate::dev::metadata().map_err(|_| "--all stops the apps of a workspace: run it in one")?;
    let mut names: Vec<String> = meta["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|p| p["targets"].as_array().into_iter().flatten())
        .filter(|t| t["kind"].as_array().is_some_and(|k| k.iter().any(|k| k == "bin")))
        .filter_map(|t| t["name"].as_str().map(String::from))
        .filter(|n| crate::dev::locate(&meta, n).is_ok_and(|p| crate::build::app_of(&p).is_ok()))
        .collect();
    names.sort();
    names.dedup();
    Ok(names)
}

/// `ax stop --all`: every running app of the workspace; the rest are said.
pub fn all(wait: Option<Duration>, json: bool) -> Out {
    let names = workspace_apps()?;
    if names.is_empty() && !json {
        return Ok("no aio-rs apps in this workspace".into());
    }
    let mut failed = false;
    let (lines, rows): (Vec<String>, Vec<aio::serde_json::Value>) = names
        .iter()
        .map(|n| match crate::target::of(n, &aio::paths::Ask::default()).and_then(|t| stopped(n, &t, false, wait)) {
            Ok(s) => (s.said(), s.json()),
            Err(e) if e.contains("isn't running") => {
                (format!("{n}: not running"), aio::serde_json::json!({ "app": n, "how": "not running" }))
            }
            Err(e) => {
                failed = true;
                (e.clone(), aio::serde_json::json!({ "app": n, "error": e }))
            }
        })
        .unzip();
    let text = if json { crate::failed_or(rows) } else { lines.join("\n") };
    if failed { Err(text) } else { Ok(text) }
}

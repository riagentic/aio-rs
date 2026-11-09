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

/// A running app no verb reaches (aio's orphans, `am kill --stale`): its
/// process holds the instance lock of a data folder deleted or made again
/// while it ran, so `ax status` says stopped, and a new start beside it
/// would run two apps on one data folder.
#[derive(Debug, PartialEq)]
pub struct Stale {
    pub pid: u32,
    /// The lock file it holds, as its path was.
    pub lock: String,
}

/// The instance lock `target` names (a `/proc/<pid>/fd` link), if it no
/// longer is the file at that path: `(deleted)`, or `replaced` (its path now
/// another file, `now` its (device, inode) against the fd's `held`) (pure).
#[cfg(any(unix, test))]
fn stale_lock(target: &str, held: (u64, u64), now: Option<(u64, u64)>) -> Option<String> {
    let path = target.strip_suffix(" (deleted)");
    let p = path.unwrap_or(target);
    let lock = p.ends_with("/data/instance.lock") || p.ends_with("/.aio-instance.lock");
    (lock && (path.is_some() || now != Some(held))).then(|| p.to_string())
}

/// The stale locks in `lsof -nP +L1 -Fpn`'s answer (macOS: each process's
/// open files that have no name left), pure.
#[cfg(any(target_os = "macos", test))]
fn lsof_stale(text: &str) -> Vec<Stale> {
    let mut pid = None;
    let mut out = vec![];
    for l in text.lines() {
        match l.split_at_checked(1) {
            Some(("p", p)) => pid = p.parse().ok(),
            Some(("n", n)) => {
                if let (Some(pid), Some(lock)) = (pid, stale_lock(&format!("{n} (deleted)"), (0, 0), None)) {
                    out.push(Stale { pid, lock });
                }
            }
            _ => {}
        }
    }
    out
}

/// This user's processes holding a stale instance lock (Linux: `/proc`;
/// macOS: `lsof`, deleted folders only; Windows: not looked for).
pub fn stale() -> Vec<Stale> {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc::getuid() }.to_string();
        let me = std::process::id();
        let o = std::process::Command::new("lsof").args(["-nP", "+L1", "-Fpn", "-u", &uid]).output();
        let text = o.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
        lsof_stale(&text).into_iter().filter(|s| s.pid != me).collect()
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        let me = std::process::id();
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc::getuid() };
        let mut out = vec![];
        for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|p| p.parse::<u32>().ok()) else { continue };
            if pid == me || e.metadata().map_or(true, |m| m.uid() != uid) {
                continue;
            }
            for fd in std::fs::read_dir(e.path().join("fd")).into_iter().flatten().flatten() {
                let (Ok(link), Ok(held)) = (std::fs::read_link(fd.path()), std::fs::metadata(fd.path())) else {
                    continue;
                };
                let link = link.to_string_lossy();
                let now = std::fs::metadata(link.trim_end_matches(" (deleted)")).ok().map(|m| (m.dev(), m.ino()));
                if let Some(lock) = stale_lock(&link, (held.dev(), held.ino()), now) {
                    out.push(Stale { pid, lock });
                }
            }
        }
        out
    }
    #[cfg(windows)]
    vec![]
}

/// `ax kill --stale`: end each of `found` (SIGTERM, then SIGKILL), saying
/// which and why; none: said, exit 0.
pub fn kill_stale(found: &[Stale], json: bool) -> Out {
    let ended: Vec<serde_json::Value> = found
        .iter()
        .map(|s| {
            // A zombie has ended; only its parent's wait is left.
            let zombie = || {
                std::fs::read_to_string(format!("/proc/{}/stat", s.pid))
                    .is_ok_and(|t| t.rsplit_once(") ").is_some_and(|(_, r)| r.starts_with('Z')))
            };
            let alive = || aio::lifecycle::alive(s.pid) && !zombie();
            // SIGTERM first (it saves), as aio's: nothing can ask it.
            let asked = Some(Err(io::Error::other("no verb reaches it")));
            let how = end(&alive, asked, Some(s.pid), &aio::lifecycle::terminate, (Duration::ZERO, SIGNAL_WAIT));
            serde_json::json!({ "pid": s.pid, "lock": s.lock, "error": how.err() })
        })
        .collect();
    if json {
        let ok = ended.iter().all(|e| e["error"].is_null());
        let v = serde_json::json!({ "killed": ended }).to_string();
        return if ok { Ok(v) } else { Err(v) };
    }
    if ended.is_empty() {
        return Ok("no stale aio-rs apps: each running one holds the lock of a data folder that is still there".into());
    }
    let lines: Vec<String> = ended
        .iter()
        .map(|e| match e["error"].as_str() {
            None => format!(
                "killed stale pid {}: it ran on {}, deleted or made again since, so no verb reached it",
                e["pid"],
                e["lock"].as_str().unwrap_or_default()
            ),
            Some(why) => format!("pid {} (stale, {}): {why}", e["pid"], e["lock"].as_str().unwrap_or_default()),
        })
        .collect();
    let text = lines.join("\n");
    if ended.iter().all(|e| e["error"].is_null()) { Ok(text) } else { Err(text) }
}

#[cfg(test)]
mod stale_tests {
    use super::*;

    #[test]
    fn a_lock_whose_file_is_gone_or_another_is_stale() {
        let held = (1, 7);
        assert_eq!(
            stale_lock("/h/apps/notes/data/instance.lock (deleted)", held, None).as_deref(),
            Some("/h/apps/notes/data/instance.lock")
        );
        assert_eq!(
            stale_lock("/h/.notes/.aio-instance.lock", held, Some((1, 8))).as_deref(),
            Some("/h/.notes/.aio-instance.lock"),
            "made again"
        );
        assert_eq!(stale_lock("/h/apps/notes/data/instance.lock", held, Some(held)), None, "its own: running");
        assert_eq!(stale_lock("/h/notes/state.db (deleted)", held, None), None, "not a lock");
    }

    #[test]
    fn lsof_names_each_process_and_its_nameless_files() {
        let text = "p41\nn/Users/u/Library/x/apps/notes/data/instance.lock\nn/tmp/other\np42\nn/Users/u/.notes/.aio-instance.lock\n";
        let found = lsof_stale(text);
        assert_eq!(found.iter().map(|s| s.pid).collect::<Vec<_>>(), [41, 42]);
        assert_eq!(found[0].lock, "/Users/u/Library/x/apps/notes/data/instance.lock");
    }

    /// A process holding a lock of a deleted data folder is found, and ended.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_process_on_a_deleted_data_folder_is_found_and_ended() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("apps/notes/data/instance.lock");
        std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
        std::fs::write(&lock, "").unwrap();
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("exec 3<>'{}'; exec sleep 30", lock.display()))
            .spawn()
            .unwrap();
        let mine = |all: Vec<Stale>| all.into_iter().filter(|s| s.pid == child.id()).collect::<Vec<_>>();
        let found = (0..100).find_map(|_| {
            std::thread::sleep(Duration::from_millis(20));
            std::fs::remove_file(&lock).ok();
            Some(mine(stale())).filter(|f| !f.is_empty())
        });
        let found = found.expect("the child is stale once its lock file is deleted");
        assert_eq!(found[0].lock, lock.display().to_string());
        let said = kill_stale(&found, false).unwrap();
        assert!(said.starts_with(&format!("killed stale pid {}", child.id())), "{said}");
        assert!(child.wait().is_ok_and(|s| !s.success()), "ended by a signal");
        assert!(kill_stale(&[], false).unwrap().starts_with("no stale"));
    }
}

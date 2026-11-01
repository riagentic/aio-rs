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

/// `ax stop <app>` (`hard`: `ax kill <app>`), at its home `t`.
pub fn one(name: &str, t: &crate::target::Target, hard: bool, wait: Duration) -> Out {
    let data = &t.plan.data;
    if !aio::paths::running(data) {
        let verb = if hard { "kill" } else { "stop" };
        return Err(format!("{name} isn't running: nothing to {verb} (`ax {verb} {name} || true` when that's fine)"));
    }
    let pid = pid(data);
    let started = Instant::now();
    let asked = (!hard).then(|| aio_shell::instance::quit_at(&t.id, data));
    let alive = || aio::paths::running(data);
    let how =
        end(&alive, asked, pid, &aio::lifecycle::terminate, (wait, SIGNAL_WAIT)).map_err(|e| format!("{name}: {e}"))?;
    let took = started.elapsed().as_secs_f64();
    Ok(match how {
        How::Asked => format!("{name}: stopped ({took:.1} s)"),
        How::Termed => {
            format!("{name}: stopped by SIGTERM: asking didn't do it within {} s ({took:.1} s)", wait.as_secs())
        }
        How::Killed if hard => format!("{name}: killed ({took:.1} s)"),
        How::Killed => format!("{name}: killed: neither asking nor SIGTERM stopped it ({took:.1} s)"),
    })
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
pub fn all(wait: Duration) -> Out {
    let names = workspace_apps()?;
    if names.is_empty() {
        return Ok("no aio-rs apps in this workspace".into());
    }
    let mut failed = false;
    let lines: Vec<String> = names
        .iter()
        .map(|n| match crate::target::of(n, &aio::paths::Ask::default()).and_then(|t| one(n, &t, false, wait)) {
            Ok(l) => l,
            Err(e) if e.contains("isn't running") => format!("{n}: not running"),
            Err(e) => {
                failed = true;
                e
            }
        })
        .collect();
    if failed { Err(lines.join("\n")) } else { Ok(lines.join("\n")) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    const NO_WAIT: (Duration, Duration) = (Duration::ZERO, Duration::ZERO);

    #[test]
    fn asked_terminated_then_killed_each_only_when_the_last_did_not_do() {
        let short = (Duration::from_millis(100), Duration::from_millis(100));
        for (asked, ends_on, want) in [
            (true, "quit", Ok(How::Asked)),
            (true, "term", Ok(How::Termed)),
            (true, "kill", Ok(How::Killed)),
            (false, "kill", Ok(How::Killed)),
        ] {
            let (up, got) = (Cell::new(true), RefCell::new(vec![]));
            let signal = |_pid: u32, hard: bool| {
                let s = if hard { "kill" } else { "term" };
                got.borrow_mut().push(s);
                if s == ends_on {
                    up.set(false);
                }
                Ok(())
            };
            let ask = asked.then(|| {
                got.borrow_mut().push("quit");
                if ends_on == "quit" {
                    up.set(false);
                }
                Ok(())
            });
            assert_eq!(end(&|| up.get(), ask, Some(7), &signal, short), want, "{ends_on}");
            let order = ["quit", "term", "kill"];
            let upto = order.iter().position(|s| *s == ends_on).unwrap();
            let sent: Vec<&str> = if asked { order[..=upto].to_vec() } else { vec!["kill"] };
            assert_eq!(*got.borrow(), sent, "nothing harder than needed");
        }
    }

    #[test]
    fn an_app_that_cant_be_asked_is_signalled_and_one_without_a_pid_is_said() {
        let up = Cell::new(true);
        let term = |_: u32, hard: bool| {
            up.set(hard);
            Ok(())
        };
        let no_door = Some(Err(io::Error::other("no hand door")));
        assert_eq!(end(&|| up.get(), no_door, Some(7), &term, NO_WAIT), Ok(How::Termed));
        let e = end(&|| true, Some(Ok(())), None, &term, NO_WAIT).unwrap_err();
        assert!(e.contains("still running after 0 s") && e.contains("no pid"), "{e}");
        let refuse = |_: u32, _: bool| Err(io::Error::from(io::ErrorKind::PermissionDenied));
        assert!(end(&|| true, None, Some(7), &refuse, NO_WAIT).unwrap_err().contains("can't be ended"));
    }

    #[test]
    fn stopping_what_isnt_running_says_there_was_nothing() {
        let t = crate::target::of("no-such-app-0a1b", &aio::paths::Ask::default()).unwrap();
        let e = one("no-such-app-0a1b", &t, false, Duration::ZERO).unwrap_err();
        assert!(e.contains("nothing to stop") && e.contains("|| true"), "{e}");
        assert!(one("no-such-app-0a1b", &t, true, Duration::ZERO).unwrap_err().contains("nothing to kill"));
    }

    #[cfg(unix)]
    #[test]
    fn a_real_process_ends_on_sigterm_and_sigkill() {
        for hard in [false, true] {
            let mut c = std::process::Command::new("sleep").arg("30").spawn().unwrap();
            aio::lifecycle::terminate(c.id(), hard).unwrap();
            let st = c.wait().unwrap();
            use std::os::unix::process::ExitStatusExt;
            assert_eq!(st.signal(), Some(if hard { 9 } else { 15 }));
        }
    }
}

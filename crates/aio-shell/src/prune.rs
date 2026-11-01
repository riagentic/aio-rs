//! What of the shared runtime store may go: aio's rules on aio's store
//! (`shared-prune.ts`, `shared-runtime.ts`), so `am prune`, `ax prune` and
//! both kinds of app agree. Each start records the runtime it starts from in
//! `apps/<key>-<version>.json`, aio's record (`recordSharedUse`): the
//! installed binary, the version, the slug, a macOS window, when. An app
//! keeps its newest two (the one it runs and its rollback) and one an update
//! fetched before its swap (`pending`). What no installed app names (its
//! binary is gone) may go: a runtime, a macOS window bundle, the record
//! itself. One an app names stays however old, so an app opened once a year
//! still opens offline. `ax prune` shows the plan and removes it with
//! `--yes`; an app's start, once a day, removes the runtimes it offers that
//! nothing started from in [`AUTO_AGE`] either (users have no `ax`).
use aio::sha256::Sha256;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How long a runtime or window a start just used is kept with no record
/// naming it: the record may be on its way.
pub const GRACE: Duration = Duration::from_secs(10 * 60);
/// How long nothing may have started from a runtime before an app's own
/// start removes it: an app the user moved names nothing until it starts.
pub const AUTO_AGE: Duration = Duration::from_secs(30 * 86_400);
use crate::runtime::USED;
/// In the store: when a start last ran the auto-prune, an ISO time.
const AUTO: &str = ".aio-auto-prune";
const DAY: Duration = Duration::from_secs(86_400);
/// How many runtimes an app's starts keep named (aio's `USES_PER_APP`).
pub const USES_PER_APP: usize = 2;

/// `t` as aio writes a time (`toISOString`): `2026-11-01T09:30:00.000Z`.
pub fn iso(t: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// A time aio wrote ([`iso`]), or `None`.
pub fn parse_iso(s: &str) -> Option<SystemTime> {
    chrono::DateTime::parse_from_rfc3339(s.trim()).ok().map(SystemTime::from)
}

/// What an installed app records about the runtime it starts from (aio's
/// `SharedUse`).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Use {
    /// The installed program (an AppImage's file, not its mount): the
    /// record is stale once it is gone.
    pub exe: String,
    pub version: String,
    pub slug: String,
    /// macOS: the app's window bundle folder in the store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    /// Written by an update before its swap: not started from yet.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pending: bool,
}

/// A record as aio writes it: the use, then `at`.
#[derive(serde::Serialize)]
struct Rec<'a> {
    #[serde(flatten)]
    used: &'a Use,
    at: String,
}

/// The record file's prefix for the program `exe` (aio's `sharedUseKey`):
/// its SHA-256's first 16 hex.
pub fn key(exe: &str) -> String {
    aio::sha256::hex(&Sha256::digest(exe.as_bytes()))[..16].to_string()
}

/// A record's text (pure): aio's `JSON.stringify(rec) + "\n"`.
pub fn record_text(u: &Use, at: SystemTime) -> String {
    aio::serde_json::to_string(&Rec { used: u, at: iso(at) }).unwrap_or_default() + "\n"
}

/// Note that `u.exe` starts from `u.version`, now ([`record_at`]).
pub fn record(root: &Path, u: &Use) -> io::Result<()> {
    record_at(root, u, SystemTime::now())
}

/// aio's `recordSharedUse` at `at`: one record per (program, version), an
/// update's never demoting one started from; the program's started records
/// past the newest [`USES_PER_APP`], and its pending ones past the newest,
/// go. One that can't be read stays (prune shows it).
pub fn record_at(root: &Path, u: &Use, at: SystemTime) -> io::Result<()> {
    let dir = root.join("apps");
    let key = key(&u.exe);
    let v: String = u
        .version
        .trim_start_matches('v')
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '_' })
        .collect();
    let file = dir.join(format!("{key}-{v}.json"));
    std::fs::create_dir_all(&dir)?;
    let read = |p: &Path| {
        std::fs::read_to_string(p).ok().and_then(|t| aio::serde_json::from_str::<aio::serde_json::Value>(&t).ok())
    };
    if u.pending && read(&file).is_some_and(|was| !was["pending"].as_bool().unwrap_or(false)) {
        return Ok(());
    }
    let tmp = dir.join(format!("{key}-{v}.json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, record_text(u, at))?;
    aio::paths::rename_over(&tmp, &file)?;
    let mut mine: Vec<(PathBuf, String, bool)> = std::fs::read_dir(&dir)?
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().is_some_and(|n| {
                n.to_string_lossy().starts_with(&format!("{key}-")) && n.to_string_lossy().ends_with(".json")
            })
        })
        .filter_map(|p| {
            let got = read(&p)?;
            let at = match &got["at"] {
                aio::serde_json::Value::Null => String::new(),
                aio::serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            let pending = got["pending"].as_bool().unwrap_or(false);
            Some((p, at, pending))
        })
        .collect();
    mine.sort_by(|a, b| b.1.cmp(&a.1));
    let started = mine.iter().filter(|m| !m.2).skip(USES_PER_APP);
    let pending = mine.iter().filter(|m| m.2).skip(1);
    for (p, _, _) in started.chain(pending) {
        match std::fs::remove_file(p) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

/// Every record in the store: its file, the use (`None`: unreadable), and
/// whether its program is still there (aio's `readSharedUses`).
pub fn uses(root: &Path) -> io::Result<Vec<(PathBuf, Option<Use>, bool)>> {
    let entries = match std::fs::read_dir(root.join("apps")) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        r => r?,
    };
    let mut out = vec![];
    for e in entries.flatten() {
        if !e.file_type().is_ok_and(|t| t.is_file()) || !e.file_name().to_string_lossy().ends_with(".json") {
            continue;
        }
        let u = std::fs::read_to_string(e.path()).ok().and_then(|t| aio::serde_json::from_str::<Use>(&t).ok());
        let live = u.as_ref().is_none_or(|u| Path::new(&u.exe).exists());
        out.push((e.path(), u, live));
    }
    Ok(out)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Runtime,
    Window,
    Record,
}

/// One thing in the store, and whether it stays, with the reason.
#[derive(Clone, Debug)]
pub struct Decision {
    pub kind: Kind,
    /// As it is in the store: `44.9.3-linux-x64`, `windows/<id>`, `apps/<file>`.
    pub name: String,
    pub path: PathBuf,
    pub bytes: u64,
    pub keep: bool,
    pub why: String,
    /// When an app last started from it (runtimes).
    pub used: Option<SystemTime>,
}

fn modified(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// When an app last started from the runtime `dir` (aio's
/// `runtimeLastUsed`): its stamp, else the folder's own mtime.
pub fn last_used(dir: &Path) -> Option<SystemTime> {
    std::fs::read_to_string(dir.join(USED)).ok().and_then(|t| parse_iso(&t)).or_else(|| modified(dir))
}

/// Bytes under `p`, following no links; what vanishes meanwhile is 0.
pub fn bytes(p: &Path) -> u64 {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => std::fs::read_dir(p).map_or(0, |d| d.flatten().map(|e| bytes(&e.path())).sum()),
        Ok(m) => m.len(),
        Err(_) => 0,
    }
}

/// `t` as "used today", "last used 3 days ago".
fn ago(t: Option<SystemTime>, now: SystemTime) -> String {
    match t.map(|t| now.duration_since(t).unwrap_or_default().as_secs() / 86_400) {
        None => "never started from".into(),
        Some(0) => "used today".into(),
        Some(1) => "last used 1 day ago".into(),
        Some(d) => format!("last used {d} days ago"),
    }
}

/// A use's runtime, as the store names it.
fn entry(u: &Use) -> String {
    format!("{}-{}", u.version.trim_start_matches('v'), u.slug)
}

/// What of the store at `root` may go, as of `now` (aio's
/// `planSharedPrune`). An absent store is an empty plan. One record that
/// can't be read keeps every runtime and window: what it protects is unknown.
pub fn plan(root: &Path, now: SystemTime) -> io::Result<Vec<Decision>> {
    let entries = match std::fs::read_dir(root) {
        Ok(d) => d,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e),
    };
    let fresh = |t: Option<SystemTime>| t.is_some_and(|t| now.duration_since(t).unwrap_or_default() < GRACE);
    let uses = uses(root)?;
    let name = |p: &Path| p.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let unreadable: Vec<String> =
        uses.iter().filter(|u| u.1.is_none()).map(|u| format!("apps/{}", name(&u.0))).collect();
    let blanket = (!unreadable.is_empty()).then(|| {
        format!(
            "app record(s) {} cannot be read, so what they protect is unknown: fix or delete them",
            unreadable.join(", ")
        )
    });
    let live = || uses.iter().filter(|u| u.2).filter_map(|u| u.1.as_ref());
    let mut out = vec![];
    for e in entries.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        // Stages (a download in flight), stamps, locks, stray files: never touched.
        if n.starts_with('.') || n.contains(".incoming.") || n == "apps" || n == "windows" || !e.path().is_dir() {
            continue;
        }
        let used = last_used(&e.path());
        let (keep, why) = match (live().find(|u| entry(u) == n), blanket.as_ref()) {
            (Some(by), _) => (true, format!("an installed app starts from it ({})", by.exe)),
            _ if fresh(used) => (true, format!("an app started from it in the last {} minutes", GRACE.as_secs() / 60)),
            (None, Some(b)) => (true, b.clone()),
            (None, None) => (false, format!("no installed app names it; {}", ago(used, now))),
        };
        let bytes = bytes(&e.path());
        out.push(Decision { kind: Kind::Runtime, name: n, path: e.path(), bytes, keep, why, used });
    }
    for e in std::fs::read_dir(root.join("windows")).into_iter().flatten().flatten() {
        if !e.path().is_dir() {
            continue;
        }
        let path = root.join("windows").join(e.file_name());
        // A start making this window now has a fresh stage in it.
        let making = std::fs::read_dir(&path)
            .into_iter()
            .flatten()
            .flatten()
            .any(|x| x.file_name().to_string_lossy().starts_with(".stage-") && fresh(modified(&x.path())));
        let by = live().find(|u| u.window.as_deref() == Some(&*path.to_string_lossy()));
        let (keep, why) = match (by, blanket.as_ref()) {
            (Some(by), _) => (true, format!("the window of {}", by.exe)),
            _ if making => (true, "an app is making this window right now".into()),
            (None, Some(b)) => (true, b.clone()),
            (None, None) => {
                (false, "no installed app's record names it (an app makes it again at its next start)".into())
            }
        };
        let bytes = bytes(&path);
        let name = format!("windows/{}", name(&path));
        out.push(Decision { kind: Kind::Window, name, path, bytes, keep, why, used: None });
    }
    for (path, u, live) in &uses {
        if let (Some(u), false) = (u, live) {
            out.push(Decision {
                kind: Kind::Record,
                name: format!("apps/{}", name(path)),
                path: path.clone(),
                bytes: bytes(path),
                keep: false,
                why: format!("the record of {}, which is no longer there", u.exe),
                used: None,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Remove the runtime `d` under its lock (held: being installed, kept),
/// out of its name first in one step (a start never sees half a runtime;
/// a stage a crash leaves is cleared by the next install as dead).
/// `still`: re-checked under the lock, false keeps it.
fn remove_runtime(d: &Decision, still: &dyn Fn() -> bool) -> io::Result<bool> {
    let file = d.path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let lockfile = d.path.with_file_name(format!("{file}.lock"));
    crate::pidlock::with_lock(&lockfile, Duration::from_secs(1), &mut || true, |held: bool| {
        if !held || !still() {
            return Ok(false);
        }
        let stage = d.path.with_file_name(format!("{file}.incoming.{}", crate::pidlock::stage_tag()));
        std::fs::rename(&d.path, &stage)?;
        std::fs::remove_dir_all(&stage).map(|()| true)
    })
}

/// Remove what a plan offers; returns the paths removed, and each failure
/// as a line (a runtime being installed now is one).
pub fn apply(plan: &[Decision]) -> (Vec<PathBuf>, Vec<String>) {
    let (mut done, mut failed) = (vec![], vec![]);
    for d in plan.iter().filter(|d| !d.keep) {
        let r = match d.kind {
            Kind::Record => std::fs::remove_file(&d.path).map(|()| true),
            Kind::Runtime => remove_runtime(d, &|| true),
            Kind::Window => std::fs::remove_dir_all(&d.path).map(|()| true),
        };
        match r {
            Ok(true) => done.push(d.path.clone()),
            Ok(false) => failed.push(format!("{}: being installed right now", d.path.display())),
            Err(e) => failed.push(format!("{}: {e}", d.path.display())),
        }
    }
    (done, failed)
}

/// An app's start, at most once a day per store (aio's
/// `autoPruneSharedStore`): remove the runtimes the plan offers, except
/// `running`, that nothing started from in [`AUTO_AGE`], each re-checked
/// under its lock. Each removal and failure is said; returns the removed.
pub fn auto(root: &Path, running: &str, now: SystemTime) -> Vec<PathBuf> {
    let stamp = root.join(AUTO);
    let last = std::fs::read_to_string(&stamp).ok().and_then(|t| parse_iso(&t));
    // A stamp in the future (a clock set back) is as good as none.
    if last.and_then(|t| now.duration_since(t).ok()).is_some_and(|d| d < DAY) {
        return vec![];
    }
    let plan = match std::fs::write(&stamp, format!("{}\n", iso(now))).and_then(|()| plan(root, now)) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[aio] runtime: the shared store's clean-up failed ({e}); `ax prune` shows it");
            return vec![];
        }
    };
    let old = |p: &Path| last_used(p).is_some_and(|t| now.duration_since(t).unwrap_or_default() >= AUTO_AGE);
    let mut done = vec![];
    for d in plan.iter().filter(|d| d.kind == Kind::Runtime && !d.keep && d.name != running && old(&d.path)) {
        let still = || {
            old(&d.path)
                && !uses(root)
                    .is_ok_and(|u| u.iter().any(|(_, u, live)| u.as_ref().is_none_or(|u| *live && entry(u) == d.name)))
        };
        match remove_runtime(d, &still) {
            Ok(true) => {
                eprintln!(
                    "[aio] runtime: removed Electron {} from the shared store ({} MB): no installed app names it, and \
                     nothing started from it in {} days",
                    d.name,
                    d.bytes >> 20,
                    AUTO_AGE.as_secs() / 86_400
                );
                done.push(d.path.clone());
            }
            Ok(false) => {}
            Err(e) => eprintln!(
                "[aio] runtime: could not remove the unused Electron {} ({e}); the next day's start tries again",
                d.path.display()
            ),
        }
    }
    done
}

/// Does a running program start from inside `dir`? (Before a pre-0.1.0 build's store
/// moves: a folder an app runs from is not moved.) Windows refuses the
/// move of a running one itself.
pub fn in_use(dir: &Path) -> bool {
    let Ok(dir) = dir.canonicalize() else { return false };
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        std::fs::read_dir("/proc").into_iter().flatten().flatten().any(|e| {
            e.file_name().to_string_lossy().bytes().all(|b| b.is_ascii_digit())
                && std::fs::read_link(e.path().join("exe")).is_ok_and(|x| x.starts_with(&dir))
        })
    }
    #[cfg(target_os = "macos")]
    {
        let mut pids = vec![0i32; 8192];
        let size = i32::try_from(pids.len() * size_of::<i32>()).unwrap_or(i32::MAX);
        // SAFETY: `pids` is a buffer of `size` bytes the call fills.
        let n = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), size) };
        pids.iter().take(usize::try_from(n).unwrap_or(0)).any(|&pid| {
            let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
            // SAFETY: `buf` is a buffer of its length the call fills.
            let len = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
            len > 0 && Path::new(&*String::from_utf8_lossy(&buf[..len as usize])).starts_with(&dir)
        })
    }
    #[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
    {
        _ = dir;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime(root: &Path, v: &str, used: SystemTime) -> PathBuf {
        let d = root.join(format!("{v}-linux-x64"));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("electron"), "xx").unwrap();
        std::fs::write(d.join(USED), iso(used)).unwrap();
        d
    }

    fn by<'a>(p: &'a [Decision], name: &str) -> &'a Decision {
        p.iter().find(|d| d.name == name).unwrap_or_else(|| panic!("no {name}"))
    }

    fn rec(exe: &Path, v: &str) -> Use {
        Use { exe: exe.to_string_lossy().into(), version: v.into(), slug: "linux-x64".into(), ..Use::default() }
    }

    fn name_of(exe: &Path, v: &str) -> String {
        format!("apps/{}-{v}.json", key(&exe.to_string_lossy()))
    }

    // What aio's code writes (`recordSharedUse`, `sharedUseKey`, run with Deno):
    #[test]
    fn a_record_is_byte_for_byte_aios() {
        let at = parse_iso("2026-11-01T09:30:00.123Z").unwrap();
        let u = Use {
            exe: "/home/u/Apps/Notes.AppImage".into(),
            version: "44.9.3".into(),
            slug: "linux-x64".into(),
            ..Use::default()
        };
        assert_eq!(
            record_text(&u, at),
            "{\"exe\":\"/home/u/Apps/Notes.AppImage\",\"version\":\"44.9.3\",\"slug\":\"linux-x64\",\"at\":\"2026-11-01T09:30:00.123Z\"}\n"
        );
        let w = Use { window: Some("/s/windows/notes".into()), ..u.clone() };
        assert_eq!(
            record_text(&w, at),
            "{\"exe\":\"/home/u/Apps/Notes.AppImage\",\"version\":\"44.9.3\",\"slug\":\"linux-x64\",\"window\":\"/s/windows/notes\",\"at\":\"2026-11-01T09:30:00.123Z\"}\n"
        );
        let p = Use { exe: r"C:\Users\u\AppData\Local\Programs\notes\notés.exe".into(), pending: true, ..u };
        assert_eq!(
            record_text(&p, at),
            "{\"exe\":\"C:\\\\Users\\\\u\\\\AppData\\\\Local\\\\Programs\\\\notes\\\\notés.exe\",\"version\":\"44.9.3\",\"slug\":\"linux-x64\",\"pending\":true,\"at\":\"2026-11-01T09:30:00.123Z\"}\n"
        );
        assert_eq!(key("/home/u/Apps/Notes.AppImage"), "9c8ad6173f86d1aa");
        assert_eq!(key(r"C:\Users\u\AppData\Local\Programs\notes\notés.exe"), "c047bf9671b875e7");
        assert_eq!(iso(std::time::UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
        // The file aio names it by: its version without the `v`.
        let t = tempfile::tempdir().unwrap();
        record_at(t.path(), &Use { version: "v44.9.3".into(), ..p }, at).unwrap();
        assert!(t.path().join("apps/c047bf9671b875e7-44.9.3.json").is_file());
    }

    #[test]
    fn a_runtime_an_installed_app_names_stays_one_nobody_names_goes_and_a_gone_apps_record_goes() {
        // aio tests/electron-shared-runtime.test.ts "shared store prune".
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("store");
        let long_ago = SystemTime::now() - Duration::from_secs(400 * 86_400);
        runtime(&root, "1.0.0", long_ago);
        runtime(&root, "2.0.0", long_ago);
        let live = t.path().join("live-app");
        std::fs::write(&live, "").unwrap();
        record(&root, &rec(&live, "1.0.0")).unwrap();
        let gone = t.path().join("uninstalled");
        record(&root, &rec(&gone, "2.0.0")).unwrap();
        // Past the grace the start's stamps buy.
        let later = SystemTime::now() + Duration::from_secs(3600);
        let p = plan(&root, later).unwrap();
        assert!(by(&p, "1.0.0-linux-x64").keep && by(&p, "1.0.0-linux-x64").why.contains("live-app"));
        assert!(!by(&p, "2.0.0-linux-x64").keep);
        let r = by(&p, &name_of(&gone, "2.0.0"));
        assert!(!r.keep && r.why.contains("uninstalled"), "{}", r.why);
        // Inside the grace, one nobody names stays: its record may be coming.
        runtime(&root, "2.0.0", SystemTime::now());
        assert!(by(&plan(&root, SystemTime::now()).unwrap(), "2.0.0-linux-x64").keep);
        // A window an app is making right now stays; a killed start's stage doesn't.
        std::fs::create_dir_all(root.join("windows/com.x.app/.stage-1")).unwrap();
        let w = |at| by(&plan(&root, at).unwrap(), "windows/com.x.app").clone();
        assert!(w(SystemTime::now()).keep && w(SystemTime::now()).why.contains("right now"));
        assert!(!w(later).keep);
        // A window a live record names stays, by its exact path.
        let home = root.join("windows").join("live");
        std::fs::create_dir_all(&home).unwrap();
        let named = Use { window: Some(home.to_string_lossy().into()), ..rec(&live, "1.0.0") };
        record(&root, &named).unwrap();
        assert!(by(&plan(&root, later).unwrap(), "windows/live").keep);
        // One record that can't be read keeps every runtime, and is named.
        std::fs::write(root.join("apps/broken.json"), "x").unwrap();
        // Not a record at all: aio reads only `.json`.
        std::fs::write(root.join("apps/old@1.0.0-linux-x64"), "x").unwrap();
        let p = plan(&root, later).unwrap();
        assert!(p.iter().filter(|d| d.kind != Kind::Record).all(|d| d.keep));
        assert!(by(&p, "2.0.0-linux-x64").why.contains("apps/broken.json"));
        assert!(!by(&p, "2.0.0-linux-x64").why.contains("old@"));
        assert!(plan(&t.path().join("nope"), later).unwrap().is_empty(), "an absent store: an empty plan");
    }

    #[test]
    fn an_app_keeps_its_newest_two_records_and_one_pending() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let exe = root.join("app");
        std::fs::write(&exe, "").unwrap();
        let start = SystemTime::now() - Duration::from_secs(100);
        for (i, v) in ["1.0.0", "2.0.0", "3.0.0"].into_iter().enumerate() {
            record_at(root, &rec(&exe, v), start + Duration::from_secs(i as u64)).unwrap();
        }
        let other = root.join("other");
        record(root, &rec(&other, "1.0.0")).unwrap();
        // Pending: the newest one kept; it never demotes a started one.
        for (i, v) in ["4.0.0", "5.0.0", "3.0.0"].into_iter().enumerate() {
            record_at(root, &Use { pending: true, ..rec(&exe, v) }, start + Duration::from_secs(10 + i as u64))
                .unwrap();
        }
        let mut left: Vec<String> = std::fs::read_dir(root.join("apps"))
            .unwrap()
            .map(|e| format!("apps/{}", e.unwrap().file_name().into_string().unwrap()))
            .collect();
        left.sort();
        let mut want =
            vec![name_of(&exe, "2.0.0"), name_of(&exe, "3.0.0"), name_of(&exe, "5.0.0"), name_of(&other, "1.0.0")];
        want.sort();
        assert_eq!(left, want);
        let three = std::fs::read_to_string(root.join(name_of(&exe, "3.0.0"))).unwrap();
        assert!(!three.contains("pending"), "{three}");
    }

    #[test]
    fn the_auto_prune_removes_only_old_unnamed_unlocked_runtimes_not_the_running_one_once_a_day() {
        // aio tests/electron-shared-store-hygiene.test.ts "auto-prune".
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("store");
        let now = SystemTime::now();
        let old = now - AUTO_AGE - Duration::from_secs(5 * 86_400);
        let nobody = runtime(&root, "1.0.0", old);
        let uninstalled = runtime(&root, "2.0.0", old);
        let installed = runtime(&root, "3.0.0", old);
        let recent = runtime(&root, "4.0.0", now - AUTO_AGE + Duration::from_secs(3 * 86_400));
        let locked = runtime(&root, "5.0.0", old);
        let running = runtime(&root, "6.0.0", old);
        let live = t.path().join("live.AppImage");
        std::fs::write(&live, "").unwrap();
        record(&root, &rec(&t.path().join("deleted.AppImage"), "2.0.0")).unwrap();
        record(&root, &rec(&live, "3.0.0")).unwrap();
        // A stage someone is unpacking is no entry.
        let stage = root.join("44.0.0-linux-x64.incoming.7-0badcafe");
        std::fs::create_dir_all(&stage).unwrap();
        // 5.0.0 is being installed: its lock is held (here, by this process).
        let lock = root.join("5.0.0-linux-x64.lock");
        assert!(crate::pidlock::try_lock(&lock).0);
        let mut removed = auto(&root, "6.0.0-linux-x64", now);
        removed.sort();
        assert_eq!(removed, [nobody.clone(), uninstalled.clone()]);
        for kept in [&installed, &recent, &running, &stage] {
            assert!(kept.exists(), "{}", kept.display());
        }
        assert!(locked.exists());
        crate::pidlock::release(&lock).unwrap();
        let names: Vec<String> =
            std::fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        assert!(names.iter().filter(|n| n.contains(".incoming.")).count() == 1, "{names:?}");
        assert!(parse_iso(&std::fs::read_to_string(root.join(AUTO)).unwrap()).is_some());
        // Once a day: the same day nothing more, the next day the rest.
        assert!(auto(&root, "6.0.0-linux-x64", now).is_empty());
        let next = auto(&root, "6.0.0-linux-x64", now + DAY + Duration::from_secs(60));
        assert_eq!(next, [locked]);
        assert!(running.exists() && installed.exists());
    }

    #[test]
    fn an_unreadable_record_keeps_everything_and_a_broken_store_is_said_never_thrown() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("store");
        let now = SystemTime::now();
        let old = runtime(&root, "1.0.0", now - AUTO_AGE * 2);
        std::fs::create_dir_all(root.join("apps")).unwrap();
        std::fs::write(root.join("apps/broken.json"), "{not json").unwrap();
        assert!(auto(&root, "", now).is_empty());
        assert!(old.exists());
        let file = t.path().join("not-a-dir");
        std::fs::write(&file, "").unwrap();
        assert!(auto(&file, "", now).is_empty());
    }

    #[test]
    fn a_folder_this_test_runs_from_is_in_use() {
        let me = std::env::current_exe().unwrap();
        if cfg!(any(target_os = "linux", target_os = "macos")) {
            assert!(in_use(me.parent().unwrap()));
        }
        assert!(!in_use(&tempfile::tempdir().unwrap().path().join("x")));
    }
}

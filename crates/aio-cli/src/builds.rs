//! ax's shared build folder (`dev::shared_target`: one per toolchain under
//! aio's cache home): its size per toolchain, target and profile, what of
//! it `ax prune` offers (a profile unused for N days; the least recently
//! used past `ax setup --build-cap`), and room on its disk before a build.
//! A profile a cargo builds in right now (it holds cargo's lock) is never
//! removed.
use crate::places::{human, size};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Free on the build folder's disk under this: a build is warned.
pub const LOW: u64 = 5 << 30;
/// Under this: refused (cargo would die mid-build, "No space left on device").
pub const FULL: u64 = 1 << 30;
/// A profile used this recently stays under the cap (today's work).
const RECENT: Duration = Duration::from_secs(86_400);

/// One profile's folder of builds (`<toolchain>/[<target>/]<profile>`).
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub path: PathBuf,
    /// As under the store: `1.98.0/desktop-dev`, `1.98.0/wasm32-unknown-unknown/debug`.
    pub name: String,
    pub bytes: u64,
    pub used: SystemTime,
}

/// The store: every toolchain's build folder ax made (not the person's
/// own, `AIORT_TARGET_DIR`: ax prunes only what it owns).
pub fn store() -> PathBuf {
    aio::paths::cache_home().join("target")
}

fn dirs(d: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> =
        std::fs::read_dir(d).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    v.sort();
    v
}

/// When `d` was last built in: the newest mtime of it and what is in it.
fn used(d: &Path) -> SystemTime {
    let m = |p: &Path| std::fs::symlink_metadata(p).and_then(|m| m.modified()).ok();
    let inside = std::fs::read_dir(d).into_iter().flatten().flatten().filter_map(|e| m(&e.path()));
    m(d).into_iter().chain(inside).max().unwrap_or(SystemTime::UNIX_EPOCH)
}

/// Every profile folder of `store`: a folder holding cargo's `.cargo-lock`,
/// in a toolchain's folder or in one of its targets'.
pub fn parts(store: &Path) -> Vec<Part> {
    let part = |p: PathBuf| {
        let name = p.strip_prefix(store).unwrap_or(&p).to_string_lossy().replace('\\', "/");
        Part { bytes: size(&p), used: used(&p), name, path: p }
    };
    let mut out = vec![];
    for tc in dirs(store) {
        for d in dirs(&tc) {
            match d.join(".cargo-lock").is_file() {
                true => out.push(part(d)),
                false => out.extend(dirs(&d).into_iter().filter(|p| p.join(".cargo-lock").is_file()).map(part)),
            }
        }
    }
    out
}

/// What goes (pure): a part unused `days` or more, then, past `cap`, the
/// least recently used not used within a day. Each with why it goes or stays.
pub fn plan(parts: Vec<Part>, days: u64, cap: Option<u64>, now: SystemTime) -> Vec<(Part, Option<String>)> {
    let age = |p: &Part| now.duration_since(p.used).unwrap_or_default();
    let mut v: Vec<(Part, Option<String>)> = parts
        .into_iter()
        .map(|p| {
            let d = age(&p).as_secs() / 86_400;
            let why = (d >= days).then(|| format!("unused {d} days"));
            (p, why)
        })
        .collect();
    if let Some(cap) = cap {
        let mut kept: u64 = v.iter().filter(|(_, w)| w.is_none()).map(|(p, _)| p.bytes).sum();
        let mut order: Vec<usize> = (0..v.len()).filter(|&i| v[i].1.is_none() && age(&v[i].0) >= RECENT).collect();
        order.sort_by_key(|&i| v[i].0.used);
        for i in order {
            if kept <= cap {
                break;
            }
            kept -= v[i].0.bytes;
            v[i].1 = Some(format!("past the {} cap, least recently used", human(cap)));
        }
    }
    v
}

/// What the plan would free (pure).
pub fn freed(plan: &[(Part, Option<String>)]) -> u64 {
    plan.iter().filter(|(_, w)| w.is_some()).map(|(p, _)| p.bytes).sum()
}

/// The plan, said (pure).
pub fn show(store: &Path, plan: &[(Part, Option<String>)], now: SystemTime) -> String {
    let total: u64 = plan.iter().map(|(p, _)| p.bytes).sum();
    let mut out = vec![format!("builds in {} ({}):", store.display(), human(total))];
    out.extend(plan.iter().map(|(p, why)| {
        let days = now.duration_since(p.used).unwrap_or_default().as_secs() / 86_400;
        let used = match days {
            0 => "used today".to_string(),
            d => format!("last used {d} days ago"),
        };
        let (mark, why) = match why {
            Some(w) => ("remove", w.clone()),
            None => ("keep  ", used),
        };
        format!("{mark} {:<48} {:>9}  {why}", p.name, human(p.bytes))
    }));
    if plan.is_empty() {
        out.push("no builds yet".into());
    }
    out.join("\n")
}

/// Remove what the plan names, each only while no cargo builds in it (its
/// locks taken first; held while it is moved aside, then deleted). What was
/// removed and what wasn't, said.
pub fn apply(plan: &[(Part, Option<String>)]) -> (Vec<String>, Vec<String>) {
    let (mut done, mut failed) = (vec![], vec![]);
    for (p, _) in plan.iter().filter(|(_, w)| w.is_some()) {
        let locks: Vec<std::fs::File> = std::fs::read_dir(&p.path)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(".cargo") && e.path().is_file())
            .filter_map(|e| std::fs::OpenOptions::new().read(true).write(true).open(e.path()).ok())
            .collect();
        if locks.iter().any(|f| f.try_lock().is_err()) {
            failed.push(format!("{}: a build is running in it", p.name));
            continue;
        }
        let aside = p.path.with_file_name(format!(
            ".pruning-{}-{}",
            p.path.file_name().unwrap_or_default().to_string_lossy(),
            std::process::id()
        ));
        let r = std::fs::rename(&p.path, &aside);
        drop(locks);
        match r.and_then(|()| std::fs::remove_dir_all(&aside)) {
            Ok(()) => done.push(format!("{} ({})", p.name, human(p.bytes))),
            Err(e) => failed.push(format!("{}: {e}", p.name)),
        }
    }
    (done, failed)
}

/// `ax prune`'s builds: the plan of `store` (`--days`, the cap), said;
/// `--yes` (`-y`) removes what it names.
pub fn run(store: &Path, args: &[&str], cap: Option<u64>, now: SystemTime) -> Result<String, String> {
    let plan = plan(parts(store), crate::prune::days(args)?, cap, now);
    let said = show(store, &plan, now);
    let n = plan.iter().filter(|(_, w)| w.is_some()).count();
    if n == 0 {
        return Ok(format!("{said}\nno build to remove"));
    }
    if !args.iter().any(|a| *a == "--yes" || *a == "-y") {
        return Ok(format!(
            "{said}\n{n} build folder(s) ({}) would go: `ax prune --yes` removes them; each app's next build \
             there starts over",
            human(freed(&plan))
        ));
    }
    let (done, failed) = apply(&plan);
    let mut out = vec![said, format!("removed {}", done.join(", "))];
    out.extend(failed.into_iter().map(|f| format!("not removed: {f}")));
    Ok(out.join("\n"))
}

/// `ax setup --build-cap`'s file: the cap, as typed (`40G`).
pub fn cap_file() -> PathBuf {
    aio::paths::tool_home().join("build-cap")
}

/// A size as typed (pure): `40G`, `40GB`, `500M`, `1.5T`; whole bytes.
pub fn parse_size(s: &str) -> Result<u64, String> {
    let t = s.trim().to_ascii_uppercase();
    let t = t.strip_suffix('B').unwrap_or(&t);
    let (n, unit) = t.split_at(t.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(t.len()));
    let shift = match unit {
        "K" => 10,
        "M" => 20,
        "G" | "" => 30,
        "T" => 40,
        _ => return Err(format!("{s}: a size like 40G, 500M or 1T")),
    };
    match n.trim().parse::<f64>() {
        Ok(v) if v.is_finite() && v > 0.0 => Ok((v * (1u64 << shift) as f64) as u64),
        _ => Err(format!("{s}: a size like 40G, 500M or 1T")),
    }
}

/// The cap `ax setup --build-cap` set, if any.
pub fn cap() -> Option<u64> {
    parse_size(&std::fs::read_to_string(cap_file()).ok()?).ok()
}

/// `ax setup --build-cap <size|off>`: kept, said.
pub fn set_cap(v: &str) -> Result<String, String> {
    let f = cap_file();
    if v == "off" {
        _ = std::fs::remove_file(&f);
        return Ok("build cap: off (ax prune still offers builds unused 30 days)".into());
    }
    let b = parse_size(v)?;
    std::fs::create_dir_all(f.parent().unwrap_or(&f)).map_err(|e| e.to_string())?;
    std::fs::write(&f, v.trim()).map_err(|e| format!("{}: {e}", f.display()))?;
    Ok(format!(
        "build cap: {}: before each ax dev, start and build, the least recently used builds past it go \
         (never today's, never one building)",
        human(b)
    ))
}

/// Before a build (pure): `free` bytes on the disk of `dir`, and what
/// `ax prune --yes` (`days0`: `--days 0 --yes`) would free. A warning, or
/// the refusal, each saying the command.
pub fn room(free: Option<u64>, dir: &Path, prune: u64, days0: u64) -> Result<Option<String>, String> {
    let Some(free) = free.filter(|&f| f < LOW) else { return Ok(None) };
    let fix = match (prune, days0) {
        (0, 0) => format!(
            "nothing of ax's to prune: free space on that disk, or build elsewhere (AIORT_TARGET_DIR=<a folder on \
             another disk>); {} holds the builds",
            dir.display()
        ),
        (0, d) => format!("`ax prune --days 0 --yes` frees {} (every build not running: each starts over)", human(d)),
        (p, d) => format!(
            "`ax prune --yes` frees {} (old runtimes and builds unused 30 days; `--days 0`: {})",
            human(p),
            human(d)
        ),
    };
    let what = format!("{} free on the disk of {}", human(free), dir.display());
    match free < FULL {
        true => Err(format!("{what}: too little to build (cargo would stop with \"No space left on device\"); {fix}")),
        false => Ok(Some(format!("ax: {what}: a build may run out; {fix}"))),
    }
}

/// The nearest folder of `p` that is there (a build folder not made yet).
fn existing(p: &Path) -> &Path {
    p.ancestors().find(|a| a.exists()).unwrap_or(p)
}

/// Before `ax dev`, `start`, `build`: the cap applied (what went, said),
/// then room on the build folder's disk checked ([`room`]).
/// `runtimes(days)`: what `ax prune` frees of the runtime store.
pub fn before(runtimes: &dyn Fn(u64) -> u64) -> Result<(), String> {
    let (store, now) = (store(), SystemTime::now());
    if let Some(cap) = cap() {
        let all = parts(&store);
        if all.iter().map(|p| p.bytes).sum::<u64>() > cap {
            let plan = plan(all, u64::MAX, Some(cap), now);
            let (done, failed) = apply(&plan);
            if !done.is_empty() {
                eprintln!("ax: builds past the {} cap (ax setup --build-cap) removed: {}", human(cap), done.join(", "));
            }
            for f in failed {
                eprintln!("ax: build cap: not removed: {f}");
            }
        }
    }
    let dir = crate::dev::shared_target();
    let free = aio_shell::update::free_bytes(existing(&dir));
    if free.is_none_or(|f| f >= LOW) {
        return Ok(());
    }
    let all = parts(&store);
    let legacy = size(&crate::upgrade::legacy_target(&aio::paths::tool_home()));
    let prune = freed(&plan(all.clone(), crate::prune::DAYS, None, now)) + runtimes(crate::prune::DAYS) + legacy;
    let days0 = freed(&plan(all, 0, None, now)) + runtimes(0) + legacy;
    if let Some(w) = room(free, &dir, prune, days0)? {
        eprintln!("{w}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1 << 30;

    fn part(name: &str, gb: u64, days: u64, now: SystemTime) -> Part {
        let used = now - Duration::from_secs(days * 86_400 + 60);
        Part { path: PathBuf::from("/s").join(name), name: name.into(), bytes: gb * GB, used }
    }

    fn gone(plan: &[(Part, Option<String>)]) -> Vec<&str> {
        plan.iter().filter(|(_, w)| w.is_some()).map(|(p, _)| p.name.as_str()).collect()
    }

    #[test]
    fn old_profiles_go_and_past_the_cap_the_least_recently_used_but_never_todays() {
        let now = SystemTime::now();
        let all = vec![
            part("1.97.0/desktop-dev", 9, 40, now),
            part("1.98.0/release", 6, 10, now),
            part("1.98.0/x86_64-pc-windows-gnu/release", 3, 5, now),
            part("1.98.0/desktop-dev", 14, 0, now),
        ];
        assert_eq!(gone(&plan(all.clone(), 30, None, now)), ["1.97.0/desktop-dev"]);
        assert_eq!(gone(&plan(all.clone(), 0, None, now)).len(), 4, "--days 0: all");
        // Cap 20 GB: 23 kept after the age rule; the oldest goes first, until under it.
        let p = plan(all.clone(), 30, Some(20 * GB), now);
        assert_eq!(gone(&p), ["1.97.0/desktop-dev", "1.98.0/release"]);
        assert!(p[1].1.as_deref().unwrap().contains("past the 20.0 GB cap"), "{p:?}");
        assert_eq!(freed(&p), 15 * GB);
        // A cap under today's work: today's stays, said by what is left.
        assert_eq!(gone(&plan(all, u64::MAX, Some(GB), now)).len(), 3, "never today's");
        let said = show(Path::new("/s"), &p, now);
        assert!(said.contains("keep   1.98.0/desktop-dev") && said.contains("used today"), "{said}");
        assert!(said.contains("remove 1.97.0/desktop-dev") && said.contains("unused 40 days"), "{said}");
    }

    #[test]
    fn little_room_warns_none_refuses_each_with_the_command() {
        let d = Path::new("/c/target/1.98.0");
        assert_eq!(room(Some(6 * GB), d, 0, 0), Ok(None));
        assert_eq!(room(None, d, 0, 0), Ok(None), "the OS doesn't say: no guess");
        let w = room(Some(3 * GB), d, 12 * GB, 40 * GB).unwrap().unwrap();
        assert!(
            w.contains("3.0 GB free") && w.contains("`ax prune --yes` frees 12.0 GB") && w.contains("40.0 GB"),
            "{w}"
        );
        let e = room(Some(GB / 2), d, 0, 8 * GB).unwrap_err();
        assert!(e.contains("No space left on device") && e.contains("`ax prune --days 0 --yes` frees 8.0 GB"), "{e}");
        assert!(room(Some(0), d, 0, 0).unwrap_err().contains("AIORT_TARGET_DIR"));
    }

    #[test]
    fn sizes_parse_as_typed() {
        assert_eq!(parse_size("40G"), Ok(40 * GB));
        assert_eq!(parse_size("40gb"), Ok(40 * GB));
        assert_eq!(parse_size(" 1.5T "), Ok(1536 * GB));
        assert_eq!(parse_size("500M"), Ok(500 << 20));
        assert_eq!(parse_size("40"), Ok(40 * GB), "GB by default");
        for bad in ["", "G", "-1G", "0", "1X", "nan", "inf"] {
            assert!(parse_size(bad).is_err(), "{bad}");
        }
    }

    /// A fake store: profiles found at both depths, ax's own folder and
    /// strangers never; a profile with a cargo holding its lock stays.
    #[test]
    fn only_profiles_are_found_and_one_building_is_never_removed() {
        let t = tempfile::tempdir().unwrap();
        let s = t.path();
        let profile = |rel: &str| {
            let d = s.join(rel);
            std::fs::create_dir_all(d.join("deps")).unwrap();
            std::fs::write(d.join(".cargo-lock"), "").unwrap();
            std::fs::write(d.join("deps/libx.rlib"), vec![0u8; 1000]).unwrap();
            d
        };
        let (a, b, c) =
            (profile("1.98.0/desktop-dev"), profile("1.98.0/wasm32-unknown-unknown/debug"), profile("1.97.0/release"));
        std::fs::create_dir_all(s.join("1.98.0/aio/hotpatch")).unwrap();
        std::fs::write(s.join("1.98.0/aio/hotpatch/x"), "ax's").unwrap();
        let found = parts(s);
        let names: Vec<&str> = found.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["1.97.0/release", "1.98.0/desktop-dev", "1.98.0/wasm32-unknown-unknown/debug"]);
        assert!(found.iter().all(|p| p.bytes >= 1000));
        // A cargo builds in desktop-dev: its lock is held.
        let held = std::fs::File::open(a.join(".cargo-lock")).unwrap();
        held.lock().unwrap();
        let (done, failed) = apply(&plan(found, 0, None, SystemTime::now()));
        assert_eq!(failed, ["1.98.0/desktop-dev: a build is running in it"]);
        assert_eq!(done.len(), 2, "{done:?}");
        assert!(a.join("deps/libx.rlib").exists() && !b.exists() && !c.exists());
        assert!(s.join("1.98.0/aio/hotpatch/x").exists(), "ax's own folder is no profile");
        assert!(
            !std::fs::read_dir(s.join("1.98.0"))
                .unwrap()
                .flatten()
                .any(|e| e.file_name().to_string_lossy().starts_with(".pruning"))
        );
    }
}

//! `ax prune` (also spelled `ax runtime --prune`): what of the shared
//! store (Electron runtimes, ~250 MB each, macOS windows, app records) may
//! go. The store is the machine's, not one app's, so nothing is judged by
//! version order or platform. Two rules, both kept: what an installed app
//! names stays (`aio_shell::prune::plan`, the rule an app's start also
//! applies), and of what it offers a runtime still stays when something
//! started from it in the last N days, it is the one this ax ships, a
//! `--keep` one, one never stamped used, or a name aio didn't write. It
//! prints the plan; `--yes` deletes exactly that.
use aio_shell::prune::{Decision, Kind};
use std::path::Path;
use std::time::SystemTime;

type Out = Result<String, String>;

/// Unused this long (days) before a runtime is offered.
pub const DAYS: u64 = 30;
const DAY: u64 = 86_400;

/// The version of a runtime dir's name (`44.9.3-linux-x64`), if it is one.
fn version(name: &str) -> Option<&str> {
    let (v, slug) = name.split_once('-')?;
    let ok = v.split('.').count() == 3 && v.split('.').all(|p| p.parse::<u32>().is_ok()) && slug.contains('-');
    ok.then_some(v)
}

/// `d` under the command's own rules (pure): a runtime the store's plan
/// offers stays if any of them keeps it; `stamped` is whether a launch ever
/// stamped it used. Windows and records keep the plan's verdict.
pub fn judge(mut d: Decision, stamped: bool, keep: &[&str], days: u64, now: SystemTime) -> Decision {
    if d.keep || d.kind != Kind::Runtime {
        return d;
    }
    let age = d.used.map(|t| now.duration_since(t).unwrap_or_default().as_secs() / DAY);
    let stay = match version(&d.name) {
        None => Some("not a name ax writes: ax deletes only what it put here".to_string()),
        Some(v) if v == aio_shell::runtime::VERSION => {
            Some(format!("Electron {v} is the one this ax ships: every app built now runs on it"))
        }
        Some(v) if keep.contains(&v) => Some(format!("Electron {v} is kept (--keep)")),
        Some(_) if !stamped => Some("never recorded as used: unknown is not unused".into()),
        Some(_) => match age {
            Some(a) if a < days => Some(format!("{}, within {days} days", d.why)),
            _ => None,
        },
    };
    if let Some(why) = stay {
        (d.keep, d.why) = (true, why);
    }
    d
}

fn mb(b: u64) -> String {
    format!("{} MB", b.div_ceil(1 << 20))
}

/// The plan, said (pure).
pub fn show(v: &[Decision], applied: bool) -> String {
    let gone: Vec<&Decision> = v.iter().filter(|d| !d.keep).collect();
    let mut out: Vec<String> = v
        .iter()
        .map(|d| {
            let mark = if d.keep { "keep  " } else { "remove" };
            format!("{mark} {:<24} {:>8}  {}", d.name, mb(d.bytes), d.why)
        })
        .collect();
    let total = mb(gone.iter().map(|d| d.bytes).sum());
    out.push(String::new());
    out.push(match (gone.len(), applied) {
        (0, _) => "nothing to remove".into(),
        (n, true) => format!("removed {n} entr{} ({total})", if n == 1 { "y" } else { "ies" }),
        (n, false) => format!(
            "{n} entr{} ({total}) would go. Nothing has been deleted: `ax prune --yes` removes exactly these.\n\
             An app that runs on one of them must fetch it again, so it won't start OFFLINE until it has.",
            if n == 1 { "y" } else { "ies" }
        ),
    });
    out.join("\n")
}

/// `ax prune [--yes] [--days N] [--keep a,b]` over the store `root`.
pub fn run(root: &Path, args: &[&str], now: SystemTime) -> Out {
    let value = |f: &str| args.iter().position(|a| *a == f).map(|i| args.get(i + 1).copied());
    let days = match value("--days") {
        None => DAYS,
        Some(v) => v.and_then(|d| d.parse::<u64>().ok()).ok_or("--days takes a whole number of days, like --days 7")?,
    };
    let keep: Vec<&str> = value("--keep").flatten().map(|k| k.split(',').map(str::trim).collect()).unwrap_or_default();
    let yes = args.iter().any(|a| *a == "--yes" || *a == "-y");
    let plan = aio_shell::prune::plan(root, now).map_err(|e| format!("{}: {e}", root.display()))?;
    let plan: Vec<Decision> = plan
        .into_iter()
        .map(|d| {
            let stamped = d.path.join(aio_shell::runtime::USED).exists();
            judge(d, stamped, &keep, days, now)
        })
        .collect();
    if !yes {
        return Ok(show(&plan, false));
    }
    let (_, failed) = aio_shell::prune::apply(&plan);
    if failed.is_empty() {
        Ok(show(&plan, true))
    } else {
        Err(format!("not removed: {}", failed.join("\nnot removed: ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    fn days(n: u64) -> Duration {
        Duration::from_secs(n * DAY)
    }

    fn at() -> SystemTime {
        SystemTime::now() + days(1)
    }

    /// A runtime in `root` last stamped `ago` days before [`at`] (`None`: never).
    fn runtime(root: &Path, name: &str, ago: Option<u64>) -> PathBuf {
        let d = root.join(name);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("electron"), "x").unwrap();
        if let Some(a) = ago {
            std::fs::write(d.join(aio_shell::runtime::USED), aio_shell::prune::iso(at() - days(a))).unwrap();
        }
        d
    }

    fn removed(root: &Path, args: &[&str]) -> Vec<String> {
        let said = run(root, args, at()).unwrap();
        said.lines()
            .filter_map(|l| l.strip_prefix("remove "))
            .map(|l| l.split_whitespace().next().unwrap().into())
            .collect()
    }

    #[test]
    fn the_runtime_this_aio_ships_is_never_offered() {
        let t = tempfile::tempdir().unwrap();
        for os in ["linux-x64", "win32-x64"] {
            runtime(t.path(), &format!("{}-{os}", aio_shell::runtime::VERSION), Some(4000));
        }
        assert!(removed(t.path(), &["--days", "0"]).is_empty());
        assert!(run(t.path(), &[], at()).unwrap().contains("this ax ships"));
    }

    #[test]
    fn use_decides_and_unknown_is_not_unused() {
        let t = tempfile::tempdir().unwrap();
        runtime(t.path(), "43.4.1-linux-x64", Some(90));
        runtime(t.path(), "43.4.2-linux-x64", Some(29));
        runtime(t.path(), "43.4.3-linux-x64", None);
        runtime(t.path(), "41.2.1-linux-x64", Some(30));
        assert_eq!(removed(t.path(), &[]), ["41.2.1-linux-x64", "43.4.1-linux-x64"], "30 days is old enough, 29 not");
        let said = run(t.path(), &[], at()).unwrap();
        assert!(said.contains("unknown is not unused") && said.contains("within 30 days"), "{said}");
        // --keep protects what an app is pinned to; --days moves the line.
        assert_eq!(removed(t.path(), &["--keep", "43.4.1, 9.9.9"]), ["41.2.1-linux-x64"]);
        assert_eq!(removed(t.path(), &["--days", "100"]), Vec::<String>::new());
    }

    #[test]
    fn what_an_installed_app_names_stays_however_old_and_a_gone_apps_record_goes() {
        // Both rules in one command: an installed app's runtime stays at any age.
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("store");
        runtime(&root, "1.0.0-linux-x64", Some(400));
        runtime(&root, "2.0.0-linux-x64", Some(400));
        let live = t.path().join("live-app");
        std::fs::write(&live, "").unwrap();
        let gone = t.path().join("uninstalled");
        for (exe, v) in [(&live, "1.0.0"), (&gone, "2.0.0")] {
            let u = aio_shell::prune::Use {
                exe: exe.to_string_lossy().into(),
                version: v.into(),
                slug: "linux-x64".into(),
                ..Default::default()
            };
            aio_shell::prune::record(&root, &u).unwrap();
        }
        let record = format!("apps/{}-2.0.0.json", aio_shell::prune::key(&gone.to_string_lossy()));
        let said = run(&root, &["--days", "0"], at()).unwrap();
        assert!(said.contains("keep   1.0.0-linux-x64") && said.contains("live-app"), "{said}");
        assert_eq!(removed(&root, &[]), ["2.0.0-linux-x64".to_string(), record]);
    }

    #[test]
    fn downloads_and_strangers_are_never_touched() {
        let t = tempfile::tempdir().unwrap();
        runtime(t.path(), ".electron-v43.4.1-linux-x64.zip.partial-77", Some(900));
        runtime(t.path(), "notes", Some(900));
        let said = run(t.path(), &["--days", "0", "--yes"], at()).unwrap();
        assert!(said.contains("only what it put here") && said.contains("nothing to remove"), "{said}");
        assert!(
            t.path().join(".electron-v43.4.1-linux-x64.zip.partial-77").exists() && t.path().join("notes").exists()
        );
    }

    #[test]
    fn it_reports_first_and_deletes_only_what_it_named_with_yes() {
        let t = tempfile::tempdir().unwrap();
        let old = runtime(t.path(), "43.4.1-linux-x64", Some(90));
        let never = runtime(t.path(), "43.4.2-linux-x64", None);
        let said = run(t.path(), &[], at()).unwrap();
        assert!(said.contains("remove 43.4.1-linux-x64") && said.contains("last used 90 days ago"), "{said}");
        assert!(
            said.contains("Nothing has been deleted") && said.contains("ax prune --yes") && said.contains("OFFLINE")
        );
        assert!(old.exists());
        let said = run(t.path(), &["-y"], at()).unwrap();
        assert!(said.contains("removed 1 entry"), "{said}");
        assert!(!old.exists() && never.exists());
        assert!(run(&t.path().join("none"), &[], at()).unwrap().contains("nothing to remove"));
    }

    #[test]
    fn a_number_it_cant_read_is_refused() {
        let t = tempfile::tempdir().unwrap();
        for bad in [&["--days", "x"][..], &["--days", "-1"], &["--days", "1.5"], &["--days", ""], &["--days"]] {
            assert!(run(t.path(), bad, at()).unwrap_err().contains("--days"), "{bad:?}");
        }
        assert!(run(t.path(), &["--days", "0"], at()).unwrap().contains("nothing to remove"));
    }
}
